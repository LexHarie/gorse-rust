// Ports: $GOROOT/src/sort/sort.go (Sort, Interface, xorshift, nextPowerOfTwo), zsortinterface.go (insertionSort, siftDown, heapSort, pdqsort, partition, partitionEqual, partialInsertionSort, breakPatterns, choosePivot, order2, median, medianAdjacent, reverseRange) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's sort package: Copyright 2009 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `sort.Sort`: pattern-defeating quicksort, the algorithm the Go
//! toolchain runs for any `sort.Interface`.
//!
//! The sort is not stable. Elements that compare equal can come out in any
//! order, and the order Go picks is what Gorse's results depend on (for
//! example, `data.SortFeedbacks` decides which feedback counts as the latest
//! when two timestamps are equal). So this is the same algorithm, making the
//! same `Less` and `Swap` calls in the same order: up to 12 elements it is an
//! insertion sort, which keeps equal elements in input order; above that
//! pdqsort may not.
//!
//! Not ported: `sort.Stable`, `sort.Slice` and the `slices` package, which
//! Gorse does not call on the paths this crate serves.

/// Go's `sort.Interface`: the collection that [`sort`] reorders by index.
pub trait Interface {
    /// The number of elements.
    fn len(&self) -> usize;

    /// Reports whether there are no elements.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reports whether the element at `i` must sort before the element at `j`.
    /// If neither `less(i, j)` nor `less(j, i)` holds, the elements are equal.
    fn less(&self, i: usize, j: usize) -> bool;

    /// Swaps the elements at `i` and `j`.
    fn swap(&mut self, i: usize, j: usize);
}

/// Sorts `data` as Go's `sort.Sort` does: the same comparisons and swaps, in
/// the same order, so equal elements end where Go puts them.
pub fn sort<D: Interface + ?Sized>(data: &mut D) {
    let n = data.len();
    if n <= 1 {
        return;
    }
    // bits.Len(uint(n))
    let limit = (usize::BITS - n.leading_zeros()) as usize;
    pdqsort(data, 0, n, limit);
}

/// The hint pdqsort takes from `choose_pivot` about the order of the slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortedHint {
    Unknown,
    Increasing,
    Decreasing,
}

/// Go's `xorshift`, the generator `break_patterns` draws from.
struct Xorshift(u64);

impl Xorshift {
    /// Go's `Next`. Named `next_value` so it does not look like an iterator.
    fn next_value(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Go's `nextPowerOfTwo`: 1 shifted left by the bit length of `length`.
fn next_power_of_two(length: usize) -> usize {
    1 << (usize::BITS - length.leading_zeros())
}

/// Go's `insertionSort`: sorts `data[a:b]`, keeping equal elements in order.
fn insertion_sort<D: Interface + ?Sized>(data: &mut D, a: usize, b: usize) {
    for i in a + 1..b {
        let mut j = i;
        while j > a && data.less(j, j - 1) {
            data.swap(j, j - 1);
            j -= 1;
        }
    }
}

/// Go's `siftDown`: restores the heap property below `root` in the heap
/// `data[first:first+hi]`.
fn sift_down<D: Interface + ?Sized>(data: &mut D, lo: usize, hi: usize, first: usize) {
    let mut root = lo;
    loop {
        let mut child = 2 * root + 1;
        if child >= hi {
            return;
        }
        if child + 1 < hi && data.less(first + child, first + child + 1) {
            child += 1;
        }
        if !data.less(first + root, first + child) {
            return;
        }
        data.swap(first + root, first + child);
        root = child;
    }
}

/// Go's `heapSort`: sorts `data[a:b]` with a heap, pdqsort's fallback when
/// the partitions keep coming out unbalanced.
fn heap_sort<D: Interface + ?Sized>(data: &mut D, a: usize, b: usize) {
    let first = a;
    let lo = 0;
    let hi = b - a;

    // Build the heap with the greatest element at the top. Go's (hi-1)/2 is
    // 0 for an empty slice, as saturating_sub gives here.
    for i in (0..=hi.saturating_sub(1) / 2).rev() {
        sift_down(data, i, hi, first);
    }

    // Pop the elements, largest first, into the end of data.
    for i in (0..hi).rev() {
        data.swap(first, first + i);
        sift_down(data, lo, i, first);
    }
}

/// Go's `pdqsort`: sorts `data[a:b]`. `limit` is the number of unbalanced
/// partitions allowed before falling back to heapsort.
fn pdqsort<D: Interface + ?Sized>(data: &mut D, mut a: usize, mut b: usize, mut limit: usize) {
    const MAX_INSERTION: usize = 12;

    // Whether the last partitioning was reasonably balanced.
    let mut was_balanced = true;
    // Whether the slice was already partitioned.
    let mut was_partitioned = true;

    loop {
        let length = b - a;

        if length <= MAX_INSERTION {
            insertion_sort(data, a, b);
            return;
        }

        // Fall back to heapsort if too many bad choices were made.
        if limit == 0 {
            heap_sort(data, a, b);
            return;
        }

        // The last partitioning was imbalanced, so break the patterns.
        if !was_balanced {
            break_patterns(data, a, b);
            limit -= 1;
        }

        let (mut pivot, mut hint) = choose_pivot(data, a, b);
        if hint == SortedHint::Decreasing {
            reverse_range(data, a, b);
            // The chosen pivot was pivot-a elements after the start of the
            // array. After reversing it is pivot-a elements before the end.
            pivot = (b - 1) - (pivot - a);
            hint = SortedHint::Increasing;
        }

        // The slice is likely already sorted.
        if was_balanced
            && was_partitioned
            && hint == SortedHint::Increasing
            && partial_insertion_sort(data, a, b)
        {
            return;
        }

        // Probably many duplicates: partition into elements equal to the
        // pivot and elements greater than it.
        if a > 0 && !data.less(a - 1, pivot) {
            a = partition_equal(data, a, b, pivot);
            continue;
        }

        let (mid, already_partitioned) = partition(data, a, b, pivot);
        was_partitioned = already_partitioned;

        let left_len = mid - a;
        let right_len = b - mid;
        let balance_threshold = length / 8;
        if left_len < right_len {
            was_balanced = left_len >= balance_threshold;
            pdqsort(data, a, mid, limit);
            a = mid + 1;
        } else {
            was_balanced = right_len >= balance_threshold;
            pdqsort(data, mid + 1, b, limit);
            b = mid;
        }
    }
}

/// Go's `partition`: one quicksort partition of `data[a:b]` around the pivot
/// at `pivot`. Returns the pivot's new index and whether the slice was
/// already partitioned.
fn partition<D: Interface + ?Sized>(
    data: &mut D,
    a: usize,
    b: usize,
    pivot: usize,
) -> (usize, bool) {
    data.swap(a, pivot);
    // i and j are inclusive of the elements still to be partitioned.
    let (mut i, mut j) = (a + 1, b - 1);

    while i <= j && data.less(i, a) {
        i += 1;
    }
    while i <= j && !data.less(j, a) {
        j -= 1;
    }
    if i > j {
        data.swap(j, a);
        return (j, true);
    }
    data.swap(i, j);
    i += 1;
    j -= 1;

    loop {
        while i <= j && data.less(i, a) {
            i += 1;
        }
        while i <= j && !data.less(j, a) {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
    data.swap(j, a);
    (j, false)
}

/// Go's `partitionEqual`: partitions `data[a:b]`, which holds no element
/// smaller than the pivot, into elements equal to the pivot followed by
/// elements greater than it. Returns the index of the first greater element.
fn partition_equal<D: Interface + ?Sized>(data: &mut D, a: usize, b: usize, pivot: usize) -> usize {
    data.swap(a, pivot);
    // i and j are inclusive of the elements still to be partitioned.
    let (mut i, mut j) = (a + 1, b - 1);

    loop {
        while i <= j && !data.less(a, i) {
            i += 1;
        }
        while i <= j && data.less(a, j) {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
    i
}

/// Go's `partialInsertionSort`: tries to sort `data[a:b]` by shifting a few
/// out-of-order neighbours. Returns true if the slice ends up sorted.
fn partial_insertion_sort<D: Interface + ?Sized>(data: &mut D, a: usize, b: usize) -> bool {
    // The most adjacent out-of-order pairs that get shifted.
    const MAX_STEPS: usize = 5;
    // Short slices are not shifted at all.
    const SHORTEST_SHIFTING: usize = 50;

    let mut i = a + 1;
    for _ in 0..MAX_STEPS {
        while i < b && !data.less(i, i - 1) {
            i += 1;
        }

        if i == b {
            return true;
        }

        if b - a < SHORTEST_SHIFTING {
            return false;
        }

        data.swap(i, i - 1);

        // Shift the smaller one to the left.
        if i - a >= 2 {
            let mut j = i - 1;
            while j >= 1 {
                if !data.less(j, j - 1) {
                    break;
                }
                data.swap(j, j - 1);
                j -= 1;
            }
        }
        // Shift the greater one to the right.
        if b - i >= 2 {
            for j in i + 1..b {
                if !data.less(j, j - 1) {
                    break;
                }
                data.swap(j, j - 1);
            }
        }
    }
    false
}

/// Go's `breakPatterns`: scatters a few elements around the middle of
/// `data[a:b]` to break patterns that make quicksort's partitions unbalanced.
fn break_patterns<D: Interface + ?Sized>(data: &mut D, a: usize, b: usize) {
    let length = b - a;
    if length >= 8 {
        let mut random = Xorshift(length as u64);
        let modulus = next_power_of_two(length);
        let center = a + (length / 4) * 2;

        for idx in center - 1..=center + 1 {
            let mut other = (random.next_value() as usize) & (modulus - 1);
            if other >= length {
                other -= length;
            }
            data.swap(idx, a + other);
        }
    }
}

/// Go's `choosePivot`: picks a pivot index in `data[a:b]` and a hint about
/// whether the slice looks sorted or reversed.
///
/// Below 8 elements the pivot is static, from 8 it is the median of three,
/// and from 50 it is Tukey's ninther.
fn choose_pivot<D: Interface + ?Sized>(data: &D, a: usize, b: usize) -> (usize, SortedHint) {
    const SHORTEST_NINTHER: usize = 50;
    const MAX_SWAPS: usize = 4 * 3;

    let l = b - a;

    let mut swaps = 0;
    let mut i = a + l / 4;
    let mut j = a + l / 4 * 2;
    let mut k = a + l / 4 * 3;

    if l >= 8 {
        if l >= SHORTEST_NINTHER {
            // Tukey's ninther: the median of medians of three.
            i = median_adjacent(data, i, &mut swaps);
            j = median_adjacent(data, j, &mut swaps);
            k = median_adjacent(data, k, &mut swaps);
        }
        // Find the median among i, j and k, and store it into j.
        j = median(data, i, j, k, &mut swaps);
    }

    match swaps {
        0 => (j, SortedHint::Increasing),
        MAX_SWAPS => (j, SortedHint::Decreasing),
        _ => (j, SortedHint::Unknown),
    }
}

/// Go's `order2`: returns `(x, y)` with `data[x] <= data[y]`, where
/// `(x, y)` is `(a, b)` or `(b, a)`. Counts the swaps it implies.
fn order2<D: Interface + ?Sized>(
    data: &D,
    a: usize,
    b: usize,
    swaps: &mut usize,
) -> (usize, usize) {
    if data.less(b, a) {
        *swaps += 1;
        return (b, a);
    }
    (a, b)
}

/// Go's `median`: the index among `a`, `b` and `c` that holds the median.
fn median<D: Interface + ?Sized>(
    data: &D,
    a: usize,
    b: usize,
    c: usize,
    swaps: &mut usize,
) -> usize {
    let (a, b) = order2(data, a, b, swaps);
    let (b, _) = order2(data, b, c, swaps);
    let (_, b) = order2(data, a, b, swaps);
    b
}

/// Go's `medianAdjacent`: the median of the neighbours of `a`, and `a`.
fn median_adjacent<D: Interface + ?Sized>(data: &D, a: usize, swaps: &mut usize) -> usize {
    median(data, a - 1, a, a + 1, swaps)
}

/// Go's `reverseRange`: reverses `data[a:b]`.
fn reverse_range<D: Interface + ?Sized>(data: &mut D, a: usize, b: usize) {
    let (mut i, mut j) = (a, b - 1);
    while i < j {
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sorts the indexes of `keys` by their keys, as `sort.Sort` does on a
    /// slice of indexes, and returns the order.
    struct ByKey<'a> {
        idx: Vec<usize>,
        keys: &'a [i64],
        descending: bool,
    }

    impl Interface for ByKey<'_> {
        fn len(&self) -> usize {
            self.idx.len()
        }

        fn less(&self, i: usize, j: usize) -> bool {
            let (x, y) = (self.keys[self.idx[i]], self.keys[self.idx[j]]);
            if self.descending { x > y } else { x < y }
        }

        fn swap(&mut self, i: usize, j: usize) {
            self.idx.swap(i, j);
        }
    }

    fn order(keys: &[i64], descending: bool) -> Vec<usize> {
        let mut data = ByKey {
            idx: (0..keys.len()).collect(),
            keys,
            descending,
        };
        sort(&mut data);
        data.idx
    }

    #[test]
    fn sorts_every_small_length_as_insertion_sort() {
        // Up to 12 elements Go uses insertion sort, which keeps equal elements
        // in input order.
        for n in 0..=12 {
            let keys: Vec<i64> = (0..n).map(|i| (i % 3) as i64).collect();
            let got = order(&keys, false);
            let mut want: Vec<usize> = (0..n).collect();
            want.sort_by_key(|&i| keys[i]);
            assert_eq!(got, want, "n = {n}");
        }
    }

    #[test]
    fn sorts_descending() {
        let keys = [3, 1, 2, 3, 0];
        assert_eq!(order(&keys, true), vec![0, 3, 2, 1, 4]);
    }

    #[test]
    fn orders_larger_inputs_by_key() {
        let keys: Vec<i64> = (0..2000).map(|i: i64| (i * 7919) % 101).collect();
        let got = order(&keys, false);
        let mut seen = got.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..keys.len()).collect::<Vec<_>>(), "a permutation");
        assert!(got.windows(2).all(|w| keys[w[0]] <= keys[w[1]]));
    }

    #[derive(serde::Deserialize)]
    struct HeapGolden {
        cases: Vec<HeapCase>,
    }

    #[derive(serde::Deserialize)]
    struct HeapCase {
        name: String,
        descending: bool,
        keys: Vec<i64>,
        order: Vec<usize>,
    }

    /// `heap_sort` against `tests/data/sort_heap_golden.json`, which
    /// `contracts/tools/sortgolden` wrote from a verbatim copy of Go's
    /// `heapSort`. pdqsort reaches it too rarely for `sort_golden.json` to
    /// cover it.
    #[test]
    fn heap_sort_matches_go() {
        let golden: HeapGolden =
            serde_json::from_str(include_str!("../tests/data/sort_heap_golden.json"))
                .expect("golden");
        assert!(
            golden.cases.len() >= 1000,
            "the golden has {} cases",
            golden.cases.len()
        );
        for case in &golden.cases {
            let mut data = ByKey {
                idx: (0..case.keys.len()).collect(),
                keys: &case.keys,
                descending: case.descending,
            };
            let n = data.len();
            heap_sort(&mut data, 0, n);
            assert_eq!(data.idx, case.order, "{}", case.name);
        }
    }

    #[test]
    fn next_power_of_two_matches_go() {
        assert_eq!(next_power_of_two(8), 16);
        assert_eq!(next_power_of_two(15), 16);
        assert_eq!(next_power_of_two(16), 32);
    }
}
