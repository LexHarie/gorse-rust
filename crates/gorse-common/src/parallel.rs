// Ports: common/parallel/parallel.go @ add21b5

//! Gorse's parallel schedulers: [`parallel()`], [`for_`], [`for_each`],
//! [`detachable`] and [`split`], plus the OpenAI rate limiters
//! (`ratelimit`, re-exported here as in Go's package).
//!
//! Go runs each worker as a goroutine fed by a channel of job ids; here each
//! worker is a scoped thread taking the next job id from a shared counter, so
//! jobs start in the same order and a worker takes a new job only when it is
//! free, as with Go's channel. Contexts are Go's ([`crate::gocontext`]): a
//! scheduler stops handing out jobs once its context is done and returns
//! `context canceled`.
//!
//! Go's `int` counts are `usize` here, so Go's negative counts (which behave
//! like zero, or panic in `make`) cannot be passed.
//!
//! A panicking worker: [`parallel()`] with more than one worker recovers it
//! and logs `panic recovered`, as Go's `util.CheckPanic` does, and the other
//! workers carry on. Everywhere else a panic in Go is not recovered: with one
//! worker it reaches the caller, and in a worker goroutine of `For`,
//! `ForEach` or `Detachable` it crashes the process. Here the other workers
//! stop taking jobs and the panic resumes on the caller once they have
//! returned (`porting/notes/WP-1-6.md`).

mod ratelimit;

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

pub use ratelimit::{
    RateLimiter, Unlimited, chat_completion_backoff, chat_completion_requests_limiter,
    chat_completion_tokens_limiter, embedding_backoff, embedding_requests_limiter,
    embedding_tokens_limiter, init_chat_completion_limiters, init_embedding_limiters,
};

use crate::gocontext::{self, ContextError};
use crate::util::check_panic;

/// `Parallel(ctx, nJobs, nWorkers, worker)`: runs `worker(worker_id, job_id)`
/// for job ids `0..n_jobs` on `n_workers` workers.
///
/// With `n_workers <= 1` the jobs run in order on the calling thread (worker
/// id 0), stopping at the first error, or before a job once `ctx` is done.
/// Otherwise a worker stops at its first error, or when `ctx` is done, and
/// the others carry on; the error of the lowest job id is returned. A
/// panicking worker is logged and stops; its job counts as done without
/// error. When no job failed, the result is `ctx.err()`.
pub fn parallel<E, F>(
    ctx: &gocontext::Context,
    n_jobs: usize,
    n_workers: usize,
    worker: F,
) -> Result<(), E>
where
    E: From<ContextError> + Send,
    F: Fn(usize, usize) -> Result<(), E> + Sync,
{
    if n_workers <= 1 {
        for i in 0..n_jobs {
            if let Some(err) = ctx.err() {
                return Err(err.into());
            }
            worker(0, i)?;
        }
    } else {
        let next = AtomicUsize::new(0);
        let errs = Mutex::new(Vec::new());
        thread::scope(|s| {
            for worker_id in 0..n_workers {
                let (next, errs, worker) = (&next, &errs, &worker);
                s.spawn(move || {
                    check_panic(AssertUnwindSafe(|| {
                        while !ctx.is_done() {
                            let job_id = next.fetch_add(1, Ordering::Relaxed);
                            if job_id >= n_jobs {
                                return;
                            }
                            let result = match ctx.err() {
                                Some(err) => Err(err.into()),
                                // run job
                                None => worker(worker_id, job_id),
                            };
                            if let Err(err) = result {
                                lock(errs).push((job_id, err));
                                return;
                            }
                        }
                    }));
                });
            }
        });
        // check errors
        let errs = errs.into_inner().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, err)) = errs.into_iter().min_by_key(|(job_id, _)| *job_id) {
            return Err(err);
        }
    }
    ctx.err().map_or(Ok(()), |err| Err(err.into()))
}

/// `For(ctx, nJobs, nWorkers, worker)`: runs `worker(job_id)` for job ids
/// `0..n_jobs` on `n_workers` workers (in order on the calling thread when
/// `n_workers <= 1`), handing out no job once `ctx` is done. Returns
/// `ctx.err()`.
pub fn for_<F>(
    ctx: &gocontext::Context,
    n_jobs: usize,
    n_workers: usize,
    worker: F,
) -> Result<(), ContextError>
where
    F: Fn(usize) + Sync,
{
    if n_workers <= 1 {
        for i in 0..n_jobs {
            if let Some(err) = ctx.err() {
                return Err(err);
            }
            worker(i);
        }
    } else {
        let next = AtomicUsize::new(0);
        run_workers(n_workers, |stop| {
            while !ctx.is_done() && !stop.load(Ordering::Relaxed) {
                let job_id = next.fetch_add(1, Ordering::Relaxed);
                if job_id >= n_jobs || ctx.is_done() {
                    return;
                }
                worker(job_id);
            }
        });
    }
    ctx.err().map_or(Ok(()), Err)
}

/// `ForEach(ctx, a, nWorkers, worker)`: [`for_`] over the elements of `a`,
/// calling `worker(index, &element)`. Go copies each element into the
/// worker; here it is borrowed.
pub fn for_each<T, F>(
    ctx: &gocontext::Context,
    a: &[T],
    n_workers: usize,
    worker: F,
) -> Result<(), ContextError>
where
    T: Sync,
    F: Fn(usize, &T) + Sync,
{
    for_(ctx, a.len(), n_workers, |i| worker(i, &a[i]))
}

/// `Split(a, n)`: splits `a` into `min(n, a.len())` consecutive chunks whose
/// sizes differ by at most one, the longer ones first. An empty `a` gives no
/// chunks.
///
/// # Panics
///
/// If `n` is 0 and `a` is not empty, like Go's integer division by zero.
pub fn split<T>(a: &[T], n: usize) -> Vec<&[T]> {
    if a.is_empty() {
        return Vec::new();
    }
    let n = n.min(a.len());
    let min_chunk_size = a.len() / n;
    let max_chunk_num = a.len() % n;
    let mut chunks = Vec::with_capacity(n);
    let mut j = 0;
    for i in 0..n {
        let mut chunk_size = min_chunk_size;
        if i < max_chunk_num {
            chunk_size += 1;
        }
        chunks.push(&a[j..j + chunk_size]);
        j += chunk_size;
    }
    chunks
}

/// The handle a [`detachable`] job gets: Go's `*parallel.Context`.
///
/// A job holds one of the `n_workers` worker slots while it runs. A job that
/// waits on something other than the CPU can [`Context::detach`], moving to
/// one of the `n_max_detached` detached slots so that another job can start,
/// and [`Context::attach`] when it needs a worker slot again. Go's methods
/// accept a nil `*Context` and then do nothing; a Rust caller without a
/// context passes `Option<&mut Context>` and skips the call.
#[derive(Debug)]
pub struct Context {
    sem: Arc<Semaphore>,
    detached_sem: Arc<Semaphore>,
    detached: bool,
}

impl Context {
    /// Go's `Detach`: takes a detached slot, waiting until one is free, then
    /// gives up the worker slot. Does nothing when already detached.
    pub fn detach(&mut self) {
        if self.detached {
            return;
        }
        self.detached_sem.acquire();
        self.detached = true;
        self.sem.release();
    }

    /// Go's `Attach`: gives up the detached slot, then takes a worker slot,
    /// waiting until one is free. Does nothing when not detached.
    pub fn attach(&mut self) {
        if !self.detached {
            return;
        }
        self.detached = false;
        self.detached_sem.release();
        self.sem.acquire();
    }
}

/// `Detachable(ctx, nJobs, nWorkers, nMaxDetached, worker)`: runs
/// `worker(context, job_id)` for job ids `0..n_jobs`, each job on its own
/// thread, starting a job when one of the `n_workers` worker slots is free
/// (see [`Context`] for detaching). Once `ctx` is done no job starts, and a
/// started job that has not begun skips the worker. Returns `ctx.err()`
/// after every started job has returned.
///
/// As in Go, `n_workers == 0` with jobs to run blocks until `ctx` is done,
/// and so does a [`Context::detach`] with `n_max_detached == 0`.
pub fn detachable<F>(
    ctx: &gocontext::Context,
    n_jobs: usize,
    n_workers: usize,
    n_max_detached: usize,
    worker: F,
) -> Result<(), ContextError>
where
    F: Fn(&mut Context, usize) + Sync,
{
    let sem = Arc::new(Semaphore::new(n_workers));
    let detached_sem = Arc::new(Semaphore::new(n_max_detached));
    // Go's `select` between `<-ctx.Done()` and `sem <- struct{}{}`.
    let waker = Arc::clone(&sem);
    let _hook = ctx.on_done(move |_| waker.wake());
    let panicked = PanicSlot::default();
    thread::scope(|s| {
        for i in 0..n_jobs {
            if !sem.acquire_unless_done(ctx) {
                // Go waits for the started jobs here; the scope does.
                return;
            }
            if panicked.is_set() {
                sem.release();
                return;
            }
            let (sem, detached_sem, worker, panicked) = (
                Arc::clone(&sem),
                Arc::clone(&detached_sem),
                &worker,
                &panicked,
            );
            s.spawn(move || {
                if ctx.is_done() {
                    sem.release();
                    return;
                }
                let mut c = Context {
                    sem,
                    detached_sem,
                    detached: false,
                };
                let result = panic::catch_unwind(AssertUnwindSafe(|| worker(&mut c, i)));
                // Record a panic before freeing the slot, so that the
                // scheduler, once it has the slot, sees it.
                if let Err(payload) = result {
                    panicked.set(payload);
                }
                if c.detached {
                    c.detached_sem.release();
                } else {
                    c.sem.release();
                }
            });
        }
    });
    panicked.resume();
    ctx.err().map_or(Ok(()), Err)
}

/// Runs `body` on `n` scoped threads. If one panics, `stop` turns true so
/// the others stop taking jobs, and the first panic resumes on the caller
/// after all of them have returned.
fn run_workers(n: usize, body: impl Fn(&AtomicBool) + Sync) {
    let stop = AtomicBool::new(false);
    let panicked = PanicSlot::default();
    thread::scope(|s| {
        for _ in 0..n {
            let (stop, panicked, body) = (&stop, &panicked, &body);
            s.spawn(move || {
                if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| body(stop))) {
                    stop.store(true, Ordering::Relaxed);
                    panicked.set(payload);
                }
            });
        }
    });
    panicked.resume();
}

/// The first panic of a set of worker threads.
#[derive(Default)]
struct PanicSlot(Mutex<Option<Box<dyn Any + Send>>>);

impl PanicSlot {
    fn set(&self, payload: Box<dyn Any + Send>) {
        lock(&self.0).get_or_insert(payload);
    }

    fn is_set(&self) -> bool {
        lock(&self.0).is_some()
    }

    fn resume(self) {
        if let Some(payload) = self.0.into_inner().unwrap_or_else(PoisonError::into_inner) {
            panic::resume_unwind(payload);
        }
    }
}

/// A Go buffered channel of `struct{}` used as a semaphore: a send takes a
/// slot (blocking while all `cap` are taken), a receive gives one back.
#[derive(Debug)]
struct Semaphore {
    cap: usize,
    taken: Mutex<usize>,
    changed: Condvar,
}

impl Semaphore {
    fn new(cap: usize) -> Semaphore {
        Semaphore {
            cap,
            taken: Mutex::new(0),
            changed: Condvar::new(),
        }
    }

    /// Go's `sem <- struct{}{}`.
    fn acquire(&self) {
        let mut taken = lock(&self.taken);
        while *taken >= self.cap {
            taken = self.wait(taken);
        }
        *taken += 1;
    }

    /// Go's `select { case <-ctx.Done(): false; case sem <- struct{}{}: true }`.
    /// When both are ready Go picks one at random; this picks `ctx`, one of
    /// Go's outcomes. Needs a hook on `ctx` that calls [`Semaphore::wake`].
    fn acquire_unless_done(&self, ctx: &gocontext::Context) -> bool {
        let mut taken = lock(&self.taken);
        loop {
            if ctx.is_done() {
                return false;
            }
            if *taken < self.cap {
                *taken += 1;
                return true;
            }
            taken = self.wait(taken);
        }
    }

    /// Go's `<-sem`, for a slot this thread holds.
    fn release(&self) {
        let mut taken = lock(&self.taken);
        debug_assert!(*taken > 0, "released a slot nobody holds");
        *taken = taken.saturating_sub(1);
        drop(taken);
        // All: a woken waiter whose context is done does not take the slot.
        self.changed.notify_all();
    }

    /// Wakes the waiters to look at their context again. Taking the lock
    /// first means a waiter is either already waiting or will see the
    /// context done.
    fn wake(&self) {
        drop(lock(&self.taken));
        self.changed.notify_all();
    }

    fn wait<'a>(&self, taken: MutexGuard<'a, usize>) -> MutexGuard<'a, usize> {
        self.changed
            .wait(taken)
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Locks `mutex`; no code that can panic runs under these locks.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    //! Go's `parallel_test.go` (the synctest bubbles run on real time here),
    //! and tests of the paths it does not reach.

    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::AtomicI32;
    use std::time::{Duration, Instant};

    fn range_int(n: usize) -> Vec<usize> {
        (0..n).collect()
    }

    fn cells(n: usize) -> Vec<AtomicUsize> {
        (0..n).map(|_| AtomicUsize::new(usize::MAX)).collect()
    }

    fn values(cells: &[AtomicUsize]) -> Vec<usize> {
        cells.iter().map(|c| c.load(Ordering::SeqCst)).collect()
    }

    #[derive(Debug, PartialEq)]
    enum TestError {
        Context(ContextError),
        Job(String),
    }

    impl From<ContextError> for TestError {
        fn from(err: ContextError) -> Self {
            TestError::Context(err)
        }
    }

    #[test]
    fn test_parallel() {
        let a = range_int(10000);
        let b = cells(a.len());
        let worker_ids = cells(a.len());
        // multiple threads
        let result: Result<(), TestError> = parallel(
            &gocontext::Context::background(),
            a.len(),
            4,
            |worker_id, job_id| {
                b[job_id].store(a[job_id], Ordering::SeqCst);
                worker_ids[job_id].store(worker_id, Ordering::SeqCst);
                thread::sleep(Duration::from_micros(1));
                Ok(())
            },
        );
        assert_eq!(result, Ok(()));
        let workers_set: HashSet<usize> = values(&worker_ids).into_iter().collect();
        assert_eq!(values(&b), a);
        assert!(4 >= workers_set.len());
        assert!(1 < workers_set.len());
        // single thread
        let result: Result<(), TestError> = parallel(
            &gocontext::Context::background(),
            a.len(),
            1,
            |worker_id, job_id| {
                b[job_id].store(a[job_id], Ordering::SeqCst);
                worker_ids[job_id].store(worker_id, Ordering::SeqCst);
                Ok(())
            },
        );
        assert_eq!(result, Ok(()));
        let workers_set: HashSet<usize> = values(&worker_ids).into_iter().collect();
        assert_eq!(values(&b), a);
        assert_eq!(1, workers_set.len());
    }

    #[test]
    fn test_for() {
        // multiple threads
        let a = range_int(10000);
        let b = cells(a.len());
        let err = for_(&gocontext::Context::background(), a.len(), 4, |job_id| {
            b[job_id].store(a[job_id], Ordering::SeqCst);
            thread::sleep(Duration::from_micros(1));
        });
        assert_eq!(err, Ok(()));
        assert_eq!(values(&b), a);
        // single thread
        let b = cells(a.len());
        let err = for_(&gocontext::Context::background(), a.len(), 1, |job_id| {
            b[job_id].store(a[job_id], Ordering::SeqCst);
            thread::sleep(Duration::from_micros(1));
        });
        assert_eq!(err, Ok(()));
        assert_eq!(values(&b), a);
    }

    #[test]
    fn test_for_cancel() {
        let (ctx, cancel) = gocontext::Context::background().with_cancel();
        let count = AtomicI32::new(0);
        let err = for_(&ctx, 1000, 4, |job_id| {
            if job_id == 0 {
                cancel.cancel();
            }
            count.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_micros(100));
        });
        assert_eq!(err, Err(ContextError::Canceled));
        assert!(count.load(Ordering::SeqCst) < 1000);
    }

    #[test]
    fn test_for_each() {
        let a = range_int(10000);
        // multiple threads
        let b = cells(a.len());
        let err = for_each(&gocontext::Context::background(), &a, 4, |i, v| {
            assert_eq!(i, *v);
            b[i].store(*v, Ordering::SeqCst);
            thread::sleep(Duration::from_micros(1));
        });
        assert_eq!(err, Ok(()));
        assert_eq!(values(&b), a);
        // single thread
        let b = cells(a.len());
        let err = for_each(&gocontext::Context::background(), &a, 1, |i, v| {
            assert_eq!(i, *v);
            b[i].store(*v, Ordering::SeqCst);
            thread::sleep(Duration::from_micros(1));
        });
        assert_eq!(err, Ok(()));
        assert_eq!(values(&b), a);
    }

    #[test]
    fn test_for_each_cancel() {
        let (ctx, cancel) = gocontext::Context::background().with_cancel();
        let count = AtomicI32::new(0);
        let err = for_each(&ctx, &range_int(1000), 4, |i, _| {
            if i == 0 {
                cancel.cancel();
            }
            count.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_micros(100));
        });
        assert_eq!(err, Err(ContextError::Canceled));
        assert!(count.load(Ordering::SeqCst) < 1000);
    }

    #[test]
    fn test_parallel_fail() {
        let ctx = gocontext::Context::background();
        // multiple threads
        let err = parallel(&ctx, 10000, 4, |_, job_id| {
            if job_id % 2 == 1 {
                return Err(TestError::Job(format!("error from {job_id}")));
            }
            Ok(())
        });
        // Go asserts an error; the lowest failing job's is returned.
        assert_eq!(err, Err(TestError::Job("error from 1".to_owned())));
        // single thread
        let err = parallel(&ctx, 10000, 1, |_, job_id| {
            if job_id % 2 == 1 {
                return Err(TestError::Job(format!("error from {job_id}")));
            }
            Ok(())
        });
        assert_eq!(err, Err(TestError::Job("error from 1".to_owned())));
    }

    #[test]
    fn test_parallel_cancel() {
        let (ctx, cancel) = gocontext::Context::background().with_cancel();
        let count = AtomicI32::new(0);
        // Go sleeps a second per job inside a synctest bubble; real time here.
        let err = parallel(&ctx, 10000, 4, |_, job_id| {
            if job_id == 0 {
                cancel.cancel();
            }
            count.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(100));
            Ok(())
        });
        assert_eq!(err, Err(TestError::Context(ContextError::Canceled)));
        assert!(count.load(Ordering::SeqCst) < 10000);
    }

    #[test]
    fn test_split() {
        let a = [1, 2, 3, 4, 5, 6];
        let b = split(&a, 3);
        assert_eq!(b, [&[1, 2][..], &[3, 4], &[5, 6]]);

        let a = [1, 2, 3, 4, 5, 6, 7];
        let b = split(&a, 3);
        assert_eq!(b, [&[1, 2, 3][..], &[4, 5], &[6, 7]]);
    }

    #[test]
    fn test_detachable() {
        let start = Instant::now();
        let err = detachable(&gocontext::Context::background(), 100, 1, 100, |ctx, _| {
            ctx.detach();
            thread::sleep(Duration::from_secs(1));
            ctx.attach();
        });
        assert_eq!(err, Ok(()));
        assert!(start.elapsed() < Duration::from_secs(2));

        let start = Instant::now();
        let err = detachable(&gocontext::Context::background(), 100, 1, 10, |ctx, _| {
            ctx.detach();
            thread::sleep(Duration::from_secs(1));
            ctx.attach();
        });
        assert_eq!(err, Ok(()));
        assert!(start.elapsed() < Duration::from_secs(11));
    }

    #[test]
    fn test_detachable_cancel() {
        let (ctx, cancel) = gocontext::Context::background().with_cancel();
        let count = AtomicI32::new(0);
        let err = detachable(&ctx, 100, 4, 10, |_, job_id| {
            if job_id == 0 {
                cancel.cancel();
            }
            count.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(10));
        });
        assert_eq!(err, Err(ContextError::Canceled));
        assert!(count.load(Ordering::SeqCst) < 20);
    }

    // Beyond Go's tests.

    #[test]
    fn parallel_returns_the_lowest_failing_job() {
        let ctx = gocontext::Context::background();
        // Jobs fail out of order; the error of job 3 wins over later ones.
        let err = parallel(&ctx, 64, 8, |_, job_id| {
            if job_id == 3 {
                thread::sleep(Duration::from_millis(50));
            }
            if job_id == 3 || job_id >= 40 {
                return Err(TestError::Job(format!("error from {job_id}")));
            }
            Ok(())
        });
        assert_eq!(err, Err(TestError::Job("error from 3".to_owned())));
    }

    #[test]
    fn parallel_failed_worker_leaves_jobs_to_the_others() {
        let ctx = gocontext::Context::background();
        let done = cells(1000);
        let err = parallel(&ctx, 1000, 4, |_, job_id| {
            done[job_id].store(job_id, Ordering::SeqCst);
            if job_id == 0 {
                return Err(TestError::Job("first".to_owned()));
            }
            Ok(())
        });
        assert_eq!(err, Err(TestError::Job("first".to_owned())));
        assert_eq!(values(&done), range_int(1000));
    }

    #[test]
    fn parallel_on_a_canceled_context_runs_nothing() {
        let (ctx, cancel) = gocontext::Context::background().with_cancel();
        cancel.cancel();
        for n_workers in [1, 4] {
            let count = AtomicI32::new(0);
            let err = parallel(&ctx, 100, n_workers, |_, _| {
                count.fetch_add(1, Ordering::SeqCst);
                Ok::<(), TestError>(())
            });
            assert_eq!(err, Err(TestError::Context(ContextError::Canceled)));
            assert_eq!(count.load(Ordering::SeqCst), 0);
            let err = for_(&ctx, 100, n_workers, |_| {
                count.fetch_add(1, Ordering::SeqCst);
            });
            assert_eq!(err, Err(ContextError::Canceled));
            assert_eq!(count.load(Ordering::SeqCst), 0);
        }
        let count = AtomicI32::new(0);
        let err = detachable(&ctx, 100, 4, 4, |_, _| {
            count.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(err, Err(ContextError::Canceled));
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn parallel_recovers_worker_panics() {
        let ctx = gocontext::Context::background();
        let count = AtomicI32::new(0);
        let err = parallel(&ctx, 100, 4, |_, job_id| {
            if job_id == 0 {
                panic!("job 0");
            }
            count.fetch_add(1, Ordering::SeqCst);
            Ok::<(), TestError>(())
        });
        assert_eq!(err, Ok(()));
        assert_eq!(count.load(Ordering::SeqCst), 99);
    }

    #[test]
    fn single_worker_panics_reach_the_caller() {
        let ctx = gocontext::Context::background();
        let caught = panic::catch_unwind(AssertUnwindSafe(|| {
            parallel(&ctx, 10, 1, |_, job_id| -> Result<(), TestError> {
                assert_ne!(job_id, 5, "job 5");
                Ok(())
            })
        }));
        assert!(caught.is_err());
    }

    #[test]
    fn for_and_detachable_panics_resume_on_the_caller() {
        // The other jobs wait until job 0's panic has unwound out of the
        // worker, then take 1ms each, so the panicking worker has their whole
        // run to stop them however long the panic hook takes (it prints a
        // backtrace under RUST_BACKTRACE=1, which can take longer than 1000
        // jobs of 1ms on a loaded machine).
        struct Unwound<'a>(&'a AtomicBool);
        impl Drop for Unwound<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let wait_for = |unwound: &AtomicBool| {
            while !unwound.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_micros(100));
            }
        };

        let ctx = gocontext::Context::background();
        let unwound = AtomicBool::new(false);
        let started = AtomicI32::new(0);
        let caught = panic::catch_unwind(AssertUnwindSafe(|| {
            for_(&ctx, 1000, 4, |job_id| {
                started.fetch_add(1, Ordering::SeqCst);
                if job_id == 0 {
                    let _unwound = Unwound(&unwound);
                    panic!("job {job_id}");
                }
                wait_for(&unwound);
                thread::sleep(Duration::from_millis(1));
            })
        }));
        let payload = caught.expect_err("the panic resumes");
        let message = payload.downcast_ref::<String>().map(String::as_str);
        assert_eq!(message, Some("job 0"));
        assert!(started.load(Ordering::SeqCst) < 1000, "the others stop");

        let unwound = AtomicBool::new(false);
        let zero_detached = AtomicBool::new(false);
        let started = AtomicI32::new(0);
        let caught = panic::catch_unwind(AssertUnwindSafe(|| {
            detachable(&ctx, 1000, 2, 2, |c, job_id| {
                started.fetch_add(1, Ordering::SeqCst);
                if job_id == 0 {
                    // Job 0 detaches first and says so. The others wait for that,
                    // so they cannot fill both detached slots before job 0 has one.
                    c.detach();
                    zero_detached.store(true, Ordering::SeqCst);
                    let _unwound = Unwound(&unwound);
                    panic!("job {job_id}");
                }
                wait_for(&zero_detached);
                c.detach();
                wait_for(&unwound);
                thread::sleep(Duration::from_millis(1));
                c.attach();
            })
        }));
        let payload = caught.expect_err("the panic resumes");
        let message = payload.downcast_ref::<String>().map(String::as_str);
        assert_eq!(message, Some("job 0"));
        assert!(started.load(Ordering::SeqCst) < 1000, "no job starts after");
    }

    #[test]
    fn detachable_bounds_attached_and_detached_jobs() {
        let attached = AtomicI32::new(0);
        let detached = AtomicI32::new(0);
        let max_attached = AtomicI32::new(0);
        let max_detached = AtomicI32::new(0);
        let err = detachable(&gocontext::Context::background(), 200, 3, 5, |c, _| {
            let n = attached.fetch_add(1, Ordering::SeqCst) + 1;
            max_attached.fetch_max(n, Ordering::SeqCst);
            thread::sleep(Duration::from_micros(200));
            attached.fetch_sub(1, Ordering::SeqCst);
            c.detach();
            c.detach(); // no-op
            let n = detached.fetch_add(1, Ordering::SeqCst) + 1;
            max_detached.fetch_max(n, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(2));
            detached.fetch_sub(1, Ordering::SeqCst);
            c.attach();
            c.attach(); // no-op
            let n = attached.fetch_add(1, Ordering::SeqCst) + 1;
            max_attached.fetch_max(n, Ordering::SeqCst);
            attached.fetch_sub(1, Ordering::SeqCst);
        });
        assert_eq!(err, Ok(()));
        assert!(max_attached.load(Ordering::SeqCst) <= 3);
        assert!(max_detached.load(Ordering::SeqCst) <= 5);
        assert!(max_detached.load(Ordering::SeqCst) > 1);
    }

    #[test]
    fn detachable_cancel_wakes_a_scheduler_with_no_slots() {
        // No worker slots: only the cancel can end the wait for one (Go's
        // select on ctx.Done()).
        let (ctx, cancel) = gocontext::Context::background().with_cancel();
        let count = AtomicI32::new(0);
        thread::scope(|s| {
            s.spawn(|| {
                thread::sleep(Duration::from_millis(50));
                cancel.cancel();
            });
            let err = detachable(&ctx, 10, 0, 1, |_, _| {
                count.fetch_add(1, Ordering::SeqCst);
            });
            assert_eq!(err, Err(ContextError::Canceled));
        });
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn detachable_starts_no_job_after_cancel() {
        // Both workers hold their slots until the context is canceled from
        // outside, while the scheduler waits for a slot.
        let (ctx, cancel) = gocontext::Context::background().with_cancel();
        let count = AtomicI32::new(0);
        thread::scope(|s| {
            s.spawn(|| {
                thread::sleep(Duration::from_millis(50));
                cancel.cancel();
            });
            let err = detachable(&ctx, 100, 2, 2, |_, _| {
                count.fetch_add(1, Ordering::SeqCst);
                ctx.wait();
            });
            assert_eq!(err, Err(ContextError::Canceled));
        });
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn split_edges() {
        let empty: [i32; 0] = [];
        assert!(split(&empty, 3).is_empty());
        assert!(split(&empty, 0).is_empty());
        assert_eq!(split(&[1, 2], 5), [&[1][..], &[2]]);
        assert_eq!(split(&[1, 2, 3], 1), [&[1, 2, 3][..]]);
        assert!(panic::catch_unwind(AssertUnwindSafe(|| split(&[1], 0))).is_err());
    }
}
