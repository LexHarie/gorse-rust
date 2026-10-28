// Ports: $GOROOT/src/math/erf.go (Erf, erf), math.go (Max, Min) @ go1.27.0
//
// Derived from Go's math package: Copyright 2009 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.
//
// The erf coefficients are from FreeBSD's /usr/src/lib/msun/src/s_erf.c by way
// of Go's math/erf.go, which came with this notice:
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunPro, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

//! The parts of Go's `math` package that the TPE sampler calls and that Rust's
//! standard library does not have the same way: `math.Erf`, and `math.Max` and
//! `math.Min`, which return NaN for a NaN argument where `f64::max` and
//! `f64::min` return the other one. `math.Exp` is in
//! [`gorse_common::util::rand::math`].

// The coefficients keep Go's digits, which are more digits than an f64 holds.
// Go and Rust both round each literal to the nearest f64 once, so the values
// are Go's.
#![allow(clippy::excessive_precision)]

use gorse_common::util::rand::math::exp;

const VERY_TINY: f64 = 2.848_094_538_889_218e-306;
const SMALL: f64 = 1.0 / 268_435_456.0; // 2**-28
// `1/0.35` in Go is the exact constant 20/7, rounded once.
const ERF_LARGE_LIMIT: f64 = 20.0 / 7.0;

// The coefficients, copied from erf.go in Go's order.
const ERX: f64 = 8.45062911510467529297e-01;
const EFX: f64 = 1.28379167095512586316e-01;
const EFX8: f64 = 1.02703333676410069053e+00;
const PP0: f64 = 1.28379167095512558561e-01;
const PP1: f64 = -3.25042107247001499370e-01;
const PP2: f64 = -2.84817495755985104766e-02;
const PP3: f64 = -5.77027029648944159157e-03;
const PP4: f64 = -2.37630166566501626084e-05;
const QQ1: f64 = 3.97917223959155352819e-01;
const QQ2: f64 = 6.50222499887672944485e-02;
const QQ3: f64 = 5.08130628187576562776e-03;
const QQ4: f64 = 1.32494738004321644526e-04;
const QQ5: f64 = -3.96022827877536812320e-06;
const PA0: f64 = -2.36211856075265944077e-03;
const PA1: f64 = 4.14856118683748331666e-01;
const PA2: f64 = -3.72207876035701323847e-01;
const PA3: f64 = 3.18346619901161753674e-01;
const PA4: f64 = -1.10894694282396677476e-01;
const PA5: f64 = 3.54783043256182359371e-02;
const PA6: f64 = -2.16637559486879084300e-03;
const QA1: f64 = 1.06420880400844228286e-01;
const QA2: f64 = 5.40397917702171048937e-01;
const QA3: f64 = 7.18286544141962662868e-02;
const QA4: f64 = 1.26171219808761642112e-01;
const QA5: f64 = 1.36370839120290507362e-02;
const QA6: f64 = 1.19844998467991074170e-02;
const RA0: f64 = -9.86494403484714822705e-03;
const RA1: f64 = -6.93858572707181764372e-01;
const RA2: f64 = -1.05586262253232909814e+01;
const RA3: f64 = -6.23753324503260060396e+01;
const RA4: f64 = -1.62396669462573470355e+02;
const RA5: f64 = -1.84605092906711035994e+02;
const RA6: f64 = -8.12874355063065934246e+01;
const RA7: f64 = -9.81432934416914548592e+00;
const SA1: f64 = 1.96512716674392571292e+01;
const SA2: f64 = 1.37657754143519042600e+02;
const SA3: f64 = 4.34565877475229228821e+02;
const SA4: f64 = 6.45387271733267880336e+02;
const SA5: f64 = 4.29008140027567833386e+02;
const SA6: f64 = 1.08635005541779435134e+02;
const SA7: f64 = 6.57024977031928170135e+00;
const SA8: f64 = -6.04244152148580987438e-02;
const RB0: f64 = -9.86494292470009928597e-03;
const RB1: f64 = -7.99283237680523006574e-01;
const RB2: f64 = -1.77579549177547519889e+01;
const RB3: f64 = -1.60636384855821916062e+02;
const RB4: f64 = -6.37566443368389627722e+02;
const RB5: f64 = -1.02509513161107724954e+03;
const RB6: f64 = -4.83519191608651397019e+02;
const SB1: f64 = 3.03380607434824582924e+01;
const SB2: f64 = 3.25792512996573918826e+02;
const SB3: f64 = 1.53672958608443695994e+03;
const SB4: f64 = 3.19985821950859553908e+03;
const SB5: f64 = 2.55305040643316442583e+03;
const SB6: f64 = 4.74528541206955367215e+02;
const SB7: f64 = -2.24409524465858183362e+01;

/// Go's `math.Erf`, the pure Go code. amd64 has no assembly `Erf`, so this is
/// what Go runs there.
pub(super) fn erf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x == f64::INFINITY {
        return 1.0;
    }
    if x == f64::NEG_INFINITY {
        return -1.0;
    }
    let (sign, x) = if x < 0.0 { (true, -x) } else { (false, x) };
    if x < 0.84375 {
        let temp = if x < SMALL {
            if x < VERY_TINY {
                0.125 * (8.0 * x + EFX8 * x)
            } else {
                x + EFX * x
            }
        } else {
            let z = x * x;
            let r = PP0 + z * (PP1 + z * (PP2 + z * (PP3 + z * PP4)));
            let s = 1.0 + z * (QQ1 + z * (QQ2 + z * (QQ3 + z * (QQ4 + z * QQ5))));
            let y = r / s;
            x + x * y
        };
        return if sign { -temp } else { temp };
    }
    if x < 1.25 {
        let s = x - 1.0;
        let p = PA0 + s * (PA1 + s * (PA2 + s * (PA3 + s * (PA4 + s * (PA5 + s * PA6)))));
        let q = 1.0 + s * (QA1 + s * (QA2 + s * (QA3 + s * (QA4 + s * (QA5 + s * QA6)))));
        return if sign { -ERX - p / q } else { ERX + p / q };
    }
    if x >= 6.0 {
        return if sign { -1.0 } else { 1.0 };
    }
    let s = 1.0 / (x * x);
    let (r_num, s_den) = if x < ERF_LARGE_LIMIT {
        (
            RA0 + s * (RA1 + s * (RA2 + s * (RA3 + s * (RA4 + s * (RA5 + s * (RA6 + s * RA7)))))),
            1.0 + s
                * (SA1
                    + s * (SA2
                        + s * (SA3 + s * (SA4 + s * (SA5 + s * (SA6 + s * (SA7 + s * SA8))))))),
        )
    } else {
        (
            RB0 + s * (RB1 + s * (RB2 + s * (RB3 + s * (RB4 + s * (RB5 + s * RB6))))),
            1.0 + s * (SB1 + s * (SB2 + s * (SB3 + s * (SB4 + s * (SB5 + s * (SB6 + s * SB7)))))),
        )
    };
    // Pseudo-single (20-bit) precision x.
    let z = f64::from_bits(x.to_bits() & 0xffff_ffff_0000_0000);
    let r = exp(-z * z - 0.5625) * exp((z - x) * (z + x) + r_num / s_den);
    if sign { r / x - 1.0 } else { 1.0 - r / x }
}

/// Go's `math.Max`: +Inf wins, then NaN, and +0 wins over -0.
pub(super) fn go_max(x: f64, y: f64) -> f64 {
    if x == f64::INFINITY || y == f64::INFINITY {
        return f64::INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == 0.0 && x == y {
        return if x.is_sign_negative() { y } else { x };
    }
    if x > y { x } else { y }
}

/// Go's `math.Min`: -Inf wins, then NaN, and -0 wins over +0.
pub(super) fn go_min(x: f64, y: f64) -> f64 {
    if x == f64::NEG_INFINITY || y == f64::NEG_INFINITY {
        return f64::NEG_INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == 0.0 && x == y {
        return if x.is_sign_negative() { x } else { y };
    }
    if x < y { x } else { y }
}

#[cfg(test)]
mod tests {
    // The values here were checked against Go's math.Erf, math.Max and
    // math.Min on go1.27.0. The golden test in tpe::golden repeats the check
    // over the whole grid and the special values, from contracts/tools/tpegolden.

    use super::*;

    #[test]
    fn erf_matches_go_at_the_table_points() {
        // Go's math.Erf at these points, bit for bit.
        let cases: [(f64, f64); 6] = [
            (0.0, 0.0),
            (0.5, 0.5204998778130465),
            (1.0, 0.8427007929497149),
            (-1.0, -0.8427007929497149),
            (2.0, 0.9953222650189527),
            (6.0, 1.0),
        ];
        for (x, want) in cases {
            assert_eq!(erf(x).to_bits(), want.to_bits(), "erf({x})");
        }
    }

    #[test]
    fn erf_special_values() {
        assert!(erf(f64::NAN).is_nan());
        assert_eq!(erf(f64::INFINITY), 1.0);
        assert_eq!(erf(f64::NEG_INFINITY), -1.0);
        assert_eq!(erf(0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(erf(-0.0).to_bits(), (-0.0f64).to_bits());
    }

    #[test]
    fn max_and_min_follow_go_for_nan_and_zero() {
        assert!(go_max(f64::NAN, 1.0).is_nan());
        assert!(go_min(1.0, f64::NAN).is_nan());
        assert_eq!(go_max(-0.0, 0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(go_min(0.0, -0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(go_max(f64::INFINITY, f64::NAN), f64::INFINITY);
        assert_eq!(go_min(f64::NEG_INFINITY, f64::NAN), f64::NEG_INFINITY);
        assert_eq!(go_max(2.0, 3.0), 3.0);
        assert_eq!(go_min(2.0, 3.0), 2.0);
    }
}
