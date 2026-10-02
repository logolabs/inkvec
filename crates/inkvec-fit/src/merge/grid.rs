//! The free-cubic search's coarse grid, with every curve sample assembled from cached
//! partial sums.
//!
//! # The problem
//!
//! `super::free_cubic` looks for the cubic `B = [P0, P1, P2, P3]` from the run's fixed
//! start `P0` to its fixed end `P3` that best fits the measured points, with
//!
//! ```text
//!     P1 = P0 + d0·c·R(r0)·u0,      P2 = P3 − d1·c·R(r1)·u1,
//! ```
//!
//! `c = |P3 − P0|` the chord (px), `u0`, `u1` the contour's own unit directions at the two
//! ends, `R(r)` a rotation by `r` degrees and `d0`, `d1` arm lengths in chords. The coarse
//! grid tries every combination of 9 rotations ([`ANGLES`]) at each end by 5 arm lengths
//! ([`ARMS`]) at each: 2,025 candidates, each scored on [`COARSE_SAMPLES`]` + 1 = 25`
//! samples `B(k/24)` against the run (`super::residual`), keeping the first one strictly
//! below the best so far. The compass search (`FreeCubicSearch::refine`) then finishes
//! from the grid's winner.
//!
//! # What is cached
//!
//! `eval_cubic` computes each coordinate as `((b0·P0 + b1·P1) + b2·P2) + b3·P3`, with
//! `b = bernstein(k/24)`, rounding after every operation. Over the grid:
//!
//! - `b0..b3` depend only on `k` (25 values);
//! - `P1` depends only on `(r0, d0)` and `P2` only on `(r1, d1)` (45 values each);
//! - `P0` and `P3` are fixed.
//!
//! So, per coordinate, the first partial sum `b0·P0 + b1·P1` is computed once per
//! `(r0, d0, k)` ("head"), the product `b2·P2` once per `(r1, d1, k)` ("tail") and
//! `b3·P3` once per `k` ("last"), and each candidate's sample is `(head + tail) + last`:
//! the same operations, on the same operands, in the same order as `eval_cubic`, so the
//! same bits. The rotations `R(r)·u` are taken once per angle, by the same `rotate` call
//! `FreeCubicSearch::build` makes, and the control points from them by the same
//! expressions. A candidate then costs two additions per coordinate per sample instead of
//! a Bernstein evaluation, four products and three additions.
//!
//! # The self-crossing test, deferred
//!
//! A self-crossing cubic is inadmissible: it used to score infinity before its residual was
//! taken, and so could never become the best. Its residual is now taken first, against the
//! same bound, and the crossing test runs only on a candidate whose residual would make it
//! the new best. A crossing candidate is therefore still never the best, a non-crossing one
//! is scored exactly as before, and the bound evolves identically; only the candidates
//! that improve on the best so far pay for the test, instead of all 2,025.
//!
//! # Order, ties and the arm range
//!
//! Candidates are visited in the original nested order (`r0`, `r1`, `d0`, `d1`, outermost
//! first) and only a strictly smaller residual replaces the best, so ties resolve as before.
//! Arms outside `[0.02, MAX_ARM]` are skipped as before; with `MAX_ARM = 1` none is.
//!
//! # Measured
//!
//! r2-qspeed (2026-10-01, screen set, one thread): alone, the cache cut grid time by 11 %
//! and the post-fit merge by 7 %. Most of a candidate's cost is the nearest-sample search
//! and the residual's terms, which `super::residual` screens; together the two cut the
//! post-fit merge by 46 %. Byte-identical on all 246 icons.
//!
//! # Literature
//!
//! - Not from the literature: caching the Bernstein partial sums of a tensor grid of
//!   control points, because the published speed-ups for evaluating many Bézier curves
//!   (forward differencing, de Casteljau subdivision, Horner's scheme) each round
//!   differently from `eval_cubic`, and the search's bits must not move. Tabulating the
//!   basis at fixed parameters is common practice in Bézier fitting; here the tabulated
//!   quantity is the partial sum itself, which is what makes the result exact.
//! - See also: Kolesnikov, A. & Fränti, P. (2007), "Polygonal approximation of closed
//!   discrete curves", *Pattern Recognition* 40:1282–1293,
//!   doi:10.1016/j.patcog.2006.09.002, §2.4, whose objection to recomputing a first run's
//!   results in a second ("the search starts again from scratch and loses the information
//!   of the previous run") is the same economy applied to the closed-curve dynamic program;
//!   their cache is of DP span costs, this one of curve samples.

use inkvec_core::{Point, Vec2};

use super::residual::score_below;
use super::{rotate, FreeCubicSearch, COARSE_SAMPLES, MAX_ARM};
use crate::curves::{bernstein, cubic_self_intersects};

/// The grid's rotations of each end direction, degrees.
pub(super) const ANGLES: [f64; 9] = [-90.0, -65.0, -45.0, -22.0, 0.0, 22.0, 45.0, 65.0, 90.0];

/// The grid's arm lengths, as fractions of the chord.
pub(super) const ARMS: [f64; 5] = [0.15, 0.3, 0.45, 0.6, 0.8];

/// Samples per candidate on the grid, `COARSE_SAMPLES + 1`.
const N1: usize = COARSE_SAMPLES + 1;

/// One end's `(angle, arm)` choices: `ANGLES.len() · ARMS.len()` of them, indexed
/// `angle · ARMS.len() + arm`.
const END_CHOICES: usize = ANGLES.len() * ARMS.len();

/// The cached partial sums of every grid candidate's samples, and the control points the
/// crossing test needs.
pub(super) struct CoarseGrid {
    /// `head[e0][k] = b0(k)·P0 + b1(k)·P1(e0)`, per coordinate, for each start choice
    /// `e0` and sample `k`.
    head: Vec<[Point; N1]>,
    /// `tail[e1][k] = b2(k)·P2(e1)`, per coordinate, for each end choice `e1`.
    tail: Vec<[Point; N1]>,
    /// `last[k] = b3(k)·P3`, per coordinate.
    last: [Point; N1],
    /// `P1` for each start choice.
    ctrl1: [Point; END_CHOICES],
    /// `P2` for each end choice.
    ctrl2: [Point; END_CHOICES],
}

impl CoarseGrid {
    /// The cache for `search`: 9 + 9 rotations, 90 control points and
    /// `2 · 45 · 25 + 25` partial-sum points, about 36 KB, on the heap (two allocations per
    /// search) so that no thread's stack, wasm32's included, has to hold it.
    ///
    /// Every quantity is computed by the expression `FreeCubicSearch::build` and
    /// `eval_cubic` use, on the same operands, so each rounds identically.
    pub(super) fn new(search: &FreeCubicSearch<'_>) -> Self {
        let (p0, p3, chord) = (search.p0, search.p3, search.chord);
        let bern: [[f64; 4]; N1] =
            std::array::from_fn(|k| bernstein(k as f64 / COARSE_SAMPLES as f64));
        let dir0: [Vec2; 9] = ANGLES.map(|r| rotate(search.base0, r));
        let dir1: [Vec2; 9] = ANGLES.map(|r| rotate(search.base1, r));
        let ctrl1: [Point; END_CHOICES] = std::array::from_fn(|e| {
            let (u, d) = (dir0[e / ARMS.len()], ARMS[e % ARMS.len()]);
            Point::new(p0.x + u.x * d * chord, p0.y + u.y * d * chord)
        });
        let ctrl2: [Point; END_CHOICES] = std::array::from_fn(|e| {
            let (u, d) = (dir1[e / ARMS.len()], ARMS[e % ARMS.len()]);
            Point::new(p3.x - u.x * d * chord, p3.y - u.y * d * chord)
        });
        let head = ctrl1
            .iter()
            .map(|q1| {
                std::array::from_fn(|k| {
                    let b = bern[k];
                    Point::new(b[0] * p0.x + b[1] * q1.x, b[0] * p0.y + b[1] * q1.y)
                })
            })
            .collect();
        let tail = ctrl2
            .iter()
            .map(|q2| {
                std::array::from_fn(|k| {
                    let b = bern[k];
                    Point::new(b[2] * q2.x, b[2] * q2.y)
                })
            })
            .collect();
        let last = std::array::from_fn(|k| Point::new(bern[k][3] * p3.x, bern[k][3] * p3.y));
        CoarseGrid {
            head,
            tail,
            last,
            ctrl1,
            ctrl2,
        }
    }

    /// The samples `B(k/24)`, `k = 0..=24`, of the candidate with start choice `e0` and
    /// end choice `e1`, written into `out`: `(head + tail) + last`, bit for bit what
    /// `eval_cubic` returns for that cubic. O(25).
    pub(super) fn samples(&self, e0: usize, e1: usize, out: &mut [Point; N1]) {
        let (h, t) = (&self.head[e0], &self.tail[e1]);
        for (k, s) in out.iter_mut().enumerate() {
            *s = Point::new(
                h[k].x + t[k].x + self.last[k].x,
                h[k].y + t[k].y + self.last[k].y,
            );
        }
    }

    /// Whether the candidate `(e0, e1)` crosses itself, by the test
    /// `FreeCubicSearch::score` applies to the cubic `build` returns.
    fn crosses(&self, p0: Point, p3: Point, e0: usize, e1: usize) -> bool {
        cubic_self_intersects(p0, self.ctrl1[e0], self.ctrl2[e1], p3)
    }
}

impl FreeCubicSearch<'_> {
    /// Coarse grid first, on a cheap residual: 9 rotations at each end by 5 arm lengths
    /// at each, 2025 candidates. Returns `[r0, r1, d0, d1]` of the first candidate with
    /// the smallest residual, or `None` if every one is inadmissible.
    ///
    /// A pattern search alone gets stuck here: the fits that matter are *asymmetric* --
    /// around -10 and +55 degrees at the two ends -- and a search started from equal arms
    /// and equal angles settles into a symmetric basin at chi-squared 174 where the true
    /// optimum is 101. The grid is what escapes it; the refinement is what makes the grid
    /// affordable, since it can then be coarse.
    ///
    /// Only a residual below the best so far can move the search, so each candidate is
    /// scored against it and dropped as soon as it cannot be (`score_below`). See the
    /// module comment for the cache and the deferred crossing test, and why neither
    /// changes the answer. O(2025 · m · 25) at worst for a run of `m` points; in practice
    /// most candidates are rejected after a few terms.
    pub(super) fn grid(&self) -> Option<[f64; 4]> {
        let grid = CoarseGrid::new(self);
        let arm_ok = ARMS.map(|d| (0.02..=MAX_ARM).contains(&d));
        let mut cur = [0.0f64, 0.0, 0.35, 0.35];
        let mut rough = f64::INFINITY;
        let mut samples = [Point::new(0.0, 0.0); N1];
        for (a0, &r0) in ANGLES.iter().enumerate() {
            for (a1, &r1) in ANGLES.iter().enumerate() {
                for (i0, &d0) in ARMS.iter().enumerate() {
                    for (i1, &d1) in ARMS.iter().enumerate() {
                        if !arm_ok[i0] || !arm_ok[i1] {
                            continue;
                        }
                        let (e0, e1) = (a0 * ARMS.len() + i0, a1 * ARMS.len() + i1);
                        grid.samples(e0, e1, &mut samples);
                        let x = score_below(&samples, self.poly, self.a, self.b, rough);
                        if x < rough && !grid.crosses(self.p0, self.p3, e0, e1) {
                            rough = x;
                            cur = [r0, r1, d0, d1];
                        }
                    }
                }
            }
        }
        rough.is_finite().then_some(cur)
    }
}

#[cfg(test)]
mod tests {
    //! The cache against the expressions it replaces, candidate by candidate.
    use super::*;
    use crate::curves::eval_cubic;
    use inkvec_core::Polyline;

    /// Every one of the 2,025 candidates' 25 samples equals `eval_cubic` of the cubic
    /// `build` returns, bit for bit, and so does its crossing test, for runs at small and
    /// large coordinates, short and long chords, and end directions from the four
    /// quadrants.
    #[test]
    fn cached_samples_are_the_evaluated_ones() {
        let mut state = 23u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let poly = Polyline::new(vec![Point::new(0.0, 0.0); 4], vec![0.5; 4], false);
        let mut out = [Point::new(0.0, 0.0); N1];
        for case in 0..24 {
            let origin = [0.0, 0.3, -17.0, 4096.5, 1e6][case % 5];
            let reach = [1e-3, 0.7, 12.0, 300.0][case % 4];
            let p0 = Point::new(origin + reach * next(), origin - reach * next());
            let p3 = Point::new(p0.x + reach * (next() - 0.5), p0.y + reach * (next() - 0.5));
            let unit = |t: f64| Vec2 {
                x: (std::f64::consts::TAU * t).cos(),
                y: (std::f64::consts::TAU * t).sin(),
            };
            let search = FreeCubicSearch {
                poly: &poly,
                a: 0,
                b: 3,
                p0,
                p3,
                chord: p0.dist(p3),
                base0: unit(next()),
                base1: unit(next()),
            };
            let grid = CoarseGrid::new(&search);
            for (a0, &r0) in ANGLES.iter().enumerate() {
                for (a1, &r1) in ANGLES.iter().enumerate() {
                    for (i0, &d0) in ARMS.iter().enumerate() {
                        for (i1, &d1) in ARMS.iter().enumerate() {
                            let (e0, e1) = (a0 * ARMS.len() + i0, a1 * ARMS.len() + i1);
                            let c = search.build(r0, r1, d0, d1);
                            grid.samples(e0, e1, &mut out);
                            for (k, s) in out.iter().enumerate() {
                                let want = eval_cubic(c, k as f64 / COARSE_SAMPLES as f64);
                                assert_eq!(
                                    (s.x.to_bits(), s.y.to_bits()),
                                    (want.x.to_bits(), want.y.to_bits()),
                                    "case {case}, candidate {e0}/{e1}, sample {k}"
                                );
                            }
                            assert_eq!(
                                grid.crosses(p0, p3, e0, e1),
                                cubic_self_intersects(c[0], c[1], c[2], c[3])
                            );
                        }
                    }
                }
            }
        }
    }
}
