//! Outward-rounded interval arithmetic in `f64`: the runtime of the generated `_iv` kernels.
//!
//! Each operation computes its endpoints in `f64` and moves each one float outward
//! (`next_down`, `next_up`). IEEE 754 rounds `+ − × ÷ √` to the nearest float, within half
//! the gap to the next one, so the widened interval contains the exact result of the
//! operation on the endpoints: the hypothesis `Inkvec.Gen.Enclosing` of the soundness theorem
//! `Inkvec.Gen.Expr.evalI_sound`. Products take the least and greatest of the four endpoint
//! products (rounding is monotone, so the least rounded product moved down is below the least
//! exact one); division multiplies by the reciprocal of a divisor interval that excludes
//! zero; a square root needs an interval of non-negative numbers. Anything else, and any
//! non-finite endpoint, is a refusal (`None`).

/// A closed interval `[lo, hi]` of reals with `f64` endpoints, `lo <= hi`, both finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Iv {
    /// Lower endpoint.
    pub lo: f64,
    /// Upper endpoint.
    pub hi: f64,
}

impl Iv {
    /// The point interval of a finite `x`: an `f64` is an exact real.
    pub fn point(x: f64) -> Option<Iv> {
        ok(x, x)
    }

    /// `[lo, hi]`, when finite and ordered.
    pub fn new(lo: f64, hi: f64) -> Option<Iv> {
        ok(lo, hi)
    }

    /// Whether `x` lies in the interval.
    pub fn contains(&self, x: f64) -> bool {
        self.lo <= x && x <= self.hi
    }

    /// Whether every number in the interval is at least zero: the verdict of a checked
    /// condition `e >= 0` (`Inkvec.Gen.Expr.nonneg_of_evalI`).
    pub fn is_nonneg(&self) -> bool {
        self.lo >= 0.0
    }

    /// Whether every number in the interval lies within `tol` of zero.
    pub fn within(&self, tol: f64) -> bool {
        -tol <= self.lo && self.hi <= tol
    }
}

fn ok(lo: f64, hi: f64) -> Option<Iv> {
    (lo.is_finite() && hi.is_finite() && lo <= hi).then_some(Iv { lo, hi })
}

/// An enclosure of the rational `num / den` (`den > 0`); `None` when either is not an
/// exactly representable `f64` integer.
pub fn lit(num: i128, den: i128) -> Option<Iv> {
    const EXACT: i128 = 1 << 53;
    if den <= 0 || num.abs() > EXACT || den > EXACT {
        return None;
    }
    let (n, d) = (num as f64, den as f64);
    if den == 1 {
        return ok(n, n);
    }
    let q = n / d;
    ok(q.next_down(), q.next_up())
}

/// `-a`, exactly.
pub fn neg(a: Iv) -> Option<Iv> {
    ok(-a.hi, -a.lo)
}

/// `a + b`.
pub fn add(a: Iv, b: Iv) -> Option<Iv> {
    ok((a.lo + b.lo).next_down(), (a.hi + b.hi).next_up())
}

/// `a − b`.
pub fn sub(a: Iv, b: Iv) -> Option<Iv> {
    ok((a.lo - b.hi).next_down(), (a.hi - b.lo).next_up())
}

/// `a · b`.
pub fn mul(a: Iv, b: Iv) -> Option<Iv> {
    let p = [a.lo * b.lo, a.lo * b.hi, a.hi * b.lo, a.hi * b.hi];
    if p.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let lo = p.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = p.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    ok(lo.next_down(), hi.next_up())
}

/// `a / b`, for a divisor interval that excludes zero.
pub fn div(a: Iv, b: Iv) -> Option<Iv> {
    if !(b.lo > 0.0 || b.hi < 0.0) {
        return None;
    }
    let r = ok((1.0 / b.hi).next_down(), (1.0 / b.lo).next_up())?;
    mul(a, r)
}

/// `√a`, for an interval of non-negative numbers.
pub fn sqrt(a: Iv) -> Option<Iv> {
    if a.lo < 0.0 {
        return None;
    }
    ok(a.lo.sqrt().next_down(), a.hi.sqrt().next_up())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::q::{self, Q};

    /// A small deterministic generator of awkward doubles.
    fn values() -> Vec<f64> {
        let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut out = vec![0.0, 1.0, -1.0, 0.1, -0.3, 1e-9, 7.0 / 3.0, 12.0, -1e6];
        for _ in 0..200 {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let m = (s >> 11) as f64 / (1u64 << 53) as f64;
            let e = ((s >> 3) % 20) as i32 - 10;
            out.push((m - 0.5) * 2f64.powi(e));
        }
        out
    }

    #[test]
    fn every_operation_encloses_the_exact_rational_result() {
        let vs = values();
        for (i, &a) in vs.iter().enumerate() {
            let b = vs[(i * 7 + 3) % vs.len()];
            let (ia, ib) = (Iv::point(a).unwrap(), Iv::point(b).unwrap());
            let (qa, qb) = (Q::from_f64(a).unwrap(), Q::from_f64(b).unwrap());
            let cases = [
                (add(ia, ib), q::add(qa, qb)),
                (sub(ia, ib), q::sub(qa, qb)),
                (mul(ia, ib), q::mul(qa, qb)),
                (div(ia, ib), q::div(qa, qb)),
            ];
            for (k, (iv, exact)) in cases.into_iter().enumerate() {
                // An endpoint too small for an i128 rational (a subnormal) cannot be
                // compared exactly; those cases are skipped.
                let comparable =
                    |iv: &Iv| Q::from_f64(iv.lo).is_some() && Q::from_f64(iv.hi).is_some();
                if let (Some(iv), Some(exact)) = (iv, exact) {
                    if !comparable(&iv) {
                        continue;
                    }
                    assert!(
                        exact.ge_f64(iv.lo) && exact.le_f64(iv.hi),
                        "op {k}: {a} {b} -> {iv:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn refusals() {
        let z = Iv::new(-1.0, 1.0).unwrap();
        assert!(div(Iv::point(1.0).unwrap(), z).is_none());
        assert!(sqrt(z).is_none());
        assert!(Iv::new(1.0, 0.0).is_none());
        assert!(Iv::point(f64::NAN).is_none());
        assert!(mul(Iv::point(1e300).unwrap(), Iv::point(1e300).unwrap()).is_none());
        assert!(lit(1, 1 << 60).is_none());
    }

    #[test]
    fn literals_and_roots_enclose() {
        let third = lit(1, 3).unwrap();
        assert!(third.lo < third.hi && third.contains(1.0 / 3.0));
        assert_eq!(lit(7, 1), Iv::point(7.0));
        let r = sqrt(Iv::point(2.0).unwrap()).unwrap();
        assert!(r.lo * r.lo <= 2.0 && r.hi * r.hi >= 2.0);
        assert!(r.within(2.0) && !r.within(1.0) && r.is_nonneg());
    }
}
