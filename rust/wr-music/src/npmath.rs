//! The few numpy functions the looks use, computed the way numpy computes them, so the pictures match
//! the Python ones (`tests/golden/music_looks.json`) to the last bit wherever that matters.
//!
//! Why this matters: floating-point addition is not associative, so adding the same numbers in another
//! order can change the last bits of the result. A changed last bit can flip a rounding (a bar 16.5
//! levels tall becomes 16 or 17), and then the golden test fails. So `sum` follows numpy's exact
//! order of additions, `percentile` its exact interpolation, and so on. All functions are pure.

/// `np.sum` of a float64 array: numpy adds in pairs (eight running sums, then halves for long
/// arrays), which rounds differently from adding one by one.
pub fn sum(a: &[f64]) -> f64 {
    // numpy's block size: arrays up to 128 values use eight running sums; longer ones are split in two
    // (at a multiple of 8) and each half summed the same way, recursively.
    const BLOCK: usize = 128;
    let n = a.len();
    if n < 8 {
        // numpy starts from -0.0 here, so an empty or all -0.0 sum stays -0.0; it makes no difference to us
        let mut res = 0.0;
        for &v in a {
            res += v;
        }
        res
    } else if n <= BLOCK {
        // Eight running sums, one per position modulo 8, over the largest multiple of 8 values ...
        let mut r = [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
        let mut i = 8;
        while i < n - (n % 8) {
            for (j, rj) in r.iter_mut().enumerate() {
                *rj += a[i + j];
            }
            i += 8;
        }
        // ... combined pairwise, then the leftover (fewer than 8) values added one by one.
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        sum(&a[..n2]) + sum(&a[n2..])
    }
}

/// `np.mean`.
/// NaN for an empty slice (0 / 0), as numpy gives (with a warning).
pub fn mean(a: &[f64]) -> f64 {
    sum(a) / a.len() as f64
}

/// `a.max()` (NaN-free input).
/// Minus infinity for an empty slice (numpy would raise instead).
pub fn max(a: &[f64]) -> f64 {
    a.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

/// numpy's `_lerp`: the same weighted average, worked out from whichever end is nearer.
/// `t` is the fraction of the way from `a` to `b` (0..1). Working from the nearer end keeps the result
/// exact at both ends and matches numpy's rounding.
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    let diff = b - a;
    if t >= 0.5 {
        b - diff * (1.0 - t)
    } else {
        a + diff * t
    }
}

/// `np.percentile(values, q)` with the default (linear) method.
/// `q` is 0..100. The value `q`% of the way through the sorted values, interpolating between the two
/// nearest. Panics on an empty slice (callers always pass at least one value).
pub fn percentile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    // `total_cmp` gives a full order even with NaN, so the sort can't panic.
    sorted.sort_by(|a, b| a.total_cmp(b));
    let n = sorted.len();
    let virtual_index = (n - 1) as f64 * (q / 100.0);
    let below = virtual_index.floor();
    let gamma = virtual_index - below;
    let i = (below as usize).min(n - 1);
    let j = (i + 1).min(n - 1);
    lerp(sorted[i], sorted[j], gamma)
}

/// `np.linspace(start, stop, num)`.
/// `num` evenly spaced values from `start` to `stop`, both included; computed as `i * step + start`
/// like numpy, with the last value set to `stop` exactly.
pub fn linspace(start: f64, stop: f64, num: usize) -> Vec<f64> {
    if num == 0 {
        return Vec::new();
    }
    if num == 1 {
        return vec![start];
    }
    let step = (stop - start) / (num - 1) as f64;
    let mut out: Vec<f64> = (0..num).map(|i| i as f64 * step + start).collect();
    out[num - 1] = stop;
    out
}

/// `np.linspace(0, len - 1, count).astype(int)`: evenly spread indexes into a slice of length len.
/// `len` must be at least 1. Used to pick 34 points out of the recent sound wave.
pub fn spread_indexes(len: usize, count: usize) -> Vec<usize> {
    linspace(0.0, (len - 1) as f64, count).into_iter().map(|v| v as usize).collect()
}

/// `np.geomspace(start, stop, num)`.
/// `num` values from `start` to `stop` with a constant ratio between neighbours (evenly spaced on a
/// log scale), with both ends set exactly. Both must be positive. Used for the analyzer's band edges.
pub fn geomspace(start: f64, stop: f64, num: usize) -> Vec<f64> {
    let mut out: Vec<f64> = linspace(start.log10(), stop.log10(), num).into_iter().map(|e| 10f64.powf(e)).collect();
    if let Some(first) = out.first_mut() {
        *first = start;
    }
    if num > 1 {
        out[num - 1] = stop;
    }
    out
}

/// `np.hanning(m)`.
/// The Hann window of `m` points: 0 at both ends, 1 in the middle, a raised cosine in between. Used to
/// fade the ends of each block of sound before the FFT. Empty for `m` = 0.
pub fn hanning(m: usize) -> Vec<f64> {
    if m == 1 {
        return vec![1.0];
    }
    let pi = std::f64::consts::PI;
    (0..m)
        .map(|i| {
            let n = 1.0 - m as f64 + 2.0 * i as f64;
            0.5 + 0.5 * (pi * n / (m - 1) as f64).cos()
        })
        .collect()
}

/// `np.clip(v, low, high)`.
/// Written as max then min (not `clamp`) so a NaN passes through like numpy instead of panicking.
pub fn clip(v: f64, low: f64, high: f64) -> f64 {
    v.max(low).min(high)
}

/// Python's `x % m` for floats (the result takes the sign of m).
/// Rust's `%` keeps the sign of `x` (-0.25 % 1.0 = -0.25); Python's gives 0.75, which the looks rely on
/// for angles and repeating patterns.
pub fn py_mod(x: f64, m: f64) -> f64 {
    let r = x % m;
    if r != 0.0 && ((r < 0.0) != (m < 0.0)) {
        r + m
    } else {
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves `sum` and `mean` give the plain result where rounding can't differ (whole and half numbers).
    #[test]
    fn pairwise_sum_matches_plain_sum_for_small_arrays() {
        let a: Vec<f64> = (0..300).map(|i| i as f64 * 0.5).collect();
        assert_eq!(sum(&a), a.iter().sum::<f64>());
        assert_eq!(sum(&[1.0, 2.0]), 3.0);
        assert_eq!(mean(&[1.0, 2.0, 3.0, 4.0]), 2.5);
    }

    /// Proves the linear interpolation of `percentile`, including on unsorted input.
    #[test]
    fn percentile_interpolates_linearly() {
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 50.0), 3.0);
        assert!((percentile(&[0.0, 10.0], 30.0) - 3.0).abs() < 1e-12);
        assert!((percentile(&[4.0, 0.0, 10.0, 2.0], 98.0) - 9.64).abs() < 1e-12);
    }

    /// Proves `spread_indexes`, the exact ends of `geomspace`, the shape of `hanning`, and `py_mod`'s sign.
    #[test]
    fn spaces_and_windows() {
        assert_eq!(spread_indexes(1024, 34)[1], 31);
        assert_eq!(*spread_indexes(1024, 34).last().unwrap(), 1023);
        let g = geomspace(22.3, 11314.0, 10);
        assert_eq!(g[0], 22.3);
        assert_eq!(g[9], 11314.0);
        let w = hanning(5);
        assert!((w[2] - 1.0).abs() < 1e-15 && w[0].abs() < 1e-15);
        assert_eq!(py_mod(-0.25, 1.0), 0.75);
    }
}
