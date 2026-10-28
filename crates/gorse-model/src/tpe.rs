// Ports: c-bata/goptuna@v0.9.0 study.go (Study, CreateStudy, Optimize, runTrial, GetBestValue), trial.go (Trial, suggest, SuggestFloat, SuggestLogFloat, SuggestDiscreteFloat, SuggestInt, SuggestStepInt, SuggestCategorical, Number), sampler.go (Sampler), storage.go (InMemoryStorage: SetTrialIntermediateValue, SetTrialValue, SetTrialState, GetBestTrial)

//! Hyper-parameter search, ported from goptuna v0.9.0 (`github.com/c-bata/goptuna`,
//! the library Gorse's tuning uses; `porting/crate-selection.md` names this
//! port).
//!
//! A [`Study`] runs an objective over trials. Each [`Trial`] suggests its
//! parameters through the study's [`Sampler`], which is [`RandomSampler`] or
//! [`TpeSampler`], and the objective returns the value to maximize or minimize.
//!
//! The port keeps the in-memory storage that Gorse uses and the paths the
//! samplers take for it. goptuna's fixed parameters (`EnqueueTrial`), relative
//! samplers, pruners, user and system attributes, and the other storage
//! backends are not ported: Gorse does not reach them (`PORTING_NOTES.md`).

pub mod distribution;
#[cfg(test)]
mod golden;
mod gomath;
mod parzen;
mod random;
mod sampler;

use std::collections::BTreeMap;

use gorse_common::gocontext::{Context, ContextError};
use gorse_model_core::model::SuggestTrial;

pub use distribution::Distribution;
pub use parzen::{FuncWeights, ParzenEstimator, ParzenEstimatorParams};
pub use random::RandomSampler;
pub use sampler::{FuncGamma, TpeSampler, default_gamma, default_weights, hyperopt_default_gamma};

/// goptuna's `StudyDirection`: whether the objective is minimized or maximized.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Direction {
    /// Smaller values are better. goptuna's default.
    #[default]
    Minimize,
    /// Larger values are better.
    Maximize,
}

/// goptuna's `TrialState`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TrialState {
    /// The trial is running.
    #[default]
    Running,
    /// The objective returned a value.
    Complete,
    /// The objective stopped early, and its last intermediate value is its value.
    Pruned,
    /// The objective returned an error.
    Fail,
}

/// goptuna's `FrozenTrial`: a trial's state after it ran, or while it runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrozenTrial {
    /// The trial's state.
    pub state: TrialState,
    /// The objective value, once the trial is complete or pruned.
    pub value: f64,
    /// The suggested parameters, in their internal representation (goptuna's
    /// `InternalParams`).
    pub internal_params: BTreeMap<String, f64>,
    /// The intermediate values, by step.
    pub intermediate_values: BTreeMap<i64, f64>,
}

/// Errors of studies and trials.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A range's lower bound is above its upper bound.
    #[error("'low' must be smaller than or equal to the 'high'")]
    LowAboveHigh,
    /// A categorical distribution has no choices.
    #[error("'choices' must contains one or more elements")]
    NoChoices,
    /// A step of an integer range is not positive.
    #[error("'step' must be larger than 0")]
    NonPositiveStep,
    /// The study has no complete trial to take the best value from.
    #[error("no trials are completed yet")]
    NoCompletedTrials,
    /// The trial is finished, so its values cannot change.
    #[error("trial cannot be updated")]
    TrialCannotBeUpdated,
    /// The trial already has a value for this step.
    #[error("step value is already exist")]
    StepExists,
    /// The trial id names no trial of the study.
    #[error("invalid trial id")]
    InvalidTrialId,
    /// The probabilities of a multinomial draw have no bucket above the draw.
    #[error("invalid pvals")]
    InvalidPvals,
    /// The study's context was cancelled before a trial started.
    #[error(transparent)]
    Context(#[from] ContextError),
    /// The objective failed, and the study does not ignore errors.
    #[error("{0}")]
    Objective(Box<dyn std::error::Error + Send + Sync>),
}

/// What an objective returns instead of a value.
#[derive(Debug)]
pub enum Failure {
    /// goptuna's `ErrTrialPruned`.
    Pruned,
    /// Any other error. The trial fails, and [`Study::optimize`] returns the
    /// error unless the study ignores errors.
    Error(Box<dyn std::error::Error + Send + Sync>),
}

impl Failure {
    /// The failure of an objective that returned `error`.
    pub fn error<E: Into<Box<dyn std::error::Error + Send + Sync>>>(error: E) -> Self {
        Failure::Error(error.into())
    }
}

/// goptuna's `Sampler`: suggests the value of one parameter of a trial.
pub trait Sampler: Send + Sync {
    /// Go's `Sample`: the value of `name` in its internal representation, given
    /// the trials of `study` so far.
    fn sample(&self, study: &Study, name: &str, distribution: &Distribution) -> Result<f64, Error>;
}

/// goptuna's `Study`: the trials of one optimization, its direction and its sampler.
pub struct Study {
    direction: Direction,
    sampler: Box<dyn Sampler>,
    ignore_error: bool,
    ctx: Option<Context>,
    trials: Vec<FrozenTrial>,
}

impl Study {
    /// goptuna's `CreateStudy` with `StudyOptionDirection` and
    /// `StudyOptionSampler`. Errors stop the study, and it has no context.
    pub fn new(direction: Direction, sampler: Box<dyn Sampler>) -> Self {
        Study {
            direction,
            sampler,
            ignore_error: false,
            ctx: None,
            trials: Vec::new(),
        }
    }

    /// goptuna's `StudyOptionIgnoreError`: a failed trial does not stop the
    /// study.
    pub fn with_ignore_error(mut self, ignore: bool) -> Self {
        self.ignore_error = ignore;
        self
    }

    /// goptuna's `WithContext`: the study stops with the context's error once
    /// it is cancelled.
    pub fn with_context(&mut self, ctx: Context) {
        self.ctx = Some(ctx);
    }

    /// goptuna's `Direction`.
    pub fn direction(&self) -> Direction {
        self.direction
    }

    /// goptuna's `GetTrials`: the trials in the order they were created.
    pub fn trials(&self) -> &[FrozenTrial] {
        &self.trials
    }

    /// goptuna's `Optimize`: runs `objective` on up to `evaluate_max` new trials,
    /// one after another. A failed trial stops the study with the error, unless
    /// the study ignores errors. A pruned trial keeps its last intermediate value.
    pub fn optimize<F>(&mut self, mut objective: F, evaluate_max: usize) -> Result<(), Error>
    where
        F: FnMut(&mut Trial<'_>) -> Result<f64, Failure>,
    {
        for _ in 0..evaluate_max {
            if let Some(err) = self.ctx.as_ref().and_then(Context::err) {
                return Err(err.into());
            }
            let trial_id = self.trials.len();
            self.trials.push(FrozenTrial::default());
            let result = {
                let mut trial = Trial {
                    study: &mut *self,
                    id: trial_id,
                };
                objective(&mut trial)
            };
            let objective_error = self.finish_trial(trial_id, result);
            if let Some(err) = objective_error
                && !self.ignore_error
            {
                return Err(Error::Objective(err));
            }
        }
        Ok(())
    }

    /// Records how a trial ended: its value and its state. Returns the
    /// objective's error, if it failed.
    fn finish_trial(
        &mut self,
        trial_id: usize,
        result: Result<f64, Failure>,
    ) -> Option<Box<dyn std::error::Error + Send + Sync>> {
        let trial = &mut self.trials[trial_id];
        match result {
            Ok(evaluation) => {
                trial.value = evaluation;
                trial.state = TrialState::Complete;
                tracing::info!(trial_id, state = "COMPLETE", evaluation, "Trial finished");
                None
            }
            Err(Failure::Pruned) => {
                // The last intermediate value, if there is one, is the value.
                if let Some((_, &last)) = trial.intermediate_values.last_key_value() {
                    trial.value = last;
                }
                trial.state = TrialState::Pruned;
                tracing::info!(
                    trial_id,
                    state = "PRUNED",
                    evaluation = trial.value,
                    "Trial finished"
                );
                None
            }
            Err(Failure::Error(err)) => {
                trial.state = TrialState::Fail;
                tracing::error!(trial_id, state = "FAIL", error = %err, "Objective function returns error");
                Some(err)
            }
        }
    }

    /// goptuna's `GetBestValue`: the value of the complete trial that is best
    /// in the study's direction. The first of equal trials wins.
    pub fn best_value(&self) -> Result<f64, Error> {
        let mut best: Option<f64> = None;
        for trial in self
            .trials
            .iter()
            .filter(|t| t.state == TrialState::Complete)
        {
            best = Some(match best {
                None => trial.value,
                Some(b) => {
                    let better = match self.direction {
                        Direction::Maximize => trial.value > b,
                        Direction::Minimize => trial.value < b,
                    };
                    if better { trial.value } else { b }
                }
            });
        }
        best.ok_or(Error::NoCompletedTrials)
    }

    /// goptuna's `Storage.SetTrialIntermediateValue`: sets the value at `step`
    /// of a running trial. Each step is set once.
    pub fn set_trial_intermediate_value(
        &mut self,
        trial_id: usize,
        step: i64,
        value: f64,
    ) -> Result<(), Error> {
        let trial = self.trials.get_mut(trial_id).ok_or(Error::InvalidTrialId)?;
        if trial.state != TrialState::Running {
            return Err(Error::TrialCannotBeUpdated);
        }
        if trial.intermediate_values.contains_key(&step) {
            return Err(Error::StepExists);
        }
        trial.intermediate_values.insert(step, value);
        Ok(())
    }
}

/// A trial of a [`Study`]: the handle an objective suggests parameters through.
pub struct Trial<'a> {
    study: &'a mut Study,
    id: usize,
}

impl Trial<'_> {
    /// goptuna's `Number`: the trial's position in its study, from 0.
    pub fn number(&self) -> usize {
        self.id
    }

    /// goptuna's `SuggestFloat`: a float in `[low, high]`.
    pub fn suggest_float(&mut self, name: &str, low: f64, high: f64) -> Result<f64, Error> {
        if low > high {
            return Err(Error::LowAboveHigh);
        }
        self.suggest(name, Distribution::Uniform { low, high })
    }

    /// goptuna's `SuggestLogFloat`: a float in `[low, high]`, uniform in the log domain.
    pub fn suggest_log_float(&mut self, name: &str, low: f64, high: f64) -> Result<f64, Error> {
        if low > high {
            return Err(Error::LowAboveHigh);
        }
        self.suggest(name, Distribution::LogUniform { low, high })
    }

    /// goptuna's `SuggestDiscreteFloat`: a float on the grid `low + k*q` up to `high`.
    pub fn suggest_discrete_float(
        &mut self,
        name: &str,
        low: f64,
        high: f64,
        q: f64,
    ) -> Result<f64, Error> {
        if low > high {
            return Err(Error::LowAboveHigh);
        }
        let distribution = Distribution::DiscreteUniform { low, high, q };
        let internal = self.suggest(name, distribution.clone())?;
        Ok(distribution.to_external_float(internal))
    }

    /// goptuna's `SuggestInt`: an integer in `[low, high]`.
    pub fn suggest_int(&mut self, name: &str, low: i64, high: i64) -> Result<i64, Error> {
        if low > high {
            return Err(Error::LowAboveHigh);
        }
        let internal = self.suggest(name, Distribution::IntUniform { low, high })?;
        Ok(internal.round() as i64)
    }

    /// goptuna's `SuggestStepInt`: an integer in `[low, high)` on a grid of `step`.
    pub fn suggest_step_int(
        &mut self,
        name: &str,
        low: i64,
        high: i64,
        step: i64,
    ) -> Result<i64, Error> {
        if low > high {
            return Err(Error::LowAboveHigh);
        }
        if step <= 0 {
            return Err(Error::NonPositiveStep);
        }
        let distribution = Distribution::StepIntUniform { low, high, step };
        let internal = self.suggest(name, distribution.clone())?;
        Ok(distribution.to_external_float(internal).round() as i64)
    }

    /// goptuna's `SuggestCategorical`: one of `choices`.
    pub fn suggest_categorical(&mut self, name: &str, choices: &[&str]) -> Result<String, Error> {
        if choices.is_empty() {
            return Err(Error::NoChoices);
        }
        let distribution = Distribution::Categorical {
            choices: choices.iter().map(|c| (*c).to_owned()).collect(),
        };
        let internal = self.suggest(name, distribution)?;
        Ok(choices[internal as usize].to_owned())
    }

    /// goptuna's `Storage.SetTrialIntermediateValue` for this trial.
    pub fn set_intermediate_value(&mut self, step: i64, value: f64) -> Result<(), Error> {
        self.study
            .set_trial_intermediate_value(self.id, step, value)
    }

    /// goptuna's `suggest`: samples `name` with the study's sampler and records
    /// the internal value on this trial.
    fn suggest(&mut self, name: &str, distribution: Distribution) -> Result<f64, Error> {
        let study: &Study = self.study;
        let value = study.sampler.sample(study, name, &distribution)?;
        self.study.trials[self.id]
            .internal_params
            .insert(name.to_owned(), value);
        Ok(value)
    }
}

/// Unwraps a suggestion the way Go's `lo.Must` does: a goptuna error panics.
fn must(suggestion: Result<f64, Error>) -> f64 {
    suggestion.unwrap_or_else(|err| panic!("{err}"))
}

impl SuggestTrial for Trial<'_> {
    fn suggest_log_float(&mut self, name: &str, low: f64, high: f64) -> f64 {
        must(Trial::suggest_log_float(self, name, low, high))
    }

    fn suggest_discrete_float(&mut self, name: &str, low: f64, high: f64, q: f64) -> f64 {
        must(Trial::suggest_discrete_float(self, name, low, high, q))
    }
}
