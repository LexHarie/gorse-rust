// Ports: master/metrics.go @ add21b5

//! The Prometheus metrics of the master, with the names and label names of
//! Go's `master/metrics.go`, and the online evaluator that feeds the positive
//! feedback ratio. Each metric is registered in the default registry the first
//! time it is used, and [`register`] registers them all at start-up, as Go's
//! `promauto` does.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use gorse_common::expression::{FeedbackTypeExpression, match_feedback_type_expressions};
use gorse_common::gotime::{HOUR, Duration, Time};
use gorse_storage::cache::{self, TimeSeriesPoint};
use prometheus::{Gauge, GaugeVec, Opts};

/// The label that names a dataset step (Go `LabelStep`).
pub const LABEL_STEP: &str = "step";
/// The label that names a feedback type (Go `LabelFeedbackType`).
pub const LABEL_FEEDBACK_TYPE: &str = "feedback_type";
/// The label that names a data set in the memory gauge (Go `LabelData`).
pub const LABEL_DATA: &str = "data";

/// The namespace every master metric lives in.
const NAMESPACE: &str = "gorse";
/// The subsystem every master metric lives in.
const SUBSYSTEM: &str = "master";

/// Builds the options of a metric with `name` in the master's namespace.
fn opts(name: &str) -> Opts {
    Opts::new(name, name)
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM)
}

/// Builds and registers a gauge. Panics when the definition is invalid or the
/// name is taken, as Go's `promauto` does.
fn gauge(name: &str) -> Gauge {
    let gauge = Gauge::with_opts(opts(name)).expect("a valid gauge definition");
    prometheus::register(Box::new(gauge.clone())).expect("register the gauge once");
    gauge
}

/// Builds and registers a gauge vector with one label. Panics when the
/// definition is invalid or the name is taken.
fn gauge_vec(name: &str, label: &str) -> GaugeVec {
    let gauge = GaugeVec::new(opts(name), &[label]).expect("a valid gauge vector definition");
    prometheus::register(Box::new(gauge.clone())).expect("register the gauge vector once");
    gauge
}

/// Go's `LoadDatasetStepSecondsVec`: the seconds each dataset step took.
pub static LOAD_DATASET_STEP_SECONDS_VEC: LazyLock<GaugeVec> =
    LazyLock::new(|| gauge_vec("load_dataset_step_seconds", LABEL_STEP));
/// Go's `LoadDatasetTotalSeconds`.
pub static LOAD_DATASET_TOTAL_SECONDS: LazyLock<Gauge> =
    LazyLock::new(|| gauge("load_dataset_total_seconds"));
/// Go's `FindUserNeighborsSecondsVec`.
pub static FIND_USER_NEIGHBORS_SECONDS_VEC: LazyLock<GaugeVec> =
    LazyLock::new(|| gauge_vec("find_user_neighbors_seconds", LABEL_STEP));
/// Go's `FindUserNeighborsTotalSeconds`.
pub static FIND_USER_NEIGHBORS_TOTAL_SECONDS: LazyLock<Gauge> =
    LazyLock::new(|| gauge("find_user_neighbors_total_seconds"));
/// Go's `FindItemNeighborsSecondsVec`.
pub static FIND_ITEM_NEIGHBORS_SECONDS_VEC: LazyLock<GaugeVec> =
    LazyLock::new(|| gauge_vec("find_item_neighbors_seconds", LABEL_STEP));
/// Go's `FindItemNeighborsTotalSeconds`.
pub static FIND_ITEM_NEIGHBORS_TOTAL_SECONDS: LazyLock<Gauge> =
    LazyLock::new(|| gauge("find_item_neighbors_total_seconds"));
/// Go's `UpdateUserNeighborsTotal`.
pub static UPDATE_USER_NEIGHBORS_TOTAL: LazyLock<Gauge> =
    LazyLock::new(|| gauge("update_user_neighbors_total"));
/// Go's `UpdateItemNeighborsTotal`.
pub static UPDATE_ITEM_NEIGHBORS_TOTAL: LazyLock<Gauge> =
    LazyLock::new(|| gauge("update_item_neighbors_total"));
/// Go's `CacheScannedTotal`.
pub static CACHE_SCANNED_TOTAL: LazyLock<Gauge> = LazyLock::new(|| gauge("cache_scanned_total"));
/// Go's `CacheReclaimedTotal`.
pub static CACHE_RECLAIMED_TOTAL: LazyLock<Gauge> =
    LazyLock::new(|| gauge("cache_reclaimed_total"));
/// Go's `CacheScannedSeconds`.
pub static CACHE_SCANNED_SECONDS: LazyLock<Gauge> =
    LazyLock::new(|| gauge("cache_scanned_seconds"));
/// Go's `CollaborativeFilteringFitSeconds`.
pub static COLLABORATIVE_FILTERING_FIT_SECONDS: LazyLock<Gauge> =
    LazyLock::new(|| gauge("collaborative_filtering_fit_seconds"));
/// Go's `CollaborativeFilteringSearchSeconds`.
pub static COLLABORATIVE_FILTERING_SEARCH_SECONDS: LazyLock<Gauge> =
    LazyLock::new(|| gauge("collaborative_filtering_search_seconds"));
/// Go's `CollaborativeFilteringNDCG10`.
pub static COLLABORATIVE_FILTERING_NDCG_10: LazyLock<Gauge> =
    LazyLock::new(|| gauge("collaborative_filtering_ndcg_10"));
/// Go's `CollaborativeFilteringPrecision10`.
pub static COLLABORATIVE_FILTERING_PRECISION_10: LazyLock<Gauge> =
    LazyLock::new(|| gauge("collaborative_filtering_precision_10"));
/// Go's `CollaborativeFilteringRecall10`.
pub static COLLABORATIVE_FILTERING_RECALL_10: LazyLock<Gauge> =
    LazyLock::new(|| gauge("collaborative_filtering_recall_10"));
/// Go's `CollaborativeFilteringSearchPrecision10`.
pub static COLLABORATIVE_FILTERING_SEARCH_PRECISION_10: LazyLock<Gauge> =
    LazyLock::new(|| gauge("collaborative_filtering_search_precision_10"));
/// Go's `RankingFitSeconds`.
pub static RANKING_FIT_SECONDS: LazyLock<Gauge> = LazyLock::new(|| gauge("ranking_fit_seconds"));
/// Go's `RankingSearchSeconds`.
pub static RANKING_SEARCH_SECONDS: LazyLock<Gauge> =
    LazyLock::new(|| gauge("ranking_search_seconds"));
/// Go's `RankingPrecision`.
pub static RANKING_PRECISION: LazyLock<Gauge> =
    LazyLock::new(|| gauge("ranking_model_precision"));
/// Go's `RankingRecall`.
pub static RANKING_RECALL: LazyLock<Gauge> = LazyLock::new(|| gauge("ranking_model_recall"));
/// Go's `RankingAUC`.
pub static RANKING_AUC: LazyLock<Gauge> = LazyLock::new(|| gauge("ranking_model_auc"));
/// Go's `RankingSearchPrecision`.
pub static RANKING_SEARCH_PRECISION: LazyLock<Gauge> =
    LazyLock::new(|| gauge("ranking_search_precision"));
/// Go's `UsersTotal`.
pub static USERS_TOTAL: LazyLock<Gauge> = LazyLock::new(|| gauge("users_total"));
/// Go's `ActiveUsersTotal`.
pub static ACTIVE_USERS_TOTAL: LazyLock<Gauge> = LazyLock::new(|| gauge("active_users_total"));
/// Go's `InactiveUsersTotal`.
pub static INACTIVE_USERS_TOTAL: LazyLock<Gauge> =
    LazyLock::new(|| gauge("inactive_users_total"));
/// Go's `ItemsTotal`.
pub static ITEMS_TOTAL: LazyLock<Gauge> = LazyLock::new(|| gauge("items_total"));
/// Go's `ActiveItemsTotal`.
pub static ACTIVE_ITEMS_TOTAL: LazyLock<Gauge> = LazyLock::new(|| gauge("active_items_total"));
/// Go's `InactiveItemsTotal`.
pub static INACTIVE_ITEMS_TOTAL: LazyLock<Gauge> =
    LazyLock::new(|| gauge("inactive_items_total"));
/// Go's `UserLabelsTotal`.
pub static USER_LABELS_TOTAL: LazyLock<Gauge> = LazyLock::new(|| gauge("user_labels_total"));
/// Go's `ItemLabelsTotal`.
pub static ITEM_LABELS_TOTAL: LazyLock<Gauge> = LazyLock::new(|| gauge("item_labels_total"));
/// Go's `FeedbacksTotal`.
pub static FEEDBACKS_TOTAL: LazyLock<Gauge> = LazyLock::new(|| gauge("feedbacks_total"));
/// Go's `ImplicitFeedbacksTotal`.
pub static IMPLICIT_FEEDBACKS_TOTAL: LazyLock<Gauge> =
    LazyLock::new(|| gauge("implicit_feedbacks_total"));
/// Go's `PositiveFeedbacksTotal`.
pub static POSITIVE_FEEDBACKS_TOTAL: LazyLock<Gauge> =
    LazyLock::new(|| gauge("positive_feedbacks_total"));
/// Go's `NegativeFeedbackTotal`.
pub static NEGATIVE_FEEDBACK_TOTAL: LazyLock<Gauge> =
    LazyLock::new(|| gauge("negative_feedbacks_total"));
/// Go's `MemoryInUseBytesVec`: the bytes in use, by data set.
pub static MEMORY_IN_USE_BYTES_VEC: LazyLock<GaugeVec> =
    LazyLock::new(|| gauge_vec("memory_inuse_bytes", LABEL_DATA));

/// Registers every master metric now. Go's `promauto` registers them when the
/// package starts, so `/metrics` lists them before any task runs.
pub fn register() {
    LazyLock::force(&LOAD_DATASET_STEP_SECONDS_VEC);
    LazyLock::force(&LOAD_DATASET_TOTAL_SECONDS);
    LazyLock::force(&FIND_USER_NEIGHBORS_SECONDS_VEC);
    LazyLock::force(&FIND_USER_NEIGHBORS_TOTAL_SECONDS);
    LazyLock::force(&FIND_ITEM_NEIGHBORS_SECONDS_VEC);
    LazyLock::force(&FIND_ITEM_NEIGHBORS_TOTAL_SECONDS);
    LazyLock::force(&UPDATE_USER_NEIGHBORS_TOTAL);
    LazyLock::force(&UPDATE_ITEM_NEIGHBORS_TOTAL);
    LazyLock::force(&CACHE_SCANNED_TOTAL);
    LazyLock::force(&CACHE_RECLAIMED_TOTAL);
    LazyLock::force(&CACHE_SCANNED_SECONDS);
    LazyLock::force(&COLLABORATIVE_FILTERING_FIT_SECONDS);
    LazyLock::force(&COLLABORATIVE_FILTERING_SEARCH_SECONDS);
    LazyLock::force(&COLLABORATIVE_FILTERING_NDCG_10);
    LazyLock::force(&COLLABORATIVE_FILTERING_PRECISION_10);
    LazyLock::force(&COLLABORATIVE_FILTERING_RECALL_10);
    LazyLock::force(&COLLABORATIVE_FILTERING_SEARCH_PRECISION_10);
    LazyLock::force(&RANKING_FIT_SECONDS);
    LazyLock::force(&RANKING_SEARCH_SECONDS);
    LazyLock::force(&RANKING_PRECISION);
    LazyLock::force(&RANKING_RECALL);
    LazyLock::force(&RANKING_AUC);
    LazyLock::force(&RANKING_SEARCH_PRECISION);
    LazyLock::force(&USERS_TOTAL);
    LazyLock::force(&ACTIVE_USERS_TOTAL);
    LazyLock::force(&INACTIVE_USERS_TOTAL);
    LazyLock::force(&ITEMS_TOTAL);
    LazyLock::force(&ACTIVE_ITEMS_TOTAL);
    LazyLock::force(&INACTIVE_ITEMS_TOTAL);
    LazyLock::force(&USER_LABELS_TOTAL);
    LazyLock::force(&ITEM_LABELS_TOTAL);
    LazyLock::force(&FEEDBACKS_TOTAL);
    LazyLock::force(&IMPLICIT_FEEDBACKS_TOTAL);
    LazyLock::force(&POSITIVE_FEEDBACKS_TOTAL);
    LazyLock::force(&NEGATIVE_FEEDBACK_TOTAL);
    LazyLock::force(&MEMORY_IN_USE_BYTES_VEC);
}

/// Go's `OnlineEvaluator`: the positive feedback ratio of the users who read
/// items in each of the last `window_size` days.
#[derive(Debug)]
pub struct OnlineEvaluator {
    /// The feedback types that count as reads (Go's `ReadTypes`).
    pub read_types: Vec<FeedbackTypeExpression>,
    /// The items each user read, by day (Go's `ReadFeedback`).
    pub read_feedback: Vec<HashMap<i32, HashSet<i32>>>,
    /// The feedback types that count as positive (Go's `PositiveTypes`).
    pub positive_types: Vec<FeedbackTypeExpression>,
    /// The items each user gave positive feedback to, by expression name. The
    /// empty name holds every positive feedback (Go's `PositiveFeedback`).
    pub positive_feedback: HashMap<String, HashMap<i32, HashSet<i32>>>,
    /// The number of days the evaluator keeps (Go's `WindowSize`).
    pub window_size: usize,
    /// The day the window ends, which is the first day of the window (Go's `WindowEnd`).
    pub window_end: Time,
}

/// Go's `NewOnlineEvaluator`.
pub fn new_online_evaluator(
    positive_types: Vec<FeedbackTypeExpression>,
    read_types: Vec<FeedbackTypeExpression>,
) -> OnlineEvaluator {
    let window_size = 30;
    let mut positive_feedback = HashMap::new();
    positive_feedback.insert(String::new(), HashMap::new());
    OnlineEvaluator {
        read_types,
        read_feedback: (0..window_size).map(|_| HashMap::new()).collect(),
        positive_types,
        positive_feedback,
        window_size,
        window_end: gorse_common::gotime::now().truncate(Duration(24 * HOUR.0)),
    }
}

impl OnlineEvaluator {
    /// Go's `Add`: records one feedback. A read counts toward the day of its
    /// timestamp, when the day is inside the window. A positive feedback counts
    /// toward each positive expression it matches, and toward the empty name.
    pub fn add(
        &mut self,
        feedback_type: &str,
        value: f64,
        user_index: i32,
        item_index: i32,
        timestamp: Time,
    ) {
        if match_feedback_type_expressions(&self.read_types, feedback_type, value) {
            let truncated = timestamp.truncate(Duration(24 * HOUR.0));
            let days = self.window_end.sub(&truncated).nanoseconds()
                / HOUR.nanoseconds()
                / 24;
            if days >= 0 && (days as usize) < self.window_size {
                self.read_feedback[days as usize]
                    .entry(user_index)
                    .or_default()
                    .insert(item_index);
            }
        }
        let mut is_positive = false;
        for positive_type in &self.positive_types {
            if !positive_type.matches(feedback_type, value) {
                continue;
            }
            is_positive = true;
            self.positive_feedback
                .entry(positive_type.to_string())
                .or_default()
                .entry(user_index)
                .or_default()
                .insert(item_index);
        }
        if is_positive {
            self.positive_feedback
                .entry(String::new())
                .or_default()
                .entry(user_index)
                .or_default()
                .insert(item_index);
        }
    }

    /// Go's `Evaluate`: the positive feedback ratio of each day and each
    /// positive expression, over the users who read in that day. A day with no
    /// reader has no point.
    pub fn evaluate(&self) -> Vec<TimeSeriesPoint> {
        let mut points = Vec::new();
        for (feedback_type, positives) in &self.positive_feedback {
            for i in 0..self.window_size {
                let date = self.window_end.add_date(0, 0, -(i as i64));
                let mut ratio_sum = 0.0_f64;
                let mut user_count = 0_usize;
                for (user_index, read_items) in &self.read_feedback[i] {
                    let Some(positive_items) = positives.get(user_index) else {
                        continue;
                    };
                    let positive_count = read_items.intersection(positive_items).count() as f64;
                    if !read_items.is_empty() {
                        ratio_sum += positive_count / read_items.len() as f64;
                        user_count += 1;
                    }
                }
                if user_count > 0 {
                    let mut name = cache::POSITIVE_FEEDBACK_RATIO.to_owned();
                    if !feedback_type.is_empty() {
                        name.push('_');
                        name.push_str(feedback_type);
                    }
                    points.push(TimeSeriesPoint {
                        name,
                        timestamp: date,
                        value: ratio_sum / user_count as f64,
                    });
                }
            }
        }
        points
    }
}

#[cfg(test)]
mod tests;
