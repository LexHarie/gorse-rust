// Ports: common/heap/pq.go, common/heap/filter.go @ add21b5

//! Binary heaps: [`PriorityQueue`], used by the vector indexes, and
//! [`TopKFilter`], which keeps the k heaviest of a stream of elements.
//!
//! Both are the Go types step for step, down to the sift-up and sift-down of
//! Go's `container/heap`, so that equal weights come out in the same order as
//! in Go and [`PriorityQueue::elems`] exposes the same heap array.

use std::collections::HashSet;
use std::io::{Read, Write};

use crate::encoding::{self, Element};

/// An element with its weight (Go `Elem[E, W]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Elem<E, W> {
    /// The element.
    pub value: E,
    /// Its weight.
    pub weight: W,
}

/// Go's `binary.Write` of an `Elem` struct: the fields in order, with no
/// padding.
impl<E: Element, W: Element> Element for Elem<E, W> {
    const SIZE: usize = E::SIZE + W::SIZE;

    fn put(self, out: &mut Vec<u8>) {
        self.value.put(out);
        self.weight.put(out);
    }

    fn get(bytes: &[u8]) -> Self {
        Self {
            value: E::get(&bytes[..E::SIZE]),
            weight: W::get(&bytes[E::SIZE..]),
        }
    }
}

/// Go's `_heap`: a min-heap on the weights, or a max-heap when `desc`, kept
/// with the operations of Go's `container/heap`.
#[derive(Debug)]
struct GoHeap<T, W> {
    elems: Vec<Elem<T, W>>,
    desc: bool,
}

impl<T, W: PartialOrd> GoHeap<T, W> {
    fn new(desc: bool) -> Self {
        Self {
            elems: Vec::new(),
            desc,
        }
    }

    /// `_heap.Less`.
    fn less(&self, i: usize, j: usize) -> bool {
        if self.desc {
            self.elems[i].weight > self.elems[j].weight
        } else {
            self.elems[i].weight < self.elems[j].weight
        }
    }

    /// `heap.Push`.
    fn push(&mut self, elem: Elem<T, W>) {
        self.elems.push(elem);
        self.up(self.elems.len() - 1);
    }

    /// `heap.Pop`, which panics in Go on an empty heap.
    fn pop(&mut self) -> Option<Elem<T, W>> {
        let n = self.elems.len().checked_sub(1)?;
        self.elems.swap(0, n);
        self.down(0, n);
        self.elems.pop()
    }

    /// `container/heap.up`.
    fn up(&mut self, mut j: usize) {
        while j > 0 {
            let i = (j - 1) / 2; // parent
            if !self.less(j, i) {
                break;
            }
            self.elems.swap(i, j);
            j = i;
        }
    }

    /// `container/heap.down`, without its unused result.
    fn down(&mut self, i0: usize, n: usize) {
        let mut i = i0;
        loop {
            let j1 = 2 * i + 1;
            if j1 >= n {
                break;
            }
            let mut j = j1; // left child
            let j2 = j1 + 1;
            if j2 < n && self.less(j2, j1) {
                j = j2; // right child
            }
            if !self.less(j, i) {
                break;
            }
            self.elems.swap(i, j);
            i = j;
        }
    }
}

/// A queue of `i32` values by `f32` weight (Go `PriorityQueue`). Pops the
/// lightest value first, or the heaviest when built descending.
///
/// The queue remembers every value ever pushed, popped ones included, and
/// ignores later pushes of them (`porting/notes/WP-1-5.md` Q-WP15-1).
#[derive(Debug)]
pub struct PriorityQueue {
    heap: GoHeap<i32, f32>,
    lookup: HashSet<i32>,
}

impl PriorityQueue {
    /// `NewPriorityQueue`: an empty queue, popping the lightest value first,
    /// or the heaviest when `desc`.
    #[must_use]
    pub fn new(desc: bool) -> Self {
        Self {
            heap: GoHeap::new(desc),
            lookup: HashSet::new(),
        }
    }

    /// The number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.heap.elems.len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.heap.elems.is_empty()
    }

    /// `Push`: inserts `v` unless it has been pushed before.
    ///
    /// # Panics
    ///
    /// Panics when `weight` is NaN, as Go does.
    pub fn push(&mut self, v: i32, weight: f32) {
        assert!(!weight.is_nan(), "NaN weight is forbidden");
        if self.lookup.insert(v) {
            self.heap.push(Elem { value: v, weight });
        }
    }

    /// `Pop`: removes the first value and returns it with its weight, or
    /// `None` when the queue is empty (Go panics; `porting/notes/WP-1-5.md`
    /// X-WP15-1). The value stays remembered (see [`PriorityQueue`]).
    pub fn pop(&mut self) -> Option<(i32, f32)> {
        self.heap.pop().map(|e| (e.value, e.weight))
    }

    /// `Peek`: the first value and its weight, or `None` when the queue is
    /// empty (Go panics).
    #[must_use]
    pub fn peek(&self) -> Option<(i32, f32)> {
        self.heap.elems.first().map(|e| (e.value, e.weight))
    }

    /// `Values`: the values in heap-array order.
    #[must_use]
    pub fn values(&self) -> Vec<i32> {
        self.heap.elems.iter().map(|e| e.value).collect()
    }

    /// `Elems`: the heap array.
    #[must_use]
    pub fn elems(&self) -> &[Elem<i32, f32>] {
        &self.heap.elems
    }

    /// Go's `Clone`: a queue with a copy of the heap array and the same
    /// order, but which remembers no values, so it accepts a push of a value
    /// it already holds (`porting/notes/WP-1-5.md` Q-WP15-2).
    #[must_use]
    pub fn clone_queue(&self) -> Self {
        let mut pq = Self::new(self.heap.desc);
        pq.heap.elems.clone_from(&self.heap.elems);
        pq
    }

    /// `Reverse`: a queue of the opposite order holding the same elements,
    /// pushed in heap-array order.
    ///
    /// # Panics
    ///
    /// Panics when an element has a NaN weight, which only
    /// [`PriorityQueue::unmarshal`] can produce, as Go does.
    #[must_use]
    pub fn reverse(&self) -> Self {
        let mut pq = Self::new(!self.heap.desc);
        pq.heap.elems.reserve(self.len());
        for e in &self.heap.elems {
            pq.push(e.value, e.weight);
        }
        pq
    }

    /// `Marshal`: the order as one byte (1 when descending), then the heap
    /// array with [`encoding::write_slice`], each element as its `i32` value
    /// and `f32` weight, little-endian.
    ///
    /// # Errors
    ///
    /// Fails when the writer fails.
    pub fn marshal(&self, w: &mut impl Write) -> Result<(), encoding::Error> {
        w.write_all(&[u8::from(self.heap.desc)])?;
        encoding::write_slice(w, &self.heap.elems)
    }

    /// `Unmarshal`: reads what [`PriorityQueue::marshal`] writes. Any non-zero
    /// order byte means descending; the heap array is taken as it is, and
    /// every value in it is remembered.
    ///
    /// Go's `Unmarshal` fills an existing queue and leaves it half-read on an
    /// error; this returns a new queue or the error
    /// (`porting/notes/WP-1-5.md` X-WP15-2).
    ///
    /// # Errors
    ///
    /// Fails when the input ends early or the length is negative.
    pub fn unmarshal(r: &mut impl Read) -> Result<Self, encoding::Error> {
        let mut desc = [0];
        r.read_exact(&mut desc)?;
        let elems: Vec<Elem<i32, f32>> = encoding::read_slice(r)?;
        let lookup = elems.iter().map(|e| e.value).collect();
        Ok(Self {
            heap: GoHeap {
                elems,
                desc: desc[0] != 0,
            },
            lookup,
        })
    }
}

/// Keeps the `k` heaviest of the elements pushed into it (Go
/// `TopKFilter[T, W]`).
///
/// Weights compare with `<`, as Go's do, so a NaN weight is neither lighter
/// nor heavier than any other and stays wherever the heap puts it.
#[derive(Debug)]
pub struct TopKFilter<T, W> {
    heap: GoHeap<T, W>,
    k: usize,
}

impl<T, W: PartialOrd> TopKFilter<T, W> {
    /// `NewTopKFilter`. Go takes an `int`, and a negative `k` keeps nothing,
    /// as `k = 0` does; a caller holding a signed value passes
    /// `usize::try_from(k).unwrap_or(0)`.
    #[must_use]
    pub fn new(k: usize) -> Self {
        Self {
            heap: GoHeap::new(false),
            k,
        }
    }

    /// The number of elements held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.heap.elems.len()
    }

    /// Whether no element is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.heap.elems.is_empty()
    }

    /// `Push`: adds the element, then drops the lightest if more than `k` are
    /// held.
    pub fn push(&mut self, item: T, weight: W) {
        self.heap.push(Elem {
            value: item,
            weight,
        });
        if self.len() > self.k {
            self.heap.pop();
        }
    }

    /// `PopAllValues`: empties the filter and returns the values, heaviest
    /// first.
    pub fn pop_all_values(&mut self) -> Vec<T> {
        self.pop_all().into_iter().map(|e| e.value).collect()
    }

    /// `PopAll`: empties the filter and returns the elements, heaviest first.
    pub fn pop_all(&mut self) -> Vec<Elem<T, W>> {
        // Pops come out lightest first; Go fills its result from the back.
        let mut results = Vec::with_capacity(self.len());
        while let Some(e) = self.heap.pop() {
            results.push(e);
        }
        results.reverse();
        results
    }
}

#[cfg(test)]
mod tests {
    // Ports: common/heap/pq_test.go, common/heap/filter_test.go @ add21b5
    use super::*;

    #[test]
    fn test_priority_queue() {
        let mut pq = PriorityQueue::new(false);
        let mut elements: Vec<i32> = vec![5, 3, 7, 8, 6, 2, 9];
        for &e in &elements {
            pq.push(e, e as f32);
        }
        assert_eq!(elements.len(), pq.len());
        let mut values = pq.values();
        values.sort_unstable();
        let mut expected = elements.clone();
        expected.sort_unstable();
        assert_eq!(expected, values, "ElementsMatch");
        assert_eq!(elements.len(), pq.elems().len());

        // test clone
        let cp = pq.clone_queue();
        assert_eq!(elements.len(), cp.len());

        // test peek pop
        elements.sort_unstable();
        for &e in &elements {
            let (value, weight) = pq.peek().expect("peek");
            assert_eq!(e, value);
            assert_eq!(e, weight as i32);
            let (value, weight) = pq.pop().expect("pop");
            assert_eq!(e, value);
            assert_eq!(e, weight as i32);
        }

        // test reverse
        let mut r = cp.reverse();
        elements.reverse();
        for &e in &elements {
            let (value, weight) = r.pop().expect("pop");
            assert_eq!(e, value);
            assert_eq!(e, weight as i32);
        }
    }

    #[test]
    fn test_marshal_unmarshal() {
        let mut pq = PriorityQueue::new(false);
        let elements: Vec<i32> = vec![5, 3, 7, 8, 6, 2, 9];
        for &e in &elements {
            pq.push(e, e as f32);
        }

        let dir = TempDir::new("test_marshal_unmarshal");
        let path = dir.0.join("pq.bin");
        let mut f = std::fs::File::create(&path).expect("create");
        pq.marshal(&mut f).expect("marshal");
        drop(f);

        let mut f = std::fs::File::open(&path).expect("open");
        let pq2 = PriorityQueue::unmarshal(&mut f).expect("unmarshal");

        assert_eq!(pq.len(), pq2.len());
        assert_eq!(pq.heap.desc, pq2.heap.desc);
        assert_eq!(pq.heap.elems, pq2.heap.elems);
        assert_eq!(pq.lookup, pq2.lookup);
    }

    #[test]
    fn test_top_k_filter() {
        // Test a adjacent vec
        let mut a = TopKFilter::<i32, f32>::new(3);
        a.push(10, 2.0);
        a.push(20, 8.0);
        a.push(30, 1.0);
        let values = a.pop_all_values();
        assert_eq!(vec![20, 10, 30], values);
        // Test a full adjacent vec
        let mut a = TopKFilter::<i32, f32>::new(3);
        a.push(10, 2.0);
        a.push(20, 8.0);
        a.push(30, 1.0);
        a.push(40, 2.0);
        a.push(50, 5.0);
        a.push(12, 10.0);
        a.push(67, 7.0);
        a.push(32, 9.0);
        let elems = a.pop_all();
        assert_eq!(
            vec![
                Elem {
                    value: 12,
                    weight: 10.0
                },
                Elem {
                    value: 32,
                    weight: 9.0
                },
                Elem {
                    value: 20,
                    weight: 8.0
                },
            ],
            elems
        );
    }

    #[test]
    fn test_top_k_string_filter() {
        // Test a adjacent vec
        let mut a = TopKFilter::<String, f64>::new(3);
        a.push("10".to_owned(), 2.0);
        a.push("20".to_owned(), 8.0);
        a.push("30".to_owned(), 1.0);
        let elems = a.pop_all();
        assert_eq!(
            vec![
                Elem {
                    value: "20".to_owned(),
                    weight: 8.0
                },
                Elem {
                    value: "10".to_owned(),
                    weight: 2.0
                },
                Elem {
                    value: "30".to_owned(),
                    weight: 1.0
                },
            ],
            elems
        );
        // Test a full adjacent vec
        let mut a = TopKFilter::<String, f64>::new(3);
        a.push("10".to_owned(), 2.0);
        a.push("20".to_owned(), 8.0);
        a.push("30".to_owned(), 1.0);
        a.push("40".to_owned(), 2.0);
        a.push("50".to_owned(), 5.0);
        a.push("12".to_owned(), 10.0);
        a.push("67".to_owned(), 7.0);
        a.push("32".to_owned(), 9.0);
        let elems = a.pop_all();
        assert_eq!(
            vec![
                Elem {
                    value: "12".to_owned(),
                    weight: 10.0
                },
                Elem {
                    value: "32".to_owned(),
                    weight: 9.0
                },
                Elem {
                    value: "20".to_owned(),
                    weight: 8.0
                },
            ],
            elems
        );
    }

    // Behaviour the Go tests leave out.

    #[test]
    #[should_panic(expected = "NaN weight is forbidden")]
    fn priority_queue_rejects_nan() {
        PriorityQueue::new(false).push(1, f32::NAN);
    }

    #[test]
    fn empty_priority_queue_pops_nothing() {
        let mut pq = PriorityQueue::new(true);
        assert_eq!(None, pq.peek());
        assert_eq!(None, pq.pop());
        assert!(pq.is_empty());
    }

    #[test]
    fn popped_values_are_not_pushed_again() {
        let mut pq = PriorityQueue::new(false);
        pq.push(1, 1.0);
        assert_eq!(Some((1, 1.0)), pq.pop());
        pq.push(1, 2.0);
        assert!(pq.is_empty());
    }

    /// A directory under the system temporary directory, removed on drop
    /// (Go's `t.TempDir()`).
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("gorse-common-heap-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod golden_tests {
    //! Replays the cases of `tests/data/heap_golden.json`, written by Go with
    //! `go run ./contracts/tools/heapgolden -out
    //! crates/gorse-common/tests/data/heap_golden.json`, and compares every
    //! result and every heap array with Go's.
    use serde_json::Value as Json;

    use super::*;

    fn golden() -> Json {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/heap_golden.json");
        let text = std::fs::read_to_string(path).expect("golden file");
        serde_json::from_str(&text).expect("golden JSON")
    }

    fn cases<'a>(g: &'a Json, section: &str) -> &'a [Json] {
        let cases = g[section].as_array().expect("section");
        assert!(!cases.is_empty(), "no cases in {section}");
        cases
    }

    fn str_of(v: &Json) -> &str {
        v.as_str().expect("string")
    }

    fn i32_of(v: &Json) -> i32 {
        i32::try_from(v.as_i64().expect("integer")).expect("int32")
    }

    fn f32_bits(v: &Json) -> f32 {
        f32::from_bits(u32::from_str_radix(str_of(v), 16).expect("hex bits"))
    }

    fn f64_bits(v: &Json) -> f64 {
        f64::from_bits(u64::from_str_radix(str_of(v), 16).expect("hex bits"))
    }

    fn list(v: &Json) -> &[Json] {
        v.as_array().expect("list")
    }

    /// Elements as (value, weight bits), so that NaN and -0 compare exactly.
    fn pq_pairs(v: &Json) -> Vec<(i32, u32)> {
        list(v)
            .iter()
            .map(|p| (i32_of(&p[0]), f32_bits(&p[1]).to_bits()))
            .collect()
    }

    fn rust_pairs(elems: &[Elem<i32, f32>]) -> Vec<(i32, u32)> {
        elems
            .iter()
            .map(|e| (e.value, e.weight.to_bits()))
            .collect()
    }

    fn hex_of(bytes: &[u8]) -> String {
        hex::encode(bytes)
    }

    #[test]
    fn heap_golden_priority_queue() {
        let g = golden();
        for case in cases(&g, "priority_queue") {
            let name = str_of(&case["name"]);
            let mut pq = PriorityQueue::new(case["desc"].as_bool().expect("desc"));
            for (i, op) in list(&case["ops"]).iter().enumerate() {
                let got = match str_of(&op["op"]) {
                    "push" => {
                        pq.push(i32_of(&op["value"]), f32_bits(&op["weight"]));
                        None
                    }
                    "pop" => Some(pq.pop().expect("pop")),
                    "peek" => Some(pq.peek().expect("peek")),
                    other => panic!("unknown op {other}"),
                };
                if let Some((value, weight)) = got {
                    assert_eq!(
                        (i32_of(&op["value"]), f32_bits(&op["weight"]).to_bits()),
                        (value, weight.to_bits()),
                        "{name}: op {i} result"
                    );
                }
                assert_eq!(
                    pq_pairs(&op["elems"]),
                    rust_pairs(pq.elems()),
                    "{name}: heap after op {i}"
                );
            }
            let values: Vec<i32> = list(&case["values"]).iter().map(i32_of).collect();
            assert_eq!(values, pq.values(), "{name}: values");
            assert_eq!(
                pq_pairs(&case["reverse"]),
                rust_pairs(pq.reverse().elems()),
                "{name}: reverse"
            );
            let mut cl = pq.clone_queue();
            for (value, weight) in pq_pairs(&case["clone_pushes"]) {
                cl.push(value, f32::from_bits(weight));
            }
            assert_eq!(
                pq_pairs(&case["clone_elems"]),
                rust_pairs(cl.elems()),
                "{name}: clone"
            );
            let mut buf = Vec::new();
            pq.marshal(&mut buf).expect("marshal");
            assert_eq!(str_of(&case["marshal"]), hex_of(&buf), "{name}: marshal");
        }
    }

    fn check_top_k<T, W>(
        name: &str,
        case: &Json,
        parse_value: impl Fn(&Json) -> T,
        parse_weight: impl Fn(&Json) -> W,
        bits: impl Fn(&W) -> u64,
    ) where
        T: PartialEq + std::fmt::Debug,
        W: PartialOrd,
    {
        let k = usize::try_from(case["k"].as_i64().expect("k")).unwrap_or(0);
        let mut filter = TopKFilter::<T, W>::new(k);
        for p in list(&case["pushes"]) {
            filter.push(parse_value(&p[0]), parse_weight(&p[1]));
        }
        let result = list(&case["result"]);
        match str_of(&case["method"]) {
            "pop_all" => {
                let want: Vec<(T, u64)> = result
                    .iter()
                    .map(|p| (parse_value(&p[0]), bits(&parse_weight(&p[1]))))
                    .collect();
                let got: Vec<(T, u64)> = filter
                    .pop_all()
                    .into_iter()
                    .map(|e| {
                        let b = bits(&e.weight);
                        (e.value, b)
                    })
                    .collect();
                assert_eq!(want, got, "{name}");
            }
            "pop_all_values" => {
                let want: Vec<T> = result.iter().map(&parse_value).collect();
                assert_eq!(want, filter.pop_all_values(), "{name}");
            }
            other => panic!("unknown method {other}"),
        }
        assert!(filter.is_empty(), "{name}: emptied");
    }

    #[test]
    fn heap_golden_top_k() {
        let g = golden();
        for case in cases(&g, "top_k") {
            let name = format!("{} {}", str_of(&case["name"]), str_of(&case["kind"]));
            match str_of(&case["kind"]) {
                "int32/float32" => check_top_k(&name, case, i32_of, f32_bits, |w: &f32| {
                    u64::from(w.to_bits())
                }),
                "string/float64" => check_top_k(
                    &name,
                    case,
                    |v| str_of(v).to_owned(),
                    f64_bits,
                    |w: &f64| w.to_bits(),
                ),
                other => panic!("unknown kind {other}"),
            }
        }
    }

    #[test]
    fn heap_golden_unmarshal() {
        let g = golden();
        for case in cases(&g, "unmarshal") {
            let name = str_of(&case["name"]);
            let data = hex::decode(str_of(&case["hex"])).expect("hex");
            let mut r = data.as_slice();
            let result = PriorityQueue::unmarshal(&mut r);
            if let Some(err) = case.get("error") {
                assert!(
                    result.is_err(),
                    "{name}: Go failed with {err}, Rust did not"
                );
                continue;
            }
            let pq = result.expect("unmarshal");
            assert_eq!(
                case["consumed"].as_u64().expect("consumed"),
                (data.len() - r.len()) as u64,
                "{name}: bytes read"
            );
            assert_eq!(case["desc"].as_bool(), Some(pq.heap.desc), "{name}: desc");
            assert_eq!(
                pq_pairs(&case["elems"]),
                rust_pairs(pq.elems()),
                "{name}: elems"
            );
            let mut lookup: Vec<i32> = pq.lookup.iter().copied().collect();
            lookup.sort_unstable();
            let want: Vec<i32> = list(&case["lookup"]).iter().map(i32_of).collect();
            assert_eq!(want, lookup, "{name}: lookup");
        }
    }
}
