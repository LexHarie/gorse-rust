//! The Aeneas spike of the verification plan (P1-3): one loop over a slice.
//!
//! `sum_u32` has the shape the plan needs translated: a loop over a slice with
//! mutable state, as the heap, metrics and ring code are. Its translation and
//! Lean build are the spike's pass condition. The f32 question is in the sibling
//! crate `porting/aeneas/spike-f32`, so that result cannot hide this one.
//!
//! The crate is standalone and has no dependencies, so Charon reads exactly
//! this function.

/// Sums a slice of `u32`, failing when the sum overflows.
pub fn sum_u32(xs: &[u32]) -> u32 {
    let mut total: u32 = 0;
    let mut i: usize = 0;
    while i < xs.len() {
        total += xs[i];
        i += 1;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_u32_adds_the_slice() {
        assert_eq!(sum_u32(&[]), 0);
        assert_eq!(sum_u32(&[1, 2, 3]), 6);
    }
}
