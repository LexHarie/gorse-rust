//! The f32 question of the Aeneas spike (P1-3): one f32 dot product.
//!
//! Whether Aeneas translates f32 arithmetic at the pinned revision is what the
//! verdict in `porting/aeneas/VERDICT.md` records. This crate is separate from
//! the loop crate, so a failure here does not hide the loop result.

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
    fn dot_f32_of_orthogonal_vectors_is_zero() {
        assert_eq!(dot_f32(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(dot_f32(&[1.0, 2.0], &[3.0, 4.0]), 11.0);
    }
}
