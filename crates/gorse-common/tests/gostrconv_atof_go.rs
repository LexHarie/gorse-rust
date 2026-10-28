// Ports: $GOROOT/src/internal/strconv/atof_test.go, fp_test.go (TestFp, TestParseFloatTestdata), decimal_test.go, math_test.go, atob_test.go, and $GOROOT/src/strconv/number_test.go (TestParseBool, TestFormatBool, TestAppendBool, TestAtof) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strconv tests: Copyright 2009 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's float and bool parsing tests, case for case.
//!
//! - Go's tests compare `FormatFloat(out, 'g', -1, size)` with the expected
//!   text; gostrconv has no `FormatFloat`, so they use the test-local one in
//!   `strconv_format` (checked against Go's own output by
//!   `gostrconv_golden.rs`).
//! - Go flips a package variable with `SetOptimize`; these call
//!   `parse_float_with_optimize` instead, so parallel tests cannot race on it.
//!   (Go's `SetOptimize` also switches `FormatFloat`'s fast path, which the
//!   test-local formatter does not have.)
//! - Go's internal tests compare the bare `ErrSyntax`/`ErrRange`; the public
//!   `parse_float` wraps them, so these compare the whole `NumError`
//!   (`ParseFloat`, the input, the reason), as Go's public `TestAtof` does.
//! - `TestAtofRandom` draws its bits from a seeded generator; Go's
//!   `math/rand` is randomly seeded.

mod strconv_format;

mod gostrconv {
    use gorse_common::gostrconv::NumErrorKind::{self, Range, Syntax};
    use gorse_common::gostrconv::decimal::Decimal;
    use gorse_common::gostrconv::uscale::{log2_pow10, log10_pow2};
    use gorse_common::gostrconv::{
        NumError, append_bool, atoi, format_bool, parse_bool, parse_float, parse_float_prefix,
        parse_float_with_optimize, parse_int,
    };

    use crate::strconv_format::{
        format_b32, format_b64, format_e32, format_e64, format_g32, format_g64,
    };

    type AtofTest = (String, &'static str, Option<NumErrorKind>);

    fn t(input: &str, out: &'static str, err: Option<NumErrorKind>) -> AtofTest {
        (input.to_owned(), out, err)
    }

    fn atoftests() -> Vec<AtofTest> {
        vec![
            t("", "0", Some(Syntax)),
            t("1", "1", None),
            t("+1", "1", None),
            t("1x", "0", Some(Syntax)),
            t("1.1.", "0", Some(Syntax)),
            t("1e23", "1e+23", None),
            t("1E23", "1e+23", None),
            t("100000000000000000000000", "1e+23", None),
            t("1e-100", "1e-100", None),
            t("123456700", "1.234567e+08", None),
            t("99999999999999974834176", "9.999999999999997e+22", None),
            t("100000000000000000000001", "1.0000000000000001e+23", None),
            t("100000000000000008388608", "1.0000000000000001e+23", None),
            t("100000000000000016777215", "1.0000000000000001e+23", None),
            t("100000000000000016777216", "1.0000000000000003e+23", None),
            t("-1", "-1", None),
            t("-0.1", "-0.1", None),
            t("-0", "-0", None),
            t("1e-20", "1e-20", None),
            t("625e-3", "0.625", None),
            // Hexadecimal floating-point.
            t("0x1p0", "1", None),
            t("0x1p1", "2", None),
            t("0x1p-1", "0.5", None),
            t("0x1ep-1", "15", None),
            t("-0x1ep-1", "-15", None),
            t("-0x1_ep-1", "-15", None),
            t("0x1p-200", "6.223015277861142e-61", None),
            t("0x1p200", "1.6069380442589903e+60", None),
            t("0x1fFe2.p0", "131042", None),
            t("0x1fFe2.P0", "131042", None),
            t("-0x2p3", "-16", None),
            t("0x0.fp4", "15", None),
            t("0x0.fp0", "0.9375", None),
            t("0x1e2", "0", Some(Syntax)),
            t("1p2", "0", Some(Syntax)),
            // zeros
            t("0", "0", None),
            t("0e0", "0", None),
            t("-0e0", "-0", None),
            t("+0e0", "0", None),
            t("0e-0", "0", None),
            t("-0e-0", "-0", None),
            t("+0e-0", "0", None),
            t("0e+0", "0", None),
            t("-0e+0", "-0", None),
            t("+0e+0", "0", None),
            t("0e+01234567890123456789", "0", None),
            t("0.00e-01234567890123456789", "0", None),
            t("-0e+01234567890123456789", "-0", None),
            t("-0.00e-01234567890123456789", "-0", None),
            t("0x0p+01234567890123456789", "0", None),
            t("0x0.00p-01234567890123456789", "0", None),
            t("-0x0p+01234567890123456789", "-0", None),
            t("-0x0.00p-01234567890123456789", "-0", None),
            t("0e291", "0", None), // issue 15364
            t("0e292", "0", None), // issue 15364
            t("0e347", "0", None), // issue 15364
            t("0e348", "0", None), // issue 15364
            t("-0e291", "-0", None),
            t("-0e292", "-0", None),
            t("-0e347", "-0", None),
            t("-0e348", "-0", None),
            t("0x0p126", "0", None),
            t("0x0p127", "0", None),
            t("0x0p128", "0", None),
            t("0x0p129", "0", None),
            t("0x0p130", "0", None),
            t("0x0p1022", "0", None),
            t("0x0p1023", "0", None),
            t("0x0p1024", "0", None),
            t("0x0p1025", "0", None),
            t("0x0p1026", "0", None),
            t("-0x0p126", "-0", None),
            t("-0x0p127", "-0", None),
            t("-0x0p128", "-0", None),
            t("-0x0p129", "-0", None),
            t("-0x0p130", "-0", None),
            t("-0x0p1022", "-0", None),
            t("-0x0p1023", "-0", None),
            t("-0x0p1024", "-0", None),
            t("-0x0p1025", "-0", None),
            t("-0x0p1026", "-0", None),
            // NaNs
            t("nan", "NaN", None),
            t("NaN", "NaN", None),
            t("NAN", "NaN", None),
            // Infs
            t("inf", "+Inf", None),
            t("-Inf", "-Inf", None),
            t("+INF", "+Inf", None),
            t("-Infinity", "-Inf", None),
            t("+INFINITY", "+Inf", None),
            t("Infinity", "+Inf", None),
            // largest float64
            t("1.7976931348623157e308", "1.7976931348623157e+308", None),
            t("-1.7976931348623157e308", "-1.7976931348623157e+308", None),
            t("0x1.fffffffffffffp1023", "1.7976931348623157e+308", None),
            t("-0x1.fffffffffffffp1023", "-1.7976931348623157e+308", None),
            t("0x1fffffffffffffp+971", "1.7976931348623157e+308", None),
            t("-0x1fffffffffffffp+971", "-1.7976931348623157e+308", None),
            t("0x.1fffffffffffffp1027", "1.7976931348623157e+308", None),
            t("-0x.1fffffffffffffp1027", "-1.7976931348623157e+308", None),
            // next float64 - too large
            t("1.7976931348623159e308", "+Inf", Some(Range)),
            t("-1.7976931348623159e308", "-Inf", Some(Range)),
            t("0x1p1024", "+Inf", Some(Range)),
            t("-0x1p1024", "-Inf", Some(Range)),
            t("0x2p1023", "+Inf", Some(Range)),
            t("-0x2p1023", "-Inf", Some(Range)),
            t("0x.1p1028", "+Inf", Some(Range)),
            t("-0x.1p1028", "-Inf", Some(Range)),
            t("0x.2p1027", "+Inf", Some(Range)),
            t("-0x.2p1027", "-Inf", Some(Range)),
            // the border is ...158079
            // borderline - okay
            t("1.7976931348623158e308", "1.7976931348623157e+308", None),
            t("-1.7976931348623158e308", "-1.7976931348623157e+308", None),
            t(
                "0x1.fffffffffffff7fffp1023",
                "1.7976931348623157e+308",
                None,
            ),
            t(
                "-0x1.fffffffffffff7fffp1023",
                "-1.7976931348623157e+308",
                None,
            ),
            // borderline - too large
            t("1.797693134862315808e308", "+Inf", Some(Range)),
            t("-1.797693134862315808e308", "-Inf", Some(Range)),
            t("0x1.fffffffffffff8p1023", "+Inf", Some(Range)),
            t("-0x1.fffffffffffff8p1023", "-Inf", Some(Range)),
            t("0x1fffffffffffff.8p+971", "+Inf", Some(Range)),
            t("-0x1fffffffffffff8p+967", "-Inf", Some(Range)),
            t("0x.1fffffffffffff8p1027", "+Inf", Some(Range)),
            t("-0x.1fffffffffffff9p1027", "-Inf", Some(Range)),
            // a little too large
            t("1e308", "1e+308", None),
            t("2e308", "+Inf", Some(Range)),
            t("1e309", "+Inf", Some(Range)),
            t("0x1p1025", "+Inf", Some(Range)),
            // way too large
            t("1e310", "+Inf", Some(Range)),
            t("-1e310", "-Inf", Some(Range)),
            t("1e400", "+Inf", Some(Range)),
            t("-1e400", "-Inf", Some(Range)),
            t("1e400000", "+Inf", Some(Range)),
            t("-1e400000", "-Inf", Some(Range)),
            t("0x1p1030", "+Inf", Some(Range)),
            t("0x1p2000", "+Inf", Some(Range)),
            t("0x1p2000000000", "+Inf", Some(Range)),
            t("-0x1p1030", "-Inf", Some(Range)),
            t("-0x1p2000", "-Inf", Some(Range)),
            t("-0x1p2000000000", "-Inf", Some(Range)),
            // denormalized
            t("1e-305", "1e-305", None),
            t("1e-306", "1e-306", None),
            t("1e-307", "1e-307", None),
            t("1e-308", "1e-308", None),
            t("1e-309", "1e-309", None),
            t("1e-310", "1e-310", None),
            t("1e-322", "1e-322", None),
            // smallest denormal
            t("5e-324", "5e-324", None),
            t("4e-324", "5e-324", None),
            t("3e-324", "5e-324", None),
            // too small
            t("2e-324", "0", None),
            // way too small
            t("1e-350", "0", None),
            t("1e-400000", "0", None),
            t("1e-345", "0", None), // picked off in atof64
            t("1e-343", "0", None), // large c.s in parseFloat64
            t("9.999999999999999999e-343", "0", None),
            // Near denormals and denormals.
            t("0x2.00000000000000p-1010", "1.8227805048890994e-304", None), // 0x00e0000000000000
            t("0x1.fffffffffffff0p-1010", "1.8227805048890992e-304", None), // 0x00dfffffffffffff
            t("0x1.fffffffffffff7p-1010", "1.8227805048890992e-304", None), // rounded down
            t("0x1.fffffffffffff8p-1010", "1.8227805048890994e-304", None), // rounded up
            t("0x1.fffffffffffff9p-1010", "1.8227805048890994e-304", None), // rounded up
            t("0x2.00000000000000p-1022", "4.450147717014403e-308", None),  // 0x0020000000000000
            t("0x1.fffffffffffff0p-1022", "4.4501477170144023e-308", None), // 0x001fffffffffffff
            t("0x1.fffffffffffff7p-1022", "4.4501477170144023e-308", None), // rounded down
            t("0x1.fffffffffffff8p-1022", "4.450147717014403e-308", None),  // rounded up
            t("0x1.fffffffffffff9p-1022", "4.450147717014403e-308", None),  // rounded up
            t("0x1.00000000000000p-1022", "2.2250738585072014e-308", None), // 0x0010000000000000
            t("0x0.fffffffffffff0p-1022", "2.225073858507201e-308", None),  // 0x000fffffffffffff
            t("0x0.ffffffffffffe0p-1022", "2.2250738585072004e-308", None), // 0x000ffffffffffffe
            t("0x0.ffffffffffffe7p-1022", "2.2250738585072004e-308", None), // rounded down
            t("0x1.ffffffffffffe8p-1023", "2.225073858507201e-308", None),  // rounded up
            t("0x1.ffffffffffffe9p-1023", "2.225073858507201e-308", None),  // rounded up
            t("0x0.00000003fffff0p-1022", "2.072261e-317", None),           // 0x00000000003fffff
            t("0x0.00000003456780p-1022", "1.694649e-317", None),           // 0x0000000000345678
            t("0x0.00000003456787p-1022", "1.694649e-317", None),           // rounded down
            t("0x0.00000003456788p-1022", "1.694649e-317", None), // rounded down (half to even)
            t("0x0.00000003456790p-1022", "1.6946496e-317", None), // 0x0000000000345679
            t("0x0.00000003456789p-1022", "1.6946496e-317", None), // rounded up
            t(
                "0x0.0000000345678800000000000000000000000001p-1022",
                "1.6946496e-317",
                None,
            ), // rounded up
            t("0x0.000000000000f0p-1022", "7.4e-323", None),      // 0x000000000000000f
            t("0x0.00000000000060p-1022", "3e-323", None),        // 0x0000000000000006
            t("0x0.00000000000058p-1022", "3e-323", None),        // rounded up
            t("0x0.00000000000057p-1022", "2.5e-323", None),      // rounded down
            t("0x0.00000000000050p-1022", "2.5e-323", None),      // 0x0000000000000005
            t("0x0.00000000000010p-1022", "5e-324", None),        // 0x0000000000000001
            t("0x0.000000000000081p-1022", "5e-324", None),       // rounded up
            t("0x0.00000000000008p-1022", "0", None),             // rounded down
            t("0x0.00000000000007fp-1022", "0", None),            // rounded down
            // try to overflow exponent
            t("1e-4294967296", "0", None),
            t("1e+4294967296", "+Inf", Some(Range)),
            t("1e-18446744073709551616", "0", None),
            t("1e+18446744073709551616", "+Inf", Some(Range)),
            t("0x1p-4294967296", "0", None),
            t("0x1p+4294967296", "+Inf", Some(Range)),
            t("0x1p-18446744073709551616", "0", None),
            t("0x1p+18446744073709551616", "+Inf", Some(Range)),
            // Parse errors
            t("1e", "0", Some(Syntax)),
            t("1e-", "0", Some(Syntax)),
            t(".e-1", "0", Some(Syntax)),
            t("1\x00.2", "0", Some(Syntax)),
            t("0x", "0", Some(Syntax)),
            t("0x.", "0", Some(Syntax)),
            t("0x1", "0", Some(Syntax)),
            t("0x.1", "0", Some(Syntax)),
            t("0x1p", "0", Some(Syntax)),
            t("0x.1p", "0", Some(Syntax)),
            t("0x1p+", "0", Some(Syntax)),
            t("0x.1p+", "0", Some(Syntax)),
            t("0x1p-", "0", Some(Syntax)),
            t("0x.1p-", "0", Some(Syntax)),
            t("0x1p+2", "4", None),
            t("0x.1p+2", "0.25", None),
            t("0x1p-2", "0.25", None),
            t("0x.1p-2", "0.015625", None),
            // https://www.exploringbinary.com/java-hangs-when-converting-2-2250738585072012e-308/
            t("2.2250738585072012e-308", "2.2250738585072014e-308", None),
            // https://www.exploringbinary.com/php-hangs-on-numeric-value-2-2250738585072011e-308/
            t("2.2250738585072011e-308", "2.225073858507201e-308", None),
            // A very large number (initially wrongly parsed by the fast algorithm).
            t("4.630813248087435e+307", "4.630813248087435e+307", None),
            // A different kind of very large number.
            t("22.222222222222222", "22.22222222222222", None),
            t(
                &["2.", &"2".repeat(4000), "e+1"].concat(),
                "22.22222222222222",
                None,
            ),
            t("0x1.1111111111111p222", "7.18931911124017e+66", None),
            t("0x2.2222222222222p221", "7.18931911124017e+66", None),
            t(
                &["0x2.", &"2".repeat(4000), "p221"].concat(),
                "7.18931911124017e+66",
                None,
            ),
            // Exactly halfway between 1 and math.Nextafter(1, 2).
            // Round to even (down).
            t(
                "1.00000000000000011102230246251565404236316680908203125",
                "1",
                None,
            ),
            t("0x1.00000000000008p0", "1", None),
            // Slightly lower; still round down.
            t(
                "1.00000000000000011102230246251565404236316680908203124",
                "1",
                None,
            ),
            t("0x1.00000000000007Fp0", "1", None),
            // Slightly higher; round up.
            t(
                "1.00000000000000011102230246251565404236316680908203126",
                "1.0000000000000002",
                None,
            ),
            t("0x1.000000000000081p0", "1.0000000000000002", None),
            t("0x1.00000000000009p0", "1.0000000000000002", None),
            // Slightly higher, but you have to read all the way to the end.
            t(
                &[
                    "1.00000000000000011102230246251565404236316680908203125",
                    &"0".repeat(10000),
                    "1",
                ]
                .concat(),
                "1.0000000000000002",
                None,
            ),
            t(
                &["0x1.00000000000008", &"0".repeat(10000), "1p0"].concat(),
                "1.0000000000000002",
                None,
            ),
            // Halfway between x := math.Nextafter(1, 2) and math.Nextafter(x, 2)
            // Round to even (up).
            t(
                "1.00000000000000033306690738754696212708950042724609375",
                "1.0000000000000004",
                None,
            ),
            t("0x1.00000000000018p0", "1.0000000000000004", None),
            // Halfway between 1090544144181609278303144771584 and 1090544144181609419040633126912
            // (15497564393479157p+46, should round to even 15497564393479156p+46, issue 36657)
            t(
                "1090544144181609348671888949248",
                "1.0905441441816093e+30",
                None,
            ),
            // slightly above, rounds up
            t(
                "1090544144181609348835077142190",
                "1.0905441441816094e+30",
                None,
            ),
            // Underscores.
            t("1_23.50_0_0e+1_2", "1.235e+14", None),
            t("-_123.5e+12", "0", Some(Syntax)),
            t("+_123.5e+12", "0", Some(Syntax)),
            t("_123.5e+12", "0", Some(Syntax)),
            t("1__23.5e+12", "0", Some(Syntax)),
            t("123_.5e+12", "0", Some(Syntax)),
            t("123._5e+12", "0", Some(Syntax)),
            t("123.5_e+12", "0", Some(Syntax)),
            t("123.5__0e+12", "0", Some(Syntax)),
            t("123.5e_+12", "0", Some(Syntax)),
            t("123.5e+_12", "0", Some(Syntax)),
            t("123.5e_-12", "0", Some(Syntax)),
            t("123.5e-_12", "0", Some(Syntax)),
            t("123.5e+1__2", "0", Some(Syntax)),
            t("123.5e+12_", "0", Some(Syntax)),
            t("0x_1_2.3_4_5p+1_2", "74565", None),
            t("-_0x12.345p+12", "0", Some(Syntax)),
            t("+_0x12.345p+12", "0", Some(Syntax)),
            t("_0x12.345p+12", "0", Some(Syntax)),
            t("0x__12.345p+12", "0", Some(Syntax)),
            t("0x1__2.345p+12", "0", Some(Syntax)),
            t("0x12_.345p+12", "0", Some(Syntax)),
            t("0x12._345p+12", "0", Some(Syntax)),
            t("0x12.3__45p+12", "0", Some(Syntax)),
            t("0x12.345_p+12", "0", Some(Syntax)),
            t("0x12.345p_+12", "0", Some(Syntax)),
            t("0x12.345p+_12", "0", Some(Syntax)),
            t("0x12.345p_-12", "0", Some(Syntax)),
            t("0x12.345p-_12", "0", Some(Syntax)),
            t("0x12.345p+1__2", "0", Some(Syntax)),
            t("0x12.345p+12_", "0", Some(Syntax)),
            t("1e100x", "0", Some(Syntax)),
            t("1e1000x", "0", Some(Syntax)),
        ]
    }

    fn atof32tests() -> Vec<AtofTest> {
        vec![
            // Hex
            t("0x1p-100", "7.888609e-31", None),
            t("0x1p100", "1.2676506e+30", None),
            // Exactly halfway between 1 and the next float32.
            // Round to even (down).
            t("1.000000059604644775390625", "1", None),
            t("0x1.000001p0", "1", None),
            // Slightly lower.
            t("1.000000059604644775390624", "1", None),
            t("0x1.0000008p0", "1", None),
            t("0x1.000000fp0", "1", None),
            // Slightly higher.
            t("1.000000059604644775390626", "1.0000001", None),
            t("0x1.000002p0", "1.0000001", None),
            t("0x1.0000018p0", "1.0000001", None),
            t("0x1.0000011p0", "1.0000001", None),
            // Slightly higher, but you have to read all the way to the end.
            t(
                &["1.000000059604644775390625", &"0".repeat(10000), "1"].concat(),
                "1.0000001",
                None,
            ),
            t(
                &["0x1.000001", &"0".repeat(10000), "1p0"].concat(),
                "1.0000001",
                None,
            ),
            // largest float32: (1<<128) * (1 - 2^-24)
            t(
                "340282346638528859811704183484516925440",
                "3.4028235e+38",
                None,
            ),
            t(
                "-340282346638528859811704183484516925440",
                "-3.4028235e+38",
                None,
            ),
            t("0x.ffffffp128", "3.4028235e+38", None),
            t(
                "-340282346638528859811704183484516925440",
                "-3.4028235e+38",
                None,
            ),
            t("-0x.ffffffp128", "-3.4028235e+38", None),
            // next float32 - too large
            t("3.4028236e38", "+Inf", Some(Range)),
            t("-3.4028236e38", "-Inf", Some(Range)),
            t("0x1.0p128", "+Inf", Some(Range)),
            t("-0x1.0p128", "-Inf", Some(Range)),
            // the border is 3.40282356779...e+38
            // borderline - okay
            t("3.402823567e38", "3.4028235e+38", None),
            t("-3.402823567e38", "-3.4028235e+38", None),
            t("0x.ffffff7fp128", "3.4028235e+38", None),
            t("-0x.ffffff7fp128", "-3.4028235e+38", None),
            // borderline - too large
            t("3.4028235678e38", "+Inf", Some(Range)),
            t("-3.4028235678e38", "-Inf", Some(Range)),
            t("0x.ffffff8p128", "+Inf", Some(Range)),
            t("-0x.ffffff8p128", "-Inf", Some(Range)),
            // Denormals: less than 2^-126
            t("1e-38", "1e-38", None),
            t("1e-39", "1e-39", None),
            t("1e-40", "1e-40", None),
            t("1e-41", "1e-41", None),
            t("1e-42", "1e-42", None),
            t("1e-43", "1e-43", None),
            t("1e-44", "1e-44", None),
            t("6e-45", "6e-45", None), // 4p-149 = 5.6e-45
            t("5e-45", "6e-45", None),
            // Smallest denormal
            t("1e-45", "1e-45", None), // 1p-149 = 1.4e-45
            t("2e-45", "1e-45", None),
            t("3e-45", "3e-45", None),
            // Near denormals and denormals.
            t("0x0.89aBcDp-125", "1.2643093e-38", None), // 0x0089abcd
            t("0x0.8000000p-125", "1.1754944e-38", None), // 0x00800000
            t("0x0.1234560p-125", "1.671814e-39", None), // 0x00123456
            t("0x0.1234567p-125", "1.671814e-39", None), // rounded down
            t("0x0.1234568p-125", "1.671814e-39", None), // rounded down
            t("0x0.1234569p-125", "1.671815e-39", None), // rounded up
            t("0x0.1234570p-125", "1.671815e-39", None), // 0x00123457
            t("0x0.0000010p-125", "1e-45", None),        // 0x00000001
            t("0x0.00000081p-125", "1e-45", None),       // rounded up
            t("0x0.0000008p-125", "0", None),            // rounded down
            t("0x0.0000007p-125", "0", None),            // rounded down
            t("1e-70", "0", None),                       // picked off in atof32
            t("1e-65", "0", None),                       // picked off in atof32
            t("1e-64", "0", None),                       // large c.s in parseFloat32
            t("9.999999999999999999e-64", "0", None),
            // 2^92 = 8388608p+69 = 4951760157141521099596496896 (4.9517602e27)
            // is an exact power of two that needs 8 decimal digits to be correctly
            // parsed back.
            // The float32 before is 16777215p+68 = 4.95175986e+27
            // The halfway is 4.951760009. A bad algorithm that thinks the previous
            // float32 is 8388607p+69 will shorten incorrectly to 4.95176e+27.
            t("4951760157141521099596496896", "4.9517602e+27", None),
        ]
    }

    /// The `*NumError` Go's public `ParseFloat` returns for `err`.
    fn float_err(input: &str, err: Option<NumErrorKind>) -> Option<NumError> {
        err.map(|err| NumError {
            func: "ParseFloat",
            num: input.as_bytes().to_vec(),
            err,
        })
    }

    /// Go's error lines print the input; the long ones are cut short here.
    fn short(s: &str) -> String {
        if s.len() <= 80 {
            return format!("{s:?}");
        }
        format!("{:?}...({} bytes)", &s[..80], s.len())
    }

    #[test]
    fn test_parse_float_prefix() {
        let mut failures = Vec::new();
        for (input, _, err) in atoftests() {
            if err.is_some() {
                continue;
            }
            // Adding characters that do not extend a number should not invalidate it.
            // Test a few. The "i" and "init" cases test that we accept "infi", "infinit"
            // correctly as "inf" with suffix.
            for suffix in [" ", "q", "+", "-", "<", "=", ">", "(", ")", "i", "init"] {
                let s = format!("{input}{suffix}");
                let (_, n, err) = parse_float_prefix(&s, 64);
                if let Some(err) = err {
                    failures.push(format!(
                        "ParseFloatPrefix({}, 64): err = {err}; want no error",
                        short(&s)
                    ));
                }
                if n != input.len() {
                    failures.push(format!(
                        "ParseFloatPrefix({}, 64): n = {n}; want {}",
                        short(&s),
                        input.len()
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    fn test_atof(opt: bool) {
        let mut failures = Vec::new();
        for (input, out, want_err) in atoftests() {
            let (out64, err) = parse_float_with_optimize(&input, 64, opt);
            let outs = format_g64(out64);
            if outs != out || err != float_err(&input, want_err) {
                failures.push(format!(
                    "ParseFloat({}, 64) = {outs}, {err:?} want {out}, {want_err:?}",
                    short(&input)
                ));
            }

            if f64::from(out64 as f32) == out64 {
                let (got, err) = parse_float_with_optimize(&input, 32, opt);
                let out32 = got as f32;
                if f64::from(out32) != got {
                    failures.push(format!(
                        "ParseFloat({}, 32) = {got}, not a float32 (closest is {})",
                        short(&input),
                        f64::from(out32)
                    ));
                    continue;
                }
                let outs = format_g32(out32);
                if outs != out || err != float_err(&input, want_err) {
                    failures.push(format!(
                        "ParseFloat({}, 32) = {outs}, {err:?} want {out}, {want_err:?}  # {got}",
                        short(&input)
                    ));
                }
            }
        }
        for (input, out, want_err) in atof32tests() {
            let (out64, err) = parse_float_with_optimize(&input, 32, opt);
            let out32 = out64 as f32;
            if f64::from(out32) != out64 {
                failures.push(format!(
                    "ParseFloat({}, 32) = {out64}, not a float32 (closest is {})",
                    short(&input),
                    f64::from(out32)
                ));
                continue;
            }
            let outs = format_g32(out32);
            if outs != out || err != float_err(&input, want_err) {
                failures.push(format!(
                    "ParseFloat({}, 32) = {outs}, {err:?} want {out}, {want_err:?}  # {out64}",
                    short(&input)
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_atof_fast() {
        test_atof(true);
    }

    #[test]
    fn test_atof_slow() {
        test_atof(false);
    }

    /// splitmix64, for `TestAtofRandom`'s bits.
    struct SplitMix64(u64);

    impl SplitMix64 {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        }
    }

    #[test]
    fn test_atof_random() {
        let mut rng = SplitMix64(20261019);
        let tests: Vec<(f64, String)> = (0..10000)
            .map(|_| {
                let x = f64::from_bits(rng.next());
                (x, format_g64(x))
            })
            .collect();
        let mut failures = Vec::new();
        for (want, s) in &tests {
            let (x, _) = parse_float(s, 64);
            if !(x == *want || want.is_nan() && x.is_nan()) {
                failures.push(format!(
                    "number {s} badly parsed as {} (expected {})",
                    format_b64(x),
                    format_b64(*want)
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        println!("tested {} random numbers", tests.len());
    }

    // Issue 2917.
    // This test will break the optimized conversion if the
    // FPU is using 80-bit registers instead of 64-bit registers,
    // usually because the operating system initialized the
    // thread with 80-bit precision and the Go runtime didn't
    // fix the FP control word.
    fn round_trip_cases() -> [(f64, &'static str); 2] {
        let shift = 2f64.powi(39); // exact: 8865794286000691 << 39
        [
            (8865794286000691.0 * shift, "4.87402195346389e+27"),
            (8865794286000692.0 * shift, "4.8740219534638903e+27"),
        ]
    }

    #[test]
    fn test_round_trip() {
        for (f, want) in round_trip_cases() {
            let s = format_g64(f);
            assert_eq!(s, want, "no-opt FormatFloat({})", format_b64(f));
            let (got, err) = parse_float_with_optimize(want, 64, false);
            assert!(
                got == f && err.is_none(),
                "no-opt ParseFloat({want}) = {}, {err:?} want {}, nil",
                format_b64(got),
                format_b64(f)
            );
            let s = format_g64(f);
            assert_eq!(s, want, "opt FormatFloat({})", format_b64(f));
            let (got, err) = parse_float_with_optimize(want, 64, true);
            assert!(
                got == f && err.is_none(),
                "opt ParseFloat({want}) = {}, {err:?} want {}, nil",
                format_b64(got),
                format_b64(f)
            );
        }
    }

    /// `TestRoundTrip32` tries a fraction of all finite positive float32 values.
    #[test]
    fn test_round_trip32() {
        let step = 997u32;
        let mut count = 0;
        let mut failures = Vec::new();
        let mut i = 0u32;
        while i < 0xff << 23 {
            let mut f = f32::from_bits(i);
            if i & 1 == 1 {
                f = -f; // negative
            }
            let s = format_g32(f);

            let (parsed, err) = parse_float(&s, 32);
            let parsed32 = parsed as f32;
            if let Some(err) = err {
                failures.push(format!("ParseFloat({s:?}, 32) gave error {err}"));
            } else if f64::from(parsed32) != parsed {
                failures.push(format!(
                    "ParseFloat({s:?}, 32) = {parsed}, not a float32 (nearest is {parsed32})"
                ));
            } else if parsed32 != f {
                failures.push(format!(
                    "ParseFloat({s:?}, 32) = {} (expected {})",
                    format_b32(parsed32),
                    format_b32(f)
                ));
            }
            count += 1;
            i += step;
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        println!("tested {count} float32's");
    }

    // Issue 42297: a lot of code in the wild accidentally calls ParseFloat(s, 10)
    // or ParseFloat(s, 0), so allow bitSize values other than 32 and 64.
    #[test]
    fn test_parse_float_incorrect_bit_size() {
        const S: &str = "1.5e308";
        const WANT: f64 = 1.5e308;

        for bit_size in [0, 10, 100, 128] {
            let (f, err) = parse_float(S, bit_size);
            if let Some(err) = err {
                panic!("ParseFloat({S:?}, {bit_size}) gave error {err}");
            }
            assert_eq!(f, WANT, "ParseFloat({S:?}, {bit_size})");
        }
    }

    /// A reproduced go1.27 quirk: decimal.set keeps at most 800 digits, and
    /// a mantissa without a point takes its decimal point from that capped
    /// count, so once a nonzero digit past the cap sends the input down the
    /// slow path the point lands in the wrong place. go1.27.0's
    /// ParseFloat(s, 64) gives these bits (measured by the integrator, and
    /// recorded again in strconv_golden.json):
    ///
    /// - "9007199254740993"+"0"*790+"1e-791" gives 41cad7f29abcaf49
    ///   (9.007199254740993e+08, not 9007199254740993)
    /// - "9007199254740993"+"0"*900+"1e-901" gives 2af42c7ff0054685
    ///   (9.007199254740992e-102)
    /// - "9007199254740993"+"0"*700+"1e-701" gives 4340000000000001 (under
    ///   the cap, so right)
    /// - "0."+"0"*900+"1e-901" gives 0
    #[test]
    fn test_go127_800_digit_quirk() {
        let cases = [
            (
                ["9007199254740993", &"0".repeat(790), "1e-791"].concat(),
                0x41cad7f29abcaf49u64,
            ),
            (
                ["9007199254740993", &"0".repeat(900), "1e-901"].concat(),
                0x2af42c7ff0054685,
            ),
            (
                ["9007199254740993", &"0".repeat(700), "1e-701"].concat(),
                0x4340000000000001,
            ),
            (["0.", &"0".repeat(900), "1e-901"].concat(), 0),
        ];
        for (s, bits) in cases {
            for opt in [true, false] {
                let (f, err) = parse_float_with_optimize(&s, 64, opt);
                assert!(err.is_none(), "ParseFloat({}, 64): {err:?}", short(&s));
                assert_eq!(
                    f.to_bits(),
                    bits,
                    "ParseFloat({}, 64) optimize={opt} = {f:e}",
                    short(&s)
                );
            }
        }
    }

    // Ports: strconv/number_test.go atoftests and TestAtof (the public
    // ParseFloat, whose errors are *NumError).
    #[test]
    fn test_public_atof() {
        let atoftests: &[(&str, &str, Option<NumErrorKind>)] = &[
            ("", "0", Some(Syntax)),
            ("1.25", "1.25", None),
            ("+1", "1", None),
            ("1x", "0", Some(Syntax)),
            ("1.1.", "0", Some(Syntax)),
            ("1e23", "1e+23", None),
            ("1E23", "1e+23", None),
            ("0x1fFe2.p0", "131042", None),
            ("0x1fFe2.P0", "131042", None),
            ("-0x2p3", "-16", None),
            ("0x0.fp4", "15", None),
            ("0x0.fp0", "0.9375", None),
            ("0x1e2", "0", Some(Syntax)),
            ("1p2", "0", Some(Syntax)),
            ("0x1p1024", "+Inf", Some(Range)),
            ("-0x1p1024", "-Inf", Some(Range)),
            (
                "0x1.fffffffffffff7fffp1023",
                "1.7976931348623157e+308",
                None,
            ),
            (
                "-0x1.fffffffffffff7fffp1023",
                "-1.7976931348623157e+308",
                None,
            ),
            ("1.797693134862315808e308", "+Inf", Some(Range)),
            ("-1.797693134862315808e308", "-Inf", Some(Range)),
        ];
        for &(input, out, want_err) in atoftests {
            let (got, err) = parse_float(input, 64);
            let outs = format_g64(got);
            assert!(
                outs == out && err == float_err(input, want_err),
                "ParseFloat({input}, 64) = {got}, {err:?} want {out}, {want_err:?}"
            );

            if f64::from(got as f32) == got {
                let (got, err) = parse_float(input, 32);
                let out32 = got as f32;
                assert!(
                    f64::from(out32) == got,
                    "ParseFloat({input}, 32) = {got}, not a float32 (closest is {})",
                    f64::from(out32)
                );
                let outs = format_g32(out32);
                assert!(
                    outs == out && err == float_err(input, want_err),
                    "ParseFloat({input}, 32) = {out32}, {err:?} want {out}, {want_err:?}  # {got}"
                );
            }
        }
    }

    fn pow2(i: i64) -> f64 {
        match i {
            _ if i < 0 => 1.0 / pow2(-i),
            0 => 1.0,
            1 => 2.0,
            _ => pow2(i / 2) * pow2(i - i / 2),
        }
    }

    /// Wrapper around ParseFloat(x, 64).  Handles dddddp+ddd (binary exponent)
    /// itself, passes the rest on to ParseFloat.
    fn myatof64(s: &str) -> Option<f64> {
        if let Some((mant, exp)) = s.split_once('p') {
            let (n, err) = parse_int(mant, 10, 64);
            if err.is_some() {
                return None;
            }
            let (e, err1) = atoi(exp);
            if err1.is_some() {
                println!("bad e {exp}");
                return None;
            }
            let mut e = e;
            let mut v = n as f64;
            // We expect that v*pow2(e) fits in a float64,
            // but pow2(e) by itself may not. Be careful.
            if e <= -1000 {
                v *= pow2(-1000);
                e += 1000;
                while e < 0 {
                    v /= 2.0;
                    e += 1;
                }
                return Some(v);
            }
            if e >= 1000 {
                v *= pow2(1000);
                e -= 1000;
                while e > 0 {
                    v *= 2.0;
                    e -= 1;
                }
                return Some(v);
            }
            return Some(v * pow2(e));
        }
        let (f1, err) = parse_float(s, 64);
        if err.is_some() {
            return None;
        }
        Some(f1)
    }

    /// Wrapper around strconv.ParseFloat(x, 32).  Handles dddddp+ddd (binary exponent)
    /// itself, passes the rest on to strconv.ParseFloat.
    fn myatof32(s: &str) -> Option<f32> {
        if let Some((mant, exp)) = s.split_once('p') {
            let (n, err) = atoi(mant);
            if err.is_some() {
                println!("bad n {mant}");
                return None;
            }
            let (e, err1) = atoi(exp);
            if err1.is_some() {
                println!("bad p {exp}");
                return None;
            }
            return Some((n as f64 * pow2(e)) as f32);
        }
        let (f64v, err1) = parse_float(s, 32);
        let f1 = f64v as f32;
        if err1.is_some() {
            return None;
        }
        Some(f1)
    }

    /// `fmt.Sprintf(format, v)` for the formats testfp.txt uses: %b and %.Ne.
    fn sprintf64(format: &str, v: f64) -> String {
        if format == "%b" {
            return format_b64(v);
        }
        let prec = format
            .strip_prefix("%.")
            .and_then(|f| f.strip_suffix('e'))
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| panic!("format {format} not in testfp.txt's set"));
        format_e64(v, prec)
    }

    fn sprintf32(format: &str, v: f32) -> String {
        if format == "%b" {
            return format_b32(v);
        }
        let prec = format
            .strip_prefix("%.")
            .and_then(|f| f.strip_suffix('e'))
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| panic!("format {format} not in testfp.txt's set"));
        format_e32(v, prec)
    }

    fn testdata(name: &str) -> String {
        let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    #[test]
    fn test_fp() {
        let testfp = testdata("testfp.txt");
        let mut failures = Vec::new();
        let mut checked = 0;
        for (lineno, line) in testfp.split_inclusive('\n').enumerate() {
            let lineno = lineno + 1;
            let line = line.split_once('#').map_or(line, |(l, _)| l);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let a: Vec<&str> = line.split(' ').collect();
            if a.len() != 4 {
                failures.push(format!("testdata/testfp.txt:{lineno}: wrong field count"));
                continue;
            }
            let s = match a[0] {
                "float64" => {
                    let Some(v) = myatof64(a[2]) else {
                        failures.push(format!(
                            "testdata/testfp.txt:{lineno}: cannot atof64 {}",
                            a[2]
                        ));
                        continue;
                    };
                    sprintf64(a[1], v)
                }
                "float32" => {
                    let Some(v1) = myatof32(a[2]) else {
                        failures.push(format!(
                            "testdata/testfp.txt:{lineno}: cannot atof32 {}",
                            a[2]
                        ));
                        continue;
                    };
                    sprintf32(a[1], v1)
                }
                _ => String::new(),
            };
            if s != a[3] {
                failures.push(format!(
                    "testdata/testfp.txt:{lineno}: {} {} {} {}: have {s} want {}",
                    a[0], a[1], a[2], a[3], a[3]
                ));
            }
            checked += 1;
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert!(checked > 100, "only {checked} lines of testfp.txt checked");
    }

    #[test]
    fn test_parse_float_testdata() {
        // Test testbase inputs, optimized against not.
        let name = "testdata/atof1k.txt";
        let data = testdata("atof1k.txt");
        let mut failures = Vec::new();
        for (lineno, line) in data.split_inclusive('\n').enumerate() {
            let lineno = lineno + 1;
            let s = line.trim();
            if s.starts_with('#') || s.is_empty() {
                continue;
            }
            let (want, err1) = parse_float_with_optimize(s, 64, false);
            let (have, err2) = parse_float_with_optimize(s, 64, true);
            if let Some(err1) = err1 {
                // Error in test data; should not happen.
                failures.push(format!("{name}:{lineno}: ParseFloat(`{s}`): {err1}"));
                continue;
            }
            if let Some(err2) = err2 {
                failures.push(format!("ParseFloat(`{s}`): {err2}"));
                continue;
            }
            if have != want {
                failures.push(format!(
                    "ParseFloat(`{s}`) = {:#x}, want {:#x}",
                    have.to_bits(),
                    want.to_bits()
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    // Ports: math_test.go. Go's float64(x) * math.Ln2 / math.Ln10 multiplies
    // and divides left to right by the float64 constants, as these do.
    #[test]
    fn test_log10_pow2() {
        for x in -1600..=1600i64 {
            let i_math = log10_pow2(x);
            let f_math =
                (x as f64 * std::f64::consts::LN_2 / std::f64::consts::LN_10).floor() as i64;
            assert_eq!(i_math, f_math, "log10Pow2({x})");
        }
    }

    #[test]
    fn test_log2_pow10() {
        for x in -500..=500i64 {
            let i_math = log2_pow10(x);
            let f_math =
                (x as f64 * std::f64::consts::LN_10 / std::f64::consts::LN_2).floor() as i64;
            assert_eq!(i_math, f_math, "log2Pow10({x})");
        }
    }

    // Ports: decimal_test.go.
    #[test]
    fn test_decimal_shift() {
        let shifttests: &[(u64, i64, &str)] = &[
            (0, -100, "0"),
            (0, 100, "0"),
            (1, 100, "1267650600228229401496703205376"),
            (
                1,
                -100,
                concat!(
                    "0.00000000000000000000000000000078886090522101180541",
                    "17285652827862296732064351090230047702789306640625"
                ),
            ),
            (12345678, 8, "3160493568"),
            (12345678, -8, "48225.3046875"),
            (195312, 9, "99999744"),
            (1953125, 9, "1000000000"),
        ];
        for &(i, shift, out) in shifttests {
            let mut d = Decimal::new(i);
            d.shift(shift);
            let s = d.to_string();
            assert_eq!(s, out, "Decimal {i} << {shift}");
        }
    }

    #[test]
    fn test_decimal_round() {
        // (i, nd, down, round, up, int); like Go's test, int is not checked.
        let roundtests: &[(u64, i64, &str, &str, &str, u64)] = &[
            (0, 4, "0", "0", "0", 0),
            (12344999, 4, "12340000", "12340000", "12350000", 12340000),
            (12345000, 4, "12340000", "12340000", "12350000", 12340000),
            (12345001, 4, "12340000", "12350000", "12350000", 12350000),
            (23454999, 4, "23450000", "23450000", "23460000", 23450000),
            (23455000, 4, "23450000", "23460000", "23460000", 23460000),
            (23455001, 4, "23450000", "23460000", "23460000", 23460000),
            (99994999, 4, "99990000", "99990000", "100000000", 99990000),
            (99995000, 4, "99990000", "100000000", "100000000", 100000000),
            (99999999, 4, "99990000", "100000000", "100000000", 100000000),
            (12994999, 4, "12990000", "12990000", "13000000", 12990000),
            (12995000, 4, "12990000", "13000000", "13000000", 13000000),
            (12999999, 4, "12990000", "13000000", "13000000", 13000000),
        ];
        for &(i, nd, down, round, up, _int) in roundtests {
            let mut d = Decimal::new(i);
            d.round_down(nd);
            assert_eq!(d.to_string(), down, "Decimal {i} RoundDown {nd}");
            let mut d = Decimal::new(i);
            d.round(nd);
            assert_eq!(d.to_string(), round, "Decimal {i} Round {nd}");
            let mut d = Decimal::new(i);
            d.round_up(nd);
            assert_eq!(d.to_string(), up, "Decimal {i} RoundUp {nd}");
        }
    }

    #[test]
    fn test_decimal_rounded_integer() {
        let roundinttests: &[(u64, i64, u64)] = &[
            (0, 100, 0),
            (512, -8, 2),
            (513, -8, 2),
            (640, -8, 2),
            (641, -8, 3),
            (384, -8, 2),
            (385, -8, 2),
            (383, -8, 1),
            (1, 100, u64::MAX),
            (1000, 0, 1000),
        ];
        for &(i, shift, want) in roundinttests {
            let mut d = Decimal::new(i);
            d.shift(shift);
            let int = d.rounded_integer();
            assert_eq!(int, want, "Decimal {i} >> {shift} RoundedInteger");
        }
    }

    // Ports: internal/strconv/atob_test.go TestParseBool (bare errors).
    #[test]
    fn test_parse_bool() {
        let atobtests: &[(&str, bool, Option<NumErrorKind>)] = &[
            ("", false, Some(Syntax)),
            ("asdf", false, Some(Syntax)),
            ("0", false, None),
            ("f", false, None),
            ("F", false, None),
            ("FALSE", false, None),
            ("false", false, None),
            ("False", false, None),
            ("1", true, None),
            ("t", true, None),
            ("T", true, None),
            ("TRUE", true, None),
            ("true", true, None),
            ("True", true, None),
        ];
        for &(input, out, want_err) in atobtests {
            let (b, e) = parse_bool(input);
            let e = e.map(|e| e.err);
            assert!(
                b == out && e == want_err,
                "ParseBool({input}) = {b}, {e:?}, want {out}, {want_err:?}"
            );
        }
    }

    // Ports: strconv/number_test.go TestParseBool (the public ParseBool,
    // whose error is a *NumError).
    #[test]
    fn test_public_parse_bool() {
        let atobtests: &[(&str, bool, Option<NumErrorKind>)] = &[
            ("", false, Some(Syntax)),
            ("asdf", false, Some(Syntax)),
            ("0", false, None),
            ("false", false, None),
            ("true", true, None),
        ];
        for &(input, out, want_err) in atobtests {
            let (b, e) = parse_bool(input);
            if let Some(want_err) = want_err {
                // expect an error
                match e {
                    None => panic!("ParseBool({input}) = nil; want {want_err}"),
                    // NumError assertion must succeed; it's the only thing we return.
                    Some(e) => {
                        assert_eq!(e.err, want_err, "ParseBool({input}) = {e}; want {want_err}");
                        assert_eq!(e.func, "ParseBool");
                        assert_eq!(e.num, input.as_bytes());
                    }
                }
            } else {
                assert!(e.is_none(), "ParseBool({input}) = {e:?}; want nil");
                assert_eq!(b, out, "ParseBool({input})");
            }
        }
    }

    // Ports: TestFormatBool, in both files.
    #[test]
    fn test_format_bool() {
        for (b, s) in [(true, "true"), (false, "false")] {
            assert_eq!(format_bool(b), s, "FormatBool({b})");
        }
    }

    // Ports: TestAppendBool, in both files.
    #[test]
    fn test_append_bool() {
        let append_bool_tests: &[(bool, &[u8], &[u8])] =
            &[(true, b"foo ", b"foo true"), (false, b"foo ", b"foo false")];
        for &(b, input, out) in append_bool_tests {
            let mut buf = input.to_vec();
            append_bool(&mut buf, b);
            assert_eq!(buf, out, "AppendBool({input:?}, {b})");
        }
    }
}
