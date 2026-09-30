//! The G1 cubic scored for the dynamic program, which rarely needs its exact residual.
//!
//! [`best_cubic`] scores every admissible root of Levien's quartic in full. The program
//! (`crate::multimodel::scan`) reads the result in three places only — whether the cubic
//! can beat the best candidate at its end, whether it decides the ellipse's price gate,
//! and whether its residual is over the search cut-off's floor — so [`best_cubic_bounded`]
//! stops each root's sum as soon as those are settled ([`Abandon`]), and says which
//! answers it is sure of ([`G1Fit`]).

use super::*;

impl CubicSamples {
    /// [`Self::chi2`] with a second way to stop: as soon as the partial sum shows the
    /// dynamic program has no use for this root's exact residual ([`Abandon::drops`]).
    ///
    /// The terms are the same expressions summed in the same order as [`Self::chi2`], so a
    /// root that runs to the end has its residual bit for bit. What comes back with
    /// [`RootEnd::Best`] or [`RootEnd::Bound`] is a partial sum, a lower bound on the
    /// residual because every term is non-negative.
    fn chi2_until(&self, cb: &Cubic, best: f64, ab: &Abandon) -> (f64, RootEnd) {
        let mut acc = 0.0;
        for m in (0..self.len).step_by(LANES) {
            #[cfg(test)]
            count_projections();
            let d2 = cb.dist2_lanes(lanes(&self.pt, m), lanes(&self.t, m));
            for (q, d2) in d2.iter().enumerate().take(self.len - m) {
                acc += self.weight * d2 / self.s2[m + q];
                if acc >= best {
                    return (acc, RootEnd::Best);
                }
                if acc >= ab.trigger && ab.drops(acc) {
                    return (acc, RootEnd::Bound);
                }
            }
        }
        (acc, RootEnd::Done)
    }
}

/// How the scoring of one root of [`best_cubic_bounded`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootEnd {
    /// Every sample was summed: the residual is exact.
    Done,
    /// The partial sum reached the best root's residual, the rule [`best_cubic`] has.
    Best,
    /// The partial sum proved the programme cannot use this root ([`Abandon::drops`]).
    Bound,
}

/// When the dynamic program no longer needs a G1 cubic's exact residual: the bound half of
/// branch and bound inside a dynamic program, applied one candidate at a time, with the
/// residual abandoned early as its partial sum grows.
///
/// - Morin, T. L. & Marsten, R. E. (1976), "Branch-and-bound strategies for dynamic
///   programming", *Operations Research* 24(4):611–627, doi:10.1287/opre.24.4.611:
///   discard a state whose relaxation (a lower bound) already exceeds the best known
///   solution.
/// - Killick, R., Fearnhead, P. & Eckley, I. A. (2012), "Optimal detection of changepoints
///   with a linear computational cost", *JASA* 107:1590–1598,
///   doi:10.1080/01621459.2012.737745: in optimal partitioning, a candidate last change
///   `τ` with `F(τ) + C(τ..t) ≥ F(t)` cannot be the minimiser at `t`. Only this
///   per-candidate test is used; PELT's permanent pruning needs its condition (4), which
///   the fixed-tangent G1 costs here violate (a sub-span often has no admissible cubic).
/// - Rakthanmanon, T. et al. (2012), "Searching and mining trillions of time series
///   subsequences under dynamic time warping", *KDD '12* 262–270,
///   doi:10.1145/2339530.2339576, §4.1.3: stop summing a distance of non-negative terms
///   once the partial sum exceeds the best so far.
///
/// Adapted: the "best so far" is the table's cost of reaching the span's end, and the
/// residual is dropped only when *every* use the programme makes of it is settled by the
/// partial sum: the cubic cannot be accepted, it cannot decide the ellipse's price gate,
/// and the search cut-off's "over" test is already answered.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Abandon {
    /// A lower bound (px-free nats) on the cost of reaching the span's start and leaving it.
    base: f64,
    /// An upper bound on the table's cost at the span's end when the span is offered.
    best: f64,
    /// `λ·params_cubic()`, the cubic's parameter price.
    cubic_floor: f64,
    /// The partial sum must reach this before a root may be dropped; non-zero where the
    /// ellipse is still in play and the cubic's cost could otherwise decide its gate.
    gate: f64,
    /// The search cut-off's per-span floor, when its answer is needed (the line is over):
    /// a root may be dropped only once `½·acc` exceeds it. `−∞` when it is not needed.
    over_floor: f64,
    /// Cheap pre-test on the partial sum below which [`Self::drops`] is not asked. It only
    /// saves arithmetic: any value is exact, a poor one merely drops later.
    trigger: f64,
}

impl Abandon {
    /// Never drop a root: [`best_cubic_bounded`] is then [`best_cubic`].
    pub(crate) const NONE: Abandon = Abandon {
        base: 0.0,
        best: f64::INFINITY,
        cubic_floor: 0.0,
        gate: 0.0,
        over_floor: f64::NEG_INFINITY,
        trigger: f64::INFINITY,
    };

    /// The bound for one candidate span: `base` at most the cost of reaching its start,
    /// `best` at least the table's value at its end, `gate` and `over_floor` as the fields.
    pub(crate) fn new(base: f64, best: f64, cubic_floor: f64, gate: f64, over_floor: f64) -> Self {
        let dead_at = 2.0 * (best - base - cubic_floor);
        let trigger = if best == f64::INFINITY {
            f64::INFINITY
        } else {
            // A little under each threshold, so rounding in this estimate never delays a
            // drop by more than a sample or two; the exact test is `drops`.
            let t = dead_at.max(gate).max(2.0 * over_floor);
            t - 1e-9 * t.abs()
        };
        Abandon {
            base,
            best,
            cubic_floor,
            gate,
            over_floor,
            trigger,
        }
    }

    /// Only the cut-off's "over" answer is wanted: the cubic is already known dead.
    pub(crate) fn over_only(over_floor: f64) -> Self {
        Abandon {
            base: 0.0,
            best: f64::NEG_INFINITY,
            cubic_floor: 0.0,
            gate: 0.0,
            over_floor,
            trigger: 2.0 * over_floor,
        }
    }

    /// Whether a root whose residual is at least `acc` costs at least `best`, whatever its
    /// wobble: `cost = ((base + ½χ²) + floor) + wobble ≥ (base + ½·acc) + floor` term by
    /// term, because IEEE addition is monotone and `χ² ≥ acc`, `wobble ≥ 0`.
    #[inline]
    fn dead(&self, acc: f64) -> bool {
        self.base + 0.5 * acc + self.cubic_floor >= self.best
    }

    /// A root with partial residual `acc` can be dropped: dead, clear of the ellipse gate,
    /// and over the cut-off floor where that is asked.
    #[inline]
    fn drops(&self, acc: f64) -> bool {
        self.dead(acc) && acc >= self.gate && 0.5 * acc > self.over_floor
    }
}

/// What [`best_cubic_bounded`] found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum G1Fit {
    /// No admissible arms (or a degenerate chord): [`best_cubic`]'s `None`.
    Untried,
    /// [`best_cubic`]'s answer, bit for bit: `(χ², d0, d1)`.
    Exact(f64, f64, f64),
    /// Admissible arms exist, but the best of them costs at least [`Abandon`]'s `best` and
    /// cannot decide the ellipse gate. `over` is whether its residual exceeds the cut-off
    /// floor (meaningful only where the floor was asked).
    Dead { over: bool },
}

/// [`best_cubic`] with subsampling, for the dynamic program, stopping each root's residual
/// as soon as [`Abandon`] says the programme cannot use it.
///
/// The roots are scored in the quartic's order with the same summation as [`best_cubic`].
/// The answer is [`G1Fit::Exact`] exactly when it is provably [`best_cubic`]'s own:
///
/// - no root was dropped by the bound: then this *is* [`best_cubic`]'s loop;
/// - or the best completed root `v` is not dead. A dropped root `r` has
///   `(base + ½·acc_r) + floor ≥ best`, so `χ²_r ≥ acc_r > v` by monotonicity; `v` is
///   smaller than every dropped root, and among the rest the loop is [`best_cubic`]'s
///   (first minimum wins).
///
/// Otherwise every root is dead, so the cubic cannot be accepted, and each dropped root
/// passed the ellipse-gate test. A completed dead root that did not (a narrow band of
/// rounding, never seen in practice) sends the call back to [`best_cubic`], so no case is
/// guessed. `over` of [`G1Fit::Dead`] is exact: dropped roots exceeded the floor by
/// construction and completed ones are tested; the minimum is over the floor exactly when
/// every root is.
#[allow(clippy::too_many_arguments)]
pub(crate) fn best_cubic_bounded(
    pts: &[Point],
    sigma: &[f64],
    s: &[f64],
    i: usize,
    j: usize,
    t0: Vec2,
    t1: Vec2,
    raw: (f64, f64, f64),
    ab: &Abandon,
) -> G1Fit {
    let exact = || match best_cubic(pts, sigma, s, i, j, t0, t1, raw, true) {
        Some((c, d0, d1)) => G1Fit::Exact(c, d0, d1),
        None => G1Fit::Untried,
    };
    let Some(fr) = g1_frame(pts[i], pts[j], t0, t1, raw) else {
        return G1Fit::Untried;
    };
    let Some(plan) = CubicSamples::new(pts, sigma, s, i, j) else {
        return exact();
    };
    let mut best: Option<(f64, f64, f64)> = None;
    let mut admissible = false;
    let mut dropped = false;
    let mut over = true;
    for (d0, d1) in arms_from_moments(fr.th0, fr.th1, fr.area, fr.mx).iter() {
        if d0 > MAX_ARM || d1 > MAX_ARM {
            continue;
        }
        admissible = true;
        let cb = Cubic::from_arms(pts[i], pts[j], t0, t1, fr.chord, d0, d1);
        let (chi2, end) = plan.chi2_until(&cb, best.map_or(f64::INFINITY, |b| b.0), ab);
        match end {
            RootEnd::Done => {
                if !chi2.is_finite() {
                    return exact();
                }
                if best.map(|b| chi2 < b.0).unwrap_or(true) {
                    best = Some((chi2, d0, d1));
                }
                if 0.5 * chi2 <= ab.over_floor {
                    over = false;
                    // Only the cut-off's answer was wanted, and it is in.
                    if ab.best == f64::NEG_INFINITY {
                        return G1Fit::Dead { over };
                    }
                }
            }
            RootEnd::Best => {}
            RootEnd::Bound => dropped = true,
        }
    }
    if !admissible {
        return G1Fit::Untried;
    }
    match best {
        Some((c, d0, d1)) if !dropped || !ab.dead(c) => G1Fit::Exact(c, d0, d1),
        Some((c, _, _)) if c < ab.gate => exact(),
        _ => G1Fit::Dead { over },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Random spans of noisy arcs, S-bends, corners and straight runs, as
    /// `(points, sigma, arc lengths, i, j, t0, t1)`.
    #[allow(clippy::type_complexity)]
    fn random_spans() -> Vec<(Vec<Point>, Vec<f64>, Vec<f64>, usize, usize, Vec2, Vec2)> {
        let mut st = 11u64;
        let mut rnd = || {
            st = st
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (st >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut out = Vec::new();
        for case in 0..600 {
            let n = 3 + (rnd() * 90.0) as usize;
            let bend = 4.0 * rnd() - 2.0;
            let wave = if case % 3 == 0 { 3.0 * rnd() } else { 0.0 };
            let noise = [0.0, 0.05, 0.4][case % 3];
            let pts: Vec<Point> = (0..n)
                .map(|k| {
                    let u = k as f64 / (n - 1) as f64;
                    let y = bend * 20.0 * u * (1.0 - u)
                        + wave * (u * 9.0).sin()
                        + noise * (rnd() - 0.5);
                    Point::new(40.0 * u, y)
                })
                .collect();
            let sigma: Vec<f64> = (0..n).map(|_| 0.05 + 0.3 * rnd()).collect();
            let s = crate::tangents::arc_lengths(&pts);
            let t0 = unit(pts[1] - pts[0]).unwrap_or(Vec2 { x: 1.0, y: 0.0 });
            let t1 = unit(pts[n - 1] - pts[n - 2]).unwrap_or(Vec2 { x: 1.0, y: 0.0 });
            // Tangents off by up to fifteen degrees, as an estimator's can be.
            let turn = |v: Vec2, a: f64| Vec2 {
                x: v.x * a.cos() - v.y * a.sin(),
                y: v.x * a.sin() + v.y * a.cos(),
            };
            let (a0, a1) = (0.5 * (rnd() - 0.5), 0.5 * (rnd() - 0.5));
            out.push((pts, sigma, s, 0, n - 1, turn(t0, a0), turn(t1, a1)));
        }
        out
    }

    /// With no bound, the bounded scoring is `best_cubic` itself; with bounds, whatever it
    /// calls exact is `best_cubic`'s answer bit for bit, whatever it calls dead costs at
    /// least the bound and clears the ellipse gate, and its "over" answer is exact.
    #[test]
    fn best_cubic_bounded_is_best_cubic_where_it_matters() {
        let mut dead = 0usize;
        let mut exact_under_bound = 0usize;
        for (pts, sigma, s, i, j, t0, t1) in random_spans() {
            let raw = raw_moments_direct(&pts, i, j);
            let want = best_cubic(&pts, &sigma, &s, i, j, t0, t1, raw, true);
            let got = best_cubic_bounded(&pts, &sigma, &s, i, j, t0, t1, raw, &Abandon::NONE);
            match (want, got) {
                (None, G1Fit::Untried) => {}
                (Some((c, d0, d1)), G1Fit::Exact(c2, e0, e1)) => {
                    assert_eq!(
                        (c.to_bits(), d0.to_bits(), d1.to_bits()),
                        (c2.to_bits(), e0.to_bits(), e1.to_bits())
                    );
                }
                other => panic!("unbounded disagrees: {other:?}"),
            }
            let Some((c, d0, d1)) = want else {
                continue;
            };
            let lambda = 3.0;
            let cubic_floor = 6.0 * lambda;
            let chord = (pts[j] - pts[i]).norm();
            let wobble =
                Cubic::from_arms(pts[i], pts[j], t0, t1, chord, d0, d1).wobble_penalty(lambda);
            for base in [0.0, 17.25, 4.0e5] {
                let cost = base + 0.5 * c + cubic_floor + wobble;
                // Bounds either side of the cubic's own cost, and on it exactly.
                for best in [cost * 0.5, cost - 1e-9, cost, cost + 1e-9, cost * 1.5 + 1.0] {
                    for gate in [0.0, 2.0 * (lambda + 1e-9 * (best.abs() + lambda))] {
                        for over_floor in [f64::NEG_INFINITY, 0.25 * c, 0.5 * c, 2.0 * c] {
                            let ab = Abandon::new(base, best, cubic_floor, gate, over_floor);
                            match best_cubic_bounded(&pts, &sigma, &s, i, j, t0, t1, raw, &ab) {
                                G1Fit::Exact(c2, e0, e1) => {
                                    assert_eq!(
                                        (c.to_bits(), d0.to_bits(), d1.to_bits()),
                                        (c2.to_bits(), e0.to_bits(), e1.to_bits())
                                    );
                                    exact_under_bound += 1;
                                }
                                G1Fit::Dead { over } => {
                                    assert!(cost >= best, "dead but cheaper: {cost} < {best}");
                                    assert!(c >= gate, "dead inside the ellipse gate");
                                    if over_floor > f64::NEG_INFINITY {
                                        assert_eq!(over, 0.5 * c > over_floor);
                                    }
                                    dead += 1;
                                }
                                G1Fit::Untried => panic!("admissible arms went missing"),
                            }
                        }
                    }
                }
            }
            // Only the cut-off's answer.
            for over_floor in [0.1 * c, 0.5 * c, 0.49999 * c, 3.0 * c] {
                let ab = Abandon::over_only(over_floor);
                let over = match best_cubic_bounded(&pts, &sigma, &s, i, j, t0, t1, raw, &ab) {
                    G1Fit::Exact(c2, ..) => 0.5 * c2 > over_floor,
                    G1Fit::Dead { over } => over,
                    G1Fit::Untried => panic!("admissible arms went missing"),
                };
                assert_eq!(
                    over,
                    0.5 * c > over_floor,
                    "over floor {over_floor}, chi2 {c}"
                );
            }
        }
        assert!(
            dead > 1000 && exact_under_bound > 1000,
            "{dead} dead, {exact_under_bound} exact"
        );
    }
}
