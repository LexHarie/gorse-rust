// Ports: $GOROOT/src/net/ipsock.go (SplitHostPort, JoinHostPort), net.go (AddrError) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's net package: Copyright 2009 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `net.SplitHostPort` and `net.JoinHostPort`, which the MySQL DSN
//! normalization uses to decide whether an address has a port.
//!
//! Go strings are bytes, so both work on bytes.

use thiserror::Error;

/// Go's `*net.AddrError`. Its text is `address <addr>: <err>`, or `<err>`
/// when the address is empty; invalid UTF-8 in the address is shown
/// replaced by U+FFFD (Go writes the raw bytes).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{}", self.text())]
pub struct AddrError {
    /// Go's `Err`.
    pub err: &'static str,
    /// Go's `Addr`.
    pub addr: Vec<u8>,
}

impl AddrError {
    fn text(&self) -> String {
        if self.addr.is_empty() {
            return self.err.to_owned();
        }
        format!(
            "address {}: {}",
            String::from_utf8_lossy(&self.addr),
            self.err
        )
    }
}

/// Go's `net.SplitHostPort`: splits `host:port`, `host%zone:port`,
/// `[host]:port` or `[host%zone]:port` into host (or `host%zone`) and port.
pub fn split_host_port(hostport: &[u8]) -> Result<(&[u8], &[u8]), AddrError> {
    const MISSING_PORT: &str = "missing port in address";
    const TOO_MANY_COLONS: &str = "too many colons in address";
    let addr_err = |why: &'static str| AddrError {
        err: why,
        addr: hostport.to_vec(),
    };
    let (mut j, mut k) = (0, 0);

    // The port starts after the last colon.
    let Some(i) = hostport.iter().rposition(|&b| b == b':') else {
        return Err(addr_err(MISSING_PORT));
    };

    let host;
    if hostport[0] == b'[' {
        // Expect the first ']' just before the last ':'.
        let Some(end) = hostport.iter().position(|&b| b == b']') else {
            return Err(addr_err("missing ']' in address"));
        };
        if end + 1 == hostport.len() {
            // There can't be a ':' behind the ']' now.
            return Err(addr_err(MISSING_PORT));
        } else if end + 1 != i {
            // Either ']' isn't followed by a colon, or it is followed by a
            // colon that is not the last one.
            if hostport[end + 1] == b':' {
                return Err(addr_err(TOO_MANY_COLONS));
            }
            return Err(addr_err(MISSING_PORT));
        }
        host = &hostport[1..end];
        // there can't be a '[' resp. ']' before these positions
        (j, k) = (1, end + 1);
    } else {
        host = &hostport[..i];
        if host.contains(&b':') {
            return Err(addr_err(TOO_MANY_COLONS));
        }
    }
    if hostport[j..].contains(&b'[') {
        return Err(addr_err("unexpected '[' in address"));
    }
    if hostport[k..].contains(&b']') {
        return Err(addr_err("unexpected ']' in address"));
    }

    Ok((host, &hostport[i + 1..]))
}

/// Go's `net.JoinHostPort`: `host:port`, or `[host]:port` when host contains
/// a colon (an IPv6 literal).
pub fn join_host_port(host: &[u8], port: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(host.len() + port.len() + 3);
    // We assume that host is a literal IPv6 address if host has colons.
    if host.contains(&b':') {
        out.push(b'[');
        out.extend_from_slice(host);
        out.extend_from_slice(b"]:");
    } else {
        out.extend_from_slice(host);
        out.push(b':');
    }
    out.extend_from_slice(port);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ports: net/ip_test.go TestSplitHostPort.
    #[test]
    fn test_split_host_port() {
        for (host_port, host, port) in [
            // Host name
            ("localhost:http", "localhost", "http"),
            ("localhost:80", "localhost", "80"),
            // Go-specific host name with zone identifier
            ("localhost%lo0:http", "localhost%lo0", "http"),
            ("localhost%lo0:80", "localhost%lo0", "80"),
            ("[localhost%lo0]:http", "localhost%lo0", "http"), // Go 1 behavior
            ("[localhost%lo0]:80", "localhost%lo0", "80"),     // Go 1 behavior
            // IP literal
            ("127.0.0.1:http", "127.0.0.1", "http"),
            ("127.0.0.1:80", "127.0.0.1", "80"),
            ("[::1]:http", "::1", "http"),
            ("[::1]:80", "::1", "80"),
            // IP literal with zone identifier
            ("[::1%lo0]:http", "::1%lo0", "http"),
            ("[::1%lo0]:80", "::1%lo0", "80"),
            // Go-specific wildcard for host name
            (":http", "", "http"), // Go 1 behavior
            (":80", "", "80"),     // Go 1 behavior
            // Go-specific wildcard for service name or transport port number
            ("golang.org:", "golang.org", ""), // Go 1 behavior
            ("127.0.0.1:", "127.0.0.1", ""),   // Go 1 behavior
            ("[::1]:", "::1", ""),             // Go 1 behavior
            // Opaque service name
            ("golang.org:https%foo", "golang.org", "https%foo"), // Go 1 behavior
        ] {
            let got = split_host_port(host_port.as_bytes());
            assert_eq!(
                got,
                Ok((host.as_bytes(), port.as_bytes())),
                "SplitHostPort({host_port:?})"
            );
        }

        for (host_port, err) in [
            ("golang.org", "missing port in address"),
            ("127.0.0.1", "missing port in address"),
            ("[::1]", "missing port in address"),
            ("[fe80::1%lo0]", "missing port in address"),
            ("[localhost%lo0]", "missing port in address"),
            ("localhost%lo0", "missing port in address"),
            ("::1", "too many colons in address"),
            ("fe80::1%lo0", "too many colons in address"),
            ("fe80::1%lo0:80", "too many colons in address"),
            // Test cases that didn't fail in Go 1
            ("[foo:bar]", "missing port in address"),
            ("[foo:bar]baz", "missing port in address"),
            ("[foo]bar:baz", "missing port in address"),
            ("[foo]:[bar]:baz", "too many colons in address"),
            ("[foo]:[bar]baz", "unexpected '[' in address"),
            ("foo[bar]:baz", "unexpected '[' in address"),
            ("foo]bar:baz", "unexpected ']' in address"),
        ] {
            // (On failure Go returns empty host and port; here there are none
            // to return.)
            match split_host_port(host_port.as_bytes()) {
                Ok(_) => panic!("SplitHostPort({host_port:?}) should have failed"),
                Err(e) => {
                    assert_eq!(e.err, err, "SplitHostPort({host_port:?})");
                    assert_eq!(e.addr, host_port.as_bytes());
                    assert_eq!(e.to_string(), format!("address {host_port}: {err}"));
                }
            }
        }
    }

    // Ports: net/ip_test.go TestJoinHostPort.
    #[test]
    fn test_join_host_port() {
        for (host, port, host_port) in [
            // Host name
            ("localhost", "http", "localhost:http"),
            ("localhost", "80", "localhost:80"),
            // Go-specific host name with zone identifier
            ("localhost%lo0", "http", "localhost%lo0:http"),
            ("localhost%lo0", "80", "localhost%lo0:80"),
            // IP literal
            ("127.0.0.1", "http", "127.0.0.1:http"),
            ("127.0.0.1", "80", "127.0.0.1:80"),
            ("::1", "http", "[::1]:http"),
            ("::1", "80", "[::1]:80"),
            // IP literal with zone identifier
            ("::1%lo0", "http", "[::1%lo0]:http"),
            ("::1%lo0", "80", "[::1%lo0]:80"),
            // Go-specific wildcard for host name
            ("", "http", ":http"), // Go 1 behavior
            ("", "80", ":80"),     // Go 1 behavior
            // Go-specific wildcard for service name or transport port number
            ("golang.org", "", "golang.org:"), // Go 1 behavior
            ("127.0.0.1", "", "127.0.0.1:"),   // Go 1 behavior
            ("::1", "", "[::1]:"),             // Go 1 behavior
            // Opaque service name
            ("golang.org", "https%foo", "golang.org:https%foo"), // Go 1 behavior
        ] {
            assert_eq!(
                join_host_port(host.as_bytes(), port.as_bytes()),
                host_port.as_bytes(),
                "JoinHostPort({host:?}, {port:?})"
            );
        }
    }
}
