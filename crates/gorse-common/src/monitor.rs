// Ports: common/monitor/progress.go @ add21b5

//! Progress of long-running work: a tracer holds named root spans, spans nest
//! through the context, and [`Span::progress`] folds the running children into
//! their parent's count.
//!
//! Go keeps spans in `sync.Map`s and updates them without locks, with `count`
//! and `status` read and written by different goroutines. Here each span
//! guards its mutable fields with a mutex, and the maps with mutexes, so the
//! same sequence of calls gives the same results without data races.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use gorse_protocol::{Progress as PbProgress, PushProgressRequest};

use crate::gocontext::Context;
use crate::gotime::{self, Time};

/// The state of a span: Go's `Status` string. Strings this module does not
/// name (possible from another node's protocol message) are kept as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// `Pending`: not started.
    Pending,
    /// `Complete`: finished.
    Complete,
    /// `Running`: in progress.
    Running,
    /// `Suspended`: paused.
    Suspended,
    /// `Failed`: finished with an error.
    Failed,
    /// Any other status string.
    Other(String),
}

impl Status {
    /// The status as Go writes it on the wire.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "Pending",
            Self::Complete => "Complete",
            Self::Running => "Running",
            Self::Suspended => "Suspended",
            Self::Failed => "Failed",
            Self::Other(text) => text,
        }
    }
}

impl From<&str> for Status {
    fn from(text: &str) -> Self {
        match text {
            "Pending" => Self::Pending,
            "Complete" => Self::Complete,
            "Running" => Self::Running,
            "Suspended" => Self::Suspended,
            "Failed" => Self::Failed,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// The root span registered with a tracer, carried by the context so that
/// [`start`] can nest under it.
struct SpanRef(Arc<Span>);

/// Go's `Monitor`: the root spans of one named tracer.
pub struct Monitor {
    name: String,
    spans: Mutex<HashMap<String, Arc<Span>>>,
}

/// Go's `NewTracer`.
pub fn new_tracer(name: &str) -> Monitor {
    Monitor {
        name: name.to_owned(),
        spans: Mutex::new(HashMap::new()),
    }
}

impl Monitor {
    /// Go's `Monitor.Start`: creates a root span, registered under `name` (a
    /// later span with the same name replaces it), and returns a context that
    /// carries it.
    pub fn start(&self, ctx: &Context, name: &str, total: i64) -> (Context, Arc<Span>) {
        let span = Arc::new(Span::new(name, total));
        lock(&self.spans).insert(name.to_owned(), Arc::clone(&span));
        (ctx.with_value(SpanRef(Arc::clone(&span))), span)
    }

    /// Go's `Monitor.List`: the progress of every root span, oldest first.
    pub fn list(&self) -> Vec<Progress> {
        let spans: Vec<Arc<Span>> = lock(&self.spans).values().cloned().collect();
        let mut progress: Vec<Progress> = spans
            .iter()
            .map(|span| {
                let mut p = span.progress();
                p.tracer = self.name.clone();
                p
            })
            .collect();
        // Go's `sort.Slice` is not stable; equal start times may come out in any order.
        progress.sort_by(|a, b| {
            if a.start_time.before(&b.start_time) {
                std::cmp::Ordering::Less
            } else if b.start_time.before(&a.start_time) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        progress
    }
}

/// One unit of tracked work: Go's `Span`.
pub struct Span {
    name: String,
    total: i64,
    start: Time,
    state: Mutex<SpanState>,
    children: Mutex<HashMap<String, Arc<Span>>>,
}

struct SpanState {
    status: Status,
    count: i64,
    err: String,
    finish: Time,
}

impl Span {
    fn new(name: &str, total: i64) -> Self {
        Self {
            name: name.to_owned(),
            total,
            start: gotime::now(),
            state: Mutex::new(SpanState {
                status: Status::Running,
                count: 0,
                err: String::new(),
                finish: Time::default(),
            }),
            children: Mutex::new(HashMap::new()),
        }
    }

    /// Go's `Span.Add`: advances the count by `n`, never past the total.
    pub fn add(&self, n: i64) {
        let mut state = lock(&self.state);
        state.count = (state.count + n).min(self.total);
    }

    /// Go's `Span.End`: a running span completes, with its count at the total.
    pub fn end(&self) {
        let mut state = lock(&self.state);
        if state.status == Status::Running {
            state.status = Status::Complete;
            state.count = self.total;
            state.finish = gotime::now();
        }
    }

    /// Go's `Span.Fail`: marks the span failed with the error's text.
    pub fn fail(&self, err: &dyn std::error::Error) {
        let mut state = lock(&self.state);
        state.status = Status::Failed;
        state.err = err.to_string();
    }

    /// Go's `Span.Count`.
    pub fn count(&self) -> i64 {
        lock(&self.state).count
    }

    /// Go's `Span.Progress`: this span's progress, with the running children
    /// folded into its count. A child that failed fails this span too.
    pub fn progress(&self) -> Progress {
        // Go ranges over the children map while it is written to; this
        // snapshot holds the map's lock only while copying the entries.
        let children: Vec<Arc<Span>> = lock(&self.children).values().cloned().collect();
        let mut running = Vec::new();
        for child in &children {
            let progress = child.progress();
            if progress.status == Status::Running {
                running.push(progress.clone());
            }
            let mut state = lock(&self.state);
            if state.err.is_empty() && !progress.error.is_empty() {
                state.err = progress.error;
                state.status = Status::Failed;
            }
        }

        // A leaf: no running children.
        if running.is_empty() {
            let count = lock(&self.state).count;
            return self.snapshot(self.total, count);
        }

        // Each running child counts for `child_total` units of the parent. The
        // largest child total sets that unit, so the result does not depend on
        // map order; with no positive total the unit is one.
        let mut child_total = running.iter().map(|c| c.total).max().unwrap_or(0);
        if child_total == 0 {
            child_total = 1;
        }
        let parent_total = self.total * child_total;
        let mut parent_count = lock(&self.state).count * child_total;
        for child in &running {
            // A child with no work is complete.
            if child.total == 0 {
                parent_count += child_total;
                continue;
            }
            parent_count += child_total * child.count / child.total;
        }
        self.snapshot(parent_total, parent_count)
    }

    fn snapshot(&self, total: i64, count: i64) -> Progress {
        let state = lock(&self.state);
        Progress {
            tracer: String::new(),
            name: self.name.clone(),
            status: state.status.clone(),
            error: state.err.clone(),
            count,
            total,
            start_time: self.start.clone(),
            finish_time: state.finish.clone(),
        }
    }
}

/// Go's `monitor.Start`: a child of the span in `ctx`, if there is one.
/// Without a span in `ctx` the new span is a root that no tracer lists.
pub fn start(ctx: &Context, name: &str, total: i64) -> (Context, Arc<Span>) {
    let child = Arc::new(Span::new(name, total));
    match ctx.value::<SpanRef>() {
        Some(parent) => {
            lock(&parent.0.children).insert(name.to_owned(), Arc::clone(&child));
            (ctx.with_value(SpanRef(Arc::clone(&child))), child)
        }
        None => (ctx.clone(), child),
    }
}

/// Go's `monitor.Fail`: marks the span in `ctx` failed; does nothing without one.
pub fn fail(ctx: &Context, err: &dyn std::error::Error) {
    if let Some(span) = ctx.value::<SpanRef>() {
        span.0.fail(err);
    }
}

/// One span's progress, as tracers report it. Go's `monitor.Progress`.
#[derive(Debug, Clone)]
pub struct Progress {
    /// The name of the tracer that owns the span.
    pub tracer: String,
    /// The span's name.
    pub name: String,
    /// The span's status.
    pub status: Status,
    /// The error text, when the span or one of its children failed.
    pub error: String,
    /// Work done.
    pub count: i64,
    /// Work to do.
    pub total: i64,
    /// When the span started.
    pub start_time: Time,
    /// When the span finished, or the zero time.
    pub finish_time: Time,
}

/// Go's `monitor.DecodeProgress`: the progress list of a `PushProgress` message.
pub fn decode_progress(input: &PushProgressRequest) -> Vec<Progress> {
    input
        .progress
        .iter()
        .map(|p| Progress {
            tracer: p.tracer.clone(),
            name: p.name.clone(),
            status: Status::from(p.status.as_str()),
            error: p.error.clone(),
            count: p.count,
            total: p.total,
            start_time: gotime::unix_milli(p.start_time),
            finish_time: gotime::unix_milli(p.finish_time),
        })
        .collect()
}

/// Go's `monitor.EncodeProgress`: the `PushProgress` message for a progress list.
pub fn encode_progress(progress: &[Progress]) -> PushProgressRequest {
    PushProgressRequest {
        progress: progress
            .iter()
            .map(|p| PbProgress {
                tracer: p.tracer.clone(),
                name: p.name.clone(),
                status: p.status.as_str().to_owned(),
                error: p.error.clone(),
                count: p.count,
                total: p.total,
                start_time: p.start_time.unix_milli(),
                finish_time: p.finish_time.unix_milli(),
            })
            .collect(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    // Ports: common/monitor/progress_test.go @ add21b5

    use super::*;

    fn span(name: &str, total: i64, count: i64) -> Arc<Span> {
        let span = Span::new(name, total);
        span.add(count);
        Arc::new(span)
    }

    fn date(day: i64) -> Time {
        gotime::date(2018, gotime::Month(1), day, 0, 0, 0, 0, &gotime::local())
    }

    #[test]
    fn encode_decode() {
        let progress_list = vec![
            Progress {
                tracer: "tracer".into(),
                name: "a".into(),
                status: Status::Running,
                error: String::new(),
                count: 50,
                total: 100,
                start_time: date(1),
                finish_time: date(2),
            },
            Progress {
                tracer: "tracer".into(),
                name: "b".into(),
                status: Status::Running,
                error: String::new(),
                count: 50,
                total: 100,
                start_time: date(1),
                finish_time: date(2),
            },
        ];
        let decoded = decode_progress(&encode_progress(&progress_list));
        // `Time` has no `==`; compare the instants and the fields.
        assert_eq!(decoded.len(), progress_list.len());
        for (want, got) in progress_list.iter().zip(&decoded) {
            assert_eq!(got.tracer, want.tracer);
            assert_eq!(got.name, want.name);
            assert_eq!(got.status, want.status);
            assert_eq!(got.error, want.error);
            assert_eq!(got.count, want.count);
            assert_eq!(got.total, want.total);
            assert!(got.start_time.equal(&want.start_time));
            assert!(got.finish_time.equal(&want.finish_time));
        }
    }

    #[test]
    fn progress_child_with_zero_total_does_not_divide_by_zero() {
        let parent = span("parent", 10, 5);
        parent
            .children
            .lock()
            .unwrap()
            .insert("child-zero-total".into(), span("child-zero-total", 0, 0));
        let progress = parent.progress();
        assert_eq!(progress.name, "parent");
        assert_eq!(progress.status, Status::Running);
        assert_eq!(progress.count, 6);
        assert_eq!(progress.total, 10);
    }

    #[test]
    fn progress_child_with_zero_total_among_others() {
        let parent = span("parent", 10, 5);
        {
            let mut children = parent.children.lock().unwrap();
            children.insert("child1".into(), span("child1", 100, 50));
            children.insert("child2".into(), span("child2", 0, 0));
        }
        let progress = parent.progress();
        assert_eq!(progress.name, "parent");
        assert_eq!(progress.count, 650);
        assert_eq!(progress.total, 1000);
    }

    #[test]
    fn progress_all_children_with_zero_total() {
        // childTotal = 1, so parentCount = 5 + 1 + 1 and parentTotal = 10.
        let parent = span("parent", 10, 5);
        {
            let mut children = parent.children.lock().unwrap();
            children.insert("child1".into(), span("child1", 0, 0));
            children.insert("child2".into(), span("child2", 0, 0));
        }
        let progress = parent.progress();
        assert_eq!(progress.name, "parent");
        assert_eq!(progress.count, 7);
        assert_eq!(progress.total, 10);
    }

    #[test]
    fn nested_spans_report_through_the_context() {
        let tracer = new_tracer("worker");
        let (ctx, root) = tracer.start(&Context::background(), "fit", 4);
        let (_, child) = start(&ctx, "epoch", 10);
        child.add(5);
        // The running child's 5 of 10 counts as 5 of 40: the root's total 4
        // times the child's total 10, with the child's share of one unit each.
        let progress = tracer.list();
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].tracer, "worker");
        assert_eq!(progress[0].count, 5);
        assert_eq!(progress[0].total, 40);
        child.end();
        root.end();
        // Finished spans are leaves again, and `end` sets the count to the total.
        let progress = tracer.list();
        assert_eq!(progress[0].status, Status::Complete);
        assert_eq!(progress[0].count, 4);
        assert_eq!(progress[0].total, 4);
    }

    #[test]
    fn failing_child_fails_the_parent() {
        let tracer = new_tracer("worker");
        let (ctx, root) = tracer.start(&Context::background(), "fit", 4);
        let (child_ctx, _) = start(&ctx, "epoch", 10);
        fail(&child_ctx, &std::io::Error::other("boom"));
        let _ = root;
        let progress = tracer.list();
        assert_eq!(progress[0].status, Status::Failed);
        assert_eq!(progress[0].error, "boom");
    }

    #[test]
    fn status_round_trips_unknown_strings() {
        assert_eq!(Status::from("Running"), Status::Running);
        assert_eq!(Status::from("Odd").as_str(), "Odd");
    }
}
