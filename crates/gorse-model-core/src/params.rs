// Ports: model/params.go @ add21b5

//! Hyper-parameters: [`Params`], the map every model takes, with Go's typed
//! accessors, and [`ParamsGrid`], the candidate values for grid search.
//!
//! Go keeps the values in a `map[ParamName]any`. [`ParamValue`] holds the same
//! dynamic Go types, because the accessors switch on them: `get_int` accepts
//! only an `int`, so an integer that came back from JSON as a `float64` falls
//! back to its default (`PORTING_NOTES.md`, Q-15). Go's `ParamName` is a string
//! type; here names are `&str`, and the predefined names are the constants below.
//! Go's `Copy` is `Clone`.

use std::collections::BTreeMap;

/// `Lr`: learning rate.
pub const LR: &str = "Lr";
/// `Reg`: regularization strength.
pub const REG: &str = "Reg";
/// `NEpochs`: number of epochs.
pub const N_EPOCHS: &str = "NEpochs";
/// `NFactors`: number of factors.
pub const N_FACTORS: &str = "NFactors";
/// `RandomState`: random state (seed).
pub const RANDOM_STATE: &str = "RandomState";
/// `InitMean`: mean of the gaussian initial parameters.
pub const INIT_MEAN: &str = "InitMean";
/// `InitStdDev`: standard deviation of the gaussian initial parameters.
pub const INIT_STD_DEV: &str = "InitStdDev";
/// `Alpha`: weight for negative samples in ALS.
pub const ALPHA: &str = "Alpha";
/// `Similarity`.
pub const SIMILARITY: &str = "Similarity";
/// `UseFeature`.
pub const USE_FEATURE: &str = "UseFeature";
/// `BatchSize`.
pub const BATCH_SIZE: &str = "BatchSize";
/// `HiddenLayers`.
pub const HIDDEN_LAYERS: &str = "HiddenLayers";
/// `Optimizer`.
pub const OPTIMIZER: &str = "Optimizer";
/// `AutoScale`.
pub const AUTO_SCALE: &str = "AutoScale";

/// The `Optimizer` value for stochastic gradient descent (Go `SGD`).
pub const SGD: &str = "sgd";
/// The `Optimizer` value for Adam (Go `Adam`).
pub const ADAM: &str = "adam";

/// A hyper-parameter value: a Go `any` with the dynamic type it had.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    /// A nil interface.
    Nil,
    /// Go `int`.
    Int(i64),
    /// Go `int32`.
    Int32(i32),
    /// Go `int64`.
    Int64(i64),
    /// Go `float32`.
    Float32(f32),
    /// Go `float64`.
    Float64(f64),
    /// Go `bool`.
    Bool(bool),
    /// Go `string`.
    String(String),
    /// Go `[]int`.
    IntSlice(Vec<i64>),
    /// A value of any other Go type, named as Go prints it (`[]string`).
    /// Gorse never stores one; the accessors treat it as a type mismatch.
    Other(&'static str),
}

impl ParamValue {
    /// The Go name of the dynamic type, as Go's panic messages print it.
    fn go_type_name(&self) -> &'static str {
        match self {
            ParamValue::Nil => "nil",
            ParamValue::Int(_) => "int",
            ParamValue::Int32(_) => "int32",
            ParamValue::Int64(_) => "int64",
            ParamValue::Float32(_) => "float32",
            ParamValue::Float64(_) => "float64",
            ParamValue::Bool(_) => "bool",
            ParamValue::String(_) => "string",
            ParamValue::IntSlice(_) => "[]int",
            ParamValue::Other(name) => name,
        }
    }
}

/// Go's `Params`: hyper-parameters by name. Entries are kept sorted by name.
///
/// ```
/// use gorse_model_core::params::{LR, N_FACTORS, ParamValue, Params};
///
/// let mut params = Params::default();
/// params.0.insert(N_FACTORS.to_owned(), ParamValue::Int(80));
/// params.0.insert(LR.to_owned(), ParamValue::Float64(0.007));
/// assert_eq!(params.get_int(N_FACTORS, 10), 80);
/// assert_eq!(params.get_float32(LR, 0.1), 0.007_f64 as f32);
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Params(pub BTreeMap<String, ParamValue>);

/// Go's type-mismatch log line. Go logs `reflect.TypeFor[ParamName]().Name()`,
/// which is always `ParamName`, and not the type of the value (Q-16).
fn type_mismatch(name: &str) {
    tracing::error!(
        param_name = name,
        actual_type = "ParamName",
        "type mismatch"
    );
}

impl Params {
    /// Go's `GetBool`: the boolean under `name`, or `default` when the name is
    /// missing or holds another type.
    pub fn get_bool(&self, name: &str, default: bool) -> bool {
        match self.0.get(name) {
            Some(ParamValue::Bool(value)) => *value,
            Some(_) => {
                type_mismatch(name);
                default
            }
            None => default,
        }
    }

    /// Go's `GetInt`: the value under `name` when it is an `int`, or `default`.
    /// An `int64`, `int32` or `float64` is a type mismatch here.
    pub fn get_int(&self, name: &str, default: i64) -> i64 {
        match self.0.get(name) {
            Some(ParamValue::Int(value)) => *value,
            Some(_) => {
                type_mismatch(name);
                default
            }
            None => default,
        }
    }

    /// Go's `GetInt64`: the value under `name` as an `int64`. An `int` is
    /// converted; any other type gives `default`.
    pub fn get_int64(&self, name: &str, default: i64) -> i64 {
        match self.0.get(name) {
            Some(ParamValue::Int64(value)) => *value,
            Some(ParamValue::Int(value)) => *value,
            Some(_) => {
                type_mismatch(name);
                default
            }
            None => default,
        }
    }

    /// Go's `GetFloat32`: the value under `name` as a `float32`. A `float64`
    /// or an `int` is converted; any other type gives `default`.
    pub fn get_float32(&self, name: &str, default: f32) -> f32 {
        match self.0.get(name) {
            Some(ParamValue::Float32(value)) => *value,
            Some(ParamValue::Float64(value)) => *value as f32,
            Some(ParamValue::Int(value)) => *value as f32,
            Some(_) => {
                type_mismatch(name);
                default
            }
            None => default,
        }
    }

    /// Go's `GetString`: the string under `name`, or `default` when the name is
    /// missing.
    ///
    /// # Panics
    ///
    /// When the name holds another type. Go asserts the type with `val.(string)`
    /// and panics, and this panics with Go's message (`PORTING_NOTES.md`).
    pub fn get_string(&self, name: &str, default: &str) -> String {
        match self.0.get(name) {
            Some(ParamValue::String(value)) => value.clone(),
            Some(other) => panic!(
                "interface conversion: interface {{}} is {}, not string",
                other.go_type_name()
            ),
            None => default.to_owned(),
        }
    }

    /// Go's `GetIntSlice`: the `[]int` under `name`, or `default` when the name
    /// is missing or holds another type.
    pub fn get_int_slice(&self, name: &str, default: &[i64]) -> Vec<i64> {
        match self.0.get(name) {
            Some(ParamValue::IntSlice(value)) => value.clone(),
            Some(_) => {
                type_mismatch(name);
                default.to_vec()
            }
            None => default.to_vec(),
        }
    }

    /// Go's `Overwrite`: a new map with every entry of `self`, and each entry of
    /// `params` replacing the one with the same name.
    pub fn overwrite(&self, params: &Params) -> Params {
        let mut merged = self.clone();
        merged.0.extend(params.0.clone());
        merged
    }
}

/// Go's `ParamsGrid`: the candidate values of each parameter for grid search.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamsGrid(pub BTreeMap<String, Vec<ParamValue>>);

impl ParamsGrid {
    /// Go's `Len`: the number of parameters in the grid.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the grid has no parameters.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Go's `NumCombinations`: the product of the number of candidates of each
    /// parameter. Go's `int` product wraps on overflow, and so does this.
    pub fn num_combinations(&self) -> usize {
        self.0
            .values()
            .fold(1_usize, |count, values| count.wrapping_mul(values.len()))
    }

    /// Go's `Fill`: adds each parameter of `default` that the grid does not have.
    pub fn fill(&mut self, default: &ParamsGrid) {
        for (param, values) in &default.0 {
            self.0
                .entry(param.clone())
                .or_insert_with(|| values.clone());
        }
    }
}
