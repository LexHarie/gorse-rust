// Ports: c-bata/goptuna@v0.9.0 distribution.go (UniformDistribution, LogUniformDistribution, IntUniformDistribution, StepIntUniformDistribution, DiscreteUniformDistribution, CategoricalDistribution)

//! The distributions a trial suggests from: goptuna's distribution types, with
//! the internal representation each one stores and the single-value test the
//! samplers use.
//!
//! Go's `Contains` is not ported. Go calls it only to validate a fixed
//! parameter from `EnqueueTrial`, which Gorse never does.

/// goptuna's `Distribution` types. A suggested value is stored in its internal
/// representation, a float, and [`Distribution::to_external_float`] maps it back.
#[derive(Debug, Clone, PartialEq)]
pub enum Distribution {
    /// `UniformDistribution`: a float in `[low, high]`.
    Uniform {
        /// The lower endpoint, included.
        low: f64,
        /// The higher endpoint, included.
        high: f64,
    },
    /// `LogUniformDistribution`: a float in `[low, high]`, uniform in the log domain.
    LogUniform {
        /// The lower endpoint, included.
        low: f64,
        /// The higher endpoint, included.
        high: f64,
    },
    /// `IntUniformDistribution`: an integer in `[low, high]`.
    IntUniform {
        /// The lower endpoint, included.
        low: i64,
        /// The higher endpoint, included.
        high: i64,
    },
    /// `StepIntUniformDistribution`: an integer in `[low, high)` on a grid of `step`.
    StepIntUniform {
        /// The lower endpoint.
        low: i64,
        /// The higher endpoint.
        high: i64,
        /// The spacing between values.
        step: i64,
    },
    /// `DiscreteUniformDistribution`: a float on the grid `low + k*q` up to `high`.
    DiscreteUniform {
        /// The lower endpoint, included.
        low: f64,
        /// The higher endpoint, included.
        high: f64,
        /// The discretization step.
        q: f64,
    },
    /// `CategoricalDistribution`: an index into `choices`.
    Categorical {
        /// The candidate values.
        choices: Vec<String>,
    },
}

impl Distribution {
    /// Go's `Single`: whether the range holds just one value, so a sampler
    /// returns it without drawing.
    pub fn single(&self) -> bool {
        match self {
            Distribution::Uniform { low, high }
            | Distribution::LogUniform { low, high }
            | Distribution::DiscreteUniform { low, high, .. } => high == low,
            Distribution::IntUniform { low, high } => high == low,
            Distribution::StepIntUniform { low, high, step } => {
                if high == low {
                    return true;
                }
                (high - low) < *step
            }
            Distribution::Categorical { choices } => choices.len() == 1,
        }
    }

    /// Go's `ToExternalRepr` for the float and integer distributions: the value
    /// a caller sees for an internal representation `ir`. The integer forms are
    /// returned as `f64` here, as the internal value is; [`crate::tpe::Trial`]
    /// converts them to integers.
    pub fn to_external_float(&self, ir: f64) -> f64 {
        match self {
            Distribution::Uniform { .. } | Distribution::LogUniform { .. } => ir,
            Distribution::IntUniform { .. } => ir.round(),
            Distribution::StepIntUniform { low, step, .. } => {
                // shift to [0, (high-low)/step]
                let r = (ir - *low as f64) / *step as f64;
                (r.round() as i64 * step + low) as f64
            }
            Distribution::DiscreteUniform { low, q, .. } => {
                ((ir - low) / q + 0.5).floor() * q + low
            }
            Distribution::Categorical { .. } => ir,
        }
    }
}

#[cfg(test)]
mod tests {
    // Ports: distribution_test.go (TestDistributionToExternalRepresentation, TestDistributionSingle) @ c-bata/goptuna v0.9.0

    use super::*;

    #[test]
    fn to_external_representation() {
        // The cases of goptuna's TestDistributionToExternalRepresentation. The
        // categorical case returns the choice's index here; Go returns the string.
        let uniform = Distribution::Uniform {
            low: 0.5,
            high: 5.5,
        };
        assert_eq!(uniform.to_external_float(3.5), 3.5);

        let log_uniform = Distribution::LogUniform {
            low: 1e-2,
            high: 1e3,
        };
        assert_eq!(log_uniform.to_external_float(1e2), 1e2);

        let int_uniform = Distribution::IntUniform { low: 0, high: 10 };
        assert_eq!(int_uniform.to_external_float(3.0), 3.0);
        assert_eq!(int_uniform.to_external_float(1.6), 2.0);

        let step_int = Distribution::StepIntUniform {
            low: 0,
            high: 10,
            step: 2,
        };
        assert_eq!(step_int.to_external_float(2.2), 2.0);
        let step_int = Distribution::StepIntUniform {
            low: 0,
            high: 10,
            step: 3,
        };
        assert_eq!(step_int.to_external_float(2.2), 3.0);
        let step_int = Distribution::StepIntUniform {
            low: -1,
            high: 10,
            step: 3,
        };
        assert_eq!(step_int.to_external_float(0.6), 2.0);

        let discrete = Distribution::DiscreteUniform {
            low: 0.5,
            high: 5.5,
            q: 0.5,
        };
        assert_eq!(discrete.to_external_float(3.5), 3.5);
        assert_eq!(discrete.to_external_float(3.3), 3.5);
        let discrete = Distribution::DiscreteUniform {
            low: 0.5,
            high: 5.5,
            q: 0.05,
        };
        assert_eq!(discrete.to_external_float(3.52), 3.5);

        let categorical = Distribution::Categorical {
            choices: vec!["a".into(), "b".into(), "c".into()],
        };
        assert_eq!(categorical.to_external_float(2.0), 2.0);
    }

    #[test]
    fn single() {
        // The cases of goptuna's TestDistributionSingle.
        let uniform = |low, high| Distribution::Uniform { low, high };
        assert!(uniform(0.5, 0.5).single());
        assert!(!uniform(0.5, 5.5).single());

        let log_uniform = |low, high| Distribution::LogUniform { low, high };
        assert!(log_uniform(1e3, 1e3).single());
        assert!(!log_uniform(1e-3, 1e2).single());

        assert!(Distribution::IntUniform { low: 10, high: 10 }.single());
        assert!(!Distribution::IntUniform { low: 0, high: 10 }.single());

        let discrete = |low, high| Distribution::DiscreteUniform { low, high, q: 1.0 };
        assert!(discrete(10.0, 10.0).single());
        assert!(!discrete(0.0, 10.0).single());

        let step_int = |low, high, step| Distribution::StepIntUniform { low, high, step };
        assert!(step_int(10, 10, 2).single());
        // A range shorter than one step holds one value.
        assert!(step_int(0, 1, 2).single());
        assert!(!step_int(0, 10, 2).single());

        let categorical = |choices: &[&str]| Distribution::Categorical {
            choices: choices.iter().map(|c| (*c).to_owned()).collect(),
        };
        assert!(categorical(&["a"]).single());
        assert!(!categorical(&["a", "b", "c"]).single());
    }
}
