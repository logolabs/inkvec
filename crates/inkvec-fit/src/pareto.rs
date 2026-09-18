//! Candidate-relative rate/distortion/smoothness selection.
//!
//! This is an experimental selection primitive, not a new pixel renderer or a claim
//! of global optimality over all SVGs. Callers must measure all candidates against
//! the same image formation model, covariance, support, and smoothness definition.
//! No epsilon dominance is used: it is generally not transitive.

pub mod refinement;

/// Measurements of one complete candidate drawing. All objectives are minimized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Score {
    /// Nonnegative pixel-domain loss under a fixed observation model.
    pub distortion: f64,
    /// Description length in a consistent integer unit (for example encoded bits).
    pub rate: u64,
    /// Nonnegative defect measure on regions supported as smooth by the input.
    pub wobble: f64,
}

impl Score {
    /// Reject missing, infinite, and negative measurements before ordering them.
    pub fn is_valid(self) -> bool {
        self.distortion.is_finite()
            && self.wobble.is_finite()
            && self.distortion >= 0.0
            && self.wobble >= 0.0
    }

    /// Strict Pareto dominance: no worse on any objective, better on at least one.
    pub fn dominates(self, other: Self) -> bool {
        self.is_valid()
            && other.is_valid()
            && self.rate <= other.rate
            && self.distortion <= other.distortion
            && self.wobble <= other.wobble
            && (self.rate < other.rate
                || self.distortion < other.distortion
                || self.wobble < other.wobble)
    }
}

/// Return every non-dominated valid input index, including equal-score duplicates.
///
/// O(n²) time and O(n) output storage. Intended for complete drawing proposals,
/// not for the millions of spans visited by the curve-fitting dynamic program.
pub fn frontier(scores: &[Score]) -> Vec<usize> {
    scores
        .iter()
        .enumerate()
        .filter(|(_, score)| score.is_valid())
        .filter(|(_, score)| !scores.iter().any(|other| other.dominates(**score)))
        .map(|(i, _)| i)
        .collect()
}

/// Fixed admissibility budgets, shared by every candidate.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// Maximum allowed pixel-domain loss.
    pub distortion: f64,
    /// Maximum allowed smooth-region defect.
    pub wobble: f64,
}

/// Choose the least rate within both budgets, breaking ties by distortion, wobble,
/// then input order. Returns `None` for invalid budgets or no admissible candidate.
///
/// The result is Pareto non-dominated among all valid input candidates, including
/// candidates outside the budgets. A dominator of a feasible point is feasible too.
/// Guarantees concern the supplied finite scores; the measurement code is separate.
pub fn select(scores: &[Score], budget: Budget) -> Option<usize> {
    if !budget.distortion.is_finite()
        || !budget.wobble.is_finite()
        || budget.distortion < 0.0
        || budget.wobble < 0.0
    {
        return None;
    }
    let mut best: Option<usize> = None;
    for (i, score) in scores.iter().enumerate() {
        if !score.is_valid() || score.distortion > budget.distortion || score.wobble > budget.wobble
        {
            continue;
        }
        if best.is_none_or(|j| {
            let old = scores[j];
            score.rate < old.rate
                || (score.rate == old.rate
                    && (score.distortion < old.distortion
                        || (score.distortion == old.distortion && score.wobble < old.wobble)))
        }) {
            best = Some(i);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(rate: u64, distortion: f64, wobble: f64) -> Score {
        Score {
            rate,
            distortion,
            wobble,
        }
    }

    #[test]
    fn unsupported_frontier_point_is_selected_by_budget() {
        // The middle point is Pareto-optimal, but no positive weighted sum of
        // distortion and rate can select it: lambda >= 3 and <= 1 are required.
        let scores = [s(1, 5.0, 0.0), s(2, 4.0, 0.0), s(3, 1.0, 0.0)];
        assert_eq!(frontier(&scores), vec![0, 1, 2]);
        assert_eq!(
            select(
                &scores,
                Budget {
                    distortion: 4.0,
                    wobble: 0.0
                }
            ),
            Some(1)
        );
    }

    #[test]
    fn small_pixel_gain_cannot_buy_unacceptable_wobble() {
        let scores = [s(10, 1.0, 0.0), s(20, 0.99, 4.0), s(8, 3.0, 0.0)];
        assert_eq!(frontier(&scores), vec![0, 1, 2]);
        assert_eq!(
            select(
                &scores,
                Budget {
                    distortion: 1.1,
                    wobble: 0.1
                }
            ),
            Some(0)
        );
        assert_eq!(
            select(
                &scores,
                Budget {
                    distortion: 0.995,
                    wobble: 0.1
                }
            ),
            None
        );
    }

    #[test]
    fn ties_and_invalid_measurements() {
        let scores = [
            s(1, f64::NAN, 0.0),
            s(1, 2.0, 1.0),
            s(1, 1.0, 1.0),
            s(1, 1.0, 0.0),
            s(1, 1.0, 0.0),
            s(0, f64::INFINITY, 0.0),
            s(0, -1.0, 0.0),
        ];
        assert_eq!(frontier(&scores), vec![3, 4]);
        assert_eq!(
            select(
                &scores,
                Budget {
                    distortion: 2.0,
                    wobble: 2.0
                }
            ),
            Some(3)
        );
        assert_eq!(
            select(
                &scores,
                Budget {
                    distortion: f64::NAN,
                    wobble: 2.0
                }
            ),
            None
        );
        assert_eq!(
            select(
                &[],
                Budget {
                    distortion: 1.0,
                    wobble: 1.0
                }
            ),
            None
        );
    }

    #[test]
    fn exhaustive_small_lattice_selection_is_feasible_minimal_and_undominated() {
        let lattice: Vec<_> = (0..3)
            .flat_map(|r| (0..3).flat_map(move |d| (0..3).map(move |w| s(r, d as f64, w as f64))))
            .collect();
        for a in &lattice {
            for b in &lattice {
                for c in &lattice {
                    let scores = [*a, *b, *c];
                    for d in 0..3 {
                        for w in 0..3 {
                            let budget = Budget {
                                distortion: d as f64,
                                wobble: w as f64,
                            };
                            let feasible: Vec<_> = scores
                                .iter()
                                .filter(|s| {
                                    s.distortion <= budget.distortion && s.wobble <= budget.wobble
                                })
                                .collect();
                            let chosen = select(&scores, budget);
                            assert_eq!(chosen.is_none(), feasible.is_empty());
                            if let Some(i) = chosen {
                                let winner = scores[i];
                                assert!(
                                    winner.distortion <= budget.distortion
                                        && winner.wobble <= budget.wobble
                                );
                                assert!(feasible.iter().all(|s| winner.rate <= s.rate));
                                assert!(!scores.iter().any(|s| s.dominates(winner)));
                                assert!(frontier(&scores).contains(&i));
                            }
                        }
                    }
                }
            }
        }
    }
}
