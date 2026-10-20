//! The port against what Go computes: `tests/data/nn_golden.json`, recorded
//! by `contracts/tools/nngolden` (run.sh), case by case and bit for bit, at
//! each kernel level. Each case below is the Go case of the same name in
//! `ops.go`, `scenarios.go` and `saveload.go` there, line for line.
//!
//! The level is chosen by `GODEBUG`, which the process reads once, so the
//! test runs itself in a child process per level. A level this CPU cannot
//! run is skipped and said so, never replaced by another.
//!
//! Two kinds of entries are inputs rather than results: the draws of Go's
//! seeded `math/rand` sources (`draws`, `normals`), which the Rust cases
//! replay through [`Replay`] because Go's generator is not ported here.

#![cfg(target_arch = "x86_64")]

use std::any::Any;
use std::collections::{BTreeMap, VecDeque};
use std::panic::{self, AssertUnwindSafe};
use std::process::Command;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::floats::Feature;
use crate::math32;

use super::optimizers::partition_aligned;
use super::tensor::partition;
use super::*;

/// splitmix64, as the Go tool's `rng`.
struct Seq(u64);

impl Seq {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A multiple of 2^-23 in [-1, 1).
    fn float(&mut self) -> f32 {
        let v = i32::try_from(self.next() >> 40).expect("24 bits") - (1 << 23);
        v as f32 / (1 << 23) as f32
    }
}

fn size(shape: &[usize]) -> usize {
    shape.iter().product()
}

/// `n` values of the sequence `seed`, times `scale`.
fn gen_values(seed: u64, n: usize, scale: f32) -> Vec<f32> {
    let mut r = Seq(seed);
    (0..n).map(|_| r.float() * scale).collect()
}

/// `n` values in [0.5, 1.5).
fn positive(seed: u64, n: usize) -> Vec<f32> {
    let mut r = Seq(seed);
    (0..n).map(|_| 0.5 + r.float().abs()).collect()
}

/// `n` class indices below `classes`.
fn labels(seed: u64, n: usize, classes: u64) -> Vec<f32> {
    let mut r = Seq(seed);
    (0..n).map(|_| (r.next() % classes) as f32).collect()
}

/// `n` values that are -1 or 1.
fn signs(seed: u64, n: usize) -> Vec<f32> {
    let mut r = Seq(seed);
    (0..n)
        .map(|_| if r.next() % 2 == 0 { -1.0 } else { 1.0 })
        .collect()
}

fn tensor(seed: u64, shape: &[usize]) -> Tensor {
    new_tensor(gen_values(seed, size(shape), 1.0), shape)
}

fn scaled(seed: u64, scale: f32, shape: &[usize]) -> Tensor {
    new_tensor(gen_values(seed, size(shape), scale), shape)
}

fn lit(data: &[f32], shape: &[usize]) -> Tensor {
    new_tensor(data.to_vec(), shape)
}

fn bits(b: u32) -> f32 {
    f32::from_bits(b)
}

const NAN: f32 = f32::NAN;
const INF: f32 = f32::INFINITY;

fn scalar(v: f32) -> Tensor {
    new_scalar(v)
}

/// Overwrites a parameter's data in place.
fn set_data(t: &Tensor, seed: u64, scale: f32) {
    let values = gen_values(seed, t.len(), scale);
    t.write(|d| d.copy_from_slice(&values));
}

/// The Go tool's `floats`: 8 hex digits per value, or a digest for more
/// than 64.
fn encode(d: &[f32]) -> String {
    if d.len() <= 64 {
        return d.iter().map(|x| format!("{:08x}", x.to_bits())).collect();
    }
    let mut h = Sha256::new();
    for x in d {
        h.update(x.to_bits().to_le_bytes());
    }
    format!("sha256:{}:{}", hex::encode(&h.finalize()[..16]), d.len())
}

fn tensor_value(t: &Tensor) -> Value {
    json!({"shape": t.shape(), "data": encode(&t.data())})
}

fn message(e: &(dyn Any + Send)) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "<no message>".to_owned())
}

/// `{"panic": message}` if `f` panics, null otherwise.
fn catch(f: impl FnOnce()) -> Value {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(()) => Value::Null,
        Err(e) => json!({"panic": message(&*e)}),
    }
}

/// Go's `errValue`, with the panics Go's `Load` throws, which this port
/// returns as [`Error::Panic`], as the panics they are in Go.
fn result_value(r: Result<(), Error>) -> Value {
    match r {
        Ok(()) => Value::Null,
        Err(Error::Panic(m)) => json!({"panic": m}),
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// The ordered entries of a case.
#[derive(Default)]
struct Rec(Vec<(String, Value)>);

impl Rec {
    fn add(&mut self, name: impl Into<String>, v: Value) {
        self.0.push((name.into(), v));
    }

    fn t(&mut self, name: impl Into<String>, t: &Tensor) {
        self.add(name, tensor_value(t));
    }

    fn t_opt(&mut self, name: impl Into<String>, t: Option<Tensor>) {
        self.add(name, t.as_ref().map_or(Value::Null, tensor_value));
    }

    fn p(&mut self, name: impl Into<String>, f: impl FnOnce()) {
        self.add(name, catch(f));
    }
}

/// The Go draws of a seeded `*rand.Rand`, replayed.
struct Replay {
    floats: VecDeque<f32>,
    normals: VecDeque<f64>,
}

impl Rng for Replay {
    fn float32(&mut self) -> f32 {
        self.floats.pop_front().expect("a recorded Float32 draw")
    }

    fn norm_float64(&mut self) -> f64 {
        self.normals
            .pop_front()
            .expect("a recorded NormFloat64 draw")
    }
}

fn parse_floats(s: &str) -> Vec<f32> {
    (0..s.len())
        .step_by(8)
        .map(|i| f32::from_bits(u32::from_str_radix(&s[i..i + 8], 16).expect("hex")))
        .collect()
}

fn parse_float64s(s: &str) -> Vec<f64> {
    (0..s.len())
        .step_by(16)
        .map(|i| f64::from_bits(u64::from_str_radix(&s[i..i + 16], 16).expect("hex")))
        .collect()
}

/// The entry `name` of the golden case `case`, a string.
fn input<'a>(
    golden: &'a BTreeMap<String, Vec<(String, Value)>>,
    case: &str,
    name: &str,
) -> &'a str {
    golden[case]
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, v)| v.as_str())
        .unwrap_or_else(|| panic!("{case}: no input {name}"))
}

type Case = Box<dyn Fn(&mut Rec, &BTreeMap<String, Vec<(String, Value)>>)>;

fn case(cases: &mut Vec<(String, Case)>, name: impl Into<String>, f: impl Fn(&mut Rec) + 'static) {
    cases.push((name.into(), Box::new(move |r, _| f(r))));
}

/// Applies `f` to the inputs and records the output (or the panic), then
/// runs `backward` on the output and records the panic if any, the gradient
/// of each input, the output's own gradient and the output's data
/// afterwards.
fn op_case(r: &mut Rec, f: impl FnOnce(&[Tensor]) -> Tensor, inputs: Vec<Tensor>) {
    let mut out = None;
    let p = catch(|| out = Some(f(&inputs)));
    let Some(out) = out else {
        r.add("out", p);
        return;
    };
    r.t("out", &out);
    r.p("backward", || out.backward());
    for (i, x) in inputs.iter().enumerate() {
        r.t_opt(format!("grad{i}"), x.grad());
    }
    r.t_opt("out.grad", out.grad());
    r.t("out.after", &out);
}

fn tf(b: bool) -> &'static str {
    if b { "t" } else { "f" }
}

fn add_all(inputs: &[Tensor]) -> Tensor {
    let rest: Vec<&Tensor> = inputs[1..].iter().collect();
    add(&inputs[0], &rest)
}

fn op_cases(cs: &mut Vec<(String, Case)>) {
    let unary = |f: fn(&Tensor) -> Tensor| move |inputs: &[Tensor]| f(&inputs[0]);
    let binop =
        |f: fn(&Tensor, &Tensor) -> Tensor| move |inputs: &[Tensor]| f(&inputs[0], &inputs[1]);
    case(cs, "op/neg", move |r| {
        op_case(r, unary(neg), vec![tensor(1, &[2, 3])])
    });
    case(cs, "op/add/same", |r| {
        op_case(r, add_all, vec![tensor(2, &[2, 3]), tensor(3, &[2, 3])])
    });
    case(cs, "op/add/scalar", |r| {
        op_case(r, add_all, vec![tensor(4, &[2, 3]), tensor(5, &[])])
    });
    case(cs, "op/add/suffix", |r| {
        op_case(r, add_all, vec![tensor(6, &[2, 3]), tensor(7, &[3])])
    });
    case(cs, "op/add/scalar_first", |r| {
        op_case(r, add_all, vec![tensor(8, &[]), tensor(9, &[2, 3])])
    });
    case(cs, "op/add/three", |r| {
        op_case(
            r,
            add_all,
            vec![tensor(10, &[2, 3]), tensor(11, &[3]), tensor(12, &[2, 3])],
        );
    });
    case(cs, "op/add/flat_quirk", |r| {
        op_case(r, add_all, vec![tensor(13, &[6]), tensor(14, &[2, 3])])
    });
    case(cs, "op/add/negative_index", |r| {
        op_case(
            r,
            add_all,
            vec![
                tensor(15, &[3]),
                tensor(16, &[2, 3]),
                tensor(17, &[4, 2, 3]),
            ],
        );
    });
    case(cs, "op/add/mismatch", |r| {
        op_case(r, add_all, vec![tensor(18, &[2, 3]), tensor(19, &[2])])
    });
    case(cs, "op/add/same_input", |r| {
        op_case(r, |i| add(&i[0], &[&i[0]]), vec![tensor(20, &[2, 3])]);
    });
    case(cs, "op/add/one", |r| {
        op_case(r, add_all, vec![tensor(21, &[2, 3])])
    });
    case(cs, "op/sub/same", move |r| {
        op_case(
            r,
            binop(sub),
            vec![tensor(22, &[2, 3]), tensor(23, &[2, 3])],
        )
    });
    case(cs, "op/sub/scalar", move |r| {
        op_case(r, binop(sub), vec![tensor(24, &[2, 3]), tensor(25, &[])])
    });
    case(cs, "op/sub/suffix", move |r| {
        op_case(r, binop(sub), vec![tensor(26, &[2, 3]), tensor(27, &[3])])
    });
    case(cs, "op/sub/longer", move |r| {
        op_case(r, binop(sub), vec![tensor(28, &[3]), tensor(29, &[2, 3])])
    });
    case(cs, "op/sub/mismatch", move |r| {
        op_case(r, binop(sub), vec![tensor(30, &[2, 3]), tensor(31, &[2])])
    });
    case(cs, "op/mul/same", move |r| {
        op_case(
            r,
            binop(mul),
            vec![tensor(32, &[2, 3]), tensor(33, &[2, 3])],
        )
    });
    case(cs, "op/mul/scalar", move |r| {
        op_case(r, binop(mul), vec![tensor(34, &[2, 3]), tensor(35, &[])])
    });
    case(cs, "op/mul/suffix", move |r| {
        op_case(r, binop(mul), vec![tensor(36, &[2, 3]), tensor(37, &[3])])
    });
    case(cs, "op/mul/swap", move |r| {
        op_case(r, binop(mul), vec![tensor(38, &[3]), tensor(39, &[2, 3])])
    });
    case(cs, "op/mul/mismatch", move |r| {
        op_case(r, binop(mul), vec![tensor(40, &[2, 3]), tensor(41, &[2])])
    });
    case(cs, "op/mul/same_input", |r| {
        op_case(r, |i| mul(&i[0], &i[0]), vec![tensor(42, &[2, 3])])
    });
    case(cs, "op/div/same", move |r| {
        op_case(
            r,
            binop(div),
            vec![tensor(43, &[2, 3]), lit(&positive(44, 6), &[2, 3])],
        );
    });
    case(cs, "op/div/scalar", move |r| {
        op_case(
            r,
            binop(div),
            vec![tensor(45, &[2, 3]), lit(&positive(46, 1), &[])],
        );
    });
    case(cs, "op/div/suffix", move |r| {
        op_case(
            r,
            binop(div),
            vec![tensor(47, &[2, 3]), lit(&positive(48, 3), &[3])],
        );
    });
    case(cs, "op/div/longer", move |r| {
        op_case(r, binop(div), vec![tensor(49, &[3]), tensor(50, &[2, 3])])
    });
    case(cs, "op/div/mismatch", move |r| {
        op_case(r, binop(div), vec![tensor(51, &[2, 3]), tensor(52, &[2])])
    });
    case(cs, "op/div/zero", move |r| {
        let neg_zero = bits(0x8000_0000);
        op_case(
            r,
            binop(div),
            vec![
                lit(&[1.0, -1.0, 0.0, 0.0, 2.0, neg_zero], &[2, 3]),
                lit(&[0.0, 0.0, 0.0, neg_zero, 0.0, 1.0], &[2, 3]),
            ],
        );
    });
    case(cs, "op/square", move |r| {
        op_case(r, unary(square), vec![tensor(53, &[2, 3])])
    });
    case(cs, "op/square/long", move |r| {
        op_case(r, unary(square), vec![scaled(54, 1e20, &[37])])
    });
    case(cs, "op/pow/same", move |r| {
        op_case(
            r,
            binop(pow),
            vec![lit(&positive(55, 6), &[2, 3]), scaled(56, 3.0, &[2, 3])],
        );
    });
    case(cs, "op/pow/scalar", move |r| {
        op_case(
            r,
            binop(pow),
            vec![lit(&positive(57, 6), &[2, 3]), lit(&[2.0], &[])],
        );
    });
    case(cs, "op/pow/suffix_quirk", move |r| {
        op_case(
            r,
            binop(pow),
            vec![lit(&positive(58, 6), &[2, 3]), tensor(59, &[3])],
        );
    });
    case(cs, "op/pow/longer", move |r| {
        op_case(
            r,
            binop(pow),
            vec![lit(&positive(60, 3), &[3]), tensor(61, &[2, 3])],
        );
    });
    case(cs, "op/pow/specials", move |r| {
        op_case(
            r,
            binop(pow),
            vec![
                lit(&[0.0, -2.0, 2.0, 0.5, -0.5, 4.0], &[6]),
                lit(&[2.0, 3.0, 0.5, -1.0, 0.5, 0.0], &[6]),
            ],
        );
    });
    case(cs, "op/exp/specials", move |r| {
        let x = [
            -104.0,
            -103.97,
            -88.0,
            -87.5,
            -1.0,
            -1e-8,
            0.0,
            bits(0x8000_0000),
            1e-8,
            0.5,
            1.0,
            10.0,
            88.7,
            88.72284,
            88.73,
            89.0,
            710.0,
            1e30,
            NAN,
            INF,
            -INF,
        ];
        op_case(r, unary(exp), vec![lit(&x, &[21])]);
    });
    case(cs, "op/exp/range", move |r| {
        op_case(r, unary(exp), vec![scaled(62, 100.0, &[50])])
    });
    case(cs, "op/log/specials", move |r| {
        let x = [
            0.0,
            bits(0x8000_0000),
            1.0,
            2.0,
            0.5,
            1e-38,
            1e-45,
            3.4e38,
            -1.0,
            NAN,
            INF,
            -INF,
        ];
        op_case(r, unary(log), vec![lit(&x, &[12])]);
    });
    case(cs, "op/log/range", move |r| {
        op_case(r, unary(log), vec![lit(&positive(63, 40), &[5, 8])])
    });
    case(cs, "op/sin", move |r| {
        op_case(r, unary(sin), vec![scaled(64, 10.0, &[4, 5])])
    });
    case(cs, "op/cos", move |r| {
        op_case(r, unary(cos), vec![scaled(65, 10.0, &[4, 5])])
    });
    // Go's constant `-0.0` is +0.
    case(cs, "op/sin/large", move |r| {
        let x = [1e5, 1e10, -3e7, 3.4e38, NAN, INF, 3.141_592_7, 0.0];
        op_case(r, unary(sin), vec![lit(&x, &[8])]);
    });
    case(cs, "op/cos/large", move |r| {
        let x = [1e5, 1e10, -3e7, 3.4e38, NAN, INF, 3.141_592_7, 0.0];
        op_case(r, unary(cos), vec![lit(&x, &[8])]);
    });
    case(cs, "op/abs/specials", move |r| {
        let x = [bits(0x8000_0000), 0.0, -1.5, 2.0, NAN, -INF, INF, -1e-45];
        op_case(r, unary(abs), vec![lit(&x, &[8])]);
    });
    case(cs, "op/sum/all", |r| {
        op_case(r, |i| sum(&i[0], &[]), vec![tensor(66, &[3, 4])])
    });
    case(cs, "op/sum/long", |r| {
        op_case(r, |i| sum(&i[0], &[]), vec![tensor(67, &[1000])])
    });
    case(cs, "op/sum/two_alongs", |r| {
        op_case(r, |i| sum(&i[0], &[0, 1]), vec![tensor(68, &[2, 3])])
    });
    case(cs, "op/mean", move |r| {
        op_case(r, unary(mean), vec![tensor(69, &[3, 4])])
    });
    case(cs, "op/mean/long", move |r| {
        op_case(r, unary(mean), vec![tensor(70, &[999])])
    });
    case(cs, "op/mse", move |r| {
        op_case(
            r,
            binop(mean_square_error),
            vec![tensor(71, &[4, 3]), tensor(72, &[4, 3])],
        );
    });
    for along in [-1isize, 0, 1, 2, 3] {
        case(cs, format!("op/sum/along{along}"), move |r| {
            op_case(r, |i| sum(&i[0], &[along]), vec![tensor(73, &[2, 3, 2])]);
        });
    }
    case(cs, "op/sum/along_1d", |r| {
        op_case(r, |i| sum(&i[0], &[0]), vec![tensor(74, &[5])])
    });

    // MatMul over the four transpositions, shapes that reach the kernels'
    // tails, and job counts that split the rows differently.
    let mut seed = 100;
    for t1 in [false, true] {
        for t2 in [false, true] {
            for (m, k, n) in [(2, 3, 4), (9, 17, 5), (33, 20, 31), (64, 64, 10)] {
                let jobs: &[usize] = if m == 33 { &[0, 1, 3, 8, 100] } else { &[0, 3] };
                for &j in jobs {
                    let xs = if t1 { [k, m] } else { [m, k] };
                    let ys = if t2 { [n, k] } else { [k, n] };
                    let (sx, sy) = (seed, seed + 1);
                    seed += 2;
                    case(
                        cs,
                        format!("op/matmul/{}{}/{m}x{k}x{n}/j{j}", tf(t1), tf(t2)),
                        move |r| {
                            op_case(
                                r,
                                |i| mat_mul(&i[0], &i[1], t1, t2, j),
                                vec![tensor(sx, &xs), tensor(sy, &ys)],
                            );
                        },
                    );
                }
            }
            case(
                cs,
                format!("op/matmul/{}{}/mismatch", tf(t1), tf(t2)),
                move |r| {
                    op_case(
                        r,
                        |i| mat_mul(&i[0], &i[1], t1, t2, 0),
                        vec![tensor(200, &[2, 3]), tensor(201, &[4, 5])],
                    );
                },
            );
        }
    }
    case(cs, "op/matmul/not2d", |r| {
        op_case(
            r,
            |i| mat_mul(&i[0], &i[1], false, false, 0),
            vec![tensor(202, &[2, 3, 1]), tensor(203, &[3, 4])],
        );
    });

    // BMM likewise; fm.go calls it with transpose1.
    let mut seed = 300;
    for t1 in [false, true] {
        for t2 in [false, true] {
            for (b, m, k, n) in [(2, 2, 3, 4), (3, 7, 9, 5), (5, 16, 17, 8)] {
                for j in [0, 4] {
                    let xs = if t1 { [b, k, m] } else { [b, m, k] };
                    let ys = if t2 { [b, n, k] } else { [b, k, n] };
                    let (sx, sy) = (seed, seed + 1);
                    seed += 2;
                    case(
                        cs,
                        format!("op/bmm/{}{}/{b}x{m}x{k}x{n}/j{j}", tf(t1), tf(t2)),
                        move |r| {
                            op_case(
                                r,
                                |i| bmm(&i[0], &i[1], t1, t2, j),
                                vec![tensor(sx, &xs), tensor(sy, &ys)],
                            );
                        },
                    );
                }
            }
            case(
                cs,
                format!("op/bmm/{}{}/mismatch", tf(t1), tf(t2)),
                move |r| {
                    op_case(
                        r,
                        |i| bmm(&i[0], &i[1], t1, t2, 0),
                        vec![tensor(400, &[2, 2, 3]), tensor(401, &[3, 5, 6])],
                    );
                },
            );
        }
    }
    case(cs, "op/bmm/not3d", |r| {
        op_case(
            r,
            |i| bmm(&i[0], &i[1], false, false, 0),
            vec![tensor(402, &[2, 3]), tensor(403, &[3, 4])],
        );
    });

    case(cs, "op/broadcast/1d", |r| {
        op_case(r, |i| broadcast(&i[0], &[3]), vec![tensor(500, &[2])])
    });
    case(cs, "op/broadcast/2d", |r| {
        op_case(r, |i| broadcast(&i[0], &[3, 2]), vec![tensor(501, &[2, 2])])
    });
    case(cs, "op/broadcast/none", |r| {
        op_case(r, |i| broadcast(&i[0], &[]), vec![tensor(502, &[2, 3])])
    });
    case(cs, "op/flatten", move |r| {
        op_case(r, unary(flatten), vec![tensor(503, &[2, 3])])
    });
    case(cs, "op/flatten/scalar", move |r| {
        op_case(r, unary(flatten), vec![tensor(504, &[])])
    });
    case(cs, "op/reshape", |r| {
        op_case(r, |i| reshape(&i[0], &[3, 2]), vec![tensor(505, &[2, 3])])
    });
    case(cs, "op/reshape/mismatch", |r| {
        op_case(r, |i| reshape(&i[0], &[4]), vec![tensor(506, &[2, 3])])
    });
    case(cs, "op/reshape/scalar_to_1", |r| {
        op_case(r, |i| reshape(&i[0], &[1]), vec![tensor(507, &[])])
    });
    case(cs, "op/reshape/to_scalar", |r| {
        op_case(r, |i| reshape(&i[0], &[]), vec![tensor(508, &[1])])
    });
    let idx = [0.0, 1.0, 0.0, 3.0, 0.0, 5.0];
    case(cs, "op/embedding/2d", move |r| {
        op_case(
            r,
            binop(embedding),
            vec![tensor(509, &[6, 2]), lit(&idx, &[2, 3])],
        );
    });
    case(cs, "op/embedding/3d", move |r| {
        op_case(
            r,
            binop(embedding),
            vec![tensor(510, &[6, 1, 2]), lit(&idx, &[2, 3])],
        );
    });
    case(cs, "op/embedding/truncation", move |r| {
        op_case(
            r,
            binop(embedding),
            vec![tensor(511, &[6, 2]), lit(&[2.7, 0.2, 5.99, -0.5], &[4])],
        );
    });
    case(cs, "op/embedding/nan_dim2", move |r| {
        op_case(
            r,
            binop(embedding),
            vec![tensor(512, &[6, 2]), lit(&[NAN, 1.0], &[2])],
        );
    });
    case(cs, "op/embedding/nan_dim1", move |r| {
        op_case(
            r,
            binop(embedding),
            vec![tensor(513, &[6, 1]), lit(&[NAN, 1.0], &[2])],
        );
    });
    case(cs, "op/embedding/out_of_range", move |r| {
        op_case(
            r,
            binop(embedding),
            vec![tensor(514, &[6, 2]), lit(&[6.0], &[1])],
        );
    });
    case(cs, "op/embedding/negative", move |r| {
        op_case(
            r,
            binop(embedding),
            vec![tensor(515, &[6, 2]), lit(&[-1.0], &[1])],
        );
    });
    case(cs, "op/embedding/huge", move |r| {
        op_case(
            r,
            binop(embedding),
            vec![tensor(516, &[6, 2]), lit(&[3e9], &[1])],
        );
    });
    case(cs, "op/embedding/view", move |r| {
        let w = tensor(517, &[6, 2]).slice(0, 3);
        op_case(r, binop(embedding), vec![w, lit(&[4.0, 1.0], &[2])]);
    });
    case(cs, "op/sigmoid", move |r| {
        op_case(r, unary(sigmoid), vec![scaled(518, 8.0, &[3, 4])])
    });
    case(cs, "op/sigmoid/specials", move |r| {
        let x = [NAN, INF, -INF, 100.0, -100.0, 0.0, bits(0x8000_0000)];
        op_case(r, unary(sigmoid), vec![lit(&x, &[7])]);
    });
    case(cs, "op/relu", move |r| {
        op_case(r, unary(relu), vec![tensor(519, &[3, 4])])
    });
    case(cs, "op/relu/specials", move |r| {
        let x = [
            NAN,
            bits(0x7fc1_2345),
            bits(0xffc0_0000),
            bits(0x7f80_0001),
            bits(0x8000_0000),
            0.0,
            -1.0,
            1.0,
            INF,
            -INF,
        ];
        op_case(r, unary(relu), vec![lit(&x, &[10])]);
    });
    let softmaxes: [(&str, &[usize], isize); 8] = [
        ("1x3_axis1", &[1, 3], 1),
        ("2x3_axis1", &[2, 3], 1),
        ("3x2_axis0", &[3, 2], 0),
        ("2x3x4_axis1", &[2, 3, 4], 1),
        ("4_axis0", &[4], 0),
        ("2x3_axis2", &[2, 3], 2),
        ("2x3_axis-1", &[2, 3], -1),
        ("3x1_axis1", &[3, 1], 1),
    ];
    for (name, shape, axis) in softmaxes {
        case(cs, format!("op/softmax/{name}"), move |r| {
            op_case(r, |i| softmax(&i[0], axis), vec![scaled(520, 3.0, shape)]);
        });
    }
    case(cs, "op/softmax/go_test", |r| {
        op_case(
            r,
            |i| softmax(&i[0], 1),
            vec![lit(&[3.0, 1.0, 0.2], &[1, 3])],
        )
    });
    let sce = binop(softmax_cross_entropy);
    case(cs, "op/sce/go_test", move |r| {
        op_case(
            r,
            sce,
            vec![
                lit(&[0.3, 2.9, 4.0, 0.2, 1.0, 3.0], &[3, 2]),
                lit(&[1.0, 0.0, 1.0], &[3]),
            ],
        );
    });
    case(cs, "op/sce/5x4", move |r| {
        op_case(
            r,
            sce,
            vec![scaled(521, 3.0, &[5, 4]), lit(&labels(522, 5, 4), &[5])],
        )
    });
    case(cs, "op/sce/label_truncation", move |r| {
        op_case(r, sce, vec![tensor(523, &[2, 3]), lit(&[2.9, 0.5], &[2])]);
    });
    case(cs, "op/sce/label_out_of_range", move |r| {
        op_case(r, sce, vec![tensor(524, &[2, 3]), lit(&[0.0, 3.0], &[2])]);
    });
    case(cs, "op/sce/label_nan", move |r| {
        op_case(r, sce, vec![tensor(525, &[2, 3]), lit(&[NAN, 0.0], &[2])])
    });
    case(cs, "op/sce/not2d", move |r| {
        op_case(r, sce, vec![tensor(526, &[6]), lit(&[0.0], &[1])])
    });
    case(cs, "op/sce/not1d", move |r| {
        op_case(
            r,
            sce,
            vec![tensor(527, &[2, 3]), lit(&[0.0, 1.0], &[2, 1])],
        )
    });
    case(cs, "op/sce/size", move |r| {
        op_case(
            r,
            sce,
            vec![tensor(528, &[2, 3]), lit(&[0.0, 1.0, 2.0], &[3])],
        )
    });
    case(cs, "op/bce/no_weights", |r| {
        op_case(
            r,
            |i| bce_with_logits(&i[0], &i[1], None),
            vec![lit(&signs(529, 8), &[8]), scaled(530, 5.0, &[8])],
        );
    });
    case(cs, "op/bce/weights", |r| {
        op_case(
            r,
            |i| bce_with_logits(&i[0], &i[1], Some(&i[2])),
            vec![
                lit(&signs(531, 8), &[8]),
                scaled(532, 5.0, &[8]),
                lit(&positive(533, 8), &[8]),
            ],
        );
    });
    case(cs, "op/bce/extreme", |r| {
        op_case(
            r,
            |i| bce_with_logits(&i[0], &i[1], None),
            vec![
                lit(&[1.0, -1.0, 1.0, -1.0, 1.0], &[5]),
                lit(&[100.0, -100.0, 0.0, 1e-30, NAN], &[5]),
            ],
        );
    });
}

/// The NaN cases: NaNs of distinct payloads, quiet and signaling, meet in
/// each operation.
fn nan_cases(cs: &mut Vec<(String, Case)>) {
    fn nx() -> Tensor {
        let x = [
            bits(0x7fc0_0011),
            bits(0x7fc0_0012),
            1.0,
            bits(0x7f80_0013),
            bits(0xffc0_0014),
            2.0,
        ];
        lit(&x, &[2, 3])
    }
    fn ny() -> Tensor {
        let y = [
            bits(0x7fc0_0021),
            3.0,
            bits(0x7fc0_0022),
            bits(0xffc0_0023),
            bits(0x7f80_0024),
            bits(0x7fc0_0025),
        ];
        lit(&y, &[2, 3])
    }
    fn nz() -> Tensor {
        lit(&[bits(0x7fc0_0031)], &[])
    }
    let binary: [(&str, fn(&[Tensor]) -> Tensor); 8] = [
        ("add", add_all),
        ("sub", |i| sub(&i[0], &i[1])),
        ("mul", |i| mul(&i[0], &i[1])),
        ("div", |i| div(&i[0], &i[1])),
        ("pow", |i| pow(&i[0], &i[1])),
        ("mse", |i| mean_square_error(&i[0], &i[1])),
        ("matmul", |i| mat_mul(&i[0], &i[1], false, true, 0)),
        ("maximum_relu", |i| relu(&add(&i[0], &[&i[1]]))),
    ];
    for (name, f) in binary {
        case(cs, format!("op/nan/{name}"), move |r| {
            op_case(r, f, vec![nx(), ny()])
        });
    }
    let scalar: [(&str, fn(&[Tensor]) -> Tensor); 5] = [
        ("add_scalar", add_all),
        ("sub_scalar", |i| sub(&i[0], &i[1])),
        ("mul_scalar", |i| mul(&i[0], &i[1])),
        ("div_scalar", |i| div(&i[0], &i[1])),
        ("pow_scalar", |i| pow(&i[0], &i[1])),
    ];
    for (name, f) in scalar {
        case(cs, format!("op/nan/{name}"), move |r| {
            op_case(r, f, vec![nx(), nz()])
        });
    }
    let unary: [(&str, fn(&Tensor) -> Tensor); 6] = [
        ("sum", |x| sum(x, &[])),
        ("sum_along0", |x| sum(x, &[0])),
        ("mean", mean),
        ("square", square),
        ("softmax", |x| softmax(x, 1)),
        ("sigmoid", sigmoid),
    ];
    for (name, f) in unary {
        case(cs, format!("op/nan/{name}"), move |r| {
            op_case(r, |i| f(&i[0]), vec![nx()])
        });
    }
}

fn goldstein(x: &Tensor, y: &Tensor) -> Tensor {
    let s = scalar;
    mul(
        &add(
            &s(1.0),
            &[&mul(
                &pow(&add(x, &[y, &s(1.0)]), &s(2.0)),
                &add(
                    &s(19.0),
                    &[
                        &mul(&s(-14.0), x),
                        &mul(&s(3.0), &pow(x, &s(2.0))),
                        &mul(&s(-14.0), y),
                        &mul(&s(6.0), &mul(x, y)),
                        &mul(&s(3.0), &pow(y, &s(2.0))),
                    ],
                ),
            )],
        ),
        &add(
            &s(30.0),
            &[&mul(
                &pow(&sub(&mul(&s(2.0), x), &mul(&s(3.0), y)), &s(2.0)),
                &add(
                    &s(18.0),
                    &[
                        &mul(&s(-32.0), x),
                        &mul(&s(12.0), &pow(x, &s(2.0))),
                        &mul(&s(48.0), y),
                        &mul(&s(-36.0), &mul(x, y)),
                        &mul(&s(27.0), &pow(y, &s(2.0))),
                    ],
                ),
            )],
        ),
    )
}

fn matyas(x: &Tensor, y: &Tensor) -> Tensor {
    sub(
        &mul(&scalar(0.26), &add(&mul(x, x), &[&mul(y, y)])),
        &mul(&scalar(0.48), &mul(x, y)),
    )
}

fn one6() -> Tensor {
    lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3])
}

fn graph_cases(cs: &mut Vec<(String, Case)>) {
    case(cs, "graph/reuse_leaf", |r| {
        let x = one6();
        let y = add(&x, &[&x]);
        y.backward();
        r.t("y", &y);
        r.t_opt("x.grad", x.grad());
    });
    case(cs, "graph/reuse_node", |r| {
        let x = one6();
        let temp = pow(&x, &lit(&[2.0], &[]));
        let y = add(&temp, &[&temp]);
        y.backward();
        r.t("y", &y);
        r.t_opt("x.grad", x.grad());
        r.t_opt("temp.grad", temp.grad());
    });
    case(cs, "graph/dependency", |r| {
        let x = one6();
        let temp = pow(&x, &lit(&[2.0], &[]));
        let y = add(&temp, &[&mul(&lit(&[2.0], &[]), &temp)]);
        y.backward();
        r.t("y", &y);
        r.t_opt("x.grad", x.grad());
    });
    case(cs, "graph/sphere", |r| {
        let (x, y) = (scalar(1.0), scalar(1.0));
        let z = add(&mul(&x, &x), &[&mul(&y, &y)]);
        z.backward();
        r.t("z", &z);
        r.t_opt("x.grad", x.grad());
        r.t_opt("y.grad", y.grad());
    });
    for (name, x0, y0) in [("graph/matyas", 1.0, 1.0)] {
        case(cs, name, move |r| {
            let (x, y) = (scalar(x0), scalar(y0));
            let z = matyas(&x, &y);
            z.backward();
            r.t("z", &z);
            r.t_opt("x.grad", x.grad());
            r.t_opt("y.grad", y.grad());
        });
    }
    for (name, x0, y0) in [
        ("graph/goldstein", 1.0, 1.0),
        ("graph/goldstein_off_minimum", 0.3, -0.7),
    ] {
        case(cs, name, move |r| {
            let (x, y) = (scalar(x0), scalar(y0));
            let z = goldstein(&x, &y);
            z.backward();
            r.t("z", &z);
            r.t_opt("x.grad", x.grad());
            r.t_opt("y.grad", y.grad());
        });
    }
    case(cs, "graph/accumulate", |r| {
        let x = tensor(600, &[2, 3]);
        let y = mul(&x, &x);
        y.backward();
        r.t_opt("after_first", x.grad());
        let z = sin(&x);
        z.backward();
        r.t_opt("after_second", x.grad());
    });
    case(cs, "graph/order", |r| {
        let x = tensor(601, &[2, 3]);
        let a = exp(&x);
        let b = sin(&x);
        let c = mul(&a, &b);
        let d = add(&c, &[&a, &b]);
        let e = sum(&mul(&d, &cos(&x)), &[]);
        let generations: Vec<usize> = [&x, &a, &b, &c, &d, &e]
            .iter()
            .map(|t| t.generation())
            .collect();
        r.add("generations", json!(generations));
        e.backward();
        r.t("e", &e);
        r.t_opt("x.grad", x.grad());
    });
    case(cs, "graph/no_grad", |r| {
        let x = tensor(602, &[2, 3]);
        let y = exp(&x);
        y.no_grad();
        let z = sin(&y);
        z.backward();
        r.t_opt("x.grad", x.grad());
        r.t_opt("y.grad", y.grad());
    });
    case(cs, "graph/backward_leaf", |r| {
        let x = tensor(603, &[2, 3]);
        r.p("backward", || x.backward());
        r.t_opt("x.grad", x.grad());
    });
    case(cs, "graph/backward_twice", |r| {
        let x = tensor(604, &[2, 3]);
        let y = square(&x);
        y.backward();
        y.backward();
        r.t_opt("x.grad", x.grad());
        r.t_opt("y.grad", y.grad());
    });
    case(cs, "graph/inference", |r| {
        let x = tensor(605, &[2, 3]);
        set_inference_mode(true);
        let y = exp(&x);
        let z = sum(&y, &[]);
        set_inference_mode(false);
        r.t("z", &z);
        r.add("generation", json!(z.generation()));
        r.p("backward", || z.backward());
        r.t_opt("z.grad", z.grad());
        r.t_opt("x.grad", x.grad());
    });
    case(cs, "graph/inference_partial", |r| {
        let x = tensor(606, &[2, 3]);
        let y = exp(&x);
        set_inference_mode(true);
        let z = sin(&y);
        set_inference_mode(false);
        let w = sum(&z, &[]);
        w.backward();
        r.t_opt("z.grad", z.grad());
        r.t_opt("y.grad", y.grad());
        r.t_opt("x.grad", x.grad());
    });
    case(cs, "graph/shape_mismatch", |r| {
        let x = tensor(607, &[2, 3]);
        let y = neg(&x);
        let mut buf = Vec::new();
        save(&tensor(608, &[6]), &mut buf).expect("save");
        load(&y, &mut buf.as_slice()).expect("load");
        r.p("backward", || y.backward());
        r.t_opt("x.grad", x.grad());
    });
    case(cs, "graph/generation", |r| {
        let x = tensor(609, &[2, 3]);
        let a = exp(&x);
        let b = add(&a, &[&x]);
        let c = mul(&b, &a);
        let d = add(&scalar(1.0), &[&c]);
        let generations: Vec<usize> = [&x, &a, &b, &c, &d]
            .iter()
            .map(|t| t.generation())
            .collect();
        r.add("generations", json!(generations));
    });
    case(cs, "graph/softmax_alias", |r| {
        let x = scaled(610, 3.0, &[2, 3]);
        let s = softmax(&x, 1);
        let f = flatten(&s);
        let loss = sum(&mul(&f, &tensor(611, &[6])), &[]);
        loss.backward();
        r.t("s.after", &s);
        r.t("f.after", &f);
        r.t_opt("x.grad", x.grad());
    });
    case(cs, "graph/reshape_alias", |r| {
        let x = tensor(612, &[2, 3]);
        let y = reshape(&x, &[3, 2]);
        let z = sum(&square(&y), &[]);
        z.backward();
        r.t_opt("x.grad", x.grad());
        new_sgd(vec![x.clone()], 0.5).step();
        r.t("x.after", &x);
        r.t("y.after", &y);
    });
    case(cs, "graph/embedding_index_node", |r| {
        let x = lit(&[0.1, 0.9, 0.5], &[3]);
        let idx = mul(&x, &scalar(4.0));
        let w = tensor(613, &[6, 2]);
        let y = embedding(&w, &idx);
        y.backward();
        r.t("y", &y);
        r.t_opt("w.grad", w.grad());
        r.t_opt("idx.grad", idx.grad());
        r.t_opt("x.grad", x.grad());
    });
}

fn tensor_cases(cs: &mut Vec<(String, Case)>) {
    case(cs, "tensor/slice", |r| {
        let x = tensor(700, &[3, 4, 5]);
        let y = x.slice(1, 3);
        r.t("y", &y);
        r.add("cap", json!(y.cap()));
        r.add(
            "get",
            json!(encode(&[
                y.get(&[0, 0, 0]),
                y.get(&[1, 3, 4]),
                y.get(&[1, 2, 0])
            ])),
        );
        r.p("scalar", || drop(scalar(1.0).slice(0, 1)));
        r.p("end", || drop(x.slice(0, 4)));
        r.p("reversed", || drop(x.slice(2, 1)));
        r.t("empty", &x.slice(1, 1));
    });
    case(cs, "tensor/slice_write_through", |r| {
        let x = tensor(701, &[3, 2]);
        let y = x.slice(1, 2);
        let z = sum(&y, &[]);
        z.backward();
        new_sgd(vec![y.clone()], 0.5).step();
        r.t("x", &x);
        r.t("y", &y);
    });
    case(cs, "tensor/slice_indices", |r| {
        let x = new_tensor(one6().data(), &[3, 2]);
        r.t("go_test", &x.slice_indices(&[2, 0]));
        let parent = tensor(702, &[3, 2]);
        let v = parent.slice(0, 1);
        r.t("beyond_len", &v.slice_indices(&[2, 0]));
        r.p("beyond_cap", || drop(v.slice_indices(&[3])));
        r.t("none", &x.slice_indices(&[]));
        r.p("scalar", || drop(scalar(1.0).slice_indices(&[0])));
        r.t("1d", &lit(&[7.0, 8.0, 9.0], &[3]).slice_indices(&[2, 2, 0]));
    });
    case(cs, "tensor/get", |r| {
        let x = tensor(703, &[2, 3, 4]);
        r.add(
            "values",
            json!(encode(&[
                x.get(&[0, 0, 0]),
                x.get(&[1, 2, 3]),
                x.get(&[1, 0, 2])
            ])),
        );
        r.p("count", || {
            let _ = x.get(&[0, 0]);
        });
        r.p("range", || {
            let _ = x.get(&[0, 3, 0]);
        });
        r.add("scalar", json!(encode(&[scalar(5.0).get(&[])])));
    });
    case(cs, "tensor/string", |r| {
        let values = [
            0.0,
            bits(0x8000_0000),
            1.0,
            -1.0,
            0.1,
            1e-5,
            1.5e-5,
            0.0001,
            0.000_123_45,
            123_456.0,
            999_999.0,
            1e6,
            1_234_567.0,
            3.402_823_5e38,
            1e-45,
            1.175_494_35e-38,
            16_777_216.0,
            0.3,
            2.5e-7,
            NAN,
            INF,
            -INF,
            100.0,
            1e21,
            1e20,
            33.333_332,
            -2.5e-5,
            // Go's float32(5e-324).
            0.0,
            65504.0,
            1.000_000_1,
        ];
        let mut s: Vec<String> = values.iter().map(|&v| scalar(v).to_string()).collect();
        s.extend([
            lit(&[1.0, 2.0, 3.0], &[3]).to_string(),
            zeros(&[0]).to_string(),
            lit(&gen_values(704, 10, 100.0), &[10]).to_string(),
            lit(&gen_values(705, 11, 100.0), &[11]).to_string(),
            lit(&gen_values(706, 24, 1e7), &[2, 3, 4]).to_string(),
            lit(&[NAN, INF, -INF, bits(0x8000_0000)], &[2, 2]).to_string(),
        ]);
        r.add("strings", json!(s));
    });
    case(cs, "tensor/format", |r| {
        // The string of scalars of random bits, and of integers divided by 1,
        // 2, 4 or 8, where two shortest forms are often equally near.
        let mut g = Seq(712);
        let mut b = String::new();
        for _ in 0..100_000 {
            b.push_str(&scalar(f32::from_bits(g.next() as u32)).to_string());
            b.push('\n');
        }
        for _ in 0..100_000 {
            let n = g.next();
            b.push_str(&scalar((n >> 40) as f32 / (1u32 << (n % 4)) as f32).to_string());
            b.push('\n');
        }
        r.add("digest", json!(hex::encode(Sha256::digest(b.as_bytes()))));
    });
    case(cs, "tensor/linspace", |r| {
        r.t("0_1_5", &lin_space(0.0, 1.0, &[5]));
        r.t("-2_3_2x3", &lin_space(-2.0, 3.0, &[2, 3]));
        r.t("2_2_1", &lin_space(2.0, 2.0, &[1]));
        r.t("scalar", &lin_space(0.0, 1.0, &[]));
        r.t("empty", &lin_space(0.0, 1.0, &[0]));
        r.t("0.1_0.7_7", &lin_space(0.1, 0.7, &[7]));
    });
    case(cs, "tensor/constructors", |r| {
        r.t("ones", &ones(&[2, 3]));
        r.t("zeros", &zeros(&[3]));
        r.t("zeros_scalar", &zeros(&[]));
        r.t("ones_empty", &ones(&[2, 0]));
        r.t("scalar", &scalar(2.5));
        r.add(
            "is_scalar",
            json!([
                zeros(&[]).is_scalar(),
                zeros(&[1]).is_scalar(),
                scalar(1.0).is_scalar()
            ]),
        );
        r.p("new_tensor_mismatch", || {
            drop(new_tensor(vec![1.0, 2.0, 3.0], &[2, 2]))
        });
        r.p("new_tensor_scalar_mismatch", || {
            drop(new_tensor(vec![1.0, 2.0], &[]))
        });
        r.t("new_tensor_scalar", &new_tensor(vec![4.0], &[]));
        r.t("new_tensor_empty", &new_tensor(vec![], &[0, 3]));
    });
    case(cs, "tensor/max", |r| {
        let x = lit(&[3.0, 2.0, 5.0, 6.0, 0.0, 0.0], &[6]);
        r.t("1d", &x.max(0, false));
        r.t("1d_keep", &x.max(0, true));
        r.p("negative", || drop(x.max(-1, false)));
        r.p("past", || drop(x.max(2, false)));
        let x = lit(
            &[
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
            ],
            &[3, 2, 2],
        );
        r.t("axis1", &x.max(1, false));
        r.t("axis1_keep", &x.max(1, true));
        r.t("axis0", &x.max(0, false));
        r.t("axis2", &x.max(2, true));
        let z = bits(0x8000_0000);
        r.t("nan_first_1d", &lit(&[NAN, 1.0, 2.0], &[3]).max(0, false));
        r.t("nan_later_1d", &lit(&[1.0, NAN, 2.0], &[3]).max(0, false));
        r.t("zeros_1d", &lit(&[z, 0.0], &[2]).max(0, false));
        r.t("zeros_1d_rev", &lit(&[0.0, z], &[2]).max(0, false));
        r.t("empty_1d", &lit(&[], &[0]).max(0, false));
        let nans = [1.0, NAN, 3.0, 4.0, bits(0x7fc1_2345), bits(0x7fc0_0001)];
        r.t("nan_2d", &lit(&nans, &[3, 2]).max(0, false));
        r.t(
            "zeros_2d",
            &lit(&[z, 0.0, 0.0, z, z, z], &[2, 3]).max(0, false),
        );
        r.t("random", &scaled(707, 9.0, &[4, 5, 3]).max(1, false));
    });
    case(cs, "tensor/sum", |r| {
        let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[6]);
        r.t("1d", &x.sum(0, false));
        r.p("negative", || drop(x.sum(-1, false)));
        r.p("past", || drop(x.sum(2, false)));
        let x = lit(
            &[
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
            ],
            &[3, 2, 2],
        );
        r.t("axis1", &x.sum(1, false));
        r.t("axis1_keep", &x.sum(1, true));
        r.t("axis0", &x.sum(0, false));
        let z = bits(0x8000_0000);
        r.t("neg_zero_1d", &lit(&[z], &[1]).sum(0, false));
        r.t("neg_zero_2d", &lit(&[z, z, z, z], &[2, 2]).sum(0, false));
        r.t("random", &scaled(708, 9.0, &[4, 5, 3]).sum(1, true));
        r.t("random_1d", &tensor(709, &[100]).sum(0, false));
    });
    case(cs, "tensor/argmax", |r| {
        r.add("ties", json!(lit(&[3.0, 7.0, 7.0, 1.0], &[4]).argmax()));
        r.add(
            "2d",
            json!(lit(&[1.0, 5.0, 2.0, 5.0, 0.0, 9.0], &[2, 3]).argmax()),
        );
        r.add("nan_first", json!(lit(&[NAN, 1.0, 2.0], &[3]).argmax()));
        r.add("nan_later", json!(lit(&[1.0, NAN, 3.0], &[3]).argmax()));
        r.add("empty", json!(zeros(&[0]).argmax()));
        r.add("scalar", json!(scalar(3.0).argmax()));
        r.add("3d", json!(tensor(710, &[2, 3, 4]).argmax()));
    });
    case(cs, "tensor/transpose", |r| {
        r.t(
            "go_test",
            &new_tensor(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[3, 2]).transpose(),
        );
        r.t("3d", &tensor(711, &[2, 3, 4]).transpose());
        r.p("1d", || drop(lit(&[1.0, 2.0], &[2]).transpose()));
        r.p("scalar", || drop(scalar(1.0).transpose()));
    });
    case(cs, "tensor/partition", |r| {
        for (n, p) in [
            (0, 3),
            (-1, 2),
            (5, 0),
            (5, 1),
            (5, -2),
            (3, 5),
            (5, 5),
            (10, 3),
            (7, 2),
            (100, 7),
            (64, 3),
        ] {
            r.add(format!("{n},{p}"), json!(partition(n, p)));
        }
    });
    case(cs, "tensor/partition_aligned", |r| {
        let cases = [
            (0, 3, 32),
            (10, 0, 2),
            (10, -1, 2),
            (10, 3, 2),
            (100, 4, 32),
            (3200, 4, 32),
            (1000, 3, 32),
            (5, 10, 32),
            (10, 3, 0),
            (10, 3, -4),
            (64, 3, 32),
            (50, 4, 32),
            (250, 4, 32),
        ];
        for (n, m, k) in cases {
            r.add(format!("{n},{m},{k}"), json!(partition_aligned(n, m, k)));
        }
    });
    cs.push((
        "tensor/rand".to_owned(),
        Box::new(|r, golden| {
            // Inputs: the draws of rand.New(rand.NewSource(42)) and of
            // rand.New(rand.NewSource(7)).
            let draws = input(golden, "tensor/rand", "draws");
            let normals = input(golden, "tensor/rand", "normals");
            let replay = || Replay {
                floats: parse_floats(draws).into(),
                normals: parse_float64s(normals).into(),
            };
            let x = rand(Some(&mut replay()), &[2, 3]);
            r.add("draws", json!(draws));
            r.t("x", &x);
            let t = zeros(&[3, 4]);
            normal_init(Some(&mut replay()), &t, 0.5, 0.1);
            r.add("normals", json!(normals));
            r.t("normal_init", &t);
            r.t("scalar", &rand(Some(&mut replay()), &[]));
        }),
    ));
    case(cs, "tensor/uniform_normal_shapes", |r| {
        // The values come from the global source, which Go seeds randomly.
        r.add("uniform", json!(uniform(-1.0, 1.0, &[2, 3]).shape()));
        r.add("normal", json!(normal(0.0, 1.0, &[4]).shape()));
        r.add("rand", json!(rand(None, &[3]).shape()));
    });
}

fn set_linear(l: &LinearLayer, seed: u64) {
    set_data(&l.w, seed, 0.5);
    set_data(&l.b, seed + 1, 0.1);
}

fn train_loop(r: &mut Rec, iters: usize, mut step: impl FnMut() -> f32, params: &[Tensor]) {
    let losses: Vec<f32> = (0..iters).map(|_| step()).collect();
    r.add("losses", json!(encode(&losses)));
    for (i, p) in params.iter().enumerate() {
        r.t(format!("param{i}"), p);
    }
}

fn mlp_sgd(r: &mut Rec, jobs: usize, wd: f32) {
    let l1 = new_linear(4, 8);
    let l2 = new_linear(8, 3);
    set_linear(&l1, 800);
    set_linear(&l2, 802);
    let mut model = new_sequential(vec![Box::new(l1), Box::new(new_sigmoid()), Box::new(l2)]);
    model.set_jobs(jobs);
    let x = scaled(804, 2.0, &[16, 4]);
    let y = lit(&labels(805, 16, 3), &[16]);
    let mut opt = new_sgd(model.parameters(), 0.1);
    opt.set_weight_decay(wd);
    opt.set_jobs(jobs);
    train_loop(
        r,
        15,
        || {
            let loss = softmax_cross_entropy(&model.forward(&x), &y);
            opt.zero_grad();
            loss.backward();
            opt.step();
            loss.data()[0]
        },
        &model.parameters(),
    );
}

fn mlp_adam(r: &mut Rec, jobs: usize) {
    let l1 = new_linear(64, 50);
    let l2 = new_linear(50, 5);
    set_linear(&l1, 810);
    set_linear(&l2, 812);
    let mut model = new_sequential(vec![Box::new(l1), Box::new(new_relu()), Box::new(l2)]);
    model.set_jobs(jobs);
    let x = tensor(814, &[20, 64]);
    let y = tensor(815, &[20, 5]);
    let mut opt = new_adam(model.parameters(), 0.01);
    opt.set_weight_decay(0.001);
    opt.set_jobs(jobs);
    train_loop(
        r,
        8,
        || {
            let loss = mean_square_error(&model.forward(&x), &y);
            opt.zero_grad();
            loss.backward();
            opt.step();
            loss.data()[0]
        },
        &model.parameters(),
    );
}

/// Go's `newAttention`. Its `W` is `NewLinear(dim, k)`, which this replaces
/// with one of its own to set its data: a `Box<dyn Layer>` has no
/// `.(*nn.LinearLayer)`.
fn attention_of(dim: usize, k: usize, seed: u64) -> Attention {
    let mut a = new_attention(dim, k);
    let w = new_linear(dim, k);
    set_linear(&w, seed);
    a.w = Box::new(w);
    set_data(&a.h, seed + 2, 0.5);
    a
}

/// model/ctr's AFM with the parts its forward pass uses.
struct Afm {
    b: Tensor,
    w: EmbeddingLayer,
    v: EmbeddingLayer,
    a: Vec<Attention>,
    e: Vec<LinearLayer>,
    n_factors: usize,
}

impl Afm {
    fn new(seed: u64) -> Afm {
        let fm = Afm {
            b: zeros(&[]),
            w: new_embedding(10, &[1]),
            v: new_embedding(10, &[4]),
            a: vec![attention_of(5, 4, seed + 3)],
            e: vec![new_linear(5, 4)],
            n_factors: 4,
        };
        set_data(&fm.w.w, seed, 0.1);
        set_data(&fm.v.w, seed + 1, 0.1);
        set_linear(&fm.e[0], seed + 6);
        fm
    }

    fn set_jobs(&mut self, jobs: usize) {
        self.w.set_jobs(jobs);
        self.v.set_jobs(jobs);
        self.a[0].set_jobs(jobs);
        self.e[0].set_jobs(jobs);
    }

    /// AFM.Forward, line for line.
    fn forward(
        &self,
        indices: &Tensor,
        values: &Tensor,
        embeddings: &[Tensor],
        jobs: usize,
    ) -> Tensor {
        let batch_size = indices.shape()[0];
        let num_dimension = indices.shape()[1];
        let v = self.v.forward(indices);
        let x = reshape(values, &[batch_size, num_dimension, 1]);
        let vx = bmm(&v, &x, true, false, jobs);
        let sum_square = square(&vx);
        let e2 = square(&v);
        let x2 = square(&x);
        let square_sum = bmm(&e2, &x2, true, false, jobs);
        let s = sub(&sum_square, &square_sum);
        let s = sum(&s, &[1]);
        let s = mul(&s, &new_scalar(0.5));
        let w = self.w.forward(indices);
        let linear = bmm(&w, &x, true, false, jobs);
        let mut fm_output = add(
            &reshape(&linear, &[batch_size]),
            &[&reshape(&s, &[batch_size]), &self.b],
        );
        for (i, embedding) in embeddings.iter().enumerate() {
            let encoded_norm = self.e[i].forward(&self.a[i].forward(embedding));
            let encoded_norm = reshape(&encoded_norm, &[batch_size, self.n_factors, 1]);
            fm_output = add(
                &fm_output,
                &[&reshape(
                    &bmm(&vx, &encoded_norm, true, false, jobs),
                    &[batch_size],
                )],
            );
        }
        flatten(&fm_output)
    }

    fn parameters(&self) -> Vec<Tensor> {
        let mut params = vec![self.b.clone()];
        params.extend(self.v.parameters());
        params.extend(self.w.parameters());
        params.extend(self.a[0].parameters());
        params.extend(self.e[0].parameters());
        params
    }
}

/// Pads `b` to a multiple of 4 and reads it as little-endian float32 bits.
fn bytes_as_floats(b: &[u8]) -> Vec<f32> {
    let mut b = b.to_vec();
    while b.len() % 4 != 0 {
        b.push(0);
    }
    b.chunks_exact(4)
        .map(|c| f32::from_bits(u32::from_le_bytes([c[0], c[1], c[2], c[3]])))
        .collect()
}

fn afm_case(r: &mut Rec, adam: bool, jobs: usize) {
    let mut fm = Afm::new(820);
    fm.set_jobs(jobs);
    let indices = lit(&labels(830, 18, 10), &[6, 3]);
    let values = tensor(831, &[6, 3]);
    let embedding = tensor(832, &[6, 5]);
    let target = lit(&signs(833, 6), &[6]);
    let mut opt: Box<dyn Optimizer> = if adam {
        Box::new(new_adam(fm.parameters(), 0.01))
    } else {
        Box::new(new_sgd(fm.parameters(), 0.05))
    };
    opt.set_weight_decay(0.001);
    opt.set_jobs(jobs);
    let embeddings = [embedding];
    train_loop(
        r,
        6,
        || {
            let output = fm.forward(&indices, &values, &embeddings, jobs);
            let loss = bce_with_logits(&target, &output, None);
            opt.zero_grad();
            loss.backward();
            opt.step();
            loss.data()[0]
        },
        &fm.parameters(),
    );
    let mut buf = Vec::new();
    save(&fm.parameters(), &mut buf).expect("save");
    r.add("saved", json!(encode(&bytes_as_floats(&buf))));
    let loaded = Afm::new(840);
    load(&loaded.parameters(), &mut buf.as_slice()).expect("load");
    r.t(
        "prediction",
        &loaded.forward(&indices, &values, &embeddings, jobs),
    );
}

fn neural_network(r: &mut Rec, golden: &BTreeMap<String, Vec<(String, Value)>>) {
    // TestNeuralNetwork, its rand.New(rand.NewSource(0)) replayed.
    let draws = input(golden, "train/neural_network", "draws");
    let normals = input(golden, "train/neural_network", "normals");
    r.add("draws", json!(draws));
    r.add("normals", json!(normals));
    let mut rng = Replay {
        floats: parse_floats(draws).into(),
        normals: parse_float64s(normals).into(),
    };
    let x = rand(Some(&mut rng), &[100, 1]);
    let y = add(
        &rand(Some(&mut rng), &[100, 1]),
        &[&sin(&mul(&x, &new_scalar(2.0 * math32::PI)))],
    );
    let l1 = new_linear(1, 10);
    let l2 = new_linear(10, 1);
    normal_init(Some(&mut rng), &l1.w, 0.0, 0.01);
    normal_init(Some(&mut rng), &l2.w, 0.0, 0.01);
    let model = new_sequential(vec![Box::new(l1), Box::new(new_sigmoid()), Box::new(l2)]);
    let mut optimizer = new_sgd(model.parameters(), 0.2);
    let mut checkpoints = Vec::new();
    for i in 0..10_000 {
        let y_pred = model.forward(&x);
        let loss = mean_square_error(&y, &y_pred);
        optimizer.zero_grad();
        loss.backward();
        optimizer.step();
        if matches!(i, 0 | 1 | 2 | 10 | 100 | 1000 | 5000 | 9999) {
            checkpoints.push(loss.data()[0]);
        }
    }
    r.add("checkpoints", json!(encode(&checkpoints)));
    for (i, p) in model.parameters().iter().enumerate() {
        r.t(format!("param{i}"), p);
    }
}

fn train_cases(cs: &mut Vec<(String, Case)>) {
    case(cs, "train/mlp_sgd/j0", |r| mlp_sgd(r, 0, 0.0));
    case(cs, "train/mlp_sgd/j3_wd", |r| mlp_sgd(r, 3, 0.01));
    case(cs, "train/mlp_adam/j0", |r| mlp_adam(r, 0));
    case(cs, "train/mlp_adam/j4", |r| mlp_adam(r, 4));
    case(cs, "train/attention", |r| {
        let mut a = attention_of(6, 4, 850);
        a.set_jobs(3);
        let x = tensor(855, &[5, 6]);
        let y = tensor(856, &[5, 6]);
        let mut opt = new_adam(a.parameters(), 0.01);
        train_loop(
            r,
            5,
            || {
                let loss = mean_square_error(&a.forward(&x), &y);
                opt.zero_grad();
                loss.backward();
                opt.step();
                loss.data()[0]
            },
            &a.parameters(),
        );
    });
    case(cs, "train/afm_sgd/j0", |r| afm_case(r, false, 0));
    case(cs, "train/afm_adam/j3", |r| afm_case(r, true, 3));
    cs.push(("train/neural_network".to_owned(), Box::new(neural_network)));
    case(cs, "train/sgd_empty_param", |r| {
        r.p("step", || new_sgd(vec![zeros(&[0])], 0.1).step());
        r.p("adam_step", || new_adam(vec![zeros(&[0])], 0.1).step());
    });
    case(cs, "train/adam_shared_param", |r| {
        let p = tensor(861, &[40]);
        let q = sum(&square(&p), &[]);
        q.backward();
        let mut opt = new_adam(vec![p.clone(), p.clone()], 0.1);
        opt.step();
        opt.step();
        r.t("p", &p);
    });
}

fn seq_model(seed: u64) -> Sequential {
    let l1 = new_linear(2, 3);
    let l2 = new_linear(3, 2);
    set_linear(&l1, seed);
    set_linear(&l2, seed + 2);
    new_sequential(vec![Box::new(l1), Box::new(new_sigmoid()), Box::new(l2)])
}

/// One length-delimited `protocol.Tensor`, as `Save` writes them.
fn message_bytes(key: &[&str], shape: &[i32], data: &[f32]) -> Vec<u8> {
    let m = super::codec::Message {
        key: key.iter().map(|&k| k.to_owned()).collect(),
        shape: shape.to_vec(),
        data: data.to_vec(),
    };
    let mut buf = Vec::new();
    super::codec::write_delimited(&mut buf, &m).expect("write");
    buf
}

fn save_case(r: &mut Rec, o: &(impl Reflect + ?Sized)) {
    let mut buf = Vec::new();
    r.add("err", result_value(save(o, &mut buf)));
    r.add("bytes", json!(hex::encode(&buf)));
}

/// Loads the stream into the target and records the error or panic and the
/// target's parameters afterwards.
fn load_case(r: &mut Rec, stream: &[u8], target: &(impl Reflect + ?Sized), params: &[Tensor]) {
    let mut result = Ok(());
    let p = catch(|| result = load(target, &mut &stream[..]));
    r.add("result", if p.is_null() { result_value(result) } else { p });
    for (i, t) in params.iter().enumerate() {
        r.t(format!("param{i}"), t);
    }
}

/// A struct with an exported field `Save` cannot write.
struct UnexportedType {
    t: Tensor,
}

impl Reflect for UnexportedType {
    fn reflect(&self) -> Reflected {
        Reflected::Struct(vec![
            Field::exported("T", self.t.reflect()),
            Field::exported("N", Reflected::Other),
        ])
    }
}

fn save_load_cases(cs: &mut Vec<(String, Case)>) {
    case(cs, "saveload/save/sequential", |r| {
        save_case(r, &seq_model(900))
    });
    case(cs, "saveload/save/params", |r| {
        save_case(r, &seq_model(900).parameters())
    });
    case(cs, "saveload/save/attention", |r| {
        save_case(r, &attention_of(3, 2, 904))
    });
    case(cs, "saveload/save/layers", |r| {
        let e = new_embedding(4, &[2]);
        set_data(&e.w, 907, 1.0);
        let layers: Vec<Box<dyn Layer>> = vec![
            Box::new(e),
            Box::new(new_flatten()),
            Box::new(new_relu()),
            Box::new(new_sigmoid()),
        ];
        save_case(r, &new_sequential(layers));
    });
    case(cs, "saveload/save/tensor", |r| {
        save_case(r, &tensor(908, &[2, 2]))
    });
    case(cs, "saveload/save/scalars", |r| {
        let ts = vec![
            zeros(&[]),
            scalar(-2.5),
            zeros(&[0]),
            lit(&[bits(0x8000_0000), NAN], &[2]),
        ];
        save_case(r, &ts);
    });
    case(cs, "saveload/save/unexpected_type", |r| {
        save_case(
            r,
            &UnexportedType {
                t: tensor(909, &[2]),
            },
        );
    });
    case(cs, "saveload/roundtrip/sequential", |r| {
        let mut buf = Vec::new();
        save(&seq_model(910), &mut buf).expect("save");
        let m = seq_model(920);
        load_case(r, &buf, &m, &m.parameters());
        r.t("forward", &m.forward(&tensor(930, &[4, 2])));
    });
    case(cs, "saveload/roundtrip/params", |r| {
        let mut buf = Vec::new();
        save(&seq_model(911).parameters(), &mut buf).expect("save");
        let m = seq_model(921);
        load_case(r, &buf, &m.parameters(), &m.parameters());
    });
    case(cs, "saveload/roundtrip/attention", |r| {
        let mut buf = Vec::new();
        save(&attention_of(3, 2, 912), &mut buf).expect("save");
        let a = attention_of(3, 2, 922);
        load_case(r, &buf, &a, &a.parameters());
    });
    let cat = |parts: &[Vec<u8>]| parts.concat();
    let seq_streams: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("mid_varint", vec![0x80]),
        ("header_only", vec![0x05]),
        ("partial_body", vec![0x05, 0x0a]),
        ("long_length", vec![0xff, 0x01, 0x0a, 0x01, 0x30]),
        ("bad_varint", vec![0x80, 0x80, 0x80, 0x80, 0x80]),
        ("five_byte_zero", vec![0x80, 0x80, 0x80, 0x80, 0x00]),
        (
            "unknown_field",
            message_bytes(&["Foo"], &[2, 3], &gen_values(940, 6, 1.0)),
        ),
        (
            "unexported",
            message_bytes(&["Layers", "0", "jobs"], &[], &[]),
        ),
        (
            "key_exhausted_struct",
            message_bytes(&["Layers", "0"], &[], &[]),
        ),
        ("key_exhausted_slice", message_bytes(&["Layers"], &[], &[])),
        (
            "key_too_long",
            message_bytes(
                &["Layers", "0", "W", "extra"],
                &[2, 3],
                &gen_values(941, 6, 1.0),
            ),
        ),
        (
            "sigmoid_layer",
            message_bytes(&["Layers", "1", "W"], &[1], &[1.0]),
        ),
        (
            "bad_layer_index",
            message_bytes(&["Layers", "first", "W"], &[], &[]),
        ),
        (
            "multiple",
            cat(&[
                message_bytes(&["Layers", "2", "B"], &[2], &[1.0, 2.0]),
                message_bytes(&["Layers", "2", "B"], &[2], &[3.0, 4.0]),
            ]),
        ),
        (
            "after_error",
            cat(&[
                message_bytes(&["Layers", "x"], &[], &[]),
                message_bytes(&["Layers", "2", "B"], &[2], &[5.0, 6.0]),
            ]),
        ),
        ("invalid_wire", vec![0x02, 0x0a, 0xff]),
        ("invalid_utf8", vec![0x03, 0x0a, 0x01, 0xff]),
        ("nil_key", message_bytes(&[], &[1], &[1.0])),
        (
            "unpacked_data",
            vec![
                0x0d, 0x0a, 0x06, b'L', b'a', b'y', b'e', b'r', b's', 0x1d, 0x00, 0x00, 0x80, 0x3f,
            ],
        ),
    ];
    for (name, stream) in seq_streams {
        case(cs, format!("saveload/load/sequential/{name}"), move |r| {
            let m = seq_model(950);
            load_case(r, &stream, &m, &m.parameters());
        });
    }
    let param_streams: Vec<(&str, Vec<u8>)> = vec![
        ("bad_key", message_bytes(&["x"], &[], &[])),
        ("index_out_of_range", message_bytes(&["9"], &[], &[])),
        ("negative_index", message_bytes(&["-1"], &[], &[])),
        ("plus_index", message_bytes(&["+1"], &[3], &[7.0, 8.0, 9.0])),
        (
            "shape_change",
            message_bytes(&["1"], &[6], &gen_values(942, 6, 1.0)),
        ),
        (
            "inconsistent",
            message_bytes(&["0"], &[2, 2], &[1.0, 2.0, 3.0]),
        ),
        ("no_shape", message_bytes(&["3"], &[], &[1.0, 2.0])),
        ("nil_key", message_bytes(&[], &[1], &[1.0])),
        (
            "unpacked_data",
            vec![
                0x0d, 0x0a, 0x01, b'0', 0x1d, 0x00, 0x00, 0x80, 0x3f, 0x1d, 0x00, 0x00, 0x00, 0x40,
            ],
        ),
        (
            "unpacked_shape",
            vec![
                0x0b, 0x0a, 0x01, b'2', 0x10, 0x01, 0x10, 0x02, 0x1a, 0x04, 0x00, 0x00, 0x40, 0x40,
            ],
        ),
    ];
    for (name, stream) in param_streams {
        case(cs, format!("saveload/load/params/{name}"), move |r| {
            let params = seq_model(960).parameters();
            load_case(r, &stream, &params, &params);
        });
    }
    let tensor_streams: Vec<(&str, Vec<u8>)> = vec![
        ("no_key", message_bytes(&[], &[3], &[1.0, 2.0, 3.0])),
        ("with_key", message_bytes(&["a", "b"], &[1, 2], &[4.0, 5.0])),
        ("empty", message_bytes(&[], &[], &[])),
    ];
    for (name, stream) in tensor_streams {
        case(cs, format!("saveload/load/tensor/{name}"), move |r| {
            let t = tensor(970, &[2, 2]);
            load_case(r, &stream, &t, std::slice::from_ref(&t));
        });
    }
}

fn all_cases() -> Vec<(String, Case)> {
    let mut cs = Vec::new();
    op_cases(&mut cs);
    nan_cases(&mut cs);
    graph_cases(&mut cs);
    tensor_cases(&mut cs);
    train_cases(&mut cs);
    save_load_cases(&mut cs);
    cs
}

type Level = BTreeMap<String, Vec<(String, Value)>>;

fn golden_levels() -> BTreeMap<String, Level> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/nn_golden.json");
    let text = std::fs::read_to_string(path).expect("read nn_golden.json");
    let v: Value = serde_json::from_str(&text).expect("parse nn_golden.json");
    let levels = v["levels"].as_object().expect("levels");
    levels
        .iter()
        .map(|(level, cases)| {
            let cases = cases
                .as_object()
                .expect("cases")
                .iter()
                .map(|(name, entries)| {
                    let entries = entries
                        .as_array()
                        .expect("entries")
                        .iter()
                        .map(|e| (e[0].as_str().expect("name").to_owned(), e[1].clone()))
                        .collect();
                    (name.clone(), entries)
                })
                .collect();
            (level.clone(), cases)
        })
        .collect()
}

/// The level of `Feature`, as the Go tool names it.
fn level_name(f: Feature) -> &'static str {
    if f.contains(Feature::AVX512) {
        "avx512"
    } else if f.contains(Feature::AVX) {
        "avx"
    } else {
        "scalar"
    }
}

const CHILD: &str = "GORSE_NN_GOLDEN_LEVEL";

/// Runs every case at the level of this process and compares it with Go's.
fn check_level(level: &str) {
    assert_eq!(level_name(Feature::current()), level, "the level in effect");
    let golden = golden_levels()
        .remove(level)
        .expect("the level in the golden file");
    // The cases that panic on purpose would print each panic.
    let hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let mut mismatches = Vec::new();
    let mut seen = Vec::new();
    for (name, run) in all_cases() {
        let mut r = Rec::default();
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| run(&mut r, &golden)));
        seen.push(name.clone());
        if let Err(e) = outcome {
            mismatches.push(format!("{name}: the case panicked: {}", message(&*e)));
            continue;
        }
        let Some(want) = golden.get(&name) else {
            mismatches.push(format!("{name}: not in the golden file"));
            continue;
        };
        if r.0.len() != want.len() {
            mismatches.push(format!(
                "{name}: {} entries, Go has {}",
                r.0.len(),
                want.len()
            ));
        }
        for ((got_name, got), (want_name, want)) in r.0.iter().zip(want) {
            if got_name != want_name || got != want {
                mismatches.push(format!(
                    "{name}: {got_name} = {got}, Go: {want_name} = {want}"
                ));
            }
        }
    }
    panic::set_hook(hook);
    for name in golden.keys() {
        if !seen.contains(name) {
            mismatches.push(format!("{name}: a golden case with no Rust case"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches at {level}:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    eprintln!("level {level}: {} cases match Go", seen.len());
}

#[test]
fn nn_golden_matches_go_at_each_level() {
    if let Ok(level) = std::env::var(CHILD) {
        check_level(&level);
        return;
    }
    let exe = std::env::current_exe().expect("test binary");
    let mut checked = 0;
    for (level, godebug) in [
        ("avx512", ""),
        ("avx", "cpu.avx512f=off"),
        ("scalar", "cpu.avx=off"),
    ] {
        let runs = level_name(Feature::detect(Some(godebug)));
        if runs != level {
            eprintln!("skipped level {level}: this CPU runs {runs} with GODEBUG={godebug:?}");
            continue;
        }
        let output = Command::new(&exe)
            .args([
                "--exact",
                "nn::golden_tests::nn_golden_matches_go_at_each_level",
                "--nocapture",
            ])
            .env("GODEBUG", godebug)
            .env(CHILD, level)
            .output()
            .expect("run the test binary");
        let (stdout, stderr) = (
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(
            output.status.success(),
            "level {level}:\n{stdout}\n{stderr}"
        );
        assert!(
            stdout.contains("1 passed"),
            "level {level}: the child ran no test:\n{stdout}"
        );
        eprint!("{stderr}");
        checked += 1;
    }
    assert!(checked > 0, "no level ran");
}
