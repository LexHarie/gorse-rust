//! `format_float`, `format_int` and `format_uint` against what Go prints
//! (`ftoa_golden.json`, written by `contracts/tools/ftoagolden`).
//!
//! Each input's outputs, in every format, precision, bit size or base, are
//! joined and hashed exactly as the generator hashes them.

use gorse_common::gostrconv::{format_float, format_int, format_uint};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Golden {
    formats: String,
    precs: Vec<i64>,
    floats: Vec<[String; 2]>,
    ints: Vec<[String; 2]>,
}

fn golden() -> Golden {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/ftoa_golden.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("read golden")).expect("golden JSON")
}

fn digest(out: &[String]) -> String {
    let sum = Sha256::digest(out.join("\n").as_bytes());
    sum[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn format_float_matches_go() {
    let g = golden();
    let mut failures = Vec::new();
    for [bits, want] in &g.floats {
        let f = f64::from_bits(u64::from_str_radix(bits, 16).expect("bits"));
        let mut out = Vec::new();
        for bit_size in [64, 32] {
            for fmt in g.formats.bytes() {
                for &prec in &g.precs {
                    out.push(format_float(f, fmt, prec, bit_size));
                }
            }
        }
        if &digest(&out) != want {
            failures.push(format!("{bits} ({f:e})"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} floats differ from Go: {failures:?}",
        failures.len(),
        g.floats.len()
    );
}

#[test]
fn format_int_matches_go() {
    let g = golden();
    let mut failures = Vec::new();
    for [text, want] in &g.ints {
        let i: i64 = text.parse().expect("int");
        let mut out = Vec::new();
        for base in 2..=36 {
            out.push(format_int(i, base));
            out.push(format_uint(i as u64, base));
        }
        if &digest(&out) != want {
            failures.push(text.clone());
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} integers differ from Go: {failures:?}",
        failures.len(),
        g.ints.len()
    );
}
