//! The generated completion kernels (`InkvecTheory.Gen.Design`) against what they mean and
//! against each other: the float, the enclosure and the exact value.

use inkvec_verified::generated::design::{
    span_link_f64, span_link_iv, span_link_q, span_within_f64, span_within_iv, span_within_q,
};
use inkvec_verified::iv::Iv;
use inkvec_verified::q::Q;

/// Coordinates on a grid of 1/64 px, so every value and every sum is exact in a double and
/// the float kernel's sign is the exact sign.
fn coords(n: usize, seed: u64) -> Vec<Vec<f64>> {
    let mut s = seed;
    let mut next = || {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((s >> 33) % 2048) as f64 / 64.0 - 16.0
    };
    (0..500).map(|_| (0..n).map(|_| next()).collect()).collect()
}

#[test]
fn span_within_is_nonneg_exactly_when_the_span_lies_in_the_widened_one() {
    for x in coords(5, 7) {
        let (a0, a1, b0, b1, s) = (x[0], x[1], x[2], x[3], x[4].abs());
        let x = [a0, a1, b0, b1, s];
        let f = span_within_f64(&x);
        let inside = a0 >= b0 - s && a1 <= b1 + s;
        assert_eq!(f[0] >= 0.0 && f[1] >= 0.0, inside, "{x:?}");
    }
}

#[test]
fn span_link_is_nonneg_exactly_when_the_widened_spans_touch() {
    for x in coords(3, 11) {
        let (p1, q0, s) = (x[0], x[1], x[2].abs());
        let x = [p1, q0, s];
        assert_eq!(span_link_f64(&x)[0] >= 0.0, p1 + s >= q0 - s, "{x:?}");
    }
}

#[test]
fn the_enclosures_hold_the_exact_value_and_the_float_one() {
    for x in coords(5, 13) {
        let x: [f64; 5] = x.try_into().unwrap();
        let iv = span_within_iv(&x.map(|v| Iv::point(v).unwrap())).unwrap();
        let q = span_within_q(&x.map(|v| Q::from_f64(v).unwrap())).unwrap();
        let f = span_within_f64(&x);
        for k in 0..2 {
            assert!(q[k].ge_f64(iv[k].lo) && q[k].le_f64(iv[k].hi), "{x:?} {k}");
            assert!(iv[k].contains(f[k]), "{x:?} {k}");
            assert_eq!(q[k].is_nonneg(), f[k] >= 0.0, "{x:?} {k}");
        }
    }
    for x in coords(3, 17) {
        let x: [f64; 3] = x.try_into().unwrap();
        let iv = span_link_iv(&x.map(|v| Iv::point(v).unwrap())).unwrap();
        let q = span_link_q(&x.map(|v| Q::from_f64(v).unwrap())).unwrap();
        let f = span_link_f64(&x);
        assert!(q[0].ge_f64(iv[0].lo) && q[0].le_f64(iv[0].hi), "{x:?}");
        assert!(iv[0].contains(f[0]), "{x:?}");
        assert_eq!(q[0].is_nonneg(), f[0] >= 0.0, "{x:?}");
    }
}

#[test]
fn an_enclosure_that_straddles_zero_decides_nothing() {
    // A span whose end may lie a hair either side of the widened one: the interval kernel
    // must not certify it. (Its start is well inside: every operation widens by a float,
    // so even an exact zero would not read as non-negative.)
    let x = [
        Iv::point(1.0).unwrap(),
        Iv::new(9.999, 10.001).unwrap(),
        Iv::point(0.0).unwrap(),
        Iv::point(10.0).unwrap(),
        Iv::point(0.0).unwrap(),
    ];
    let r = span_within_iv(&x).unwrap();
    assert!(r[0].is_nonneg());
    assert!(!r[1].is_nonneg());
    assert!(r[1].contains(0.0));
}
