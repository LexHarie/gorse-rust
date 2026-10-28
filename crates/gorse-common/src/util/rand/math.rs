// Ports: math/exp_amd64.s, math/exp_amd64.go, math/log_amd64.s @ go1.27.0
//
// Derived from Go's math package: Copyright 2010 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.
//
// The Log constants are from FreeBSD's /usr/src/lib/msun/src/e_log.c by way
// of Go's math/log.go, which came with this notice:
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunPro, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

//! `math.Exp` and `math.Log` exactly as Go computes them on amd64, where both
//! are assembly routines rather than the portable `exp.go`/`log.go`. The
//! normal and exponential generators of `math/rand` call them, so their last
//! bit decides whether a sample is accepted and therefore the whole stream.
//!
//! Go's `Exp` takes an FMA path when the CPU has AVX and FMA, and a plain
//! path otherwise, and the two round differently (`PORTING_NOTES.md`,
//! WP-1-4 notes). [`exp`] picks the path the same way Go does, `GODEBUG`
//! included.
//!
//! The constants keep the decimal text of Go's assembly, which, like a Rust
//! literal, rounds to the nearest `f64`; a test checks the bits.

#![allow(
    clippy::approx_constant,
    clippy::excessive_precision,
    reason = "constants are copied verbatim from Go's assembly"
)]

/// `log_e(2)` split into an upper part with trailing zero bits and the rest.
const LN2U: f64 = 0.693_147_180_559_662_956_511_601_805_686_950_683_593_75;
const LN2L: f64 = 0.282_352_905_630_315_771_225_884_481_750_134_360_255_254_120_68e-12;
const LOG2E: f64 = 1.442_695_040_888_963_407_359_924_681_001_892_0;
/// Above this `Exp` overflows.
const OVERFLOW: f64 = 7.097_827_128_933_84e+02;
/// Taylor coefficients, in the order of `exprodata` in `exp_amd64.s`.
const E: [f64; 9] = [
    0.5,
    1.0,
    2.0,
    1.666_666_666_666_666_666_7e-1,
    4.166_666_666_666_666_666_7e-2,
    8.333_333_333_333_333_333_3e-3,
    1.388_888_888_888_888_888_9e-3,
    1.984_126_984_126_984_127_0e-4,
    2.480_158_730_158_730_158_7e-5,
];

const POS_INF: u64 = 0x7FF0_0000_0000_0000;
const NEG_INF: u64 = 0xFFF0_0000_0000_0000;
/// Go's `math.NaN()`.
const NAN: u64 = 0x7FF8_0000_0000_0001;

/// Whether Go would take the FMA path of `Exp` on this CPU
/// (`useFMA = cpu.X86.HasAVX && cpu.X86.HasFMA`). The features are read the way
/// Go reads them, so `GODEBUG=cpu.avx=off` and `cpu.fma=off` turn the path off.
fn use_fma() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let features = crate::floats::Feature::current();
        features.contains(crate::floats::Feature::AVX)
            && features.contains(crate::floats::Feature::FMA)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Go's `math.Exp(x)` on amd64, taking the FMA or the plain path as Go does on this CPU.
pub fn exp(x: f64) -> f64 {
    exp_with(x, use_fma())
}

/// `CVTSD2SL`: round to nearest even, and the "integer indefinite" value
/// `i32::MIN` when the result does not fit.
fn cvtsd2sl(x: f64) -> i32 {
    let r = x.round_ties_even();
    if (-2_147_483_648.0..=2_147_483_647.0).contains(&r) {
        r as i32
    } else {
        i32::MIN
    }
}

/// `archExp` with the FMA path chosen by `fma`.
pub(crate) fn exp_with(x: f64, fma: bool) -> f64 {
    let bits = x.to_bits();
    if bits & !(1 << 63) >= POS_INF {
        // NaN or ±Inf: -Inf gives 0, the others themselves.
        return if bits == NEG_INF { 0.0 } else { x };
    }
    if x > OVERFLOW {
        return f64::INFINITY;
    }
    let k = cvtsd2sl(LOG2E * x);
    let kf = f64::from(k);
    let mut x0 = x;
    if fma {
        x0 = (-kf).mul_add(LN2U, x0);
        x0 = (-kf).mul_add(LN2L, x0);
        x0 *= 0.0625;
        let mut x1 = E[8];
        for c in [E[7], E[6], E[5], E[4], E[3], E[0], E[1]] {
            x1 = x1.mul_add(x0, c);
        }
        x0 *= x1;
        for _ in 0..3 {
            x1 = x0 + E[2];
            x0 *= x1;
        }
        x1 = x0 + E[2];
        x0 = x0.mul_add(x1, E[1]);
    } else {
        x0 -= LN2U * kf;
        x0 -= LN2L * kf;
        x0 *= 0.0625;
        let mut x1 = E[8] * x0;
        for c in [E[7], E[6], E[5], E[4], E[3], E[0]] {
            x1 += c;
            x1 *= x0;
        }
        x1 += E[1];
        x0 *= x1;
        for _ in 0..4 {
            x1 = E[2] + x0;
            x0 *= x1;
        }
        x0 += E[1];
    }
    ldexp(x0, k)
}

/// The `ldexp` tail of `archExp`: `x0 * 2**k`, through a denormal step when
/// the biased exponent is not positive.
fn ldexp(mut x0: f64, k: i32) -> f64 {
    // ADDL $0x3FF, BX: k is above i32::MIN + 1023 or exactly i32::MIN.
    let mut biased = k.wrapping_add(0x3FF);
    if biased <= 0 {
        if biased < -52 {
            return 0.0;
        }
        biased += 0x3FE;
        x0 *= f64::from_bits(u64::from(biased.cast_unsigned()) << 52);
        biased = 1;
    } else if biased >= 0x7FF {
        return f64::INFINITY;
    }
    x0 * f64::from_bits(u64::from(biased.cast_unsigned()) << 52)
}

const HSQRT2: f64 = 7.071_067_811_865_475_244_01e-01;
const LN2HI: f64 = 6.931_471_803_691_238_164_90e-01;
const LN2LO: f64 = 1.908_214_929_270_587_700_02e-10;
const L1: f64 = 6.666_666_666_666_735_130e-01;
const L2: f64 = 3.999_999_999_940_941_908e-01;
const L3: f64 = 2.857_142_874_366_239_149e-01;
const L4: f64 = 2.222_219_843_214_978_396e-01;
const L5: f64 = 1.818_357_216_161_805_012e-01;
const L6: f64 = 1.531_383_769_920_937_332e-01;
const L7: f64 = 1.479_819_860_511_658_591e-01;

/// Go's `math.Log(x)` on amd64 (`archLog`).
pub fn log(x: f64) -> f64 {
    let bits = x.to_bits();
    if bits & !(1 << 63) == 0 {
        return f64::NEG_INFINITY;
    }
    if (bits as i64) < 0 {
        return f64::from_bits(NAN);
    }
    if bits >= POS_INF {
        return x;
    }
    // f1, ki := math.Frexp(x) by bit manipulation (subnormals are not
    // normalised, as in the assembly).
    let mut f1 = f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0.5_f64.to_bits());
    let exponent = ((bits >> 52) & 0x7FF) as i32;
    let mut k = f64::from(exponent - 0x3FE);
    // CMPSD $5 (not less than): k -= 1 and f1 *= 2 when f1 <= Sqrt2/2.
    let adjust = if HSQRT2 < f1 { 0.0 } else { 1.0 };
    k -= adjust;
    f1 *= adjust + 1.0;
    let f = f1 - 1.0;
    let s = f / (2.0 + f);
    let s2 = s * s;
    let s4 = s2 * s2;
    let mut t = L7 * s4;
    t += L5;
    t *= s4;
    t += L3;
    t *= s4;
    t += L1;
    let t1 = s2 * t;
    let mut t = L6 * s4;
    t += L4;
    t *= s4;
    t += L2;
    let t2 = s4 * t;
    let r = t1 + t2;
    let hfsq = 0.5 * f * f;
    let mut x3 = s * (r + hfsq);
    x3 += LN2LO * k;
    let x0 = (hfsq - x3) - f;
    k * LN2HI - x0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bits Go's assembler gives the same decimal constants
    /// (`strconv.ParseFloat(s, 64)` in go1.27.0).
    #[test]
    fn util_rand_math_constants_have_go_bits() {
        let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(
            bits(&[LN2U, LN2L, LOG2E, OVERFLOW]),
            [
                0x3FE6_2E42_FEFA_3000,
                0x3D53_DE6A_F278_ECE6,
                0x3FF7_1547_652B_82FE,
                0x4086_2E42_FEFA_39EF
            ]
        );
        assert_eq!(
            bits(&E),
            [
                0x3FE0_0000_0000_0000,
                0x3FF0_0000_0000_0000,
                0x4000_0000_0000_0000,
                0x3FC5_5555_5555_5555,
                0x3FA5_5555_5555_5555,
                0x3F81_1111_1111_1111,
                0x3F56_C16C_16C1_6C17,
                0x3F2A_01A0_1A01_A01A,
                0x3EFA_01A0_1A01_A01A,
            ]
        );
        assert_eq!(
            bits(&[HSQRT2, LN2HI, LN2LO, L1, L2, L3, L4, L5, L6, L7]),
            [
                0x3FE6_A09E_667F_3BCD,
                0x3FE6_2E42_FEE0_0000,
                0x3DEA_39EF_3579_3C76,
                0x3FE5_5555_5555_5593,
                0x3FD9_9999_9997_FA04,
                0x3FD2_4924_9422_9359,
                0x3FCC_71C5_1D8E_78AF,
                0x3FC7_4664_96CB_03DE,
                0x3FC3_9A09_D078_C69F,
                0x3FC2_F112_DF3E_5244,
            ]
        );
    }

    /// The splitmix64 input generator of `contracts/tools/randgolden`.
    struct SplitMix64(u64);

    impl SplitMix64 {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (self.next() >> 11) as f64 * (1.0 / (1u64 << 53) as f64) * (hi - lo)
        }
    }

    fn hex(v: &serde_json::Value) -> u64 {
        u64::from_str_radix(v.as_str().expect("string"), 16).expect("hex")
    }

    /// Both `Exp` paths and `Log` against Go's outputs on each variant: `fma`,
    /// and `nofma` and `avx_off` (`GODEBUG=cpu.fma=off` and `cpu.avx=off`), written
    /// by `contracts/tools/randgolden`.
    #[test]
    fn util_rand_exp_and_log_match_go() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/rand_golden.json");
        let golden: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("parse");
        for (variant, fma) in [("fma", true), ("nofma", false), ("avx_off", false)] {
            let m = &golden["math"][variant];
            for c in m["exp"].as_array().expect("exp") {
                let x = f64::from_bits(hex(&c["in"]));
                assert_eq!(
                    exp_with(x, fma).to_bits(),
                    hex(&c["out"]),
                    "{variant} Exp({x:e})"
                );
            }
            for c in m["log"].as_array().expect("log") {
                let x = f64::from_bits(hex(&c["in"]));
                assert_eq!(log(x).to_bits(), hex(&c["out"]), "{variant} Log({x:e})");
            }
            for d in m["random"].as_array().expect("random") {
                let func = d["func"].as_str().expect("func");
                let n = d["n"].as_u64().expect("n");
                let mut g = SplitMix64(d["seed"].as_u64().expect("seed"));
                let (lo, hi) = (d["lo"].as_f64().expect("lo"), d["hi"].as_f64().expect("hi"));
                let mut digest: u64 = 14_695_981_039_346_656_037;
                for i in 0..n {
                    let out = match func {
                        "exp" => exp_with(g.uniform(lo, hi), fma),
                        "log" => log(g.uniform(lo, hi)),
                        "log_bits" => log(f64::from_bits(g.next() & 0x7fef_ffff_ffff_ffff)),
                        "log_half_sqrt2" => {
                            let mantissa = HSQRT2.to_bits() & 0x000F_FFFF_FFFF_FFFF;
                            let bits = ((1 + i / 5) << 52) | (mantissa + i % 5 - 2);
                            log(f64::from_bits(bits))
                        }
                        _ => panic!("unknown function {func}"),
                    };
                    for b in out.to_bits().to_le_bytes() {
                        digest ^= u64::from(b);
                        digest = digest.wrapping_mul(1_099_511_628_211);
                    }
                }
                assert_eq!(
                    format!("{digest:016x}"),
                    d["digest"].as_str().expect("digest"),
                    "{variant} {func} on [{lo}, {hi}) x{n}"
                );
            }
        }
    }

    #[test]
    fn util_rand_exp_follows_the_cpu_like_go() {
        assert_eq!(exp(-0.3).to_bits(), exp_with(-0.3, use_fma()).to_bits());
    }

    /// The `GODEBUG` settings that change `Exp` on amd64, each with the golden
    /// variant that holds Go's outputs under it.
    const GODEBUG_CHILDREN: [(&str, &str); 2] =
        [("cpu.fma=off", "nofma"), ("cpu.avx=off", "avx_off")];

    /// `Exp` and `Log` follow `GODEBUG` as Go's do: under each setting in
    /// [`GODEBUG_CHILDREN`] the process takes the non-FMA path, and `exp` and
    /// `log` match Go's outputs under that setting. Each setting runs in a child
    /// process of this test binary, since Go reads `GODEBUG` once per process.
    /// The FMA path only exists on x86-64.
    #[test]
    fn exp_and_log_follow_godebug_in_child_processes() {
        if cfg!(target_arch = "x86_64") {
            for (godebug, variant) in GODEBUG_CHILDREN {
                run_godebug_child(godebug, variant);
            }
        }
    }

    /// Runs `godebug_child` in a child process of this test binary, with
    /// `GODEBUG` set to `godebug`, and asserts that it ran and passed.
    fn run_godebug_child(godebug: &str, variant: &str) {
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "util::rand::math::tests::godebug_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("GODEBUG", godebug)
            .env("GORSE_GODEBUG_VARIANT", variant)
            .output()
            .expect("run the child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "GODEBUG={godebug} child did not pass:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The child of [`exp_and_log_follow_godebug_in_child_processes`]. It is
    /// ignored so that a normal run does not count it; the parent runs it with
    /// `--ignored` under the `GODEBUG` setting named by `GORSE_GODEBUG_VARIANT`.
    #[test]
    #[ignore = "run by exp_and_log_follow_godebug_in_child_processes"]
    fn godebug_child() {
        let variant = std::env::var("GORSE_GODEBUG_VARIANT").expect("set by the parent");
        assert!(
            !use_fma(),
            "GODEBUG={:?} must take the non-FMA path",
            std::env::var("GODEBUG")
        );
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/rand_golden.json");
        let golden: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("parse");
        let m = &golden["math"][variant.as_str()];
        for c in m["exp"].as_array().expect("exp") {
            let x = f64::from_bits(hex(&c["in"]));
            assert_eq!(exp(x).to_bits(), hex(&c["out"]), "{variant} Exp({x:e})");
        }
        for c in m["log"].as_array().expect("log") {
            let x = f64::from_bits(hex(&c["in"]));
            assert_eq!(log(x).to_bits(), hex(&c["out"]), "{variant} Log({x:e})");
        }
    }

    #[test]
    fn cvtsd2sl_rounds_like_the_instruction() {
        assert_eq!(cvtsd2sl(0.5), 0);
        assert_eq!(cvtsd2sl(1.5), 2);
        assert_eq!(cvtsd2sl(-2.5), -2);
        assert_eq!(cvtsd2sl(2_147_483_647.4), i32::MAX);
        assert_eq!(cvtsd2sl(2_147_483_647.5), i32::MIN);
        assert_eq!(cvtsd2sl(-2_147_483_648.4), i32::MIN);
        assert_eq!(cvtsd2sl(-1e300), i32::MIN);
    }
}
