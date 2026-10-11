//! The generated strip kernels against the formula they replaced and against each other.

use inkvec_verified::generated::strip::{
    histopolate_f64, histopolate_iv, histopolate_q, strip_residual_f64, strip_residual_iv,
};
use inkvec_verified::iv::Iv;
use inkvec_verified::q::{self, Q};

/// `strip.rs`'s formula before it was generated.
fn by_hand(m: &[f64; 4]) -> [f64; 4] {
    [
        (-m[0] + 7.0 * m[1] + 7.0 * m[2] - m[3]) / 12.0,
        (m[0] - 15.0 * m[1] + 15.0 * m[2] - m[3]) / 12.0,
        (m[0] - m[1] - m[2] + m[3]) / 4.0,
        (-m[0] + 3.0 * m[1] - 3.0 * m[2] + m[3]) / 6.0,
    ]
}

fn means() -> Vec<[f64; 4]> {
    let mut s: u64 = 12345;
    let mut next = || {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((s >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 6.0
    };
    (0..500).map(|_| [next(), next(), next(), next()]).collect()
}

#[test]
fn the_generated_formula_is_bit_for_bit_the_one_it_replaced() {
    for m in means() {
        let (a, b) = (histopolate_f64(&m), by_hand(&m));
        for k in 0..4 {
            assert_eq!(a[k].to_bits(), b[k].to_bits(), "{m:?}");
        }
    }
}

#[test]
fn the_enclosure_holds_the_exact_value_and_the_float_one() {
    for m in means() {
        let iv = histopolate_iv(&m.map(|v| Iv::point(v).unwrap())).unwrap();
        let q = histopolate_q(&m.map(|v| Q::from_f64(v).unwrap())).unwrap();
        let f = histopolate_f64(&m);
        for k in 0..4 {
            assert!(q[k].ge_f64(iv[k].lo) && q[k].le_f64(iv[k].hi), "{m:?} {k}");
            assert!(iv[k].contains(f[k]), "{m:?} {k}");
        }
    }
}

#[test]
fn a_cubic_is_rebuilt_exactly_and_its_crossing_certified() {
    // The cell means of q(s) = 1/4 + s/2 − s²/8 + s³/16 over [-2,-1] … [1,2], computed
    // exactly; histopolate returns q exactly (`histopolate_cubic`).
    let r = |n: i128, d: i128| Q::new(n, d).unwrap();
    let c = [r(1, 4), r(1, 2), r(-1, 8), r(1, 16)];
    // The antiderivative c₀s + c₁s²/2 + c₂s³/3 + c₃s⁴/4 at an integer s.
    let prim = |s: i128| {
        let terms = [
            q::mul(c[0], r(s, 1)).unwrap(),
            q::mul(c[1], r(s * s, 2)).unwrap(),
            q::mul(c[2], r(s * s * s, 3)).unwrap(),
            q::mul(c[3], r(s * s * s * s, 4)).unwrap(),
        ];
        terms
            .into_iter()
            .fold(Q::int(0), |a, t| q::add(a, t).unwrap())
    };
    let m: [Q; 4] = [-2i128, -1, 0, 1].map(|k| q::sub(prim(k + 1), prim(k)).unwrap());
    let back = histopolate_q(&m).unwrap();
    assert_eq!(back, c);
    // Where the vertical through s₀ = 0.3 meets q: t = q(0.3). The means as doubles
    // (exact here: dyadic denominators) feed the float and interval kernels.
    let mf = m.map(|v| v.num as f64 / v.den as f64);
    let cf = [0.25, 0.5, -0.125, 0.0625];
    let s0 = 0.3;
    let t = cf[0] + s0 * (cf[1] + s0 * (cf[2] + s0 * cf[3]));
    let x = [mf[0], mf[1], mf[2], mf[3], s0, 0.0, 1.0, t];
    assert!(strip_residual_f64(&x)[0].abs() < 1e-12);
    let res = strip_residual_iv(&x.map(|v| Iv::point(v).unwrap())).unwrap();
    assert!(res[0].within(1e-12));
    // A shift 0.01 off is refused.
    let mut off = x;
    off[7] += 0.01;
    let res = strip_residual_iv(&off.map(|v| Iv::point(v).unwrap())).unwrap();
    assert!(!res[0].within(1e-9));
}
