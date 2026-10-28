// Ports: model/ctr/transformer_test.go @ add21b5

use super::{AutoScaler, MinMaxScaler, RobustScaler, log1p};

#[test]
fn min_max_scaler_fit_and_transform() {
    let mut scaler = MinMaxScaler::new();
    scaler.fit(&[1.0, 2.0, 3.0, 4.0, 5.0]);

    assert_eq!(scaler.min, 1.0);
    assert_eq!(scaler.max, 5.0);

    // Test transform
    assert_eq!(scaler.transform(1.0), 0.0);
    assert_eq!(scaler.transform(2.0), 0.25);
    assert_eq!(scaler.transform(3.0), 0.5);
    assert_eq!(scaler.transform(4.0), 0.75);
    assert_eq!(scaler.transform(5.0), 1.0);
}

#[test]
fn min_max_scaler_constant_values() {
    let mut scaler = MinMaxScaler::new();
    scaler.fit(&[5.0, 5.0, 5.0]);

    assert_eq!(scaler.min, 5.0);
    assert_eq!(scaler.max, 5.0);

    // When range is 0, should return 1
    assert_eq!(scaler.transform(5.0), 1.0);
}

#[test]
fn min_max_scaler_negative_values() {
    let mut scaler = MinMaxScaler::new();
    scaler.fit(&[-5.0, -3.0, 0.0, 3.0, 5.0]);

    assert_eq!(scaler.min, -5.0);
    assert_eq!(scaler.max, 5.0);

    assert_eq!(scaler.transform(-5.0), 0.0);
    assert_eq!(scaler.transform(-2.0), 0.3);
    assert_eq!(scaler.transform(0.0), 0.5);
    assert_eq!(scaler.transform(5.0), 1.0);
}

#[test]
fn min_max_scaler_marshal_and_unmarshal() {
    let mut scaler = MinMaxScaler::new();
    scaler.fit(&[10.0, 20.0, 30.0]);

    // Marshal
    let mut buf = Vec::new();
    scaler.marshal(&mut buf).expect("marshal");

    // Unmarshal
    let mut scaler2 = MinMaxScaler::new();
    scaler2.unmarshal(&mut buf.as_slice()).expect("unmarshal");

    assert_eq!(scaler.min, scaler2.min);
    assert_eq!(scaler.max, scaler2.max);
}

#[test]
fn min_max_scaler_empty_values() {
    let mut scaler = MinMaxScaler::new();
    scaler.fit(&[]);

    assert_eq!(scaler.min, f32::INFINITY);
    assert_eq!(scaler.max, f32::NEG_INFINITY);

    assert_eq!(scaler.transform(5.0), 5.0);
    assert_eq!(scaler.transform(-5.0), -5.0);
}

#[test]
fn robust_scaler_fit_and_transform() {
    let mut scaler = RobustScaler::new();
    // Values: 1, 2, 3, 4, 5, 6, 7, 8, 9, 10
    scaler.fit(&[10.0, 9.0, 8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0]);

    // Sorted: 1, 2, 3, 4, 5, 6, 7, 8, 9, 10
    // Median = (5 + 6) / 2 = 5.5
    // Q1 (index 2) = 3
    // Q3 (index 7) = 8
    // IQR = 8 - 3 = 5
    assert_eq!(scaler.median, 5.5);
    assert_eq!(scaler.q1, 3.0);
    assert_eq!(scaler.q3, 8.0);
    assert_eq!(scaler.iqr, 5.0);

    // Transform: (X - median) / IQR
    assert_eq!(scaler.transform(1.0), -0.9);
    assert_eq!(scaler.transform(5.5), 0.0);
    assert_eq!(scaler.transform(10.0), 0.9);
}

#[test]
fn robust_scaler_robust_to_outliers() {
    let mut scaler = RobustScaler::new();
    // Values with outliers: 1, 2, 3, 4, 5, 100, 1000
    scaler.fit(&[1.0, 2.0, 3.0, 4.0, 5.0, 100.0, 1000.0]);

    // Sorted: 1, 2, 3, 4, 5, 100, 1000
    // n = 7, median index = 3, median = 4
    // Q1 index = 1, Q1 = 2
    // Q3 index = 5, Q3 = 100
    // IQR = 100 - 2 = 98
    assert_eq!(scaler.median, 4.0);
    assert_eq!(scaler.q1, 2.0);
    assert_eq!(scaler.q3, 100.0);
    assert_eq!(scaler.iqr, 98.0);
}

#[test]
fn robust_scaler_constant_values() {
    let mut scaler = RobustScaler::new();
    scaler.fit(&[5.0, 5.0, 5.0, 5.0, 5.0]);

    assert_eq!(scaler.median, 5.0);
    assert_eq!(scaler.q1, 5.0);
    assert_eq!(scaler.q3, 5.0);
    assert_eq!(scaler.iqr, 1.0); // Avoid division by zero
}

#[test]
fn robust_scaler_marshal_and_unmarshal() {
    let mut scaler = RobustScaler::new();
    scaler.fit(&[1.0, 5.0, 10.0, 15.0, 20.0]);

    // Marshal
    let mut buf = Vec::new();
    scaler.marshal(&mut buf).expect("marshal");

    // Unmarshal
    let mut scaler2 = RobustScaler::new();
    scaler2.unmarshal(&mut buf.as_slice()).expect("unmarshal");

    assert_eq!(scaler.median, scaler2.median);
    assert_eq!(scaler.q1, scaler2.q1);
    assert_eq!(scaler.q3, scaler2.q3);
    assert_eq!(scaler.iqr, scaler2.iqr);

    // Verify transform gives same result
    assert_eq!(scaler.transform(7.0), scaler2.transform(7.0));
}

#[test]
fn auto_scaler_non_negative_values_use_log1p_and_min_max() {
    let mut scaler = AutoScaler::new();
    scaler.fit(&[1.0, 10.0, 100.0, 1000.0]);

    assert!(scaler.use_log);

    // Verify log transformation
    // log1p(1) ≈ 0.693, log1p(1000) ≈ 6.909
    let min_log = log1p(1.0);
    let max_log = log1p(1000.0);

    assert_eq!(scaler.transform(1.0), 0.0);
    assert_eq!(scaler.transform(1000.0), 1.0);

    // Verify intermediate values
    let expected_val = (log1p(100.0) - min_log) / (max_log - min_log);
    assert_eq!(scaler.transform(100.0), expected_val);
}

#[test]
fn auto_scaler_negative_values_use_robust_scaler() {
    let mut scaler = AutoScaler::new();
    scaler.fit(&[-10.0, -5.0, 0.0, 5.0, 10.0]);

    assert!(!scaler.use_log);

    // Should use RobustScaler
    // median = 0, Q1 = -5, Q3 = 5, IQR = 10
    assert_eq!(scaler.robust.median, 0.0);
    assert_eq!(scaler.robust.iqr, 10.0);

    // MinMaxScaler boundaries: -1 to 1 based on robust transform of -10 to 10
    assert_eq!(scaler.min_max.min, -1.0);
    assert_eq!(scaler.min_max.max, 1.0);

    assert_eq!(scaler.transform(0.0), 0.5);
    assert_eq!(scaler.transform(10.0), 1.0);
    assert_eq!(scaler.transform(-10.0), 0.0);
}

#[test]
fn auto_scaler_mixed_values_with_single_negative() {
    let mut scaler = AutoScaler::new();
    scaler.fit(&[-1.0, 10.0, 100.0, 1000.0]);

    assert!(!scaler.use_log);
}

#[test]
fn auto_scaler_marshal_and_unmarshal_with_log() {
    let mut scaler = AutoScaler::new();
    scaler.fit(&[0.0, 100.0, 1000.0]);

    assert!(scaler.use_log);

    // Marshal
    let mut buf = Vec::new();
    scaler.marshal(&mut buf).expect("marshal");

    // Unmarshal
    let mut scaler2 = AutoScaler::new();
    scaler2.unmarshal(&mut buf.as_slice()).expect("unmarshal");

    assert_eq!(scaler.use_log, scaler2.use_log);
    assert_eq!(scaler.min_max.min, scaler2.min_max.min);
    assert_eq!(scaler.min_max.max, scaler2.min_max.max);

    // Verify transform gives same result
    assert_eq!(scaler.transform(500.0), scaler2.transform(500.0));
}

#[test]
fn auto_scaler_marshal_and_unmarshal_with_robust() {
    let mut scaler = AutoScaler::new();
    scaler.fit(&[-100.0, -50.0, 0.0, 50.0, 100.0]);

    assert!(!scaler.use_log);

    // Marshal
    let mut buf = Vec::new();
    scaler.marshal(&mut buf).expect("marshal");

    // Unmarshal
    let mut scaler2 = AutoScaler::new();
    scaler2.unmarshal(&mut buf.as_slice()).expect("unmarshal");

    assert_eq!(scaler.use_log, scaler2.use_log);
    assert_eq!(scaler.robust.median, scaler2.robust.median);
    assert_eq!(scaler.robust.iqr, scaler2.robust.iqr);
    assert_eq!(scaler.min_max.min, scaler2.min_max.min);
    assert_eq!(scaler.min_max.max, scaler2.min_max.max);

    // Verify transform gives same result
    assert_eq!(scaler.transform(25.0), scaler2.transform(25.0));
}
