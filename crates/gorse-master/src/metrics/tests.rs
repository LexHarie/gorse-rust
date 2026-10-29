// Ports: master/metrics_test.go @ add21b5

use std::collections::HashSet;

use gorse_common::expression::must_parse_feedback_type_expression;
use gorse_common::gotime::{self, Month};
use gorse_storage::cache::TimeSeriesPoint;
use prometheus::core::Collector;

use super::*;

/// The metric name, the namespace, the subsystem and the Go file's `Name:` must
/// agree. A rename in either place fails here.
#[test]
fn metric_names_match_go() {
    let go = include_str!("../../../../master/metrics.go");
    let go_names: HashSet<String> = go
        .lines()
        .filter_map(|line| {
            let rest = line.split("Name:").nth(1)?;
            let value = rest.trim().strip_prefix('"')?;
            Some(value.split('"').next()?.to_owned())
        })
        .collect();
    let collectors: Vec<&dyn Collector> = vec![
        &*LOAD_DATASET_STEP_SECONDS_VEC,
        &*LOAD_DATASET_TOTAL_SECONDS,
        &*FIND_USER_NEIGHBORS_SECONDS_VEC,
        &*FIND_USER_NEIGHBORS_TOTAL_SECONDS,
        &*FIND_ITEM_NEIGHBORS_SECONDS_VEC,
        &*FIND_ITEM_NEIGHBORS_TOTAL_SECONDS,
        &*UPDATE_USER_NEIGHBORS_TOTAL,
        &*UPDATE_ITEM_NEIGHBORS_TOTAL,
        &*CACHE_SCANNED_TOTAL,
        &*CACHE_RECLAIMED_TOTAL,
        &*CACHE_SCANNED_SECONDS,
        &*COLLABORATIVE_FILTERING_FIT_SECONDS,
        &*COLLABORATIVE_FILTERING_SEARCH_SECONDS,
        &*COLLABORATIVE_FILTERING_NDCG_10,
        &*COLLABORATIVE_FILTERING_PRECISION_10,
        &*COLLABORATIVE_FILTERING_RECALL_10,
        &*COLLABORATIVE_FILTERING_SEARCH_PRECISION_10,
        &*RANKING_FIT_SECONDS,
        &*RANKING_SEARCH_SECONDS,
        &*RANKING_PRECISION,
        &*RANKING_RECALL,
        &*RANKING_AUC,
        &*RANKING_SEARCH_PRECISION,
        &*USERS_TOTAL,
        &*ACTIVE_USERS_TOTAL,
        &*INACTIVE_USERS_TOTAL,
        &*ITEMS_TOTAL,
        &*ACTIVE_ITEMS_TOTAL,
        &*INACTIVE_ITEMS_TOTAL,
        &*USER_LABELS_TOTAL,
        &*ITEM_LABELS_TOTAL,
        &*FEEDBACKS_TOTAL,
        &*IMPLICIT_FEEDBACKS_TOTAL,
        &*POSITIVE_FEEDBACKS_TOTAL,
        &*NEGATIVE_FEEDBACK_TOTAL,
        &*MEMORY_IN_USE_BYTES_VEC,
    ];
    let mut rust_names = HashSet::new();
    for collector in collectors {
        for desc in collector.desc() {
            assert!(
                rust_names.insert(desc.fq_name.clone()),
                "duplicate metric {}",
                desc.fq_name
            );
        }
    }
    let expected: HashSet<String> = go_names
        .iter()
        .map(|name| format!("gorse_master_{name}"))
        .collect();
    assert_eq!(expected, rust_names);
}

#[test]
fn test_online_evaluator() {
    let positive_types = vec![must_parse_feedback_type_expression("read>=100")];
    let read_types = vec![must_parse_feedback_type_expression("read")];

    let evaluator = new_online_evaluator(positive_types.clone(), read_types.clone());
    assert!(evaluator.evaluate().is_empty());

    let mut evaluator = new_online_evaluator(positive_types, read_types);
    let utc = gotime::utc();
    evaluator.window_end = gotime::date(2005, Month(6), 16, 0, 0, 0, 0, &utc);
    evaluator.window_size = 2;
    let d15 = gotime::date(2005, Month(6), 15, 0, 0, 0, 0, &utc);
    let d16 = gotime::date(2005, Month(6), 16, 0, 0, 0, 0, &utc);
    evaluator.add("read", 0.0, 1, 1, d15.clone());
    evaluator.add("read", 0.0, 1, 2, d15.clone());
    evaluator.add("read", 0.0, 1, 3, d15.clone());
    evaluator.add("read", 100.0, 1, 4, d15.clone());
    evaluator.add("read", 100.0, 2, 1, d15.clone());
    evaluator.add("read", 0.0, 2, 2, d15.clone());
    evaluator.add("read", 0.0, 2, 3, d15.clone());
    evaluator.add("read", 0.0, 2, 4, d15.clone());
    evaluator.add("read", 0.0, 2, 3, d16.clone());
    evaluator.add("read", 100.0, 3, 3, d16.clone());

    let result = evaluator.evaluate();
    let expected = vec![
        TimeSeriesPoint {
            name: "positive_feedback_ratio_read>=100".to_owned(),
            timestamp: d16.clone(),
            value: 0.5,
        },
        TimeSeriesPoint {
            name: "positive_feedback_ratio_read>=100".to_owned(),
            timestamp: d15.clone(),
            value: 0.25,
        },
        TimeSeriesPoint {
            name: "positive_feedback_ratio".to_owned(),
            timestamp: d16,
            value: 0.5,
        },
        TimeSeriesPoint {
            name: "positive_feedback_ratio".to_owned(),
            timestamp: d15,
            value: 0.25,
        },
    ];
    assert_eq!(expected.len(), result.len());
    for point in &expected {
        assert!(
            result
                .iter()
                .any(|p| p.name == point.name && p.timestamp == point.timestamp && p.value == point.value),
            "missing point {} {:?}",
            point.name,
            point.timestamp
        );
    }
}
