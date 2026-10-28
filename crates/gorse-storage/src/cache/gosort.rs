// Ports: $GOROOT/src/sort/slice.go, sort.go (xorshift, nextPowerOfTwo, the
// hints) and zsortfunc.go (pdqsort_func and its helpers) @ go1.27.0 (the
// toolchain add21b5 builds with)
//
// Derived from the Go sort package: Copyright 2009 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `sort.Slice`, the pattern-defeating quicksort (pdqsort) with a heap
//! sort fallback. It is not stable, and the order it gives equal elements
//! depends on the whole algorithm, so the cache's score documents are sorted
//! with this port and not with a stable sort.
//!
//! The Go code works on indices through `Less` and `Swap` closures. Here
//! [`LessSwap`] holds the slice and the comparison, and every index is the
//! same index Go uses.

/// The sortedness hint `choosePivot` returns (Go's `sortedHint`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SortedHint {
    Unknown,
    Increasing,
    Decreasing,
}

/// Go's `xorshift`: the pseudo-random generator `breakPatterns` uses.
struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Go's `bits.Len`: the number of bits needed to represent `n`.
fn bit_len(n: usize) -> usize {
    (usize::BITS - n.leading_zeros()) as usize
}

/// Go's `nextPowerOfTwo`.
fn next_power_of_two(length: usize) -> usize {
    1 << bit_len(length)
}

/// Go's `lessSwap`: the data being sorted and its `Less` function.
struct LessSwap<'a, T> {
    data: &'a mut [T],
    less: &'a dyn Fn(&T, &T) -> bool,
}

impl<T> LessSwap<'_, T> {
    fn less(&self, i: usize, j: usize) -> bool {
        (self.less)(&self.data[i], &self.data[j])
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.data.swap(i, j);
    }
}

/// Go's `sort.Slice`: sorts `data` so that `less` holds between neighbours.
pub(super) fn slice<T>(data: &mut [T], less: impl Fn(&T, &T) -> bool) {
    let length = data.len();
    let limit = bit_len(length);
    let less: &dyn Fn(&T, &T) -> bool = &less;
    pdqsort(&mut LessSwap { data, less }, 0, length, limit);
}

/// Go's `insertionSort_func`: sorts `data[a:b]`.
fn insertion_sort<T>(data: &mut LessSwap<T>, a: usize, b: usize) {
    for i in a + 1..b {
        let mut j = i;
        while j > a && data.less(j, j - 1) {
            data.swap(j, j - 1);
            j -= 1;
        }
    }
}

/// Go's `siftDown_func`: restores the heap property on `data[lo:hi]`, whose
/// root is at offset `first`.
fn sift_down<T>(data: &mut LessSwap<T>, lo: usize, hi: usize, first: usize) {
    let mut root = lo;
    loop {
        let mut child = 2 * root + 1;
        if child >= hi {
            break;
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

/// Go's `heapSort_func`: sorts `data[a:b]` with a heap.
fn heap_sort<T>(data: &mut LessSwap<T>, a: usize, b: usize) {
    let first = a;
    let lo = 0;
    let hi = b - a;
    // Build the heap with the greatest element at the top.
    for i in (0..=(hi - 1) / 2).rev() {
        sift_down(data, i, hi, first);
    }
    // Pop the elements, largest first, into the end of the data.
    for i in (0..hi).rev() {
        data.swap(first, first + i);
        sift_down(data, lo, i, first);
    }
}

/// Go's `pdqsort_func`: sorts `data[a:b]`. `limit` is the number of bad
/// pivots allowed before the heap sort takes over.
fn pdqsort<T>(data: &mut LessSwap<T>, mut a: usize, mut b: usize, mut limit: usize) {
    const MAX_INSERTION: usize = 12;
    let mut was_balanced = true;
    let mut was_partitioned = true;
    loop {
        let length = b - a;
        if length <= MAX_INSERTION {
            insertion_sort(data, a, b);
            return;
        }
        // Fall back to the heap sort when too many bad choices were made.
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
            // The pivot was `pivot - a` elements after the start; after the
            // reversal it is that many elements before the end.
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
        let (left_len, right_len) = (mid - a, b - mid);
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

/// Go's `partition_func`: one quicksort partition around `data[pivot]`. It
/// returns the pivot's new index and whether the slice was already
/// partitioned.
fn partition<T>(data: &mut LessSwap<T>, a: usize, b: usize, pivot: usize) -> (usize, bool) {
    data.swap(a, pivot);
    // i and j are inclusive of the elements still to be partitioned.
    let (mut i, mut j) = (a + 1, b as isize - 1);
    while (i as isize) <= j && data.less(i, a) {
        i += 1;
    }
    while (i as isize) <= j && !data.less(j as usize, a) {
        j -= 1;
    }
    if (i as isize) > j {
        data.swap(j as usize, a);
        return (j as usize, true);
    }
    data.swap(i, j as usize);
    i += 1;
    j -= 1;
    loop {
        while (i as isize) <= j && data.less(i, a) {
            i += 1;
        }
        while (i as isize) <= j && !data.less(j as usize, a) {
            j -= 1;
        }
        if (i as isize) > j {
            break;
        }
        data.swap(i, j as usize);
        i += 1;
        j -= 1;
    }
    data.swap(j as usize, a);
    (j as usize, false)
}

/// Go's `partitionEqual_func`: partitions `data[a:b]` into the elements equal
/// to `data[pivot]`, then the greater ones. It returns the first greater
/// index. The slice has no element smaller than the pivot.
fn partition_equal<T>(data: &mut LessSwap<T>, a: usize, b: usize, pivot: usize) -> usize {
    data.swap(a, pivot);
    let (mut i, mut j) = (a + 1, b as isize - 1);
    loop {
        while (i as isize) <= j && !data.less(a, i) {
            i += 1;
        }
        while (i as isize) <= j && data.less(a, j as usize) {
            j -= 1;
        }
        if (i as isize) > j {
            break;
        }
        data.swap(i, j as usize);
        i += 1;
        j -= 1;
    }
    i
}

/// Go's `partialInsertionSort_func`: shifts a few out-of-order pairs into
/// place. It returns whether `data[a:b]` is sorted at the end.
fn partial_insertion_sort<T>(data: &mut LessSwap<T>, a: usize, b: usize) -> bool {
    const MAX_STEPS: usize = 5;
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
            for j in (1..i).rev() {
                if !data.less(j, j - 1) {
                    break;
                }
                data.swap(j, j - 1);
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

/// Go's `breakPatterns_func`: scatters a few elements near the middle to break
/// patterns that make partitions unbalanced.
fn break_patterns<T>(data: &mut LessSwap<T>, a: usize, b: usize) {
    let length = b - a;
    if length >= 8 {
        let mut random = Xorshift(length as u64);
        let modulus = next_power_of_two(length);
        for idx in a + (length / 4) * 2 - 1..=a + (length / 4) * 2 + 1 {
            let mut other = (random.next() as usize) & (modulus - 1);
            if other >= length {
                other -= length;
            }
            data.swap(idx, a + other);
        }
    }
}

/// Go's `choosePivot_func`: picks a pivot in `data[a:b]` and reports whether
/// the slice looks sorted or reversed.
fn choose_pivot<T>(data: &mut LessSwap<T>, a: usize, b: usize) -> (usize, SortedHint) {
    const SHORTEST_NINTHER: usize = 50;
    const MAX_SWAPS: usize = 4 * 3;
    let l = b - a;
    let mut swaps = 0;
    let mut i = a + l / 4;
    let mut j = a + l / 4 * 2;
    let mut k = a + l / 4 * 3;
    if l >= 8 {
        if l >= SHORTEST_NINTHER {
            // Tukey's ninther: the median of three medians.
            i = median_adjacent(data, i, &mut swaps);
            j = median_adjacent(data, j, &mut swaps);
            k = median_adjacent(data, k, &mut swaps);
        }
        // The median of i, j and k is stored in j.
        j = median(data, i, j, k, &mut swaps);
    }
    match swaps {
        0 => (j, SortedHint::Increasing),
        MAX_SWAPS => (j, SortedHint::Decreasing),
        _ => (j, SortedHint::Unknown),
    }
}

/// Go's `order2_func`: returns `(x, y)` with `data[x] <= data[y]`, where
/// `(x, y)` is `(a, b)` or `(b, a)`.
fn order2<T>(data: &mut LessSwap<T>, a: usize, b: usize, swaps: &mut usize) -> (usize, usize) {
    if data.less(b, a) {
        *swaps += 1;
        return (b, a);
    }
    (a, b)
}

/// Go's `median_func`: the index of the median of `data[a]`, `data[b]` and
/// `data[c]`.
fn median<T>(data: &mut LessSwap<T>, a: usize, b: usize, c: usize, swaps: &mut usize) -> usize {
    let (a, b) = order2(data, a, b, swaps);
    let (b, _) = order2(data, b, c, swaps);
    let (_, b) = order2(data, a, b, swaps);
    b
}

/// Go's `medianAdjacent_func`: the median of the neighbours of `a`.
fn median_adjacent<T>(data: &mut LessSwap<T>, a: usize, swaps: &mut usize) -> usize {
    median(data, a - 1, a, a + 1, swaps)
}

/// Go's `reverseRange_func`.
fn reverse_range<T>(data: &mut LessSwap<T>, a: usize, b: usize) {
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

    /// A document is its score and its input position, so ties are visible.
    fn order(scores: &[u8]) -> Vec<(u8, usize)> {
        let mut docs: Vec<(u8, usize)> = scores.iter().copied().zip(0..).collect();
        slice(&mut docs, |a, b| a.0 > b.0);
        docs
    }

    #[test]
    fn sorts_descending() {
        let scores: Vec<u8> = (0..200).map(|i| (i * 37 % 101) as u8).collect();
        let sorted = order(&scores);
        assert!(sorted.windows(2).all(|w| w[0].0 >= w[1].0));
        let mut expected = scores.clone();
        expected.sort_unstable_by(|a, b| b.cmp(a));
        let got: Vec<u8> = sorted.iter().map(|d| d.0).collect();
        assert_eq!(got, expected);
    }

    /// `sort.Slice` output (Go 1.27.0) for the 40 scores `(i * 7) % 5`:
    /// the input positions, highest score first.
    #[test]
    fn ties_come_out_in_go_order_of_40_items() {
        let scores: Vec<u8> = (0..40u32).map(|i| ((i * 7) % 5) as u8).collect();
        let got: Vec<usize> = order(&scores).iter().map(|d| d.1).collect();
        assert_eq!(got, GO_40);
    }

    /// `sort.Slice` output (Go 1.27.0) for the 200 scores `(i * 37) % 101`,
    /// which are mostly ties. This input takes the pattern-breaking and
    /// heap paths that the 40-item input does not.
    #[test]
    fn ties_come_out_in_go_order_of_200_items() {
        let scores: Vec<u8> = (0..200u32).map(|i| ((i * 37) % 101) as u8).collect();
        let got: Vec<usize> = order(&scores).iter().map(|d| d.1).collect();
        assert_eq!(got, GO_200);
    }

    const GO_40: [usize; 40] = [
        22, 37, 2, 27, 32, 17, 12, 7, 4, 9, 24, 19, 34, 39, 14, 29, 16, 26, 31, 11, 36, 21, 1, 6,
        8, 23, 13, 3, 28, 38, 33, 18, 20, 30, 25, 35, 0, 15, 10, 5,
    ];

    const GO_200: [usize; 200] = [
        131, 30, 161, 60, 90, 191, 120, 19, 150, 49, 180, 79, 8, 109, 38, 139, 68, 169, 98, 199,
        128, 27, 57, 158, 188, 87, 16, 117, 46, 147, 76, 177, 5, 106, 136, 35, 65, 166, 196, 95,
        125, 24, 54, 155, 185, 84, 114, 13, 144, 43, 174, 73, 103, 2, 133, 32, 163, 62, 92, 193,
        21, 122, 51, 152, 81, 182, 10, 111, 141, 40, 70, 171, 100, 130, 29, 160, 59, 89, 190, 18,
        119, 149, 48, 179, 78, 7, 108, 138, 37, 168, 67, 198, 97, 26, 127, 56, 157, 86, 187, 116,
        15, 146, 45, 176, 75, 105, 4, 135, 34, 64, 165, 94, 195, 124, 23, 154, 53, 184, 83, 113,
        12, 42, 143, 173, 72, 102, 1, 132, 31, 61, 162, 192, 91, 121, 20, 151, 50, 181, 80, 9, 110,
        140, 39, 170, 69, 99, 28, 129, 58, 159, 88, 189, 118, 17, 47, 148, 77, 178, 6, 107, 36,
        137, 66, 167, 96, 197, 25, 126, 55, 156, 186, 85, 115, 14, 145, 44, 74, 175, 104, 3, 33,
        134, 164, 63, 93, 194, 123, 22, 52, 153, 82, 183, 11, 112, 142, 41, 71, 172, 0, 101,
    ];

    /// `SortDocuments` on the 40-item input gives Go's order too, so the
    /// comparator it passes is the one the golden was made with.
    #[test]
    fn sort_documents_breaks_ties_as_go_does() {
        use crate::cache::{Score, convert_documents_to_values, sort_documents};
        use gorse_common::gotime::Time;
        let mut documents: Vec<Score> = (0..40u32)
            .map(|i| Score {
                id: i.to_string(),
                score: ((i * 7) % 5) as f64,
                is_hidden: false,
                categories: Vec::new(),
                timestamp: Time::default(),
            })
            .collect();
        sort_documents(&mut documents);
        let got = convert_documents_to_values(&documents);
        let want: Vec<String> = GO_40.iter().map(|i| i.to_string()).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn handles_short_and_empty_slices() {
        assert!(order(&[]).is_empty());
        assert_eq!(order(&[3]), vec![(3, 0)]);
        assert_eq!(order(&[1, 2]), vec![(2, 1), (1, 0)]);
    }
}
