//! numpy's `np.random.default_rng(seed)`, number for number: SeedSequence seeding a PCG64 generator.
//!
//! The Python monitor's random effects (the unplug drain, the animations, some music looks) use numpy
//! with fixed seeds, so the Rust ones pick exactly the same numbers and draw exactly the same frames.
//! Only the calls those effects make are here. Sources: numpy `bit_generator.pyx` (SeedSequence),
//! `pcg64.h`, `distributions.c` (random_bounded_uint64_fill, the Lemire bounded integers).
//!
//! Two parts. `seed_sequence_state` turns an integer seed into well-mixed 64-bit words, the way numpy's
//! `SeedSequence` does (hash each 32-bit piece of the seed into a pool of four words, mix the pool,
//! then hash it out again). `Generator` is the PCG64 generator numpy uses by default: a 128-bit state
//! advanced by a multiply and add, with the output made by XOR-ing the two halves and rotating.
//! The odd-looking constants are numpy's own; any change would change every number.
//!
//! Pure computation: no threads, no I/O except reading the clock in `entropy_seed`.

/// SeedSequence hashing constants (`INIT_A`, `MULT_A`: hashing into the pool; `INIT_B`, `MULT_B`: hashing
/// the pool out; `MIX_MULT_*`: mixing two pool words). Values from numpy's `bit_generator.pyx`.
const INIT_A: u32 = 0x43b0_d7e5;
/// Multiplier that advances the hash constant while hashing into the pool.
const MULT_A: u32 = 0x931e_8875;
/// Starting hash constant for hashing the pool out into state words.
const INIT_B: u32 = 0x8b51_f9dd;
/// Multiplier that advances the hash constant while hashing the pool out.
const MULT_B: u32 = 0x58f3_8ded;
/// Left multiplier of `mix`.
const MIX_MULT_L: u32 = 0xca01_f9dd;
/// Right multiplier of `mix`.
const MIX_MULT_R: u32 = 0x4973_f715;
/// The right shift used after each multiply to fold the high bits back into the low ones.
const XSHIFT: u32 = 16;
/// SeedSequence's pool holds four 32-bit words.
const POOL_SIZE: usize = 4;

/// The 128-bit multiplier of PCG64's linear congruential step.
const PCG_MULT: u128 = 0x2360_ED05_1FC6_5DA4_4385_DF64_9FCC_F645;

/// SeedSequence's `hashmix`: scrambles one 32-bit word and advances the running hash constant.
fn hashmix(value: u32, hash_const: &mut u32) -> u32 {
    let mut value = value ^ *hash_const;
    *hash_const = hash_const.wrapping_mul(MULT_A);
    value = value.wrapping_mul(*hash_const);
    value ^ (value >> XSHIFT)
}

/// SeedSequence's `mix`: combines two pool words into one.
fn mix(x: u32, y: u32) -> u32 {
    let result = MIX_MULT_L.wrapping_mul(x).wrapping_sub(MIX_MULT_R.wrapping_mul(y));
    result ^ (result >> XSHIFT)
}

/// numpy's SeedSequence(seed).generate_state(n, uint64) for a non-negative integer seed.
///
/// Returns `n_words64` 64-bit words, each made from two 32-bit outputs (low word first).
fn seed_sequence_state(seed: u64, n_words64: usize) -> Vec<u64> {
    let mut entropy = vec![seed as u32];
    if seed >> 32 != 0 {
        entropy.push((seed >> 32) as u32);
    }
    // Fill the pool from the seed's 32-bit words (missing ones count as 0), then mix every pool word
    // into every other one so each depends on the whole seed.
    let mut pool = [0u32; POOL_SIZE];
    let mut hash_const = INIT_A;
    for (i, p) in pool.iter_mut().enumerate() {
        *p = hashmix(entropy.get(i).copied().unwrap_or(0), &mut hash_const);
    }
    for src in 0..POOL_SIZE {
        for dst in 0..POOL_SIZE {
            if src != dst {
                pool[dst] = mix(pool[dst], hashmix(pool[src], &mut hash_const));
            }
        }
    }
    // Seeds longer than four words (never the case here, seeds are at most 64 bits) are mixed in too.
    for &e in entropy.iter().skip(POOL_SIZE) {
        for p in pool.iter_mut() {
            *p = mix(*p, hashmix(e, &mut hash_const));
        }
    }
    // Hash the pool out into as many 32-bit words as asked for.
    let mut hash_const = INIT_B;
    let words: Vec<u32> = (0..n_words64 * 2)
        .map(|i| {
            let mut v = pool[i % POOL_SIZE] ^ hash_const;
            hash_const = hash_const.wrapping_mul(MULT_B);
            v = v.wrapping_mul(hash_const);
            v ^ (v >> XSHIFT)
        })
        .collect();
    words.chunks(2).map(|w| w[0] as u64 | (w[1] as u64) << 32).collect()
}

/// `np.random.default_rng(seed)`.
///
/// A PCG64 generator: same seed, same numbers as numpy. Cloning gives a copy that continues identically.
#[derive(Clone, Debug)]
pub struct Generator {
    /// The 128-bit LCG state.
    state: u128,
    /// The LCG increment; always odd (PCG requires it).
    inc: u128,
    /// numpy keeps the other half of a 64-bit draw for the next 32-bit one.
    spare_u32: Option<u32>,
}

impl Generator {
    /// A generator seeded like `np.random.default_rng(seed)`: four SeedSequence words give the starting
    /// state and the stream (increment), then PCG's standard "step, add, step" start-up.
    pub fn new(seed: u64) -> Generator {
        let v = seed_sequence_state(seed, 4);
        let initstate = (v[0] as u128) << 64 | v[1] as u128;
        let initseq = (v[2] as u128) << 64 | v[3] as u128;
        let mut g = Generator { state: 0, inc: (initseq << 1) | 1, spare_u32: None };
        g.step();
        g.state = g.state.wrapping_add(initstate);
        g.step();
        g
    }

    /// Seeded from the clock, for effects that are random every time (numpy's `default_rng()`).
    pub fn from_entropy() -> Generator {
        Generator::new(crate::nprand::entropy_seed())
    }

    /// Advances the LCG: state = state x multiplier + increment (mod 2^128).
    fn step(&mut self) {
        self.state = self.state.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
    }

    /// The next 64-bit output (PCG's "XSL-RR" output): XOR the state's high and low halves, then rotate
    /// right by the state's top 6 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.step();
        let s = self.state;
        let rot = (s >> 122) as u32;
        let xored = ((s >> 64) as u64) ^ (s as u64);
        xored.rotate_right(rot)
    }

    /// The next 32-bit output. Like numpy, one 64-bit draw serves two of these (low half first, high half
    /// kept in `spare_u32` for the next call), so the sequence only matches if this is copied exactly.
    pub fn next_u32(&mut self) -> u32 {
        if let Some(v) = self.spare_u32.take() {
            return v;
        }
        let next = self.next_u64();
        self.spare_u32 = Some((next >> 32) as u32);
        next as u32
    }

    /// `rng.random()`: a float in [0, 1).
    ///
    /// The top 53 bits of a 64-bit draw divided by 2^53.
    pub fn random(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `rng.random(n)` (also any shape: numpy fills it in C order, one `random()` after another).
    pub fn random_n(&mut self, n: usize) -> Vec<f64> {
        (0..n).map(|_| self.random()).collect()
    }

    /// `rng.uniform(low, high)`.
    /// One `random()` scaled to the range; `high` itself is (almost) never returned.
    pub fn uniform(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.random()
    }

    /// `rng.uniform(low, high, n)`: `n` draws in order.
    pub fn uniform_n(&mut self, low: f64, high: f64, n: usize) -> Vec<f64> {
        (0..n).map(|_| self.uniform(low, high)).collect()
    }

    /// `rng.integers(low, high)`: low <= result < high.
    /// Panics if `high <= low`. Wrapping arithmetic keeps very wide ranges (near the whole i64) correct.
    pub fn integers(&mut self, low: i64, high: i64) -> i64 {
        assert!(high > low, "integers: high must be above low");
        let rng = (high.wrapping_sub(low) as u64).wrapping_sub(1);
        low.wrapping_add(self.bounded(rng) as i64)
    }

    /// `rng.integers(low, high, n)`: `n` draws in order.
    pub fn integers_n(&mut self, low: i64, high: i64, n: usize) -> Vec<i64> {
        (0..n).map(|_| self.integers(low, high)).collect()
    }

    /// 0..=rng, numpy's unmasked (Lemire) way.
    ///
    /// Returns a value from 0 to `rng` inclusive. Lemire's method multiplies a random word by the range
    /// size and keeps the high half; the rare draws that would make some results more likely than others
    /// are rejected and redrawn. Like numpy, ranges that fit in 32 bits use 32-bit draws (so they use up
    /// half a 64-bit draw each), and the full 32- and 64-bit ranges take a raw draw directly.
    fn bounded(&mut self, rng: u64) -> u64 {
        if rng == 0 {
            0
        } else if rng < 0xFFFF_FFFF {
            let excl = rng as u32 + 1;
            let mut m = self.next_u32() as u64 * excl as u64;
            if (m as u32) < excl {
                let threshold = (u32::MAX - rng as u32) % excl;
                while (m as u32) < threshold {
                    m = self.next_u32() as u64 * excl as u64;
                }
            }
            m >> 32
        } else if rng == 0xFFFF_FFFF {
            self.next_u32() as u64
        } else if rng == u64::MAX {
            self.next_u64()
        } else {
            let excl = rng + 1;
            let mut m = self.next_u64() as u128 * excl as u128;
            if (m as u64) < excl {
                let threshold = (u64::MAX - rng) % excl;
                while (m as u64) < threshold {
                    m = self.next_u64() as u128 * excl as u128;
                }
            }
            (m >> 64) as u64
        }
    }
}

/// A seed from the clock (and a counter, so two made in the same instant differ).
///
/// Used by both `Generator::from_entropy` and `pyrand::Random::from_entropy`. The counter is multiplied
/// by the 64-bit golden-ratio constant so consecutive seeds differ in many bits. If the clock is before
/// 1970 the time part is 0 and only the counter varies.
pub fn entropy_seed() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    nanos ^ COUNT.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Values printed by numpy 2.x
    /// Proves `random`, `integers`, `uniform` and their `_n` forms give numpy 2.x's exact numbers, including
    /// a seed over 32 bits and a range wider than 32 bits.
    #[test]
    fn same_numbers_as_numpy() {
        let mut g = Generator::new(1);
        assert_eq!(g.random_n(3), [0.5118216247002567, 0.9504636963259353, 0.14415961271963373]);
        assert_eq!(g.integers_n(-1, 2, 5), [1, 1, -1, -1, 1]);
        assert_eq!(g.integers(0, 10), 4);
        assert_eq!(g.uniform(4.0, 9.0), 8.138512969102209);
        assert_eq!(g.random(), 0.4091991363691613);

        assert_eq!(Generator::new(11).uniform_n(-0.2, 0.2, 2), [-0.14857191889232016, -0.00028885502395400997]);
        assert_eq!(Generator::new(0).random(), 0.6369616873214543);

        let mut big = Generator::new((1 << 40) + 5);
        assert_eq!(big.random(), 0.004739459374490829);
        assert_eq!(big.integers_n(0, 1 << 40, 2), [409661080691, 645665582826]);
    }
}
