# rustls 0.23.46, patched for Gorse

This is the `rustls` 0.23.46 package as published on crates.io (sha256
`48e13bd8c0e9365c43cfa5c9e8f9ad49d3c8444926c9aac819e0e4dc503c8fdf`), with one
change. The workspace uses it through `[patch.crates-io]` in the root
`Cargo.toml`. Decided by Ritchie on 2026-10-18 (WP-1-4 notes, decision card
"How should a Rust master accept Go workers' TLS connections?").

## The change

`src/server/hs.rs`: a server that receives a `server_name` extension whose
host name is not a valid DNS name now ignores it, as rustls already did for an
IP address, instead of failing the handshake with
`PeerMisbehaved::ServerNameMustContainOneHostName` and an `illegal_parameter`
alert. `src/server/test.rs`: the unit test for that case now expects the
handshake to go on.

## Why

Gorse's workers and servers reach the master's gRPC port with
`util.NewClientCreds`, built on `google.golang.org/grpc/security/advancedtls`.
Its `ClientHandshake` uses the dial target, `host:port`, as the TLS server name
without stripping the port, and Go's `crypto/tls` sends it as is. Unpatched,
a Rust master refuses every Go client over TLS; `contracts/tools/tlsgolden
-interop` shows it (34 of 62 cases, every one that reaches a handshake).

The server name is only a hint for choosing a certificate. Gorse's master has
one certificate and verifies every client certificate whatever name was sent,
so ignoring an unusable name changes nothing else.

## Upgrading

Copy the new release from the cargo registry over this directory, re-apply the
two hunks (`diff -ru` against the registry copy shows them), keep this file,
and re-run the checks below.

The published package leaves out the test data rustls's own unit tests read,
so those tests do not build from this copy. The change is covered instead by
`util_tls_server_ignores_a_host_and_port_server_name` in
`crates/gorse-common/tests/util_tls.rs`, which fails without the patch, and by
`go run ./contracts/tools/tlsgolden -interop <tls_peer>` (see
`porting/notes/WP-1-4.md`), where every Go/Rust pairing must match Go/Go.
