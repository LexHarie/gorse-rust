// Ports: $GOROOT/src/internal/strconv/atoi_test.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strconv tests: Copyright 2009 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's integer-parsing tests, case for case. Go's tests call
//! `internal/strconv`, whose errors are the bare `ErrSyntax`/`ErrRange`; the
//! public functions wrap them in a `*NumError`, so these compare its reason.
//! `IntSize` is 64, so the 64-bit tables run for `ParseInt(s, 10, 0)`,
//! `ParseUint(s, 10, 0)` and `Atoi`.

mod gostrconv {
    use gorse_common::gostrconv::NumErrorKind::{self, Range, Syntax};
    use gorse_common::gostrconv::{atoi, parse_int, parse_uint};

    const PARSE_UINT64_TESTS: &[(&str, u64, Option<NumErrorKind>)] = &[
        ("", 0, Some(Syntax)),
        ("0", 0, None),
        ("1", 1, None),
        ("12345", 12345, None),
        ("012345", 12345, None),
        ("12345x", 0, Some(Syntax)),
        ("98765432100", 98765432100, None),
        ("18446744073709551615", 18446744073709551615, None),
        ("18446744073709551616", 18446744073709551615, Some(Range)),
        ("18446744073709551620", 18446744073709551615, Some(Range)),
        ("1_2_3_4_5", 0, Some(Syntax)), // base=10 so no underscores allowed
        ("_12345", 0, Some(Syntax)),
        ("1__2345", 0, Some(Syntax)),
        ("12345_", 0, Some(Syntax)),
        ("-0", 0, Some(Syntax)),
        ("-1", 0, Some(Syntax)),
        ("+1", 0, Some(Syntax)),
    ];

    const PARSE_UINT64_BASE_TESTS: &[(&str, i64, u64, Option<NumErrorKind>)] = &[
        ("", 0, 0, Some(Syntax)),
        ("0", 0, 0, None),
        ("0x", 0, 0, Some(Syntax)),
        ("0X", 0, 0, Some(Syntax)),
        ("1", 0, 1, None),
        ("12345", 0, 12345, None),
        ("012345", 0, 5349, None),
        ("0x12345", 0, 74565, None),
        ("0X12345", 0, 74565, None),
        ("12345x", 0, 0, Some(Syntax)),
        ("0xabcdefg123", 0, 0, Some(Syntax)),
        ("123456789abc", 0, 0, Some(Syntax)),
        ("98765432100", 0, 98765432100, None),
        ("18446744073709551615", 0, 18446744073709551615, None),
        ("18446744073709551616", 0, 18446744073709551615, Some(Range)),
        ("18446744073709551620", 0, 18446744073709551615, Some(Range)),
        ("0xFFFFFFFFFFFFFFFF", 0, 18446744073709551615, None),
        ("0x10000000000000000", 0, 18446744073709551615, Some(Range)),
        ("01777777777777777777777", 0, 18446744073709551615, None),
        ("01777777777777777777778", 0, 0, Some(Syntax)),
        (
            "02000000000000000000000",
            0,
            18446744073709551615,
            Some(Range),
        ),
        ("0200000000000000000000", 0, 2305843009213693952, None),
        ("0b", 0, 0, Some(Syntax)),
        ("0B", 0, 0, Some(Syntax)),
        ("0b101", 0, 5, None),
        ("0B101", 0, 5, None),
        ("0o", 0, 0, Some(Syntax)),
        ("0O", 0, 0, Some(Syntax)),
        ("0o377", 0, 255, None),
        ("0O377", 0, 255, None),
        // underscores allowed with base == 0 only
        ("1_2_3_4_5", 0, 12345, None), // base 0 => 10
        ("_12345", 0, 0, Some(Syntax)),
        ("1__2345", 0, 0, Some(Syntax)),
        ("12345_", 0, 0, Some(Syntax)),
        ("1_2_3_4_5", 10, 0, Some(Syntax)), // base 10
        ("_12345", 10, 0, Some(Syntax)),
        ("1__2345", 10, 0, Some(Syntax)),
        ("12345_", 10, 0, Some(Syntax)),
        ("0x_1_2_3_4_5", 0, 74565, None), // base 0 => 16
        ("_0x12345", 0, 0, Some(Syntax)),
        ("0x__12345", 0, 0, Some(Syntax)),
        ("0x1__2345", 0, 0, Some(Syntax)),
        ("0x1234__5", 0, 0, Some(Syntax)),
        ("0x12345_", 0, 0, Some(Syntax)),
        ("1_2_3_4_5", 16, 0, Some(Syntax)), // base 16
        ("_12345", 16, 0, Some(Syntax)),
        ("1__2345", 16, 0, Some(Syntax)),
        ("1234__5", 16, 0, Some(Syntax)),
        ("12345_", 16, 0, Some(Syntax)),
        ("0_1_2_3_4_5", 0, 5349, None), // base 0 => 8 (0377)
        ("_012345", 0, 0, Some(Syntax)),
        ("0__12345", 0, 0, Some(Syntax)),
        ("01234__5", 0, 0, Some(Syntax)),
        ("012345_", 0, 0, Some(Syntax)),
        ("0o_1_2_3_4_5", 0, 5349, None), // base 0 => 8 (0o377)
        ("_0o12345", 0, 0, Some(Syntax)),
        ("0o__12345", 0, 0, Some(Syntax)),
        ("0o1234__5", 0, 0, Some(Syntax)),
        ("0o12345_", 0, 0, Some(Syntax)),
        ("0_1_2_3_4_5", 8, 0, Some(Syntax)), // base 8
        ("_012345", 8, 0, Some(Syntax)),
        ("0__12345", 8, 0, Some(Syntax)),
        ("01234__5", 8, 0, Some(Syntax)),
        ("012345_", 8, 0, Some(Syntax)),
        ("0b_1_0_1", 0, 5, None), // base 0 => 2 (0b101)
        ("_0b101", 0, 0, Some(Syntax)),
        ("0b__101", 0, 0, Some(Syntax)),
        ("0b1__01", 0, 0, Some(Syntax)),
        ("0b10__1", 0, 0, Some(Syntax)),
        ("0b101_", 0, 0, Some(Syntax)),
        ("1_0_1", 2, 0, Some(Syntax)), // base 2
        ("_101", 2, 0, Some(Syntax)),
        ("1_01", 2, 0, Some(Syntax)),
        ("10_1", 2, 0, Some(Syntax)),
        ("101_", 2, 0, Some(Syntax)),
    ];

    const PARSE_INT64_TESTS: &[(&str, i64, Option<NumErrorKind>)] = &[
        ("", 0, Some(Syntax)),
        ("0", 0, None),
        ("-0", 0, None),
        ("+0", 0, None),
        ("1", 1, None),
        ("-1", -1, None),
        ("+1", 1, None),
        ("12345", 12345, None),
        ("-12345", -12345, None),
        ("012345", 12345, None),
        ("-012345", -12345, None),
        ("98765432100", 98765432100, None),
        ("-98765432100", -98765432100, None),
        ("9223372036854775807", 9223372036854775807, None),
        ("-9223372036854775807", -9223372036854775807, None),
        ("9223372036854775808", 9223372036854775807, Some(Range)),
        ("-9223372036854775808", i64::MIN, None),
        ("9223372036854775809", 9223372036854775807, Some(Range)),
        ("-9223372036854775809", i64::MIN, Some(Range)),
        ("-1_2_3_4_5", 0, Some(Syntax)), // base=10 so no underscores allowed
        ("-_12345", 0, Some(Syntax)),
        ("_12345", 0, Some(Syntax)),
        ("1__2345", 0, Some(Syntax)),
        ("12345_", 0, Some(Syntax)),
        ("123%45", 0, Some(Syntax)),
    ];

    const PARSE_INT64_BASE_TESTS: &[(&str, i64, i64, Option<NumErrorKind>)] = &[
        ("", 0, 0, Some(Syntax)),
        ("0", 0, 0, None),
        ("-0", 0, 0, None),
        ("1", 0, 1, None),
        ("-1", 0, -1, None),
        ("12345", 0, 12345, None),
        ("-12345", 0, -12345, None),
        ("012345", 0, 5349, None),
        ("-012345", 0, -5349, None),
        ("0x12345", 0, 74565, None),
        ("-0X12345", 0, -74565, None),
        ("12345x", 0, 0, Some(Syntax)),
        ("-12345x", 0, 0, Some(Syntax)),
        ("98765432100", 0, 98765432100, None),
        ("-98765432100", 0, -98765432100, None),
        ("9223372036854775807", 0, 9223372036854775807, None),
        ("-9223372036854775807", 0, -9223372036854775807, None),
        ("9223372036854775808", 0, 9223372036854775807, Some(Range)),
        ("-9223372036854775808", 0, i64::MIN, None),
        ("9223372036854775809", 0, 9223372036854775807, Some(Range)),
        ("-9223372036854775809", 0, i64::MIN, Some(Range)),
        // other bases
        ("g", 17, 16, None),
        ("10", 25, 25, None),
        ("holycow", 35, 32544027072, None),
        ("holycow", 36, 38493362624, None),
        // base 2
        ("0", 2, 0, None),
        ("-1", 2, -1, None),
        ("1010", 2, 10, None),
        ("1000000000000000", 2, 32768, None),
        (
            "111111111111111111111111111111111111111111111111111111111111111",
            2,
            9223372036854775807,
            None,
        ),
        (
            "1000000000000000000000000000000000000000000000000000000000000000",
            2,
            9223372036854775807,
            Some(Range),
        ),
        (
            "-1000000000000000000000000000000000000000000000000000000000000000",
            2,
            i64::MIN,
            None,
        ),
        (
            "-1000000000000000000000000000000000000000000000000000000000000001",
            2,
            i64::MIN,
            Some(Range),
        ),
        // base 8
        ("-10", 8, -8, None),
        ("57635436545", 8, 6416645477, None),
        ("100000000", 8, 16777216, None),
        // base 16
        ("10", 16, 16, None),
        ("-123456789abcdef", 16, -81985529216486895, None),
        ("7fffffffffffffff", 16, 9223372036854775807, None),
        // underscores
        ("-0x_1_2_3_4_5", 0, -74565, None),
        ("0x_1_2_3_4_5", 0, 74565, None),
        ("-_0x12345", 0, 0, Some(Syntax)),
        ("_-0x12345", 0, 0, Some(Syntax)),
        ("_0x12345", 0, 0, Some(Syntax)),
        ("0x__12345", 0, 0, Some(Syntax)),
        ("0x1__2345", 0, 0, Some(Syntax)),
        ("0x1234__5", 0, 0, Some(Syntax)),
        ("0x12345_", 0, 0, Some(Syntax)),
        ("-0_1_2_3_4_5", 0, -5349, None), // octal
        ("0_1_2_3_4_5", 0, 5349, None),   // octal
        ("-_012345", 0, 0, Some(Syntax)),
        ("_-012345", 0, 0, Some(Syntax)),
        ("_012345", 0, 0, Some(Syntax)),
        ("0__12345", 0, 0, Some(Syntax)),
        ("01234__5", 0, 0, Some(Syntax)),
        ("012345_", 0, 0, Some(Syntax)),
        ("+0xf", 0, 15, None),
        ("-0xf", 0, -15, None),
        ("0x+f", 0, 0, Some(Syntax)),
        ("0x-f", 0, 0, Some(Syntax)),
    ];

    const PARSE_UINT32_TESTS: &[(&str, u64, Option<NumErrorKind>)] = &[
        ("", 0, Some(Syntax)),
        ("0", 0, None),
        ("1", 1, None),
        ("12345", 12345, None),
        ("012345", 12345, None),
        ("12345x", 0, Some(Syntax)),
        ("987654321", 987654321, None),
        ("4294967295", 4294967295, None),
        ("4294967296", 4294967295, Some(Range)),
        ("1_2_3_4_5", 0, Some(Syntax)), // base=10 so no underscores allowed
        ("_12345", 0, Some(Syntax)),
        ("_12345", 0, Some(Syntax)),
        ("1__2345", 0, Some(Syntax)),
        ("12345_", 0, Some(Syntax)),
    ];

    const PARSE_INT32_TESTS: &[(&str, i64, Option<NumErrorKind>)] = &[
        ("", 0, Some(Syntax)),
        ("0", 0, None),
        ("-0", 0, None),
        ("1", 1, None),
        ("-1", -1, None),
        ("12345", 12345, None),
        ("-12345", -12345, None),
        ("012345", 12345, None),
        ("-012345", -12345, None),
        ("12345x", 0, Some(Syntax)),
        ("-12345x", 0, Some(Syntax)),
        ("987654321", 987654321, None),
        ("-987654321", -987654321, None),
        ("2147483647", 2147483647, None),
        ("-2147483647", -2147483647, None),
        ("2147483648", 2147483647, Some(Range)),
        ("-2147483648", -2147483648, None),
        ("2147483649", 2147483647, Some(Range)),
        ("-2147483649", -2147483648, Some(Range)),
        ("-1_2_3_4_5", 0, Some(Syntax)), // base=10 so no underscores allowed
        ("-_12345", 0, Some(Syntax)),
        ("_12345", 0, Some(Syntax)),
        ("1__2345", 0, Some(Syntax)),
        ("12345_", 0, Some(Syntax)),
        ("123%45", 0, Some(Syntax)),
    ];

    fn kind(err: Option<gorse_common::gostrconv::NumError>) -> Option<NumErrorKind> {
        err.map(|e| e.err)
    }

    #[test]
    fn test_parse_uint32() {
        for &(input, want, want_err) in PARSE_UINT32_TESTS {
            let (out, err) = parse_uint(input, 10, 32);
            assert_eq!(
                (out, kind(err)),
                (want, want_err),
                "ParseUint({input:?}, 10, 32)"
            );
        }
    }

    #[test]
    fn test_parse_uint64() {
        for &(input, want, want_err) in PARSE_UINT64_TESTS {
            let (out, err) = parse_uint(input, 10, 64);
            assert_eq!(
                (out, kind(err)),
                (want, want_err),
                "ParseUint({input:?}, 10, 64)"
            );
        }
    }

    #[test]
    fn test_parse_uint64_base() {
        for &(input, base, want, want_err) in PARSE_UINT64_BASE_TESTS {
            let (out, err) = parse_uint(input, base, 64);
            assert_eq!(
                (out, kind(err)),
                (want, want_err),
                "ParseUint({input:?}, {base}, 64)"
            );
        }
    }

    #[test]
    fn test_parse_int32() {
        for &(input, want, want_err) in PARSE_INT32_TESTS {
            let (out, err) = parse_int(input, 10, 32);
            assert_eq!(
                (out, kind(err)),
                (want, want_err),
                "ParseInt({input:?}, 10, 32)"
            );
        }
    }

    #[test]
    fn test_parse_int64() {
        for &(input, want, want_err) in PARSE_INT64_TESTS {
            let (out, err) = parse_int(input, 10, 64);
            assert_eq!(
                (out, kind(err)),
                (want, want_err),
                "ParseInt({input:?}, 10, 64)"
            );
        }
    }

    #[test]
    fn test_parse_int64_base() {
        for &(input, base, want, want_err) in PARSE_INT64_BASE_TESTS {
            let (out, err) = parse_int(input, base, 64);
            assert_eq!(
                (out, kind(err)),
                (want, want_err),
                "ParseInt({input:?}, {base}, 64)"
            );
        }
    }

    #[test]
    fn test_parse_uint() {
        // IntSize == 64
        for &(input, want, want_err) in PARSE_UINT64_TESTS {
            let (out, err) = parse_uint(input, 10, 0);
            assert_eq!(
                (out, kind(err)),
                (want, want_err),
                "ParseUint({input:?}, 10, 0)"
            );
        }
    }

    #[test]
    fn test_parse_int() {
        // IntSize == 64
        for &(input, want, want_err) in PARSE_INT64_TESTS {
            let (out, err) = parse_int(input, 10, 0);
            assert_eq!(
                (out, kind(err)),
                (want, want_err),
                "ParseInt({input:?}, 10, 0)"
            );
        }
    }

    #[test]
    fn test_atoi() {
        // IntSize == 64
        for &(input, want, want_err) in PARSE_INT64_TESTS {
            let (out, err) = atoi(input);
            assert_eq!((out, kind(err)), (want, want_err), "Atoi({input:?})");
        }
    }

    const PARSE_BIT_SIZE_TESTS: &[(i64, Option<NumErrorKind>)] = &[
        (-1, Some(NumErrorKind::BitSize(-1))),
        (0, None),
        (64, None),
        (65, Some(NumErrorKind::BitSize(65))),
    ];

    const PARSE_BASE_TESTS: &[(i64, Option<NumErrorKind>)] = &[
        (-1, Some(NumErrorKind::Base(-1))),
        (0, None),
        (1, Some(NumErrorKind::Base(1))),
        (2, None),
        (36, None),
        (37, Some(NumErrorKind::Base(37))),
    ];

    #[test]
    fn test_parse_int_bit_size() {
        for &(arg, want) in PARSE_BIT_SIZE_TESTS {
            let (_, err) = parse_int("0", 0, arg);
            assert_eq!(kind(err), want, "ParseInt(\"0\", 0, {arg})");
        }
    }

    #[test]
    fn test_parse_uint_bit_size() {
        for &(arg, want) in PARSE_BIT_SIZE_TESTS {
            let (_, err) = parse_uint("0", 0, arg);
            assert_eq!(kind(err), want, "ParseUint(\"0\", 0, {arg})");
        }
    }

    #[test]
    fn test_parse_int_base() {
        for &(arg, want) in PARSE_BASE_TESTS {
            let (_, err) = parse_int("0", arg, 0);
            assert_eq!(kind(err), want, "ParseInt(\"0\", {arg}, 0)");
        }
    }

    #[test]
    fn test_parse_uint_base() {
        for &(arg, want) in PARSE_BASE_TESTS {
            let (_, err) = parse_uint("0", arg, 0);
            assert_eq!(kind(err), want, "ParseUint(\"0\", {arg}, 0)");
        }
    }
}
