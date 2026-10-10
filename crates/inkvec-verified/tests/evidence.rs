//! The generated evidence kernels (`Gen/Evidence.lean`): each form against the others, and
//! the exact facts their theorems state.

use inkvec_verified::generated::evidence::{
    corner_excess_f64, corner_excess_iv, corner_excess_q, fourth_difference_f64,
    fourth_difference_iv, fourth_difference_q, trapezoid_f64, trapezoid_iv, trapezoid_q,
    window_term_f64, window_term_iv, window_term_q,
};
use inkvec_verified::iv::Iv;
use inkvec_verified::q::{self, Q};

/// Deterministic values in `[lo, hi)`.
fn values(n: usize, lo: f64, hi: f64) -> Vec<f64> {
    let mut s: u64 = 987654321;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            lo + (hi - lo) * ((s >> 11) as f64 / (1u64 << 53) as f64)
        })
        .collect()
}

/// Every form agrees: the exact value and the float one lie in the enclosure.
fn agree<const N: usize, const M: usize>(
    x: [f64; N],
    f: fn(&[f64; N]) -> [f64; M],
    i: fn(&[Iv; N]) -> Option<[Iv; M]>,
    e: fn(&[Q; N]) -> Option<[Q; M]>,
) {
    let fx = f(&x);
    let ix = i(&x.map(|v| Iv::point(v).unwrap())).unwrap();
    let qx = e(&x.map(|v| Q::from_f64(v).unwrap()));
    for k in 0..M {
        assert!(ix[k].contains(fx[k]), "{x:?} {k}");
        if let Some(qx) = qx {
            assert!(
                qx[k].ge_f64(ix[k].lo) && qx[k].le_f64(ix[k].hi),
                "{x:?} {k}"
            );
        }
    }
}

#[test]
fn the_forms_agree() {
    let v = values(4 * 11 * 50, -3.0, 3.0);
    for c in v.as_chunks::<11>().0 {
        let x5 = [c[0], c[1], c[2], c[3], c[4]];
        agree(x5, trapezoid_f64, trapezoid_iv, trapezoid_q);
        agree(
            x5,
            fourth_difference_f64,
            fourth_difference_iv,
            fourth_difference_q,
        );
        // A variance is positive.
        agree(
            [c[0], c[1], c[2].abs() + 0.01],
            window_term_f64,
            window_term_iv,
            window_term_q,
        );
        let mut x11 = *c;
        for v in &mut x11[5..] {
            *v = v.abs();
        }
        agree(x11, corner_excess_f64, corner_excess_iv, corner_excess_q);
    }
}

#[test]
fn a_straight_piece_is_a_trapezoid() {
    // From (0, 1/2) to (1, 3/4) above 0: area 5/8, exactly.
    let r = |n: i128, d: i128| Q::new(n, d).unwrap();
    let [a] = trapezoid_q(&[r(0, 1), r(1, 2), r(1, 1), r(3, 4), r(0, 1)]).unwrap();
    assert_eq!(a, r(5, 8));
    // Walking the other way the area changes sign (`trapezoidK_integral`'s orientation).
    let [b] = trapezoid_q(&[r(1, 1), r(3, 4), r(0, 1), r(1, 2), r(0, 1)]).unwrap();
    assert_eq!(b, r(-5, 8));
}

#[test]
fn the_fourth_difference_annihilates_cubics_and_reads_a_kink() {
    // `fourthDiff_cubic`: zero on every cubic, exactly.
    let r = |n: i128, d: i128| Q::new(n, d).unwrap();
    let cubic = |s: i128| r(3 + 2 * s - 5 * s * s + 7 * s * s * s, 4);
    let x = [-2i128, -1, 0, 1, 2].map(cubic);
    let [d] = fourth_difference_q(&x).unwrap();
    assert_eq!(d, Q::int(0));
    // A slope change of 1 at the centre, read as window means (the cell averages of
    // max(0, s): 0, 0, 1/8, 1, 2), gives |D| = 5/4: the kink response.
    let x = [r(0, 1), r(0, 1), r(1, 8), r(1, 1), r(2, 1)];
    let [d] = fourth_difference_q(&x).unwrap();
    assert_eq!(d, r(-5, 4));
}

#[test]
fn the_corner_excess_decides_the_threshold() {
    // h = (0, 0, 0, 1, 2) gives D = −2; with unit variances Σ wᵢ² = 70, so
    // D² − z²·70 ≥ 0 iff z² ≤ 4/70.
    let h = [0.0, 0.0, 0.0, 1.0, 2.0];
    let at = |z2: f64| {
        let mut x = [Iv::point(1.0).unwrap(); 11];
        for k in 0..5 {
            x[k] = Iv::point(h[k]).unwrap();
        }
        x[10] = Iv::point(z2).unwrap();
        corner_excess_iv(&x).unwrap()[0]
    };
    assert!(at(4.0 / 71.0).is_nonneg());
    assert!(!at(4.0 / 69.0).is_nonneg());
    // Exactly on the threshold the excess is zero.
    let mut x = [Q::int(1); 11];
    for (k, v) in [0, 0, 0, 1, 2].into_iter().enumerate() {
        x[k] = Q::int(v);
    }
    x[10] = Q::new(4, 70).unwrap();
    let [e] = corner_excess_q(&x).unwrap();
    assert_eq!(q::cmp(e, Q::int(0)), std::cmp::Ordering::Equal);
}

#[test]
fn a_window_term_is_the_squared_standardised_residual() {
    let r = |n: i128, d: i128| Q::new(n, d).unwrap();
    let [t, res] = window_term_q(&[r(7, 4), r(5, 4), r(1, 16)]).unwrap();
    assert_eq!(res, r(1, 2));
    assert_eq!(t, Q::int(4));
}
