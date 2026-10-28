//! `floats` against what Go computes, bit for bit, at every kernel level of
//! an x86-64 CPU (`floats_golden.json`, written by
//! `contracts/tools/floatsgolden`).
//!
//! The inputs are the generator's splitmix64 sequences, reproduced here; each
//! function's output bits are hashed with SHA-256 exactly as the generator
//! hashes them. Each level is selected with the same `GODEBUG` value the
//! generator gave its child process. A level this CPU lacks is reported and
//! skipped, never replaced by another.

#![cfg(target_arch = "x86_64")]

use std::collections::BTreeMap;
use std::process::Command;

use gorse_common::floats::{self, Feature};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Golden {
    lengths: Vec<usize>,
    shapes: Vec<[usize; 3]>,
    f16c: BTreeMap<String, bool>,
    levels: BTreeMap<String, BTreeMap<String, Outcome>>,
}

#[derive(Deserialize)]
struct Outcome {
    hashes: Vec<String>,
    empty_panics: bool,
}

/// splitmix64, as `rng` in the generator, with its `specials`: NaNs of
/// random payloads and signs, quiet and signaling, infinities and zeros mixed
/// in with the ordinary values.
struct Rng {
    state: u64,
    specials: bool,
}

impl Rng {
    fn seed(index: u64, n: usize) -> Rng {
        Rng {
            state: index << 32 | u64::try_from(n).expect("length"),
            specials: false,
        }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A multiple of 2^-23 in [-1, 1), or with `specials`, one time in two a
    /// special value.
    fn float(&mut self) -> f32 {
        if self.specials {
            let k = self.next();
            if k.is_multiple_of(2) {
                let sign = (k >> 8) as u32 & 0x8000_0000;
                let payload = (k >> 16) as u32 & 0x3f_ffff;
                return f32::from_bits(match (k >> 1) % 4 {
                    0 => sign | 0x7fc0_0000 | payload,
                    1 => sign | 0x7f80_0000 | payload | 1,
                    2 => sign | 0x7f80_0000,
                    _ => sign,
                });
            }
        }
        let v = i32::try_from(self.next() >> 40).expect("24 bits") - (1 << 23);
        v as f32 / (1 << 23) as f32
    }

    fn floats(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.float()).collect()
    }

    /// `float32(math.Ldexp(float64(r.float()), e))` with `e` in -28..=17.
    fn wide(&mut self, n: usize) -> Vec<f32> {
        (0..n)
            .map(|_| {
                let e = i32::try_from(self.next() % 46).expect("small") - 28;
                (f64::from(self.float()) * 2.0_f64.powi(e)) as f32
            })
            .collect()
    }

    fn halves(&mut self, n: usize) -> Vec<u16> {
        (0..n)
            .map(|_| u16::try_from(self.next() & 0xffff).expect("16 bits"))
            .collect()
    }
}

#[derive(Default)]
struct Hasher(Vec<u8>);

impl Hasher {
    fn f32(&mut self, v: &[f32]) {
        for x in v {
            self.0.extend_from_slice(&x.to_bits().to_le_bytes());
        }
    }

    fn u16(&mut self, v: &[u16]) {
        for x in v {
            self.0.extend_from_slice(&x.to_le_bytes());
        }
    }

    fn sum(&self) -> String {
        hex::encode(&Sha256::digest(&self.0)[..8])
    }
}

type Run = fn(Feature, &mut Rng, usize, &mut Hasher);
type Empty = fn(Feature);

fn abs(mut v: Vec<f32>) -> Vec<f32> {
    for x in &mut v {
        *x = x.abs();
    }
    v
}

/// The generator's `functions()`, in its order (the index seeds the inputs).
/// Kernel operations run at the level under test; the others are the
/// package functions, which Go computes the same way at every level.
fn functions() -> Vec<(&'static str, Run, Empty)> {
    vec![
        (
            "dot",
            |f, r, n, h| h.f32(&[f.dot(&r.floats(n), &r.floats(n))]),
            |f| {
                f.dot(&[], &[]);
            },
        ),
        (
            "euclidean",
            |f, r, n, h| h.f32(&[f.euclidean(&r.floats(n), &r.floats(n))]),
            |f| {
                f.euclidean(&[], &[]);
            },
        ),
        (
            "mul_const_add_to",
            |f, r, n, h| {
                let (a, b, c) = (r.floats(n), r.floats(n), r.float());
                let mut dst = vec![0.0; n];
                // MulConstAddTo(a, c, b, dst) = feature.mulConstAddTo(a, c, b, dst)
                f.mul_const_add_to(&a, c, &b, &mut dst);
                h.f32(&dst);
            },
            |f| f.mul_const_add_to(&[], 1.0, &[], &mut []),
        ),
        (
            "mul_const_add",
            |f, r, n, h| {
                let (a, mut dst, c) = (r.floats(n), r.floats(n), r.float());
                f.mul_const_add(&a, c, &mut dst);
                h.f32(&dst);
            },
            |f| f.mul_const_add(&[], 1.0, &mut []),
        ),
        (
            "mul_const_to",
            |f, r, n, h| {
                let (a, c) = (r.floats(n), r.float());
                let mut dst = vec![0.0; n];
                f.mul_const_to(&a, c, &mut dst);
                h.f32(&dst);
            },
            |f| f.mul_const_to(&[], 1.0, &mut []),
        ),
        (
            "mul_const",
            |f, r, n, h| {
                let (mut dst, c) = (r.floats(n), r.float());
                f.mul_const(&mut dst, c);
                h.f32(&dst);
            },
            |f| f.mul_const(&mut [], 1.0),
        ),
        (
            "add_const",
            |f, r, n, h| {
                let (mut dst, c) = (r.floats(n), r.float());
                f.add_const(&mut dst, c);
                h.f32(&dst);
            },
            |f| f.add_const(&mut [], 1.0),
        ),
        (
            "sub",
            |f, r, n, h| {
                let (mut dst, s) = (r.floats(n), r.floats(n));
                f.sub(&mut dst, &s);
                h.f32(&dst);
            },
            |f| f.sub(&mut [], &[]),
        ),
        (
            "sub_to",
            |f, r, n, h| {
                let (a, b) = (r.floats(n), r.floats(n));
                let mut dst = vec![0.0; n];
                f.sub_to(&a, &b, &mut dst);
                h.f32(&dst);
            },
            |f| f.sub_to(&[], &[], &mut []),
        ),
        (
            "mul_to",
            |f, r, n, h| {
                let (a, b) = (r.floats(n), r.floats(n));
                let mut dst = vec![0.0; n];
                f.mul_to(&a, &b, &mut dst);
                h.f32(&dst);
            },
            |f| f.mul_to(&[], &[], &mut []),
        ),
        (
            "div_to",
            |f, r, n, h| {
                let (a, b) = (r.floats(n), r.floats(n));
                let mut dst = vec![0.0; n];
                f.div_to(&a, &b, &mut dst);
                h.f32(&dst);
            },
            |f| f.div_to(&[], &[], &mut []),
        ),
        (
            "sqrt_to",
            |f, r, n, h| {
                let a = abs(r.floats(n));
                let mut dst = vec![0.0; n];
                f.sqrt_to(&a, &mut dst);
                h.f32(&dst);
            },
            |f| f.sqrt_to(&[], &mut []),
        ),
        (
            "from_float32",
            |f, r, n, h| {
                let a = r.wide(n);
                let mut dst = vec![0; n];
                f.from_float32(&a, &mut dst);
                h.u16(&dst);
            },
            |f| f.from_float32(&[], &mut []),
        ),
        (
            "to_float32",
            |f, r, n, h| {
                let a = r.halves(n);
                let mut dst = vec![0.0; n];
                f.to_float32(&a, &mut dst);
                h.f32(&dst);
            },
            |f| f.to_float32(&[], &mut []),
        ),
        (
            "add",
            |_, r, n, h| {
                let (mut dst, s) = (r.floats(n), r.floats(n));
                floats::add(&mut dst, &s);
                h.f32(&dst);
            },
            |_| floats::add(&mut [], &[]),
        ),
        (
            "div",
            |_, r, n, h| {
                let (mut dst, s) = (r.floats(n), r.floats(n));
                floats::div(&mut dst, &s);
                h.f32(&dst);
            },
            |_| floats::div(&mut [], &[]),
        ),
        (
            "mul_add_to",
            |_, r, n, h| {
                let (a, b, mut c) = (r.floats(n), r.floats(n), r.floats(n));
                floats::mul_add_to(&a, &b, &mut c);
                h.f32(&c);
            },
            |_| floats::mul_add_to(&[], &[], &mut []),
        ),
        (
            "add_to",
            |_, r, n, h| {
                let (a, b) = (r.floats(n), r.floats(n));
                let mut dst = vec![0.0; n];
                floats::add_to(&a, &b, &mut dst);
                h.f32(&dst);
            },
            |_| floats::add_to(&[], &[], &mut []),
        ),
        (
            "sqrt",
            |_, r, n, h| {
                let mut a = abs(r.floats(n));
                floats::sqrt(&mut a);
                h.f32(&a);
            },
            |_| floats::sqrt(&mut []),
        ),
    ]
}

/// The generator's `mm(transA, transB)` on shape `shape`, with packed leading
/// dimensions.
fn run_mm(
    f: Feature,
    trans_a: bool,
    trans_b: bool,
    r: &mut Rng,
    shape: [usize; 3],
    h: &mut Hasher,
) {
    let [m, n, k] = shape;
    let (a, b, mut c) = (r.floats(m * k), r.floats(k * n), r.floats(m * n));
    let lda = if trans_a { m } else { k };
    let ldb = if trans_b { k } else { n };
    f.mm(trans_a, trans_b, m, n, k, &a, lda, &b, ldb, &mut c, n);
    h.f32(&c);
}

const TRANSPOSES: [(bool, bool); 4] = [(false, false), (false, true), (true, false), (true, true)];

/// The operations Go's SIMD builds send to a kernel, which index `a[0]` and
/// so panic on empty input (`PORTING_NOTES.md` X-9).
const KERNEL_OPERATIONS: &[&str] = &[
    "dot",
    "euclidean",
    "mul_const_add_to",
    "mul_const_add",
    "mul_const_to",
    "mul_const",
    "add_const",
    "sub",
    "sub_to",
    "mul_to",
    "div_to",
    "sqrt_to",
    "mm_false_false",
    "mm_false_true",
    "mm_true_false",
    "mm_true_true",
];

/// The generator's levels and the `GODEBUG` value that selects each.
const LEVELS: [(&str, &str); 3] = [
    ("avx512", ""),
    ("avx", "cpu.avx512f=off"),
    ("scalar", "cpu.avx=off"),
];

/// The level `floats_amd64.go` runs with these features.
fn level_name(f: Feature) -> &'static str {
    if f.contains(Feature::AVX512) {
        "avx512"
    } else if f.contains(Feature::AVX) {
        "avx"
    } else {
        "scalar"
    }
}

fn load() -> Golden {
    let path = format!(
        "{}/tests/data/floats_golden.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("golden file")).expect("golden JSON")
}

#[test]
fn every_level_matches_go() {
    let golden = load();
    let functions = functions();
    let mut checked = 0;
    let mut mismatches = Vec::new();
    for (level, godebug) in LEVELS {
        let f = Feature::detect(Some(godebug));
        if level_name(f) != level {
            eprintln!(
                "skipped level {level}: this CPU runs {} with GODEBUG={godebug:?}",
                level_name(f)
            );
            continue;
        }
        if f.contains(Feature::F16C) != golden.f16c[level] {
            eprintln!(
                "note: level {level} here {} F16C, Go's run {} (the conversions agree either way)",
                if f.contains(Feature::F16C) {
                    "has"
                } else {
                    "lacks"
                },
                if golden.f16c[level] { "had" } else { "lacked" },
            );
        }
        let expected = &golden.levels[level];
        assert_eq!(
            expected.len(),
            2 * (functions.len() + TRANSPOSES.len()),
            "{level}: functions"
        );
        // Ordinary inputs, then inputs with special values under
        // "specials/<name>" with seeds offset by 1000, as the generator does.
        for (specials, prefix, offset) in [(false, "", 0), (true, "specials/", 1000)] {
            let rng = |index: u64, n: usize| Rng {
                specials,
                ..Rng::seed(offset + index, n)
            };
            for (index, (name, run, empty)) in functions.iter().enumerate() {
                let name = format!("{prefix}{name}");
                let outcome = &expected[&name];
                assert_eq!(outcome.hashes.len(), golden.lengths.len());
                for (&n, want) in golden.lengths.iter().zip(&outcome.hashes) {
                    let mut h = Hasher::default();
                    run(f, &mut rng(index as u64, n), n, &mut h);
                    if &h.sum() != want {
                        mismatches.push(format!("{level}: {name} on length {n}"));
                    }
                }
                check_empty(level, &name[prefix.len()..], outcome.empty_panics);
                empty(f);
            }
            for (j, (trans_a, trans_b)) in TRANSPOSES.into_iter().enumerate() {
                let name = format!("{prefix}mm_{trans_a}_{trans_b}");
                let outcome = &expected[&name];
                assert_eq!(outcome.hashes.len(), golden.shapes.len());
                for (s, (&shape, want)) in golden.shapes.iter().zip(&outcome.hashes).enumerate() {
                    let mut h = Hasher::default();
                    run_mm(
                        f,
                        trans_a,
                        trans_b,
                        &mut rng(100 + j as u64, s),
                        shape,
                        &mut h,
                    );
                    if &h.sum() != want {
                        mismatches.push(format!("{level}: {name} on shape {shape:?}"));
                    }
                }
                check_empty(level, &name[prefix.len()..], outcome.empty_panics);
                f.mm(trans_a, trans_b, 0, 0, 0, &[], 0, &[], 0, &mut [], 0);
            }
        }
        checked += 1;
        eprintln!("level {level} ({f}) checked");
    }
    assert!(
        mismatches.is_empty(),
        "{} results differ from Go's:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    assert!(checked > 0, "no level of the golden file runs on this CPU");
}

/// Go panics on empty input exactly where X-9 says: the kernel operations at
/// the SIMD levels. The Rust calls after each check show the port does not.
fn check_empty(level: &str, name: &str, go_panics: bool) {
    let expected = level != "scalar" && KERNEL_OPERATIONS.contains(&name);
    assert_eq!(go_panics, expected, "{level}: Go's verdict on empty {name}");
}

/// `GODEBUG` in the environment selects the level of the package functions,
/// as in Go. The test runs itself in child processes with each value.
#[test]
fn godebug_selects_the_process_level() {
    const CHILD: &str = "GORSE_FLOATS_EXPECTED_LEVEL";
    if let Ok(expected) = std::env::var(CHILD) {
        assert_eq!(Feature::current().to_string(), expected);
        let (a, b) = (Rng::seed(7, 33).floats(33), Rng::seed(8, 33).floats(33));
        assert_eq!(
            floats::dot(&a, &b).to_bits(),
            Feature::detect(std::env::var("GODEBUG").ok().as_deref())
                .dot(&a, &b)
                .to_bits()
        );
        return;
    }
    let exe = std::env::current_exe().expect("test binary");
    for godebug in ["", "cpu.avx512f=off", "cpu.avx=off", "cpu.all=off"] {
        let expected = Feature::detect(Some(godebug)).to_string();
        let output = Command::new(&exe)
            .args([
                "--exact",
                "godebug_selects_the_process_level",
                "--nocapture",
            ])
            .env("GODEBUG", godebug)
            .env(CHILD, &expected)
            .output()
            .expect("run the test binary");
        assert!(
            output.status.success(),
            "GODEBUG={godebug:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("1 passed"),
            "GODEBUG={godebug:?}: the child ran no test: {stdout}"
        );
    }
}
