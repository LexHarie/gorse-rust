// Ports: github.com/chewxy/math32/all_test.go @ add21b5 (v1.11.2 in go.mod)

//! math32's own tests (`all_test.go`) for the functions ported here, with the
//! same cases and assertions.

#![allow(clippy::excessive_precision, clippy::approx_constant)]

use super::*;

const VF: [f32; 10] = [
    4.9790119248836735e+00,
    7.7388724745781045e+00,
    -2.7688005719200159e-01,
    -5.0106036182710749e+00,
    9.6362937071984173e+00,
    2.9263772392439646e+00,
    5.2290834314593066e+00,
    2.7279399104360102e+00,
    1.8253080916808550e+00,
    -8.6859247685756013e+00,
];

const COPYSIGN: [f32; 10] = [
    -4.9790119248836735e+00,
    -7.7388724745781045e+00,
    -2.7688005719200159e-01,
    -5.0106036182710749e+00,
    -9.6362937071984173e+00,
    -2.9263772392439646e+00,
    -5.2290834314593066e+00,
    -2.7279399104360102e+00,
    -1.8253080916808550e+00,
    -8.6859247685756013e+00,
];

const COS_: [f32; 10] = [
    2.634752140995199110787593e-01,
    1.148551260848219865642039e-01,
    9.6191297325640768154550453e-01,
    2.938141150061714816890637e-01,
    -9.777138189897924126294461e-01,
    -9.7693041344303219127199518e-01,
    4.940088096948647263961162e-01,
    -9.1565869021018925545016502e-01,
    -2.517729313893103197176091e-01,
    -7.39241351595676573201918e-01,
];

const EXP: [f32; 10] = [
    1.4533071302642137507696589e+02,
    2.2958822575694449002537581e+03,
    7.5814542574851666582042306e-01,
    6.6668778421791005061482264e-03,
    1.5310493273896033740861206e+04,
    1.8659907517999328638667732e+01,
    1.8662167355098714543942057e+02,
    1.5301332413189378961665788e+01,
    6.2047063430646876349125085e+00,
    1.6894712385826521111610438e-04,
];

const FABS: [f32; 10] = [
    4.9790119248836735e+00,
    7.7388724745781045e+00,
    2.7688005719200159e-01,
    5.0106036182710749e+00,
    9.6362937071984173e+00,
    2.9263772392439646e+00,
    5.2290834314593066e+00,
    2.7279399104360102e+00,
    1.8253080916808550e+00,
    8.6859247685756013e+00,
];

const FLOOR: [f32; 10] = [4.0, 7.0, -1.0, -6.0, 9.0, 2.0, 5.0, 2.0, 1.0, -9.0];

const FREXP: [(f32, i64); 10] = [
    (6.2237649061045918750e-01, 3),
    (9.6735905932226306250e-01, 3),
    (-5.5376011438400318000e-01, -1),
    (-6.2632545228388436250e-01, 3),
    (6.02268356699901081250e-01, 4),
    (7.3159430981099115000e-01, 2),
    (6.5363542893241332500e-01, 3),
    (6.8198497760900255000e-01, 2),
    (9.1265404584042750000e-01, 1),
    (-5.4287029803597508250e-01, 4),
];

const LOG: [f32; 10] = [
    1.605231462693062999102599e+00,
    2.0462560018708770653153909e+00,
    -1.2841708730962657801275038e+00,
    1.6115563905281545116286206e+00,
    2.2655365644872016636317461e+00,
    1.0737652208918379856272735e+00,
    1.6542360106073546632707956e+00,
    1.0035467127723465801264487e+00,
    6.0174879014578057187016475e-01,
    2.161703872847352815363655e+00,
];

const MODF: [(f32, f32); 10] = [
    (4.0000000000000000e+00, 9.7901192488367350108546816e-01),
    (7.0000000000000000e+00, 7.3887247457810456552351752e-01),
    (-0.0, -2.7688005719200159404635997e-01),
    (-5.0000000000000000e+00, -1.060361827107492160848778e-02),
    (9.0000000000000000e+00, 6.3629370719841737980004837e-01),
    (2.0000000000000000e+00, 9.2637723924396464525443662e-01),
    (5.0000000000000000e+00, 2.2908343145930665230025625e-01),
    (2.0000000000000000e+00, 7.2793991043601025126008608e-01),
    (1.0000000000000000e+00, 8.2530809168085506044576505e-01),
    (-8.0000000000000000e+00, -6.8592476857560136238589621e-01),
];

const POW: [f32; 10] = [
    9.5282232631648411840742957e+04,
    5.4811599352999901232411871e+07,
    5.2859121715894396531132279e-01,
    9.7587991957286474464259698e-06,
    4.328064329346044846740467e+09,
    8.4406761805034547437659092e+02,
    1.6946633276191194947742146e+05,
    5.3449040147551939075312879e+02,
    6.688182138451414936380374e+01,
    2.0609869004248742886827439e-09,
];

const SIN_: [f32; 10] = [
    -9.6466616586009283766724726e-01,
    9.9338225271646545763467022e-01,
    -2.7335587039794393342449301e-01,
    9.5586257685042792878173752e-01,
    -2.099421066779969164496634e-01,
    2.135578780799860532750616e-01,
    -8.694568971167362743327708e-01,
    4.019566681155577786649878e-01,
    9.6778633541687993721617774e-01,
    -6.734405869050344734943028e-01,
];

const SQRT: [f32; 10] = [
    2.2313699659365484748756904e+00,
    2.7818829009464263511285458e+00,
    5.2619393496314796848143251e-01,
    2.2384377628763938724244104e+00,
    3.1042380236055381099288487e+00,
    1.7106657298385224403917771e+00,
    2.286718922705479046148059e+00,
    1.6516476350711159636222979e+00,
    1.3510396336454586262419247e+00,
    2.9471892997524949215723329e+00,
];

const TANH: [f32; 10] = [
    9.9990531206936338549262119e-01,
    9.9999962057085294197613294e-01,
    -2.7001505097318677233756845e-01,
    -9.9991110943061718603541401e-01,
    9.9999999146798465745022007e-01,
    9.9427249436125236705001048e-01,
    9.9994257600983138572705076e-01,
    9.9149409509772875982054701e-01,
    9.4936501296239685514466577e-01,
    -9.9999994291374030946055701e-01,
];

const SMALLEST_NONZERO: f32 = 1.401298464324817070923729583289916131280e-45;
const SMALLEST_NORMAL: f32 = 1.1754943508222875079687365e-38;
const LARGEST_SUBNORMAL: f32 = SMALLEST_NORMAL - SMALLEST_NONZERO;

fn tolerance(a: f32, b: f32, e: f32) -> bool {
    let mut d = a - b;
    if d < 0.0 {
        d = -d;
    }
    // b is the expected value: the tolerance is a fraction of b.
    let mut e = e;
    if b != 0.0 {
        e *= b;
        if e < 0.0 {
            e = -e;
        }
    }
    d < e
}

fn close(a: f32, b: f32) -> bool {
    tolerance(a, b, 1e-5)
}

fn veryclose(a: f32, b: f32) -> bool {
    tolerance(a, b, 1e-6)
}

fn thisclose(a: f32, b: f32, e: f32) -> bool {
    tolerance(a, b, e)
}

fn alike(a: f32, b: f32) -> bool {
    if is_nan(a) && is_nan(b) {
        return true;
    }
    if a == b {
        return a.is_sign_negative() == b.is_sign_negative();
    }
    false
}

#[test]
fn test_abs() {
    for i in 0..VF.len() {
        let f = abs(VF[i]);
        assert_eq!(FABS[i], f, "Abs({})", VF[i]);
    }
    let vf_sc = [inf(-1), -0.0, 0.0, inf(1), nan()];
    let sc = [inf(1), 0.0, 0.0, inf(1), nan()];
    for i in 0..vf_sc.len() {
        let f = abs(vf_sc[i]);
        assert!(alike(sc[i], f), "Abs({}) = {f}, want {}", vf_sc[i], sc[i]);
    }
}

#[test]
fn test_copysign() {
    for i in 0..VF.len() {
        let f = copysign(VF[i], -1.0);
        assert_eq!(COPYSIGN[i], f, "Copysign({}, -1)", VF[i]);
    }
    for i in 0..VF.len() {
        let f = copysign(VF[i], 1.0);
        assert_eq!(-COPYSIGN[i], f, "Copysign({}, 1)", VF[i]);
    }
    let vf_sc = [inf(-1), inf(1), nan()];
    let sc = [inf(-1), inf(-1), nan()];
    for i in 0..vf_sc.len() {
        let f = copysign(vf_sc[i], -1.0);
        assert!(
            alike(sc[i], f),
            "Copysign({}, -1) = {f}, want {}",
            vf_sc[i],
            sc[i]
        );
    }
}

#[test]
fn test_cos() {
    for i in 0..VF.len() {
        let f = cos(VF[i]);
        assert!(close(COS_[i], f), "Cos({}) = {f}, want {}", VF[i], COS_[i]);
    }
    let vf_sc = [inf(-1), inf(1), nan()];
    let sc = [nan(), nan(), nan()];
    for i in 0..vf_sc.len() {
        let f = cos(vf_sc[i]);
        assert!(alike(sc[i], f), "Cos({}) = {f}, want {}", vf_sc[i], sc[i]);
    }
}

#[test]
fn test_exp() {
    for i in 0..VF.len() {
        let f = exp(VF[i]);
        assert!(close(EXP[i], f), "Exp({}) = {f}, want {}", VF[i], EXP[i]);
    }
    let vf_sc = [inf(-1), -2000.0, 2000.0, inf(1), nan()];
    let sc = [0.0, 0.0, inf(1), inf(1), nan()];
    for i in 0..vf_sc.len() {
        let f = exp(vf_sc[i]);
        assert!(alike(sc[i], f), "Exp({}) = {f}, want {}", vf_sc[i], sc[i]);
    }
}

#[test]
fn test_floor() {
    for i in 0..VF.len() {
        let f = floor(VF[i]);
        assert_eq!(FLOOR[i], f, "Floor({})", VF[i]);
    }
    let vf_sc = [inf(-1), -0.0, 0.0, inf(1), nan()];
    let sc = [inf(-1), -0.0, 0.0, inf(1), nan()];
    for i in 0..vf_sc.len() {
        let f = floor(vf_sc[i]);
        assert!(alike(sc[i], f), "Floor({}) = {f}, want {}", vf_sc[i], sc[i]);
    }
}

const VF_FREXP_SC: [f32; 5] = [f32::NEG_INFINITY, -0.0, 0.0, f32::INFINITY, f32::NAN];
const FREXP_SC: [(f32, i64); 5] = [
    (f32::NEG_INFINITY, 0),
    (-0.0, 0),
    (0.0, 0),
    (f32::INFINITY, 0),
    (f32::NAN, 0),
];
const VF_FREXP_BC: [f32; 8] = [
    SMALLEST_NORMAL,
    LARGEST_SUBNORMAL,
    SMALLEST_NONZERO,
    MAX_FLOAT32,
    -SMALLEST_NORMAL,
    -LARGEST_SUBNORMAL,
    -SMALLEST_NONZERO,
    -MAX_FLOAT32,
];
const FREXP_BC: [(f32, i64); 8] = [
    (0.5, -125),
    (0.9999999, -126),
    (0.5, -148),
    (0.99999994, 128),
    (-0.5, -125),
    (-0.9999999, -126),
    (-0.5, -148),
    (-0.99999994, 128),
];

#[test]
fn test_frexp() {
    for i in 0..VF.len() {
        let (f, j) = frexp(VF[i]);
        assert!(
            veryclose(FREXP[i].0, f) && FREXP[i].1 == j,
            "Frexp({}) = {f}, {j}, want {:?}",
            VF[i],
            FREXP[i]
        );
    }
    for i in 0..VF_FREXP_SC.len() {
        let (f, j) = frexp(VF_FREXP_SC[i]);
        assert!(
            alike(FREXP_SC[i].0, f) && FREXP_SC[i].1 == j,
            "Frexp({}) = {f}, {j}, want {:?}",
            VF_FREXP_SC[i],
            FREXP_SC[i]
        );
    }
    for i in 0..VF_FREXP_BC.len() {
        let (f, j) = frexp(VF_FREXP_BC[i]);
        assert!(
            alike(FREXP_BC[i].0, f) && FREXP_BC[i].1 == j,
            "Frexp({}) = {f}, {j}, want {:?}",
            VF_FREXP_BC[i],
            FREXP_BC[i]
        );
    }
}

#[test]
fn test_ldexp() {
    for i in 0..VF.len() {
        let f = ldexp(FREXP[i].0, FREXP[i].1);
        assert!(
            veryclose(VF[i], f),
            "Ldexp{:?} = {f}, want {}",
            FREXP[i],
            VF[i]
        );
    }
    for i in 0..VF_FREXP_SC.len() {
        let f = ldexp(FREXP_SC[i].0, FREXP_SC[i].1);
        assert!(
            alike(VF_FREXP_SC[i], f),
            "Ldexp{:?} = {f}, want {}",
            FREXP_SC[i],
            VF_FREXP_SC[i]
        );
    }
    let vf_ldexp_sc: [(f32, i64); 11] = [
        (0.0, 0),
        (0.0, -1075),
        (0.0, 1024),
        (-0.0, 0),
        (-0.0, -1075),
        (-0.0, 1024),
        (inf(1), 0),
        (inf(1), -1024),
        (inf(-1), 0),
        (inf(-1), -1024),
        (nan(), -1024),
    ];
    let ldexp_sc = [
        0.0,
        0.0,
        0.0,
        -0.0,
        -0.0,
        -0.0,
        inf(1),
        inf(1),
        inf(-1),
        inf(-1),
        nan(),
    ];
    for i in 0..vf_ldexp_sc.len() {
        let f = ldexp(vf_ldexp_sc[i].0, vf_ldexp_sc[i].1);
        assert!(
            alike(ldexp_sc[i], f),
            "Ldexp{:?} = {f}, want {}",
            vf_ldexp_sc[i],
            ldexp_sc[i]
        );
    }
    for i in 0..VF_FREXP_BC.len() {
        let f = ldexp(FREXP_BC[i].0, FREXP_BC[i].1);
        assert!(
            alike(VF_FREXP_BC[i], f),
            "Ldexp{:?} = {f}, want {}",
            FREXP_BC[i],
            VF_FREXP_BC[i]
        );
    }
    let vf_ldexp_bc: [(f32, i64); 8] = [
        (SMALLEST_NORMAL, -23),
        (LARGEST_SUBNORMAL, -22),
        (SMALLEST_NONZERO, 256),
        (MAX_FLOAT32, -(127 + 149)),
        (1.0, -150),
        (-1.0, -150),
        (1.0, 128),
        (-1.0, 128),
    ];
    let ldexp_bc = [
        SMALLEST_NONZERO,
        3e-45,
        1.6225928e32,
        3e-45,
        0.0,
        -0.0,
        inf(1),
        inf(-1),
    ];
    for i in 0..vf_ldexp_bc.len() {
        let f = ldexp(vf_ldexp_bc[i].0, vf_ldexp_bc[i].1);
        assert!(
            alike(ldexp_bc[i], f),
            "Ldexp{:?} = {f}, want {}",
            vf_ldexp_bc[i],
            ldexp_bc[i]
        );
    }
}

#[test]
fn test_log() {
    for i in 0..VF.len() {
        let a = abs(VF[i]);
        let f = log(a);
        assert!(close(LOG[i], f), "Log({a}) = {f}, want {}", LOG[i]);
    }
    assert_eq!(log(10.0), LN10, "Log(10)");
    let vf_sc = [inf(-1), -PI, -0.0, 0.0, 1.0, inf(1), nan()];
    let sc = [nan(), nan(), inf(-1), inf(-1), 0.0, inf(1), nan()];
    for i in 0..vf_sc.len() {
        let f = log(vf_sc[i]);
        assert!(alike(sc[i], f), "Log({}) = {f}, want {}", vf_sc[i], sc[i]);
    }
}

#[test]
fn test_modf() {
    for i in 0..VF.len() {
        let (f, g) = modf(VF[i]);
        assert!(
            veryclose(MODF[i].0, f) && thisclose(MODF[i].1, g, 1e-4),
            "Modf({}) = {f}, {g}, want {:?}",
            VF[i],
            MODF[i]
        );
    }
    let vf_sc = [inf(-1), -0.0, inf(1), nan()];
    let sc = [
        (inf(-1), nan()),
        (-0.0, -0.0),
        (inf(1), nan()),
        (nan(), nan()),
    ];
    for i in 0..vf_sc.len() {
        let (f, g) = modf(vf_sc[i]);
        assert!(
            alike(sc[i].0, f) && alike(sc[i].1, g),
            "Modf({}) = {f}, {g}, want {:?}",
            vf_sc[i],
            sc[i]
        );
    }
}

#[test]
fn test_pow() {
    for i in 0..VF.len() {
        let f = pow(10.0, VF[i]);
        assert!(
            close(POW[i], f),
            "Pow(10, {}) = {f}, want {}",
            VF[i],
            POW[i]
        );
    }
    let vf_sc: [(f32, f32); 59] = [
        (inf(-1), -PI),
        (inf(-1), -3.0),
        (inf(-1), -0.0),
        (inf(-1), 0.0),
        (inf(-1), 1.0),
        (inf(-1), 3.0),
        (inf(-1), PI),
        (inf(-1), nan()),
        (-PI, inf(-1)),
        (-PI, -PI),
        (-PI, -0.0),
        (-PI, 0.0),
        (-PI, 1.0),
        (-PI, PI),
        (-PI, inf(1)),
        (-PI, nan()),
        (-1.0, inf(-1)),
        (-1.0, inf(1)),
        (-1.0, nan()),
        (-1.0 / 2.0, inf(-1)),
        (-1.0 / 2.0, inf(1)),
        (-0.0, inf(-1)),
        (-0.0, -PI),
        (-0.0, -3.0),
        (-0.0, 3.0),
        (-0.0, PI),
        (-0.0, inf(1)),
        (0.0, inf(-1)),
        (0.0, -PI),
        (0.0, -3.0),
        (0.0, -0.0),
        (0.0, 0.0),
        (0.0, 3.0),
        (0.0, PI),
        (0.0, inf(1)),
        (0.0, nan()),
        (1.0 / 2.0, inf(-1)),
        (1.0 / 2.0, inf(1)),
        (1.0, inf(-1)),
        (1.0, inf(1)),
        (1.0, nan()),
        (PI, inf(-1)),
        (PI, -0.0),
        (PI, 0.0),
        (PI, 1.0),
        (PI, inf(1)),
        (PI, nan()),
        (inf(1), -PI),
        (inf(1), -0.0),
        (inf(1), 0.0),
        (inf(1), 1.0),
        (inf(1), PI),
        (inf(1), nan()),
        (nan(), -PI),
        (nan(), -0.0),
        (nan(), 0.0),
        (nan(), 1.0),
        (nan(), PI),
        (nan(), nan()),
    ];
    let sc: [f32; 59] = [
        0.0,
        -0.0,
        1.0,
        1.0,
        inf(-1),
        inf(-1),
        inf(1),
        nan(),
        0.0,
        nan(),
        1.0,
        1.0,
        -PI,
        nan(),
        inf(1),
        nan(),
        1.0,
        1.0,
        nan(),
        inf(1),
        0.0,
        inf(1),
        inf(1),
        inf(-1),
        -0.0,
        0.0,
        0.0,
        inf(1),
        inf(1),
        inf(1),
        1.0,
        1.0,
        0.0,
        0.0,
        0.0,
        nan(),
        inf(1),
        0.0,
        1.0,
        1.0,
        1.0,
        0.0,
        1.0,
        1.0,
        PI,
        inf(1),
        nan(),
        0.0,
        1.0,
        1.0,
        inf(1),
        inf(1),
        nan(),
        nan(),
        1.0,
        1.0,
        nan(),
        nan(),
        nan(),
    ];
    for i in 0..vf_sc.len() {
        let f = pow(vf_sc[i].0, vf_sc[i].1);
        assert!(alike(sc[i], f), "Pow{:?} = {f}, want {}", vf_sc[i], sc[i]);
    }
}

#[test]
fn test_sin() {
    for i in 0..VF.len() {
        let f = sin(VF[i]);
        assert!(close(SIN_[i], f), "Sin({}) = {f}, want {}", VF[i], SIN_[i]);
    }
    let vf_sc = [inf(-1), -0.0, 0.0, inf(1), nan()];
    let sc = [nan(), -0.0, 0.0, nan(), nan()];
    for i in 0..vf_sc.len() {
        let f = sin(vf_sc[i]);
        assert!(alike(sc[i], f), "Sin({}) = {f}, want {}", vf_sc[i], sc[i]);
    }
}

#[test]
fn test_sqrt() {
    const TOL: f32 = 1e-7;
    for i in 0..VF.len() {
        let a = abs(VF[i]);
        let f = sqrt_go(a);
        assert!(
            thisclose(f, SQRT[i], TOL),
            "SqrtGo({a}) = {f}, want {}",
            SQRT[i]
        );
        let f = sqrt(a);
        assert!(
            thisclose(f, SQRT[i], TOL),
            "Sqrt({a}) = {f}, want {}",
            SQRT[i]
        );
    }
    let vf_sc = [inf(-1), -PI, -0.0, 0.0, inf(1), nan(), f32::from_bits(2)];
    let sc = [
        nan(),
        nan(),
        -0.0,
        0.0,
        inf(1),
        nan(),
        5.293955920339377e-23,
    ];
    for i in 0..vf_sc.len() {
        let f = sqrt_go(vf_sc[i]);
        assert!(
            alike(sc[i], f),
            "SqrtGo({}) = {f}, want {}",
            vf_sc[i],
            sc[i]
        );
        let f = sqrt(vf_sc[i]);
        assert!(alike(sc[i], f), "Sqrt({}) = {f}, want {}", vf_sc[i], sc[i]);
    }
}

#[test]
fn test_tanh() {
    for i in 0..VF.len() {
        let f = tanh(VF[i]);
        assert!(
            veryclose(TANH[i], f),
            "Tanh({}) = {f}, want {}",
            VF[i],
            TANH[i]
        );
    }
    let vf_sc = [inf(-1), -0.0, 0.0, inf(1), nan()];
    let sc = [-1.0, -0.0, 0.0, 1.0, nan()];
    for i in 0..vf_sc.len() {
        let f = tanh(vf_sc[i]);
        assert!(alike(sc[i], f), "Tanh({}) = {f}, want {}", vf_sc[i], sc[i]);
    }
}

/// The assembly's constants (float64 decimals rounded to float32) are the
/// bits the Go code's float32 constants have, as the comments in log.go and
/// log_amd64.s say.
#[test]
fn log_constants_match_their_hex() {
    let pairs: [(f32, f64, u32); 9] = [
        (LN2_HI, 6.9313812256e-01, 0x3f31_7180),
        (LN2_LO, 9.0580006145e-06, 0x3717_f7d1),
        (L1, 6.6666668653e-01, 0x3f2a_aaab),
        (L2, 4.0000000596e-01, 0x3ecc_cccd),
        (L3, 2.8571429849e-01, 0x3e92_4925),
        (L4, 2.2222198546e-01, 0x3e63_8e29),
        (L5, 1.8183572590e-01, 0x3e3a_3325),
        (L6, 1.5313838422e-01, 0x3e1c_d04f),
        (L7, 1.4798198640e-01, 0x3e17_8897),
    ];
    for (go, asm, bits) in pairs {
        assert_eq!(go.to_bits(), bits);
        assert_eq!((asm as f32).to_bits(), bits);
    }
    assert_eq!(
        (7.07106781186547524401e-01_f64 as f32).to_bits(),
        (SQRT2 / 2.0).to_bits()
    );
}

#[test]
fn go_conversions() {
    assert_eq!(go_f32_to_i32(2.9), 2);
    assert_eq!(go_f32_to_i32(-2.9), -2);
    assert_eq!(go_f32_to_i32(-0.5), 0);
    assert_eq!(go_f32_to_i64(-1.5), -1);
    #[cfg(target_arch = "x86_64")]
    {
        assert_eq!(go_f32_to_i32(f32::NAN), i32::MIN);
        assert_eq!(go_f32_to_i32(3e9), i32::MIN);
        assert_eq!(go_f32_to_i32(-3e9), i32::MIN);
        assert_eq!(go_f32_to_i32(-2_147_483_648.0), i32::MIN);
        assert_eq!(go_f32_to_i32(2_147_483_520.0), 2_147_483_520);
        assert_eq!(go_f32_to_i64(f32::NAN), i64::MIN);
        assert_eq!(go_f32_to_i64(1e19), i64::MIN);
        assert_eq!(go_f32_to_i64(f32::INFINITY), i64::MIN);
        assert_eq!(go_f32_to_i64(-9.223_372e18), -9_223_372_036_854_775_808);
    }
}
