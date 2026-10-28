// Ports: model/ctr/transformer.go @ add21b5

//! Scalers for numerical features: min-max, robust (median and IQR), and the
//! automatic choice between them that factorization machines fit per feature.

use std::io::{Read, Write};

use gorse_common::encoding::{Error, read_gob, write_gob};
use gorse_common::gosort;

#[cfg(test)]
mod tests;

/// Scales a feature to the range `[0, 1]`:
///
/// ```text
/// X_scaled = (X - X.min) / (X.max - X.min)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MinMaxScaler {
    /// The smallest value fitted, or `+Inf` before a fit.
    pub min: f32,
    /// The largest value fitted, or `-Inf` before a fit.
    pub max: f32,
}

impl MinMaxScaler {
    /// A scaler with no values fitted (Go `NewMinMaxScaler`). Its minimum is
    /// `+Inf` and its maximum `-Inf`, so any fit replaces both.
    pub fn new() -> Self {
        Self {
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
        }
    }

    /// Updates the minimum and maximum with `values`.
    pub fn fit(&mut self, values: &[f32]) {
        for &v in values {
            if v < self.min {
                self.min = v;
            }
            if v > self.max {
                self.max = v;
            }
        }
    }

    /// Scales `value` to the range `[0, 1]`. A scaler with no values fitted
    /// returns `value` unchanged, and a constant fit returns 1.
    pub fn transform(&self, value: f32) -> f32 {
        if self.min > self.max {
            return value;
        }
        let range = self.max - self.min;
        if range == 0.0 {
            return 1.0;
        }
        (value - self.min) / range
    }

    /// Writes the minimum and the maximum as two gob values (Go `Marshal`).
    pub fn marshal(&self, w: &mut impl Write) -> Result<(), Error> {
        write_gob(w, &self.min)?;
        write_gob(w, &self.max)
    }

    /// Reads what [`MinMaxScaler::marshal`] wrote (Go `Unmarshal`).
    pub fn unmarshal(&mut self, r: &mut impl Read) -> Result<(), Error> {
        self.min = read_gob(r)?;
        self.max = read_gob(r)?;
        Ok(())
    }
}

/// Scales a feature by its median and interquartile range, which outliers do
/// not move much:
///
/// ```text
/// X_scaled = (X - median) / IQR, where IQR = Q3 - Q1
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RobustScaler {
    /// The median of the fitted values.
    pub median: f32,
    /// The 25th percentile.
    pub q1: f32,
    /// The 75th percentile.
    pub q3: f32,
    /// The interquartile range `Q3 - Q1`, or 1 when that is zero.
    pub iqr: f32,
}

impl RobustScaler {
    /// Go's `NewRobustScaler`: a scaler with nothing fitted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the median, the quartiles and the IQR from `values`. Empty input
    /// leaves the scaler as it was.
    pub fn fit(&mut self, values: &[f32]) {
        if values.is_empty() {
            return;
        }

        // Sort values to compute percentiles
        let mut sorted = values.to_vec();
        gosort::sort(&mut Float32s(&mut sorted));

        let n = sorted.len();

        // Compute median
        self.median = if n.is_multiple_of(2) {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        } else {
            sorted[n / 2]
        };

        // Compute Q1 (25th percentile) and Q3 (75th percentile)
        let q1_index = (n / 4).min(n - 1);
        let q3_index = (3 * n / 4).min(n - 1);

        self.q1 = sorted[q1_index];
        self.q3 = sorted[q3_index];
        self.iqr = self.q3 - self.q1;

        // Avoid division by zero
        if self.iqr == 0.0 {
            self.iqr = 1.0;
        }
    }

    /// Scales `value` by the median and the IQR. Before a fit, when the IQR
    /// is zero, `value` is returned unchanged.
    pub fn transform(&self, value: f32) -> f32 {
        if self.iqr == 0.0 {
            return value;
        }
        (value - self.median) / self.iqr
    }

    /// Writes the median, the quartiles and the IQR as four gob values (Go
    /// `Marshal`).
    pub fn marshal(&self, w: &mut impl Write) -> Result<(), Error> {
        write_gob(w, &self.median)?;
        write_gob(w, &self.q1)?;
        write_gob(w, &self.q3)?;
        write_gob(w, &self.iqr)
    }

    /// Reads what [`RobustScaler::marshal`] wrote (Go `Unmarshal`).
    pub fn unmarshal(&mut self, r: &mut impl Read) -> Result<(), Error> {
        self.median = read_gob(r)?;
        self.q1 = read_gob(r)?;
        self.q3 = read_gob(r)?;
        self.iqr = read_gob(r)?;
        Ok(())
    }
}

/// Chooses the scaling of a feature from its values. A feature with a
/// negative value is scaled robustly and then min-max scaled. A feature with
/// no negative value is log-transformed (`log1p`) and then min-max scaled.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AutoScaler {
    /// Whether the feature is log-transformed. When false, it is robust
    /// scaled.
    pub use_log: bool,
    /// The min-max scaler of the values after the first step: `log1p` when
    /// `use_log`, the robust scaling otherwise.
    pub min_max: MinMaxScaler,
    /// The robust scaler, used when `use_log` is false.
    pub robust: RobustScaler,
}

impl AutoScaler {
    /// Go's `NewAutoScaler`: a scaler with the min-max scaler unfitted.
    pub fn new() -> Self {
        Self {
            min_max: MinMaxScaler::new(),
            ..Self::default()
        }
    }

    /// Chooses the scaling from `values` and fits it. Empty input leaves the
    /// scaler as it was.
    pub fn fit(&mut self, values: &[f32]) {
        if values.is_empty() {
            return;
        }

        // Check for negative values
        let has_negative = values.iter().any(|&v| v < 0.0);

        if has_negative {
            // Use RobustScaler then MinMaxScaler for data with negative values
            self.use_log = false;
            self.robust.fit(values);

            let robust_values: Vec<f32> =
                values.iter().map(|&v| self.robust.transform(v)).collect();
            self.min_max.fit(&robust_values);
        } else {
            // Apply log1p then MinMaxScaler for non-negative data
            self.use_log = true;

            let log_values: Vec<f32> = values.iter().map(|&v| log1p(v)).collect();
            self.min_max.fit(&log_values);
        }
    }

    /// Scales `value` with the chosen method. Under `use_log`, a negative
    /// value is clamped to 0 before `log1p`.
    pub fn transform(&self, value: f32) -> f32 {
        if !self.use_log {
            let robust_value = self.robust.transform(value);
            return self.min_max.transform(robust_value);
        }

        // Apply log1p then MinMax
        let log_value = log1p(go_max_zero(value));
        self.min_max.transform(log_value)
    }

    /// Writes `use_log`, the min-max scaler and, when `use_log` is false, the
    /// robust scaler (Go `Marshal`).
    pub fn marshal(&self, w: &mut impl Write) -> Result<(), Error> {
        write_gob(w, &self.use_log)?;
        self.min_max.marshal(w)?;
        if !self.use_log {
            self.robust.marshal(w)?;
        }
        Ok(())
    }

    /// Reads what [`AutoScaler::marshal`] wrote (Go `Unmarshal`).
    pub fn unmarshal(&mut self, r: &mut impl Read) -> Result<(), Error> {
        self.use_log = read_gob(r)?;
        self.min_max.unmarshal(r)?;
        if !self.use_log {
            self.robust.unmarshal(r)?;
        }
        Ok(())
    }
}

/// Go's `max(0, value)` for a float32: `NaN` stays `NaN`, and negative zero
/// gives positive zero.
fn go_max_zero(value: f32) -> f32 {
    if value.is_nan() || value > 0.0 {
        value
    } else {
        0.0
    }
}

/// `chewxy/math32.Log1p`: Go's `math.Log1p` on the float64 of `x`, rounded to
/// float32.
fn log1p(x: f32) -> f32 {
    go_log1p(f64::from(x)) as f32
}

/// Go's `math.Log1p` (the pure Go code, which amd64 uses), from FreeBSD's
/// `s_log1p.c`.
// The constants are Go's, digit for digit, so each can be checked against the
// source. The digits past float64 precision round to the same value.
#[allow(clippy::excessive_precision)]
fn go_log1p(x: f64) -> f64 {
    const SQRT2M1: f64 = 4.142135623730950488017e-01;
    const SQRT2HALF_M1: f64 = -2.928932188134524755992e-01;
    const SMALL: f64 = 1.0 / (1u64 << 29) as f64;
    const TINY: f64 = 1.0 / (1u64 << 54) as f64;
    const TWO53: f64 = (1u64 << 53) as f64;
    const LN2HI: f64 = 6.93147180369123816490e-01;
    const LN2LO: f64 = 1.90821492927058770002e-10;
    const LP1: f64 = 6.666666666666735130e-01;
    const LP2: f64 = 3.999999999940941908e-01;
    const LP3: f64 = 2.857142874366239149e-01;
    const LP4: f64 = 2.222219843214978396e-01;
    const LP5: f64 = 1.818357216161805012e-01;
    const LP6: f64 = 1.531383769920937332e-01;
    const LP7: f64 = 1.479819860511658591e-01;

    if x < -1.0 || x.is_nan() {
        return f64::NAN;
    }
    if x == -1.0 {
        return f64::NEG_INFINITY;
    }
    if x == f64::INFINITY {
        return f64::INFINITY;
    }

    let absx = x.abs();
    let mut f: f64 = 0.0;
    let mut iu: u64 = 0;
    let mut k: i64 = 1;
    if absx < SQRT2M1 {
        // |x| < Sqrt(2)-1
        if absx < SMALL {
            // |x| < 2**-29
            if absx < TINY {
                // |x| < 2**-54
                return x;
            }
            return x - x * x * 0.5;
        }
        if x > SQRT2HALF_M1 {
            // Sqrt(2)/2-1 < x
            k = 0;
            f = x;
            iu = 1;
        }
    }
    let mut c: f64 = 0.0;
    if k != 0 {
        let mut u: f64;
        if absx < TWO53 {
            // 1<<53
            u = 1.0 + x;
            iu = u.to_bits();
            k = (iu >> 52) as i64 - 1023;
            if k > 0 {
                c = 1.0 - (u - x);
            } else {
                c = x - (u - 1.0);
            }
            c /= u;
        } else {
            u = x;
            iu = u.to_bits();
            k = (iu >> 52) as i64 - 1023;
            c = 0.0;
        }
        iu &= 0x000f_ffff_ffff_ffff;
        if iu < 0x0006_a09e_667f_3bcd {
            // mantissa of Sqrt(2)
            u = f64::from_bits(iu | 0x3ff0_0000_0000_0000); // normalize u
        } else {
            k += 1;
            u = f64::from_bits(iu | 0x3fe0_0000_0000_0000); // normalize u/2
            iu = (0x0010_0000_0000_0000 - iu) >> 2;
        }
        f = u - 1.0; // Sqrt(2)/2 < u < Sqrt(2)
    }
    let hfsq = 0.5 * f * f;
    let kf = k as f64;
    if iu == 0 {
        // |f| < 2**-20
        if f == 0.0 {
            if k == 0 {
                return 0.0;
            }
            c += kf * LN2LO;
            return kf * LN2HI + c;
        }
        let r = hfsq * (1.0 - 0.66666666666666666 * f); // avoid division
        if k == 0 {
            return f - r;
        }
        return kf * LN2HI - ((r - (kf * LN2LO + c)) - f);
    }
    let s = f / (2.0 + f);
    let z = s * s;
    let r = z * (LP1 + z * (LP2 + z * (LP3 + z * (LP4 + z * (LP5 + z * (LP6 + z * LP7))))));
    if k == 0 {
        return f - (hfsq - s * (hfsq + r));
    }
    kf * LN2HI - ((hfsq - (s * (hfsq + r) + (kf * LN2LO + c))) - f)
}

/// A slice of floats in Go's `<` order, for `sort.Slice`.
struct Float32s<'a>(&'a mut [f32]);

impl gosort::Interface for Float32s<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        self.0[i] < self.0[j]
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.0.swap(i, j);
    }
}
