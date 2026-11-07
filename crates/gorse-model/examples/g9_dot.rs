//! G9 for the dot kernels (D-349, P4-7d): the Lean oracle `oracle_dot` against the Rust kernels.
//!
//! The Lean executable (proofs/Oracle/Dot.lean) computes each dot product with the rounding of
//! the model, over the rationals (the twins in Gorse.Spec.DotRat, D-348), and prints the binary32
//! result as `m e`, meaning `m * 2^e`, and `0 0` for every zero. This example writes the cases, and
//! compares the Rust kernels with the oracle's results bit for bit, apart from the sign of a zero.
//!
//! Usage:
//!   g9_dot selftest                       check the decomposition of binary32 values
//!   g9_dot gen <cases.jsonl> [count]      write `count` vector pairs, three cases for each
//!   g9_dot check <cases.jsonl> <results>  compare the oracle's output with the Rust kernels
//!
//! Each vector pair is written three times, for the models `seq` (the scalar loop, which is
//! `Feature::NONE`), `avx` and `avx512`. The oracle computes each model's result, and the Rust
//! side runs the kernel of each level its CPU has: `avx` needs AVX, and `avx512` needs AVX, FMA
//! and AVX-512F. A level the CPU lacks is reported as not run, and it is not compared.
//!
//! The cases are seeded and adversarial: ordinary values, exact cancellation (a second half that
//! negates the first, with one-ulp perturbations on half of it), subnormal operands, large
//! operands, and zeros. The lengths run from 0 to 64. The generator keeps every partial sum
//! finite (see `MAX_ABS_SUM`), so the oracle never refuses an overflow.
//!
//! The sign of a zero. The oracle prints every zero as `0 0`, because its rounding `rnQ` has no
//! negative zero. So the oracle does not say which sign IEEE gives. The check compares every result
//! bit for bit with the oracle's bits, except for the four cases in `SIGNED_ZEROS`. Those expect
//! -0.0, and each sign is derived by hand from IEEE round-to-nearest (D-350). So G9 checks the sign
//! of a zero only on those listed cases.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::ExitCode;

use gorse_common::floats::Feature;

/// The models, in the order the generator writes them for each vector pair.
const MODELS: [&str; 3] = ["seq", "avx", "avx512"];

/// Vector pairs per run when `gen` is not given a count. The oracle is run in chunks, so the
/// number of lines it reads is three times this.
const DEFAULT_COUNT: u64 = 4_000;

/// The longest vectors the generator writes.
const MAX_LEN: u64 = 64;

/// A bound on the sum of the absolute products of one vector pair. Each element has magnitude
/// below 2^60, so each product is below 2^120, and 64 products are below 2^126. The bound 1.7e38
/// is below 2^127, which is below the largest finite binary32 (about 2^128), so no partial sum
/// can overflow. The generator asserts it.
const MAX_ABS_SUM: f64 = 1.7e38;

/// SplitMix64 seed of the case stream.
const SEED: u64 = 0x6F9A_0D1E_CA5E_0002;

/// SplitMix64, a fixed generator, so every run writes the same cases.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n`, for `n > 0`.
    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// A binary32 value written as the exact pair `m * 2^e` of integers, with `|m| < 2^24`.
#[derive(Clone, Copy)]
struct Elem {
    m: i64,
    e: i64,
}

/// Decomposes a finite binary32 value, other than a negative zero, into `m * 2^e`.
fn decompose(x: f32) -> Elem {
    let bits = x.to_bits();
    let sign = if bits >> 31 == 1 { -1 } else { 1 };
    let biased = ((bits >> 23) & 0xFF) as i64;
    let frac = i64::from(bits & 0x7F_FFFF);
    if biased == 0 {
        // A subnormal, or zero: frac * 2^-149.
        if frac == 0 {
            return Elem { m: 0, e: 0 };
        }
        return Elem {
            m: sign * frac,
            e: -149,
        };
    }
    Elem {
        m: sign * (frac | 1 << 23),
        e: biased - 150,
    }
}

/// The binary32 value of `m * 2^e`, for `|m| < 2^24` and an `m * 2^e` that is an f32.
fn f32_of(m: i64, e: i64) -> f32 {
    // 2^e is exact for -1022 <= e <= 1023, and m * 2^e is exact in f64 because |m| < 2^24.
    // The value is an f32, so the cast back is exact too.
    let scale = f64::from_bits(((e + 1023) as u64) << 52);
    (m as f64 * scale) as f32
}

/// A normal f32 whose unbiased exponent lies in `lo..=hi`, with a random sign and mantissa.
fn normal(rng: &mut Rng, lo: i64, hi: i64) -> f32 {
    let exponent = lo + rng.below((hi - lo + 1) as u64) as i64;
    let sign = (rng.next_u64() & 1) as u32;
    let frac = (rng.next_u64() as u32) & 0x7F_FFFF;
    f32::from_bits((sign << 31) | (((exponent + 127) as u32) << 23) | frac)
}

/// A nonzero subnormal f32, with a random sign.
fn subnormal(rng: &mut Rng) -> f32 {
    let sign = (rng.next_u64() & 1) as u32;
    let frac = 1 + (rng.next_u64() as u32) % 0x7F_FFFF;
    f32::from_bits((sign << 31) | frac)
}

/// What a vector pair is made of.
#[derive(Clone, Copy, PartialEq)]
enum Style {
    /// Values of moderate size.
    Ordinary,
    /// The second half negates the first: the products cancel, and the sum is near zero.
    Cancellation,
    /// Subnormal operands, which underflow in the products, mixed with normal ones.
    Subnormal,
    /// The largest operands that keep every partial sum finite.
    Large,
}

/// One element. It is +0.0 for one in ten, so the zeros of a vector are not rare.
fn element(rng: &mut Rng, style: Style) -> f32 {
    if rng.below(10) == 0 {
        return 0.0;
    }
    match style {
        Style::Ordinary | Style::Cancellation => normal(rng, -20, 20),
        Style::Subnormal => {
            if rng.below(2) == 0 {
                subnormal(rng)
            } else {
                normal(rng, -60, 59)
            }
        }
        Style::Large => normal(rng, 50, 59),
    }
}

/// A vector pair of length 0 to 64, of the style the generator draws.
fn vector_pair(rng: &mut Rng) -> (Vec<f32>, Vec<f32>) {
    let n = rng.below(MAX_LEN + 1) as usize;
    let style = match rng.below(100) {
        0..=39 => Style::Ordinary,
        40..=59 => Style::Cancellation,
        60..=79 => Style::Subnormal,
        _ => Style::Large,
    };
    let mut xs = Vec::with_capacity(n);
    let mut ys = Vec::with_capacity(n);
    for _ in 0..n {
        xs.push(element(rng, style));
        ys.push(element(rng, style));
    }
    if style == Style::Cancellation {
        let half = n / 2;
        for k in 0..half {
            xs[half + k] = xs[k];
            let y = ys[k];
            let mut negated = if y == 0.0 { 0.0 } else { -y };
            if rng.below(2) == 0 && negated != 0.0 {
                // One ulp in magnitude, so the products no longer cancel exactly.
                negated = f32::from_bits(negated.to_bits() + 1);
            }
            ys[half + k] = negated;
        }
    }
    let abs_sum: f64 = xs
        .iter()
        .zip(&ys)
        .map(|(&x, &y)| (f64::from(x) * f64::from(y)).abs())
        .sum();
    assert!(abs_sum < MAX_ABS_SUM, "generator bound violated: {abs_sum}");
    (xs, ys)
}

fn write_elems(out: &mut impl Write, xs: &[f32]) -> std::io::Result<()> {
    for (i, &x) in xs.iter().enumerate() {
        if i > 0 {
            write!(out, ",")?;
        }
        let e = decompose(x);
        write!(out, "[{},{}]", e.m, e.e)?;
    }
    Ok(())
}

fn write_case(out: &mut impl Write, model: &str, xs: &[f32], ys: &[f32]) -> std::io::Result<()> {
    write!(out, "{{\"model\":\"{model}\",\"xs\":[")?;
    write_elems(out, xs)?;
    write!(out, "],\"ys\":[")?;
    write_elems(out, ys)?;
    writeln!(out, "]}}")
}

fn generate(path: &str, count: u64) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("{path}: {e}"))?;
    let mut out = BufWriter::new(file);
    let mut rng = Rng(SEED);
    for _ in 0..count {
        let (xs, ys) = vector_pair(&mut rng);
        for model in MODELS {
            write_case(&mut out, model, &xs, &ys).map_err(|e| e.to_string())?;
        }
    }
    out.flush().map_err(|e| e.to_string())
}

/// The f32 values of a JSON array of `[m, e]` elements.
fn elements(json: &serde_json::Value) -> Result<Vec<f32>, String> {
    let items = json
        .as_array()
        .ok_or_else(|| "elements are not an array".to_string())?;
    items
        .iter()
        .map(|item| -> Result<f32, String> {
            let pair = item
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or_else(|| "an element is not [m, e]".to_string())?;
            let m = pair[0]
                .as_i64()
                .ok_or_else(|| "m is not an integer".to_string())?;
            let e = pair[1]
                .as_i64()
                .ok_or_else(|| "e is not an integer".to_string())?;
            Ok(f32_of(m, e))
        })
        .collect()
}

/// The bits of the oracle's result: `m e` is `m * 2^e`, and `0 0` is +0.0.
fn oracle_bits(line: &str) -> Result<u32, String> {
    let mut parts = line.split_whitespace();
    let m: i64 = parts
        .next()
        .ok_or("empty result line")?
        .parse()
        .map_err(|e| format!("result m: {e}"))?;
    let e: i64 = parts
        .next()
        .ok_or("result without e")?
        .parse()
        .map_err(|e| format!("result e: {e}"))?;
    if parts.next().is_some() {
        return Err("result with more than two fields".into());
    }
    Ok(if m == 0 { 0 } else { f32_of(m, e).to_bits() })
}

/// The cases whose expected result is -0.0, although the oracle prints `0 0`: the case number
/// (1-based, in the cases file), the model, and the expected bits. Each sign is derived by hand from
/// IEEE round-to-nearest, not from the Rust output (D-350).
const SIGNED_ZEROS: [(u64, &str, u32); 4] = [
    (2799, "avx512", 0x8000_0000),
    (3012, "avx512", 0x8000_0000),
    (4608, "avx512", 0x8000_0000),
    (7032, "avx512", 0x8000_0000),
];

/// The bits that case `number` (1-based) of `model` must produce, given the oracle's bits. A listed
/// case must be a zero in the oracle, and its bits come from `SIGNED_ZEROS`. Every other case must
/// match the oracle bit for bit.
fn expected_bits(model: &str, number: u64, oracle: u32) -> Result<u32, String> {
    match SIGNED_ZEROS
        .iter()
        .find(|(n, m, _)| *n == number && *m == model)
    {
        Some((_, _, bits)) if oracle == 0 => Ok(*bits),
        Some(_) => Err(format!(
            "case {number} is listed as a signed zero, but the oracle prints a nonzero result"
        )),
        None => Ok(oracle),
    }
}

/// Whether case `number` (1-based) of `model` is one of the listed signed zeros.
fn is_listed(model: &str, number: u64) -> bool {
    SIGNED_ZEROS
        .iter()
        .any(|(n, m, _)| *n == number && *m == model)
}

fn check(cases_path: &str, results_path: &str) -> Result<bool, String> {
    let cases = BufReader::new(File::open(cases_path).map_err(|e| format!("{cases_path}: {e}"))?);
    let results =
        BufReader::new(File::open(results_path).map_err(|e| format!("{results_path}: {e}"))?);
    let mut results = results.lines();
    let cpu = Feature::detect(None);
    let avx = cpu.contains(Feature::AVX);
    let avx512 = cpu.contains(Feature::AVX512);
    println!(
        "cpu levels: AVX {}, FMA {}, AVX-512 {}",
        yes_no(avx),
        yes_no(cpu.contains(Feature::FMA)),
        yes_no(avx512),
    );
    // Per model: (cases compared, mismatches).
    let mut counts = [(0u64, 0u64); MODELS.len()];
    // The listed signed zeros that matched.
    let mut listed_matched = 0u64;
    let mut shown = 0;
    for (number, case) in cases.lines().enumerate() {
        let case = case.map_err(|e| e.to_string())?;
        let result = results
            .next()
            .ok_or_else(|| format!("results end before case {}", number + 1))?
            .map_err(|e| e.to_string())?;
        let json: serde_json::Value = serde_json::from_str(&case).map_err(|e| e.to_string())?;
        let model = json["model"].as_str().ok_or("case without a model")?;
        let index = MODELS
            .iter()
            .position(|m| *m == model)
            .ok_or_else(|| format!("unknown model {model}"))?;
        let (feature, supported) = match model {
            "seq" => (Feature::NONE, true),
            "avx" => (Feature::AVX, avx),
            "avx512" => (Feature::AVX512, avx512),
            other => return Err(format!("unknown model {other}")),
        };
        let oracle = oracle_bits(result.trim())?;
        let case_number = number as u64 + 1;
        let expected = expected_bits(model, case_number, oracle)?;
        if !supported {
            continue;
        }
        let xs = elements(&json["xs"])?;
        let ys = elements(&json["ys"])?;
        if xs.len() != ys.len() {
            return Err(format!("case {}: xs and ys differ in length", number + 1));
        }
        let rust = feature.dot(&xs, &ys).to_bits();
        counts[index].0 += 1;
        if rust == expected {
            if is_listed(model, case_number) {
                listed_matched += 1;
            }
        } else {
            counts[index].1 += 1;
            if shown < 20 {
                eprintln!(
                    "mismatch {model} case {case_number}: expected {expected:#010x} rust {rust:#010x} ({} elements)",
                    xs.len()
                );
                shown += 1;
            }
        }
    }
    if results.next().is_some() {
        return Err("results have more lines than cases".to_string());
    }
    let mut clean = true;
    for (model, (compared, mismatches)) in MODELS.iter().zip(counts) {
        let reason = match *model {
            "seq" => None,
            "avx" if !avx => Some("the CPU lacks AVX"),
            "avx512" if !avx512 => Some("the CPU lacks AVX-512F, FMA or AVX"),
            _ => None,
        };
        match reason {
            Some(reason) => println!("{model}: not run, {reason}"),
            None => {
                println!("{model}: {compared} cases, {mismatches} mismatches");
                clean &= compared > 0 && mismatches == 0;
            }
        }
    }
    // The listed signed zeros are the whole check of the sign of a zero. On a runner with AVX-512,
    // every one of them must be compared and must match.
    if avx512 {
        println!(
            "signed zeros: {listed_matched} of {} listed cases matched",
            SIGNED_ZEROS.len()
        );
        clean &= listed_matched == SIGNED_ZEROS.len() as u64;
    }
    Ok(clean)
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// Every finite binary32 value, other than a negative zero, decomposes into `m * 2^e` with
/// `|m| < 2^24`, and comes back to the same bits.
fn selftest() -> Result<(), String> {
    let mut rng = Rng(SEED ^ 1);
    let mut checked = 0u64;
    for _ in 0..200_000 {
        let bits = rng.next_u64() as u32;
        let x = f32::from_bits(bits);
        if !x.is_finite() || bits == 0x8000_0000 {
            continue;
        }
        let e = decompose(x);
        if e.m.abs() >= 1 << 24 {
            return Err(format!("{bits:#010x}: |m| is not below 2^24"));
        }
        let back = f32_of(e.m, e.e).to_bits();
        if back != bits {
            return Err(format!(
                "{bits:#010x} decomposes to {} * 2^{} and comes back as {back:#010x}",
                e.m, e.e
            ));
        }
        checked += 1;
    }
    println!("selftest: {checked} binary32 values decompose to m * 2^e and back");
    Ok(())
}

fn run(args: &[String]) -> Result<bool, String> {
    match args.get(1).map(String::as_str) {
        Some("selftest") => selftest().map(|()| true),
        Some("gen") => {
            let path = args.get(2).ok_or("gen needs a path")?;
            let count = match args.get(3) {
                Some(c) => c.parse::<u64>().map_err(|e| format!("count: {e}"))?,
                None => DEFAULT_COUNT,
            };
            generate(path, count).map(|()| true)
        }
        Some("check") => {
            let cases = args.get(2).ok_or("check needs the cases path")?;
            let results = args.get(3).ok_or("check needs the results path")?;
            check(cases, results)
        }
        _ => Err("usage: g9_dot selftest | gen <cases> [count] | check <cases> <results>".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{SIGNED_ZEROS, expected_bits, is_listed};

    #[test]
    fn an_unlisted_zero_expects_positive_zero_and_other_values_match_exactly() {
        // A negative zero where the oracle prints 0 0 is a mismatch, because it is not listed.
        assert_eq!(expected_bits("avx512", 1, 0), Ok(0));
        assert_ne!(expected_bits("avx512", 1, 0), Ok(0x8000_0000));
        assert_eq!(expected_bits("seq", 2799, 0), Ok(0));
        assert_eq!(expected_bits("avx512", 2800, 0x3f80_0000), Ok(0x3f80_0000));
    }

    #[test]
    fn the_four_listed_cases_expect_negative_zero() {
        assert_eq!(SIGNED_ZEROS.len(), 4);
        for case in [2799, 3012, 4608, 7032] {
            assert!(is_listed("avx512", case));
            assert_eq!(expected_bits("avx512", case, 0), Ok(0x8000_0000));
            assert!(!is_listed("seq", case));
        }
    }

    #[test]
    fn a_listed_case_must_be_a_zero_in_the_oracle() {
        assert!(expected_bits("avx512", 2799, 0x3f80_0000).is_err());
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("g9_dot: mismatches found, or nothing was compared");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("g9_dot: {e}");
            ExitCode::FAILURE
        }
    }
}
