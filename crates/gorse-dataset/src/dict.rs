// Ports: dataset/dict.go @ add21b5

//! The frequency dictionary: strings to dense `i32` indices, with the number of
//! times each string was added.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, PoisonError};

use crate::index::{Index, NOT_ID};

/// Go's `FreqDict`: a dictionary from strings to dense indices that counts how
/// often each string is added.
///
/// Every index has its own lock, from [`FreqDict::lock`] and
/// [`FreqDict::unlock`]. As with Go's `sync.Mutex`, a lock is not tied to a
/// guard: one thread may lock and a later call may unlock it.
#[derive(Debug, Default)]
pub struct FreqDict {
    si: HashMap<String, i32>,
    is: Vec<String>,
    cnt: Vec<i32>,
    mu: Vec<IndexLock>,
}

impl FreqDict {
    /// `NewFreqDict`: an empty dictionary.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of strings in the dictionary.
    pub fn count(&self) -> i32 {
        self.is.len() as i32
    }

    /// Adds `s` and counts one more occurrence of it, and returns its index.
    pub fn add(&mut self, s: &str) -> i32 {
        if let Some(&y) = self.si.get(s) {
            self.cnt[y as usize] += 1;
            return y;
        }
        self.push(s, 1)
    }

    /// Adds `s` without counting it, and returns its index. If `s` is already
    /// in the dictionary, its index is returned and its count is unchanged.
    pub fn add_no_count(&mut self, s: &str) -> i32 {
        if let Some(&y) = self.si.get(s) {
            return y;
        }
        self.push(s, 0)
    }

    /// The index of `s`, or [`NOT_ID`] when `s` is not in the dictionary.
    pub fn id(&self, s: &str) -> i32 {
        self.si.get(s).copied().unwrap_or(NOT_ID)
    }

    /// The string at index `id`, or `None` when `id` is past the end. A
    /// negative `id` panics, as Go's index expression does.
    pub fn string(&self, id: i32) -> Option<&str> {
        if id >= self.count() {
            return None;
        }
        Some(&self.is[slot(id)])
    }

    /// The count of the string at index `id`, or 0 when `id` is past the end.
    /// A negative `id` panics, as Go's index expression does.
    pub fn freq(&self, id: i32) -> i32 {
        if id >= self.cnt.len() as i32 {
            return 0;
        }
        self.cnt[slot(id)]
    }

    /// Locks index `index`, blocking until it is free. Panics if `index` is
    /// out of range.
    pub fn lock(&self, index: i32) {
        self.mu[slot(index)].lock();
    }

    /// Unlocks index `index`. Panics if the index is not locked, as Go's
    /// `sync.Mutex` does.
    pub fn unlock(&self, index: i32) {
        self.mu[slot(index)].unlock();
    }

    /// Locks index `index` until the returned guard is dropped, as Go's
    /// `Lock` followed by `defer Unlock`.
    pub(crate) fn lock_guard(&self, index: i32) -> IndexGuard<'_> {
        self.lock(index);
        IndexGuard { dict: self, index }
    }

    /// `ToIndex`: an [`Index`] with the dictionary's strings and indices.
    ///
    /// Go's index shares the dictionary's map. This one is a copy, so adding
    /// to the index does not change the dictionary (`PORTING_NOTES.md`, X-WP118-1).
    pub fn to_index(&self) -> Index {
        Index {
            numbers: self.si.clone(),
            names: self.is.clone(),
        }
    }

    /// Appends `s` with `count` and a new unlocked index, and returns the index.
    fn push(&mut self, s: &str, count: i32) -> i32 {
        let y = self.is.len() as i32;
        self.si.insert(s.to_owned(), y);
        self.is.push(s.to_owned());
        self.cnt.push(count);
        self.mu.push(IndexLock::default());
        y
    }
}

impl Clone for FreqDict {
    /// A copy with the same strings and counts. Its locks are new and unlocked,
    /// since a copy is a value of its own.
    fn clone(&self) -> Self {
        FreqDict {
            si: self.si.clone(),
            is: self.is.clone(),
            cnt: self.cnt.clone(),
            mu: self.mu.iter().map(|_| IndexLock::default()).collect(),
        }
    }
}

/// The dense index as a `usize`. A negative index becomes a huge one, which is
/// out of range, so using it panics as Go's negative index does.
fn slot(index: i32) -> usize {
    index as usize
}

/// Unlocks an index of a [`FreqDict`] when dropped.
pub(crate) struct IndexGuard<'a> {
    dict: &'a FreqDict,
    index: i32,
}

impl Drop for IndexGuard<'_> {
    fn drop(&mut self) {
        self.dict.unlock(self.index);
    }
}

/// The lock of one index: a flag under a mutex, with a condition variable that
/// wakes the waiters when the flag clears. Unlike a [`Mutex`] guard, it can be
/// locked and unlocked in separate calls, from any thread.
#[derive(Debug, Default)]
struct IndexLock {
    held: Mutex<bool>,
    released: Condvar,
}

impl IndexLock {
    fn lock(&self) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        while *held {
            held = self
                .released
                .wait(held)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *held = true;
    }

    fn unlock(&self) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(*held, "sync: unlock of unlocked mutex");
        *held = false;
        drop(held);
        self.released.notify_one();
    }
}

#[cfg(test)]
mod tests;
