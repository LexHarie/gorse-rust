//! The functions against what Go computed (`tests/data/math32_golden.json`,
//! written by `contracts/tools/math32golden`), bit for bit.
//!
//! The `asm` column is math32 as Go runs it on amd64 and is checked against
//! the public functions on x86-64. The `noasm` column is math32's Go code
//! (built with `-tags noasm`) and is checked against the Go-code kernels on
//! every architecture.
//!
//! The golden also holds a digest of each unary function over all 2^32
//! inputs, per column, which the ignored test `math32_exhaustive` recomputes:
//!
//!     CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true \
//!         cargo test -p gorse-common --release math32_exhaustive -- --ignored

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Deserialize;

use super::*;

#[derive(Deserialize)]
struct Golden {
    /// Column ("asm" or "noasm"), then function name, then digest.
    exhaustive: BTreeMap<String, BTreeMap<String, String>>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    #[serde(rename = "fn")]
    func: String,
    x: String,
    #[serde(default)]
    y: String,
    asm: String,
    noasm: String,
}

fn load() -> Golden {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/math32_golden.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("golden file")).expect("golden JSON")
}

fn floats(hex_digits: &str) -> Vec<f32> {
    hex::decode(hex_digits)
        .expect("hex")
        .chunks(4)
        .map(|b| f32::from_bits(u32::from_be_bytes([b[0], b[1], b[2], b[3]])))
        .collect()
}

fn ints(s: &str) -> Vec<i64> {
    s.split(' ').map(|v| v.parse().expect("integer")).collect()
}

/// The bits a case's function gives for every input, in the golden's form.
fn run<K: Kernels>(case: &Case) -> String {
    let xs = floats(&case.x);
    let bits = |v: Vec<f32>| -> String {
        v.iter()
            .map(|x| format!("{:08x}", x.to_bits()))
            .collect::<String>()
    };
    match case.func.as_str() {
        "frexp" => {
            let (fr, ex): (Vec<f32>, Vec<i64>) = xs.iter().map(|&x| frexp(x)).unzip();
            let ex: Vec<String> = ex.iter().map(i64::to_string).collect();
            format!("{}|{}", bits(fr), ex.join(" "))
        }
        "modf" => {
            let (int, frac): (Vec<f32>, Vec<f32>) = xs.iter().map(|&x| modf(x)).unzip();
            format!("{}|{}", bits(int), bits(frac))
        }
        "copysign" => {
            let ys = floats(&case.y);
            bits(xs.iter().zip(&ys).map(|(&x, &y)| copysign(x, y)).collect())
        }
        "ldexp" => {
            let es = ints(&case.y);
            bits(xs.iter().zip(&es).map(|(&x, &e)| ldexp(x, e)).collect())
        }
        "pow" => {
            let ys = floats(&case.y);
            bits(
                xs.iter()
                    .zip(&ys)
                    .map(|(&x, &y)| pow_with::<K>(x, y))
                    .collect(),
            )
        }
        name => {
            let f: fn(f32) -> f32 = match name {
                "abs" => abs,
                "floor" => floor,
                "sqrt" => K::sqrt,
                "exp" => K::exp,
                "log" => K::log,
                "sin" => sin,
                "cos" => cos,
                "tanh" => tanh_with::<K>,
                other => panic!("unknown function {other}"),
            };
            bits(xs.iter().map(|&x| f(x)).collect())
        }
    }
}

/// The values of a golden column: space-separated integers, or IEEE bits as
/// 8 hex digits each.
fn values(s: &str) -> Vec<&str> {
    if s.contains(' ') {
        s.split(' ').collect()
    } else {
        (0..s.len())
            .step_by(8)
            .map(|i| &s[i..s.len().min(i + 8)])
            .collect()
    }
}

/// Compares value by value so a failure names the input.
fn check(case: &Case, column: &str, got: &str, want: &str) {
    if got == want {
        return;
    }
    let (xs, ys) = (values(&case.x), values(&case.y));
    // frexp and modf give two parts, separated by '|'.
    let first = got
        .split('|')
        .zip(want.split('|'))
        .enumerate()
        .find_map(|(part, (g, w))| {
            let (g, w) = (values(g), values(w));
            (0..g.len().max(w.len()))
                .find(|&i| g.get(i) != w.get(i))
                .map(|i| {
                    format!(
                        "part {part}, value {i}: got {:?}, want {:?}, x {:?}, y {:?}",
                        g.get(i),
                        w.get(i),
                        xs.get(i),
                        ys.get(i)
                    )
                })
        });
    panic!(
        "{} ({column}) differs from Go over {} inputs; first difference: {}",
        case.func,
        xs.len(),
        first.unwrap_or_else(|| "in the number of parts".to_string())
    );
}

#[test]
fn math32_matches_go_code() {
    let golden = load();
    assert_eq!(golden.cases.len(), 13);
    for case in &golden.cases {
        check(case, "noasm", &run::<GoCode>(case), &case.noasm);
    }
}

#[cfg(target_arch = "x86_64")]
#[test]
fn math32_matches_go_on_amd64() {
    let golden = load();
    for case in &golden.cases {
        check(case, "asm", &run::<Native>(case), &case.asm);
    }
}

/// The assembly paths are what the golden says they are: they differ from
/// the Go code exactly where the golden's two columns differ.
#[cfg(target_arch = "x86_64")]
#[test]
fn math32_amd64_differs_from_go_code() {
    let golden = load();
    let differing: Vec<&str> = golden
        .cases
        .iter()
        .filter(|c| c.asm != c.noasm)
        .map(|c| c.func.as_str())
        .collect();
    assert_eq!(differing, ["sqrt", "exp", "log", "tanh", "pow"]);
    assert_eq!(log(1e-40), -88.02122);
    assert_eq!(log_go(1e-40), -92.103_41);
    assert_eq!(exp(88.7), f32::INFINITY);
    assert_eq!(exp_go(88.7), 3.325_976_9e38);
    // The ranges the module documentation gives.
    assert!(exp(88.376_26).is_finite());
    assert_eq!(exp(88.376_27), f32::INFINITY);
    assert_eq!(exp(1.0e9), f32::INFINITY);
    assert_eq!(exp(2.0e9), 0.0);
    assert!(exp(-87.683_12) > 0.0);
    assert_eq!(exp(-87.683_13), 0.0);
    assert!(exp_go(-103.0) > 0.0);
    // At f1 = √2/2 rounded the assembly halves and the Go code does not.
    let half_sqrt2 = f32::from_bits(0x3f35_04f3);
    assert_eq!(log(half_sqrt2).to_bits(), 0xbeb1_7218);
    assert_eq!(log_go(half_sqrt2).to_bits(), 0xbeb1_7219);
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// One FNV-1a step over a 32-bit word, as the Go tool takes it.
fn fnv(h: u64, word: u32) -> u64 {
    (h ^ u64::from(word)).wrapping_mul(FNV_PRIME)
}

/// The words the Go tool hashes for one input: the result bits, and for frexp,
/// modf and int a second word (the exponent as uint32, the fractional part, or
/// the high half).
fn words<K: Kernels>(name: &str) -> fn(f32) -> (u32, Option<u32>) {
    match name {
        "abs" => |x| (abs(x).to_bits(), None),
        "floor" => |x| (floor(x).to_bits(), None),
        "frexp" => |x| {
            let (frac, exp) = frexp(x);
            (frac.to_bits(), Some(exp as u32))
        },
        "modf" => |x| {
            let (int, frac) = modf(x);
            (int.to_bits(), Some(frac.to_bits()))
        },
        "sqrt" => |x| (K::sqrt(x).to_bits(), None),
        "exp" => |x| (K::exp(x).to_bits(), None),
        "log" => |x| (K::log(x).to_bits(), None),
        "sin" => |x| (sin(x).to_bits(), None),
        "cos" => |x| (cos(x).to_bits(), None),
        "tanh" => |x| (tanh_with::<K>(x).to_bits(), None),
        "int32" => |x| (go_f32_to_i32(x) as u32, None),
        "int" => |x| {
            let v = go_f32_to_i64(x);
            (v as u32, Some((v >> 32) as u32))
        },
        other => panic!("unknown function {other}"),
    }
}

/// The Go tool's digest of a function over all 2^32 inputs: FNV-1a over each
/// block of 2^24 consecutive input bit patterns, then FNV-1a over the 256
/// block digests (each as two words, low half first).
fn exhaustive<K: Kernels>(name: &str) -> String {
    let f = words::<K>(name);
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut blocks = [0u64; 256];
    let done: Vec<Vec<(usize, u64)>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let b = next.fetch_add(1, Ordering::Relaxed);
                        if b >= 256 {
                            return out;
                        }
                        let mut h = FNV_OFFSET;
                        for i in 0..1u32 << 24 {
                            let (w0, w1) = f(f32::from_bits((b as u32) << 24 | i));
                            h = fnv(h, w0);
                            if let Some(w1) = w1 {
                                h = fnv(h, w1);
                            }
                        }
                        out.push((b, h));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().expect("digest worker"))
            .collect()
    });
    for (b, h) in done.into_iter().flatten() {
        blocks[b] = h;
    }
    let h = blocks
        .iter()
        .fold(FNV_OFFSET, |h, &d| fnv(fnv(h, d as u32), (d >> 32) as u32));
    format!("{h:016x}")
}

/// Every unary function, on every input, against the Go tool's digests.
#[test]
#[ignore = "evaluates each function on all 2^32 inputs; run in release (see the module doc)"]
fn math32_exhaustive() {
    let golden = load();
    let names = [
        "abs", "cos", "exp", "floor", "frexp", "int", "int32", "log", "modf", "sin", "sqrt", "tanh",
    ];
    type Digest = fn(&str) -> String;
    let mut columns: Vec<(&str, Digest)> = vec![("noasm", exhaustive::<GoCode>)];
    if cfg!(target_arch = "x86_64") {
        columns.push(("asm", exhaustive::<Native>));
    }
    let mut failures = Vec::new();
    for (column, digest) in columns {
        let want = &golden.exhaustive[column];
        assert_eq!(want.keys().map(String::as_str).collect::<Vec<_>>(), names);
        for (name, want) in want {
            let got = digest(name);
            eprintln!("{column} {name}: got {got}, want {want}");
            if got != *want {
                failures.push(format!("{column} {name}"));
            }
        }
    }
    assert!(failures.is_empty(), "digests differ from Go: {failures:?}");
}
