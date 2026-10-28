// Ports: model/ctr/evaluator_test.go @ add21b5

use super::{accuracy, precision, recall};

#[test]
fn precision_test() {
    let pos_prediction = [1.0, 1.0, 1.0];
    let neg_prediction = [1.0];
    assert_eq!(precision(&pos_prediction, &neg_prediction), 0.75);
    assert_eq!(precision(&[], &[]), 0.0);
}

#[test]
fn recall_test() {
    let pos_prediction = [1.0, -1.0, -1.0, -1.0];
    assert_eq!(recall(&pos_prediction, &[]), 0.25);
    assert_eq!(recall(&[], &[]), 0.0);
}

#[test]
fn accuracy_test() {
    let pos_prediction = [1.0, 1.0, -1.0, -1.0];
    let neg_prediction = [1.0, 1.0, -1.0, -1.0];
    assert_eq!(accuracy(&pos_prediction, &neg_prediction), 0.5);
    assert_eq!(accuracy(&[], &[]), 0.0);
}
