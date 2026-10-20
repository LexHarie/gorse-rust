//! `gostrconv::parse_float` and `parse_bool` against
//! `tests/data/strconv_golden.json`, which `contracts/tools/strconvgolden`
//! wrote by running go1.27.0's strconv: `ParseFloat` at bit sizes 32 and 64
//! over hand-picked edge cases and a seeded corpus (decimal, hex,
//! underscores, inf/nan, mantissas past the 800-digit cap, halfway points,
//! huge exponents, garbage), `ParseFloat` at other bit sizes, and `ParseBool`.
//! Each case must match bit for bit, NaN payloads included, with the same
//! error and error text. The test-local `FormatFloat` the ports of Go's tests
//! use is checked against Go's output here too.

mod strconv_format;

mod gostrconv {
    use gorse_common::gostrconv::{NumError, NumErrorKind, parse_bool, parse_float};
    use serde::Deserialize;

    use crate::strconv_format::{
        format_b32, format_b64, format_e32, format_e64, format_g32, format_g64,
    };

    #[derive(Deserialize)]
    struct Golden {
        go_version: String,
        parse_float: Vec<FloatCase>,
        bit_size: Vec<BitSizeCase>,
        format: Vec<FormatCase>,
        parse_bool: Vec<BoolCase>,
    }

    #[derive(Deserialize)]
    struct FloatCase {
        #[serde(rename = "in")]
        input: String,
        in_hex: Option<String>,
        bits64: String,
        err64: String,
        text64: String,
        g64: String,
        bits32: String,
        err32: String,
        text32: String,
        g32: String,
    }

    #[derive(Deserialize)]
    struct BitSizeCase {
        #[serde(rename = "in")]
        input: String,
        bit_size: i64,
        bits: String,
        err: String,
        text: String,
    }

    #[derive(Deserialize)]
    struct FormatCase {
        size: u32,
        bits: String,
        g: String,
        b: String,
        e: Vec<String>,
    }

    #[derive(Deserialize)]
    struct BoolCase {
        #[serde(rename = "in")]
        input: String,
        in_hex: Option<String>,
        out: bool,
        err: String,
        text: String,
    }

    fn golden() -> Golden {
        let path = format!(
            "{}/tests/data/strconv_golden.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let golden: Golden = serde_json::from_slice(&data).expect("strconv_golden.json");
        assert_eq!(golden.go_version, "go1.27.0");
        golden
    }

    /// The input bytes: `in`, or `in_hex` for one that is not UTF-8.
    fn input(input: &str, in_hex: Option<&String>) -> Vec<u8> {
        match in_hex {
            Some(h) => hex::decode(h).expect("in_hex"),
            None => input.as_bytes().to_vec(),
        }
    }

    fn bits(s: &str) -> u64 {
        u64::from_str_radix(s, 16).expect("hex bits")
    }

    /// The golden's error kind and text for an error.
    fn err_info(err: Option<&NumError>, func: &str, s: &[u8]) -> (String, String) {
        let Some(err) = err else {
            return (String::new(), String::new());
        };
        assert_eq!(err.func, func);
        assert_eq!(err.num, s);
        let kind = match err.err {
            NumErrorKind::Syntax => "syntax",
            NumErrorKind::Range => "range",
            other => panic!("unexpected reason {other:?}"),
        };
        (kind.into(), err.to_string())
    }

    fn short(s: &[u8]) -> String {
        let s = String::from_utf8_lossy(s);
        if s.len() <= 80 {
            return format!("{s:?}");
        }
        format!(
            "{:?}...({} bytes)",
            s.chars().take(80).collect::<String>(),
            s.len()
        )
    }

    #[test]
    fn parse_float_matches_go() {
        let golden = golden();
        assert!(golden.parse_float.len() > 7000, "corpus too small");
        let mut failures = Vec::new();
        for c in &golden.parse_float {
            let s = input(&c.input, c.in_hex.as_ref());
            for (bit_size, want_bits, want_err, want_text) in [
                (64, &c.bits64, &c.err64, &c.text64),
                (32, &c.bits32, &c.err32, &c.text32),
            ] {
                let (f, err) = parse_float(&s, bit_size);
                let (kind, text) = err_info(err.as_ref(), "ParseFloat", &s);
                if f.to_bits() != bits(want_bits) || kind != *want_err || text != *want_text {
                    failures.push(format!(
                        "ParseFloat({}, {bit_size}) = {:016x}, {kind:?} {text:?}; Go: {want_bits}, {want_err:?} {want_text:?}",
                        short(&s),
                        f.to_bits()
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} cases differ:\n{}",
            failures.len(),
            golden.parse_float.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn parse_float_other_bit_sizes_match_go() {
        let golden = golden();
        for c in &golden.bit_size {
            let (f, err) = parse_float(&c.input, c.bit_size);
            let (kind, text) = err_info(err.as_ref(), "ParseFloat", c.input.as_bytes());
            assert_eq!(
                (f.to_bits(), kind, text),
                (bits(&c.bits), c.err.clone(), c.text.clone()),
                "ParseFloat({:?}, {})",
                c.input,
                c.bit_size
            );
        }
    }

    #[test]
    fn parse_bool_matches_go() {
        let golden = golden();
        for c in &golden.parse_bool {
            let s = input(&c.input, c.in_hex.as_ref());
            let (b, err) = parse_bool(&s);
            let (kind, text) = err_info(err.as_ref(), "ParseBool", &s);
            assert_eq!(
                (b, kind, text),
                (c.out, c.err.clone(), c.text.clone()),
                "ParseBool({})",
                short(&s)
            );
        }
    }

    /// The test-local `FormatFloat` gives Go's text for every parse result in
    /// the golden ('g', -1, 32 and 64) and every value in its format section
    /// ('g', 'b' and 'e' at each precision).
    #[test]
    fn test_formatter_matches_go() {
        let golden = golden();
        for c in &golden.parse_float {
            let f64v = f64::from_bits(bits(&c.bits64));
            assert_eq!(
                format_g64(f64v),
                c.g64,
                "FormatFloat({}, 'g', -1, 64)",
                c.bits64
            );
            let f32v = f64::from_bits(bits(&c.bits32)) as f32;
            assert_eq!(
                format_g32(f32v),
                c.g32,
                "FormatFloat({}, 'g', -1, 32)",
                c.bits32
            );
        }
        assert!(golden.format.len() > 600, "format section too small");
        for c in &golden.format {
            match c.size {
                64 => {
                    let f = f64::from_bits(bits(&c.bits));
                    assert_eq!(format_g64(f), c.g, "FormatFloat({}, 'g', -1, 64)", c.bits);
                    assert_eq!(format_b64(f), c.b, "FormatFloat({}, 'b', -1, 64)", c.bits);
                    for (prec, want) in c.e.iter().enumerate() {
                        assert_eq!(
                            format_e64(f, prec),
                            *want,
                            "FormatFloat({}, 'e', {prec}, 64)",
                            c.bits
                        );
                    }
                }
                32 => {
                    let f = f32::from_bits(u32::from_str_radix(&c.bits, 16).expect("bits"));
                    assert_eq!(format_g32(f), c.g, "FormatFloat({}, 'g', -1, 32)", c.bits);
                    assert_eq!(format_b32(f), c.b, "FormatFloat({}, 'b', -1, 32)", c.bits);
                    for (prec, want) in c.e.iter().enumerate() {
                        assert_eq!(
                            format_e32(f, prec),
                            *want,
                            "FormatFloat({}, 'e', {prec}, 32)",
                            c.bits
                        );
                    }
                }
                size => panic!("size {size}"),
            }
        }
    }
}
