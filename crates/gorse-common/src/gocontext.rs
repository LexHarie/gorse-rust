// Ports: $GOROOT/src/context/context.go (Background, WithCancel, CancelFunc, Canceled, the cancelCtx tree) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's context package: Copyright 2014 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `context` cancellation tree: [`Context::background`],
//! [`Context::with_cancel`] and the `context canceled` error.
//!
//! Gorse hands a `context.Context` to its parallel schedulers
//! ([`crate::parallel`]) and stops them by cancelling it. The callers in
//! add21b5 pass `context.Background()` or contexts made by `WithCancel`
//! (`master/master.go:120,472`), so this module ports that part of Go's
//! `context`: cancelling a context cancels every context derived from it,
//! [`CancelFunc::cancel`] removes a context from its parent, and a context
//! derived from a cancelled parent starts cancelled. Values
//! ([`Context::with_value`]) are ported too. Deadlines, causes and `AfterFunc`
//! are not (`porting/notes/WP-1-6.md`).
//!
//! A [`Context`] is a handle: clone it where Go copies the interface value.
//! Go's `<-ctx.Done()` is [`Context::wait`], and a `select` with a `default`
//! branch is [`Context::is_done`].
//!
//! ```
//! use gorse_common::gocontext::{Context, ContextError};
//!
//! let (parent, cancel) = Context::background().with_cancel();
//! let (child, _) = parent.with_cancel();
//! cancel.cancel();
//! assert_eq!(child.err(), Some(ContextError::Canceled));
//! assert_eq!(child.err().map(|e| e.to_string()).as_deref(), Some("context canceled"));
//! ```

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use thiserror::Error;

/// The error a done context reports: Go's `context.Canceled`.
///
/// Go also has `context.DeadlineExceeded`, which only deadlines produce; they
/// are not ported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ContextError {
    /// Go's `context.Canceled`: the context was cancelled.
    #[error("context canceled")]
    Canceled,
}

/// Go's `context.Context`, limited to cancellation.
///
/// [`Context::background`] (also [`Default`]) is never cancelled; contexts
/// made by [`Context::with_cancel`] are cancelled by their [`CancelFunc`] or
/// with their parent.
#[derive(Clone, Default)]
pub struct Context {
    /// `None` for Go's `Background()`, whose `Done()` is nil.
    node: Option<Arc<Node>>,
    /// The values added by [`Context::with_value`], newest first.
    values: Option<Arc<Value>>,
}

/// Go's `valueCtx`: one value and the values it was added to.
struct Value {
    key: TypeId,
    value: Arc<dyn Any + Send + Sync>,
    parent: Option<Arc<Value>>,
}

/// Go's `context.CancelFunc`: cancels the context [`Context::with_cancel`]
/// returned it with.
///
/// As in Go, dropping it does not cancel anything; a context that is never
/// cancelled stays registered with its parent until the parent is cancelled.
#[derive(Clone)]
pub struct CancelFunc {
    node: Arc<Node>,
}

/// Go's `cancelCtx`.
struct Node {
    /// Go's `err`, set once, under `state`'s lock; set means done.
    err: OnceLock<ContextError>,
    state: Mutex<State>,
    /// Woken when `err` is set; waiters hold `state`'s lock.
    done: Condvar,
}

#[derive(Default)]
struct State {
    /// Go's `children`, generalised: what to run when this context is
    /// cancelled. A child context registers a hook that cancels it.
    hooks: BTreeMap<u64, Hook>,
    /// Where this context's own hook is registered in its parent, so that
    /// cancelling it removes it there (Go's `removeChild`). Weak: a parent
    /// keeps its children alive, not the other way round.
    parent: Option<(Weak<Node>, u64)>,
}

type Hook = Box<dyn FnOnce(ContextError) + Send>;

/// Hook ids are unique across all contexts, so a child can record its id
/// before its hook is registered.
static NEXT_HOOK_ID: AtomicU64 = AtomicU64::new(0);

impl Context {
    /// Go's `context.Background()`: never cancelled.
    pub fn background() -> Context {
        Context::default()
    }

    /// Go's `context.WithCancel(parent)`: a context that is cancelled when
    /// the returned [`CancelFunc`] is called or when `self` is cancelled,
    /// whichever happens first. If `self` is already cancelled, so is the new
    /// context.
    pub fn with_cancel(&self) -> (Context, CancelFunc) {
        let child = Arc::new(Node {
            err: OnceLock::new(),
            state: Mutex::new(State::default()),
            done: Condvar::new(),
        });
        // Go's propagateCancel.
        if let Some(parent) = &self.node {
            let id = NEXT_HOOK_ID.fetch_add(1, Ordering::Relaxed);
            child.lock().parent = Some((Arc::downgrade(parent), id));
            let hook_child = Arc::clone(&child);
            parent.register(id, Box::new(move |err| hook_child.cancel(false, err)));
        }
        let cancel = CancelFunc {
            node: Arc::clone(&child),
        };
        (
            Context {
                node: Some(child),
                values: self.values.clone(),
            },
            cancel,
        )
    }

    /// Go's `context.WithValue`: a context that answers [`Context::value`] for
    /// type `V` with `value`, and is otherwise `self`. Keys are Rust types,
    /// which play the part of Go's unexported key types.
    pub fn with_value<V: Any + Send + Sync>(&self, value: V) -> Context {
        Context {
            node: self.node.clone(),
            values: Some(Arc::new(Value {
                key: TypeId::of::<V>(),
                value: Arc::new(value),
                parent: self.values.clone(),
            })),
        }
    }

    /// Go's `ctx.Value(key)` for the key type `V`: the nearest value added
    /// with that type, or `None`.
    pub fn value<V: Any + Send + Sync>(&self) -> Option<Arc<V>> {
        let mut link = self.values.as_deref();
        while let Some(current) = link {
            if current.key == TypeId::of::<V>() {
                return Arc::clone(&current.value).downcast::<V>().ok();
            }
            link = current.parent.as_deref();
        }
        None
    }

    /// Go's `ctx.Err()`: `None` until the context is done, then why.
    pub fn err(&self) -> Option<ContextError> {
        self.node.as_ref().and_then(|node| node.err.get().copied())
    }

    /// Whether the context is done: Go's
    /// `select { case <-ctx.Done(): true; default: false }`.
    pub fn is_done(&self) -> bool {
        self.err().is_some()
    }

    /// Go's `<-ctx.Done()`: blocks until the context is done. On
    /// [`Context::background`] it never returns, like a receive from Go's nil
    /// `Done()` channel.
    pub fn wait(&self) {
        match &self.node {
            Some(node) => {
                let mut state = node.lock();
                while node.err.get().is_none() {
                    state = node
                        .done
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
            None => loop {
                std::thread::park();
            },
        }
    }

    /// Like [`Context::wait`], but gives up after `timeout` (Go's `select`
    /// on `ctx.Done()` and a timer). Reports whether the context is done.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let Some(node) = &self.node else {
            std::thread::sleep(timeout);
            return false;
        };
        let deadline = Instant::now().checked_add(timeout);
        let mut state = node.lock();
        while node.err.get().is_none() {
            let left = match deadline {
                Some(deadline) => match deadline.checked_duration_since(Instant::now()) {
                    Some(left) if !left.is_zero() => left,
                    _ => return false,
                },
                None => Duration::MAX,
            };
            state = node
                .done
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Runs `hook` once, with the context's error, when the context is
    /// cancelled, on the cancelling thread before its cancel returns. If the
    /// context is already done, `hook` runs now; on
    /// [`Context::background`] it never runs. Dropping the returned
    /// [`DoneHook`] unregisters `hook` if it has not run.
    ///
    /// The schedulers use this to wake threads blocked on something else
    /// (Go's `select` on `ctx.Done()` and a channel). `hook` must not block.
    pub(crate) fn on_done(&self, hook: impl FnOnce(ContextError) + Send + 'static) -> DoneHook {
        let Some(node) = &self.node else {
            return DoneHook { link: None };
        };
        let id = NEXT_HOOK_ID.fetch_add(1, Ordering::Relaxed);
        node.register(id, Box::new(hook));
        DoneHook {
            link: Some((Arc::downgrade(node), id)),
        }
    }

    /// How many hooks (children included) the context holds: Go's tests read
    /// `len(cancelCtx.children)`.
    #[cfg(test)]
    fn hook_count(&self) -> usize {
        self.node.as_ref().map_or(0, |node| node.lock().hooks.len())
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.node {
            None => f.write_str("Context::background()"),
            Some(_) => f.debug_struct("Context").field("err", &self.err()).finish(),
        }
    }
}

impl CancelFunc {
    /// Cancels the context, and with it every context derived from it, and
    /// removes it from its parent. Calls after the first do nothing.
    pub fn cancel(&self) {
        self.node.cancel(true, ContextError::Canceled);
    }
}

impl fmt::Debug for CancelFunc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelFunc")
            .field("err", &self.node.err.get())
            .finish()
    }
}

/// A hook registered by [`Context::on_done`]; dropping it unregisters the
/// hook if it has not run.
pub(crate) struct DoneHook {
    link: Option<(Weak<Node>, u64)>,
}

impl Drop for DoneHook {
    fn drop(&mut self) {
        if let Some((node, id)) = self.link.take()
            && let Some(node) = node.upgrade()
        {
            node.unregister(id);
        }
    }
}

impl Node {
    fn lock(&self) -> MutexGuard<'_, State> {
        // No code that can panic runs under the lock; recover anyway.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds `hook` under `id`, or runs it now if the context is done.
    fn register(&self, id: u64, hook: Hook) {
        let mut state = self.lock();
        match self.err.get().copied() {
            Some(err) => {
                drop(state);
                hook(err);
            }
            None => {
                state.hooks.insert(id, hook);
            }
        }
    }

    fn unregister(&self, id: u64) {
        self.lock().hooks.remove(&id);
    }

    /// Go's `cancelCtx.cancel`. Go cancels the children while holding the
    /// parent's lock; the hooks run after it is released (a hook may lock
    /// anything), but still before this returns, so a cancel has reached
    /// every descendant when it returns, as in Go.
    fn cancel(&self, remove_from_parent: bool, err: ContextError) {
        let (hooks, parent) = {
            let mut state = self.lock();
            if self.err.set(err).is_err() {
                return; // already canceled
            }
            (std::mem::take(&mut state.hooks), state.parent.take())
        };
        self.done.notify_all();
        for hook in hooks.into_values() {
            hook(err);
        }
        if remove_from_parent
            && let Some((parent, id)) = parent
            && let Some(parent) = parent.upgrade()
        {
            parent.unregister(id);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Go's context tests for the ported part (`x_test.go`,
    //! `context_test.go` @ go1.27.0), without the deadline, value, cause and
    //! `String()` checks.

    use super::*;
    use std::sync::atomic::AtomicUsize;

    const QUIESCENT: Duration = Duration::from_secs(10);

    #[test]
    fn background() {
        let c = Context::background();
        assert_eq!(c.err(), None);
        assert!(!c.is_done());
        assert!(!c.wait_timeout(Duration::from_millis(10)));
        let (_, cancel) = c.with_cancel();
        cancel.cancel();
        assert_eq!(c.err(), None);
        assert_eq!(c.hook_count(), 0);
    }

    #[test]
    fn with_cancel() {
        let (c1, cancel) = Context::background().with_cancel();
        // Go's otherContext wrapper is a copy of the interface here.
        let o = c1.clone();
        let (c2, _) = o.with_cancel();
        let contexts = [&c1, &o, &c2];
        for c in contexts {
            assert_eq!(c.err(), None);
            assert!(!c.is_done());
        }
        cancel.cancel(); // Should propagate synchronously.
        for c in contexts {
            assert!(c.is_done());
            assert_eq!(c.err(), Some(ContextError::Canceled));
        }
        assert_eq!(
            ContextError::Canceled.to_string(),
            "context canceled",
            "Go's context.Canceled text"
        );
    }

    /// Go's XTestParentFinishesChild, with the cancel children.
    #[test]
    fn parent_finishes_child() {
        let (parent, cancel) = Context::background().with_cancel();
        let (cancel_child, stop) = parent.with_cancel();
        let (grandchild, _) = cancel_child.with_cancel();
        assert!(!parent.is_done());
        assert!(!cancel_child.is_done());
        assert!(!grandchild.is_done());
        assert_eq!(parent.hook_count(), 1);

        cancel.cancel();
        assert_eq!(parent.hook_count(), 0, "pc.cancel didn't clear pc.children");
        for c in [&parent, &cancel_child, &grandchild] {
            assert!(c.is_done());
            assert_eq!(c.err(), Some(ContextError::Canceled));
        }
        stop.cancel();

        // WithCancel should return a canceled context on a canceled parent.
        let (precanceled_child, _) = parent.with_cancel();
        assert!(precanceled_child.is_done());
        assert_eq!(precanceled_child.err(), Some(ContextError::Canceled));
        assert_eq!(parent.hook_count(), 0);
    }

    /// Go's XTestChildFinishesFirst.
    #[test]
    fn child_finishes_first() {
        let (cancelable, stop) = Context::background().with_cancel();
        for parent in [Context::background(), cancelable.clone()] {
            let (child, cancel) = parent.with_cancel();
            assert!(!parent.is_done());
            assert!(!child.is_done());
            let linked = parent.node.is_some();
            if linked {
                assert_eq!(parent.hook_count(), 1, "bad linkage");
            }

            cancel.cancel();
            if linked {
                assert_eq!(
                    parent.hook_count(),
                    0,
                    "child's cancel didn't remove self from pc.children"
                );
            }
            // child should be finished.
            assert!(child.is_done());
            assert_eq!(child.err(), Some(ContextError::Canceled));
            // parent should not be finished.
            assert!(!parent.is_done());
            assert_eq!(parent.err(), None);
        }
        stop.cancel();
    }

    /// Go's XTestCancelRemoves, for WithCancel children.
    #[test]
    fn cancel_removes() {
        let (ctx, _) = Context::background().with_cancel();
        assert_eq!(ctx.hook_count(), 0, "after creation");
        let (_, cancel) = ctx.with_cancel();
        assert_eq!(ctx.hook_count(), 1, "with WithCancel child");
        cancel.cancel();
        assert_eq!(ctx.hook_count(), 0, "after canceling WithCancel child");
        // A second cancel finds nothing to remove.
        cancel.cancel();
        assert_eq!(ctx.hook_count(), 0);

        // The schedulers' hooks are removed the same way.
        let hook = ctx.on_done(|_| {});
        assert_eq!(ctx.hook_count(), 1, "with a hook");
        drop(hook);
        assert_eq!(ctx.hook_count(), 0, "after dropping the hook");
    }

    #[test]
    fn simultaneous_cancels() {
        let (root, cancel) = Context::background().with_cancel();
        let mut contexts = vec![(root.clone(), cancel)];
        let mut queue = vec![root];
        // Create a tree of contexts.
        while !queue.is_empty() && contexts.len() < 100 {
            let parent = queue.remove(0);
            for _ in 0..4 {
                let (ctx, cancel) = parent.with_cancel();
                contexts.push((ctx.clone(), cancel));
                queue.push(ctx);
            }
        }
        // Start all the cancels in a scrambled order.
        let cancels: Vec<CancelFunc> = contexts.iter().map(|(_, c)| c.clone()).collect();
        std::thread::scope(|s| {
            for i in 0..cancels.len() {
                let cancel = &cancels[(i * 37) % cancels.len()];
                s.spawn(move || cancel.cancel());
            }
            // Wait on all the contexts in another order.
            for i in 0..contexts.len() {
                let (ctx, _) = &contexts[(i * 53) % contexts.len()];
                assert!(
                    ctx.wait_timeout(QUIESCENT),
                    "timed out waiting for <-ctx.Done()"
                );
            }
        });
    }

    #[test]
    fn interlocked_cancels() {
        let (parent, cancel_parent) = Context::background().with_cancel();
        let (child, cancel_child) = parent.with_cancel();
        std::thread::scope(|s| {
            s.spawn(|| {
                parent.wait();
                cancel_child.cancel();
            });
            cancel_parent.cancel();
            assert!(
                child.wait_timeout(QUIESCENT),
                "timed out waiting for child.Done()"
            );
        });
    }

    /// Go's testLayers with testTimeout false, for WithCancel layers.
    #[test]
    fn layers_cancel() {
        for seed in 0..10_u64 {
            let mut ctx = Context::background();
            let mut cancels = Vec::new();
            for _ in 0..30 {
                let (next, cancel) = ctx.with_cancel();
                ctx = next;
                cancels.push(cancel);
            }
            assert!(
                !ctx.is_done(),
                "seed={seed}: ctx should not be canceled yet"
            );
            let pick = (seed as usize * 7919) % cancels.len();
            cancels[pick].cancel();
            assert!(ctx.is_done(), "seed={seed}: ctx should be canceled");
            assert_eq!(ctx.err(), Some(ContextError::Canceled));
        }
    }

    #[test]
    fn with_cancel_canceled_parent() {
        let (parent, pcancel) = Context::background().with_cancel();
        pcancel.cancel();
        let (c, _) = parent.with_cancel();
        assert!(c.is_done(), "child not done immediately upon construction");
        assert_eq!(c.err(), Some(ContextError::Canceled));
    }

    #[test]
    fn with_cancel_simultaneously_canceled_parent() {
        // Cancel the parent concurrently with creating a child.
        for _ in 0..100 {
            let (parent, pcancel) = Context::background().with_cancel();
            std::thread::scope(|s| {
                s.spawn(|| pcancel.cancel());
                let (c, _) = parent.with_cancel();
                c.wait();
                assert_eq!(c.err(), Some(ContextError::Canceled));
            });
        }
    }

    #[test]
    fn hooks_run_once_and_late_hooks_run_now() {
        let ran = Arc::new(AtomicUsize::new(0));
        let (ctx, cancel) = Context::background().with_cancel();
        let counter = Arc::clone(&ran);
        let _hook = ctx.on_done(move |err| {
            assert_eq!(err, ContextError::Canceled);
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        cancel.cancel();
        cancel.cancel();
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        let counter = Arc::clone(&ran);
        let _late = ctx.on_done(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(ran.load(Ordering::SeqCst), 2);
        let counter = Arc::clone(&ran);
        let _never = Context::background().on_done(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(ran.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn wait_timeout_wakes_on_cancel() {
        let (ctx, cancel) = Context::background().with_cancel();
        assert!(!ctx.wait_timeout(Duration::from_millis(10)));
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(20));
                cancel.cancel();
            });
            assert!(ctx.wait_timeout(QUIESCENT));
        });
    }
}
