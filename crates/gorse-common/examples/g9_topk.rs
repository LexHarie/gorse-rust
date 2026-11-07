//! G9, the Rust side of the Lean oracle for the top-k filter (P3-4, D-335).
//!
//! The same cases are run through `TopKFilter` here and through the compiled Lean executable
//! `oracle_topk` (proofs/Oracle/TopK.lean), which runs the computable mirror of the top-k model
//! (proofs/Gorse/Oracle/TopKMirror.lean). The results are discrete, so they are exact: the
//! check is a byte comparison of the two outputs (scripts/check-oracle-topk.sh).
//!
//! Usage:
//!   g9_topk gen <cases.jsonl> <count>   write `count` cases, from a fixed seed
//!   g9_topk run <cases.jsonl>           print the `TopKFilter` result of each case
//!
//! A case is one JSON line, `{"k": k, "ids": [...], "ws": [...]}`, with `ids` distinct, so the
//! D-329 order is total. A result line is the elements, best first, as `id:weight` tokens.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};

use gorse_common::heap::TopKFilter;
use serde_json::{Value, json};

/// The most items in a case, and the largest `k`. The Lean oracle runs every case, so these
/// keep it inside the CI time.
const MAX_ITEMS: u64 = 24;
const MAX_K: u64 = 18;

/// The seed of the generator, so the cases are the same on every run.
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// SplitMix64: a small generator with no dependency.
struct SplitMix(u64);

impl SplitMix {
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

/// A weight: mostly small, so ties are common, with the extremes and the sign edge too.
fn weight(rng: &mut SplitMix) -> i64 {
    match rng.below(10) {
        0..=5 => rng.below(11) as i64 - 5,
        6 => i64::MIN,
        7 => i64::MAX,
        8 => -1,
        _ => rng.next_u64() as i64,
    }
}

/// One random case: `k`, and the items as (id, weight), with distinct ids. The ids sit near
/// zero, near the middle of `u32`, or near `u32::MAX`, so the extremes are covered.
fn case(rng: &mut SplitMix) -> (u64, Vec<(u32, i64)>) {
    let k = rng.below(MAX_K + 1);
    let n = rng.below(MAX_ITEMS + 1) as usize;
    let base: u32 = match rng.below(3) {
        0 => 0,
        1 => 1 << 31,
        _ => u32::MAX - 64,
    };
    let mut pool: Vec<u32> = (0..64).collect();
    // A partial Fisher-Yates shuffle picks n distinct offsets from the pool.
    for i in 0..n {
        let j = i + rng.below((64 - i) as u64) as usize;
        pool.swap(i, j);
    }
    let items = pool[..n]
        .iter()
        .map(|&offset| (base + offset, weight(rng)))
        .collect();
    (k, items)
}

/// The `TopKFilter` result of one case, as `id:weight` tokens, best first.
fn run_case(k: u64, items: &[(u32, i64)]) -> String {
    let mut filter = TopKFilter::new(k as usize);
    for &(id, w) in items {
        filter.push(id, w);
    }
    filter
        .pop_all()
        .iter()
        .map(|e| format!("{}:{}", e.value, e.weight))
        .collect::<Vec<_>>()
        .join(" ")
}

fn generate(path: &str, count: u64) -> std::io::Result<()> {
    let mut rng = SplitMix(SEED);
    let mut out = BufWriter::new(File::create(path)?);
    for _ in 0..count {
        let (k, items) = case(&mut rng);
        let ids: Vec<u32> = items.iter().map(|&(id, _)| id).collect();
        let ws: Vec<i64> = items.iter().map(|&(_, w)| w).collect();
        writeln!(out, "{}", json!({ "k": k, "ids": ids, "ws": ws }))?;
    }
    out.flush()
}

/// Reads one case line into `(k, items)`.
fn parse_case(line: &str) -> (u64, Vec<(u32, i64)>) {
    let v: Value = serde_json::from_str(line).expect("a case line is JSON");
    let k = v["k"].as_u64().expect("k is an unsigned integer");
    let ids = v["ids"].as_array().expect("ids is an array");
    let ws = v["ws"].as_array().expect("ws is an array");
    assert_eq!(ids.len(), ws.len(), "ids and ws have the same length");
    let items = ids
        .iter()
        .zip(ws)
        .map(|(id, w)| {
            (
                id.as_u64().expect("id is an unsigned integer") as u32,
                w.as_i64().expect("w is a signed integer"),
            )
        })
        .collect();
    (k, items)
}

fn run(path: &str) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let (k, items) = parse_case(&line);
        writeln!(out, "{}", run_case(k, &items))?;
    }
    out.flush()
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [_, "gen", path, count] => generate(path, count.parse().expect("count is a number")),
        [_, "run", path] => run(path),
        _ => {
            eprintln!("usage: g9_topk gen <cases.jsonl> <count> | run <cases.jsonl>");
            std::process::exit(2);
        }
    }
}
