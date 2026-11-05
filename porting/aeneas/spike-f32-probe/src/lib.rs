//! The f32 probe of the Aeneas spike (P1-3): f32 arithmetic with no float literal.
//!
//! `dot_f32` in `porting/aeneas/spike-f32` failed at its literal `0.0`, so this
//! crate takes the starting accumulator as a parameter and writes no float
//! constant in the function it translates. Whether Aeneas translates the
//! arithmetic on its own is what the verdict in `porting/aeneas/VERDICT.md`
//! records.

/// The dot product of two f32 slices, added to `acc`.
pub fn dot_f32_from(mut acc: f32, a: &[f32], b: &[f32]) -> f32 {
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
    fn dot_f32_from_adds_the_dot_product_to_the_start() {
        assert_eq!(dot_f32_from(1.0, &[1.0, 0.0], &[0.0, 1.0]), 1.0);
        assert_eq!(dot_f32_from(1.0, &[1.0, 2.0], &[3.0, 4.0]), 12.0);
    }
}
