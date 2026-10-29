// Ports: worker/metrics.go @ add21b5

//! The Prometheus metrics of the worker, with the names and label names of
//! Go's `worker/metrics.go`. Each is registered in the default registry the
//! first time it is used, as Go's `promauto` registers them at start-up.

use std::sync::LazyLock;

use prometheus::{Gauge, GaugeVec, Opts};

/// The label that names an offline recommendation step (Go `LabelStep`).
pub const LABEL_STEP: &str = "step";
/// The label that names a data set in the memory gauge (Go `LabelData`).
pub const LABEL_DATA: &str = "data";

/// The namespace every worker metric lives in.
const NAMESPACE: &str = "gorse";
/// The subsystem every worker metric lives in.
const SUBSYSTEM: &str = "worker";

/// Go's `UpdateUserRecommendTotal`: the users whose recommendation ran.
pub static UPDATE_USER_RECOMMEND_TOTAL: LazyLock<Gauge> = LazyLock::new(|| {
    let gauge =
        Gauge::with_opts(opts("update_user_recommend_total")).expect("a valid gauge definition");
    prometheus::register(Box::new(gauge.clone())).expect("register the gauge once");
    gauge
});

/// Go's `OfflineRecommendStepSecondsVec`: the seconds each step took, by step.
pub static OFFLINE_RECOMMEND_STEP_SECONDS_VEC: LazyLock<GaugeVec> = LazyLock::new(|| {
    let gauge = GaugeVec::new(opts("offline_recommend_step_seconds"), &[LABEL_STEP])
        .expect("a valid gauge vector definition");
    prometheus::register(Box::new(gauge.clone())).expect("register the gauge vector once");
    gauge
});

/// Go's `OfflineRecommendTotalSeconds`: the seconds the whole run took.
pub static OFFLINE_RECOMMEND_TOTAL_SECONDS: LazyLock<Gauge> = LazyLock::new(|| {
    let gauge = Gauge::with_opts(opts("offline_recommend_total_seconds"))
        .expect("a valid gauge definition");
    prometheus::register(Box::new(gauge.clone())).expect("register the gauge once");
    gauge
});

/// Go's `MemoryInuseBytesVec`: the bytes in use, by data set.
pub static MEMORY_INUSE_BYTES_VEC: LazyLock<GaugeVec> = LazyLock::new(|| {
    let gauge = GaugeVec::new(opts("memory_inuse_bytes"), &[LABEL_DATA])
        .expect("a valid gauge vector definition");
    prometheus::register(Box::new(gauge.clone())).expect("register the gauge vector once");
    gauge
});

/// Registers every worker metric now. Go's `promauto` registers them when the
/// package starts, so `/metrics` lists them before any recommendation runs.
pub fn register() {
    LazyLock::force(&UPDATE_USER_RECOMMEND_TOTAL);
    LazyLock::force(&OFFLINE_RECOMMEND_STEP_SECONDS_VEC);
    LazyLock::force(&OFFLINE_RECOMMEND_TOTAL_SECONDS);
    LazyLock::force(&MEMORY_INUSE_BYTES_VEC);
}

/// The options of a worker metric with the given name.
fn opts(name: &str) -> Opts {
    Opts::new(name, name)
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM)
}
