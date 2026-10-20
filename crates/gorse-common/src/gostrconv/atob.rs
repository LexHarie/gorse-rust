// Ports: $GOROOT/src/internal/strconv/atob.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strconv package: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! `internal/strconv.ParseBool`, `FormatBool` and `AppendBool`.

use super::NumErrorKind;

/// `parse_bool` returns the boolean value represented by the string.
/// It accepts 1, t, T, TRUE, true, True, 0, f, F, FALSE, false, False.
/// Any other value returns an error.
pub(super) fn parse_bool(s: &[u8]) -> (bool, Option<NumErrorKind>) {
    match s {
        b"1" | b"t" | b"T" | b"true" | b"TRUE" | b"True" => (true, None),
        b"0" | b"f" | b"F" | b"false" | b"FALSE" | b"False" => (false, None),
        _ => (false, Some(NumErrorKind::Syntax)),
    }
}

/// `format_bool` returns "true" or "false" according to the value of b.
pub(super) const fn format_bool(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// `append_bool` appends "true" or "false", according to the value of b,
/// to dst.
pub(super) fn append_bool(dst: &mut Vec<u8>, b: bool) {
    if b {
        dst.extend_from_slice(b"true");
    } else {
        dst.extend_from_slice(b"false");
    }
}
