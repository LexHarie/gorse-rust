// Ports: model/ctr/evaluator.go @ add21b5

//! Scores of factorization machines on a test set: the root mean square error
//! of a regression, and precision, recall, accuracy and AUC of a
//! classification.

use gorse_common::gosort;
use gorse_common::math32;
use gorse_dataset::dataset::CtrSplit;

use super::data::Dataset;
use super::model::{FactorizationMachines, Score};

/// Evaluates factorization machines in regression task (Go
/// `EvaluateRegression`).
pub fn evaluate_regression(estimator: &dyn FactorizationMachines, test_set: &Dataset) -> Score {
    let mut sum = 0.0f32;
    // For all UserFeedback
    for i in 0..test_set.count() {
        let (features, values, _, target) = test_set.get(i);
        let prediction = estimator.internal_predict(&features, &values);
        sum += (target - prediction) * (target - prediction);
    }
    if test_set.count() == 0 {
        return Score::default();
    }
    Score {
        rmse: math32::sqrt(sum / test_set.count() as f32),
        ..Score::default()
    }
}

/// Evaluates factorization machines in classification task (Go
/// `EvaluateClassification`). Predicts with the batch inference of `estimator`
/// when it has one, and with [`FactorizationMachines::internal_predict`]
/// otherwise.
pub fn evaluate_classification(
    estimator: &dyn FactorizationMachines,
    test_set: &dyn CtrSplit,
    jobs: usize,
) -> Score {
    // For all UserFeedback
    let mut pos_features = Vec::new();
    let mut neg_features = Vec::new();
    let mut pos_embeddings = Vec::new();
    let mut neg_embeddings = Vec::new();
    for i in 0..test_set.count() {
        let (indices, values, embeddings, target) = test_set.get(i);
        if target > 0.0 {
            pos_features.push((indices, values));
            pos_embeddings.push(embeddings);
        } else {
            neg_features.push((indices, values));
            neg_embeddings.push(embeddings);
        }
    }
    let (mut pos_prediction, mut neg_prediction): (Vec<f32>, Vec<f32>) =
        if let Some(batch) = estimator.as_batch_inference() {
            (
                batch.batch_internal_predict(&pos_features, &pos_embeddings, jobs),
                batch.batch_internal_predict(&neg_features, &neg_embeddings, jobs),
            )
        } else {
            (
                pos_features
                    .iter()
                    .map(|(indices, values)| estimator.internal_predict(indices, values))
                    .collect(),
                neg_features
                    .iter()
                    .map(|(indices, values)| estimator.internal_predict(indices, values))
                    .collect(),
            )
        };
    if test_set.count() == 0 {
        return Score::default();
    }
    Score {
        precision: precision(&pos_prediction, &neg_prediction),
        recall: recall(&pos_prediction, &neg_prediction),
        accuracy: accuracy(&pos_prediction, &neg_prediction),
        auc: auc(&mut pos_prediction, &mut neg_prediction),
        ..Score::default()
    }
}

/// The fraction of positive predictions that are true positives, or 0 when
/// nothing is predicted positive (Go `Precision`).
pub fn precision(pos_prediction: &[f32], neg_prediction: &[f32]) -> f32 {
    let mut tp = 0.0f32;
    let mut fp = 0.0f32;
    for &p in pos_prediction {
        if p > 0.0 {
            // true positive
            tp += 1.0;
        }
    }
    for &p in neg_prediction {
        if p > 0.0 {
            // false positive
            fp += 1.0;
        }
    }
    if tp + fp == 0.0 {
        return 0.0;
    }
    tp / (tp + fp)
}

/// The fraction of positive samples predicted positive, or 0 when there are
/// none (Go `Recall`). The negative predictions are not used.
pub fn recall(pos_prediction: &[f32], _neg_prediction: &[f32]) -> f32 {
    let mut tp = 0.0f32;
    let mut fn_ = 0.0f32;
    for &p in pos_prediction {
        if p > 0.0 {
            // true positive
            tp += 1.0;
        } else {
            // false negative
            fn_ += 1.0;
        }
    }
    if tp + fn_ == 0.0 {
        return 0.0;
    }
    tp / (tp + fn_)
}

/// The fraction of predictions with the right sign, or 0 when there are no
/// predictions (Go `Accuracy`).
pub fn accuracy(pos_prediction: &[f32], neg_prediction: &[f32]) -> f32 {
    let mut correct = 0.0f32;
    for &p in pos_prediction {
        if p > 0.0 {
            correct += 1.0;
        }
    }
    for &p in neg_prediction {
        if p < 0.0 {
            correct += 1.0;
        }
    }
    let total = pos_prediction.len() + neg_prediction.len();
    if total == 0 {
        return 0.0;
    }
    correct / total as f32
}

/// The area under the ROC curve: the fraction of (positive, negative) pairs
/// in which the positive sample has the greater prediction, or 0 when either
/// side is empty (Go `AUC`). Sorts both slices in place, as Go does.
pub fn auc(pos_prediction: &mut [f32], neg_prediction: &mut [f32]) -> f32 {
    gosort::sort(&mut Floats(pos_prediction));
    gosort::sort(&mut Floats(neg_prediction));
    let mut sum = 0.0f32;
    let mut n_pos = 0usize;
    for pos in pos_prediction.iter() {
        // find the negative sample with the greatest prediction less than current positive sample
        while n_pos < neg_prediction.len() && neg_prediction[n_pos] < *pos {
            n_pos += 1;
        }
        // add the number of negative samples have less prediction than current positive sample
        sum += n_pos as f32;
    }
    let pairs = pos_prediction.len() * neg_prediction.len();
    if pairs == 0 {
        return 0.0;
    }
    sum / pairs as f32
}

/// Floats in `<` order, for sorting predictions.
struct Floats<'a>(&'a mut [f32]);

impl gosort::Interface for Floats<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        self.0[i] < self.0[j]
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.0.swap(i, j);
    }
}

#[cfg(test)]
mod tests;
