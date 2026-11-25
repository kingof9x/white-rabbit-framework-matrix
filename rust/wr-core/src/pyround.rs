//! Rounding the way Python and numpy do it, so the pictures come out exactly the same.
//!
//! The Rust drawing code is a port of the Python monitor and is tested pixel for pixel against pictures
//! Python saved (`tests/golden.rs`). Rust's own `f64::round` rounds halves away from zero (2.5 -> 3),
//! while Python's `round` and numpy's `rint` round halves to the even neighbour (2.5 -> 2). One pixel
//! rounded the other way would make a bar one LED longer, so every place that mirrors a Python
//! `round` / `int` uses these helpers instead of the standard ones.

/// Python's `round(x)` and numpy's `rint`: halves go to the even neighbour (2.5 -> 2, 3.5 -> 4).
pub fn round_half_even(x: f64) -> f64 {
    x.round_ties_even()
}

/// Python's `int(round(x))`.
pub fn int_round(x: f64) -> i64 {
    round_half_even(x) as i64
}

/// Python's `int(x)`: towards zero.
pub fn int(x: f64) -> i64 {
    x.trunc() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves halves round to the even neighbour (also for negatives) and `int` truncates towards zero.
    #[test]
    fn halves_go_to_even() {
        assert_eq!(int_round(0.5), 0);
        assert_eq!(int_round(1.5), 2);
        assert_eq!(int_round(2.5), 2);
        assert_eq!(int_round(-0.5), 0);
        assert_eq!(int_round(-1.5), -2);
        assert_eq!(int_round(2.6), 3);
        assert_eq!(int(-0.7), 0);
        assert_eq!(int(7.9), 7);
    }
}
