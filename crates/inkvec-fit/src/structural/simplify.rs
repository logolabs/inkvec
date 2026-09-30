//! Structural MDL simplification pass (see docs/DESIGN.md §4).
//!
//! # Background & Architecture
//!
//! Tracing raster images often produces piecewise boundaries where a curve or line is
//! broken into multiple short segments. While local peephole passes like [`crate::merge`]
//! only attempt free cubics on runs <= 4, this structural simplification pass evaluates
//! 4 primitive candidate models:
//!
//! 1. **Straight Line (`Segment::Line`)**: Connects endpoints directly (rate: 2 params).
//! 2. **Tangent-Constrained Cubic (`Segment::Cubic`)**: Cubic whose tangent directions
//!    match the outside neighbouring segments (G1, not parametric C1), with handle lengths fitted via 2x2
//!    non-negative least squares (NNLS) (rate: 6 params).
//! 3. **Free Cubic (`Segment::Cubic`)**: Linear least-squares cubic through endpoints
//!    with both interior control points free (rate: 6 params).
//! 4. **Circular Arc (`Segment::Arc`)**: Circle through endpoints with center fitted along
//!    the perpendicular bisector via 1D scalar minimization (rate: 5 params).
//!
//! Candidates are evaluated for runs of length 2, 3, 4, 6, 8, 12.
//!
//! # Safety & Quality Invariants
//!
//! - **Tight geometric pre-filter**: Proposals with maximum sample deviation > `max_dist`
//!   (default 0.6 px) are pruned immediately before rate-distortion scoring.
//! - **Tangent continuity guard**: A candidate may not introduce an external kink
//!   exceeding `max(existing_kink, 0.03 rad) + 1e-6`.
//! - **No self-intersection**: Cubics crossing themselves are rejected via
//!   [`crate::curves::cubic_self_intersects`].
//! - **Independent intra-path batching**: Replacements leave their outside neighbours
//!   unchanged and are applied in descending index order, preserving tangent evidence
//!   and index stability, including the seam of a closed path.
//!
//! # Where this sits
//!
//! Research builds only (`INKVEC_STRUCTURAL`): `crate::multimodel` calls
//! [`simplify_with_poly`] last among the post-fit passes, on the centred polyline, and
//! scores candidates against the measured points with the same `½·χ² + λ·params` as the
//! fit. [`simplify_path_structural`] is the geometry-only variant (no measured points:
//! distortion is the candidate's squared distance from the run it replaces) used by the
//! tests and `examples/structural_demo.rs`. Coordinates are px, angles radians.
//!
//! The pre-filter's `max_dist` is 0.6 px in [`StructuralConfig::default`] but 0.85 px
//! from [`StructuralConfig::from_fit_config`] (overridable by
//! `INKVEC_STRUCTURAL_MAX_DIST`), and [`simplify_with_poly`] caps it further by the
//! run's own sigma.

use crate::curves::{arc_ellipse_center, cubic_self_intersects, Segment, PARAMS_ARC};
use crate::{FitConfig, FittedPath, PARAMS_LINE};
use inkvec_core::{Point, Polyline, Vec2};

use super::eval_segment;

/// Parameters charged for a cubic segment.
pub const PARAMS_CUBIC: f64 = 6.0;

/// Default run lengths evaluated for replacement.
pub const DEFAULT_RUN_LENGTHS: [usize; 6] = [2, 3, 4, 6, 8, 12];

/// Configuration for structural simplification.
#[derive(Debug, Clone)]
pub struct StructuralConfig {
    /// Maximum allowable deviation between candidate curve and original polyline samples (default 0.6 px).
    pub max_dist: f64,
    /// Minimum kink threshold in radians (default 0.03 rad ~ 1.7 degrees).
    pub min_kink_rad: f64,
    /// Description length weight lambda (nats per parameter).
    pub lambda: f64,
    /// Parameter cost charged for a circular arc (default 5.0).
    pub arc_params: f64,
    /// Maximum rounds of greedy batch simplification (default 6).
    pub max_rounds: usize,
    /// Run lengths to evaluate.
    pub run_lengths: Vec<usize>,
}

impl Default for StructuralConfig {
    fn default() -> Self {
        Self {
            max_dist: 0.6,
            min_kink_rad: 0.03,
            lambda: 1.0,
            arc_params: PARAMS_ARC,
            max_rounds: 6,
            run_lengths: DEFAULT_RUN_LENGTHS.to_vec(),
        }
    }
}

impl StructuralConfig {
    /// Create config from a [`FitConfig`].
    pub fn from_fit_config(cfg: &FitConfig) -> Self {
        let max_dist = inkvec_core::env::number("INKVEC_STRUCTURAL_MAX_DIST")
            .filter(|v| *v > 0.0)
            .unwrap_or(0.85);
        Self {
            max_dist,
            lambda: cfg.lambda,
            ..Default::default()
        }
    }
}

/// Multiplier applied to `mean_sigma` of a run to cap the structural max-deviation
/// bound for precise (low-sigma) boundaries.  Prevents simplified curves from
/// bulging into neighbouring edges and triggering repair-stage inflation.
///
/// Defaults to 2.0.  Increase to allow looser proposals; decrease to be more
/// conservative.  Set via `INKVEC_STRUCTURAL_SIGMA_CAP`.
fn sigma_cap() -> f64 {
    inkvec_core::env::number("INKVEC_STRUCTURAL_SIGMA_CAP")
        .filter(|&v| v > 0.0)
        .unwrap_or(2.0)
}

/// Compute the unit vector of `v`, or `None` if degenerate.
#[inline]
pub fn unit_vec(v: Vec2) -> Option<Vec2> {
    let n = v.norm();
    if n > 1e-12 {
        Some(Vec2 {
            x: v.x / n,
            y: v.y / n,
        })
    } else {
        None
    }
}

/// Unsigned angle between two 2D vectors in radians [0, pi].
pub fn vector_angle(a: Vec2, b: Vec2) -> f64 {
    let na = a.norm();
    let nb = b.norm();
    if na <= 1e-12 || nb <= 1e-12 {
        return 0.0;
    }
    let dot = (a.dot(b) / (na * nb)).clamp(-1.0, 1.0);
    dot.acos()
}

/// Outgoing tangent unit vector of a segment at its end (t = 1).
///
/// A line's direction; a cubic's last non-degenerate control-polygon leg (`end − c2`,
/// else `end − c1`, else the chord, each needing more than 1e-9 px); an arc's
/// derivative of `c + R(φ)·(rx cos θ, ry sin θ)` at its end angle, in the direction of
/// travel. `None` when the direction has no length.
pub fn segment_tangent_end(seg: &Segment, start: Point) -> Option<Vec2> {
    match *seg {
        Segment::Line(end) => unit_vec(end - start),
        Segment::Cubic(c1, c2, end) => {
            let d = end - c2;
            if d.norm() > 1e-9 {
                unit_vec(d)
            } else {
                let d2 = end - c1;
                if d2.norm() > 1e-9 {
                    unit_vec(d2)
                } else {
                    unit_vec(end - start)
                }
            }
        }
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            let f = arc_ellipse_center(start, rx, ry, phi, large_arc, sweep, end);
            let theta = f.theta1 + f.delta;
            let (sp, cp) = f.phi.sin_cos();
            let (st, ct) = theta.sin_cos();
            let dx_ell = -f.rx * st;
            let dy_ell = f.ry * ct;
            let sign = if f.delta >= 0.0 { 1.0 } else { -1.0 };
            let vx = (cp * dx_ell - sp * dy_ell) * sign;
            let vy = (sp * dx_ell + cp * dy_ell) * sign;
            unit_vec(Vec2 { x: vx, y: vy })
        }
    }
}

/// Incoming tangent unit vector of a segment at its start (t = 0).
///
/// As [`segment_tangent_end`], at the other end: `c1 − start`, else `c2 − start`, else
/// the chord for a cubic, the derivative at the start angle for an arc.
pub fn segment_tangent_start(seg: &Segment, start: Point) -> Option<Vec2> {
    match *seg {
        Segment::Line(end) => unit_vec(end - start),
        Segment::Cubic(c1, c2, end) => {
            let d = c1 - start;
            if d.norm() > 1e-9 {
                unit_vec(d)
            } else {
                let d2 = c2 - start;
                if d2.norm() > 1e-9 {
                    unit_vec(d2)
                } else {
                    unit_vec(end - start)
                }
            }
        }
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            let f = arc_ellipse_center(start, rx, ry, phi, large_arc, sweep, end);
            let theta = f.theta1;
            let (sp, cp) = f.phi.sin_cos();
            let (st, ct) = theta.sin_cos();
            let dx_ell = -f.rx * st;
            let dy_ell = f.ry * ct;
            let sign = if f.delta >= 0.0 { 1.0 } else { -1.0 };
            let vx = (cp * dx_ell - sp * dy_ell) * sign;
            let vy = (sp * dx_ell + cp * dy_ell) * sign;
            unit_vec(Vec2 { x: vx, y: vy })
        }
    }
}

/// Sample a run of segments at roughly uniform arc-length intervals.
///
/// Returns `(samples, total_arc_length)`.
///
/// Each segment is first evaluated at `clamp(48 / segments, 4, 16)` parameters evenly
/// spaced in `t`, ends included; `count` points are then placed at equal steps of the
/// cumulative length of that dense polyline, by linear interpolation, and the first and
/// last are pinned exactly to `start` and the run's end. The length is that of the dense
/// polyline, px. `None` for no segments, `count < 2` or a run of (near) zero length.
pub fn sample_run_uniform(
    start: Point,
    segs: &[Segment],
    count: usize,
) -> Option<(Vec<Point>, f64)> {
    if segs.is_empty() || count < 2 {
        return None;
    }
    let sub_samples = (48 / segs.len()).clamp(4, 16);
    let mut raw_pts = Vec::with_capacity(segs.len() * sub_samples);
    let mut cur = start;
    for s in segs {
        for i in 0..sub_samples {
            let t = i as f64 / (sub_samples - 1) as f64;
            raw_pts.push(eval_segment(s, cur, t));
        }
        cur = s.end();
    }

    let mut cum_dist = Vec::with_capacity(raw_pts.len());
    cum_dist.push(0.0);
    let mut acc = 0.0;
    for w in raw_pts.windows(2) {
        acc += w[0].dist(w[1]);
        cum_dist.push(acc);
    }
    if acc < 1e-8 {
        return None;
    }

    // Resample along cumulative arc length
    let mut out = Vec::with_capacity(count);
    let total_len = acc;
    let step = total_len / (count - 1) as f64;
    let mut cursor = 0usize;

    for k in 0..count {
        let target_s = (k as f64 * step).min(total_len);
        while cursor + 1 < cum_dist.len() && cum_dist[cursor + 1] < target_s {
            cursor += 1;
        }
        if cursor + 1 >= cum_dist.len() {
            out.push(
                *raw_pts
                    .last()
                    .expect("raw_pts holds at least four samples per segment"),
            );
        } else {
            let s0 = cum_dist[cursor];
            let s1 = cum_dist[cursor + 1];
            let span = (s1 - s0).max(1e-12);
            let u = ((target_s - s0) / span).clamp(0.0, 1.0);
            let p0 = raw_pts[cursor];
            let p1 = raw_pts[cursor + 1];
            out.push(Point::new(
                p0.x + (p1.x - p0.x) * u,
                p0.y + (p1.y - p0.y) * u,
            ));
        }
    }

    // Exact pin to endpoints
    if let Some(first) = out.first_mut() {
        *first = start;
    }
    if let Some(last) = out.last_mut() {
        *last = segs
            .last()
            .expect("segs is non-empty: checked on entry")
            .end();
    }
    Some((out, total_len))
}

/// Description length in parameters for a segment under `cfg`.
pub fn segment_rate(seg: &Segment, cfg: &StructuralConfig) -> f64 {
    match seg {
        Segment::Line(_) => PARAMS_LINE,
        Segment::Cubic(..) => crate::multimodel::params_cubic(),
        Segment::Arc { .. } => cfg.arc_params,
    }
}

/// Compute maximum distance from points `samples` to the polyline `ref_pts` using a monotonic cursor.
///
/// Both sequences run along the same curve in the same direction, so the nearest
/// reference point advances monotonically and only a window of `clamp(m/8, 8, 64)`
/// reference points around the expected position needs searching; see
/// `NearestScan::distance`. Infinite when either is empty. px.
pub fn max_deviation_to_samples(samples: &[Point], ref_pts: &[Point]) -> f64 {
    if samples.is_empty() || ref_pts.is_empty() {
        return f64::INFINITY;
    }
    let mut scan = NearestScan::new(samples.len(), ref_pts);
    let mut max_d = 0.0f64;
    for (k, &p) in samples.iter().enumerate() {
        let d = scan.distance(k, p);
        if d > max_d {
            max_d = d;
        }
    }
    max_d
}

/// Check if the maximum deviation from `samples` to `ref_pts` is within `max_dist`, early-exiting on failure.
///
/// The same distance as [`max_deviation_to_samples`]. False when either is empty. The
/// test is one-sided (every sample is near the reference, not the reverse); callers that
/// need both directions call it twice.
pub fn is_deviation_within(samples: &[Point], ref_pts: &[Point], max_dist: f64) -> bool {
    if samples.is_empty() || ref_pts.is_empty() {
        return false;
    }
    let mut scan = NearestScan::new(samples.len(), ref_pts);
    for (k, &p) in samples.iter().enumerate() {
        if scan.distance(k, p) > max_dist {
            return false;
        }
    }
    true
}

/// The nearest-distance search shared by [`max_deviation_to_samples`],
/// [`is_deviation_within`] and [`mean_sq_deviation`].
struct NearestScan<'a> {
    /// The reference polyline.
    ref_pts: &'a [Point],
    /// How many samples will be asked about, to place each one's expected position.
    samples: usize,
    /// Index of the previous sample's nearest reference point.
    cursor: usize,
    /// Half-width of the searched window, in reference points.
    window: usize,
}

impl<'a> NearestScan<'a> {
    /// A scan over `ref_pts` (non-empty) for `samples` samples.
    fn new(samples: usize, ref_pts: &'a [Point]) -> Self {
        let m = ref_pts.len();
        NearestScan {
            ref_pts,
            samples,
            cursor: 0,
            window: (m / 8).clamp(8, 64),
        }
    }

    /// Distance (px) from sample `k`, at `p`, to the reference polyline.
    ///
    /// The nearest reference point is searched within `window` of the larger of the
    /// sample's proportional position `k·(m−1)/(samples−1)` and the previous match less
    /// half a window; the distance is then the least of that point's and the two
    /// adjoining edges', so a point on the curve between two reference points reads as
    /// on it.
    fn distance(&mut self, k: usize, p: Point) -> f64 {
        let ref_pts = self.ref_pts;
        let m = ref_pts.len();
        let window = self.window;
        let guess = if self.samples > 1 {
            (k * (m - 1)) / (self.samples - 1)
        } else {
            0
        };
        let centre = guess.max(self.cursor.saturating_sub(window / 2));
        let lo = centre.saturating_sub(window);
        let hi = (centre + window).min(m - 1);
        let mut best = f64::INFINITY;
        let mut best_i = self.cursor;
        for (i, &q) in ref_pts[lo..=hi].iter().enumerate() {
            let d2 = (p.x - q.x) * (p.x - q.x) + (p.y - q.y) * (p.y - q.y);
            if d2 < best {
                best = d2;
                best_i = lo + i;
            }
        }
        self.cursor = best_i;
        let mut d = best.sqrt();
        if best_i > 0 {
            d = d.min(point_to_segment_dist(
                p,
                ref_pts[best_i - 1],
                ref_pts[best_i],
            ));
        }
        if best_i + 1 < m {
            d = d.min(point_to_segment_dist(
                p,
                ref_pts[best_i],
                ref_pts[best_i + 1],
            ));
        }
        d
    }
}

/// Distance from point `p` to line segment `a -> b`.
#[inline]
fn point_to_segment_dist(p: Point, a: Point, b: Point) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    if l2 <= 1e-24 {
        return p.dist(a);
    }
    let t = ((p - a).dot(d) / l2).clamp(0.0, 1.0);
    p.dist(Point::new(a.x + d.x * t, a.y + d.y * t))
}

/// Mean squared distance from `samples` to polyline `ref_pts`.
///
/// px², with the distance of [`max_deviation_to_samples`]; infinite when either is empty.
pub fn mean_sq_deviation(samples: &[Point], ref_pts: &[Point]) -> f64 {
    if samples.is_empty() || ref_pts.is_empty() {
        return f64::INFINITY;
    }
    let mut scan = NearestScan::new(samples.len(), ref_pts);
    let mut sum_sq = 0.0;
    for (k, &p) in samples.iter().enumerate() {
        let d = scan.distance(k, p);
        sum_sq += d * d;
    }
    sum_sq / samples.len() as f64
}

/// Generate candidate replacements for `run`, filtered by geometric deviation and tangent continuity.
///
/// The run (starting at `start`) is resampled at 48 points of equal arc length
/// ([`sample_run_uniform`]), and four single-segment candidates with the same end
/// points are fitted to them: the chord, a cubic whose end directions are the outside
/// neighbours' (`before` is the previous segment with its start point, `after` the next
/// segment; the run's own end directions stand in where there is no neighbour), a free
/// cubic, and a circular arc. A candidate survives if its 48 samples and the run's are
/// within `cfg.max_dist` of each other both ways, and if at each end the kink it makes
/// with the neighbour is no worse than `max(existing kink, cfg.min_kink_rad) + 1e-6`.
/// Empty when the run is empty, degenerate, or closes on itself (ends under 1e-5 px
/// apart).
pub fn generate_replacements(
    start: Point,
    run: &[Segment],
    before: Option<(&Segment, Point)>,
    after: Option<&Segment>,
    cfg: &StructuralConfig,
) -> Vec<Segment> {
    let Some(last) = run.last() else {
        return Vec::new();
    };
    let count = 48;
    let Some((z, total_len)) = sample_run_uniform(start, run, count) else {
        return Vec::new();
    };

    let a = start;
    let b = last.end();
    if a.dist(b) < 1e-5 {
        return Vec::new();
    }

    let mut candidates: Vec<Segment> = Vec::with_capacity(4);

    // 1. Line candidate
    candidates.push(Segment::Line(b));

    // 2. C1-constrained cubic
    let v_opt = before
        .and_then(|(s, st)| segment_tangent_end(s, st))
        .or_else(|| segment_tangent_start(&run[0], a));
    let w_opt = after
        .and_then(|s| segment_tangent_start(s, b))
        .or_else(|| segment_tangent_end(last, run_prev_point(start, run)));

    if let (Some(v), Some(w)) = (v_opt, w_opt) {
        if let Some(cubic) = fit_c1_cubic(&z, a, b, v, w, total_len) {
            candidates.push(cubic);
        }
    }

    // 3. Free cubic
    if let Some(cubic) = fit_free_cubic(&z, a, b) {
        candidates.push(cubic);
    }

    // 4. Circular arc
    if let Some(arc) = fit_circular_arc(&z, a, b) {
        candidates.push(arc);
    }

    // Geometric pre-filter and external tangent guard
    let mut filtered = Vec::with_capacity(candidates.len());
    let ref_before_tan = before.and_then(|(s, st)| segment_tangent_end(s, st));
    let ref_run_start_tan = segment_tangent_start(&run[0], a);
    let ref_run_end_tan = segment_tangent_end(last, run_prev_point(start, run));
    let ref_after_tan = after.and_then(|s| segment_tangent_start(s, b));

    for cand in candidates {
        // Sample candidate curve at same sample count
        let mut cand_pts = Vec::with_capacity(count);
        for i in 0..count {
            let t = i as f64 / (count - 1) as f64;
            cand_pts.push(eval_segment(&cand, a, t));
        }

        // Geometric pre-filter with early exit
        if !is_deviation_within(&cand_pts, &z, cfg.max_dist)
            || !is_deviation_within(&z, &cand_pts, cfg.max_dist)
        {
            continue;
        }

        // Tangent continuity guards
        let cand_tan_start = segment_tangent_start(&cand, a);
        let cand_tan_end = segment_tangent_end(&cand, a);

        if let (Some(b_tan), Some(c_tan), Some(r_tan)) =
            (ref_before_tan, cand_tan_start, ref_run_start_tan)
        {
            let existing_kink = vector_angle(b_tan, r_tan);
            let new_kink = vector_angle(b_tan, c_tan);
            if new_kink > existing_kink.max(cfg.min_kink_rad) + 1e-6 {
                continue;
            }
        }

        if let (Some(a_tan), Some(c_tan), Some(r_tan)) =
            (ref_after_tan, cand_tan_end, ref_run_end_tan)
        {
            let existing_kink = vector_angle(r_tan, a_tan);
            let new_kink = vector_angle(c_tan, a_tan);
            if new_kink > existing_kink.max(cfg.min_kink_rad) + 1e-6 {
                continue;
            }
        }

        filtered.push(cand);
    }

    filtered
}

/// Helper to get start point of the last segment in a run.
fn run_prev_point(start: Point, run: &[Segment]) -> Point {
    if run.len() <= 1 {
        start
    } else {
        run[run.len() - 2].end()
    }
}

/// Fit C1-constrained cubic using 2x2 non-negative least squares for handle lengths.
///
/// Despite the name the constraint is G1: the end directions are fixed to the unit
/// vectors `v` (leaving `a`) and `w` (arriving at `b`) and only the arm lengths `x0`,
/// `x1` (px) are fitted. Sample `k` of `z` is given the parameter `t_k = k/(n−1)`, a
/// chord-length parametrisation since `z` is evenly spaced along the run, and
///
/// ```text
///     B(t) = (b0 + b1)·a + (b2 + b3)·b + b1·x0·v − b2·x1·w
/// ```
///
/// is linear in `(x0, x1)`, so `Σ|z_k − B(t_k)|²` is a convex quadratic with normal
/// matrix `[[Σb1², −(v·w)·Σb1b2], [−(v·w)·Σb1b2, Σb2²]]`. It is minimised over the box
/// `0 ≤ x ≤ 2·total_len`: the unconstrained minimiser if it lies inside, otherwise the
/// best of the clamped one-dimensional minimisers along the four edges (and the
/// origin), which for a convex quadratic is the box minimum. `None` for fewer than two
/// samples or a cubic that crosses itself.
fn fit_c1_cubic(
    z: &[Point],
    a: Point,
    b: Point,
    v: Vec2,
    w: Vec2,
    total_len: f64,
) -> Option<Segment> {
    let n = z.len();
    if n < 2 {
        return None;
    }

    let mut a00 = 0.0;
    let mut a11 = 0.0;
    let mut a01_scalar = 0.0;
    let mut y0 = 0.0;
    let mut y1 = 0.0;

    for (k, &p) in z.iter().enumerate() {
        let t = k as f64 / (n - 1) as f64;
        let u = 1.0 - t;
        let c1 = 3.0 * u * u * t;
        let c2 = 3.0 * u * t * t;
        let base_x = (u * u * u + c1) * a.x + (t * t * t + c2) * b.x;
        let base_y = (u * u * u + c1) * a.y + (t * t * t + c2) * b.y;
        let rhs_x = p.x - base_x;
        let rhs_y = p.y - base_y;

        a00 += c1 * c1;
        a11 += c2 * c2;
        a01_scalar += c1 * c2;

        let col0_x = c1 * v.x;
        let col0_y = c1 * v.y;
        let col1_x = -c2 * w.x;
        let col1_y = -c2 * w.y;

        y0 += col0_x * rhs_x + col0_y * rhs_y;
        y1 += col1_x * rhs_x + col1_y * rhs_y;
    }

    let a01 = -(v.dot(w)) * a01_scalar;
    let max_len = 2.0 * total_len;

    // Solve 2x2 NNLS in bounds [0, max_len]
    let det = a00 * a11 - a01 * a01;
    let mut best_x0 = 0.0;
    let mut best_x1 = 0.0;
    let mut solved = false;

    if det.abs() > 1e-12 {
        let x0 = (y0 * a11 - y1 * a01) / det;
        let x1 = (a00 * y1 - a01 * y0) / det;
        if (0.0..=max_len).contains(&x0) && (0.0..=max_len).contains(&x1) {
            best_x0 = x0;
            best_x1 = x1;
            solved = true;
        }
    }

    if !solved {
        // Evaluate 4 boundary edges
        let quad_cost = |x: f64, y: f64| -> f64 {
            0.5 * (a00 * x * x + 2.0 * a01 * x * y + a11 * y * y) - (y0 * x + y1 * y)
        };

        let mut best_cost = f64::INFINITY;
        let candidates = [
            ((y0 / a00).clamp(0.0, max_len), 0.0),
            (0.0, (y1 / a11).clamp(0.0, max_len)),
            (((y0 - a01 * max_len) / a00).clamp(0.0, max_len), max_len),
            (max_len, ((y1 - a01 * max_len) / a11).clamp(0.0, max_len)),
            (0.0, 0.0),
        ];

        for (x, y) in candidates {
            let cost = quad_cost(x, y);
            if cost < best_cost {
                best_cost = cost;
                best_x0 = x;
                best_x1 = y;
            }
        }
    }

    let cp1 = Point::new(a.x + best_x0 * v.x, a.y + best_x0 * v.y);
    let cp2 = Point::new(b.x - best_x1 * w.x, b.y - best_x1 * w.y);

    if cubic_self_intersects(a, cp1, cp2, b) {
        return None;
    }
    Some(Segment::Cubic(cp1, cp2, b))
}

/// Fit free cubic through endpoints using linear least squares on Bernstein basis.
///
/// Ends pinned to `a` and `b`, parameters `t_k = k/(n−1)` as in `fit_c1_cubic`, and the
/// two control points solved from the 2x2 normal equations shared by x and y (Cramer's
/// rule), unweighted. `None` for fewer than two samples, a singular system or a cubic
/// that crosses itself.
fn fit_free_cubic(z: &[Point], a: Point, b: Point) -> Option<Segment> {
    let n = z.len();
    if n < 2 {
        return None;
    }

    let mut n00 = 0.0;
    let mut n11 = 0.0;
    let mut n01 = 0.0;
    let mut rx0 = 0.0;
    let mut rx1 = 0.0;
    let mut ry0 = 0.0;
    let mut ry1 = 0.0;

    for (k, &p) in z.iter().enumerate() {
        let t = k as f64 / (n - 1) as f64;
        let u = 1.0 - t;
        let c1 = 3.0 * u * u * t;
        let c2 = 3.0 * u * t * t;
        let rhs_x = p.x - u * u * u * a.x - t * t * t * b.x;
        let rhs_y = p.y - u * u * u * a.y - t * t * t * b.y;

        n00 += c1 * c1;
        n11 += c2 * c2;
        n01 += c1 * c2;

        rx0 += c1 * rhs_x;
        rx1 += c2 * rhs_x;
        ry0 += c1 * rhs_y;
        ry1 += c2 * rhs_y;
    }

    let det = n00 * n11 - n01 * n01;
    if det.abs() <= 1e-12 {
        return None;
    }

    let p1x = (rx0 * n11 - rx1 * n01) / det;
    let p2x = (n00 * rx1 - n01 * rx0) / det;
    let p1y = (ry0 * n11 - ry1 * n01) / det;
    let p2y = (n00 * ry1 - n01 * ry0) / det;

    let cp1 = Point::new(p1x, p1y);
    let cp2 = Point::new(p2x, p2y);

    if cubic_self_intersects(a, cp1, cp2, b) {
        return None;
    }
    Some(Segment::Cubic(cp1, cp2, b))
}

/// Fit circular arc through endpoints minimizing radial error via golden section search.
///
/// Every circle through `a` and `b` has its centre on their perpendicular bisector,
/// `c(h) = mid + h·n`, so the fit is one-dimensional: minimise the mean squared radial
/// error `mean_k (|z_k − c(h)| − |a − c(h)|)²` over `h ∈ [−10, 10]·chord` by golden-section
/// search (38 steps shrink the bracket by about 1e-8). The sweep is the unwrapped change
/// of the samples' angle about the centre; arcs sweeping under 0.02 rad (a line's job) or
/// over 1.9π are refused, as is a chord under 1e-6 px. Assumes the loss is unimodal in
/// `h`, which holds for samples that really lie near one arc.
fn fit_circular_arc(z: &[Point], a: Point, b: Point) -> Option<Segment> {
    let chord = a.dist(b);
    if chord <= 1e-6 {
        return None;
    }
    let mid = Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
    let normal = Vec2 {
        x: -(b.y - a.y) / chord,
        y: (b.x - a.x) / chord,
    };

    let loss = |h: f64| -> f64 {
        let cx = mid.x + h * normal.x;
        let cy = mid.y + h * normal.y;
        let r = ((a.x - cx) * (a.x - cx) + (a.y - cy) * (a.y - cy)).sqrt();
        let mut sum_sq = 0.0;
        for &p in z {
            let d = ((p.x - cx) * (p.x - cx) + (p.y - cy) * (p.y - cy)).sqrt();
            let err = d - r;
            sum_sq += err * err;
        }
        sum_sq / z.len() as f64
    };

    // Golden section search in [-10 * chord, 10 * chord]
    let invphi = (5.0_f64.sqrt() - 1.0) * 0.5;
    let invphi2 = (3.0 - 5.0_f64.sqrt()) * 0.5;
    let mut lo = -10.0 * chord;
    let mut span = 20.0 * chord;
    let mut c = lo + invphi2 * span;
    let mut d = lo + invphi * span;
    let mut yc = loss(c);
    let mut yd = loss(d);

    for _ in 0..38 {
        if yc < yd {
            d = c;
            yd = yc;
            span *= invphi;
            c = lo + invphi2 * span;
            yc = loss(c);
        } else {
            lo = c;
            c = d;
            yc = yd;
            span *= invphi;
            d = lo + invphi * span;
            yd = loss(d);
        }
    }

    let best_h = if yc < yd { c } else { d };
    let center = Point::new(mid.x + best_h * normal.x, mid.y + best_h * normal.y);
    let radius = a.dist(center);

    // Unwrap angles along polyline samples
    let mut prev_ang = (z[0].y - center.y).atan2(z[0].x - center.x);
    let mut cum_sweep = 0.0;
    for &p in &z[1..] {
        let ang = (p.y - center.y).atan2(p.x - center.x);
        let mut diff = ang - prev_ang;
        while diff > std::f64::consts::PI {
            diff -= std::f64::consts::TAU;
        }
        while diff < -std::f64::consts::PI {
            diff += std::f64::consts::TAU;
        }
        cum_sweep += diff;
        prev_ang = ang;
    }

    let sweep_val = cum_sweep;
    let abs_sweep = sweep_val.abs();
    if abs_sweep < 0.02 || abs_sweep > 1.9 * std::f64::consts::PI {
        return None;
    }

    let large_arc = abs_sweep > std::f64::consts::PI;
    let sweep_flag = sweep_val > 0.0;

    Some(Segment::circular_arc(radius, large_arc, sweep_flag, b))
}

/// A proposed replacement candidate for a run of segments.
#[derive(Debug, Clone)]
pub struct Proposal {
    /// Start segment index in path.
    pub start: usize,
    /// End segment index (exclusive) in path.
    pub end: usize,
    /// Replacement segment.
    pub repl: Segment,
    /// Parameters saved: `run_params - repl_params`.
    pub saving: f64,
    /// Rate-distortion gain: `-delta_distortion + lambda * saving`.
    pub gain: f64,
}

/// Each proposal was checked against unchanged outside tangents. Adjacent
/// replacements share that evidence and must be deferred to a later round.
/// The first and last runs are adjacent too when the path is closed.
///
/// The runs are the segment ranges `[s, e)` and `[bs, be)` of a path of `n` segments.
/// They are independent when at least one untouched segment separates them (`e < bs`
/// or `s > be`) and, on a closed path, they do not meet across the seam.
fn independent_runs(s: usize, e: usize, bs: usize, be: usize, n: usize, closed: bool) -> bool {
    (e < bs || s > be) && !(closed && ((s == 0 && be == n) || (bs == 0 && e == n)))
}

/// Simplify a [`FittedPath`] using the structural MDL rate-distortion algorithm.
///
/// Runs multiple greedy rounds of candidate generation and non-overlapping batch
/// replacement. Returns the total number of segments eliminated.
///
/// Geometry only: each run of `cfg.run_lengths` segments is replaced by a candidate from
/// [`generate_replacements`] when that saves parameters and
///
/// ```text
///     gain = −Δdistortion + λ·(params_old − params_new) > 1e-9
/// ```
///
/// with distortion the mean squared distance (px²) of 64 candidate samples from 64
/// samples of the run, and `λ` = `cfg.lambda`. Each round takes the proposals in
/// descending gain, keeps those independent of every one already kept, and applies them
/// back to front; rounds stop when nothing is proposed or after `cfg.max_rounds`. A path
/// whose end meets its start is treated as closed.
pub fn simplify_path_structural(path: &mut FittedPath, cfg: &StructuralConfig) -> usize {
    if path.segments.len() < 2 {
        return 0;
    }

    let closed = path.closed || path.start.dist(path.end()) <= 1e-9;
    let mut eliminated = 0usize;

    for _round in 0..cfg.max_rounds {
        let n = path.segments.len();
        if n < 2 {
            break;
        }

        let mut proposals = structural_proposals(path, cfg, closed);
        if proposals.is_empty() {
            break;
        }

        // Sort proposals descending by gain
        proposals.sort_by(|a, b| b.gain.total_cmp(&a.gain));

        // Greedy disjoint interval packing
        let mut batch = independent_batch(proposals, n, closed, |p| (p.start, p.end));

        if batch.is_empty() {
            break;
        }

        // Apply batch back-to-front by start index so earlier indices remain valid
        batch.sort_by_key(|proposal| std::cmp::Reverse(proposal.start));

        let before_count = path.segments.len();
        for p in batch {
            path.segments.splice(p.start..p.end, [p.repl]);
        }
        let after_count = path.segments.len();
        eliminated += before_count.saturating_sub(after_count);
    }

    eliminated
}

/// Every proposal of one round of [`simplify_path_structural`], in scan order.
fn structural_proposals(path: &FittedPath, cfg: &StructuralConfig, closed: bool) -> Vec<Proposal> {
    let n = path.segments.len();
    let starts = segment_starts(path);
    let mut proposals: Vec<Proposal> = Vec::new();

    for start_idx in 0..n {
        for &run_len in &cfg.run_lengths {
            let end_idx = start_idx + run_len;
            if end_idx > n {
                continue;
            }

            let run = &path.segments[start_idx..end_idx];
            let run_start = starts[start_idx];
            let (before, after) = run_neighbours(path, &starts, (start_idx, end_idx), closed);

            let cand_list = generate_replacements(run_start, run, before, after, cfg);
            if cand_list.is_empty() {
                continue;
            }

            // Sample run ground truth
            let Some((ref_samples, _)) = sample_run_uniform(run_start, run, 64) else {
                continue;
            };

            let old_rate: f64 = run.iter().map(|s| segment_rate(s, cfg)).sum();
            let old_dist = mean_sq_deviation(&ref_samples, &ref_samples); // 0.0

            for cand in cand_list {
                let repl_rate = segment_rate(&cand, cfg);
                let saving = old_rate - repl_rate;
                if saving <= 0.0 {
                    continue;
                }

                // Sample candidate
                let mut cand_samples = Vec::with_capacity(64);
                for k in 0..64 {
                    let t = k as f64 / 63.0;
                    cand_samples.push(eval_segment(&cand, run_start, t));
                }

                let new_dist = mean_sq_deviation(&cand_samples, &ref_samples);
                let delta_dist = new_dist - old_dist;
                let gain = -delta_dist + cfg.lambda * saving;

                if gain > 1e-9 {
                    proposals.push(Proposal {
                        start: start_idx,
                        end: end_idx,
                        repl: cand,
                        saving,
                        gain,
                    });
                }
            }
        }
    }
    proposals
}

/// Where each segment of `path` starts: `path.start`, then every segment's end but the
/// last.
fn segment_starts(path: &FittedPath) -> Vec<Point> {
    let mut starts = Vec::with_capacity(path.segments.len());
    let mut cur = path.start;
    for s in &path.segments {
        starts.push(cur);
        cur = s.end();
    }
    starts
}

/// The segments outside the run `[start_idx, end_idx)`: the one before it with its start
/// point, and the one after it. On a closed path the seam wraps; on an open one the ends
/// have none.
fn run_neighbours<'p>(
    path: &'p FittedPath,
    starts: &[Point],
    (start_idx, end_idx): (usize, usize),
    closed: bool,
) -> (Option<(&'p Segment, Point)>, Option<&'p Segment>) {
    let n = path.segments.len();
    let before = if start_idx > 0 {
        Some((&path.segments[start_idx - 1], starts[start_idx - 1]))
    } else if closed && n > 1 {
        Some((&path.segments[n - 1], starts[n - 1]))
    } else {
        None
    };

    let after = if end_idx < n {
        Some(&path.segments[end_idx])
    } else if closed && n > 1 {
        Some(&path.segments[0])
    } else {
        None
    };
    (before, after)
}

/// Greedy disjoint packing: walk `proposals` in the order given (best first) and keep each
/// whose run, `span(proposal)`, is independent of every run already kept
/// ([`independent_runs`]).
fn independent_batch<T>(
    proposals: Vec<T>,
    n: usize,
    closed: bool,
    span: impl Fn(&T) -> (usize, usize),
) -> Vec<T> {
    let mut batch: Vec<T> = Vec::new();
    for prop in proposals {
        let (s, e) = span(&prop);
        let overlaps = batch.iter().any(|b| {
            let (bs, be) = span(b);
            !independent_runs(s, e, bs, be, n, closed)
        });
        if !overlaps {
            batch.push(prop);
        }
    }
    batch
}

/// Simplify a [`FittedPath`] using the exact contour points and chi2 weights from tracing.
/// Missing contour correspondence is not permission to substitute a different loss.
///
/// `poly` is the measured polyline the path was fitted to and `vertices` the index in it
/// of every segment end (one more than there are segments). A run is replaced by a
/// candidate from [`generate_replacements`] when the candidate has fewer parameters and
///
/// ```text
///     ½·χ²_new + λ·params_new < ½·χ²_old + λ·params_old − 1e-9
/// ```
///
/// with `χ²` sampled against the run's measured points (`crate::curves::chi2`) and `λ`
/// from `cfg`. A run whose vertex indices wrap across the seam of a closed contour is
/// skipped rather than scored some other way. Batching is as in
/// [`simplify_path_structural`], and `vertices` is kept aligned. Returns the number of
/// segments eliminated; 0 without changing anything when the correspondence is
/// inconsistent.
pub fn simplify_with_poly(
    path: &mut FittedPath,
    poly: &Polyline,
    vertices: &[usize],
    cfg: &FitConfig,
) -> usize {
    if path.segments.len() < 2 {
        return 0;
    }
    let struct_cfg = StructuralConfig::from_fit_config(cfg);
    if vertices.len() != path.segments.len() + 1
        || poly.sigma.len() != poly.points.len()
        || vertices.iter().any(|&v| v >= poly.points.len())
    {
        return 0;
    }
    let closed = path.closed || path.start.dist(path.end()) <= 1e-9;
    let mut verts = vertices.to_vec();
    let mut eliminated = 0usize;

    for _round in 0..struct_cfg.max_rounds {
        let n = path.segments.len();
        if n < 2 {
            break;
        }

        let mut proposals = poly_proposals(path, poly, &verts, cfg, &struct_cfg, closed);
        if proposals.is_empty() {
            break;
        }

        proposals.sort_by(|a, b| b.3.total_cmp(&a.3));

        let mut batch = independent_batch(proposals, n, closed, |p| (p.0, p.1));

        if batch.is_empty() {
            break;
        }

        batch.sort_by_key(|proposal| std::cmp::Reverse(proposal.0));
        let before_count = path.segments.len();
        for (s, e, repl, _) in batch {
            path.segments.splice(s..e, [repl]);
            verts.drain(s + 1..e);
        }
        let after_count = path.segments.len();
        eliminated += before_count.saturating_sub(after_count);
    }

    eliminated
}

/// Every proposal of one round of [`simplify_with_poly`], in scan order, as
/// `(start, end, replacement, gain)` with the gain in nats.
fn poly_proposals(
    path: &FittedPath,
    poly: &Polyline,
    verts: &[usize],
    cfg: &FitConfig,
    struct_cfg: &StructuralConfig,
    closed: bool,
) -> Vec<(usize, usize, Segment, f64)> {
    let n = path.segments.len();
    let starts = segment_starts(path);
    let mut proposals: Vec<(usize, usize, Segment, f64)> = Vec::new();

    for start_idx in 0..n {
        for &run_len in &struct_cfg.run_lengths {
            let end_idx = start_idx + run_len;
            if end_idx > n {
                continue;
            }

            let run = &path.segments[start_idx..end_idx];
            let run_start = starts[start_idx];
            let (before, after) = run_neighbours(path, &starts, (start_idx, end_idx), closed);

            // Vertex correspondence must be resolved before we can compute
            // sigma — do the bounds check here and skip early if invalid.
            let a_vert = verts[start_idx];
            let b_vert = verts[end_idx];
            // Closed contour indices wrap at the seam. That is valid
            // correspondence; only this non-contiguous span is unsupported.
            if b_vert <= a_vert || b_vert >= poly.points.len() {
                continue;
            }

            let pts_span = &poly.points[a_vert..=b_vert];
            let sig_span = &poly.sigma[a_vert..=b_vert];

            let cand_list = {
                // The fixed max_dist bound (default 0.85 px) prevents the simplified
                // curve from deviating too far from the *run's own* polyline.  But it
                // does not prevent the candidate from bulging toward an adjacent
                // boundary — and if it does, the ring-repair stage introduces extra
                // capped refits, inflating the final output.
                //
                // A tighter, signal-aware limit: the trace already tells us how
                // uncertain each boundary point is (sigma).  A highly-confident
                // boundary (small sigma) is precisely located and probably close to
                // a neighbouring edge.  Cap max_dist to sigma_cap * mean_sigma(span)
                // for such spans, so the structural pass is more conservative exactly
                // where crossing risk is highest.
                let mean_sig = if !sig_span.is_empty() {
                    sig_span.iter().sum::<f64>() / sig_span.len() as f64
                } else {
                    struct_cfg.max_dist
                };
                let run_max_dist = struct_cfg.max_dist.min(sigma_cap() * mean_sig);
                let run_cfg = StructuralConfig {
                    max_dist: run_max_dist,
                    ..struct_cfg.clone()
                };
                generate_replacements(run_start, run, before, after, &run_cfg)
            };

            if cand_list.is_empty() {
                continue;
            }

            let old_chi2 = crate::curves::chi2(pts_span, sig_span, run_start, run);
            let old_params: f64 = run.iter().map(|s| s.params()).sum();
            let old_cost = 0.5 * old_chi2 + cfg.lambda * old_params;

            for cand in cand_list {
                let new_chi2 =
                    crate::curves::chi2(pts_span, sig_span, run_start, std::slice::from_ref(&cand));
                let new_params = cand.params();
                if new_params >= old_params {
                    continue;
                }
                let new_cost = 0.5 * new_chi2 + cfg.lambda * new_params;

                if new_cost < old_cost - 1e-9 {
                    let gain = old_cost - new_cost;
                    proposals.push((start_idx, end_idx, cand, gain));
                }
            }
        }
    }
    proposals
}

#[cfg(test)]
mod batch_tests {
    use super::*;

    #[test]
    fn tangent_evidence_requires_an_unchanged_neighbor() {
        // Each side can deviate 0.03 rad from an old straight join, yet together
        // they would form a 0.06 rad kink. Non-overlap alone is insufficient.
        assert!(!independent_runs(0, 2, 2, 4, 8, false));
        assert!(!independent_runs(2, 4, 0, 2, 8, false));
        assert!(!independent_runs(0, 3, 2, 4, 8, false));
        assert!(independent_runs(0, 2, 3, 5, 8, false));
        assert!(!independent_runs(0, 2, 6, 8, 8, true));
        assert!(independent_runs(0, 2, 6, 8, 8, false));
    }

    #[test]
    fn arc_sampling_keeps_exact_stored_endpoints() {
        let start = Point::new(1.23456789, 2.34567891);
        let end = Point::new(9.87654321, 3.45678912);
        let arc = Segment::circular_arc(8.1234567, false, true, end);
        assert_eq!(eval_segment(&arc, start, 0.0), start);
        assert_eq!(eval_segment(&arc, start, 1.0), end);
    }
}
