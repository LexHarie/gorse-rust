//! G9 for `map` (D-352, P4-9): the Lean oracle `oracle_map` against `gorse_model::cf::evaluator::map`.
//!
//! `map` depends on the size of the target set, `t = |T|`, and on which entries of the ranked list
//! are relevant, in rank order. A case is `{"t": t, "hits": [..]}`. The check rebuilds a target set
//! of the ids `0..t` and a ranked list whose relevant entries take ids of that set, and whose other
//! entries take ids outside it. A relevant id may repeat, because `map` counts each entry, so the
//! cases cover the duplicates that a ranked list can hold.
//!
//! The Lean executable (proofs/Oracle/Map.lean) computes each case over the rationals, with the
//! rounding of each step of the Rust sum (the twin Gorse.Spec.MapRat.mapQ, D-352). It prints the
//! binary32 result as `m e`, or `none` when `map` is undefined. Every result is compared bit for bit.
//!
//! Usage:
//!   g9_map gen <cases.jsonl> [count]      write `count` cases (default 100,000)
//!   g9_map check <cases.jsonl> <results>  compare the oracle's output with `map`

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::ExitCode;

use gorse_model::cf::evaluator::map;

/// The seed of the cases. Each run of `gen` writes the same cases.
const SEED: u64 = 0x6A9D_0C5E_0352_0001;
/// The number of cases when none is given.
const DEFAULT_COUNT: u64 = 100_000;
/// The longest ranked list, and the largest target set.
const MAX_LEN: u64 = 64;

/// A seeded generator (SplitMix64).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// One case: the target size, and the relevance flags of the ranked list in order. No flag is set
/// when the target is empty, because nothing can be relevant then.
fn case(rng: &mut Rng) -> (u64, Vec<bool>) {
    // The target is empty about one case in twenty.
    let t = if rng.below(20) == 0 {
        0
    } else {
        1 + rng.below(MAX_LEN)
    };
    let len = rng.below(MAX_LEN + 1);
    // The share of relevant entries varies from case to case, from none to all of them.
    let share = rng.below(101);
    let hits = (0..len).map(|_| t > 0 && rng.below(100) < share).collect();
    (t, hits)
}

/// The target set and the ranked list that a case stands for. A relevant entry takes an id of the
/// target, and the others take ids outside it. A relevant id may repeat.
fn rebuild(t: u64, hits: &[bool]) -> (HashSet<i32>, Vec<i32>) {
    let target: HashSet<i32> = (0..t as i32).collect();
    let ranked = hits
        .iter()
        .enumerate()
        .map(|(i, &hit)| {
            if hit && t > 0 {
                (i as u64 % t) as i32
            } else {
                1000 + i as i32
            }
        })
        .collect();
    (target, ranked)
}

/// Writes `count` cases, one JSON object per line.
fn generate(path: &str, count: u64) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("{path}: {e}"))?;
    let mut out = BufWriter::new(file);
    let mut rng = Rng(SEED);
    for _ in 0..count {
        let (t, hits) = case(&mut rng);
        let flags: Vec<&str> = hits
            .iter()
            .map(|&h| if h { "true" } else { "false" })
            .collect();
        writeln!(out, "{{\"t\":{t},\"hits\":[{}]}}", flags.join(",")).map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())
}

/// The bits of the oracle's result: `none` is `None`, `0 0` is +0.0, and `m e` is `m * 2^e`.
fn oracle_bits(line: &str) -> Result<Option<u32>, String> {
    let mut parts = line.split_whitespace();
    let first = parts.next().ok_or("empty result line")?;
    if first == "none" {
        return match parts.next() {
            None => Ok(None),
            Some(_) => Err("none with more fields".into()),
        };
    }
    let m: i64 = first.parse().map_err(|e| format!("result m: {e}"))?;
    let e: i64 = parts
        .next()
        .ok_or("result without e")?
        .parse()
        .map_err(|e| format!("result e: {e}"))?;
    if parts.next().is_some() {
        return Err("result with more than two fields".into());
    }
    if m == 0 {
        return Ok(Some(0));
    }
    // 2^e is exact for -1022 <= e <= 1023, and m * 2^e is exact in f64 because |m| < 2^24. The value
    // is an f32, so the cast back is exact too.
    let scale = f64::from_bits(((e + 1023) as u64) << 52);
    let value = (m as f64 * scale) as f32;
    Ok(Some(value.to_bits()))
}

/// Compares the oracle's output with `map`, case by case. Returns whether every case agreed and at
/// least one case was compared.
fn check(cases_path: &str, results_path: &str) -> Result<bool, String> {
    let cases = BufReader::new(File::open(cases_path).map_err(|e| format!("{cases_path}: {e}"))?);
    let results =
        BufReader::new(File::open(results_path).map_err(|e| format!("{results_path}: {e}"))?);
    let mut results = results.lines();
    let (mut compared, mut mismatches, mut undefined) = (0u64, 0u64, 0u64);
    let mut shown = 0;
    for (number, case) in cases.lines().enumerate() {
        let case = case.map_err(|e| e.to_string())?;
        let result = results
            .next()
            .ok_or_else(|| format!("results end before case {}", number + 1))?
            .map_err(|e| e.to_string())?;
        let json: serde_json::Value = serde_json::from_str(&case).map_err(|e| e.to_string())?;
        let t = json["t"].as_u64().ok_or("case without t")?;
        let hits = json["hits"]
            .as_array()
            .ok_or("case without hits")?
            .iter()
            .map(|h| h.as_bool().ok_or("a flag is not a boolean"))
            .collect::<Result<Vec<bool>, _>>()?;
        let oracle = oracle_bits(result.trim())?;
        let (target, ranked) = rebuild(t, &hits);
        let rust = map(&target, &ranked).map(f32::to_bits);
        compared += 1;
        if oracle.is_none() {
            undefined += 1;
        }
        if rust != oracle {
            mismatches += 1;
            if shown < 20 {
                eprintln!(
                    "mismatch case {}: oracle {oracle:?} rust {rust:?} (t {t}, {} entries)",
                    number + 1,
                    hits.len()
                );
                shown += 1;
            }
        }
    }
    if results.next().is_some() {
        return Err("results have more lines than cases".to_string());
    }
    println!("map: {compared} cases, {mismatches} mismatches, {undefined} undefined");
    Ok(compared > 0 && mismatches == 0)
}

fn run(args: &[String]) -> Result<bool, String> {
    match args.get(1).map(String::as_str) {
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
        _ => Err("usage: g9_map gen <cases> [count] | check <cases> <results>".into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("g9_map: mismatches found, or nothing was compared");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("g9_map: {e}");
            ExitCode::FAILURE
        }
    }
}
