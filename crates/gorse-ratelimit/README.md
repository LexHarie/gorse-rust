# gorse-ratelimit

A Rust translation of [juju/ratelimit](https://github.com/juju/ratelimit)
v1.0.2 (`ratelimit.go` and `reader.go`), the token bucket that Gorse's
`common/parallel/ratelimit.go` builds its OpenAI rate limiters on.

## Licence

juju/ratelimit is Copyright (C) 2014-2015 Canonical Ltd. and licensed under the
GNU Lesser General Public License version 3 with a static-linking exception
([LICENSE](LICENSE); the GNU GPL version 3 it supplements is in
[COPYING](COPYING)). This crate is a modified version of that library and is
distributed under the same terms, exception included
(`LGPL-3.0-only WITH LGPL-3.0-linking-exception`). The rest of the gorse-rust
workspace is Apache-2.0; this crate is kept separate so that the LGPL applies
to it alone, and the exception lets the Gorse binaries link it statically.
`src/format.rs`, a few lines of Go's `strconv` for one panic message, is
derived from Go (BSD-3-Clause, [licenses/GO-LICENSE](licenses/GO-LICENSE)).

Modifications: on 2026-10-19 the Go source was translated to Rust for the
gorse-rust port (behaviour kept; Go's `time.Duration` is `i64` nanoseconds and
Go's `time.Time` is `std::time::Instant`). Every difference from the Go
behaviour is listed in the crate documentation and in
`porting/notes/WP-1-6.md`.
