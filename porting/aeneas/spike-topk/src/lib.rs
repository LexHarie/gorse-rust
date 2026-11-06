//! The TopKFilter attempt of the Aeneas spike (P3-2, D-331): the heap loops of
//! `TopKFilter` in `crates/gorse-common/src/heap.rs`, on an integer weight.
//!
//! The values are `u32` and the weights `i64`, so no float and no NaN check is
//! involved. The order is the one of `worse` in heap.rs: the root is the
//! lightest element, and among equal weights the one with the largest value.
//! The loops are `up`, `down` and `pop_worst`, which `push` runs.
//!
//! The crate is standalone and has no dependencies, so Charon reads exactly
//! these functions. It is translated by Charon and Aeneas in CI and is not a
//! member of the root workspace.
//!
//! The helpers take the `Vec` and index it by hand, as heap.rs does, so the code
//! Charon reads is the heap's shape; clippy's slice and swap suggestions are allowed
//! for that reason.

#![allow(clippy::ptr_arg, clippy::manual_swap, clippy::len_without_is_empty)]

/// A filter that keeps the `k` heaviest elements of a stream.
pub struct TopK {
    /// A heap of `(value, weight)` pairs whose root is the worst element held.
    elems: Vec<(u32, i64)>,
    k: usize,
}

/// Whether `a` is worse than `b`: lighter, or equal in weight with a larger value.
fn worse(a: (u32, i64), b: (u32, i64)) -> bool {
    a.1 < b.1 || (a.1 == b.1 && a.0 > b.0)
}

/// Whether the element at `i` is worse than the one at `j`.
fn worse_at(elems: &Vec<(u32, i64)>, i: usize, j: usize) -> bool {
    worse(elems[i], elems[j])
}

/// Swaps the elements at `i` and `j`.
fn swap_at(elems: &mut Vec<(u32, i64)>, i: usize, j: usize) {
    let t = elems[i];
    elems[i] = elems[j];
    elems[j] = t;
}

/// Sift-up, as `container/heap.up`, on the order of [`worse`].
fn up(elems: &mut Vec<(u32, i64)>, j0: usize) {
    let mut j = j0;
    while j > 0 {
        let i = (j - 1) / 2;
        if !worse_at(elems, j, i) {
            break;
        }
        swap_at(elems, i, j);
        j = i;
    }
}

/// Sift-down, as `container/heap.down`, on the order of [`worse`].
fn down(elems: &mut Vec<(u32, i64)>, i0: usize, n: usize) {
    let mut i = i0;
    loop {
        let j1 = 2 * i + 1;
        if j1 >= n {
            break;
        }
        let mut j = j1;
        let j2 = j1 + 1;
        if j2 < n && worse_at(elems, j2, j1) {
            j = j2;
        }
        if !worse_at(elems, j, i) {
            break;
        }
        swap_at(elems, i, j);
        i = j;
    }
}

/// Removes the worst element, the root of the heap. The heap must not be empty.
fn pop_worst(elems: &mut Vec<(u32, i64)>) {
    let n = elems.len() - 1;
    swap_at(elems, 0, n);
    down(elems, 0, n);
    elems.pop();
}

impl TopK {
    /// Makes an empty filter that keeps `k` elements.
    pub fn new(k: usize) -> TopK {
        TopK {
            elems: Vec::new(),
            k,
        }
    }

    /// Adds the element, then drops the worst if more than `k` are held.
    pub fn push(&mut self, value: u32, weight: i64) {
        self.elems.push((value, weight));
        let last = self.elems.len() - 1;
        up(&mut self.elems, last);
        if self.elems.len() > self.k {
            pop_worst(&mut self.elems);
        }
    }

    /// The number of elements held.
    pub fn len(&self) -> usize {
        self.elems.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_heaviest_k() {
        let mut f = TopK::new(2);
        f.push(1, 5);
        f.push(2, 9);
        f.push(3, 9);
        f.push(4, 1);
        assert_eq!(f.len(), 2);
        // The kept elements are (2, 9) and (3, 9): the value 1 was the lightest,
        // and the value 4 at weight 1 was dropped on arrival.
        let mut values: Vec<u32> = f.elems.iter().map(|e| e.0).collect();
        values.sort();
        assert_eq!(values, vec![2, 3]);
    }

    #[test]
    fn breaks_equal_weights_by_smaller_value() {
        let mut f = TopK::new(1);
        f.push(3, 9);
        f.push(2, 9);
        assert_eq!(f.elems, vec![(2, 9)]);
        f.push(5, 9);
        assert_eq!(f.elems, vec![(2, 9)]);
    }
}
