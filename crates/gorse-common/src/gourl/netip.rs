// Ports: $GOROOT/src/net/netip/netip.go (ParseAddr, parseAddrError, parseIPv4Fields, parseIPv4, parseIPv6, WithZone, Is4, Zone) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's net/netip package: Copyright 2020 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! The part of Go's `net/netip` that `url.parseHost` calls: `ParseAddr`,
//! which decides whether a bracketed host is an IPv6 address and words the
//! error when it is not.

use crate::gostrconv::quote;

/// Go's `netip.Addr`, as far as `net/url` looks at it: the 16 address bytes
/// (an IPv4 address in the first four), whether it is IPv4, and the zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addr {
    bytes: [u8; 16],
    is4: bool,
    zone: Vec<u8>,
}

impl Addr {
    fn from4(addr: [u8; 4]) -> Addr {
        let mut bytes = [0; 16];
        bytes[..4].copy_from_slice(&addr);
        Addr {
            bytes,
            is4: true,
            zone: Vec::new(),
        }
    }

    /// Go's `AddrFrom16(addr).WithZone(zone)`.
    fn from16_with_zone(addr: [u8; 16], zone: &[u8]) -> Addr {
        Addr {
            bytes: addr,
            is4: false,
            zone: zone.to_vec(),
        }
    }

    /// Go's `Addr.Is4`: whether the address is IPv4.
    pub fn is4(&self) -> bool {
        self.is4
    }

    /// Go's `Addr.Zone`: the IPv6 scoped addressing zone, if any.
    pub fn zone(&self) -> &[u8] {
        &self.zone
    }

    /// Go's `Addr.As4` for an IPv4 address and `As16` for an IPv6 one.
    pub fn as_slice(&self) -> &[u8] {
        if self.is4 {
            &self.bytes[..4]
        } else {
            &self.bytes
        }
    }
}

/// Go's `netip.parseAddrError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("ParseAddr({}): {msg}{}", quote(.input), at_suffix(.at))]
pub struct ParseAddrError {
    /// The string given to `ParseAddr`.
    pub input: Vec<u8>,
    /// An explanation of the parse failure.
    pub msg: &'static str,
    /// Optionally, the unparsed portion of `input` at which the error
    /// occurred.
    pub at: Vec<u8>,
}

fn at_suffix(at: &[u8]) -> String {
    if at.is_empty() {
        String::new()
    } else {
        format!(" (at {})", quote(at))
    }
}

fn err(input: &[u8], msg: &'static str) -> ParseAddrError {
    err_at(input, msg, b"")
}

fn err_at(input: &[u8], msg: &'static str, at: &[u8]) -> ParseAddrError {
    ParseAddrError {
        input: input.to_vec(),
        msg,
        at: at.to_vec(),
    }
}

/// Go's `netip.ParseAddr`: parses s as an IP address, in dotted decimal
/// (`192.0.2.1`), IPv6 (`2001:db8::68`), or IPv6 with a scoped addressing
/// zone (`fe80::1cc0:3e8c:119f:c2e1%ens18`).
pub fn parse_addr(s: &[u8]) -> Result<Addr, ParseAddrError> {
    for &c in s {
        match c {
            b'.' => return parse_ipv4(s),
            b':' => return parse_ipv6(s),
            b'%' => {
                // Assume that this was trying to be an IPv6 address with
                // a zone specifier, but the address is missing.
                return Err(err(s, "missing IPv6 address"));
            }
            _ => {}
        }
    }
    Err(err(s, "unable to parse IP"))
}

/// Go's `parseIPv4Fields`: parses `input[off..end]` into the four bytes of
/// `fields`.
fn parse_ipv4_fields(
    input: &[u8],
    off: usize,
    end: usize,
    fields: &mut [u8],
) -> Result<(), ParseAddrError> {
    let mut val: u32 = 0;
    let mut pos = 0;
    let mut dig_len = 0; // number of digits in current octet
    let s = &input[off..end];
    for i in 0..s.len() {
        if s[i].is_ascii_digit() {
            if dig_len == 1 && val == 0 {
                return Err(err(input, "IPv4 field has octet with leading zero"));
            }
            val = val * 10 + u32::from(s[i] - b'0');
            dig_len += 1;
            if val > 255 {
                return Err(err(input, "IPv4 field has value >255"));
            }
        } else if s[i] == b'.' {
            // .1.2.3
            // 1.2.3.
            // 1..2.3
            if i == 0 || i == s.len() - 1 || s[i - 1] == b'.' {
                return Err(err_at(
                    input,
                    "IPv4 field must have at least one digit",
                    &s[i..],
                ));
            }
            // 1.2.3.4.5
            if pos == 3 {
                return Err(err(input, "IPv4 address too long"));
            }
            fields[pos] = val as u8; // val <= 255
            pos += 1;
            val = 0;
            dig_len = 0;
        } else {
            return Err(err_at(input, "unexpected character", &s[i..]));
        }
    }
    if pos < 3 {
        return Err(err(input, "IPv4 address too short"));
    }
    fields[3] = val as u8; // val <= 255
    Ok(())
}

/// Go's `parseIPv4`: parses s as an IPv4 address (in form `192.168.0.1`).
fn parse_ipv4(s: &[u8]) -> Result<Addr, ParseAddrError> {
    let mut fields = [0u8; 4];
    parse_ipv4_fields(s, 0, s.len(), &mut fields)?;
    Ok(Addr::from4(fields))
}

/// Go's `parseIPv6`: parses `input` as an IPv6 address (in form
/// `2001:db8::68`).
fn parse_ipv6(input: &[u8]) -> Result<Addr, ParseAddrError> {
    let mut s = input;

    // Split off the zone right from the start. Yes it's a second scan
    // of the string, but trying to handle it inline makes a bunch of
    // other inner loop conditionals more expensive, and it ends up
    // being slower.
    let mut zone: &[u8] = b"";
    if let Some(i) = s.iter().position(|&b| b == b'%') {
        (s, zone) = (&s[..i], &s[i + 1..]);
        if zone.is_empty() {
            // Not allowed to have an empty zone if explicitly specified.
            return Err(err(input, "zone must be a non-empty string"));
        }
    }

    let mut ip = [0u8; 16];
    let mut ellipsis: Option<usize> = None; // position of ellipsis in ip

    // Might have leading ellipsis
    if s.len() >= 2 && s[0] == b':' && s[1] == b':' {
        ellipsis = Some(0);
        s = &s[2..];
        // Might be only ellipsis
        if s.is_empty() {
            return Ok(Addr::from16_with_zone([0; 16], zone));
        }
    }

    // Loop, parsing hex numbers followed by colon.
    let mut i = 0;
    while i < 16 {
        // Hex number. Similar to parseIPv4, inlining the hex number
        // parsing yields a significant performance increase.
        let mut off = 0;
        let mut acc: u32 = 0;
        while off < s.len() {
            let c = s[off];
            if c.is_ascii_digit() {
                acc = (acc << 4) + u32::from(c - b'0');
            } else if (b'a'..=b'f').contains(&c) {
                acc = (acc << 4) + u32::from(c - b'a' + 10);
            } else if (b'A'..=b'F').contains(&c) {
                acc = (acc << 4) + u32::from(c - b'A' + 10);
            } else {
                break;
            }
            if off > 3 {
                //more than 4 digits in group, fail.
                return Err(err_at(input, "each group must have 4 or less digits", s));
            }
            if acc > u32::from(u16::MAX) {
                // Overflow, fail.
                return Err(err_at(input, "IPv6 field has value >=2^16", s));
            }
            off += 1;
        }
        if off == 0 {
            // No digits found, fail.
            return Err(err_at(
                input,
                "each colon-separated field must have at least one digit",
                s,
            ));
        }

        // If followed by dot, might be in trailing IPv4.
        if off < s.len() && s[off] == b'.' {
            if ellipsis.is_none() && i != 12 {
                // Not the right place.
                return Err(err_at(
                    input,
                    "embedded IPv4 address must replace the final 2 fields of the address",
                    s,
                ));
            }
            if i + 4 > 16 {
                // Not enough room.
                return Err(err_at(
                    input,
                    "too many hex fields to fit an embedded IPv4 at the end of the address",
                    s,
                ));
            }

            let mut end = input.len();
            if !zone.is_empty() {
                end -= zone.len() + 1;
            }
            parse_ipv4_fields(input, end - s.len(), end, &mut ip[i..i + 4])?;
            s = b"";
            i += 4;
            break;
        }

        // Save this 16-bit chunk.
        ip[i] = (acc >> 8) as u8;
        ip[i + 1] = acc as u8;
        i += 2;

        // Stop at end of string.
        s = &s[off..];
        if s.is_empty() {
            break;
        }

        // Otherwise must be followed by colon and more.
        if s[0] != b':' {
            return Err(err_at(input, "unexpected character, want colon", s));
        } else if s.len() == 1 {
            return Err(err_at(
                input,
                "colon must be followed by more characters",
                s,
            ));
        }
        s = &s[1..];

        // Look for ellipsis.
        if s[0] == b':' {
            if ellipsis.is_some() {
                // already have one
                return Err(err_at(input, "multiple :: in address", s));
            }
            ellipsis = Some(i);
            s = &s[1..];
            if s.is_empty() {
                // can be at end
                break;
            }
        }
    }

    // Must have used entire string.
    if !s.is_empty() {
        return Err(err_at(input, "trailing garbage after address", s));
    }

    // If didn't parse enough, expand ellipsis.
    if i < 16 {
        let Some(ellipsis) = ellipsis else {
            return Err(err(input, "address string too short"));
        };
        let n = 16 - i;
        ip.copy_within(ellipsis..i, ellipsis + n);
        ip[ellipsis..ellipsis + n].fill(0);
    } else if ellipsis.is_some() {
        // Ellipsis must represent at least one 0 group.
        return Err(err(
            input,
            "the :: must expand to at least one field of zeros",
        ));
    }
    Ok(Addr::from16_with_zone(ip, zone))
}
