//! Rounding the way Python and numpy do it, so the pictures come out exactly the same.

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
