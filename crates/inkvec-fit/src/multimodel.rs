//! One global dynamic program over a `{line, cubic}` alphabet (DESIGN.md S4).
//!
//! [`crate::fit_path`] is two passes: a line-only dynamic program decides the corners,
//! then cubics are fitted to the runs between them. The seam between the two passes is
//! where its bugs have lived, and it is not the single optimization the design
//! specifies. The obstacle to unifying them was cost: fitting a cubic to every candidate
//! span `(i, j)` is O(n) work, which makes the dynamic program O(n^3).
//!
//! This module removes that obstacle with Levien's 2021 derivation. Under G1
//! constraints — endpoints and end tangents fixed — a cubic has only two free numbers,
//! the control-arm lengths `d0, d1` (as fractions of the chord). Matching the source's
//! **signed area** fixes one relation between them; matching its **first moment** fixes
//! a second; together they reduce to a single quartic in `d0`, solved in closed form.
//! Area and moment of a polyline are Green's-theorem sums of closed-form per-edge terms,
//! so they are **prefix-summable**: the area and moment of any sub-polyline `(i..j)`
//! closed by its chord is a difference of two prefix entries. The G1 cubic for any
//! candidate span is therefore O(1), and the dynamic program with cubics in its alphabet
//! is O(n^2) — the same order as the line-only version.
//!
//! # The mathematics
//!
//! Per edge `a -> b` with `dx = b.x - a.x`, `dy = b.y - a.y`, parametrized linearly:
//!
//! ```text
//!   ∫ y dx     = dx · (a.y + dy/2)
//!   ∫ x y dx   = dx · (a.x·a.y + (a.x·dy + a.y·dx)/2 + dx·dy/3)
//!   ∫ y² dx    = dx · (a.y² + a.y·dy + dy²/3)
//! ```
//!
//! Summing over edges `i..j` and subtracting the same three quantities for the chord
//! `p_j -> p_i` gives the loop integrals of the region between the sub-polyline and its
//! chord. By Green's theorem, `∮ y dx = -A`, `∮ x y dx = -∫∫ x dA` and `½ ∮ y² dx =
//! -∫∫ y dA`, so these are the signed area and first moments. Translating the start point
//! to the origin, rotating so the chord lies on the x-axis and scaling to unit chord gives
//! `(area, mx)`, the two numbers Levien's quartic consumes. The coefficient formulas are
//! those of `kurbo::fit::cubic_fit` (referenced, not re-derived) and produce up to four
//! real candidates for `d0`, each with its paired `d1`:
//!
//! ```text
//!   d1 = (d0·sin θ0 − 10/3·area) / (½·d0·sin(θ0+θ1) − sin θ1)
//! ```
//!
//! where `θ0` is the start tangent's angle from the chord and `θ1` the chord's angle from
//! the end tangent. The residual of each candidate is then measured against the measured
//! points. Only that last step is O(j − i), and it is entered only after an O(1)
//! pre-check: a cubic costs `λ·6` before any residual, so a span whose line description
//! already costs less can never be won by a cubic, and the residual is never evaluated on
//! straight runs at all. On curved spans the residual is evaluated on at most
//! [`MAX_RESIDUAL_SAMPLES`] evenly spaced interior points, so a candidate costs O(1) in
//! `n` and the whole program is O(n² · MAX_RESIDUAL_SAMPLES).
//!
//! # Tangents and the DP state
//!
//! The state is the **vertex index alone**. The tangent at each vertex comes from the
//! polyline itself, estimated once, one-sidedly, before the program runs: `t⁻_k` from a
//! local quadratic fitted to the points *before* `k` and evaluated at `k`, and `t⁺_k`
//! likewise from the points *after*. One-sided is essential — a symmetric window
//! straddling a true corner returns the bisector, which is the wrong tangent for both
//! neighbouring segments. The window is the widest whose quadratic fit is still
//! consistent with the measurement model (`χ²/dof ≤ τ²`), so it is wide on smooth runs,
//! where noise averaging matters, and narrow beside corners, where bias would matter.
//!
//! This was chosen over the alternative of a `(vertex, tangent-bin)` state because it is
//! **exact**: there is no quantization, the objective is a sum of per-segment and
//! per-vertex terms, and the dynamic program is verified against exhaustive enumeration
//! of every segmentation and type assignment. The price is that the tangent at a join is
//! *estimated* rather than *optimized* jointly with its two segments; that is the job of
//! S5, and a first step in that direction is taken here after the program has run
//! (see `refine`).
//!
//! # Corners and the cost of a tangent break
//!
//! At a G1 join the outgoing segment's initial direction is implied by the incoming one,
//! so one of its parameters is free. A corner therefore costs one extra parameter, `λ`
//! nats — that is the whole justification for the break cost, and it is why it scales
//! with `λ` rather than being a separate knob. Because tangent estimates carry noise, the
//! cost ramps quadratically from zero at a perfect join to `λ` at
//! [`G1_BREAK_DEGREES`], and saturates there. It is charged as: a per-vertex term for the
//! polyline's own turn `∠(t⁻_k, t⁺_k)` at every chosen vertex, plus, for a line, the
//! disagreement between its chord and the polyline tangents at each end. Cubics match
//! the polyline tangents by construction and carry no such term. This is a
//! triangle-inequality decomposition of the break between the *emitted* tangents, and
//! it is what makes the state a bare vertex index.
//!
//! Corners then **emerge** from the program: a true corner is where `t⁻ ≠ t⁺`, no single
//! segment can span it without a large residual, and the vertex pays its turn cost
//! whichever types meet there.

// The dynamic program indexes several parallel arrays by the same counter (points,
// sigmas, prefix sums, tangents), so `needless_range_loop` would have us zip four
// iterators where one index is clearer. The cost function likewise takes the whole
// problem state; bundling it into a struct would only move the arguments.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

pub(crate) use crate::candidates::{
    arcs_enabled, best_cubic, bow_penalty, chi2_cubic, edge_terms, ellipses_enabled,
    line_cost_terms, raw_moments_direct, scatter_min_eigen, try_arc, try_ellipse, try_free_cubic,
    unit, CirclePrefix, Cubic,
};
pub use crate::candidates::{
    fit_cubic_moments, free_cubic_fit, params_cubic, MAX_ARM, MAX_RESIDUAL_SAMPLES,
};
use crate::curves::Segment;
pub(crate) use crate::tangents::{arc_lengths, g1_break_radians, symmetric_tangent, turn_angle};
pub use crate::tangents::{
    break_cost, estimate_tangents, vertex_cost, Tangents, G1_BREAK_DEGREES, TANGENT_WINDOW_MAX,
};
use crate::{adjust_vertices_at, FitConfig, FittedPath, Segmentation, PARAMS_LINE};
use inkvec_core::{Point, Polyline, Vec2};
use kurbo::{CubicBez, Line as KLine, ParamCurveNearest, Point as KPoint};

/// Parameters a cubic adds to the document: two control points and an endpoint.
pub const PARAMS_CUBIC: f64 = 6.0;

/// Overridable for experiments: `INKVEC_PARAMS_CUBIC`, `INKVEC_G1_BREAK`.
///
/// These two set how willing the program is to spend a curve. A cubic costs three times a
/// line, and the counterweight — the penalty for the tangent breaks a polyline leaves — is
/// quadratic in the break angle and saturates at one parameter's worth, so the small breaks
/// that a chain of short lines makes are nearly free. Traced output is 21% curved where the
/// ground truth is 78%, which is that trade showing up as faceted curves.
pub fn dp_debug() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var_os("INKVEC_DPDBG").is_some())
}

/// Safety factor on the search cut-off (see [`crate::optimal_polygon`]).
use crate::PRUNE_SLACK;

/// Consecutive candidates that must exceed the cut-off before the scan from a start
/// vertex stops. The line residual is monotone in the span, so one exceedance would
/// do for lines alone; the cubic residual is not — the end tangent changes with `j`,
/// and a span that fits badly can be followed by a longer one that fits well. That
/// was measured, not supposed: with a single-exceedance cut-off the program returned
/// a non-optimal S-curve segmentation that exhaustive enumeration caught.
const PRUNE_PATIENCE: usize = 8;

/// Most measured points the dynamic program is run on; longer boundaries are decimated
/// to at most this many (see [`optimal_multimodel_capped_full`]). A 128px image never
/// reaches it, so the benchmark at that size is untouched.
const DP_MAX_POINTS: usize = 768;

/// The alphabet a segment of the multimodel dynamic program's fit was chosen from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegKind {
    /// A straight line segment.
    Line,
    /// A cubic Bézier segment.
    Cubic,
    /// A circular arc segment.
    Arc,
}

/// Result of the multimodel dynamic program, with the discrete decisions exposed.
#[derive(Debug, Clone)]
pub struct MultimodelFit {
    /// The fitted path itself.
    pub path: FittedPath,
    /// Indices into the source polyline chosen as vertices. For a closed input the
    /// first and last are the same index.
    pub vertices: Vec<usize>,
    /// Type of each segment `vertices[k] -> vertices[k+1]`.
    pub kinds: Vec<SegKind>,
    /// Value of the dynamic program's objective at the optimum, *before* the continuous
    /// refinement of `refine`. This is what the exhaustive test verifies.
    pub cost: f64,
}

/// Fit a measured boundary with lines and cubics in one global optimization.
pub fn optimal_multimodel(poly: &Polyline, cfg: &FitConfig) -> FittedPath {
    optimal_multimodel_full(poly, cfg).path
}

/// Baseline for transactional structural trials, without changing process-wide
/// environment variables (boundary fitting is parallel).
pub fn optimal_multimodel_without_structural(poly: &Polyline, cfg: &FitConfig) -> FittedPath {
    optimal_multimodel_impl(poly, cfg, usize::MAX, false).path
}

/// As [`optimal_multimodel`], but forbidding any single segment from spanning more than
/// `max_span` measured points.
///
/// This exists for the self-intersection repair in [`crate::simple`]. It is a blunt
/// instrument on purpose: the objective has no term for "the assembled ring crosses
/// itself", so rather than trying to teach it one, the repair re-runs the same objective
/// under a constraint that provably ends the problem. At `max_span = 1` every segment is
/// a single polyline edge, which reproduces the measured contour — and the measured
/// contour is a simple closed curve by construction, so the loop always terminates.
pub fn optimal_multimodel_capped(poly: &Polyline, cfg: &FitConfig, max_span: usize) -> FittedPath {
    optimal_multimodel_capped_full(poly, cfg, max_span).path
}

/// As [`optimal_multimodel`], also returning the chosen vertices, types and objective.
pub fn optimal_multimodel_full(poly: &Polyline, cfg: &FitConfig) -> MultimodelFit {
    optimal_multimodel_capped_full(poly, cfg, usize::MAX)
}

/// [`optimal_multimodel_full`] with a cap on how many measured points one segment may
/// span.
pub fn optimal_multimodel_capped_full(
    poly: &Polyline,
    cfg: &FitConfig,
    max_span: usize,
) -> MultimodelFit {
    optimal_multimodel_impl(
        poly,
        cfg,
        max_span,
        std::env::var("INKVEC_STRUCTURAL").is_ok_and(|v| v != "0"),
    )
}

fn optimal_multimodel_impl(
    poly: &Polyline,
    cfg: &FitConfig,
    max_span: usize,
    structural: bool,
) -> MultimodelFit {
    let n = poly.len();
    if n < 2 {
        return MultimodelFit {
            path: FittedPath {
                start: poly.points.first().copied().unwrap_or(Point::new(0.0, 0.0)),
                segments: Vec::new(),
                closed: poly.closed,
            },
            vertices: (0..n).collect(),
            kinds: Vec::new(),
            cost: 0.0,
        };
    }

    // The program is O(n²) in the measured points with a constant set by two cubic
    // evaluations per span, and its cut-off never fires on a smooth arc, where a cubic
    // spanning most of the loop is still a passable fit. A ring of a 512px image is
    // 1,500 points; eleven of them took eleven seconds. Long boundaries are therefore
    // decimated for the program only: one point per sampling cell, with sigma scaled so
    // each kept point carries the weight of the run it stands for and χ² keeps its
    // meaning against λ. Nothing downstream depends on the vertex grid being 1px — the
    // path is polished against the image afterwards. Preserve significant bends within
    // each cell: blindly keeping every stride-th sample clips exact corners and makes
    // even a noiseless square require 6-8 segments instead of four. The capped variant
    // is exempt: the
    // self-intersection repair relies on the measured contour being reproducible at
    // `max_span = 1`, and a decimated contour is not simple by construction.
    let stride = if max_span == usize::MAX {
        n.div_ceil(DP_MAX_POINTS)
    } else {
        1
    };
    if stride > 1 {
        let keep = crate::decimate::indices(poly, stride, cfg);
        let w = (stride as f64).sqrt();
        let dec = Polyline {
            points: keep.iter().map(|&i| poly.points[i]).collect(),
            sigma: keep.iter().map(|&i| poly.sigma[i] / w).collect(),
            closed: poly.closed,
        };
        let mut fit = optimal_multimodel_impl(&dec, cfg, max_span, structural);
        for v in &mut fit.vertices {
            *v = keep[*v];
        }
        return fit;
    }

    // Work about the bounding-box centre. The Green's-theorem sums involve x·y·dx, which
    // for a 256px canvas reaches 1e7 per edge; the region moments we want are
    // differences of such sums and are only ~chord³. Centring keeps the cancellation
    // well inside f64's precision. The objective is translation invariant.
    let (mut lo, mut hi) = (
        (f64::INFINITY, f64::INFINITY),
        (f64::NEG_INFINITY, f64::NEG_INFINITY),
    );
    for p in &poly.points {
        lo = (lo.0.min(p.x), lo.1.min(p.y));
        hi = (hi.0.max(p.x), hi.1.max(p.y));
    }
    let centre = Vec2 {
        x: 0.5 * (lo.0 + hi.0),
        y: 0.5 * (lo.1 + hi.1),
    };
    let shifted = Polyline {
        points: poly
            .points
            .iter()
            .map(|p| Point::new(p.x - centre.x, p.y - centre.y))
            .collect(),
        sigma: poly.sigma.clone(),
        closed: poly.closed,
    };

    let mut fit = if poly.closed && n >= 3 {
        solve_closed(&shifted, cfg, max_span)
    } else {
        let tan = estimate_tangents(&shifted, cfg);
        let (sol, path) = solve_and_refine(&shifted, &tan, cfg, false, max_span);
        MultimodelFit {
            path,
            vertices: sol.vertices,
            kinds: sol.kinds,
            cost: sol.cost,
        }
    };

    // Corners come out of the program as chord + cubic + chord, because a cubic here is
    // G1 and the tangent it inherits at a corner is wrong. Merge such runs into one cubic
    // with free tangents wherever the same objective prefers it. Done here, on the
    // centred polyline the fit was computed against, and before anything is translated
    // back. See `crate::merge`.
    //
    // Not under a span cap. The cap exists for the self-intersection repair, which needs
    // the constrained program's own answer: the merge re-joins short runs into free
    // cubics that can cross again, so the repair never converged, and its cost grows
    // with the segment count the cap produces - 1-2 s per round on a 250-point ring at
    // span 7, against 0.2 ms for the program itself (family emoji: 11.8 s in repair).
    if max_span == usize::MAX {
        crate::merge::merge_free_cubics(&mut fit.path, &shifted, &fit.vertices, cfg);
        // The opposite failure at sharp corners: a cubic through the anti-aliasing
        // chamfer where two lines meet. See `crate::merge::sharpen_corners`.
        crate::merge::sharpen_corners(&mut fit.path);
        // A line the measurement cannot tell from axis-aligned has one degree of freedom,
        // not two. See `crate::merge::snap_axis_aligned`.
        if std::env::var("INKVEC_AXIS").is_ok_and(|v| v != "0") {
            crate::merge::snap_axis_aligned(&mut fit.path, &shifted, &fit.vertices, cfg);
        }
        // A cubic that continues the one before it smoothly needs four numbers, not six.
        // See `crate::merge::snap_smooth_joins`.
        if std::env::var("INKVEC_G1").is_ok_and(|v| v != "0") {
            crate::merge::snap_smooth_joins(&mut fit.path, &shifted, &fit.vertices, cfg);
        }
        // Structural MDL rate-distortion simplification across 4 primitives.
        // Experimental: keep opt-in until acceptance uses calibrated image evidence.
        if structural {
            crate::structural::simplify_with_poly(&mut fit.path, &shifted, &fit.vertices, cfg);
        }
    }

    let back = |p: Point| Point::new(p.x + centre.x, p.y + centre.y);
    fit.path.start = back(fit.path.start);
    for s in &mut fit.path.segments {
        *s = match *s {
            Segment::Line(p) => Segment::Line(back(p)),
            Segment::Cubic(a, b, c) => Segment::Cubic(back(a), back(b), back(c)),
            // Arcs are translation-invariant apart from their endpoint. This DP does not
            // emit them itself; the arm exists so a path that has been through the
            // primitive fitter can still be translated back.
            Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end,
            } => Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end: back(end),
            },
        };
    }
    fit
}

// --- prefix sums ---------------------------------------------------------------------

/// Everything a candidate span needs, in O(1).
///
/// The six weighted sums make the line residual O(1) (the same construction as
/// `PrefixSums` in the crate root); the three Green's-theorem partials make the area
/// and first moments of any sub-polyline O(1); the arc length gives each point's initial
/// parameter on a candidate cubic in O(1).
struct Prefix {
    w: Vec<f64>,
    x: Vec<f64>,
    y: Vec<f64>,
    xx: Vec<f64>,
    yy: Vec<f64>,
    xy: Vec<f64>,
    /// `ga[k]` = Σ over edges `< k` of ∫ y dx; likewise `gx` for ∫ x y dx, `gy` for ∫ y² dx.
    ga: Vec<f64>,
    gx: Vec<f64>,
    gy: Vec<f64>,
    s: Vec<f64>,
}

impl Prefix {
    fn new(pts: &[Point], sigma: &[f64]) -> Self {
        let n = pts.len();
        let mut p = Prefix {
            w: vec![0.0; n + 1],
            x: vec![0.0; n + 1],
            y: vec![0.0; n + 1],
            xx: vec![0.0; n + 1],
            yy: vec![0.0; n + 1],
            xy: vec![0.0; n + 1],
            ga: vec![0.0; n],
            gx: vec![0.0; n],
            gy: vec![0.0; n],
            s: arc_lengths(pts),
        };
        for k in 0..n {
            let q = pts[k];
            let iv = 1.0 / (sigma[k] * sigma[k]);
            p.w[k + 1] = p.w[k] + iv;
            p.x[k + 1] = p.x[k] + q.x * iv;
            p.y[k + 1] = p.y[k] + q.y * iv;
            p.xx[k + 1] = p.xx[k] + q.x * q.x * iv;
            p.yy[k + 1] = p.yy[k] + q.y * q.y * iv;
            p.xy[k + 1] = p.xy[k] + q.x * q.y * iv;
            if k + 1 < n {
                let (a, x, y) = edge_terms(q, pts[k + 1]);
                p.ga[k + 1] = p.ga[k] + a;
                p.gx[k + 1] = p.gx[k] + x;
                p.gy[k + 1] = p.gy[k] + y;
            }
        }
        p
    }

    /// Weighted total-least-squares residual of points `[i, j]` about their best line.
    fn chi2_line(&self, i: usize, j: usize) -> f64 {
        if j <= i {
            return 0.0;
        }
        let (a, b) = (i, j + 1);
        let w = self.w[b] - self.w[a];
        if w <= 0.0 {
            return 0.0;
        }
        let sx = self.x[b] - self.x[a];
        let sy = self.y[b] - self.y[a];
        let sxx = self.xx[b] - self.xx[a];
        let syy = self.yy[b] - self.yy[a];
        let sxy = self.xy[b] - self.xy[a];
        scatter_min_eigen(w, sx, sy, sxx, syy, sxy)
    }

    /// Raw `(∫ y dx, ∫ x y dx, ∫ y² dx)` along the polyline from `i` to `j`.
    #[inline]
    fn raw_moments(&self, i: usize, j: usize) -> (f64, f64, f64) {
        (
            self.ga[j] - self.ga[i],
            self.gx[j] - self.gx[i],
            self.gy[j] - self.gy[i],
        )
    }
}

// --- the dynamic program -------------------------------------------------------------

struct Solution {
    vertices: Vec<usize>,
    kinds: Vec<SegKind>,
    /// Arms `(d0, d1)` for cubic segments, `None` for lines.
    arms: Vec<Option<(f64, f64)>>,
    /// End tangents for cubics the program fitted without inheriting them, `None` where
    /// the estimator's directions were used. An arc always carries its own.
    tans: Vec<Option<(Vec2, Vec2)>>,
    /// `(rx, ry, phi, large_arc, sweep)` for arc segments, `None` for everything else.
    #[allow(clippy::type_complexity)]
    arcs: Vec<Option<(f64, f64, f64, bool, bool)>>,
    cost: f64,
}

/// The dynamic program on an open polyline. `joins_at_ends` marks the endpoints as a
/// join (the cut of a closed loop) so the line tangent terms apply there too.
#[allow(clippy::too_many_arguments)]
/// Why the program searches every endpoint, and what was measured when it was not.
///
/// The scan is the tracer's largest single cost — 978,236 spans evaluated on a 768-px
/// wordmark to keep 448 segments, a ratio of two thousand to one — so it looks like the
/// obvious place to be cleverer. Four ways of being cleverer were tried on the 246-icon
/// gate. Three are refuted, and the fourth says where the real constraint is.
///
/// - **Cap the reach.** The scan's own cut-off already stops the median start after 51
///   points, so a hard cap at 128 saves only about a tenth of the time; at 64 it saves
///   more but forces extra segments, and the parameter ratio goes to 1.390 against a
///   limit of 1.353. The reach is not the waste.
/// - **Assume the optimal predecessor is monotone** — the Knuth/quadrangle condition that
///   would let a windowed search replace the scan, and turn the program into O(n log n).
///   It does not hold: 11.7% of transitions move the predecessor backwards, by as much as
///   253 points. A window would be wrong, not merely approximate.
/// - **Propose breakpoints from a cheap line-only polygon.** Poor recall, and structurally
///   so: at a quarter of lambda the polygon proposes 18% of the points and contains only
///   65% of the breaks this program chooses. A break between two cubics sits where the
///   description length trades, not where the geometry has a corner, so a corner detector
///   is looking for the wrong thing.
/// - **Coarsen where a break may fall.** Allowing breaks only on even indices — corners
///   and every local curvature maximum exempted — costs dE00 0.1497 -> 0.2207 at almost
///   unchanged parameter count. Half the resolution, half again the error.
///
/// That last one is the finding. Placement matters to the point because these cubics are
/// G1: the end tangent *directions* come from a per-vertex estimator and only the arm
/// lengths are fitted (see [`try_free_cubic`]). Moving a break one point changes the model
/// the span is offered, not just the partition, and a wrong tangent is not something arm
/// lengths can recover from. The program is searching every endpoint to compensate for a
/// local model that is too rigid.
///
/// So the way to make this cheap is not a better order estimator. It is a local model whose
/// tangents do not have to be guessed — which is exactly the model `try_free_cubic`
/// already implements and which is switched off because it fits correlated contour noise
/// too faithfully. A noise model that knows the error along a boundary is correlated would
/// buy both: a cubic that no longer needs the perfect endpoint, and then a coarse candidate
/// grid that costs g-squared less to search.
fn solve_open(
    pts: &[Point],
    sigma: &[f64],
    tan: &Tangents,
    pre: &Prefix,
    cfg: &FitConfig,
    joins_at_ends: bool,
    max_span: usize,
) -> Solution {
    let n = pts.len();
    let mut best = vec![f64::INFINITY; n];
    let mut from = vec![usize::MAX; n];
    let mut kind = vec![SegKind::Line; n];
    let mut arms: Vec<Option<(f64, f64)>> = vec![None; n];
    let mut tans: Vec<Option<(Vec2, Vec2)>> = vec![None; n];
    #[allow(clippy::type_complexity)]
    let mut arcs: Vec<Option<(f64, f64, f64, bool, bool)>> = vec![None; n];
    best[0] = 0.0;
    let cubic_floor = cfg.lambda * params_cubic();
    let arc_floor = cfg.lambda * crate::curves::PARAMS_ARC;
    let circles = arcs_enabled().then(|| CirclePrefix::new(pts, sigma));

    for i in 0..n - 1 {
        if !best[i].is_finite() {
            continue;
        }
        // Leaving `i` makes it a vertex; that is when its turn is paid.
        let base = best[i] + if i > 0 { vertex_cost(tan, i, cfg) } else { 0.0 };
        let t0 = tan.outgoing[i];
        let mut over = 0usize;

        for j in i + 1..(i.saturating_add(max_span).saturating_add(1)).min(n) {
            let chi2_l = pre.chi2_line(i, j);
            let line_plain = line_cost_terms(pts, tan, i, j, chi2_l, cfg, joins_at_ends);
            // The circle is asked for first, because what it finds is evidence about the
            // line: see `bow_penalty`. It is O(1) from the moment sums, so asking costs
            // nothing but the guards.
            // The price floor is a proof, not a heuristic: an arc costs at least its own
            // 5 lambda, so a span the line already covers for less can never take one.
            let circle = if arcs_enabled()
                && j >= i + 2
                && (line_plain > arc_floor || chi2_l > (j - i) as f64)
            {
                circles
                    .as_ref()
                    .and_then(|pre| try_arc(pts, tan, pre, i, j, cfg, joins_at_ends))
            } else {
                None
            };
            let line = line_plain
                + circle
                    .as_ref()
                    .map_or(0.0, |f| bow_penalty(chi2_l, f.chi2, j - i));
            let mut c = base + line;
            let mut k = SegKind::Line;
            let mut a = None;
            let mut tv: Option<(Vec2, Vec2)> = None;
            let mut arc: Option<(f64, f64, f64, bool, bool)> = None;
            let mut chi2_a = f64::INFINITY;

            // A cubic needs an interior point to be worth anything, and costs `6λ` before
            // any residual: if the line already costs less, the residual is never
            // evaluated. This is the O(1) pre-check that keeps straight runs cheap.
            let mut chi2_c = f64::INFINITY;
            let mut cubic_tried = false;
            if j >= i + 2 && line > cubic_floor {
                cubic_tried = true;
                if let Some((chi2, d0, d1)) = best_cubic(
                    pts,
                    sigma,
                    &pre.s,
                    i,
                    j,
                    t0,
                    tan.incoming[j],
                    pre.raw_moments(i, j),
                    true,
                ) {
                    chi2_c = chi2;
                    let chord = (pts[j] - pts[i]).norm();
                    let cb = Cubic::from_arms(pts[i], pts[j], t0, tan.incoming[j], chord, d0, d1);
                    let wobble = cb.wobble_penalty(cfg.lambda);
                    let cc = base + 0.5 * chi2 + cubic_floor + wobble;
                    if cc < c {
                        c = cc;
                        k = SegKind::Cubic;
                        a = Some((d0, d1));
                    }
                } else {
                    cubic_tried = false; // no admissible arms: not evidence of hopelessness
                }

                // The same span with the tangent directions fitted rather than
                // inherited. It gives up G1 with its neighbours, so it pays for the two
                // breaks it makes, and only wins if the residual it saves is worth more
                // than the smoothness it costs.
                if let Some(f) = try_free_cubic(
                    pts,
                    sigma,
                    &pre.s,
                    i,
                    j,
                    t0,
                    tan.incoming[j],
                    cfg.lambda,
                    true,
                ) {
                    let cc = base + 0.5 * f.chi2 + cubic_floor + f.brk;
                    if cc < c {
                        c = cc;
                        k = SegKind::Cubic;
                        a = Some(f.arms);
                        tv = Some(f.tans);
                    }
                    if f.chi2 < chi2_c {
                        chi2_c = f.chi2;
                    }
                }
            }

            // The arc, tried on the same terms as the cubic: only once the line is
            // already paying more than the arc's price, so straight runs never fit a circle.
            if let Some(f) = &circle {
                chi2_a = f.chi2;
                let cc = base + f.cost;
                if cc < c {
                    c = cc;
                    k = SegKind::Arc;
                    a = None;
                    tv = Some(f.tans);
                    arc = Some((f.radius, f.radius, 0.0, f.large_arc, f.sweep));
                }
            }

            // An ellipse is asked about only where a circle has not already described the
            // span — it is two parameters dearer, and a boundary a circle fits is not an
            // ellipse's to claim. "Has not" includes the circle declining outright: a
            // strongly elliptical run is exactly the shape whose angles about a *circle's*
            // centre do not advance monotonically, so the circular candidate returns
            // nothing and the ellipse has to be reached anyway.
            let circle_fits = circle
                .as_ref()
                .is_some_and(|f| f.chi2 <= 4.0 * (j - i) as f64);
            // And the same proof, sharpened: what an ellipse has to beat is the best
            // candidate so far, not the line. A cubic that already covers the span for
            // less than an ellipse's seven lambda settles it without a conic being fitted
            // at all, which is most spans of a letterform — measured on a 768-px wordmark,
            // the ellipse was 635 ms of the fitter's 1439.
            let ellipse_floor = cfg.lambda * crate::curves::PARAMS_ELLIPTICAL_ARC;
            if arcs_enabled()
                && ellipses_enabled()
                && !circle_fits
                && j >= i + 2
                && c - base > ellipse_floor
            {
                if let Some(e) = try_ellipse(pts, sigma, tan, i, j, cfg, joins_at_ends) {
                    let cc = base + e.cost;
                    if cc < c {
                        c = cc;
                        k = SegKind::Arc;
                        a = None;
                        tv = Some(e.tans);
                        arc = Some((e.rx, e.ry, e.phi, e.large_arc, e.sweep));
                    }
                }
            }

            let _ = chi2_a;

            if c < best[j] {
                best[j] = c;
                from[j] = i;
                kind[j] = k;
                arms[j] = a;
                tans[j] = tv;
                arcs[j] = arc;
            }

            // Why does a line win where the boundary curves? Dumps the two models' own
            // numbers for every span considered, so the answer comes from the program
            // rather than from a story about it.
            if dp_debug() && j >= i + 2 {
                println!(
                    "DP {i} {j} span {} line_chi2 {:.3} cubic_chi2 {:.3} line_cost {:.3} cubic_cost {:.3} chose {}",
                    j - i,
                    chi2_l,
                    chi2_c,
                    line,
                    if chi2_c.is_finite() {
                        0.5 * chi2_c + cubic_floor
                    } else {
                        f64::INFINITY
                    },
                    if matches!(k, SegKind::Cubic) { "cubic" } else { "line" }
                );
            }

            // Search cut-off derived from the objective (see `optimal_polygon`): covering
            // `i..j` with the finest segmentation costs at least `2λ(j−i)`, so once both
            // models' fidelity terms alone exceed that by the slack, no longer span from
            // `i` can win. Both models must be over the bound — a line blows up at the
            // first bend while the cubic is still fine.
            let floor = PRUNE_SLACK * cfg.lambda * PARAMS_LINE * (j - i) as f64;
            if 0.5 * chi2_l > floor && cubic_tried && 0.5 * chi2_c > floor {
                over += 1;
                if over >= PRUNE_PATIENCE {
                    break;
                }
            } else {
                over = 0;
            }
        }
    }

    let mut vertices = Vec::new();
    let mut kinds = Vec::new();
    let mut seg_arms = Vec::new();
    let mut seg_tans = Vec::new();
    let mut seg_arcs = Vec::new();
    let mut cur = n - 1;
    while cur != usize::MAX {
        vertices.push(cur);
        if cur == 0 {
            break;
        }
        kinds.push(kind[cur]);
        seg_arms.push(arms[cur]);
        seg_tans.push(tans[cur]);
        seg_arcs.push(arcs[cur]);
        cur = from[cur];
    }
    vertices.reverse();
    kinds.reverse();
    seg_arms.reverse();
    seg_tans.reverse();
    seg_arcs.reverse();
    Solution {
        vertices,
        kinds,
        arms: seg_arms,
        tans: seg_tans,
        arcs: seg_arcs,
        cost: best[n - 1],
    }
}

/// Cost of one candidate segment, computed without prefix sums.
///
/// O(j − i). This is the independent implementation the exhaustive test checks the
/// dynamic program against: the residual and the quartic are shared, the bookkeeping —
/// prefix sums, moment differencing, pruning — is not.
pub fn segment_cost_direct(
    poly: &Polyline,
    tan: &Tangents,
    i: usize,
    j: usize,
    kind: SegKind,
    cfg: &FitConfig,
    joins_at_ends: bool,
) -> f64 {
    let pts = &poly.points;
    if j <= i || j >= pts.len() {
        return f64::INFINITY;
    }
    match kind {
        // The arc's own fitter is already independent of the prefix sums: it reads the
        // points of the span and nothing else, so there is nothing to re-derive here.
        SegKind::Arc => {
            let pre = CirclePrefix::new(pts, &poly.sigma);
            try_arc(pts, tan, &pre, i, j, cfg, joins_at_ends).map_or(f64::INFINITY, |a| a.cost)
        }
        SegKind::Line => {
            let (mut w, mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
            for k in i..=j {
                let iv = 1.0 / (poly.sigma[k] * poly.sigma[k]);
                let p = pts[k];
                w += iv;
                sx += p.x * iv;
                sy += p.y * iv;
                sxx += p.x * p.x * iv;
                syy += p.y * p.y * iv;
                sxy += p.x * p.y * iv;
            }
            let chi2 = scatter_min_eigen(w, sx, sy, sxx, syy, sxy);
            let plain = line_cost_terms(pts, tan, i, j, chi2, cfg, joins_at_ends);
            // The same evidence the program charges: a line whose residuals all bow one
            // way is not a line. Computed here from scratch, like everything else in this
            // function.
            let arc_floor = cfg.lambda * crate::curves::PARAMS_ARC;
            let bow =
                if arcs_enabled() && j >= i + 2 && (plain > arc_floor || chi2 > (j - i) as f64) {
                    let pre = CirclePrefix::new(pts, &poly.sigma);
                    try_arc(pts, tan, &pre, i, j, cfg, joins_at_ends)
                        .map_or(0.0, |f| bow_penalty(chi2, f.chi2, j - i))
                } else {
                    0.0
                };
            plain + bow
        }
        SegKind::Cubic => {
            if j < i + 2 {
                return f64::INFINITY;
            }
            let s = arc_lengths(pts);
            let free = try_free_cubic(
                pts,
                &poly.sigma,
                &s,
                i,
                j,
                tan.outgoing[i],
                tan.incoming[j],
                cfg.lambda,
                true,
            )
            .map(|f| 0.5 * f.chi2 + cfg.lambda * params_cubic() + f.brk)
            .unwrap_or(f64::INFINITY);
            match best_cubic(
                pts,
                &poly.sigma,
                &s,
                i,
                j,
                tan.outgoing[i],
                tan.incoming[j],
                raw_moments_direct(pts, i, j),
                false,
            ) {
                Some((chi2, d0, d1)) => {
                    let chord = (pts[j] - pts[i]).norm();
                    let cb = Cubic::from_arms(
                        pts[i],
                        pts[j],
                        tan.outgoing[i],
                        tan.incoming[j],
                        chord,
                        d0,
                        d1,
                    );
                    let wobble = cb.wobble_penalty(cfg.lambda);
                    (0.5 * chi2 + cfg.lambda * params_cubic() + wobble).min(free)
                }
                // No admissible G1 arms does not mean no admissible cubic.
                None => free,
            }
        }
    }
}

// --- continuous refinement ----------------------------------------------------------

/// Minimize the full residual over the two arm lengths, tangents fixed.
///
/// The moment-matched arms are the initializer the design calls for (§S4: algebraic
/// initialization, orthogonal-distance refinement); this is the refinement. Newton on
/// two parameters with finite differences, with a gradient fallback when the Hessian
/// is not positive definite, and a backtracking line search.
#[allow(clippy::too_many_arguments)]
fn polish_arms(
    pts: &[Point],
    sigma: &[f64],
    s: &[f64],
    i: usize,
    j: usize,
    t0: Vec2,
    t1: Vec2,
    arms: (f64, f64),
) -> ((f64, f64), f64) {
    let chord = pts[i].dist(pts[j]);
    let f = |d: (f64, f64)| -> f64 {
        let cb = Cubic::from_arms(pts[i], pts[j], t0, t1, chord, d.0, d.1);
        chi2_cubic(pts, sigma, s, i, j, &cb, false)
    };
    let clamp = |d: (f64, f64)| (d.0.clamp(1e-3, 1.5), d.1.clamp(1e-3, 1.5));
    let mut d = clamp(arms);
    let mut cur = f(d);
    let h = 1e-3;
    for _ in 0..12 {
        let fp0 = f((d.0 + h, d.1));
        let fm0 = f((d.0 - h, d.1));
        let fp1 = f((d.0, d.1 + h));
        let fm1 = f((d.0, d.1 - h));
        let fpp = f((d.0 + h, d.1 + h));
        let g = ((fp0 - fm0) / (2.0 * h), (fp1 - fm1) / (2.0 * h));
        let h00 = (fp0 - 2.0 * cur + fm0) / (h * h);
        let h11 = (fp1 - 2.0 * cur + fm1) / (h * h);
        let h01 = (fpp - fp0 - fp1 + cur) / (h * h);
        let det = h00 * h11 - h01 * h01;
        let step = if h00 > 0.0 && det > 0.0 {
            (
                -(h11 * g.0 - h01 * g.1) / det,
                -(h00 * g.1 - h01 * g.0) / det,
            )
        } else {
            let gn = g.0.hypot(g.1).max(1e-12);
            (-0.05 * g.0 / gn, -0.05 * g.1 / gn)
        };
        let mut alpha = 1.0;
        let mut improved = false;
        while alpha > 1e-4 {
            let cand = clamp((d.0 + alpha * step.0, d.1 + alpha * step.1));
            let v = f(cand);
            if v < cur {
                let gain = cur - v;
                d = cand;
                cur = v;
                improved = gain > 1e-9 * cur.max(1.0);
                break;
            }
            alpha *= 0.5;
        }
        if !improved {
            break;
        }
    }
    (d, cur)
}

/// Turn the program's discrete decisions into geometry, then improve the continuous
/// parameters the program could not optimize:
///
/// 1. line–line corners move to the intersection of their fitted lines
///    ([`adjust_vertices_at`], as in [`crate::fit_path`]);
/// 2. each cubic's arms are polished against the full residual ([`polish_arms`]);
/// 3. at joins the program left smooth (break below [`G1_BREAK_DEGREES`]) the shared
///    tangent is re-estimated symmetrically, or set to the adjacent line's direction, so
///    the emitted path is exactly G1 there; accepted only when it does not cost more
///    residual than the break it removes.
///
/// `poly` must be the (opened) polyline the solution indexes into.
fn refine(
    poly: &Polyline,
    tan: &Tangents,
    pre: &Prefix,
    sol: &Solution,
    cfg: &FitConfig,
) -> FittedPath {
    let pts = &poly.points;
    let sigma = &poly.sigma;
    let s = &pre.s;
    let v = &sol.vertices;
    let m = v.len();
    let nseg = m - 1;
    // Corner adjustment and smooth joins treat the cut of an opened loop as a vertex
    // like any other (see `spans_loop`); the emitted path keeps the caller's notion.
    let closed = crate::spans_loop(poly, v);
    let emitted_closed = poly.closed && m >= 2 && v.first() == v.last();

    // 1. Corners.
    let seg = Segmentation {
        vertices: v.clone(),
        cost: sol.cost,
    };
    let kinds = &sol.kinds;
    let is_corner = |k: usize| -> bool {
        let (a, b) = if closed {
            ((k + nseg - 1) % nseg, k % nseg)
        } else {
            if k == 0 || k >= m - 1 {
                return false;
            }
            (k - 1, k)
        };
        kinds[a] == SegKind::Line && kinds[b] == SegKind::Line
    };
    let arcs = &sol.arcs;
    let max_shift = 3.0 * sigma.iter().copied().fold(0.0, f64::max).max(0.25);
    let pos = adjust_vertices_at(poly, &seg, max_shift, is_corner);

    // Per-segment tangents (cubics only) and arms.
    // A cubic the program fitted with free tangents carries its own directions; the rest
    // inherit the estimator's, as before.
    let mut t_start: Vec<Vec2> = (0..nseg)
        .map(|q| {
            sol.tans
                .get(q)
                .and_then(|t| *t)
                .map_or(tan.outgoing[v[q]], |t| t.0)
        })
        .collect();
    let mut t_end: Vec<Vec2> = (0..nseg)
        .map(|q| {
            sol.tans
                .get(q)
                .and_then(|t| *t)
                .map_or(tan.incoming[v[q + 1]], |t| t.1)
        })
        .collect();
    let mut arms: Vec<Option<(f64, f64)>> = sol.arms.clone();
    let mut chi2: Vec<f64> = vec![0.0; nseg];

    // 2. Arms.
    for q in 0..nseg {
        if let Some(a) = arms[q] {
            let (d, c) = polish_arms(pts, sigma, s, v[q], v[q + 1], t_start[q], t_end[q], a);
            arms[q] = Some(d);
            chi2[q] = c;
        }
    }

    // 3. Smooth joins.
    let line_dir = |q: usize| -> Option<Vec2> { unit(pos[q + 1] - pos[q]) };
    let solve_seg = |q: usize, t0: Vec2, t1: Vec2| -> Option<((f64, f64), f64)> {
        let (i, j) = (v[q], v[q + 1]);
        let (_, d0, d1) = best_cubic(pts, sigma, s, i, j, t0, t1, pre.raw_moments(i, j), false)?;
        Some(polish_arms(pts, sigma, s, i, j, t0, t1, (d0, d1)))
    };
    let joins: Vec<usize> = if closed {
        (0..nseg).collect()
    } else {
        (1..nseg).collect()
    };
    for &k in &joins {
        let (a, b) = if closed {
            ((k + nseg - 1) % nseg, k % nseg)
        } else {
            (k - 1, k)
        };
        if a == b {
            continue;
        }
        let out_a = match kinds[a] {
            SegKind::Line => line_dir(a),
            SegKind::Cubic | SegKind::Arc => Some(t_end[a]),
        };
        let in_b = match kinds[b] {
            SegKind::Line => line_dir(b),
            SegKind::Cubic | SegKind::Arc => Some(t_start[b]),
        };
        let (Some(out_a), Some(in_b)) = (out_a, in_b) else {
            continue;
        };
        let angle = turn_angle(out_a, in_b);
        if angle >= g1_break_radians() {
            continue;
        }
        // An arc's direction is its own: it is the circle the points fit, and turning
        // its end to meet a neighbour would move geometry the residual already settled.
        // So a join with an arc on either side is left alone.
        if matches!(kinds[a], SegKind::Arc) || matches!(kinds[b], SegKind::Arc) {
            continue;
        }
        let target = match (kinds[a], kinds[b]) {
            (SegKind::Line, SegKind::Line) => continue,
            (SegKind::Line, SegKind::Cubic) => out_a,
            (SegKind::Cubic, SegKind::Line) => in_b,
            (SegKind::Arc, _) | (_, SegKind::Arc) => continue,
            (SegKind::Cubic, SegKind::Cubic) => {
                let half = ((v[a + 1] - v[a]) / 2).min((v[b + 1] - v[b]) / 2).max(1);
                symmetric_tangent(poly, v[k % m], half, cfg)
                    .or_else(|| {
                        unit(Vec2 {
                            x: out_a.x + in_b.x,
                            y: out_a.y + in_b.y,
                        })
                    })
                    .unwrap_or(out_a)
            }
        };
        let old_break = break_cost(out_a, in_b, cfg.lambda);
        let mut new_a = None;
        let mut new_b = None;
        let mut old_chi2 = 0.0;
        let mut new_chi2 = 0.0;
        if kinds[a] == SegKind::Cubic {
            let Some(r) = solve_seg(a, t_start[a], target) else {
                continue;
            };
            old_chi2 += chi2[a];
            new_chi2 += r.1;
            new_a = Some(r);
        }
        if kinds[b] == SegKind::Cubic {
            let Some(r) = solve_seg(b, target, t_end[b]) else {
                continue;
            };
            old_chi2 += chi2[b];
            new_chi2 += r.1;
            new_b = Some(r);
        }
        if 0.5 * new_chi2 <= 0.5 * old_chi2 + old_break {
            if let Some((d, c)) = new_a {
                t_end[a] = target;
                arms[a] = Some(d);
                chi2[a] = c;
            }
            if let Some((d, c)) = new_b {
                t_start[b] = target;
                arms[b] = Some(d);
                chi2[b] = c;
            }
        }
    }

    // Assemble.
    let mut segments = Vec::with_capacity(nseg);
    for q in 0..nseg {
        match (kinds[q], arms[q]) {
            (SegKind::Arc, _) => match arcs.get(q).and_then(|a| *a) {
                Some((rx, ry, phi, large_arc, sweep)) => segments.push(Segment::Arc {
                    rx,
                    ry,
                    phi,
                    large_arc,
                    sweep,
                    end: pos[q + 1],
                }),
                None => segments.push(Segment::Line(pos[q + 1])),
            },
            (SegKind::Cubic, Some((d0, d1))) => {
                let (i, j) = (v[q], v[q + 1]);
                let cb = Cubic::from_arms(
                    pts[i],
                    pts[j],
                    t_start[q],
                    t_end[q],
                    pts[i].dist(pts[j]),
                    d0,
                    d1,
                );
                segments.push(Segment::Cubic(cb.p1, cb.p2, pos[q + 1]));
            }
            _ => segments.push(Segment::Line(pos[q + 1])),
        }
    }
    FittedPath {
        start: pos[0],
        segments,
        closed: emitted_closed,
    }
}

fn solve_and_refine(
    poly: &Polyline,
    tan: &Tangents,
    cfg: &FitConfig,
    joins_at_ends: bool,
    max_span: usize,
) -> (Solution, FittedPath) {
    let t0 = inkvec_core::clock::Instant::now();
    let pre = Prefix::new(&poly.points, &poly.sigma);
    let sol = solve_open(
        &poly.points,
        &poly.sigma,
        tan,
        &pre,
        cfg,
        joins_at_ends,
        max_span,
    );
    let t1 = t0.elapsed();
    let path = refine(poly, tan, &pre, &sol, cfg);
    if max_span != usize::MAX && std::env::var_os("INKVEC_TIMING").is_some() {
        eprintln!(
            "  [t]   capped fit n={} span={} segs={}: solve {:.1} ms, refine {:.1} ms",
            poly.len(),
            max_span,
            sol.vertices.len().saturating_sub(1),
            t1.as_secs_f64() * 1e3,
            (t0.elapsed() - t1).as_secs_f64() * 1e3
        );
    }
    (sol, path)
}

// --- closed loops -----------------------------------------------------------------

/// Open a closed polyline at `cut`, duplicating the cut vertex at the end, with the
/// wrapped tangent estimates re-indexed to match.
fn open_at(poly: &Polyline, tan: &Tangents, cut: usize) -> (Polyline, Tangents) {
    let n = poly.len();
    let mut points = Vec::with_capacity(n + 1);
    let mut sigma = Vec::with_capacity(n + 1);
    let mut incoming = Vec::with_capacity(n + 1);
    let mut outgoing = Vec::with_capacity(n + 1);
    for k in 0..=n {
        let i = (cut + k) % n;
        points.push(poly.points[i]);
        sigma.push(poly.sigma[i]);
        incoming.push(tan.incoming[i]);
        outgoing.push(tan.outgoing[i]);
    }
    (
        // Marked closed so vertex adjustment treats the cut as a join.
        Polyline::new(points, sigma, true),
        Tangents { incoming, outgoing },
    )
}

/// Closed loops: cut, solve the open problem, and try once more from a better cut.
///
/// The true optimum is a minimum-cost *cycle*; fixing a cut vertex is an approximation
/// that can cost one segment when the cut lands mid-curve (a rounded rectangle cut in
/// the middle of a fillet needs an extra cubic for that fillet). Two mitigations, both
/// heuristic: the first cut is placed at the sharpest corner when there is one, and
/// otherwise at the point farthest from the centroid; then the program is run again
/// from the chosen vertex farthest from that cut — a vertex of a near-optimal solution
/// is a far better cut than an arbitrary point — and the cheaper result is kept.
/// Potrace solves the cyclic problem exactly; that remains future work.
fn solve_closed(poly: &Polyline, cfg: &FitConfig, max_span: usize) -> MultimodelFit {
    let n = poly.len();
    let tan = estimate_tangents(poly, cfg);

    let sharpest = (0..n)
        .max_by(|&a, &b| {
            turn_angle(tan.incoming[a], tan.outgoing[a])
                .partial_cmp(&turn_angle(tan.incoming[b], tan.outgoing[b]))
                .unwrap()
        })
        .unwrap_or(0);
    let cut1 = if turn_angle(tan.incoming[sharpest], tan.outgoing[sharpest]) >= g1_break_radians() {
        sharpest
    } else {
        let cx = poly.points.iter().map(|p| p.x).sum::<f64>() / n as f64;
        let cy = poly.points.iter().map(|p| p.y).sum::<f64>() / n as f64;
        let c = Point::new(cx, cy);
        (0..n)
            .max_by(|&a, &b| {
                poly.points[a]
                    .dist(c)
                    .partial_cmp(&poly.points[b].dist(c))
                    .unwrap()
            })
            .unwrap_or(0)
    };

    let run = |cut: usize| -> MultimodelFit {
        let (opened, otan) = open_at(poly, &tan, cut);
        let (sol, path) = solve_and_refine(&opened, &otan, cfg, true, max_span);
        MultimodelFit {
            path,
            vertices: sol.vertices.iter().map(|&i| (cut + i) % n).collect(),
            kinds: sol.kinds,
            cost: sol.cost + vertex_cost(&tan, cut, cfg),
        }
    };

    let first = run(cut1);
    let circ = |a: usize, b: usize| -> usize {
        let d = (a + n - b) % n;
        d.min(n - d)
    };
    let cut2 = first
        .vertices
        .iter()
        .copied()
        .max_by_key(|&v| circ(v, cut1))
        .unwrap_or(cut1);
    if cut2 == cut1 {
        return first;
    }
    let second = run(cut2);
    if second.cost < first.cost {
        second
    } else {
        first
    }
}

// --- evaluation -------------------------------------------------------------------

/// Weighted chi-squared of a fitted path against the measured points, by exact nearest
/// distance to each segment (kurbo's `nearest`), taking the closest segment per point.
///
/// This is the evaluator used to compare fitters. It is deliberately not the sampled
/// distance of `curves::chi2`, whose 0.25px sample spacing puts a floor of roughly two
/// units of chi-squared per point at `sigma = 0.05` — larger than the residual of a good
/// fit, and enough to drown the comparison.
pub fn path_chi2(poly: &Polyline, path: &FittedPath) -> f64 {
    let Some(segs) = kurbo_segments(path) else {
        return f64::INFINITY;
    };
    poly.points
        .iter()
        .zip(&poly.sigma)
        .map(|(p, sg)| nearest_dist2(&segs, *p) / (sg * sg))
        .sum()
}

/// Largest distance from any measured point to the path.
pub fn path_max_deviation(poly: &Polyline, path: &FittedPath) -> f64 {
    let Some(segs) = kurbo_segments(path) else {
        return f64::INFINITY;
    };
    poly.points
        .iter()
        .map(|p| nearest_dist2(&segs, *p).sqrt())
        .fold(0.0, f64::max)
}

enum KSeg {
    L(KLine),
    C(CubicBez),
}

fn kurbo_segments(path: &FittedPath) -> Option<Vec<KSeg>> {
    if path.segments.is_empty() {
        return None;
    }
    let kp = |p: Point| KPoint::new(p.x, p.y);
    let mut segs = Vec::with_capacity(path.segments.len());
    let mut cur = path.start;
    for s in &path.segments {
        match *s {
            Segment::Line(p) => {
                segs.push(KSeg::L(KLine::new(kp(cur), kp(p))));
                cur = p;
            }
            Segment::Cubic(a, b, p) => {
                segs.push(KSeg::C(CubicBez::new(kp(cur), kp(a), kp(b), kp(p))));
                cur = p;
            }
            // Sampled finely, not chorded. The chord over-states an arc's distance by its
            // own sagitta — which is the entire reason for drawing one — and that was
            // tolerable only while arcs came from elsewhere. Now the program emits them, so
            // scoring one as its chord would report a clean circle as millions of units of
            // cost and hide whatever it is being compared against.
            Segment::Arc { end, .. } => {
                let pts = crate::curves::sample_run(cur, std::slice::from_ref(s), 0.02);
                for w in pts.windows(2) {
                    segs.push(KSeg::L(KLine::new(kp(w[0]), kp(w[1]))));
                }
                cur = end;
            }
        }
    }
    Some(segs)
}

fn nearest_dist2(segs: &[KSeg], p: Point) -> f64 {
    let q = KPoint::new(p.x, p.y);
    segs.iter()
        .map(|s| match s {
            KSeg::L(l) => l.nearest(q, 1e-9).distance_sq,
            KSeg::C(c) => c.nearest(q, 1e-6).distance_sq,
        })
        .fold(f64::INFINITY, f64::min)
}

/// The MDL objective of a fitted path: `0.5·chi² + λ·params`, parameters counted per
/// segment as the dynamic program counts them (the start point is not charged).
pub fn path_cost(poly: &Polyline, path: &FittedPath, cfg: &FitConfig) -> f64 {
    let params: f64 = path.segments.iter().map(|s| s.params()).sum();
    0.5 * path_chi2(poly, path) + cfg.lambda * params
}
