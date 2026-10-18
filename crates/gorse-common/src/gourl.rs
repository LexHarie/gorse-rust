// Ports: $GOROOT/src/net/url/url.go, encoding_table.go, gen_encoding_table.go (shouldEscape) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's net/url package: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `net/url`: [`parse`], [`Url::string`], query [`Values`] and the
//! escaping functions, with Go's exact results and error texts.
//!
//! Gorse parses storage URLs (`sqlite://…`, `qdrant://…`, blob URIs, log
//! sinks) and builds query strings with `net/url`; the strings it hands to
//! drivers and prints in errors come from these functions, so they are ported
//! rather than replaced by a WHATWG parser, which normalizes differently.
//!
//! Go strings are bytes and unescaping can produce invalid UTF-8, so the API
//! is byte based: inputs are `impl AsRef<[u8]>`, and URL fields, query keys
//! and values are `Vec<u8>`. Escaping always produces ASCII, so the escape
//! functions return `String`.
//!
//! Go reads two `GODEBUG` settings here, `urlstrictcolons` and
//! `urlmaxqueryparams`; they are read from the `GODEBUG` environment variable
//! the way Go's `internal/godebug` reads them, with go1.27 defaults (Gorse's
//! `go.mod` sets no `godebug` lines).
//!
//! Not ported, because Gorse does not call them: `URL.ResolveReference`,
//! `URL.Parse` (relative references), `JoinPath`, `URL.JoinPath`,
//! `URL.Clone` (Rust's `Clone` does it), `Values.Clone` (likewise),
//! `URL.MarshalBinary`, `AppendBinary`, `UnmarshalBinary`, and the
//! `Timeout`/`Temporary` forwarding of `*url.Error` (the errors here never
//! wrap network errors).

mod netip;

use std::collections::BTreeMap;
use std::fmt;

use crate::gostrconv::quote;

pub use netip::ParseAddrError;

/// Go's `*url.Error`: the operation, the URL and the error that caused it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{} {}: {err}", .op, quote(.url))]
pub struct Error {
    /// The operation, `"parse"` for [`parse`] and [`parse_request_uri`].
    pub op: &'static str,
    /// The URL (for a fragment error, the whole input; otherwise the part
    /// before `#`).
    pub url: Vec<u8>,
    /// What went wrong.
    pub err: ErrorKind,
}

/// The errors Go's `net/url` creates, each displaying Go's text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ErrorKind {
    /// `getScheme` found a `:` at the start.
    #[error("missing protocol scheme")]
    MissingProtocolScheme,
    /// An ASCII control byte (`< 0x20` or `0x7f`) in the URL.
    #[error("net/url: invalid control character in URL")]
    InvalidControlCharacter,
    /// [`parse_request_uri`] of an empty string.
    #[error("empty url")]
    EmptyUrl,
    /// [`parse_request_uri`] of a rootless path without a scheme.
    #[error("invalid URI for request")]
    InvalidUriForRequest,
    /// A relative URL whose first path segment has a colon.
    #[error("first path segment in URL cannot contain colon")]
    ColonInFirstSegment,
    /// The userinfo has a byte RFC 3986 does not allow.
    #[error("net/url: invalid userinfo")]
    InvalidUserinfo,
    /// A `[` that does not open the host, or brackets around an IPv4 address.
    #[error("invalid IP-literal")]
    InvalidIpLiteral,
    /// A `[` without a closing `]`.
    #[error("missing ']' in host")]
    MissingBracket,
    /// The text after the host is not `:` and digits.
    #[error("invalid port {} after host", quote(.0))]
    InvalidPort(Vec<u8>),
    /// A bracketed host that `netip.ParseAddr` rejects.
    #[error("invalid host: {0}")]
    InvalidHost(ParseAddrError),
    /// A malformed `%` escape.
    #[error(transparent)]
    Escape(#[from] EscapeError),
    /// A byte a host may not contain.
    #[error(transparent)]
    InvalidHostChar(#[from] InvalidHostError),
    /// [`parse_query`] found a `;` in a key-value pair.
    #[error("invalid semicolon separator in query")]
    SemicolonSeparator,
    /// [`parse_query`] found more parameters than the limit.
    #[error("number of URL query parameters exceeded limit")]
    TooManyQueryParameters,
}

/// Go's `url.EscapeError`: the malformed escape, at most three bytes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid URL escape {}", quote(.0))]
pub struct EscapeError(pub Vec<u8>);

/// Go's `url.InvalidHostError`: the byte a host may not contain.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid character {} in host name", quote(.0))]
pub struct InvalidHostError(pub Vec<u8>);

/// Go's `encoding`: which part of a URL a string is escaped for, as a bit in
/// Go's generated `table`. The modes are in [`internal`], for the test ports.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding(u8);

const ENCODE_PATH: Encoding = Encoding(1 << 0);
const ENCODE_PATH_SEGMENT: Encoding = Encoding(1 << 1);
const ENCODE_HOST: Encoding = Encoding(1 << 2);
const ENCODE_ZONE: Encoding = Encoding(1 << 3);
const ENCODE_USER_PASSWORD: Encoding = Encoding(1 << 4);
const ENCODE_QUERY_COMPONENT: Encoding = Encoding(1 << 5);
const ENCODE_FRAGMENT: Encoding = Encoding(1 << 6);
// hexChar is actually NOT an encoding mode, but there are only seven encoding
// modes. We might as well abuse the otherwise unused most significant bit in
// uint8 to indicate whether a character is hexadecimal.
const HEX_CHAR: u8 = 1 << 7;

const MODES: [Encoding; 7] = [
    ENCODE_PATH,
    ENCODE_PATH_SEGMENT,
    ENCODE_HOST,
    ENCODE_ZONE,
    ENCODE_USER_PASSWORD,
    ENCODE_QUERY_COMPONENT,
    ENCODE_FRAGMENT,
];

/// Go's generated `table`: for each byte, `hexChar` if it is a hex digit,
/// and the bit of every mode in which it is NOT escaped. Built at compile
/// time by the reference `shouldEscape` of `gen_encoding_table.go`, as
/// `go generate` builds Go's.
static TABLE: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let c = i as u8;
        let mut v = 0;
        if reference_ishex(c) {
            v |= HEX_CHAR;
        }
        let mut m = 0;
        while m < MODES.len() {
            if !reference_should_escape(c, MODES[m]) {
                v |= MODES[m].0;
            }
            m += 1;
        }
        table[i] = v;
        i += 1;
    }
    table
};

/// `gen_encoding_table.go`'s `ishex`.
const fn reference_ishex(c: u8) -> bool {
    c.is_ascii_digit() || matches!(c, b'a'..=b'f') || matches!(c, b'A'..=b'F')
}

/// `gen_encoding_table.go`'s `shouldEscape`, from which the table is
/// generated. Please be informed that for now shouldEscape does not check
/// all reserved characters correctly. See golang.org/issue/5684.
const fn reference_should_escape(c: u8, mode: Encoding) -> bool {
    // §2.3 Unreserved characters (alphanum)
    if c.is_ascii_alphanumeric() {
        return false;
    }

    if mode.0 == ENCODE_HOST.0 || mode.0 == ENCODE_ZONE.0 {
        // §3.2.2 Host allows
        //	sub-delims = "!" / "$" / "&" / "'" / "(" / ")" / "*" / "+" / "," / ";" / "="
        // as part of reg-name.
        // We add : because we include :port as part of host.
        // We add [ ] because we include [ipv6]:port as part of host.
        // We add < > because they're the only characters left that
        // we could possibly allow, and Parse will reject them if we
        // escape them (because hosts can't use %-encoding for
        // ASCII bytes).
        if matches!(
            c,
            b'!' | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'['
                | b']'
                | b'<'
                | b'>'
                | b'"'
        ) {
            return false;
        }
    }

    match c {
        b'-' | b'_' | b'.' | b'~' => return false, // §2.3 Unreserved characters (mark)
        b'$' | b'&' | b'+' | b',' | b'/' | b':' | b';' | b'=' | b'?' | b'@' => {
            // §2.2 Reserved characters (reserved)
            // Different sections of the URL allow a few of
            // the reserved characters to appear unescaped.
            if mode.0 == ENCODE_PATH.0 {
                // §3.3
                // The RFC allows : @ & = + $ but saves / ; , for assigning
                // meaning to individual path segments. This package
                // only manipulates the path as a whole, so we allow those
                // last three as well. That leaves only ? to escape.
                return c == b'?';
            } else if mode.0 == ENCODE_PATH_SEGMENT.0 {
                // §3.3
                // The RFC allows : @ & = + $ but saves / ; , for assigning
                // meaning to individual path segments.
                return c == b'/' || c == b';' || c == b',' || c == b'?';
            } else if mode.0 == ENCODE_USER_PASSWORD.0 {
                // §3.2.1
                // The RFC allows ';', ':', '&', '=', '+', '$', and ',' in
                // userinfo, so we must escape only '@', '/', and '?'.
                // The parsing of userinfo treats ':' as special so we must escape
                // that too.
                return c == b'@' || c == b'/' || c == b'?' || c == b':';
            } else if mode.0 == ENCODE_QUERY_COMPONENT.0 {
                // §3.4
                // The RFC reserves (so we must escape) everything.
                return true;
            } else if mode.0 == ENCODE_FRAGMENT.0 {
                // §4.1
                // The RFC text is silent but the grammar allows
                // everything, so escape nothing.
                return false;
            }
        }
        _ => {}
    }

    if mode.0 == ENCODE_FRAGMENT.0 {
        // RFC 3986 §2.2 allows not escaping sub-delims. A subset of sub-delims are
        // included in reserved from RFC 2396 §2.2. The remaining sub-delims do not
        // need to be escaped. To minimize potential breakage, we apply two restrictions:
        // (1) we always escape sub-delims outside of the fragment, and (2) we always
        // escape single quote to avoid breaking callers that had previously assumed that
        // single quotes would be escaped. See issue #19917.
        if matches!(c, b'!' | b'(' | b')' | b'*') {
            return false;
        }
    }

    // Everything else must be escaped.
    true
}

const UPPER_HEX: &[u8; 16] = b"0123456789ABCDEF";

fn ishex(c: u8) -> bool {
    TABLE[usize::from(c)] & HEX_CHAR != 0
}

// Precondition: ishex(c) is true.
fn unhex(c: u8) -> u8 {
    9 * (c >> 6) + (c & 15)
}

fn should_escape(c: u8, mode: Encoding) -> bool {
    TABLE[usize::from(c)] & mode.0 == 0
}

/// Go's `GODEBUG` setting `name` as `internal/godebug` resolves it: the last
/// `name=value` in the `GODEBUG` environment variable, cut at `#`, else the
/// binary's default, which for Gorse (a go1.27 module without `godebug`
/// lines) is empty for every setting `net/url` reads.
///
/// A bisect pattern after the `#` (`name=value#pattern`, a debugging aid
/// that makes Go return the value only for call stacks the pattern matches)
/// is not evaluated: the value applies everywhere.
pub(crate) fn godebug(name: &[u8]) -> Vec<u8> {
    let Some(env) = std::env::var_os("GODEBUG") else {
        return Vec::new();
    };
    let s = env.as_encoded_bytes();
    // Scan the string backward so that later settings are used and earlier
    // settings are ignored.
    let mut end = s.len();
    let mut eq: Option<usize> = None;
    let mut i = s.len();
    loop {
        // i is one past the index Go's loop variable holds, so i == 0 is
        // Go's i == -1.
        if i == 0 || s[i - 1] == b',' {
            if let Some(eq) = eq {
                let start = i;
                if &s[start..eq] == name {
                    let arg = &s[eq + 1..end];
                    let text = match arg.iter().position(|&b| b == b'#') {
                        Some(j) => &arg[..j],
                        None => arg,
                    };
                    return text.to_vec();
                }
            }
            eq = None;
            if i == 0 {
                break;
            }
            end = i - 1;
        } else if s[i - 1] == b'=' {
            eq = Some(i - 1);
        }
        i -= 1;
    }
    Vec::new()
}

/// Go's `url.QueryUnescape`: decodes each `%AB` into the byte 0xAB and `+`
/// into a space. Fails if any `%` is not followed by two hexadecimal digits.
pub fn query_unescape(s: impl AsRef<[u8]>) -> Result<Vec<u8>, EscapeError> {
    unescape(s.as_ref(), ENCODE_QUERY_COMPONENT).map_err(|e| match e {
        ErrorKind::Escape(e) => e,
        // Only the host and zone modes produce other errors.
        _ => unreachable!("query unescaping only fails with an EscapeError"),
    })
}

/// Go's `url.PathUnescape`: like [`query_unescape`] but leaves `+` alone.
pub fn path_unescape(s: impl AsRef<[u8]>) -> Result<Vec<u8>, EscapeError> {
    unescape(s.as_ref(), ENCODE_PATH_SEGMENT).map_err(|e| match e {
        ErrorKind::Escape(e) => e,
        _ => unreachable!("path unescaping only fails with an EscapeError"),
    })
}

/// Go's `unescape`: unescapes a string; the mode specifies which section of
/// the URL string is being unescaped.
fn unescape(s: &[u8], mode: Encoding) -> Result<Vec<u8>, ErrorKind> {
    // Count %, check that they're well-formed.
    let mut n = 0;
    let mut has_plus = false;
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'%' => {
                n += 1;
                if i + 2 >= s.len() || !ishex(s[i + 1]) || !ishex(s[i + 2]) {
                    let rest = &s[i..];
                    return Err(EscapeError(rest[..rest.len().min(3)].to_vec()).into());
                }
                // Per https://tools.ietf.org/html/rfc3986#page-21
                // in the host component %-encoding can only be used
                // for non-ASCII bytes.
                // But https://tools.ietf.org/html/rfc6874#section-2
                // introduces %25 being allowed to escape a percent sign
                // in IPv6 scoped-address literals. Yay.
                if mode == ENCODE_HOST && unhex(s[i + 1]) < 8 && &s[i..i + 3] != b"%25" {
                    return Err(EscapeError(s[i..i + 3].to_vec()).into());
                }
                if mode == ENCODE_ZONE {
                    // RFC 6874 says basically "anything goes" for zone identifiers
                    // and that even non-ASCII can be redundantly escaped,
                    // but it seems prudent to restrict %-escaped bytes here to those
                    // that are valid host name bytes in their unescaped form.
                    // That is, you can use escaping in the zone identifier but not
                    // to introduce bytes you couldn't just write directly.
                    // But Windows puts spaces here! Yay.
                    let v = unhex(s[i + 1]) << 4 | unhex(s[i + 2]);
                    if &s[i..i + 3] != b"%25" && v != b' ' && should_escape(v, ENCODE_HOST) {
                        return Err(EscapeError(s[i..i + 3].to_vec()).into());
                    }
                }
                i += 3;
            }
            b'+' => {
                has_plus = mode == ENCODE_QUERY_COMPONENT;
                i += 1;
            }
            c => {
                if (mode == ENCODE_HOST || mode == ENCODE_ZONE)
                    && c < 0x80
                    && should_escape(c, mode)
                {
                    return Err(InvalidHostError(vec![c]).into());
                }
                i += 1;
            }
        }
    }

    if n == 0 && !has_plus {
        return Ok(s.to_vec());
    }

    let unescaped_plus_sign = if mode == ENCODE_QUERY_COMPONENT {
        b' '
    } else {
        b'+'
    };
    let mut t = Vec::with_capacity(s.len() - 2 * n);
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'%' => {
                // In the loop above, we established that unhex's precondition
                // is fulfilled for both s[i+1] and s[i+2].
                t.push(unhex(s[i + 1]) << 4 | unhex(s[i + 2]));
                i += 2;
            }
            b'+' => t.push(unescaped_plus_sign),
            c => t.push(c),
        }
        i += 1;
    }
    Ok(t)
}

/// Go's `url.QueryEscape`: escapes s so it can be placed inside a URL query.
pub fn query_escape(s: impl AsRef<[u8]>) -> String {
    escape(s.as_ref(), ENCODE_QUERY_COMPONENT)
}

/// Go's `url.PathEscape`: escapes s so it can be placed inside a URL path
/// segment, replacing special characters (including `/`) with `%XX`.
pub fn path_escape(s: impl AsRef<[u8]>) -> String {
    escape(s.as_ref(), ENCODE_PATH_SEGMENT)
}

/// Go's `escape`. Every byte it leaves unescaped is ASCII.
fn escape(s: &[u8], mode: Encoding) -> String {
    let mut t = Vec::with_capacity(s.len());
    for &c in s {
        if c == b' ' && mode == ENCODE_QUERY_COMPONENT {
            t.push(b'+');
        } else if should_escape(c, mode) {
            t.push(b'%');
            t.push(UPPER_HEX[usize::from(c >> 4)]);
            t.push(UPPER_HEX[usize::from(c & 15)]);
        } else {
            t.push(c);
        }
    }
    String::from_utf8(t).expect("escape writes ASCII")
}

/// Go's `url.URL`, a parsed URL (technically, a URI reference):
/// `[scheme:][//[userinfo@]host][/]path[?query][#fragment]`, or
/// `scheme:opaque[?query][#fragment]` when no slash follows the scheme.
///
/// `path` and `fragment` are stored decoded; [`Url::escaped_path`] and
/// [`Url::escaped_fragment`] give the encoded forms [`Url::string`] uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Url {
    /// The scheme, lower-cased.
    pub scheme: Vec<u8>,
    /// Encoded opaque data.
    pub opaque: Vec<u8>,
    /// Username and password information.
    pub user: Option<Userinfo>,
    /// `host` or `host:port`.
    pub host: Vec<u8>,
    /// The path (relative paths may omit the leading slash), decoded.
    pub path: Vec<u8>,
    /// An optional encoded path hint; see [`Url::escaped_path`].
    pub raw_path: Vec<u8>,
    /// Whether the original URL had a `?` with an empty query.
    pub force_query: bool,
    /// The encoded query values, without the `?`.
    pub raw_query: Vec<u8>,
    /// The fragment, decoded, without the `#`.
    pub fragment: Vec<u8>,
    /// An optional encoded fragment hint; see [`Url::escaped_fragment`].
    pub raw_fragment: Vec<u8>,
    /// Whether the URL has an empty host (authority).
    pub omit_host: bool,
}

/// Go's `url.Userinfo`: a username and an optional password.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Userinfo {
    username: Vec<u8>,
    password: Vec<u8>,
    password_set: bool,
}

/// Go's `url.User`: a [`Userinfo`] with a username and no password.
pub fn user(username: impl AsRef<[u8]>) -> Userinfo {
    Userinfo {
        username: username.as_ref().to_vec(),
        password: Vec::new(),
        password_set: false,
    }
}

/// Go's `url.UserPassword`: a [`Userinfo`] with a username and a password.
pub fn user_password(username: impl AsRef<[u8]>, password: impl AsRef<[u8]>) -> Userinfo {
    Userinfo {
        username: username.as_ref().to_vec(),
        password: password.as_ref().to_vec(),
        password_set: true,
    }
}

impl Userinfo {
    /// Go's `Userinfo.Username`.
    pub fn username(&self) -> &[u8] {
        &self.username
    }

    /// Go's `Userinfo.Password`: the password and whether it is set.
    pub fn password(&self) -> (&[u8], bool) {
        (&self.password, self.password_set)
    }

    /// Go's `Userinfo.String`: `username[:password]`, escaped.
    pub fn string(&self) -> String {
        let mut s = escape(&self.username, ENCODE_USER_PASSWORD);
        if self.password_set {
            s.push(':');
            s.push_str(&escape(&self.password, ENCODE_USER_PASSWORD));
        }
        s
    }
}

/// Go's `getScheme`: if raw_url is of the form `scheme:path` (scheme
/// `[a-zA-Z][a-zA-Z0-9+.-]*`), returns scheme and path; else empty and
/// raw_url.
fn get_scheme(raw_url: &[u8]) -> Result<(&[u8], &[u8]), ErrorKind> {
    for (i, &c) in raw_url.iter().enumerate() {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' => {} // do nothing
            b'0'..=b'9' | b'+' | b'-' | b'.' => {
                if i == 0 {
                    return Ok((b"", raw_url));
                }
            }
            b':' => {
                if i == 0 {
                    return Err(ErrorKind::MissingProtocolScheme);
                }
                return Ok((&raw_url[..i], &raw_url[i + 1..]));
            }
            _ => {
                // we have encountered an invalid character,
                // so there is no valid scheme
                return Ok((b"", raw_url));
            }
        }
    }
    Ok((b"", raw_url))
}

fn cut(s: &[u8], sep: u8) -> (&[u8], &[u8], bool) {
    match s.iter().position(|&b| b == sep) {
        Some(i) => (&s[..i], &s[i + 1..], true),
        None => (s, b"", false),
    }
}

fn index_of(s: &[u8], sep: &[u8]) -> Option<usize> {
    s.windows(sep.len()).position(|w| w == sep)
}

fn last_index_of(s: &[u8], sep: u8) -> Option<usize> {
    s.iter().rposition(|&b| b == sep)
}

/// Go's `url.Parse`: parses a URL, absolute or relative.
pub fn parse(raw_url: impl AsRef<[u8]>) -> Result<Url, Error> {
    let raw_url = raw_url.as_ref();
    // Cut off #frag
    let (u, frag, _) = cut(raw_url, b'#');
    let mut url = parse_internal(u, false).map_err(|err| Error {
        op: "parse",
        url: u.to_vec(),
        err,
    })?;
    if frag.is_empty() {
        return Ok(url);
    }
    url.set_fragment(frag).map_err(|err| Error {
        op: "parse",
        url: raw_url.to_vec(),
        err,
    })?;
    Ok(url)
}

/// Go's `url.ParseRequestURI`: parses a URL received in an HTTP request,
/// which is only an absolute URI or an absolute path, without `#fragment`.
pub fn parse_request_uri(raw_url: impl AsRef<[u8]>) -> Result<Url, Error> {
    let raw_url = raw_url.as_ref();
    parse_internal(raw_url, true).map_err(|err| Error {
        op: "parse",
        url: raw_url.to_vec(),
        err,
    })
}

/// Go's `parse`. If via_request is true, the URL is assumed to have arrived
/// via an HTTP request, in which case only absolute URLs or path-absolute
/// relative URLs are allowed.
fn parse_internal(raw_url: &[u8], via_request: bool) -> Result<Url, ErrorKind> {
    if string_contains_ctl_byte(raw_url) {
        return Err(ErrorKind::InvalidControlCharacter);
    }

    if raw_url.is_empty() && via_request {
        return Err(ErrorKind::EmptyUrl);
    }
    let mut url = Url::default();

    if raw_url == b"*" {
        url.path = b"*".to_vec();
        return Ok(url);
    }

    // Split off possible leading "http:", "mailto:", etc.
    // Cannot contain escaped characters.
    let (scheme, mut rest) = get_scheme(raw_url)?;
    url.scheme = scheme.to_ascii_lowercase();

    if rest.ends_with(b"?") && rest.iter().filter(|&&b| b == b'?').count() == 1 {
        url.force_query = true;
        rest = &rest[..rest.len() - 1];
    } else {
        let (r, q, _) = cut(rest, b'?');
        rest = r;
        url.raw_query = q.to_vec();
    }

    if !rest.starts_with(b"/") {
        if !url.scheme.is_empty() {
            // We consider rootless paths per RFC 3986 as opaque.
            url.opaque = rest.to_vec();
            return Ok(url);
        }
        if via_request {
            return Err(ErrorKind::InvalidUriForRequest);
        }

        // Avoid confusion with malformed schemes, like cache_object:foo/bar.
        // See golang.org/issue/16822.
        //
        // RFC 3986, §3.3:
        // In addition, a URI reference (Section 4.1) may be a relative-path reference,
        // in which case the first path segment cannot contain a colon (":") character.
        let (segment, _, _) = cut(rest, b'/');
        if segment.contains(&b':') {
            // First path segment has colon. Not allowed in relative URL.
            return Err(ErrorKind::ColonInFirstSegment);
        }
    }

    if (!url.scheme.is_empty() || !via_request && !rest.starts_with(b"///"))
        && rest.starts_with(b"//")
    {
        let mut authority = &rest[2..];
        rest = b"";
        if let Some(i) = authority.iter().position(|&b| b == b'/') {
            rest = &authority[i..];
            authority = &authority[..i];
        }
        let (user, host) = parse_authority(&url.scheme, authority)?;
        url.user = user;
        url.host = host;
    } else if !url.scheme.is_empty() && rest.starts_with(b"/") {
        // OmitHost is set to true when rawURL has an empty host (authority).
        // See golang.org/issue/46059.
        url.omit_host = true;
    }

    // Set Path and, optionally, RawPath.
    // RawPath is a hint of the encoding of Path. We don't want to set it if
    // the default escaping of Path is equivalent, to help make sure that people
    // don't rely on it in general.
    url.set_path(rest)?;
    Ok(url)
}

/// Go's `parseAuthority`.
fn parse_authority(
    scheme: &[u8],
    authority: &[u8],
) -> Result<(Option<Userinfo>, Vec<u8>), ErrorKind> {
    let i = last_index_of(authority, b'@');
    let host = match i {
        None => parse_host(scheme, authority)?,
        Some(i) => parse_host(scheme, &authority[i + 1..])?,
    };
    let Some(i) = i else {
        return Ok((None, host));
    };
    let userinfo = &authority[..i];
    if !valid_userinfo(userinfo) {
        return Err(ErrorKind::InvalidUserinfo);
    }
    let user = if !userinfo.contains(&b':') {
        user(unescape(userinfo, ENCODE_USER_PASSWORD)?)
    } else {
        let (username, password, _) = cut(userinfo, b':');
        let username = unescape(username, ENCODE_USER_PASSWORD)?;
        let password = unescape(password, ENCODE_USER_PASSWORD)?;
        user_password(username, password)
    };
    Ok((Some(user), host))
}

/// Go's `parseHost`: parses host as an authority without user information,
/// that is, as `host[:port]`.
fn parse_host(scheme: &[u8], host: &[u8]) -> Result<Vec<u8>, ErrorKind> {
    let open_bracket_idx = last_index_of(host, b'[');
    if open_bracket_idx.is_some_and(|i| i > 0) {
        return Err(ErrorKind::InvalidIpLiteral);
    } else if open_bracket_idx == Some(0) {
        // Parse an IP-Literal in RFC 3986 and RFC 6874.
        // E.g., "[fe80::1]", "[fe80::1%25en0]", "[fe80::1]:80".
        let Some(close_bracket_idx) = last_index_of(host, b']') else {
            return Err(ErrorKind::MissingBracket);
        };

        let colon_port = &host[close_bracket_idx + 1..];
        if !valid_optional_port(colon_port) {
            return Err(ErrorKind::InvalidPort(colon_port.to_vec()));
        }
        let unescaped_colon_port = unescape(colon_port, ENCODE_HOST)?;

        // (Go slices host[openBracketIdx+1 : closeBracketIdx]; a "]" before
        // the "[" at index 0 is impossible, as the "[" is the first byte.)
        let hostname = &host[1..close_bracket_idx];
        // RFC 6874 defines that %25 (%-encoded percent) introduces
        // the zone identifier, and the zone identifier can use basically
        // any %-encoding it likes. That's different from the host, which
        // can only %-encode non-ASCII bytes.
        // We do impose some restrictions on the zone, to avoid stupidity
        // like newlines.
        let unescaped_hostname = match index_of(hostname, b"%25") {
            Some(zone_idx) => {
                let mut host_part = unescape(&hostname[..zone_idx], ENCODE_HOST)?;
                let zone_part = unescape(&hostname[zone_idx..], ENCODE_ZONE)?;
                host_part.extend_from_slice(&zone_part);
                host_part
            }
            None => unescape(hostname, ENCODE_HOST)?,
        };

        // Per RFC 3986, only a host identified by a valid
        // IPv6 address can be enclosed by square brackets.
        // This excludes any IPv4, but notably not IPv4-mapped addresses.
        let addr = netip::parse_addr(&unescaped_hostname).map_err(ErrorKind::InvalidHost)?;
        if addr.is4() {
            return Err(ErrorKind::InvalidIpLiteral);
        }
        let mut out = Vec::with_capacity(unescaped_hostname.len() + 2 + unescaped_colon_port.len());
        out.push(b'[');
        out.extend_from_slice(&unescaped_hostname);
        out.push(b']');
        out.extend_from_slice(&unescaped_colon_port);
        return Ok(out);
    } else if let Some(mut i) = host.iter().position(|&b| b == b':') {
        let last_colon = last_index_of(host, b':').unwrap_or(i);
        if last_colon != i {
            // RFC 3986 does not allow colons to appear in the host subcomponent.
            //
            // However, a number of databases including PostgreSQL and MongoDB
            // permit a comma-separated list of hosts (with optional ports) in the
            // host subcomponent.
            //
            // Since we historically permitted colons to appear in the host,
            // enforce strict colons only for http and https URLs.
            //
            // See https://go.dev/issue/75223 and https://go.dev/issue/78077.
            if scheme == b"http" || scheme == b"https" {
                if godebug(b"urlstrictcolons") == b"0" {
                    i = last_colon;
                }
            } else {
                i = last_colon;
            }
        }
        let colon_port = &host[i..];
        if !valid_optional_port(colon_port) {
            return Err(ErrorKind::InvalidPort(colon_port.to_vec()));
        }
    }

    unescape(host, ENCODE_HOST)
}

/// Go's `validEncoded`: whether s is a valid encoded path or fragment for
/// mode, with no byte that requires escaping during encoding.
fn valid_encoded(s: &[u8], mode: Encoding) -> bool {
    for &c in s {
        // RFC 3986, Appendix A.
        // pchar = unreserved / pct-encoded / sub-delims / ":" / "@".
        // shouldEscape is not quite compliant with the RFC,
        // so we check the sub-delims ourselves and let
        // shouldEscape handle the others.
        match c {
            b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':'
            | b'@' => {} // ok
            b'[' | b']' => {} // ok - not specified in RFC 3986 but left alone by modern browsers
            b'%' => {}        // ok - percent encoded, will decode
            _ => {
                if should_escape(c, mode) {
                    return false;
                }
            }
        }
    }
    true
}

/// Go's `validOptionalPort`: whether port is empty or matches `^:\d*$`.
fn valid_optional_port(port: &[u8]) -> bool {
    match port.split_first() {
        None => true,
        Some((&b':', digits)) => digits.iter().all(u8::is_ascii_digit),
        Some(_) => false,
    }
}

/// Go's `validUserinfo`: whether s is a valid userinfo string per RFC 3986
/// Section 3.2.1 (plus `@`, which Go allows; see go.dev/issue/3439). It
/// doesn't validate pct-encoded; unescape does that.
fn valid_userinfo(s: &[u8]) -> bool {
    // (Go ranges over runes; a non-ASCII rune, like an invalid byte, is
    // rejected, so checking bytes is the same.)
    s.iter().all(|&r| {
        r.is_ascii_alphanumeric()
            || matches!(
                r,
                b'-' | b'.'
                    | b'_'
                    | b':'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b'%'
                    | b'@'
            )
    })
}

/// Go's `stringContainsCTLByte`: whether s contains an ASCII control byte.
fn string_contains_ctl_byte(s: &[u8]) -> bool {
    s.iter().any(|&b| b < b' ' || b == 0x7f)
}

impl Url {
    /// Go's `setPath`: sets `path` and `raw_path` from the escaped path p,
    /// keeping `raw_path` empty when it equals the default encoding.
    fn set_path(&mut self, p: &[u8]) -> Result<(), ErrorKind> {
        let path = unescape(p, ENCODE_PATH)?;
        let escp = escape(&path, ENCODE_PATH);
        self.path = path;
        if p == escp.as_bytes() {
            // Default encoding is fine.
            self.raw_path = Vec::new();
        } else {
            self.raw_path = p.to_vec();
        }
        Ok(())
    }

    /// Go's `URL.EscapedPath`: `raw_path` when it is a valid escaping of
    /// `path`, else `path` escaped.
    pub fn escaped_path(&self) -> Vec<u8> {
        if !self.raw_path.is_empty()
            && valid_encoded(&self.raw_path, ENCODE_PATH)
            && unescape(&self.raw_path, ENCODE_PATH).is_ok_and(|p| p == self.path)
        {
            return self.raw_path.clone();
        }
        if self.path == b"*" {
            return b"*".to_vec(); // don't escape (Issue 11202)
        }
        escape(&self.path, ENCODE_PATH).into_bytes()
    }

    /// Go's `setFragment`: like `set_path` for `fragment`/`raw_fragment`.
    fn set_fragment(&mut self, f: &[u8]) -> Result<(), ErrorKind> {
        let frag = unescape(f, ENCODE_FRAGMENT)?;
        let escf = escape(&frag, ENCODE_FRAGMENT);
        self.fragment = frag;
        if f == escf.as_bytes() {
            // Default encoding is fine.
            self.raw_fragment = Vec::new();
        } else {
            self.raw_fragment = f.to_vec();
        }
        Ok(())
    }

    /// Go's `URL.EscapedFragment`: `raw_fragment` when it is a valid escaping
    /// of `fragment`, else `fragment` escaped.
    pub fn escaped_fragment(&self) -> Vec<u8> {
        if !self.raw_fragment.is_empty()
            && valid_encoded(&self.raw_fragment, ENCODE_FRAGMENT)
            && unescape(&self.raw_fragment, ENCODE_FRAGMENT).is_ok_and(|f| f == self.fragment)
        {
            return self.raw_fragment.clone();
        }
        escape(&self.fragment, ENCODE_FRAGMENT).into_bytes()
    }

    /// Go's `URL.String`: reassembles the URL, as
    /// `scheme:opaque?query#fragment` when `opaque` is set, else
    /// `scheme://userinfo@host/path?query#fragment` with the parts that are
    /// empty left out. Non-ASCII bytes in the host are escaped.
    pub fn string(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        if !self.scheme.is_empty() {
            buf.extend_from_slice(&self.scheme);
            buf.push(b':');
        }
        if !self.opaque.is_empty() {
            buf.extend_from_slice(&self.opaque);
        } else {
            if !self.scheme.is_empty() || !self.host.is_empty() || self.user.is_some() {
                if self.omit_host && self.host.is_empty() && self.user.is_none() {
                    // omit empty host
                } else {
                    if !self.host.is_empty() || !self.path.is_empty() || self.user.is_some() {
                        buf.extend_from_slice(b"//");
                    }
                    if let Some(ui) = &self.user {
                        buf.extend_from_slice(ui.string().as_bytes());
                        buf.push(b'@');
                    }
                    if !self.host.is_empty() {
                        buf.extend_from_slice(escape(&self.host, ENCODE_HOST).as_bytes());
                    }
                }
            }
            let escaped = self.escaped_path();
            let mut path = &escaped[..];
            if self.omit_host
                && self.host.is_empty()
                && self.user.is_none()
                && path.starts_with(b"//")
            {
                // Escape the first / in a path starting with "//" and no authority
                // so that re-parsing the URL doesn't turn the path into an authority
                // (e.g., Path="//host/p" producing "http://host/p").
                buf.extend_from_slice(b"%2F");
                path = &path[1..];
            }
            if !path.is_empty() && path[0] != b'/' && !self.host.is_empty() {
                buf.push(b'/');
            }
            if buf.is_empty() {
                // RFC 3986 §4.2
                // A path segment that contains a colon character (e.g., "this:that")
                // cannot be used as the first segment of a relative-path reference, as
                // it would be mistaken for a scheme name. Such a segment must be
                // preceded by a dot-segment (e.g., "./this:that") to make a relative-
                // path reference.
                let (segment, _, _) = cut(path, b'/');
                if segment.contains(&b':') {
                    buf.extend_from_slice(b"./");
                }
            }
            buf.extend_from_slice(path);
        }
        if self.force_query || !self.raw_query.is_empty() {
            buf.push(b'?');
            buf.extend_from_slice(&self.raw_query);
        }
        if !self.fragment.is_empty() {
            buf.push(b'#');
            buf.extend_from_slice(&self.escaped_fragment());
        }
        buf
    }

    /// Go's `URL.Redacted`: like [`Url::string`] with any password replaced
    /// by `xxxxx`.
    pub fn redacted(&self) -> Vec<u8> {
        let mut ru = self.clone();
        if let Some(u) = &self.user
            && u.password_set
        {
            ru.user = Some(user_password(&u.username, "xxxxx"));
        }
        ru.string()
    }

    /// Go's `URL.IsAbs`: whether the URL has a scheme.
    pub fn is_abs(&self) -> bool {
        !self.scheme.is_empty()
    }

    /// Go's `URL.Query`: `raw_query` parsed, silently discarding malformed
    /// pairs.
    pub fn query(&self) -> Values {
        parse_query(&self.raw_query).0
    }

    /// Go's `URL.RequestURI`: the encoded `path?query` or `opaque?query` an
    /// HTTP request for the URL would use.
    pub fn request_uri(&self) -> Vec<u8> {
        let mut result = self.opaque.clone();
        if result.is_empty() {
            result = self.escaped_path();
            if result.is_empty() {
                result = b"/".to_vec();
            }
        } else if result.starts_with(b"//") {
            let mut r = self.scheme.clone();
            r.push(b':');
            r.extend_from_slice(&result);
            result = r;
        }
        if self.force_query || !self.raw_query.is_empty() {
            result.push(b'?');
            result.extend_from_slice(&self.raw_query);
        }
        result
    }

    /// Go's `URL.Hostname`: `host` without a valid port, and without the
    /// brackets of an IPv6 literal.
    pub fn hostname(&self) -> &[u8] {
        split_host_port(&self.host).0
    }

    /// Go's `URL.Port`: the port of `host` without the colon, or empty if
    /// it has no valid numeric port.
    pub fn port(&self) -> &[u8] {
        split_host_port(&self.host).1
    }
}

/// Go's `splitHostPort`: separates host and port. If the port is not valid,
/// the whole input is the host, whose validity is not checked. Unlike
/// `net.SplitHostPort`, but per RFC 3986, ports must be numeric.
fn split_host_port(host_port: &[u8]) -> (&[u8], &[u8]) {
    let mut host = host_port;
    let mut port: &[u8] = b"";

    if let Some(colon) = last_index_of(host, b':')
        && valid_optional_port(&host[colon..])
    {
        port = &host[colon + 1..];
        host = &host[..colon];
    }

    if host.starts_with(b"[") && host.ends_with(b"]") && host.len() >= 2 {
        host = &host[1..host.len() - 1];
    }

    (host, port)
}

/// Go's `url.Values`: a query's keys, each with its list of values. Keys
/// are case sensitive. The map is ordered by key bytes, which is the order
/// Go's `Encode` sorts into.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Values(pub BTreeMap<Vec<u8>, Vec<Vec<u8>>>);

impl Values {
    /// Go's `Values.Get`: the first value for key, or empty.
    pub fn get(&self, key: impl AsRef<[u8]>) -> &[u8] {
        self.0
            .get(key.as_ref())
            .and_then(|vs| vs.first())
            .map_or(b"", Vec::as_slice)
    }

    /// Go's `Values.Set`: replaces the values of key with value.
    pub fn set(&mut self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) {
        self.0
            .insert(key.as_ref().to_vec(), vec![value.as_ref().to_vec()]);
    }

    /// Go's `Values.Add`: appends value to the values of key.
    pub fn add(&mut self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) {
        self.0
            .entry(key.as_ref().to_vec())
            .or_default()
            .push(value.as_ref().to_vec());
    }

    /// Go's `Values.Del`: removes key.
    pub fn del(&mut self, key: impl AsRef<[u8]>) {
        self.0.remove(key.as_ref());
    }

    /// Go's `Values.Has`: whether key is set.
    pub fn has(&self, key: impl AsRef<[u8]>) -> bool {
        self.0.contains_key(key.as_ref())
    }

    /// Go's `len(values)`: the number of keys.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no keys.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Go's `Values.Encode`: `key=value` pairs, escaped, sorted by key and
    /// joined with `&`.
    pub fn encode(&self) -> String {
        let mut buf = String::new();
        for (k, vs) in &self.0 {
            let key_escaped = query_escape(k);
            for v in vs {
                if !buf.is_empty() {
                    buf.push('&');
                }
                buf.push_str(&key_escaped);
                buf.push('=');
                buf.push_str(&query_escape(v));
            }
        }
        buf
    }
}

/// Keep this in sync with net/http/httputil.
const DEFAULT_MAX_PARAMS: i64 = 10000;

/// Go's `urlParamsWithinMax`, with the `urlmaxqueryparams` setting.
fn url_params_within_max(params: i64) -> bool {
    let within_default_max = params <= DEFAULT_MAX_PARAMS;
    let value = godebug(b"urlmaxqueryparams");
    if value.is_empty() {
        return within_default_max;
    }
    // Go's strconv.Atoi: an optional sign and decimal digits that fit an
    // int, which is what i64's FromStr accepts.
    let Some(custom_max) = std::str::from_utf8(&value)
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
    else {
        return within_default_max;
    };
    custom_max == 0 || params < custom_max
}

/// Go's `url.ParseQuery`: parses a URL-encoded query into the values for
/// each key, with the first decoding error encountered, if any. The values
/// hold every valid pair either way. A setting without `=` is a key with an
/// empty value; one with an unescaped `;` is invalid.
pub fn parse_query(query: impl AsRef<[u8]>) -> (Values, Option<ErrorKind>) {
    let mut m = Values::default();
    let err = parse_query_into(&mut m, query.as_ref());
    (m, err)
}

/// Go's `parseQuery`.
fn parse_query_into(m: &mut Values, mut query: &[u8]) -> Option<ErrorKind> {
    let ampersands = query.iter().filter(|&&b| b == b'&').count();
    if !url_params_within_max(
        i64::try_from(ampersands)
            .unwrap_or(i64::MAX)
            .saturating_add(1),
    ) {
        return Some(ErrorKind::TooManyQueryParameters);
    }
    let mut err = None;
    while !query.is_empty() {
        let (key, rest, _) = cut(query, b'&');
        query = rest;
        if key.contains(&b';') {
            // (Go assigns this error even over an earlier one.)
            err = Some(ErrorKind::SemicolonSeparator);
            continue;
        }
        if key.is_empty() {
            continue;
        }
        let (key, value, _) = cut(key, b'=');
        let key = match query_unescape(key) {
            Ok(key) => key,
            Err(err1) => {
                if err.is_none() {
                    err = Some(err1.into());
                }
                continue;
            }
        };
        let value = match query_unescape(value) {
            Ok(value) => value,
            Err(err1) => {
                if err.is_none() {
                    err = Some(err1.into());
                }
                continue;
            }
        };
        m.0.entry(key).or_default().push(value);
    }
    err
}

impl fmt::Display for Url {
    /// [`Url::string`], with invalid UTF-8 replaced by U+FFFD.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.string()))
    }
}

#[doc(hidden)]
pub mod internal {
    //! Unexported Go helpers that Go's own tests use; the Rust ports of
    //! those tests reach them from here. Not part of the API.

    pub use super::netip::{Addr, parse_addr};

    /// Go's `encoding` modes, by their Go names.
    pub use super::Encoding;

    /// `encodePath`.
    pub const ENCODE_PATH: Encoding = super::ENCODE_PATH;
    /// `encodePathSegment`.
    pub const ENCODE_PATH_SEGMENT: Encoding = super::ENCODE_PATH_SEGMENT;
    /// `encodeHost`.
    pub const ENCODE_HOST: Encoding = super::ENCODE_HOST;
    /// `encodeZone`.
    pub const ENCODE_ZONE: Encoding = super::ENCODE_ZONE;
    /// `encodeUserPassword`.
    pub const ENCODE_USER_PASSWORD: Encoding = super::ENCODE_USER_PASSWORD;
    /// `encodeQueryComponent`.
    pub const ENCODE_QUERY_COMPONENT: Encoding = super::ENCODE_QUERY_COMPONENT;
    /// `encodeFragment`.
    pub const ENCODE_FRAGMENT: Encoding = super::ENCODE_FRAGMENT;

    /// Go's `shouldEscape` (the generated-table lookup).
    pub fn should_escape(c: u8, mode: Encoding) -> bool {
        super::should_escape(c, mode)
    }

    /// Go's `escape`.
    pub fn escape(s: &[u8], mode: Encoding) -> String {
        super::escape(s, mode)
    }

    /// Go's `unescape`.
    pub fn unescape(s: &[u8], mode: Encoding) -> Result<Vec<u8>, super::ErrorKind> {
        super::unescape(s, mode)
    }

    /// The `GODEBUG` setting `name`, as `internal/godebug` resolves it.
    pub fn godebug(name: &[u8]) -> Vec<u8> {
        super::godebug(name)
    }

    /// `defaultMaxParams`.
    pub const DEFAULT_MAX_PARAMS: i64 = super::DEFAULT_MAX_PARAMS;
}
