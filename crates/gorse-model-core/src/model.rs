// Ports: model/model.go @ add21b5

//! The [`Model`] interface every recommendation model implements, and
//! [`BaseModel`], the part of a model that holds the hyper-parameters and the
//! random generator. Go models embed `BaseModel`; Rust models hold it as a
//! field and delegate to it.

use gorse_common::util::RandomGenerator;

use crate::params::{Params, RANDOM_STATE};

/// Go's `Model` interface.
///
/// Go's interface also has `SuggestParams(trial goptuna.Trial) Params`. It is
/// added with the tuning port (WP-3-3), which brings the `Trial` type it needs
/// (`PORTING_NOTES.md`).
pub trait Model {
    /// Go's `SetParams`.
    fn set_params(&mut self, params: Params);

    /// Go's `GetParams`.
    fn get_params(&self) -> &Params;

    /// Go's `Clear`: drops the fitted state, so [`Model::invalid`] reports true.
    fn clear(&mut self);

    /// Go's `Invalid`: whether the model has no usable fitted state.
    fn invalid(&self) -> bool;
}

/// Go's `BaseModel`: the hyper-parameters and the random generator, which
/// [`BaseModel::set_params`] seeds from `RandomState`.
#[derive(Default)]
pub struct BaseModel {
    /// The hyper-parameters.
    pub params: Params,
    /// The random generator. It is `None` until [`BaseModel::set_params`] runs.
    rng: Option<RandomGenerator>,
}

impl BaseModel {
    /// Go's `SetParams`: stores the parameters and seeds a new random generator
    /// with `RandomState` (0 when the name is missing or not an `int64`).
    pub fn set_params(&mut self, params: Params) {
        let seed = params.get_int64(RANDOM_STATE, 0);
        self.params = params;
        self.rng = Some(RandomGenerator::new(seed));
    }

    /// Go's `GetParams`.
    pub fn get_params(&self) -> &Params {
        &self.params
    }

    /// Go's `GetRandomGenerator`. Go returns a handle that shares the
    /// generator's state, so every draw advances the model's one generator,
    /// and so does a draw through this `&mut`.
    ///
    /// # Panics
    ///
    /// When [`BaseModel::set_params`] has not run. Go's generator is then nil,
    /// and it panics on first use (`PORTING_NOTES.md`).
    pub fn get_random_generator(&mut self) -> &mut RandomGenerator {
        self.rng
            .as_mut()
            .expect("random generator used before set_params")
    }
}

#[cfg(test)]
mod tests {
    // Go has no test for model.go. These check what `SetParams` does with
    // `RandomState`, which every model relies on.

    use super::*;
    use crate::params::ParamValue;

    fn params(entries: &[(&str, ParamValue)]) -> Params {
        let mut params = Params::default();
        for (name, value) in entries {
            params.0.insert((*name).to_owned(), value.clone());
        }
        params
    }

    #[test]
    fn set_params_seeds_the_generator_from_random_state() {
        let mut model = BaseModel::default();
        model.set_params(params(&[(RANDOM_STATE, ParamValue::Int(7))]));
        let first = model.get_random_generator().float32();
        assert_eq!(first, RandomGenerator::new(7).float32());
    }

    #[test]
    fn set_params_seeds_with_zero_when_random_state_is_not_an_integer() {
        let mut model = BaseModel::default();
        model.set_params(params(&[(
            RANDOM_STATE,
            ParamValue::String("x".to_owned()),
        )]));
        let first = model.get_random_generator().float32();
        assert_eq!(first, RandomGenerator::new(0).float32());
    }

    #[test]
    fn get_params_returns_what_set_params_stored() {
        let mut model = BaseModel::default();
        let stored = params(&[(RANDOM_STATE, ParamValue::Int(3))]);
        model.set_params(stored.clone());
        assert_eq!(model.get_params(), &stored);
    }

    #[test]
    #[should_panic(expected = "random generator used before set_params")]
    fn random_generator_panics_before_set_params() {
        let mut model = BaseModel::default();
        let _ = model.get_random_generator();
    }
}
