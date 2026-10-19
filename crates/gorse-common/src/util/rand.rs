// Ports: math/rand rng.go, rand.go (Rand, Source, Source64), normal.go, exp.go @ go1.27.0

//! Go's `math/rand` generator, so that seeded code produces the same numbers
//! as the Go build (`PORTING.md` D-9, risk 2).
//!
//! [`RngSource`] is `rand.NewSource`: the additive lagged Fibonacci generator
//! of Mitchell and Reeds, seeded through `rngCooked`. [`Rand`] is `rand.Rand`
//! with every method that returns a value, each consuming the source exactly
//! as Go does. The tables come from the Go source (`rand/tables.rs`, generated
//! by `contracts/tools/randgolden`), and `NormFloat64`/`ExpFloat64` use Go's
//! amd64 `math.Exp` and `math.Log` (`rand/math.rs`), so even the rare
//! rejection branches decide alike.
//!
//! Go's top-level functions (`rand.Float32`, ...) draw from a randomly seeded
//! runtime source since Go 1.20, and `rand.Seed` is a no-op since Go 1.24, so
//! nothing about them is reproducible and they are not ported here.

mod math;
#[rustfmt::skip]
mod tables;

use std::sync::{Arc, Mutex, PoisonError};

use tables::{FE, FN, KE, KN, RNG_COOKED, WE, WN};

const RNG_LEN: usize = 607;
const RNG_TAP: usize = 273;
const RNG_MASK: u64 = (1 << 63) - 1;
const INT32_MAX: i64 = (1 << 31) - 1;

/// A source of uniformly distributed non-negative 63-bit integers, Go's
/// `rand.Source`.
pub trait Source {
    /// Returns a non-negative pseudo-random 63-bit integer.
    fn int63(&mut self) -> i64;

    /// Re-initialises the source deterministically from `seed`.
    fn seed(&mut self, seed: i64);

    /// Returns a pseudo-random 64-bit value for [`Rand::uint64`].
    ///
    /// The default is what `rand.Rand.Uint64` does for a source that is not a
    /// `rand.Source64`: two calls to [`Source::int63`]. A source that is a
    /// `Source64` overrides it with its own `Uint64`.
    fn uint64(&mut self) -> u64 {
        let low = self.int63().cast_unsigned() >> 31;
        let high = self.int63().cast_unsigned() << 32;
        low | high
    }
}

/// Go's `rngSource`, the source returned by `rand.NewSource`. It is a
/// `Source64`.
#[derive(Clone)]
pub struct RngSource {
    tap: usize,
    feed: usize,
    vec: [i64; RNG_LEN],
}

/// `x[n+1] = 48271 * x[n] mod (2**31 - 1)`.
fn seedrand(x: i32) -> i32 {
    const A: i32 = 48271;
    const Q: i32 = 44488;
    const R: i32 = 3399;
    let hi = x / Q;
    let lo = x % Q;
    let mut x = A * lo - R * hi;
    if x < 0 {
        x += INT32_MAX as i32;
    }
    x
}

impl RngSource {
    /// `rand.NewSource(seed)`.
    pub fn new(seed: i64) -> Self {
        let mut source = RngSource {
            tap: 0,
            feed: 0,
            vec: [0; RNG_LEN],
        };
        source.seed(seed);
        source
    }
}

impl Source for RngSource {
    fn int63(&mut self) -> i64 {
        (self.uint64() & RNG_MASK).cast_signed()
    }

    fn seed(&mut self, seed: i64) {
        self.tap = 0;
        self.feed = RNG_LEN - RNG_TAP;
        let mut seed = seed % INT32_MAX;
        if seed < 0 {
            seed += INT32_MAX;
        }
        if seed == 0 {
            seed = 89_482_311;
        }
        // seed is now in [1, 2**31 - 2].
        let mut x = seed as i32;
        for i in -20..RNG_LEN as i32 {
            x = seedrand(x);
            if i >= 0 {
                let mut u = i64::from(x) << 40;
                x = seedrand(x);
                u ^= i64::from(x) << 20;
                x = seedrand(x);
                u ^= i64::from(x);
                u ^= RNG_COOKED[i as usize];
                self.vec[i as usize] = u;
            }
        }
    }

    fn uint64(&mut self) -> u64 {
        self.tap = if self.tap == 0 {
            RNG_LEN - 1
        } else {
            self.tap - 1
        };
        self.feed = if self.feed == 0 {
            RNG_LEN - 1
        } else {
            self.feed - 1
        };
        let x = self.vec[self.feed].wrapping_add(self.vec[self.tap]);
        self.vec[self.feed] = x;
        x.cast_unsigned()
    }
}

/// The source of `util.NewRand`: an [`RngSource`] behind a mutex, shared by
/// every clone. Like Gorse's `lockedSource` it is not a `Source64`, so
/// [`Rand::uint64`] takes two values from it.
#[derive(Clone)]
pub struct LockedSource {
    src: Arc<Mutex<RngSource>>,
}

impl LockedSource {
    /// A locked source seeded with `seed`.
    pub fn new(seed: i64) -> Self {
        LockedSource {
            src: Arc::new(Mutex::new(RngSource::new(seed))),
        }
    }
}

impl Source for LockedSource {
    fn int63(&mut self) -> i64 {
        // The generator state is valid after any panic, so a poisoned lock is
        // still usable.
        self.src
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .int63()
    }

    fn seed(&mut self, seed: i64) {
        self.src
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .seed(seed);
    }
}

/// Go's `rand.Rand`: random numbers of several kinds drawn from a [`Source`].
#[derive(Clone)]
pub struct Rand<S = RngSource> {
    src: S,
    read_val: i64,
    read_pos: i8,
}

impl Rand<RngSource> {
    /// `rand.New(rand.NewSource(seed))`.
    pub fn new(seed: i64) -> Self {
        Rand::from_source(RngSource::new(seed))
    }
}

/// `1.0 / rn` evaluated exactly and rounded once, as the Go compiler folds the
/// constant (checked against Go in the golden test).
const INV_RN: f64 = f64::from_bits(0x3fd2_972a_8afc_6175);
const RN: f64 = 3.442_619_855_899;
#[allow(clippy::excessive_precision, reason = "Go's `re` constant, verbatim")]
const RE: f64 = 7.697_117_470_131_049_72;

impl<S: Source> Rand<S> {
    /// `rand.New(src)`.
    pub fn from_source(src: S) -> Self {
        Rand {
            src,
            read_val: 0,
            read_pos: 0,
        }
    }

    /// Re-seeds the source and forgets bytes left over from [`Rand::read`].
    pub fn seed(&mut self, seed: i64) {
        self.src.seed(seed);
        self.read_pos = 0;
    }

    /// A non-negative pseudo-random 63-bit integer.
    pub fn int63(&mut self) -> i64 {
        self.src.int63()
    }

    /// A pseudo-random 32-bit value.
    pub fn uint32(&mut self) -> u32 {
        (self.int63() >> 31) as u32
    }

    /// A pseudo-random 64-bit value.
    pub fn uint64(&mut self) -> u64 {
        self.src.uint64()
    }

    /// A non-negative pseudo-random 31-bit integer.
    pub fn int31(&mut self) -> i32 {
        (self.int63() >> 32) as i32
    }

    /// A non-negative pseudo-random `int` (64 bits, as on every platform Go
    /// Gorse is built for).
    pub fn int(&mut self) -> i64 {
        self.int63()
    }

    /// A non-negative pseudo-random number in `[0, n)`.
    ///
    /// # Panics
    ///
    /// If `n <= 0`, like Go.
    pub fn int63n(&mut self, n: i64) -> i64 {
        assert!(n > 0, "invalid argument to Int63n");
        if n & (n - 1) == 0 {
            return self.int63() & (n - 1);
        }
        let max = (u64::MAX / 2 - (1 << 63) % n.cast_unsigned()) as i64;
        let mut v = self.int63();
        while v > max {
            v = self.int63();
        }
        v % n
    }

    /// A non-negative pseudo-random number in `[0, n)`.
    ///
    /// # Panics
    ///
    /// If `n <= 0`, like Go.
    pub fn int31n(&mut self, n: i32) -> i32 {
        assert!(n > 0, "invalid argument to Int31n");
        if n & (n - 1) == 0 {
            return self.int31() & (n - 1);
        }
        let max = (i32::MAX.cast_unsigned() - (1u32 << 31) % n.cast_unsigned()) as i32;
        let mut v = self.int31();
        while v > max {
            v = self.int31();
        }
        v % n
    }

    /// Go's unexported `int31n` (Lemire's method), used by
    /// [`Rand::shuffle`]; `n` must be positive.
    fn int31n_lemire(&mut self, n: i32) -> i32 {
        let n = n.cast_unsigned();
        let mut v = self.uint32();
        let mut prod = u64::from(v) * u64::from(n);
        let mut low = prod as u32;
        if low < n {
            let thresh = n.wrapping_neg() % n;
            while low < thresh {
                v = self.uint32();
                prod = u64::from(v) * u64::from(n);
                low = prod as u32;
            }
        }
        (prod >> 32) as i32
    }

    /// A non-negative pseudo-random number in `[0, n)`.
    ///
    /// # Panics
    ///
    /// If `n <= 0`, like Go.
    pub fn intn(&mut self, n: i64) -> i64 {
        assert!(n > 0, "invalid argument to Intn");
        if n <= INT32_MAX {
            return i64::from(self.int31n(n as i32));
        }
        self.int63n(n)
    }

    /// A pseudo-random number in `[0.0, 1.0)`.
    pub fn float64(&mut self) -> f64 {
        loop {
            let f = self.int63() as f64 / 9_223_372_036_854_775_808.0;
            if f != 1.0 {
                return f;
            }
        }
    }

    /// A pseudo-random number in `[0.0, 1.0)`.
    pub fn float32(&mut self) -> f32 {
        loop {
            let f = self.float64() as f32;
            if f != 1.0 {
                return f;
            }
        }
    }

    /// A pseudo-random permutation of `0..n`.
    pub fn perm(&mut self, n: usize) -> Vec<i64> {
        let mut m = vec![0; n];
        for i in 0..n {
            let j = self.intn(i as i64 + 1) as usize;
            m[i] = m[j];
            m[j] = i as i64;
        }
        m
    }

    /// Fisher-Yates shuffle of `n` elements through `swap`.
    pub fn shuffle(&mut self, n: usize, mut swap: impl FnMut(usize, usize)) {
        let mut i = n as i64 - 1;
        while i > INT32_MAX - 1 {
            let j = self.int63n(i + 1);
            swap(i as usize, j as usize);
            i -= 1;
        }
        while i > 0 {
            let j = self.int31n_lemire((i + 1) as i32);
            swap(i as usize, j as usize);
            i -= 1;
        }
    }

    /// Fills `p` with random bytes, seven per 63-bit value, keeping the bytes
    /// of the last value that were not used for the next call.
    pub fn read(&mut self, p: &mut [u8]) {
        let mut pos = self.read_pos;
        let mut val = self.read_val;
        for b in p.iter_mut() {
            if pos == 0 {
                val = self.src.int63();
                pos = 7;
            }
            *b = val as u8;
            val >>= 8;
            pos -= 1;
        }
        self.read_pos = pos;
        self.read_val = val;
    }

    /// A standard normal sample (ziggurat method of Marsaglia and Tsang).
    pub fn norm_float64(&mut self) -> f64 {
        loop {
            let j = self.uint32().cast_signed();
            let i = (j & 0x7F) as usize;
            let x = f64::from(j) * f64::from(WN[i]);
            if j.unsigned_abs() < KN[i] {
                return x;
            }
            if i == 0 {
                let mut x;
                loop {
                    x = -math::log(self.float64()) * INV_RN;
                    let y = -math::log(self.float64());
                    if y + y >= x * x {
                        break;
                    }
                }
                return if j > 0 { RN + x } else { -RN - x };
            }
            if FN[i] + self.float64() as f32 * (FN[i - 1] - FN[i]) < math::exp(-0.5 * x * x) as f32
            {
                return x;
            }
        }
    }

    /// An exponential sample with rate 1 (ziggurat method).
    pub fn exp_float64(&mut self) -> f64 {
        loop {
            let j = self.uint32();
            let i = (j & 0xFF) as usize;
            let x = f64::from(j) * f64::from(WE[i]);
            if j < KE[i] {
                return x;
            }
            if i == 0 {
                return RE - math::log(self.float64());
            }
            if FE[i] + self.float64() as f32 * (FE[i - 1] - FE[i]) < math::exp(-x) as f32 {
                return x;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seedrand_matches_the_lehmer_recurrence() {
        let mut x: i64 = 1;
        let mut y: i32 = 1;
        for _ in 0..1000 {
            x = x * 48271 % INT32_MAX;
            y = seedrand(y);
            assert_eq!(i64::from(y), x);
        }
    }

    #[test]
    fn locked_clones_share_one_generator() {
        let mut a = Rand::from_source(LockedSource::new(3));
        let mut b = a.clone();
        let mut plain = Rand::new(3);
        assert_eq!(a.int63(), plain.int63());
        assert_eq!(b.int63(), plain.int63());
        assert_eq!(a.int63(), plain.int63());
    }

    #[test]
    #[should_panic(expected = "invalid argument to Intn")]
    fn intn_rejects_zero() {
        Rand::new(0).intn(0);
    }
}
