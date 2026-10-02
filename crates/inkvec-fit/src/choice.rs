//! Each boundary's final description: its fitted curve, or a whole-boundary primitive when
//! that is cheaper -- and how to reach that choice without doing work the choice discards.
//!
//! # The problem
//!
//! Model selection under the crate's MDL objective. Every boundary of the planar map (a
//! measured [`Polyline`], points in px, each with its uncertainty `σ_k`) gets two
//! candidate descriptions:
//!
//! - the **curve**: the dynamic program's path of lines, cubics and arcs
//!   ([`crate::multimodel::optimal_multimodel`]), scored by [`boundary_cost`];
//! - the **primitive**: the cheapest whole-ring circle, ellipse or (rounded) rectangle, or
//!   a run of arcs ([`fit_primitive_or_arcs`]), which prices itself.
//!
//! The primitive wins when its cost is strictly below the curve's ([`choose`]). Both are
//! pure functions of the polyline and the configuration.
//!
//! # The order of the work ([`describe`])
//!
//! 1. **The image frame.** When the whole image border is one boundary (one face touches
//!    every border pixel: the background of 166 of the 246 screen icons), that boundary is
//!    a closed ring of `2(W + H)` lattice points on the image rectangle, with σ = 0.5 px.
//!    Its dynamic program ran both of its cuts over hundreds of points and was then thrown
//!    away: the rectangle won 166 times out of 166. So for the frame the primitive search
//!    runs first, and when its cost is below [`cost_floor`] -- a lower bound on what *any*
//!    fitted path of this polyline can cost -- the dynamic program is not run at all. When
//!    it is not below the floor, the program runs and the choice is made as before.
//! 2. **Every other boundary.** The dynamic program and the primitive search run side by
//!    side ([`rayon::join`]): the search reads only the polyline, and only the final
//!    comparison needs both. The program runs on the calling thread and the search is
//!    offered to an idle one, so the boundary's wall time falls from the sum of the two
//!    towards the longer of them.
//!
//! # Why both are exact
//!
//! Step 2 computes the same two pure functions and compares them as before. Step 1 skips
//! the program only when the primitive provably wins: [`cost_floor`] is at most the
//! [`boundary_cost`] of every path with at least one segment (a path with none costs
//! infinity), so a primitive below it is below the program's path whatever that path is,
//! and the comparison the program would have lost is not made. The one assumption: the
//! program's path has finite coordinates, as it does for finite input. A NaN path would
//! have cost NaN, compared false, and been kept.
//!
//! # Measured
//!
//! r2-qspeed (2026-10-01, a07b394, 246 screen icons at 128 px, one thread, 60.6 s of ring
//! work): the frame's program was 8.1 s of it (13 %), its primitive search 1.1 s. The
//! frame is the largest ring in 50 of the 246 icons, and at 128 px the stage's wall time
//! is its largest ring (work/span 2.86, median). In that ring the primitive search is
//! 18 % of the time at 128 px and 33 % at 2048 px, which step 2 takes off the critical
//! path whenever a thread is idle -- at the stage's tail, where the largest ring is left
//! running alone.
//!
//! # Literature
//!
//! - Inspired by: Morin, T. L. & Marsten, R. E. (1976), "Branch-and-bound strategies for
//!   dynamic programming", *Operations Research* 24(4):611–627, doi:10.1287/opre.24.4.611
//!   -- discard a subproblem whose lower bound is no better than the incumbent. Here the
//!   subproblem is the whole dynamic program of one ring, the incumbent is its primitive,
//!   and the bound is [`cost_floor`], which needs no part of the program's own work.
//! - Method from: Pearson, K. (1901), "On lines and planes of closest fit to systems of
//!   points in space", *Phil. Mag.* 2(11):559–572, doi:10.1080/14786440109462720 -- the
//!   least weighted sum of squared distances from points to any line is the smallest
//!   eigenvalue of their weighted scatter matrix about the centroid; [`cost_floor`] uses
//!   it to bound a single-line path.
//! - Method from: Blumofe, R. D. & Leiserson, C. E. (1999), "Scheduling multithreaded
//!   computations by work stealing", *J. ACM* 46(5):720–748, doi:10.1145/324133.324234 --
//!   the randomised work-stealing fork-join that `rayon::join` implements, with expected
//!   time `T1/P + O(T∞)`; step 2 lowers the span `T∞` (the largest ring) without adding
//!   work. A thread waiting in a join may run another boundary's whole job meanwhile
//!   (rayon's documentation of `join`), which can delay that join's return; measured, see
//!   the commit that added it.
//! - Not from the literature: the parameter-count floor itself, because it is a property
//!   of this objective's prices (every segment costs at least two numbers). See also the
//!   price floor of `crate::merge`, the same argument applied to one merge.

use inkvec_core::{Point, Polyline};

use crate::curves::{self, Segment};
use crate::primitives::{fit_primitive_or_arcs, PrimitiveFit};
use crate::{FitConfig, FittedPath, PARAMS_LINE};

/// A primitive-or-arc description from [`fit_primitive_or_arcs`]: the path form, the
/// whole-ring primitive if it is one, and its cost.
pub type PrimitiveOffer = (Vec<Segment>, Option<PrimitiveFit>, f64);

/// The MDL cost of a fitted path as the curve-against-primitive choice scores it:
/// `0.5·χ² + λ·P`.
///
/// `χ² = Σ_k (d_k/σ_k)²` sums each measured point's distance `d_k` (px) to the path,
/// sampled every quarter pixel ([`curves::chi2`], σ floored at 1e-3 px), `λ` is
/// `cfg.lambda` (nats per parameter) and `P = path.params()`, the start point's two
/// numbers plus every segment's own. Half of χ² is the Gaussian negative log-likelihood of
/// the points, so both terms are in nats. A path with no segments costs infinity. O(n +
/// path length / 0.25 px).
pub fn boundary_cost(poly: &Polyline, path: &FittedPath, cfg: &FitConfig) -> f64 {
    let chi2 = curves::chi2(&poly.points, &poly.sigma, path.start, &path.segments);
    0.5 * chi2 + cfg.lambda * path.params()
}

/// A lower bound on [`boundary_cost`] over every [`FittedPath`] of `poly` with finite
/// coordinates: `min(6λ, (2 + c)λ, 4λ + ½·L)`, with `c` the cubic's price in force and
/// `L` a lower bound on the χ² of any single straight segment. Minus infinity when `λ` is
/// negative or NaN, which proves nothing.
///
/// Why: every segment costs at least two parameters (a line 2, a cubic `c ≥ 2` (the
/// [`crate::cost::CostModel`] range), an arc 5 or 7) and the start point costs 2, so:
///
/// - two or more segments: `P ≥ 6`;
/// - one cubic: `P = 2 + c`; one arc: `P ≥ 7 ≥ 6`;
/// - one line: `P = 4`, and its χ² is at least the χ² of the whole straight line through
///   it, which is at least the smallest eigenvalue of the points' weighted scatter matrix
///   (Pearson 1901).
///
/// The cost is computed as `fl(fl(½χ²) + fl(λ·P))` with `χ² ≥ 0`, and rounding is
/// monotone, so it is at least `fl(λ·P_min)` and, for the line, at least
/// `fl(fl(½·L) + fl(4λ))` -- the very expressions evaluated here. `L` is shrunk by half and
/// by an absolute `1e-9·(tr S + Σw·C²)` (`C` the largest coordinate magnitude), which
/// dwarfs the rounding of the scatter, of its eigenvalue, and of the quarter-pixel samples
/// `curves::chi2` measures to, for any polyline under 2^20 points. O(n).
pub fn cost_floor(poly: &Polyline, cfg: &FitConfig) -> f64 {
    let lambda = cfg.lambda;
    if lambda.is_nan() || lambda < 0.0 {
        return f64::NEG_INFINITY;
    }
    let two_segments = lambda * (3.0 * PARAMS_LINE);
    let one_cubic = lambda * (2.0 + crate::cost::cubic_params());
    let one_line = 0.5 * line_chi2_floor(poly) + lambda * (2.0 * PARAMS_LINE);
    two_segments.min(one_cubic).min(one_line)
}

/// A lower bound on the χ² `curves::chi2` gives any single straight segment against
/// `poly`: half the smallest eigenvalue of the weighted scatter matrix `S` of the points
/// about their weighted centroid, less `1e-9·(tr S + Σw·C²)`, and never below zero.
///
/// Weights are `w_k = 1/s_k²` with `s_k = max(σ_k, 1e-3)` px, σ = 0.5 where missing, as
/// `curves::chi2` weighs them. For `S = [[a, b], [b, d]]` the smallest eigenvalue is
/// `(a + d)/2 − √(((a − d)/2)² + b²)`. Zero for an empty polyline, and for NaN or infinite
/// coordinates. O(n).
fn line_chi2_floor(poly: &Polyline) -> f64 {
    let weight = |k: usize| {
        let s = poly.sigma.get(k).copied().unwrap_or(0.5).max(1e-3);
        1.0 / (s * s)
    };
    let (mut sw, mut sx, mut sy, mut big) = (0.0, 0.0, 0.0, 0.0f64);
    for (k, p) in poly.points.iter().enumerate() {
        let w = weight(k);
        sw += w;
        sx += w * p.x;
        sy += w * p.y;
        big = big.max(p.x.abs()).max(p.y.abs());
    }
    // No points, or weights that are NaN: nothing to bound.
    if sw.is_nan() || sw <= 0.0 {
        return 0.0;
    }
    let centre = Point::new(sx / sw, sy / sw);
    let (mut a, mut b, mut d) = (0.0, 0.0, 0.0);
    for (k, p) in poly.points.iter().enumerate() {
        let w = weight(k);
        let (dx, dy) = (p.x - centre.x, p.y - centre.y);
        a += w * dx * dx;
        b += w * dx * dy;
        d += w * dy * dy;
    }
    let half_gap = 0.5 * (a - d);
    let smallest = 0.5 * (a + d) - (half_gap * half_gap + b * b).sqrt();
    let floor = 0.5 * smallest - 1e-9 * (a + d + sw * big * big);
    // `>` is false for NaN, so a NaN anywhere bounds nothing.
    if floor > 0.0 && floor.is_finite() {
        floor
    } else {
        0.0
    }
}

/// Whether every point lies on the border of a `width × height` image: on `x = −0.5`,
/// `x = width − 0.5`, `y = −0.5` or `y = height − 0.5`, the pixel-corner lines the planar
/// map puts the image edge on (px, pixel centres at integers). False for no points.
///
/// Exact comparisons: border nodes are lattice points that the sub-pixel refinement and
/// the boundary solve leave in place, so a point on the border is exactly on it. Used only
/// to decide what to try first; the choice itself rests on [`cost_floor`]. O(n).
pub fn lies_on_frame(points: &[Point], width: usize, height: usize) -> bool {
    let (x1, y1) = (width as f64 - 0.5, height as f64 - 0.5);
    !points.is_empty()
        && points
            .iter()
            .all(|p| p.x == -0.5 || p.y == -0.5 || p.x == x1 || p.y == y1)
}

/// The cheaper of the fitted `curve` and the primitive `offer`: the primitive's path form
/// (starting at the ring's first point) and the primitive itself when the offer's cost is
/// strictly below [`boundary_cost`] of the curve, otherwise the curve and `None`. A tie
/// keeps the curve; a NaN on either side keeps the curve. O(n + curve length).
pub fn choose(
    poly: &Polyline,
    curve: FittedPath,
    offer: Option<PrimitiveOffer>,
    cfg: &FitConfig,
) -> (FittedPath, Option<PrimitiveFit>) {
    match offer {
        Some((segs, prim, cost)) if cost < boundary_cost(poly, &curve, cfg) => {
            (primitive_path(poly, segs), prim)
        }
        _ => (curve, None),
    }
}

/// The path form of a primitive offer: its segments, from the ring's first point.
fn primitive_path(poly: &Polyline, segments: Vec<Segment>) -> FittedPath {
    FittedPath {
        start: poly.points[0],
        segments,
        closed: poly.closed,
    }
}

/// The primitive search for `poly`, as [`describe`] and the CLI's other callers run it.
pub fn primitive_offer(poly: &Polyline, cfg: &FitConfig) -> Option<PrimitiveOffer> {
    fit_primitive_or_arcs(&poly.points, &poly.sigma, poly.closed, cfg)
}

/// A boundary's description, with no work the choice would discard: see the module
/// comment. `frame` says the boundary is the image frame ([`lies_on_frame`]); `fit` runs
/// the dynamic program on `poly` (with whatever cancellation and timing the caller wraps
/// it in) and is called at most once, possibly on another thread (hence `Send`).
///
/// The result is exactly [`choose`]`(poly, fit(), primitive_offer(poly, cfg), cfg)`.
/// Cost: one primitive search, plus the program unless the frame's primitive is below
/// [`cost_floor`].
pub fn describe(
    poly: &Polyline,
    cfg: &FitConfig,
    frame: bool,
    fit: impl FnOnce() -> FittedPath + Send,
) -> (FittedPath, Option<PrimitiveFit>) {
    if frame {
        let offer = primitive_offer(poly, cfg);
        if let Some((segs, prim, cost)) = offer {
            if cost < cost_floor(poly, cfg) {
                return (primitive_path(poly, segs), prim);
            }
            return choose(poly, fit(), Some((segs, prim, cost)), cfg);
        }
        return (fit(), None);
    }
    // The program on this thread, the search offered to an idle one. Both are pure
    // functions of `poly` and `cfg`, so which thread runs which, and in what order,
    // changes nothing but the wall time.
    let (curve, offer) = rayon::join(fit, || primitive_offer(poly, cfg));
    choose(poly, curve, offer, cfg)
}

#[cfg(test)]
mod tests;
