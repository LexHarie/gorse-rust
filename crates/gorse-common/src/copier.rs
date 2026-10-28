// Ports: common/copier/copier.go @ add21b5

//! Deep copy into an existing value, reusing the destination's memory where it
//! can, as Go's `copier.Copy` does.
//!
//! Go walks both values with `reflect`. Rust has no such walk, so a type says
//! how it copies itself with [`DeepCopy`]; the standard types implement it the
//! way Go's code does. Three of Go's errors cannot happen here: a destination
//! that is not a pointer, a mismatch between two concrete types, and a struct
//! whose names differ are all compile-time errors in Rust. [`copy`] therefore
//! returns nothing.
//!
//! Go's aliasing (a copied slice sharing its backing array with a variable the
//! caller still holds) cannot be expressed with Rust ownership. Tests check the
//! same reuse by keeping the destination's buffer, through `as_ptr`.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

/// A value that copies itself deeply into an existing value of its type.
pub trait DeepCopy {
    /// Makes `self` equal to `src`, sharing no memory with it and reusing the
    /// memory `self` already has where it can.
    fn copy_from(&mut self, src: &Self);
}

/// Go's `copier.Copy`: deep copies `src` into `dst`.
pub fn copy<T: DeepCopy + ?Sized>(dst: &mut T, src: &T) {
    dst.copy_from(src);
}

macro_rules! scalar_deep_copy {
    ($($ty:ty),* $(,)?) => {$(
        impl DeepCopy for $ty {
            fn copy_from(&mut self, src: &Self) {
                *self = src.clone();
            }
        }
    )*};
}

scalar_deep_copy!(
    bool, i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64, String
);

impl<T: DeepCopy + Default + Clone> DeepCopy for Vec<T> {
    /// Go's slice case: the destination keeps its array when it has the
    /// capacity, and gets a new one of the source's length otherwise. Every
    /// element is then copied in place, so an element that owns memory keeps it.
    fn copy_from(&mut self, src: &Self) {
        if self.capacity() < src.len() {
            *self = Vec::with_capacity(src.len());
        }
        self.resize_with(src.len(), T::default);
        for (dst, src) in self.iter_mut().zip(src) {
            dst.copy_from(src);
        }
    }
}

impl<K, V, S> DeepCopy for HashMap<K, V, S>
where
    K: Eq + Hash + Clone,
    V: DeepCopy + Default + Clone + PartialEq,
    S: BuildHasher + Default,
{
    /// Go's map case: a destination equal to the source is left alone, so its
    /// values keep their memory. Otherwise the map is rebuilt from the source.
    fn copy_from(&mut self, src: &Self) {
        if self == src {
            return;
        }
        *self = HashMap::with_hasher(S::default());
        for (key, value) in src {
            let mut copied = V::default();
            copied.copy_from(value);
            self.insert(key.clone(), copied);
        }
    }
}

impl<T: DeepCopy + Default> DeepCopy for Option<T> {
    /// Go's pointer and interface cases: a nil source gives nil, and a nil
    /// destination gets a zero value before the copy.
    fn copy_from(&mut self, src: &Self) {
        match src {
            None => *self = None,
            Some(src) => self.get_or_insert_with(T::default).copy_from(src),
        }
    }
}

impl<T: DeepCopy + Default> DeepCopy for Box<T> {
    fn copy_from(&mut self, src: &Self) {
        self.as_mut().copy_from(src.as_ref());
    }
}

#[cfg(test)]
mod tests {
    // Ports: common/copier/copier_test.go @ add21b5

    use super::*;

    #[test]
    fn primitives() {
        let a = 1;
        let mut b = 0;
        copy(&mut b, &a);
        assert_eq!(a, b);
    }

    /// A dynamically typed value, the analogue of Go's `any` holding a
    /// `*Foo`, a `[]int` or an `int`.
    #[derive(Debug, Clone, PartialEq, Default)]
    enum Dynamic {
        #[default]
        Nil,
        Int(i64),
        Ints(Vec<i64>),
        Foo(Box<Foo>),
    }

    impl DeepCopy for Dynamic {
        /// Go's interface case: a value of the same type is copied in place,
        /// and a value of another type replaces the destination's.
        fn copy_from(&mut self, src: &Self) {
            match (self, src) {
                (Self::Int(dst), Self::Int(src)) => dst.copy_from(src),
                (Self::Ints(dst), Self::Ints(src)) => dst.copy_from(src),
                (Self::Foo(dst), Self::Foo(src)) => dst.copy_from(src),
                (dst, src) => *dst = src.clone(),
            }
        }
    }

    #[test]
    fn interface() {
        // Copy to an `any` holding nothing.
        let mut d = Dynamic::Nil;
        copy(&mut d, &Dynamic::Int(1));
        assert_eq!(d, Dynamic::Int(1));
    }

    #[test]
    fn slice() {
        let a: Vec<Vec<i64>> = vec![vec![1], vec![2], vec![3], vec![4]];
        let mut b: Vec<Vec<i64>> = Vec::new();
        copy(&mut b, &a);
        assert_eq!(a, b);

        // test deep copy
        let mut a = a;
        a[0][0] = 100;
        assert_eq!(b[0][0], 1);

        // test reuse memory: the destination keeps its arrays
        let c_first: Vec<i64> = vec![10];
        let mut c = vec![c_first, vec![20], vec![30], vec![40]];
        let kept = c[0].as_ptr();
        copy(&mut c, &a);
        assert_eq!(c[0].as_ptr(), kept);
        assert_eq!(c[0][0], 100);

        // copy to a list of `any` (the Dynamic analogue)
        let mut d: Vec<Vec<i64>> = Vec::new();
        copy(&mut d, &a);
        assert_eq!(a, d);

        // copy empty slice
        let mut e: Vec<i64> = vec![7];
        copy(&mut e, &Vec::new());
        assert!(e.is_empty());

        // copy to larger slice: the length follows the source, the capacity stays
        let mut f: Vec<Vec<i64>> = vec![vec![10], vec![20], vec![30], vec![40], vec![50]];
        let capacity = f.capacity();
        copy(&mut f, &a);
        assert_eq!(a, f);
        assert_eq!(f.capacity(), capacity);
        assert_eq!(f.capacity(), 5);
    }

    #[test]
    fn map() {
        let mut a: HashMap<i64, Vec<i64>> =
            HashMap::from([(1, vec![1]), (2, vec![1]), (3, vec![1])]);
        let mut b: HashMap<i64, Vec<i64>> =
            HashMap::from([(4, vec![100]), (5, vec![200]), (6, vec![300])]);
        copy(&mut b, &a);
        assert_eq!(a, b);

        // test deep copy
        a.get_mut(&1).expect("key 1")[0] = 100;
        assert_eq!(b[&1][0], 1);

        // test no copy: equal maps are left alone, so the value keeps its array
        let mut c: HashMap<i64, Vec<i64>> =
            HashMap::from([(1, vec![100]), (2, vec![1]), (3, vec![1])]);
        let kept = c[&1].as_ptr();
        copy(&mut c, &a);
        assert_eq!(a, c);
        assert_eq!(c[&1].as_ptr(), kept);
    }

    #[derive(Debug, Clone, PartialEq, Default)]
    struct Foo {
        a: i64,
        b: Vec<String>,
    }

    impl DeepCopy for Foo {
        fn copy_from(&mut self, src: &Self) {
            self.a.copy_from(&src.a);
            self.b.copy_from(&src.b);
        }
    }

    #[test]
    fn structure() {
        let mut a = Foo {
            a: 3,
            b: vec!["3".into()],
        };
        let mut b = Foo {
            a: 4,
            b: vec!["4".into()],
        };
        copy(&mut b, &a);
        assert_eq!(a, b);

        // test deep copy
        a.b[0] = "100".into();
        assert_eq!(b.b[0], "3");
    }

    #[test]
    fn pointer() {
        let a = Some(Box::new(Foo {
            a: 3,
            b: vec!["3".into()],
        }));
        let mut b = Some(Box::new(Foo {
            a: 4,
            b: vec!["4".into()],
        }));
        copy(&mut b, &a);
        assert_eq!(a, b);
        // A nil source gives nil, and a nil destination takes a zero value first.
        let mut nil_dst: Option<Box<Foo>> = None;
        copy(&mut nil_dst, &a);
        assert_eq!(nil_dst, a);
        let mut to_nil: Option<Box<Foo>> = a.clone();
        copy(&mut to_nil, &None);
        assert_eq!(to_nil, None);
    }

    #[test]
    fn interfaces_in_a_list() {
        let a = vec![
            Dynamic::Foo(Box::new(Foo {
                a: 3,
                b: vec!["3".into()],
            })),
            Dynamic::Ints(vec![100]),
            Dynamic::Int(1),
        ];
        let mut b: Vec<Dynamic> = Vec::new();
        copy(&mut b, &a);
        assert_eq!(a, b);
    }

    #[derive(Debug, Clone, PartialEq, Default)]
    struct PrivateStruct {
        text: String,
    }

    impl DeepCopy for PrivateStruct {
        /// Go's `BinaryMarshaler` path copies through the bytes; the text is
        /// the whole state here, so copying it is the same thing.
        fn copy_from(&mut self, src: &Self) {
            self.text = src.text.clone();
        }
    }

    #[test]
    fn private() {
        let mut a = PrivateStruct {
            text: "hello".into(),
        };
        let mut b = PrivateStruct::default();
        copy(&mut b, &a);
        assert_eq!(a, b);
        // test deep copy
        a.text = "world".into();
        assert_eq!(b.text, "hello");
    }

    #[derive(Debug, Clone, PartialEq, Default)]
    struct NilStruct {
        interface: Option<Dynamic>,
        pointer: Option<Box<Foo>>,
    }

    impl DeepCopy for NilStruct {
        fn copy_from(&mut self, src: &Self) {
            self.interface.copy_from(&src.interface);
            self.pointer.copy_from(&src.pointer);
        }
    }

    #[test]
    fn nil() {
        let mut d = NilStruct {
            interface: Some(Dynamic::Int(100)),
            pointer: Some(Box::new(Foo {
                a: 100,
                b: Vec::new(),
            })),
        };
        let e = NilStruct::default();
        copy(&mut d, &e);
        assert_eq!(d.interface, None);
        assert_eq!(d.pointer, None);
    }
}
