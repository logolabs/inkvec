//! The exact and interval runtimes' edge cases, which the kernels' own tests do not reach.

use std::cmp::Ordering::{Greater, Less};

use inkvec_verified::q::{self, Q};

#[test]
fn a_double_beyond_the_mantissa_is_an_exact_integer() {
    assert_eq!(Q::from_f64(2f64.powi(60)), Some(Q::int(1 << 60)));
    assert_eq!(
        Q::from_f64(-(2f64.powi(53) + 2.0)),
        Some(Q::int(-(1 << 53) - 2))
    );
    assert_eq!(Q::from_f64(2f64.powi(100)), Q::new(1 << 100, 1));
    // A mantissa shifted more than 70 places (from 2^123) leaves no headroom: refused.
    assert!(Q::from_f64(2f64.powi(124)).is_none());
}

#[test]
fn equal_integer_parts_are_decided_by_the_fractions() {
    let half = |n: i128| Q::new(n, 2).unwrap();
    assert_eq!(q::cmp(Q::int(2), half(5)), Less);
    assert_eq!(q::cmp(half(5), Q::int(2)), Greater);
    assert_eq!(q::cmp(half(-5), Q::int(-3)), Greater);
}
