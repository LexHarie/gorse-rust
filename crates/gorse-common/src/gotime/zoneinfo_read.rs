// Ports: $GOROOT/src/time/zoneinfo_read.go, zoneinfo_unix.go, zoneinfo_goroot.go,
// sys_unix.go (file access) and zoneinfo.go (LoadLocation) @ go1.27.0 (the
// toolchain add21b5 builds with)
//
// Derived from Go's time package: Copyright 2009 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Loading time zones: the TZif parser (`LoadLocationFromTZData`), the zone
//! file sources (directories, Go's uncompressed `zoneinfo.zip`), Go's
//! `LoadLocation` search order, and `Local` from `$TZ`.
//!
//! Go reads files through raw system calls and reports their failures as
//! `syscall.Errno`; the errors here carry the errno and print Go's text for
//! it. Go's `time/tzdata` embedding is not ported: Gorse does not import it.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError, RwLock};

use super::errno::ERRNO_TEXT;
use super::zoneinfo::{
    ALPHA, Location, LocationData, OMEGA, Zone, ZoneTrans, find_zone, local, tzset, utc,
};

const ENOENT: i32 = 2;
const EINVAL: i32 = 22;

/// An error loading a time zone, with Go's text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocationError {
    /// Go's `errLocation`: the name contains ".." or starts with a slash.
    #[error("time: invalid location name")]
    InvalidName,
    /// Go's `errBadData`: the TZif data is malformed.
    #[error("malformed time zone information")]
    BadData,
    /// A failed system call, as Go's `syscall.Errno` (2 is ENOENT).
    #[error("{}", errno_text(*.0))]
    Errno(i32),
    /// Go's `fileSizeError`: a zone file over 10 MiB.
    #[error("time: file {0} is too large")]
    FileTooLarge(String),
    /// Any other error, such as "unknown time zone X" or "corrupt zip file
    /// X".
    #[error("{0}")]
    Other(String),
}

/// `syscall.Errno.Error` on linux/amd64.
fn errno_text(errno: i32) -> String {
    match ERRNO_TEXT.binary_search_by_key(&errno, |&(e, _)| e) {
        Ok(i) => ERRNO_TEXT[i].1.to_owned(),
        Err(_) => format!("errno {errno}"),
    }
}

/// The errno Go's raw system call would have reported. An error without one
/// is a path std rejected before the call (an interior NUL), which Go's
/// syscall package rejects with EINVAL.
fn errno_of(e: &io::Error) -> LocationError {
    LocationError::Errno(e.raw_os_error().unwrap_or(EINVAL))
}

// The max permitted size of files read by read_file. As reference, the
// zoneinfo.zip distributed by Go is ~350 KB, so 10MB is overkill.
const MAX_FILE_SIZE: usize = 10 << 20;

/// Simple I/O interface to a binary blob of data.
struct DataIo<'a> {
    p: &'a [u8],
    error: bool,
}

impl<'a> DataIo<'a> {
    fn read(&mut self, n: usize) -> &'a [u8] {
        if self.p.len() < n {
            self.p = &[];
            self.error = true;
            return &[];
        }
        let (p, rest) = self.p.split_at(n);
        self.p = rest;
        p
    }

    fn big4(&mut self) -> Option<u32> {
        let p = self.read(4);
        if p.len() < 4 {
            self.error = true;
            return None;
        }
        Some(u32::from_be_bytes([p[0], p[1], p[2], p[3]]))
    }

    fn big8(&mut self) -> Option<u64> {
        let n1 = self.big4();
        let n2 = self.big4();
        match (n1, n2) {
            (Some(n1), Some(n2)) => Some(u64::from(n1) << 32 | u64::from(n2)),
            _ => {
                self.error = true;
                None
            }
        }
    }

    fn byte(&mut self) -> Option<u8> {
        let p = self.read(1);
        if p.is_empty() {
            self.error = true;
            return None;
        }
        Some(p[0])
    }

    /// The rest of the data in the buffer.
    fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.p)
    }
}

/// A string up to the first NUL. Go keeps the raw bytes; TZif abbreviations
/// are ASCII, and any invalid UTF-8 becomes U+FFFD here.
fn byte_string(p: &[u8]) -> String {
    let p = match p.iter().position(|&b| b == 0) {
        Some(i) => &p[..i],
        None => p,
    };
    String::from_utf8_lossy(p).into_owned()
}

/// Go's `LoadLocationFromTZData`: a location named `name` from IANA TZif
/// data (for example the content of /etc/localtime).
pub fn load_location_from_tz_data(name: &str, data: &[u8]) -> Result<Location, LocationError> {
    let mut d = DataIo {
        p: data,
        error: false,
    };

    // 4-byte magic "TZif"
    if d.read(4) != b"TZif" {
        return Err(LocationError::BadData);
    }

    // 1-byte version, then 15 bytes of padding
    let p = d.read(16);
    if p.len() != 16 {
        return Err(LocationError::BadData);
    }
    let version = match p[0] {
        0 => 1,
        b'2' => 2,
        b'3' => 3,
        _ => return Err(LocationError::BadData),
    };

    // six big-endian 32-bit integers:
    //	number of UTC/local indicators
    //	number of standard/wall indicators
    //	number of leap seconds
    //	number of transition times
    //	number of local time zones
    //	number of characters of time zone abbrev strings
    const NUTC_LOCAL: usize = 0;
    const NSTD_WALL: usize = 1;
    const NLEAP: usize = 2;
    const NTIME: usize = 3;
    const NZONE: usize = 4;
    const NCHAR: usize = 5;
    // (Go also rejects counts that do not fit an int, which on 64-bit
    // platforms none fail.)
    let mut n = [0usize; 6];
    for v in &mut n {
        *v = d.big4().ok_or(LocationError::BadData)? as usize;
    }

    // If we have version 2 or 3, then the data is first written out in a
    // 32-bit format, then written out again in a 64-bit format. Skip the
    // 32-bit format and read the 64-bit one, as it can describe a broader
    // range of dates.
    let mut is64 = false;
    if version > 1 {
        // Skip the 32-bit data.
        let mut skip = n[NTIME] * 4
            + n[NTIME]
            + n[NZONE] * 6
            + n[NCHAR]
            + n[NLEAP] * 8
            + n[NSTD_WALL]
            + n[NUTC_LOCAL];
        // Skip the version 2 header that we just read.
        skip += 4 + 16;
        d.read(skip);

        is64 = true;

        // Read the counts again, they can differ.
        for v in &mut n {
            *v = d.big4().ok_or(LocationError::BadData)? as usize;
        }
    }

    let size = if is64 { 8 } else { 4 };

    // Transition times.
    let mut txtimes = DataIo {
        p: d.read(n[NTIME] * size),
        error: false,
    };

    // Time zone indices for transition times.
    let txzones = d.read(n[NTIME]);

    // Zone info structures
    let mut zonedata = DataIo {
        p: d.read(n[NZONE] * 6),
        error: false,
    };

    // Time zone abbreviations.
    let abbrev = d.read(n[NCHAR]);

    // Leap-second time pairs
    d.read(n[NLEAP] * (size + 4));

    // Whether tx times associated with local time types are specified as
    // standard time or wall time.
    let isstd = d.read(n[NSTD_WALL]);

    // Whether tx times associated with local time types are specified as UTC
    // or local time.
    let isutc = d.read(n[NUTC_LOCAL]);

    if d.error {
        // ran out of data
        return Err(LocationError::BadData);
    }

    let mut extend = String::new();
    let rest = d.rest();
    if rest.len() > 2 && rest[0] == b'\n' && rest[rest.len() - 1] == b'\n' {
        // Go keeps the raw bytes; a POSIX TZ string is ASCII.
        extend = String::from_utf8_lossy(&rest[1..rest.len() - 1]).into_owned();
    }

    // Now we can build up a useful data structure. First the zone
    // information.
    //	utcoff[4] isdst[1] nameindex[1]
    let nzone = n[NZONE];
    if nzone == 0 {
        // Reject tzdata files with no zones. There's nothing useful in them.
        // This also avoids a panic later when we add and then use a fake
        // transition (golang.org/issue/29437).
        return Err(LocationError::BadData);
    }
    let mut zones = Vec::with_capacity(nzone);
    for _ in 0..nzone {
        let n = zonedata.big4().ok_or(LocationError::BadData)?;
        let offset = i64::from(n as i32);
        let is_dst = zonedata.byte().ok_or(LocationError::BadData)? != 0;
        let b = zonedata.byte().ok_or(LocationError::BadData)?;
        if usize::from(b) >= abbrev.len() {
            return Err(LocationError::BadData);
        }
        zones.push(Zone {
            name: byte_string(&abbrev[usize::from(b)..]),
            offset,
            is_dst,
        });
    }

    // Now the transition time info.
    let mut tx = Vec::with_capacity(n[NTIME]);
    for (i, &index) in txzones.iter().enumerate() {
        let when = if !is64 {
            i64::from(txtimes.big4().ok_or(LocationError::BadData)? as i32)
        } else {
            txtimes.big8().ok_or(LocationError::BadData)? as i64
        };
        if usize::from(index) >= zones.len() {
            return Err(LocationError::BadData);
        }
        tx.push(ZoneTrans {
            when,
            index,
            isstd: isstd.get(i).is_some_and(|&b| b != 0),
            isutc: isutc.get(i).is_some_and(|&b| b != 0),
        });
    }

    if tx.is_empty() {
        // Build fake transition to cover all time. This happens in fixed
        // locations like "Etc/GMT0".
        tx.push(ZoneTrans {
            when: ALPHA,
            index: 0,
            isstd: false,
            isutc: false,
        });
    }

    // Committed to succeed.
    let mut l = LocationData {
        name: name.to_owned(),
        zone: zones,
        tx,
        extend,
        cache_start: 0,
        cache_end: 0,
        cache_zone: None,
    };

    // Fill in the cache with information about right now, since that will be
    // the most common lookup.
    let (sec, _, _) = super::runtime_now();
    for i in 0..l.tx.len() {
        if l.tx[i].when <= sec && (i + 1 == l.tx.len() || sec < l.tx[i + 1].when) {
            l.cache_start = l.tx[i].when;
            l.cache_end = OMEGA;
            l.cache_zone = Some(l.zone[usize::from(l.tx[i].index)].clone());
            if i + 1 < l.tx.len() {
                l.cache_end = l.tx[i + 1].when;
            } else if !l.extend.is_empty() {
                // If we're at the end of the known zone transitions, try the
                // extend string.
                let (name, offset, estart, eend, is_dst, ok) = tzset(&l.extend, l.cache_start, sec);
                if ok {
                    let zone = match find_zone(&l.zone, name, offset, is_dst) {
                        Some(i) => l.zone[i].clone(),
                        None => Zone {
                            name: name.to_owned(),
                            offset,
                            is_dst,
                        },
                    };
                    l.cache_start = estart;
                    l.cache_end = eend;
                    l.cache_zone = Some(zone);
                }
            }
            break;
        }
    }

    Ok(Location::from_data(l))
}

/// The contents of the file `name` in `dir`, which is a directory or an
/// uncompressed zip file.
fn load_tzinfo_from_dir_or_zip(dir: &str, name: &str) -> Result<Vec<u8>, LocationError> {
    if dir.len() > 4 && dir.ends_with(".zip") {
        return load_tzinfo_from_zip(dir, name);
    }
    if dir.is_empty() {
        return read_file(name);
    }
    read_file(&format!("{dir}/{name}"))
}

/// The little-endian 32-bit value in b, 0 if b is short.
fn get4(b: &[u8]) -> usize {
    if b.len() < 4 {
        return 0;
    }
    usize::from(b[0]) | usize::from(b[1]) << 8 | usize::from(b[2]) << 16 | usize::from(b[3]) << 24
}

/// The little-endian 16-bit value in b, 0 if b is short.
fn get2(b: &[u8]) -> usize {
    if b.len() < 2 {
        return 0;
    }
    usize::from(b[0]) | usize::from(b[1]) << 8
}

/// The contents of the file `name` in the uncompressed zip file `zipfile`.
fn load_tzinfo_from_zip(zipfile: &str, name: &str) -> Result<Vec<u8>, LocationError> {
    let mut fd = open(zipfile)?;
    let corrupt = || LocationError::Other(format!("corrupt zip file {zipfile}"));

    const ZECHEADER: usize = 0x0605_4b50;
    const ZCHEADER: usize = 0x0201_4b50;
    const ZTAILSIZE: usize = 22;

    const ZHEADERSIZE: usize = 30;
    const ZHEADER: usize = 0x0403_4b50;

    let mut buf = vec![0u8; ZTAILSIZE];
    if preadn(&mut fd, &mut buf, -(ZTAILSIZE as i64)).is_err() || get4(&buf) != ZECHEADER {
        return Err(corrupt());
    }
    let n = get2(&buf[10..]);
    let size = get4(&buf[12..]);
    let off = get4(&buf[16..]);

    let mut dir = vec![0u8; size];
    if preadn(&mut fd, &mut dir, off as i64).is_err() {
        return Err(corrupt());
    }

    let mut buf = &dir[..];
    for _ in 0..n {
        // zip entry layout:
        //	0	magic[4]
        //	4	madevers[1]
        //	5	madeos[1]
        //	6	extvers[1]
        //	7	extos[1]
        //	8	flags[2]
        //	10	meth[2]
        //	12	modtime[2]
        //	14	moddate[2]
        //	16	crc[4]
        //	20	csize[4]
        //	24	uncsize[4]
        //	28	namelen[2]
        //	30	xlen[2]
        //	32	fclen[2]
        //	34	disknum[2]
        //	36	iattr[2]
        //	38	eattr[4]
        //	42	off[4]
        //	46	name[namelen]
        //	46+namelen+xlen+fclen - next header
        if get4(buf) != ZCHEADER {
            break;
        }
        // Go slices the entry without checking its length and panics on a
        // truncated central directory; that is reported as corrupt here.
        if buf.len() < 46 {
            return Err(corrupt());
        }
        let meth = get2(&buf[10..]);
        let size = get4(&buf[24..]);
        let namelen = get2(&buf[28..]);
        let xlen = get2(&buf[30..]);
        let fclen = get2(&buf[32..]);
        let off = get4(&buf[42..]);
        let (Some(zname), Some(next)) = (
            buf.get(46..46 + namelen),
            buf.get(46 + namelen + xlen + fclen..),
        ) else {
            return Err(corrupt());
        };
        buf = next;
        if zname != name.as_bytes() {
            continue;
        }
        if meth != 0 {
            return Err(LocationError::Other(format!(
                "unsupported compression for {name} in {zipfile}"
            )));
        }

        // zip per-file header layout:
        //	0	magic[4]
        //	4	extvers[1]
        //	5	extos[1]
        //	6	flags[2]
        //	8	meth[2]
        //	10	modtime[2]
        //	12	moddate[2]
        //	14	crc[4]
        //	18	csize[4]
        //	22	uncsize[4]
        //	26	namelen[2]
        //	28	xlen[2]
        //	30	name[namelen]
        //	30+namelen+xlen - file data
        let mut hdr = vec![0u8; ZHEADERSIZE + namelen];
        if preadn(&mut fd, &mut hdr, off as i64).is_err()
            || get4(&hdr) != ZHEADER
            || get2(&hdr[8..]) != meth
            || get2(&hdr[26..]) != namelen
            || &hdr[30..30 + namelen] != name.as_bytes()
        {
            return Err(corrupt());
        }
        let xlen = get2(&hdr[28..]);

        let mut data = vec![0u8; size];
        if preadn(&mut fd, &mut data, (off + 30 + namelen + xlen) as i64).is_err() {
            return Err(corrupt());
        }

        return Ok(data);
    }

    Err(LocationError::Errno(ENOENT))
}

/// Go's `loadTzinfo`: the zone file `name` from a source, which is a zone
/// directory or an uncompressed zip of one. (Go also reads Android "tzdata"
/// files here, through a hook only Android sets.)
pub fn load_tzinfo(name: &str, source: &str) -> Result<Vec<u8>, LocationError> {
    load_tzinfo_from_dir_or_zip(source, name)
}

/// Go's `loadLocation`: the location `name` from the first of `sources` that
/// has it, else from the GOROOT zoneinfo.zip; the first error other than a
/// missing file, else "unknown time zone NAME".
pub fn load_location_from_sources(name: &str, sources: &[&str]) -> Result<Location, LocationError> {
    let mut first_err: Option<LocationError> = None;
    let mut try_source = |source: &str| -> Option<Location> {
        let err = match load_tzinfo(name, source) {
            Ok(zone_data) => match load_location_from_tz_data(name, &zone_data) {
                Ok(z) => return Some(z),
                Err(e) => e,
            },
            Err(e) => e,
        };
        if first_err.is_none() && err != LocationError::Errno(ENOENT) {
            first_err = Some(err);
        }
        None
    };
    for source in sources {
        if let Some(z) = try_source(source) {
            return Ok(z);
        }
    }
    // (Go tries the time/tzdata package here when a program imports it.)
    if let Some(source) = goroot_zone_source(&runtime_goroot())
        && let Some(z) = try_source(&source)
    {
        return Ok(z);
    }
    match first_err {
        Some(err) => Err(err),
        None => Err(LocationError::Other(format!("unknown time zone {name}"))),
    }
}

/// Go's `readFile`: the contents of the file `name`, refusing files over 10
/// MiB.
pub fn read_file(name: &str) -> Result<Vec<u8>, LocationError> {
    let mut f = open(name)?;
    let mut buf = [0u8; 4096];
    let mut ret = Vec::new();
    loop {
        let n = f.read(&mut buf).map_err(|e| errno_of(&e))?;
        if n == 0 {
            return Ok(ret);
        }
        ret.extend_from_slice(&buf[..n]);
        if ret.len() > MAX_FILE_SIZE {
            return Err(LocationError::FileTooLarge(name.to_owned()));
        }
    }
}

fn open(name: &str) -> Result<File, LocationError> {
    File::open(name).map_err(|e| errno_of(&e))
}

/// Reads exactly buf.len() bytes at off (from the end if off < 0).
fn preadn(fd: &mut File, buf: &mut [u8], off: i64) -> Result<(), LocationError> {
    let pos = if off < 0 {
        SeekFrom::End(off)
    } else {
        SeekFrom::Start(off as u64)
    };
    fd.seek(pos).map_err(|e| errno_of(&e))?;
    let mut buf = buf;
    while !buf.is_empty() {
        match fd.read(buf) {
            Ok(0) => return Err(LocationError::Other("short read".into())),
            Ok(m) => buf = &mut buf[m..],
            Err(e) => return Err(errno_of(&e)),
        }
    }
    Ok(())
}

/// Go's `gorootZoneSource`: `$GOROOT/lib/time/zoneinfo.zip`, or None without
/// a GOROOT.
pub fn goroot_zone_source(goroot: &str) -> Option<String> {
    if goroot.is_empty() {
        return None;
    }
    Some(format!("{goroot}/lib/time/zoneinfo.zip"))
}

// Go's defaultGOROOT, which the Go toolchain bakes into every binary; a Rust
// binary has none unless set_default_goroot gives one.
static DEFAULT_GOROOT: RwLock<String> = RwLock::new(String::new());

/// Sets the GOROOT used when $GOROOT is unset (Go's build-time
/// `defaultGOROOT`); [`load_location`] falls back to its
/// `lib/time/zoneinfo.zip`.
pub fn set_default_goroot(goroot: &str) {
    *DEFAULT_GOROOT
        .write()
        .unwrap_or_else(PoisonError::into_inner) = goroot.to_owned();
}

/// Go's `runtime.GOROOT`: $GOROOT if set, else the default.
fn runtime_goroot() -> String {
    match std::env::var_os("GOROOT") {
        Some(s) if !s.is_empty() => s.to_string_lossy().into_owned(),
        _ => DEFAULT_GOROOT
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone(),
    }
}

// Many systems use /usr/share/zoneinfo, Solaris 2 has /usr/share/lib/zoneinfo,
// IRIX 6 has /usr/lib/locale/TZ, NixOS has /etc/zoneinfo.
const PLATFORM_ZONE_SOURCES: &[&str] = &[
    "/usr/share/zoneinfo/",
    "/usr/share/lib/zoneinfo/",
    "/usr/lib/locale/TZ/",
    "/etc/zoneinfo",
];

static PLATFORM_SOURCES_DISABLED: AtomicBool = AtomicBool::new(false);

fn platform_zone_sources() -> &'static [&'static str] {
    if PLATFORM_SOURCES_DISABLED.load(Ordering::SeqCst) {
        return &[];
    }
    PLATFORM_ZONE_SOURCES
}

/// Restores the platform zone sources when dropped.
#[must_use = "dropping the guard enables the platform sources again"]
pub struct PlatformSourcesGuard(bool);

impl Drop for PlatformSourcesGuard {
    fn drop(&mut self) {
        PLATFORM_SOURCES_DISABLED.store(self.0, Ordering::SeqCst);
    }
}

/// Stops [`load_location`] and `Local` from reading the system zone
/// directories until the guard drops (Go's `DisablePlatformSources`). For
/// tests.
pub fn disable_platform_sources() -> PlatformSourcesGuard {
    PlatformSourcesGuard(PLATFORM_SOURCES_DISABLED.swap(true, Ordering::SeqCst))
}

// $ZONEINFO, read once at the first LoadLocation.
static ZONEINFO: Mutex<Option<String>> = Mutex::new(None);

fn zoneinfo_env() -> String {
    ZONEINFO
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(|| {
            std::env::var_os("ZONEINFO")
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .clone()
}

/// The $ZONEINFO value read at the first [`load_location`], or None before
/// it (Go's `ZoneinfoForTesting`).
pub fn zoneinfo_for_testing() -> Option<String> {
    ZONEINFO
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Forgets the $ZONEINFO value, so the next [`load_location`] reads it again
/// (Go's `ResetZoneinfoForTesting`).
pub fn reset_zoneinfo_for_testing() {
    *ZONEINFO.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Reports whether s contains "..".
fn contains_dot_dot(s: &str) -> bool {
    s.contains("..")
}

/// Go's `time.LoadLocation`. "" and "UTC" give [`utc`], "Local" gives
/// [`local`]; any other name is an IANA zone ("America/New_York") read from,
/// in order: the directory or uncompressed zip named by $ZONEINFO, the system
/// zone directories, and `$GOROOT/lib/time/zoneinfo.zip`.
pub fn load_location(name: &str) -> Result<Location, LocationError> {
    if name.is_empty() || name == "UTC" {
        return Ok(utc());
    }
    if name == "Local" {
        return Ok(local());
    }
    if contains_dot_dot(name) || name.starts_with('/') || name.starts_with('\\') {
        // No valid IANA Time Zone name contains a single dot, much less dot
        // dot. Likewise, none begin with a slash.
        return Err(LocationError::InvalidName);
    }
    let zoneinfo = zoneinfo_env();
    let mut first_err = None;
    if !zoneinfo.is_empty() {
        match load_tzinfo_from_dir_or_zip(&zoneinfo, name) {
            Ok(zone_data) => {
                if let Ok(z) = load_location_from_tz_data(name, &zone_data) {
                    return Ok(z);
                }
                // Go assigns `firstErr = err` here, but that err is the read's
                // (nil), not the parse's, which its own if statement scopes:
                // malformed data under $ZONEINFO is not reported.
            }
            Err(err) if err != LocationError::Errno(ENOENT) => first_err = Some(err),
            Err(_) => {}
        }
    }
    match load_location_from_sources(name, platform_zone_sources()) {
        Ok(z) => Ok(z),
        Err(err) => Err(first_err.unwrap_or(err)),
    }
}

/// Go's `initLocal` on Unix: `Local` from $TZ. Unset means /etc/localtime
/// (named "Local"); "" or "UTC" means UTC; ":foo" is "foo"; an absolute path
/// is that file; anything else is a zone name searched in the system
/// directories. If loading fails, UTC.
pub(super) fn init_local() -> Location {
    match std::env::var_os("TZ") {
        None => {
            if let Ok(z) = load_location_from_sources("localtime", &["/etc"]) {
                return z.renamed("Local");
            }
        }
        Some(tz) if !tz.is_empty() => {
            let tz = tz.to_string_lossy();
            let tz = tz.strip_prefix(':').unwrap_or(&tz);
            if tz.starts_with('/') {
                if let Ok(z) = load_location_from_sources(tz, &[""]) {
                    return z.renamed(if tz == "/etc/localtime" { "Local" } else { tz });
                }
            } else if !tz.is_empty()
                && tz != "UTC"
                && let Ok(z) = load_location_from_sources(tz, platform_zone_sources())
            {
                return z;
            }
        }
        Some(_) => {}
    }

    // Fall back to UTC: an empty location named "UTC" (not `utc()` itself).
    Location::from_data(LocationData {
        name: "UTC".into(),
        zone: Vec::new(),
        tx: Vec::new(),
        extend: String::new(),
        cache_start: 0,
        cache_end: 0,
        cache_zone: None,
    })
}
