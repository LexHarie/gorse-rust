// Ports: model/ctr/optimize.go @ add21b5

//! Hyper-parameter search over the click-through-rate models: the model type is
//! one categorical choice, and the chosen model suggests its own parameters, is
//! fitted, and scores by AUC. The tests are Go's `TestTPE`.

use std::collections::BTreeMap;
use std::sync::Arc;

use gorse_common::gocontext::Context;
use gorse_common::monitor::Span;
use gorse_dataset::dataset::CtrSplit;
use gorse_model_core::params::Params;
use gorse_storage::meta::Model as MetaModel;

use super::model::{FactorizationMachines, FitConfig, Score};
use crate::tpe::{Failure, Trial};

/// Go's `ModelCreator`: makes a fresh model of one type.
pub type ModelCreator = Box<dyn Fn() -> Box<dyn FactorizationMachines>>;

/// Errors of the model search.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The search was built with no model to choose from.
    #[error("no model to search")]
    NoModel,
}

/// Go's `ModelSearch`: the objective of a tuning study over click-through-rate
/// models, and the best model it has found.
pub struct ModelSearch {
    model_creators: BTreeMap<String, ModelCreator>,
    train_set: Box<dyn CtrSplit>,
    test_set: Box<dyn CtrSplit>,
    config: FitConfig,
    ctx: Context,
    span: Option<Arc<Span>>,
    result: MetaModel<Score>,
}

impl ModelSearch {
    /// Go's `NewModelSearch`. The model types are tried in name order. Go
    /// takes them from a map, whose order is random, so any fixed order is
    /// Go's behaviour too.
    pub fn new(
        models: BTreeMap<String, ModelCreator>,
        train_set: Box<dyn CtrSplit>,
        test_set: Box<dyn CtrSplit>,
        config: FitConfig,
    ) -> Self {
        ModelSearch {
            model_creators: models,
            train_set,
            test_set,
            config,
            ctx: Context::background(),
            span: None,
            result: MetaModel::default(),
        }
    }

    /// Go's `WithContext`: a cancelled context stops the fits.
    pub fn with_context(mut self, ctx: Context) -> Self {
        self.ctx = ctx;
        self
    }

    /// Go's `WithSpan`: counts each finished trial on the span.
    pub fn with_span(mut self, span: Arc<Span>) -> Self {
        self.span = Some(span);
        self
    }

    /// Go's `Objective`: chooses a model type, suggests its parameters, fits it,
    /// and returns its AUC. A better model than the best so far becomes the result.
    pub fn objective(&mut self, trial: &mut Trial<'_>) -> Result<f64, Failure> {
        if self.model_creators.is_empty() {
            return Err(Failure::error(Error::NoModel));
        }
        let types: Vec<&str> = self.model_creators.keys().map(String::as_str).collect();
        let model_type = trial
            .suggest_categorical("Model", &types)
            .map_err(Failure::error)?;
        let mut m = (self.model_creators[&model_type])();
        let params: Params = m.suggest_params(trial);
        m.set_params(params);
        let score = m.fit(
            &self.ctx,
            self.train_set.as_ref(),
            self.test_set.as_ref(),
            &self.config,
        );
        if score.auc > self.result.score.auc {
            self.result = MetaModel {
                r#type: model_type,
                params: m.get_params().clone(),
                score,
                ..MetaModel::default()
            };
        }
        if let Some(span) = &self.span {
            span.add(1);
        }
        Ok(f64::from(score.auc))
    }

    /// Go's `Result`: the best model type, its parameters and its score so far.
    pub fn result(&self) -> MetaModel<Score> {
        self.result.clone()
    }
}

#[cfg(test)]
mod tests {
    // Ports: model/ctr/optimize_test.go, TestTPE @ add21b5

    use std::io::Write;

    use gorse_dataset::index::Index;
    use gorse_dataset::unified_index::UnifiedIndex;
    use gorse_model_core::model::{BaseModel, Model as CoreModel, SuggestTrial};
    use gorse_model_core::params::{INIT_MEAN, INIT_STD_DEV, N_FACTORS, ParamValue};

    use super::*;
    use crate::ctr::data::Label;
    use crate::ctr::model::Error as ModelError;
    use crate::tpe::{Direction, Study, TpeSampler};

    /// Go's `mockFactorizationMachineForSearch`. Its fit scores the sum of its
    /// three parameters, so the best trial is the largest suggestion of each.
    struct MockFactorizationMachineForSearch {
        base: BaseModel,
    }

    impl CoreModel for MockFactorizationMachineForSearch {
        fn set_params(&mut self, params: Params) {
            self.base.set_params(params);
        }

        fn get_params(&self) -> &Params {
            self.base.get_params()
        }

        fn suggest_params(&self, trial: &mut dyn SuggestTrial) -> Params {
            Params(BTreeMap::from([
                (
                    N_FACTORS.to_owned(),
                    ParamValue::Float64(trial.suggest_discrete_float(N_FACTORS, 1.0, 4.0, 1.0)),
                ),
                (
                    INIT_MEAN.to_owned(),
                    ParamValue::Float64(trial.suggest_discrete_float(INIT_MEAN, 1.0, 4.0, 1.0)),
                ),
                (
                    INIT_STD_DEV.to_owned(),
                    ParamValue::Float64(trial.suggest_discrete_float(INIT_STD_DEV, 4.0, 4.0, 1.0)),
                ),
            ]))
        }

        fn clear(&mut self) {
            // do nothing
        }

        fn invalid(&self) -> bool {
            panic!("implement me")
        }
    }

    impl FactorizationMachines for MockFactorizationMachineForSearch {
        fn header(&self) -> &'static str {
            panic!("don't call me")
        }

        fn predict(
            &self,
            _user_id: &str,
            _item_id: &str,
            _user_features: &[Label],
            _item_features: &[Label],
        ) -> f32 {
            panic!("don't call me")
        }

        fn internal_predict(&self, _x: &[i32], _values: &[f32]) -> f32 {
            panic!("don't call me")
        }

        fn fit(
            &mut self,
            _ctx: &Context,
            _train_set: &dyn CtrSplit,
            _test_set: &dyn CtrSplit,
            _config: &FitConfig,
        ) -> Score {
            let params = self.base.get_params();
            let auc = params.get_float32(N_FACTORS, 0.0)
                + params.get_float32(INIT_MEAN, 0.0)
                + params.get_float32(INIT_STD_DEV, 0.0);
            Score {
                auc,
                ..Score::default()
            }
        }

        fn marshal<W: Write>(&self, _w: &mut W) -> Result<(), ModelError>
        where
            Self: Sized,
        {
            panic!("implement me")
        }
    }

    /// Go's nil `dataset.CTRSplit`. The mock never reads its splits, so every
    /// method panics if the search ever calls one.
    struct NoSplit;

    impl CtrSplit for NoSplit {
        fn count(&self) -> usize {
            panic!("don't call me")
        }

        fn count_users(&self) -> usize {
            panic!("don't call me")
        }

        fn count_items(&self) -> usize {
            panic!("don't call me")
        }

        fn count_user_labels(&self) -> usize {
            panic!("don't call me")
        }

        fn count_item_labels(&self) -> usize {
            panic!("don't call me")
        }

        fn count_context_labels(&self) -> usize {
            panic!("don't call me")
        }

        fn count_positive(&self) -> usize {
            panic!("don't call me")
        }

        fn count_negative(&self) -> usize {
            panic!("don't call me")
        }

        fn get_index(&self) -> Option<&UnifiedIndex> {
            panic!("don't call me")
        }

        fn get_target(&self, _i: usize) -> f32 {
            panic!("don't call me")
        }

        fn get(&self, _i: usize) -> (Vec<i32>, Vec<f32>, Vec<Vec<u16>>, f32) {
            panic!("don't call me")
        }

        fn get_item_embedding_dim(&self) -> Vec<usize> {
            panic!("don't call me")
        }

        fn get_item_embedding_index(&self) -> Option<&Index> {
            panic!("don't call me")
        }
    }

    #[test]
    fn test_tpe() {
        let models: BTreeMap<String, ModelCreator> = BTreeMap::from([(
            "mock".to_owned(),
            Box::new(|| -> Box<dyn FactorizationMachines> {
                Box::new(MockFactorizationMachineForSearch {
                    base: BaseModel::default(),
                })
            }) as ModelCreator,
        )]);
        let mut search = ModelSearch::new(
            models,
            Box::new(NoSplit),
            Box::new(NoSplit),
            FitConfig::new(),
        );
        let mut study = Study::new(Direction::Maximize, Box::new(TpeSampler::new()));
        study
            .optimize(|trial| search.objective(trial), 10)
            .expect("the study runs");
        assert_eq!(study.best_value().expect("a trial completed"), 12.0);
        let result = search.result();
        assert_eq!(result.r#type, "mock");
        assert_eq!(
            result.params,
            Params(BTreeMap::from([
                (N_FACTORS.to_owned(), ParamValue::Float64(4.0)),
                (INIT_MEAN.to_owned(), ParamValue::Float64(4.0)),
                (INIT_STD_DEV.to_owned(), ParamValue::Float64(4.0)),
            ]))
        );
        assert_eq!(
            result.score,
            Score {
                auc: 12.0,
                ..Score::default()
            }
        );
    }

    #[test]
    fn objective_without_models_reports_no_model() {
        let mut search = ModelSearch::new(
            BTreeMap::new(),
            Box::new(NoSplit),
            Box::new(NoSplit),
            FitConfig::new(),
        );
        let mut study = Study::new(Direction::Maximize, Box::new(TpeSampler::new()));
        let err = study
            .optimize(|trial| search.objective(trial), 1)
            .expect_err("the search has no model to choose");
        assert_eq!(err.to_string(), "no model to search");
    }
}
