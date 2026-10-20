// Ports: math/rand regress_test.go (TestRegress) @ go1.27.0, and checks common/util/random.go @ add21b5

//! The Rust `math/rand` port and `util::RandomGenerator` against the outputs
//! of the Go oracle, written by `contracts/tools/randgolden` to
//! `tests/data/rand_golden.json`.

use std::collections::HashSet;
use std::sync::OnceLock;

use gorse_common::util::RandomGenerator;
use gorse_common::util::new_rand;
use gorse_common::util::rand::{Rand, Source};
use serde_json::Value;

fn golden() -> &'static Value {
    static GOLDEN: OnceLock<Value> = OnceLock::new();
    GOLDEN.get_or_init(|| {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/rand_golden.json");
        let text = std::fs::read_to_string(path).expect("read rand_golden.json");
        serde_json::from_str(&text).expect("parse rand_golden.json")
    })
}

fn section() -> &'static Value {
    &golden()["rand"]
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn int(v: &Value) -> i64 {
    v.as_i64().expect("integer")
}

fn hx(v: u64) -> String {
    format!("{v:016x}")
}

/// FNV-1a 64, as in the Go tool.
struct Digest(u64);

impl Digest {
    fn new() -> Self {
        Digest(14_695_981_039_346_656_037)
    }

    fn bytes(&mut self, b: &[u8]) {
        for &c in b {
            self.0 ^= u64::from(c);
            self.0 = self.0.wrapping_mul(1_099_511_628_211);
        }
    }

    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    fn hex(&self) -> String {
        hx(self.0)
    }
}

/// One output, in the forms the Go tool writes and hashes.
enum Out {
    Num(u64),
    Ints(Vec<i64>),
    Bytes(Vec<u8>),
}

impl Out {
    fn encode(&self) -> String {
        match self {
            Out::Num(v) => hx(*v),
            Out::Ints(v) => v.iter().map(i64::to_string).collect::<Vec<_>>().join(","),
            Out::Bytes(b) => b.iter().map(|c| format!("{c:02x}")).collect(),
        }
    }

    fn hash(&self, d: &mut Digest) {
        match self {
            Out::Num(v) => d.u64(*v),
            Out::Ints(v) => v.iter().for_each(|&e| d.u64(e.cast_unsigned())),
            Out::Bytes(b) => d.bytes(b),
        }
    }
}

fn call<S: Source>(r: &mut Rand<S>, method: &str, arg: i64) -> Out {
    match method {
        "ExpFloat64" => Out::Num(r.exp_float64().to_bits()),
        "Float32" => Out::Num(u64::from(r.float32().to_bits())),
        "Float64" => Out::Num(r.float64().to_bits()),
        "Int" => Out::Num(r.int().cast_unsigned()),
        "Int31" => Out::Num(i64::from(r.int31()).cast_unsigned()),
        "Int31n" => {
            Out::Num(i64::from(r.int31n(i32::try_from(arg).expect("int32"))).cast_unsigned())
        }
        "Int63" => Out::Num(r.int63().cast_unsigned()),
        "Int63n" => Out::Num(r.int63n(arg).cast_unsigned()),
        "Intn" => Out::Num(r.intn(arg).cast_unsigned()),
        "NormFloat64" => Out::Num(r.norm_float64().to_bits()),
        "Perm" => Out::Ints(r.perm(usize::try_from(arg).expect("size"))),
        "Shuffle" => {
            let mut a: Vec<i64> = (0..arg).collect();
            let n = a.len();
            r.shuffle(n, |i, j| a.swap(i, j));
            Out::Ints(a)
        }
        "Read" => {
            let mut b = vec![0; usize::try_from(arg).expect("size")];
            r.read(&mut b);
            Out::Bytes(b)
        }
        "Uint32" => Out::Num(u64::from(r.uint32())),
        "Uint64" => Out::Num(r.uint64()),
        _ => panic!("unknown method {method}"),
    }
}

#[test]
fn util_rand_inv_rn_matches_go() {
    // The constant 1.0 / rn that NormFloat64 multiplies by, as Go folds it.
    let go = u64::from_str_radix(s(&section()["inv_rn"]), 16).expect("hex");
    assert_eq!((1.0 / 3.442_619_855_899_f64).to_bits(), go);
}

/// math/rand's TestRegress: every value-returning method, 20 calls each from
/// Seed(0), with the arguments the Go test uses.
#[test]
fn util_rand_regress_matches_go() {
    let cases = section()["regress"].as_array().expect("regress");
    assert_eq!(
        cases.len(),
        14 * 20,
        "14 methods of rand.Rand return a value"
    );
    let mut r = Rand::new(0);
    let mut current = "";
    for c in cases {
        let method = s(&c["method"]);
        if method != current {
            r.seed(0);
            current = method;
        }
        let arg = s(&c["arg"]);
        let arg = if arg.is_empty() {
            0
        } else {
            arg.parse().expect("arg")
        };
        let out = call(&mut r, method, arg).encode();
        assert_eq!(out, s(&c["out"]), "{method}({arg})");
    }
}

fn check_streams<S: Source>(name: &str, new: impl Fn(i64) -> Rand<S>) {
    let streams = section()[name].as_array().expect("streams");
    assert!(!streams.is_empty());
    let mut calls = 0;
    for st in streams {
        let seed = int(&st["seed"]);
        let method = s(&st["method"]);
        let arg = int(&st["arg"]);
        let n = int(&st["n"]);
        let first: Vec<&str> = st["first"]
            .as_array()
            .expect("first")
            .iter()
            .map(s)
            .collect();
        let mut r = new(seed);
        let mut d = Digest::new();
        for i in 0..n {
            let out = call(&mut r, method, arg);
            if let Some(want) = first.get(i as usize) {
                assert_eq!(
                    &out.encode(),
                    want,
                    "{name} seed {seed} {method}({arg}) call {i}"
                );
            }
            out.hash(&mut d);
        }
        assert_eq!(
            d.hex(),
            s(&st["digest"]),
            "{name} seed {seed} {method}({arg}) x{n}"
        );
        calls += n;
    }
    assert!(calls > 1_000_000);
}

/// Long streams of every method for 15 seeds, from rand.New(rand.NewSource).
#[test]
fn util_rand_streams_match_go() {
    check_streams("streams", Rand::new);
}

/// The same streams from util.NewRand, whose source is not a Source64.
#[test]
fn util_rand_locked_streams_match_go() {
    check_streams("locked", new_rand);
}

/// Read keeps unused bytes across calls until Seed.
#[test]
fn util_rand_mixed_calls_match_go() {
    for mx in section()["mixed"].as_array().expect("mixed") {
        let mut r = Rand::new(int(&mx["seed"]));
        let outs = mx["outs"].as_array().expect("outs");
        for (call_text, want) in mx["calls"].as_array().expect("calls").iter().zip(outs) {
            let mut fields = s(call_text).split(' ');
            let method = fields.next().expect("method");
            let arg = fields.next().map_or(0, |a| a.parse().expect("arg"));
            let out = if method == "Seed" {
                r.seed(arg);
                String::new()
            } else {
                call(&mut r, method, arg).encode()
            };
            assert_eq!(out, s(want), "{}", s(call_text));
        }
    }
}

fn f(v: &Value) -> f64 {
    v.as_f64().expect("number")
}

fn sets<T: TryFrom<i64> + std::hash::Hash + Eq>(c: &Value) -> Vec<HashSet<T>> {
    c.get("exclude")
        .and_then(Value::as_array)
        .map(|sets| {
            sets.iter()
                .map(|set| {
                    set.as_array()
                        .expect("set")
                        .iter()
                        .map(|v| T::try_from(int(v)).ok().expect("element"))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn check_values(c: &Value, values: &[u64], len: usize, what: &str) {
    assert_eq!(len as i64, int(&c["len"]), "{what} length");
    // Go writes the first values of an empty output as null.
    let first = c["first"].as_array().map(Vec::as_slice).unwrap_or_default();
    for (i, want) in first.iter().enumerate() {
        assert_eq!(hx(values[i]), s(want), "{what} value {i}");
    }
    if let Some(digest) = c["digest"].as_str().filter(|d| !d.is_empty()) {
        let mut d = Digest::new();
        values.iter().for_each(|&v| d.u64(v));
        assert_eq!(d.hex(), digest, "{what} digest");
    }
}

/// util.RandomGenerator: the Go tests' calls and more, in sequence on one
/// generator per seed.
#[test]
fn util_random_generator_matches_go() {
    for case in section()["random_generator"]
        .as_array()
        .expect("random_generator")
    {
        let seed = int(&case["seed"]);
        let mut rng = RandomGenerator::new(seed);
        for c in case["calls"].as_array().expect("calls") {
            let method = s(&c["method"]);
            let a: Vec<f64> = c["args"]
                .as_array()
                .map(|a| a.iter().map(f).collect())
                .unwrap_or_default();
            let what = format!("seed {seed} {method}{a:?}");
            let bits32 = |v: Vec<f32>| {
                v.into_iter()
                    .map(|x| u64::from(x.to_bits()))
                    .collect::<Vec<_>>()
            };
            match method {
                "UniformVector" => {
                    let v = bits32(rng.uniform_vector(a[0] as usize, a[1] as f32, a[2] as f32));
                    check_values(c, &v, v.len(), &what);
                }
                "NormalVector" => {
                    let v = bits32(rng.normal_vector(a[0] as usize, a[1] as f32, a[2] as f32));
                    check_values(c, &v, v.len(), &what);
                }
                "NormalVector64" => {
                    let v: Vec<u64> = rng
                        .normal_vector64(a[0] as usize, a[1], a[2])
                        .into_iter()
                        .map(f64::to_bits)
                        .collect();
                    check_values(c, &v, v.len(), &what);
                }
                "NormalMatrix" | "UniformMatrix" => {
                    let m = if method == "NormalMatrix" {
                        rng.normal_matrix(a[0] as usize, a[1] as usize, a[2] as f32, a[3] as f32)
                    } else {
                        rng.uniform_matrix(a[0] as usize, a[1] as usize, a[2] as f32, a[3] as f32)
                    };
                    assert!(m.iter().all(|row| row.len() == a[1] as usize), "{what}");
                    let rows = m.len();
                    let v = bits32(m.into_iter().flatten().collect());
                    check_values(c, &v, rows, &what);
                }
                "Sample" => {
                    let sets = sets::<i64>(c);
                    let refs: Vec<&HashSet<i64>> = sets.iter().collect();
                    let v = rng.sample(a[0] as i64, a[1] as i64, a[2] as usize, &refs);
                    let v: Vec<u64> = v.into_iter().map(i64::cast_unsigned).collect();
                    check_values(c, &v, v.len(), &what);
                }
                "SampleInt32" => {
                    let sets = sets::<i32>(c);
                    let refs: Vec<&HashSet<i32>> = sets.iter().collect();
                    let v = rng.sample_int32(a[0] as i32, a[1] as i32, a[2] as usize, &refs);
                    let v: Vec<u64> = v
                        .into_iter()
                        .map(|x| i64::from(x).cast_unsigned())
                        .collect();
                    check_values(c, &v, v.len(), &what);
                }
                "Int63" => check_values(c, &[rng.int63().cast_unsigned()], 1, &what),
                "Intn" => check_values(c, &[rng.intn(a[0] as i64).cast_unsigned()], 1, &what),
                "Int31n" => {
                    let v = i64::from(rng.int31n(a[0] as i32)).cast_unsigned();
                    check_values(c, &[v], 1, &what);
                }
                _ => panic!("unknown method {method}"),
            }
        }
    }
}
