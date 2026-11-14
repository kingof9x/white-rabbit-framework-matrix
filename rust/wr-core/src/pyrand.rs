//! Python's `random.Random(seed)` (the Mersenne Twister), number for number, for the effects that
//! use it (the sand battery), so a seeded Rust pile falls exactly like the Python one.
//! Source: CPython `Modules/_randommodule.c` and `Lib/random.py` (3.11+ shuffle, choice).

const N: usize = 624;
const M: usize = 397;

#[derive(Clone)]
pub struct Random {
    mt: Box<[u32; N]>,
    index: usize,
}

impl Random {
    /// `random.Random(seed)` for a non-negative integer seed.
    pub fn new(seed: u64) -> Random {
        let mut key = vec![seed as u32];
        if seed >> 32 != 0 {
            key.push((seed >> 32) as u32);
        }
        let mut r = Random { mt: Box::new([0; N]), index: N };
        r.init_genrand(19_650_218);
        let mt = &mut r.mt;
        let (mut i, mut j) = (1usize, 0usize);
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
        for _ in 0..N - 1 {
            mt[i] = (mt[i] ^ (mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
        }
        mt[0] = 0x8000_0000;
        r
    }

    /// `random.Random()`: seeded from the clock.
    pub fn from_entropy() -> Random {
        Random::new(crate::nprand::entropy_seed())
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..N {
            self.mt[i] = 1_812_433_253u32.wrapping_mul(self.mt[i - 1] ^ (self.mt[i - 1] >> 30)).wrapping_add(i as u32);
        }
        self.index = N;
    }

    pub fn next_u32(&mut self) -> u32 {
        const UPPER: u32 = 0x8000_0000;
        const LOWER: u32 = 0x7fff_ffff;
        const MATRIX_A: u32 = 0x9908_b0df;
        if self.index >= N {
            let mt = &mut self.mt;
            for k in 0..N {
                let y = (mt[k] & UPPER) | (mt[(k + 1) % N] & LOWER);
                mt[k] = mt[(k + M) % N] ^ (y >> 1) ^ if y & 1 != 0 { MATRIX_A } else { 0 };
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `random()`: a float in [0, 1).
    pub fn random(&mut self) -> f64 {
        let a = (self.next_u32() >> 5) as f64;
        let b = (self.next_u32() >> 6) as f64;
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `getrandbits(k)` for 1 <= k <= 32.
    pub fn getrandbits(&mut self, k: u32) -> u32 {
        assert!((1..=32).contains(&k));
        self.next_u32() >> (32 - k)
    }

    /// `_randbelow(n)`: 0 <= result < n (n <= 2^32).
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
    pub fn choice<T: Copy>(&mut self, seq: &[T]) -> T {
        seq[self.randbelow(seq.len())]
    }

    /// `shuffle(list)`.
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
