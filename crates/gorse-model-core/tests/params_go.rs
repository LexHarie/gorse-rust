// Ports: model/params_test.go @ add21b5

//! Go's `params_test.go`, case for case. The Go literals' default types are
//! kept: an untyped integer is an `int`, so it is `ParamValue::Int`, and a
//! float literal is a `float64`.

use gorse_model_core::params::{
    ADAM, HIDDEN_LAYERS, LR, N_EPOCHS, N_FACTORS, ParamValue, Params, ParamsGrid, RANDOM_STATE,
    SIMILARITY, USE_FEATURE,
};

use ParamValue::{Bool, Float32, Float64, Int, Int64, IntSlice, Json, Other, String as Str};
use gorse_common::gojson::Value;

/// Builds a `Params` from `(name, value)` pairs, as a Go composite literal does.
fn params(entries: &[(&str, ParamValue)]) -> Params {
    let mut params = Params::default();
    for (name, value) in entries {
        params.0.insert((*name).to_owned(), value.clone());
    }
    params
}

#[test]
fn params_copy() {
    // Create parameters
    let a = params(&[
        (N_FACTORS, Int(1)),
        (LR, Float64(0.1)),
        (RANDOM_STATE, Int(0)),
    ]);
    // Create copy
    let mut b = a.clone();
    b.0.insert(N_FACTORS.to_owned(), Int(2));
    b.0.insert(LR.to_owned(), Float64(0.2));
    b.0.insert(RANDOM_STATE.to_owned(), Int(1));
    // Check original parameters
    assert_eq!(a.get_int(N_FACTORS, -1), 1);
    assert_eq!(a.get_float32(LR, -0.1), 0.1_f32);
    assert_eq!(a.get_int64(RANDOM_STATE, -1), 0);
    // Check copy parameters
    assert_eq!(b.get_int(N_FACTORS, -1), 2);
    assert_eq!(b.get_float32(LR, -0.1), 0.2_f32);
    assert_eq!(b.get_int64(RANDOM_STATE, -1), 1);
}

#[test]
fn params_get_float32() {
    let mut p = Params::default();
    // Empty case
    assert_eq!(p.get_float32(LR, 0.1), 0.1_f32);
    // Normal case
    p.0.insert(LR.to_owned(), Float32(1.0));
    assert_eq!(p.get_float32(LR, 0.1), 1.0_f32);
    // Convertible case
    p.0.insert(LR.to_owned(), Float64(2.0));
    assert_eq!(p.get_float32(LR, 0.1), 2.0_f32);
    p.0.insert(LR.to_owned(), Int(3));
    assert_eq!(p.get_float32(LR, 0.1), 3.0_f32);
    // Wrong type case
    p.0.insert(LR.to_owned(), Int(1));
    assert_eq!(p.get_float32(LR, 0.1), 1.0_f32);
    p.0.insert(LR.to_owned(), Str("hello".to_owned()));
    assert_eq!(p.get_float32(LR, 0.1), 0.1_f32);
}

#[test]
fn params_get_bool() {
    let mut p = Params::default();
    // Empty case
    assert!(p.get_bool(USE_FEATURE, true));
    // Normal case
    p.0.insert(USE_FEATURE.to_owned(), Bool(false));
    assert!(!p.get_bool(USE_FEATURE, true));
    // Wrong type case
    p.0.insert(USE_FEATURE.to_owned(), Int(1));
    assert!(p.get_bool(USE_FEATURE, true));
}

#[test]
fn params_get_int() {
    let mut p = Params::default();
    // Empty case
    assert_eq!(p.get_int(N_FACTORS, -1), -1);
    // Normal case
    p.0.insert(N_FACTORS.to_owned(), Int(0));
    assert_eq!(p.get_int(N_FACTORS, -1), 0);
    // Wrong type case
    p.0.insert(N_FACTORS.to_owned(), Str("hello".to_owned()));
    assert_eq!(p.get_int(N_FACTORS, -1), -1);
}

#[test]
fn params_get_int64() {
    let mut p = Params::default();
    // Empty case
    assert_eq!(p.get_int64(RANDOM_STATE, -1), -1);
    // Normal case
    p.0.insert(RANDOM_STATE.to_owned(), Int64(0));
    assert_eq!(p.get_int64(RANDOM_STATE, -1), 0);
    // Wrong type case
    p.0.insert(RANDOM_STATE.to_owned(), Int(0));
    assert_eq!(p.get_int64(RANDOM_STATE, -1), 0);
    p.0.insert(RANDOM_STATE.to_owned(), Str("hello".to_owned()));
    assert_eq!(p.get_int64(RANDOM_STATE, -1), -1);
}

#[test]
fn params_get_string() {
    let mut p = Params::default();
    // Empty case
    assert_eq!(p.get_string(SIMILARITY, "xyz"), "xyz");
    // Normal case
    p.0.insert(SIMILARITY.to_owned(), Str("abc".to_owned()));
    assert_eq!(p.get_string(SIMILARITY, "abc"), "abc");
}

#[test]
fn params_get_int_slice() {
    let mut p = Params::default();
    // Empty case
    assert_eq!(p.get_int_slice(HIDDEN_LAYERS, &[1, 2, 3]), vec![1, 2, 3]);
    // Normal case
    p.0.insert(HIDDEN_LAYERS.to_owned(), IntSlice(vec![4, 5, 6]));
    assert_eq!(p.get_int_slice(HIDDEN_LAYERS, &[1, 2, 3]), vec![4, 5, 6]);
    // Wrong type case: a `[]string` in Go.
    p.0.insert(HIDDEN_LAYERS.to_owned(), Other("[]string"));
    assert_eq!(p.get_int_slice(HIDDEN_LAYERS, &[1, 2, 3]), vec![1, 2, 3]);
    // A `[]any` decoded from JSON is a wrong type too, as Go's `GetIntSlice`
    // treats it.
    p.0.insert(
        HIDDEN_LAYERS.to_owned(),
        Json(Value::Array(vec![Value::Number(64.0), Value::Number(32.0)])),
    );
    assert_eq!(p.get_int_slice(HIDDEN_LAYERS, &[1, 2, 3]), vec![1, 2, 3]);
}

#[test]
fn params_overwrite() {
    let a = params(&[(N_FACTORS, Int(10)), (LR, Float64(0.5))]);
    let b = params(&[(N_EPOCHS, Int(100)), (N_FACTORS, Int(20))]);
    let c = a.overwrite(&b);
    assert_eq!(c.0.get(N_FACTORS), Some(&Int(20)));
    assert_eq!(c.0.get(LR), Some(&Float64(0.5)));
    assert_eq!(c.0.get(N_EPOCHS), Some(&Int(100)));
}

#[test]
fn params_grid() {
    let mut grid = ParamsGrid::default();
    grid.0.insert("a".to_owned(), vec![Int(0), Int(1)]);
    let mut default_grid = ParamsGrid::default();
    default_grid.0.insert("a".to_owned(), vec![Int(2), Int(3)]);
    default_grid.0.insert("b".to_owned(), vec![Int(4), Int(5)]);
    assert_eq!(grid.len(), 1);
    grid.fill(&default_grid);
    assert_eq!(grid.0["a"], vec![Int(0), Int(1)]);
    assert_eq!(grid.0["b"], vec![Int(4), Int(5)]);
    assert_eq!(grid.num_combinations(), 4);
}

// The tests below are not in Go's file. They check quirks of the Go code that
// the Go tests do not cover (`PORTING_NOTES.md`).

#[test]
fn optimizer_constants_are_the_go_strings() {
    // `model.Adam` is an untyped string constant, so it is a plain string here.
    assert_eq!(ADAM, "adam");
    let p = params(&[("Optimizer", Str(ADAM.to_owned()))]);
    assert_eq!(p.get_string("Optimizer", "sgd"), "adam");
}

#[test]
#[should_panic(expected = "interface conversion: interface {} is float64, not string")]
fn get_string_panics_on_another_type_as_go_does() {
    // Go's `val.(string)` panics; it does not log and return the default.
    let p = params(&[(SIMILARITY, Float64(0.5))]);
    let _ = p.get_string(SIMILARITY, "xyz");
}

#[test]
fn get_int_does_not_accept_an_int64_or_a_float_from_json() {
    // Q-15: `GetInt` takes only `int`. A reloaded model's integers are float64.
    let p = params(&[(N_EPOCHS, Float64(3.0)), (N_FACTORS, Int64(4))]);
    assert_eq!(p.get_int(N_EPOCHS, 100), 100);
    assert_eq!(p.get_int(N_FACTORS, 10), 10);
    assert_eq!(p.get_float32(N_EPOCHS, 0.5), 3.0_f32);
}
