//! The Aeneas spike of the verification plan (P1-3).
//!
//! Two functions, chosen to answer two questions about the pinned Aeneas:
//!
//! - `sum_u32` is a loop over a slice with `&mut` state, the shape the plan
//!   needs translated (heap, metrics and ring code are loops like it).
//! - `dot_f32` is the f32 dot product. Whether Aeneas translates it at the pin
//!   is the question the verdict in `porting/aeneas/VERDICT.md` records.
//!
//! The crate is standalone and has no dependencies, so Charon reads exactly
//! these functions.

/// Sums a slice of `u32`, failing (in Lean, and in Rust under debug checks)
/// when the sum overflows.
pub fn sum_u32(xs: &[u32]) -> u32 {
    let mut total: u32 = 0;
    let mut i: usize = 0;
    while i < xs.len() {
        total += xs[i];
        i += 1;
    }
    total
}

/// The dot product of two f32 slices of the same length.
pub fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
    let mut acc: f32 = 0.0;
    let mut i: usize = 0;
    while i < a.len() {
        acc += a[i] * b[i];
        i += 1;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_u32_adds_the_slice() {
        assert_eq!(sum_u32(&[]), 0);
        assert_eq!(sum_u32(&[1, 2, 3]), 6);
    }

    #[test]
    fn dot_f32_of_orthogonal_vectors_is_zero() {
        assert_eq!(dot_f32(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(dot_f32(&[1.0, 2.0], &[3.0, 4.0]), 11.0);
    }
}
