//! Python's `random.Random(seed)` (the Mersenne Twister), number for number, for the effects that
//! use it (the sand battery), so a seeded Rust pile falls exactly like the Python one.
//! Source: CPython `Modules/_randommodule.c` and `Lib/random.py` (3.11+ shuffle, choice).
//!
//! Why not a Rust random crate: the tests compare frames with ones the Python monitor saved, and those
//! depend on the exact sequence of random numbers. So this is a line-by-line port of MT19937 (the
//! "Mersenne Twister" with 624 words of state) plus the few Python helpers built on it. The constants
//! below are the standard MT19937 ones; changing any of them would change every number.
//!
//! Pure computation: no threads, no I/O except `from_entropy`, which reads the clock.

/// Number of 32-bit words in the Mersenne Twister state.
const N: usize = 624;
/// The "middle word" offset MT19937 mixes in when it regenerates the state.
const M: usize = 397;

/// One Python-compatible random number generator. Cloning it gives a copy that will produce the same
/// numbers from here on.
#[derive(Clone)]
pub struct Random {
    /// The 624-word state (boxed: 2.5 KB is a lot to copy around on the stack).
    mt: Box<[u32; N]>,
    /// The next word of `mt` to hand out; `N` means "regenerate the whole state first".
    index: usize,
}

impl Random {
    /// `random.Random(seed)` for a non-negative integer seed.
    ///
    /// Port of CPython's `init_by_array`: the seed is split into 32-bit words (low word first, a second
    /// word only when the seed needs it) and stirred into a state made by `init_genrand(19650218)`.
    pub fn new(seed: u64) -> Random {
        // Python seeds from the integer's absolute value as an array of 32-bit words, least significant
        // first; a seed below 2^32 is a one-word key.
        let mut key = vec![seed as u32];
        if seed >> 32 != 0 {
            key.push((seed >> 32) as u32);
        }
        let mut r = Random { mt: Box::new([0; N]), index: N };
        r.init_genrand(19_650_218);
        let mt = &mut r.mt;
        let (mut i, mut j) = (1usize, 0usize);
        // First pass: mix each key word into the state (1664525 is the MT reference multiplier).
        for _ in 0..N.max(key.len()) {
            mt[i] = (mt[i] ^ (mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        // Second pass: scramble the whole state again (1566083941 is the other reference multiplier).
        for _ in 0..N - 1 {
            mt[i] = (mt[i] ^ (mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
        }
        // Guarantees the state is not all zero, as the reference code does.
        mt[0] = 0x8000_0000;
        r
    }

    /// `random.Random()`: seeded from the clock.
    pub fn from_entropy() -> Random {
        Random::new(crate::nprand::entropy_seed())
    }

    /// The basic MT19937 seeding from one 32-bit number (`init_genrand` in the C source); `new` builds on it.
    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..N {
            self.mt[i] = 1_812_433_253u32.wrapping_mul(self.mt[i - 1] ^ (self.mt[i - 1] >> 30)).wrapping_add(i as u32);
        }
        self.index = N;
    }

    /// The next raw 32-bit output (`genrand_uint32` in the C source). Every 624 calls the whole state is
    /// regenerated ("twisted"); each output is then "tempered" with the fixed shifts and masks below.
    pub fn next_u32(&mut self) -> u32 {
        const UPPER: u32 = 0x8000_0000;
        const LOWER: u32 = 0x7fff_ffff;
        const MATRIX_A: u32 = 0x9908_b0df;
        if self.index >= N {
            let mt = &mut self.mt;
            // Combine the top bit of one word with the low 31 bits of the next, and XOR in the word
            // M places ahead, plus MATRIX_A when the combined value is odd.
            for k in 0..N {
                let y = (mt[k] & UPPER) | (mt[(k + 1) % N] & LOWER);
                mt[k] = mt[(k + M) % N] ^ (y >> 1) ^ if y & 1 != 0 { MATRIX_A } else { 0 };
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        // Tempering: the four standard MT19937 shift/mask steps that spread the bits out.
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `random()`: a float in [0, 1).
    ///
    /// As CPython does it: 27 random bits and 26 random bits make a 53-bit integer, divided by 2^53
    /// (67108864 is 2^26 and 9007199254740992 is 2^53), so every double in the result is equally likely.
    pub fn random(&mut self) -> f64 {
        let a = (self.next_u32() >> 5) as f64;
        let b = (self.next_u32() >> 6) as f64;
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `getrandbits(k)` for 1 <= k <= 32.
    /// The top `k` bits of one 32-bit output. Panics if `k` is 0 or above 32.
    pub fn getrandbits(&mut self, k: u32) -> u32 {
        assert!((1..=32).contains(&k));
        self.next_u32() >> (32 - k)
    }

    /// `_randbelow(n)`: 0 <= result < n (n <= 2^32).
    ///
    /// Python's rejection method: draw as many bits as `n` needs and try again while the number is too
    /// big, so every result is equally likely. Returns 0 for `n == 0` without drawing anything.
    /// In practice `n` must be below 2^32: at exactly 2^32 it would ask for 33 bits and panic.
    pub fn randbelow(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        // Even n == 1 draws numbers (getrandbits(1) until it gives 0), like Python does
        let k = usize::BITS - n.leading_zeros();
        loop {
            let r = self.getrandbits(k) as usize;
            if r < n {
                return r;
            }
        }
    }

    /// `choice(seq)`.
    /// Panics if `seq` is empty (index 0 of an empty slice), as Python raises an error there.
    pub fn choice<T: Copy>(&mut self, seq: &[T]) -> T {
        seq[self.randbelow(seq.len())]
    }

    /// `shuffle(list)`.
    /// The Fisher-Yates shuffle, walking from the end as Python 3.11+ does, so the order matches Python.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.randbelow(i + 1);
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Values printed by CPython 3.12
    /// Proves `random`, `choice`, `shuffle` and `getrandbits` give exactly CPython's numbers, for small
    /// and for over-32-bit seeds, and that a one-item `choice` still consumes random numbers.
    #[test]
    fn same_numbers_as_python() {
        let mut r = Random::new(7);
        assert_eq!(r.random(), 0.32383276483316237);
        assert_eq!(r.choice(&[1, 2, 3, 4, 5]), 2);
        let mut l: Vec<i32> = (0..7).collect();
        r.shuffle(&mut l);
        assert_eq!(l, [1, 6, 2, 4, 0, 5, 3]);
        assert_eq!(r.getrandbits(5), 11);
        assert_eq!(Random::new(0).random(), 0.8444218515250481);
        assert_eq!(Random::new((1 << 40) + 5).random(), 0.5043802970418443);
        // A choice from one item still uses up random numbers
        let mut r = Random::new(1);
        assert_eq!(r.choice(&[5]), 5);
        assert_eq!(r.random(), 0.5692038748222122);
    }
}
