//! Exact rationals on checked `i128`: the runtime of the generated `_q` kernels.
//!
//! A [`Q`] is `num / den` in lowest terms with `den > 0`. Every operation is the field
//! operation on ℚ (`Inkvec.Gen.Expr.evalQ`) or a refusal (`None`) when a numerator or
//! denominator would leave `i128`, or on a division by zero. Every finite `f64` is a
//! rational (`Q::from_f64`, exact when it fits).

/// An exact rational `num / den`, in lowest terms, `den > 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Q {
    /// Numerator.
    pub num: i128,
    /// Denominator, positive.
    pub den: i128,
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Q {
    /// The integer `n`.
    pub fn int(n: i64) -> Q {
        Q {
            num: n as i128,
            den: 1,
        }
    }

    /// `num / den` in lowest terms; `None` for `den == 0`.
    pub fn new(num: i128, den: i128) -> Option<Q> {
        if den == 0 {
            return None;
        }
        let g = gcd(num, den).max(1);
        let (mut n, mut d) = (num / g, den / g);
        if d < 0 {
            n = n.checked_neg()?;
            d = d.checked_neg()?;
        }
        Some(Q { num: n, den: d })
    }

    /// The exact value of a finite `f64`, when its numerator and denominator fit `i128`.
    pub fn from_f64(x: f64) -> Option<Q> {
        if !x.is_finite() {
            return None;
        }
        if x == 0.0 {
            return Some(Q::int(0));
        }
        let bits = x.to_bits();
        let sign = if bits >> 63 == 1 { -1i128 } else { 1 };
        let exp = ((bits >> 52) & 0x7ff) as i32;
        let frac = (bits & ((1u64 << 52) - 1)) as i128;
        let (mant, e) = if exp == 0 {
            (frac, -1074)
        } else {
            (frac | (1i128 << 52), exp - 1075)
        };
        if e >= 0 {
            if e > 70 {
                return None;
            }
            Q::new(sign * (mant << e), 1)
        } else {
            let k = (-e) as u32;
            if k > 126 {
                return None;
            }
            Q::new(sign * mant, 1i128 << k)
        }
    }

    /// Whether `self >= x` for a finite `f64` `x`, exactly; `false` when `x` has no `i128`
    /// rational form (a subnormal, or beyond 2^70).
    pub fn ge_f64(&self, x: f64) -> bool {
        Q::from_f64(x).is_some_and(|q| cmp(*self, q).is_ge())
    }

    /// Whether `self <= x` for a finite `f64` `x`, exactly; `false` when `x` has no `i128`
    /// rational form.
    pub fn le_f64(&self, x: f64) -> bool {
        Q::from_f64(x).is_some_and(|q| cmp(*self, q).is_le())
    }

    /// Whether the value is at least zero.
    pub fn is_nonneg(&self) -> bool {
        self.num >= 0
    }
}

/// `a` against `b`, exactly and without overflow: integer parts first, then the
/// reciprocals of the fractional parts in reverse order (Euclid's algorithm, as continued
/// fractions compare).
pub fn cmp(a: Q, b: Q) -> std::cmp::Ordering {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let (mut an, mut ad, mut bn, mut bd) = (a.num, a.den, b.num, b.den);
    let mut flip = false;
    loop {
        let (qa, ra) = (an.div_euclid(ad), an.rem_euclid(ad));
        let (qb, rb) = (bn.div_euclid(bd), bn.rem_euclid(bd));
        let o = if qa != qb {
            qa.cmp(&qb)
        } else if ra == 0 || rb == 0 {
            match (ra == 0, rb == 0) {
                (true, true) => Equal,
                (true, false) => Less,
                _ => Greater,
            }
        } else {
            // ra/ad against rb/bd is ad/ra against bd/rb, the other way round.
            (an, ad, bn, bd) = (ad, ra, bd, rb);
            flip = !flip;
            continue;
        };
        return if flip { o.reverse() } else { o };
    }
}

/// The literal `num / den`.
pub fn lit(num: i128, den: i128) -> Option<Q> {
    Q::new(num, den)
}

/// `-a`.
pub fn neg(a: Q) -> Option<Q> {
    Some(Q {
        num: a.num.checked_neg()?,
        den: a.den,
    })
}

/// `a + b`.
pub fn add(a: Q, b: Q) -> Option<Q> {
    let n = a
        .num
        .checked_mul(b.den)?
        .checked_add(b.num.checked_mul(a.den)?)?;
    Q::new(n, a.den.checked_mul(b.den)?)
}

/// `a − b`.
pub fn sub(a: Q, b: Q) -> Option<Q> {
    add(a, neg(b)?)
}

/// `a · b`.
pub fn mul(a: Q, b: Q) -> Option<Q> {
    // Cross-reduce first so products stay small.
    let g1 = gcd(a.num, b.den).max(1);
    let g2 = gcd(b.num, a.den).max(1);
    let n = (a.num / g1).checked_mul(b.num / g2)?;
    let d = (a.den / g2).checked_mul(b.den / g1)?;
    Q::new(n, d)
}

/// `a / b`; `None` for `b == 0`.
pub fn div(a: Q, b: Q) -> Option<Q> {
    if b.num == 0 {
        return None;
    }
    mul(a, Q::new(b.den, b.num)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_operations() {
        let a = Q::new(1, 3).unwrap();
        let b = Q::new(-1, 6).unwrap();
        assert_eq!(add(a, b), Q::new(1, 6));
        assert_eq!(sub(a, b), Q::new(1, 2));
        assert_eq!(mul(a, b), Q::new(-1, 18));
        assert_eq!(div(a, b), Q::new(-2, 1));
        assert_eq!(div(a, Q::int(0)), None);
        assert_eq!(Q::new(4, -8), Q::new(-1, 2));
        assert!(Q::new(1, 0).is_none());
        assert!(add(Q::new(i128::MAX, 1).unwrap(), Q::int(1)).is_none());
        assert_eq!(cmp(a, b), std::cmp::Ordering::Greater);
        assert_eq!(cmp(b, a), std::cmp::Ordering::Less);
        assert_eq!(cmp(a, Q::new(2, 6).unwrap()), std::cmp::Ordering::Equal);
        // Cross products far beyond i128: compared exactly all the same.
        let big = Q::new(i128::MAX - 1, i128::MAX).unwrap();
        let bigger = Q::new(i128::MAX - 2, i128::MAX - 1).unwrap();
        assert_eq!(cmp(big, bigger), std::cmp::Ordering::Greater);
        assert_eq!(
            cmp(Q::int(-3), Q::new(-5, 2).unwrap()),
            std::cmp::Ordering::Less
        );
        assert!(a.is_nonneg() && !b.is_nonneg());
    }

    #[test]
    fn doubles_are_exact_rationals() {
        assert_eq!(Q::from_f64(0.5), Q::new(1, 2));
        assert_eq!(Q::from_f64(-3.0), Some(Q::int(-3)));
        assert_eq!(
            Q::from_f64(0.1),
            Q::new(3602879701896397, 36028797018963968)
        );
        assert!(Q::from_f64(f64::NAN).is_none());
        assert!(Q::from_f64(1e300).is_none());
        let q = Q::from_f64(0.1).unwrap();
        assert!(q.ge_f64(0.1) && q.le_f64(0.1) && !q.le_f64(0.0999));
    }
}
