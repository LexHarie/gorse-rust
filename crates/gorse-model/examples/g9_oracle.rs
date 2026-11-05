//! G9, the Lean oracle for the T1 metrics (D-326): `precision`, `recall`, `hr` and `mrr`.
//!
//! The Lean executable `oracle` (proofs/Oracle/Main.lean) computes each metric exactly over
//! the naturals and rounds the quotient to the nearest f32, ties to even. The Rust metrics
//! divide two f32 values that are exact integers below 2^24, and IEEE division rounds the
//! exact quotient the same way, so the two must agree bit for bit. Every count here is at
//! most 64 (the list length in the generator), far below 2^24.
//!
//! Usage:
//!   g9_oracle selftest                      check the rounding that the Lean side mirrors
//!   g9_oracle gen <cases.jsonl> [count]     write `count` cases per metric (default 10^6)
//!   g9_oracle check <cases.jsonl> <results> compare the oracle's output with the Rust metrics
//!
//! The oracle reads one case per line and writes one line per case: the f32 bit pattern of
//! the result, or `none`. `scripts/check-oracle.sh` runs the three steps in order.

use std::collections::{BTreeSet, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::ExitCode;

use gorse_model::cf::evaluator::{hr, mrr, precision, recall};

/// The metrics in the order the generator writes them.
const METRICS: [&str; 4] = ["precision", "recall", "hr", "mrr"];

/// Cases per metric when `gen` is not given a count: the accept clause of G9.
const DEFAULT_COUNT: u64 = 1_000_000;

/// The seed of the case stream. Each metric gets its own stream, derived from this.
const SEED: u64 = 0x6F9A_0D1E_CA5E_0001;

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

    /// A value in `lo..=hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }
}

/// An item id. Most come from a small range that the lists overlap, and some are the
/// extremes of `i32`, where an ordering bug would show.
fn pool_item(rng: &mut Rng) -> i32 {
    let roll = rng.below(100);
    if roll < 90 {
        rng.below(43) as i32 - 3
    } else if roll < 95 {
        [i32::MIN, i32::MAX, 0, -1][rng.below(4) as usize]
    } else {
        rng.next_u64() as u32 as i32
    }
}

/// A target set: distinct ids, from empty to 40 of them.
fn gen_target(rng: &mut Rng) -> Vec<i32> {
    let size = match rng.below(100) {
        0..=4 => 0,
        5..=49 => rng.range(1, 8),
        50..=89 => rng.range(1, 20),
        _ => rng.range(21, 40),
    };
    let size = size as usize;
    let mut set = BTreeSet::new();
    let mut attempts = 0;
    while set.len() < size && attempts < 4 * size + 4 {
        set.insert(pool_item(rng));
        attempts += 1;
    }
    set.into_iter().collect()
}

/// A ranked list, from empty to 64 entries. It may repeat entries, and it may start with
/// the target, so that every hit count from zero to the full target occurs.
fn gen_list(rng: &mut Rng, target: &[i32]) -> Vec<i32> {
    let len = match rng.below(100) {
        0..=4 => 0,
        5..=54 => rng.range(1, 16),
        55..=84 => rng.range(17, 40),
        _ => rng.range(41, 64),
    };
    let len = len as usize;
    let regime = rng.below(100);
    let mut list: Vec<i32> = Vec::with_capacity(len);
    if regime < 8 {
        list.extend(target.iter().copied().take(len));
    } else if regime < 16 {
        list.extend(target.iter().rev().copied().take(len));
    }
    while list.len() < len {
        let item = match rng.below(100) {
            0..=39 if !target.is_empty() => target[rng.below(target.len() as u64) as usize],
            40..=54 if !list.is_empty() => list[list.len() - 1],
            55..=64 if !list.is_empty() => list[rng.below(list.len() as u64) as usize],
            _ => pool_item(rng),
        };
        list.push(item);
    }
    list
}

fn write_ints(out: &mut impl Write, xs: &[i32]) -> std::io::Result<()> {
    out.write_all(b"[")?;
    for (i, x) in xs.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        write!(out, "{x}")?;
    }
    out.write_all(b"]")
}

fn write_case(
    out: &mut impl Write,
    metric: &str,
    target: &[i32],
    list: &[i32],
) -> std::io::Result<()> {
    write!(out, "{{\"m\":\"{metric}\",\"t\":")?;
    write_ints(out, target)?;
    out.write_all(b",\"l\":")?;
    write_ints(out, list)?;
    out.write_all(b"}\n")
}

fn generate(path: &str, count: u64) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("{path}: {e}"))?;
    let mut out = BufWriter::new(file);
    for (index, metric) in METRICS.iter().enumerate() {
        let mut rng = Rng(SEED ^ ((index as u64 + 1) << 40));
        for _ in 0..count {
            let target = gen_target(&mut rng);
            let list = gen_list(&mut rng, &target);
            write_case(&mut out, metric, &target, &list).map_err(|e| e.to_string())?;
        }
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// The f32 bit pattern of `n / d`, rounded to nearest with ties to even, for `n / d` in the
/// normal f32 range. This is the algorithm `ratioBits` in proofs/Oracle/Main.lean mirrors,
/// step for step.
fn ratio_bits(n: u128, d: u128) -> u32 {
    if n == 0 {
        return 0;
    }
    // geq(e): n / d >= 2^e.
    let geq = |e: i64| -> bool {
        if e >= 0 {
            n >= d << e
        } else {
            (n << (-e)) >= d
        }
    };
    // e = floor(log2(n / d)), found by stepping from 0.
    let mut e: i64 = 0;
    while !geq(e) {
        e -= 1;
    }
    while geq(e + 1) {
        e += 1;
    }
    // m = (n / d) / 2^(e - 23), which lies in [2^23, 2^24).
    let s = 23 - e;
    let (num, den) = if s >= 0 { (n << s, d) } else { (n, d << (-s)) };
    let mut m = num / den;
    let r = num % den;
    if 2 * r > den || (2 * r == den && m % 2 == 1) {
        m += 1;
    }
    if m == 1 << 24 {
        m = 1 << 23;
        e += 1;
    }
    (((e + 127) as u32) << 23) | ((m - (1 << 23)) as u32)
}

/// Checks `ratio_bits` against the hardware: IEEE division of two exact f32 integers.
fn selftest() -> Result<(), String> {
    let mut checked = 0u64;
    for n in 1..=300u128 {
        for d in 1..=300u128 {
            let hardware = (n as f32 / d as f32).to_bits();
            if ratio_bits(n, d) != hardware {
                return Err(format!(
                    "{n}/{d}: ratio_bits {} != hardware {hardware}",
                    ratio_bits(n, d)
                ));
            }
            checked += 1;
        }
    }
    let mut rng = Rng(SEED);
    for _ in 0..200_000 {
        let n = rng.range(1, (1 << 24) - 1) as u128;
        let d = rng.range(1, (1 << 24) - 1) as u128;
        let hardware = (n as f32 / d as f32).to_bits();
        if ratio_bits(n, d) != hardware {
            return Err(format!(
                "{n}/{d}: ratio_bits {} != hardware {hardware}",
                ratio_bits(n, d)
            ));
        }
        checked += 1;
    }
    println!("selftest: ratio_bits agrees with IEEE division on {checked} pairs");
    Ok(())
}

fn encode(value: Option<f32>) -> String {
    match value {
        None => "none".to_string(),
        Some(x) => x.to_bits().to_string(),
    }
}

fn ints(value: &serde_json::Value) -> Result<Vec<i32>, String> {
    value
        .as_array()
        .ok_or("expected an array")?
        .iter()
        .map(|v| {
            v.as_i64()
                .and_then(|x| i32::try_from(x).ok())
                .ok_or_else(|| format!("bad id {v}"))
        })
        .collect()
}

fn check(cases_path: &str, results_path: &str) -> Result<bool, String> {
    let cases = BufReader::new(File::open(cases_path).map_err(|e| format!("{cases_path}: {e}"))?);
    let results =
        BufReader::new(File::open(results_path).map_err(|e| format!("{results_path}: {e}"))?);
    let mut results = results.lines();
    // Per metric: (cases, mismatches).
    let mut counts = [(0u64, 0u64); METRICS.len()];
    let mut shown = 0;
    for (number, case) in cases.lines().enumerate() {
        let case = case.map_err(|e| e.to_string())?;
        let result = results
            .next()
            .ok_or_else(|| format!("results end before case {}", number + 1))?
            .map_err(|e| e.to_string())?;
        let value: serde_json::Value = serde_json::from_str(&case).map_err(|e| e.to_string())?;
        let metric = value["m"].as_str().ok_or("case without a metric")?;
        let target: HashSet<i32> = ints(&value["t"])?.into_iter().collect();
        let list = ints(&value["l"])?;
        let rust = match metric {
            "precision" => precision(&target, &list),
            "recall" => recall(&target, &list),
            "hr" => hr(&target, &list),
            "mrr" => mrr(&target, &list),
            other => return Err(format!("unknown metric {other}")),
        };
        let index = METRICS
            .iter()
            .position(|m| *m == metric)
            .ok_or("unknown metric")?;
        let expected = encode(rust);
        counts[index].0 += 1;
        if result.trim() != expected {
            counts[index].1 += 1;
            if shown < 20 {
                eprintln!(
                    "mismatch {metric} case {}: oracle {} rust {expected} ({case})",
                    number + 1,
                    result.trim()
                );
                shown += 1;
            }
        }
    }
    if results.next().is_some() {
        return Err("results have more lines than cases".to_string());
    }
    let mut clean = true;
    for (metric, (cases, mismatches)) in METRICS.iter().zip(counts) {
        println!("{metric}: {cases} cases, {mismatches} mismatches");
        clean &= cases > 0 && mismatches == 0;
    }
    Ok(clean)
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
        _ => {
            Err("usage: g9_oracle selftest | gen <cases> [count] | check <cases> <results>".into())
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("g9: mismatches found");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("g9: {e}");
            ExitCode::FAILURE
        }
    }
}
