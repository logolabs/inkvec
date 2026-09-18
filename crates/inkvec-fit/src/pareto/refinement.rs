//! Conservative acceptance of a proposed refinement, for any fixed metric set.
//!
//! Bounds concern candidate-minus-incumbent measurements with identical support,
//! scale, renderer and uncertainty model. Empirical confidence intervals confer
//! only their stated coverage, not deterministic guarantees. Missing/unsupported
//! metrics fail closed. This module does not estimate these bounds itself.

/// An externally established interval for a paired metric difference.
#[derive(Clone, Copy, Debug)]
pub struct Delta {
    /// Lower bound on candidate-minus-incumbent change.
    pub lower: f64,
    /// Upper bound on candidate-minus-incumbent change.
    pub upper: f64,
}

/// Keep the incumbent unless every metric is known not to increase and at least
/// one strictly decreases. Negate maximization metrics before making bounds.
///
/// Zero-width intervals can represent exact discrete/deterministic measurements.
/// Do not round a small positive upper bound down to zero: tolerances accumulate
/// over repeated refinements and would invalidate monotonicity.
pub fn accepts(deltas: &[Option<Delta>]) -> bool {
    !deltas.is_empty()
        && deltas.iter().all(|d| {
            d.is_some_and(|d| {
                d.lower.is_finite() && d.upper.is_finite() && d.lower <= d.upper && d.upper <= 0.0
            })
        })
        && deltas.iter().any(|d| d.is_some_and(|d| d.upper < 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(lo: f64, hi: f64) -> Option<Delta> {
        Some(Delta {
            lower: lo,
            upper: hi,
        })
    }

    #[test]
    fn tradeoffs_uncertainty_and_missing_evidence_block_changes() {
        assert!(accepts(&[d(-2., -1.), d(0., 0.)]));
        assert!(!accepts(&[d(-2., -1.), d(-1., 0.001)]));
        assert!(!accepts(&[d(-2., -1.), None]));
        assert!(!accepts(&[d(0., 0.)]));
        assert!(!accepts(&[]));
        assert!(!accepts(&[d(-1., f64::NAN)]));
        assert!(!accepts(&[d(f64::NEG_INFINITY, -1.)]));
        assert!(!accepts(&[d(0., -1.)]));
    }

    #[test]
    fn exhaustive_intervals_preserve_every_contained_metric_change() {
        let intervals: Vec<_> = (-3..=3)
            .flat_map(|lo| (lo..=3).map(move |hi| (lo, hi)))
            .collect();
        for &(al, au) in &intervals {
            for &(bl, bu) in &intervals {
                let accepted = accepts(&[d(al as f64, au as f64), d(bl as f64, bu as f64)]);
                let all_safe =
                    (al..=au).all(|a| (bl..=bu).all(|b| a <= 0 && b <= 0 && (a < 0 || b < 0)));
                assert_eq!(accepted, all_safe);
            }
        }
    }
}
