// Ports: common/sizeof/size.go @ add21b5

//! The size of a value in bytes, as Go's `sizeof.DeepSize` reports it: the
//! value's own size plus everything it points to.
//!
//! Go walks the value with `reflect`. Rust has no such walk, so each type
//! says what it weighs in Go: [`DeepSize::GO_SIZE`] is `reflect.Type.Size()`
//! and [`DeepSize::value_size`] is Go's `valueSize`, with the same cycle
//! check (each pointer counts once) and the same map estimate. A struct
//! implements [`DeepSize`] by adding its pointer and slice fields, as Go's
//! struct case does, with [`chase`] for the pointers.
//!
//! One Go quirk is kept on purpose: the unused capacity of a slice is charged
//! at the size of a slice header (24 bytes) per slot, not at the element size,
//! because Go's code asks for `v.Type().Size()` of the slice type
//! (`PORTING_NOTES.md`, Q-160).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use crate::util::rand::math::log;

/// A value whose size in Go, and everything it points to, [`deep_size`]
/// reports.
pub trait DeepSize {
    /// Go's `reflect.Type.Size()` of the type this value stands for.
    const GO_SIZE: usize;

    /// Go's `valueSize`: this value's size plus what it points to. `seen`
    /// holds the addresses of the pointers already counted.
    fn value_size(&self, seen: &mut HashSet<usize>) -> usize;
}

/// Go's `sizeof.DeepSize`: the size of `value` and what it points to, each
/// pointer counted once.
pub fn deep_size<T: DeepSize + ?Sized>(value: &T) -> usize {
    value.value_size(&mut HashSet::new())
}

/// Go's pointer case for a struct field or an element: counts the pointee the
/// first time its address is seen, and nothing for nil or a repeat. The
/// pointer's own size is not included, since the enclosing struct already has it.
pub fn chase<T: DeepSize>(pointer: Option<&Rc<T>>, seen: &mut HashSet<usize>) -> usize {
    match pointer {
        Some(rc) if seen.insert(Rc::as_ptr(rc) as usize) => rc.value_size(seen),
        _ => 0,
    }
}

macro_rules! scalar_deep_size {
    ($($ty:ty => $size:expr),* $(,)?) => {$(
        impl DeepSize for $ty {
            const GO_SIZE: usize = $size;

            fn value_size(&self, _seen: &mut HashSet<usize>) -> usize {
                Self::GO_SIZE
            }
        }
    )*};
}

// Go's sizes on a 64-bit platform: `int` and `uint` are 8 bytes.
scalar_deep_size! {
    bool => 1, i8 => 1, u8 => 1, i16 => 2, u16 => 2, i32 => 4, u32 => 4, f32 => 4,
    i64 => 8, u64 => 8, f64 => 8, isize => 8, usize => 8,
}

impl DeepSize for String {
    const GO_SIZE: usize = 16;

    /// A Go string is a 16-byte header and its bytes.
    fn value_size(&self, _seen: &mut HashSet<usize>) -> usize {
        Self::GO_SIZE + self.len()
    }
}

impl<T: DeepSize> DeepSize for Vec<T> {
    const GO_SIZE: usize = 24;

    fn value_size(&self, seen: &mut HashSet<usize>) -> usize {
        let mut size = Self::GO_SIZE;
        for item in self {
            size += item.value_size(seen);
        }
        // Go charges the unused capacity at the slice type's size (a header),
        // not the element size. See the module docs.
        if self.capacity() > self.len() {
            size += Self::GO_SIZE * (self.capacity() - self.len());
        }
        size
    }
}

impl<K: DeepSize, V: DeepSize, S> DeepSize for HashMap<K, V, S> {
    const GO_SIZE: usize = 8;

    /// Go's map estimate: `len/6.5` rounded up to a power of two buckets (at
    /// least one), 16 bytes of header per bucket, the keys and values, and
    /// zero slots for the empty entries of the 8-slot buckets.
    fn value_size(&self, seen: &mut HashSet<usize>) -> usize {
        let len = self.len() as f64;
        let exponent = (log(len / 6.5) / log(2.0)).ceil();
        let mut buckets = 2f64.powf(exponent) as usize;
        if buckets == 0 {
            buckets = 1;
        }
        let mut size = 16 * buckets;
        for (key, value) in self {
            size += key.value_size(seen);
            size += value.value_size(seen);
        }
        let zero_slots = 8 * buckets - self.len();
        size + zero_slots * (K::GO_SIZE + V::GO_SIZE)
    }
}

impl<T: DeepSize> DeepSize for Box<T> {
    const GO_SIZE: usize = 8;

    fn value_size(&self, seen: &mut HashSet<usize>) -> usize {
        let address = self.as_ref() as *const T as usize;
        if seen.insert(address) {
            Self::GO_SIZE + self.as_ref().value_size(seen)
        } else {
            Self::GO_SIZE
        }
    }
}

impl<T: DeepSize> DeepSize for Rc<T> {
    const GO_SIZE: usize = 8;

    fn value_size(&self, seen: &mut HashSet<usize>) -> usize {
        if seen.insert(Rc::as_ptr(self) as usize) {
            Self::GO_SIZE + self.as_ref().value_size(seen)
        } else {
            Self::GO_SIZE
        }
    }
}

impl<T: DeepSize> DeepSize for Arc<T> {
    const GO_SIZE: usize = 8;

    fn value_size(&self, seen: &mut HashSet<usize>) -> usize {
        if seen.insert(Arc::as_ptr(self) as usize) {
            Self::GO_SIZE + self.as_ref().value_size(seen)
        } else {
            Self::GO_SIZE
        }
    }
}

impl<T: DeepSize> DeepSize for Option<Rc<T>> {
    const GO_SIZE: usize = 8;

    /// A nil pointer is 8 bytes; a non-nil one is [`Rc`]'s size.
    fn value_size(&self, seen: &mut HashSet<usize>) -> usize {
        match self {
            Some(rc) => rc.value_size(seen),
            None => Self::GO_SIZE,
        }
    }
}

#[cfg(test)]
mod tests {
    // Ports: common/sizeof/size_test.go @ add21b5

    use std::cell::RefCell;

    use super::*;

    /// Go's `type V struct { Z int; E *V }`.
    struct V {
        #[allow(dead_code)] // Go's `Z`: an int, which adds nothing to the size.
        z: i64,
        e: RefCell<Option<Rc<V>>>,
    }

    impl DeepSize for V {
        const GO_SIZE: usize = 16;

        fn value_size(&self, seen: &mut HashSet<usize>) -> usize {
            Self::GO_SIZE + chase(self.e.borrow().as_ref(), seen)
        }
    }

    #[test]
    fn cyclic() {
        let v = Rc::new(V {
            z: 25,
            e: RefCell::new(None),
        });
        let want = deep_size(&v);
        // Induce a cycle: the pointer that closes it is counted once only.
        *v.e.borrow_mut() = Some(Rc::clone(&v));
        let got = deep_size(&v);
        *v.e.borrow_mut() = None;
        assert_eq!(got, want);
    }

    #[test]
    fn deep_size_of_slices_and_strings() {
        // matrix
        let a: Vec<Vec<i64>> = vec![vec![1], vec![2], vec![3], vec![4]];
        assert_eq!(deep_size(&a), 5 * 24 + 4 * 8);
        let b: Vec<Vec<i32>> = vec![vec![1], vec![2], vec![3], vec![4]];
        assert_eq!(deep_size(&b), 5 * 24 + 4 * 4);
        let c: Vec<Vec<i16>> = vec![vec![1], vec![2], vec![3], vec![4]];
        assert_eq!(deep_size(&c), 5 * 24 + 4 * 2);
        let d: Vec<Vec<i8>> = vec![vec![1], vec![2], vec![3], vec![4]];
        assert_eq!(deep_size(&d), 5 * 24 + 4);

        // strings
        let e: Vec<String> = vec!["abc".into(), "de".into(), "f".into()];
        assert_eq!(deep_size(&e), 24 + 16 * 3 + 6);
        let f: Vec<String> = vec!["♥♥♥".into(), "♥♥".into(), "♥".into()];
        assert_eq!(deep_size(&f), 24 + 16 * 3 + 18);

        // slice
        let g: Vec<i64> = vec![1, 2, 3, 4];
        assert_eq!(deep_size(&g), 7 * 8);
        let h: Vec<i32> = vec![1, 2, 3, 4];
        assert_eq!(deep_size(&h), 3 * 8 + 4 * 4);
        let i: Vec<i16> = vec![1, 2, 3, 4];
        assert_eq!(deep_size(&i), 3 * 8 + 2 * 4);
        let j: Vec<i8> = vec![1, 2, 3, 4];
        assert_eq!(deep_size(&j), 3 * 8 + 4);
    }

    #[test]
    fn map_estimate() {
        // Values from Go's DeepSize on the same maps (go1.27.0).
        let empty: HashMap<i64, i64> = HashMap::new();
        // One empty bucket: 16 bytes of header and eight zero slots of 16 bytes.
        assert_eq!(deep_size(&empty), 144);
        let thirteen: HashMap<i64, i64> = (0..13).map(|i| (i, i)).collect();
        assert_eq!(deep_size(&thirteen), 288);
        let forty: HashMap<i64, i64> = (0..40).map(|i| (i, i)).collect();
        assert_eq!(deep_size(&forty), 1152);
    }

    #[test]
    fn spare_capacity_matches_go() {
        // Go's DeepSize of make([]int64, 1, 3) is 80, not 24 + 8 + 2 * 8.
        let k: Vec<i64> = Vec::with_capacity(3);
        let mut k = k;
        k.push(0);
        assert_eq!(deep_size(&k), 80);
        let nil: Vec<i64> = Vec::new();
        assert_eq!(deep_size(&nil), 24);
    }
}
