//! Global optimal segmentation of a measured boundary (DESIGN.md S4).
//!
//! This is the stage VTracer dropped for speed, and `docs/M0-BASELINE.md` §5 measures
//! what dropping it costs: under smooth boundary perturbation, Potrace's parameter count
//! grows **0.0% (median)** where VTracer's grows **+18.0%**. The non-local optimization
//! is what buys noise robustness — a local, greedy tracer spends anchors on every wobble
//! the segmentation invents.
//!
//! Potrace's version works on *integer pixel corners*. Ours has to work on floating-point
//! sub-pixel positions with a per-point uncertainty, which changes both halves of the
//! algorithm:
//!
//! **The straightness test.** Potrace asks whether every point of a subpath lies within
//! L-infinity distance 1 of the chord — "1" being one pixel, the only length scale an
//! integer lattice offers. We ask whether every point is *statistically consistent* with
//! the chord: `|d_k| <= tau * sigma_k`. Where the boundary was well localized, sigma is
//! small and we demand tightness; where it was faint, sigma is large and we simplify
//! hard. Adaptive simplification is then a consequence of the measurement model rather
//! than a separate heuristic with its own knob.
//!
//! **The cost.** Potrace minimizes (segment count, penalty) lexicographically. We
//! minimize the MDL objective of DESIGN.md §5.0 directly — Gaussian negative
//! log-likelihood plus a description-length term — so the segment count falls out of
//! `lambda` instead of being a separate lexicographic stage. That generalizes to the
//! multi-model alphabet (arc, elliptical arc, cubic) without changing the DP.
//!
//! Both halves keep the O(1)-per-candidate-segment property that makes the DP O(n^2)
//! rather than O(n^3): fidelity by weighted prefix sums, and a scan cut off by a bound
//! tied to the cost function itself.
//!
//! This header described the cut-off as "straightness by incremental cone intersection"
//! until 2026-09-08, and that has not been how the shipping program works for some time.
//! The cone was removed as a correctness bug -- on a 49 px straight edge it excluded the
//! whole-edge segment, so the program returned the best of what was left and split
//! straight edges at collinear points (see the comment in `optimal_polygon`).
//! `DirectionCone` and [`is_admissible`] survive as a reference implementation used by
//! the tests and by `examples/lambda_sweep.rs`; nothing in the shipping path calls them.

mod decimate;

pub(crate) mod candidates;
pub mod cost;
pub mod curves;
pub mod harmonize;
pub mod merge;
pub mod multimodel;
pub mod pareto;
pub mod primitives;
pub mod simple;
pub mod structural;
pub(crate) mod tangents;

use inkvec_core::{Point, Polyline, Vec2};

/// Parameters a line segment adds to the document: its endpoint (x, y). The start point
/// is shared with the previous segment, so it is not charged twice.
pub const PARAMS_LINE: f64 = 2.0;

/// Safety factor on the search cut-off. The cut-off below is a provable bound, so this
/// only guards against the residual being non-monotonic in the span.
///
/// Defined once, here, and used by `multimodel` through this path: the two modules ran
/// identical private copies until 2026-09-08, which is two numbers that must agree with
/// nothing keeping them in agreement.
pub(crate) const PRUNE_SLACK: f64 = 4.0;

/// Parameters of the MDL objective `0.5·chi² + λ·params` that every fitter in this crate
/// is scored by.
#[derive(Debug, Clone, Copy)]
pub struct FitConfig {
    /// Confidence multiplier on the per-point sigma. `tau = 2.0` admits a chord that
    /// stays within ~2 standard deviations of every measurement.
    pub tau: f64,
    /// Cost charged per emitted parameter, in nats — the same units as the
    /// log-likelihood. This is the exchange rate between fidelity and description
    /// length: the `lambda` of DESIGN.md §5.0, and per §9.6 the constant most likely to
    /// be mis-set in a way that looks like a pipeline defect.
    ///
    /// Prefer [`FitConfig::from_precision`] over setting this by hand. It has a
    /// derivation, and picking it by taste produces exactly the confusion documented in
    /// `examples/lambda_sweep.rs`.
    pub lambda: f64,
}

impl FitConfig {
    /// Derive `lambda` from what a coordinate actually costs to write down.
    ///
    /// MDL measures description length in nats. A coordinate confined to a range
    /// `extent` and stored to resolution `precision` carries `ln(extent / precision)`
    /// nats of information. For a 256px canvas written at 0.1px precision that is
    /// `ln(2560) ~ 7.85` — not the 1.0 a first guess suggests, and the difference is
    /// roughly a factor of two in emitted segment count.
    ///
    /// This ties the fidelity/compactness trade to two quantities that are known rather
    /// than tuned: how big the image is, and how precisely we intend to write numbers.
    pub fn from_precision(extent: f64, precision: f64, tau: f64) -> Self {
        let ratio = (extent / precision.max(f64::MIN_POSITIVE)).max(std::f64::consts::E);
        Self {
            tau,
            lambda: ratio.ln(),
        }
    }
}

impl Default for FitConfig {
    /// A 256px canvas at 0.1px output precision, admitting chords within 2 sigma.
    fn default() -> Self {
        Self::from_precision(256.0, 0.1, 2.0)
    }
}

/// Weighted prefix sums that make the fit residual of any candidate segment O(1).
///
/// The residual of segment `(i, j)` is a quadratic form in the chord direction whose
/// coefficients are sums over the interior points of `x`, `y`, `x^2`, `y^2`, `xy` and
/// `1`, each weighted by `1 / sigma^2`. Because the chord's *origin* `v_i` also varies,
/// the sums are expanded about the origin so that every term is a plain prefix sum.
/// This is Potrace's constant-time penalty trick, generalized to per-point weights.
struct PrefixSums {
    w: Vec<f64>,
    x: Vec<f64>,
    y: Vec<f64>,
    xx: Vec<f64>,
    yy: Vec<f64>,
    xy: Vec<f64>,
}

impl PrefixSums {
    fn new(poly: &Polyline) -> Self {
        let n = poly.len();
        let mut s = PrefixSums {
            w: vec![0.0; n + 1],
            x: vec![0.0; n + 1],
            y: vec![0.0; n + 1],
            xx: vec![0.0; n + 1],
            yy: vec![0.0; n + 1],
            xy: vec![0.0; n + 1],
        };
        for k in 0..n {
            let p = poly.points[k];
            let iv = 1.0 / (poly.sigma[k] * poly.sigma[k]);
            s.w[k + 1] = s.w[k] + iv;
            s.x[k + 1] = s.x[k] + p.x * iv;
            s.y[k + 1] = s.y[k] + p.y * iv;
            s.xx[k + 1] = s.xx[k] + p.x * p.x * iv;
            s.yy[k + 1] = s.yy[k] + p.y * p.y * iv;
            s.xy[k + 1] = s.xy[k] + p.x * p.y * iv;
        }
        s
    }

    /// Weighted sum of squared **orthogonal** distances from the points `[i, j]` to the
    /// best-fit line through them.
    ///
    /// Not the distance to the chord `v_i -> v_j`. That distinction turned out to matter
    /// a great deal in practice. Sub-pixel boundary extraction gives every point its own
    /// error, endpoints included; a chord is pinned to two of those errors, so over a
    /// 40px span two endpoint errors of opposite sign tilt the chord enough to push the
    /// interior points outside a tolerance they individually satisfy. Measured effect: a
    /// rotated rectangle with four true edges was being segmented into 29.
    ///
    /// The total-least-squares residual is the smaller eigenvalue of the weighted scatter
    /// matrix, which is a closed form in exactly the prefix sums already maintained — so
    /// the principled fit is also the O(1) one. (DESIGN.md S4 specifies orthogonal
    /// distance fitting; algebraic fitting carries a high-curvature bias.)
    fn chi2_line(&self, i: usize, j: usize) -> f64 {
        if j <= i {
            return 0.0;
        }
        let (a, b) = (i, j + 1); // inclusive range [i, j]
        let w = self.w[b] - self.w[a];
        if w <= 0.0 {
            return 0.0;
        }
        let sx = self.x[b] - self.x[a];
        let sy = self.y[b] - self.y[a];
        let sxx = self.xx[b] - self.xx[a];
        let syy = self.yy[b] - self.yy[a];
        let sxy = self.xy[b] - self.xy[a];

        // Scatter about the weighted centroid.
        let cxx = sxx - sx * sx / w;
        let cyy = syy - sy * sy / w;
        let cxy = sxy - sx * sy / w;

        let tr = cxx + cyy;
        let diff = cxx - cyy;
        let disc = (diff * diff + 4.0 * cxy * cxy).max(0.0).sqrt();
        (0.5 * (tr - disc)).max(0.0)
    }
}

/// Bring `angle` within +/- pi of `reference`, so an interval can be intersected
/// without wrap-around bookkeeping.
#[inline]
fn wrap_near(angle: f64, reference: f64) -> f64 {
    let two_pi = std::f64::consts::TAU;
    let mut a = angle;
    while a - reference > std::f64::consts::PI {
        a -= two_pi;
    }
    while reference - a > std::f64::consts::PI {
        a += two_pi;
    }
    a
}

/// Incrementally maintained set of chord directions consistent with every interior
/// point seen so far.
///
/// A point `p_k` at offset `w_k` from the segment start constrains the chord direction
/// `u` by `|cross(w_k, u)| <= tau * sigma_k`. Writing `u` as an angle, that is an
/// interval of half-width `asin(tau * sigma_k / |w_k|)` centred on `w_k`'s own angle.
/// Admissible directions are the intersection of those intervals, which only ever
/// shrinks — so once it is empty, no longer segment from this start can be straight and
/// the scan stops. Each update is O(1).
///
/// The `asin` also encodes something useful for free: a point closer to the start than
/// its own uncertainty constrains nothing, which is exactly right — it carries no
/// information about direction.
struct DirectionCone {
    lo: f64,
    hi: f64,
    initialized: bool,
    /// Largest distance from the segment start to any interior point seen so far.
    max_reach: f64,
}

impl DirectionCone {
    fn new() -> Self {
        Self {
            lo: 0.0,
            hi: 0.0,
            initialized: false,
            max_reach: 0.0,
        }
    }

    fn is_empty(&self) -> bool {
        self.initialized && self.lo > self.hi
    }

    /// Does direction `phi` (with chord length `reach`) satisfy every constraint?
    fn admits(&self, phi: f64, reach: f64) -> bool {
        if !self.initialized {
            return true;
        }
        if self.lo > self.hi {
            return false;
        }
        // The segment must span its interior points: if some interior point lies
        // farther from the start than the endpoint does, the boundary has run past
        // `j` and come back, and `j` is not a sensible place to end a segment.
        if reach + f64::EPSILON < self.max_reach {
            return false;
        }
        let p = wrap_near(phi, 0.5 * (self.lo + self.hi));
        p >= self.lo && p <= self.hi
    }

    /// Fold in the constraint imposed by an interior point.
    fn constrain(&mut self, w: Vec2, tol: f64) {
        let reach = w.norm();
        if reach > self.max_reach {
            self.max_reach = reach;
        }
        if reach <= tol {
            return; // uninformative: the point is inside its own uncertainty ball
        }
        let half = (tol / reach).clamp(-1.0, 1.0).asin();
        let phi = w.angle();
        if !self.initialized {
            self.lo = phi - half;
            self.hi = phi + half;
            self.initialized = true;
            return;
        }
        let c = wrap_near(phi, 0.5 * (self.lo + self.hi));
        self.lo = self.lo.max(c - half);
        self.hi = self.hi.min(c + half);
    }
}

/// A chosen segmentation of the input polyline.
#[derive(Debug, Clone)]
pub struct Segmentation {
    /// Indices into the source polyline that were kept as vertices.
    pub vertices: Vec<usize>,
    /// Total MDL cost of this segmentation.
    pub cost: f64,
}

impl Segmentation {
    /// Number of segments between the kept vertices.
    pub fn segment_count(&self) -> usize {
        self.vertices.len().saturating_sub(1)
    }

    /// The kept vertices' positions, looked up in `poly`.
    pub fn points(&self, poly: &Polyline) -> Vec<Point> {
        self.vertices.iter().map(|&i| poly.points[i]).collect()
    }
}

/// Globally optimal polygonal segmentation under the MDL objective.
///
/// Exact: the dynamic program considers every admissible segmentation, so the result is
/// the global minimum of `0.5 * chi^2 + lambda * params` over all of them. There is no
/// greedy step and no local corner decision to get wrong.
pub fn optimal_polygon(poly: &Polyline, cfg: &FitConfig) -> Segmentation {
    let n = poly.len();
    if n < 2 {
        return Segmentation {
            vertices: (0..n).collect(),
            cost: 0.0,
        };
    }
    if poly.closed {
        return optimal_polygon_closed(poly, cfg);
    }

    let sums = PrefixSums::new(poly);
    let mut best = vec![f64::INFINITY; n];
    let mut from = vec![usize::MAX; n];
    best[0] = 0.0;

    for i in 0..n - 1 {
        if !best[i].is_finite() {
            continue;
        }

        for j in i + 1..n {
            let chi2 = sums.chi2_line(i, j);
            let c = best[i] + 0.5 * chi2 + cfg.lambda * PARAMS_LINE;
            if c < best[j] {
                best[j] = c;
                from[j] = i;
            }

            // Search cut-off, derived from the objective rather than from geometry.
            //
            // Covering the span `i..j` with the finest possible segmentation costs at
            // least `lambda * PARAMS_LINE * (j - i)`. So once a single segment's fidelity
            // term alone exceeds that, no longer segment from `i` can ever win, and the
            // scan can stop.
            //
            // An earlier version pruned with an angular cone on the chord direction
            // instead. That was a correctness bug wearing an optimization's clothes: on a
            // 49px straight edge the cone excluded the whole-edge segment, and the
            // dynamic program dutifully returned the best of the *remaining* options —
            // splitting straight edges at collinear points and returning 12 segments for
            // a hexagon. A bound tied to the cost function cannot fail that way, because
            // it discards only segments the cost function would have rejected anyway.
            let floor = cfg.lambda * PARAMS_LINE * (j - i) as f64;
            if 0.5 * chi2 > PRUNE_SLACK * floor {
                break;
            }
        }
    }

    let mut vertices = Vec::new();
    let mut cur = n - 1;
    // `from[0]` is never set; walking back from the last index terminates at 0.
    while cur != usize::MAX {
        vertices.push(cur);
        if cur == 0 {
            break;
        }
        cur = from[cur];
    }
    vertices.reverse();
    Segmentation {
        vertices,
        cost: best[n - 1],
    }
}

/// Closed loops with no forced junction vertex.
///
/// Most boundaries in a planar map are *open* arcs between junctions, so this case only
/// arises for an isolated loop — a lone circle, a counter. We cut at the point farthest
/// from the centroid, which is stable under rotation and under resampling, then solve the
/// open problem.
///
/// This is an approximation: the true optimum is a minimum-cost *cycle*, and fixing a cut
/// point can cost one extra segment. Potrace solves the cyclic problem properly and we
/// should too — tracked as future work rather than hidden here.
fn optimal_polygon_closed(poly: &Polyline, cfg: &FitConfig) -> Segmentation {
    let n = poly.len();
    let cx = poly.points.iter().map(|p| p.x).sum::<f64>() / n as f64;
    let cy = poly.points.iter().map(|p| p.y).sum::<f64>() / n as f64;
    let c = Point::new(cx, cy);
    let cut = (0..n)
        .max_by(|&a, &b| {
            poly.points[a]
                .dist(c)
                .partial_cmp(&poly.points[b].dist(c))
                .unwrap()
        })
        .unwrap_or(0);

    let mut points = Vec::with_capacity(n + 1);
    let mut sigma = Vec::with_capacity(n + 1);
    for k in 0..=n {
        let idx = (cut + k) % n;
        points.push(poly.points[idx]);
        sigma.push(poly.sigma[idx]);
    }
    let opened = Polyline::new(points, sigma, false);
    let seg = optimal_polygon(&opened, cfg);
    Segmentation {
        vertices: seg.vertices.iter().map(|&i| (cut + i) % n).collect(),
        cost: seg.cost,
    }
}

/// Is the straight segment from `i` to `j` consistent with every point between them?
///
/// Recomputes the cone from scratch, so this is O(j - i) rather than the O(1) amortized
/// update the dynamic program uses. Exposed because an independent, obviously-correct
/// implementation of the admissibility rule is what lets the tests verify the fast path.
pub fn is_admissible(poly: &Polyline, i: usize, j: usize, cfg: &FitConfig) -> bool {
    if j <= i || j >= poly.len() {
        return false;
    }
    let origin = poly.points[i];
    let mut cone = DirectionCone::new();
    for k in i + 1..j {
        cone.constrain(poly.points[k] - origin, cfg.tau * poly.sigma[k]);
        if cone.is_empty() {
            return false;
        }
    }
    let w = poly.points[j] - origin;
    cone.admits(w.angle(), w.norm())
}

/// MDL cost of a single candidate segment: `0.5 * chi^2 + lambda * params`.
pub fn segment_cost(poly: &Polyline, i: usize, j: usize, cfg: &FitConfig) -> f64 {
    let sums = PrefixSums::new(poly);
    0.5 * sums.chi2_line(i, j) + cfg.lambda * PARAMS_LINE
}

/// Move each vertex to the intersection of its two adjacent best-fit lines.
///
/// Marching squares cannot represent a sharp corner: the level set cuts across the corner
/// pixel, chamfering it over a point or two. Trusting those measured points as vertices
/// both rounds the corner and costs an extra segment to cross the chamfer — measured
/// effect, a hexagon coming back with 12 vertices instead of 6.
///
/// The corner is not where the contour was sampled; it is where the two edges *meet*, so
/// intersecting their fitted lines recovers it. This is Potrace's vertex adjustment step,
/// with the constraint generalized from its fixed unit square to `max_shift`, which the
/// caller can scale by the measurement uncertainty. Near-parallel neighbours and
/// intersections that land implausibly far away fall back to the measured point.
/// Whether the vertex list `v` runs all the way round a closed contour, so that its first
/// and last vertex are one point and the join between the last and first segment is a
/// vertex like any other.
///
/// Two spellings reach here. A loop solved directly has the same index at both ends. A
/// loop the multimodel program *opened at a cut* carries the cut point twice — index 0
/// and index `n - 1` — and `v.first() == v.last()` is false for it although the points
/// coincide. That second spelling is every closed contour in the pipeline, and the cut
/// is placed at the sharpest corner; comparing indices alone therefore exempted exactly
/// the most corner-like vertex of every shape from corner adjustment and from smooth
/// joins. Measured on a rotated bar: the cut corner stayed 0.77 px inside the true
/// corner while its neighbours were recovered to within 0.1 px.
pub fn spans_loop(poly: &Polyline, v: &[usize]) -> bool {
    if !poly.closed || v.len() < 2 {
        return false;
    }
    if v.first() == v.last() {
        return true;
    }
    let n = poly.len();
    v[0] == 0 && v[v.len() - 1] == n - 1 && poly.points[0].dist(poly.points[n - 1]) < 1e-9
}

/// Distance from a vertex within which contour samples lie on the anti-aliasing chamfer
/// rather than on the edge, in pixels: one pixel, the sampling step of the level set.
pub const CORNER_CHAMFER: f64 = 1.0;
/// Turn between two fitted lines from which their meeting is treated as a corner whose
/// chamfer the intersection may cross (30 degrees).
pub const CORNER_TURN_MIN: f64 = std::f64::consts::PI / 6.0;

/// As [`adjust_vertices_at`], moving every vertex.
pub fn adjust_vertices(poly: &Polyline, seg: &Segmentation, max_shift: f64) -> Vec<Point> {
    adjust_vertices_at(poly, seg, max_shift, |_| true)
}

/// As [`adjust_vertices`], moving only the vertices `is_corner` selects.
///
/// Restricting this to corners is not an optimization, it is a correctness requirement.
/// Intersecting the fitted lines of two adjacent segments is well conditioned only when
/// they actually meet at an angle. On a smooth curve they are nearly parallel — 9.7
/// degrees apart on a 37-segment circle — and the intersection runs off far from the
/// curve. Clamping the displacement keeps it bounded but not *right*: the vertex still
/// moves sideways by up to the clamp, in a direction with no geometric meaning.
///
/// The measured consequence was subtle and expensive. Displaced vertices made the
/// polygon's turn angles erratic, corner detection then fired every few vertices, a
/// circle was cut into sixteen short runs, and no run was long enough for a cubic to be
/// worth its parameters. Curve fitting looked broken; the actual fault was here.
pub fn adjust_vertices_at(
    poly: &Polyline,
    seg: &Segmentation,
    max_shift: f64,
    is_corner: impl Fn(usize) -> bool,
) -> Vec<Point> {
    let v = &seg.vertices;
    let mut out: Vec<Point> = v.iter().map(|&i| poly.points[i]).collect();
    if v.len() < 3 {
        return out;
    }
    let n_pts = poly.len();
    let closed = spans_loop(poly, v);
    let n = v.len();
    let seg_count = n - 1;

    // Fit each segment's line from its actual points.
    //
    // Deliberately not via the prefix sums: on a closed contour the chosen vertex indices
    // wrap around the cut point, so a range like `350..12` is not expressible as a prefix
    // difference. An earlier version used the prefix path and silently returned `None`
    // for every wrapped segment, which made this whole function a no-op on exactly the
    // inputs it exists for. Walking the points is O(total points) and always correct.
    let lines: Vec<Option<(Point, Vec2)>> = (0..seg_count)
        .map(|k| {
            let (a, b) = (v[k], v[k + 1]);
            let (pa, pb) = (poly.points[a], poly.points[b]);
            let mut pts = Vec::new();
            let mut i = a;
            loop {
                pts.push((poly.points[i], 1.0 / (poly.sigma[i] * poly.sigma[i])));
                if i == b {
                    break;
                }
                i = (i + 1) % n_pts;
                if pts.len() > n_pts {
                    break;
                }
            }
            // The samples nearest a vertex sit on the chamfer, inside the true edge;
            // they are exactly the points that must not vote on where the edge is.
            // Drop the first pixel at each end when enough of the edge remains.
            let trimmed: Vec<(Point, f64)> = pts
                .iter()
                .copied()
                .filter(|(p, _)| p.dist(pa) > CORNER_CHAMFER && p.dist(pb) > CORNER_CHAMFER)
                .collect();
            if trimmed.len() >= 3 {
                fit_line(&trimmed)
            } else {
                fit_line(&pts)
            }
        })
        .collect();

    let interior: Vec<usize> = if closed {
        (0..seg_count).collect()
    } else {
        (1..n - 1).collect()
    };

    for &k in &interior {
        if !is_corner(k) {
            continue;
        }
        let (prev, next) = if closed {
            ((k + seg_count - 1) % seg_count, k % seg_count)
        } else {
            (k - 1, k)
        };
        let (Some((p0, d0)), Some((p1, d1))) = (lines[prev], lines[next]) else {
            continue;
        };

        let denom = d0.cross(d1);
        // Near-parallel neighbours have no well-conditioned intersection, and the vertex
        // between them is not really a corner.
        if denom.abs() < 1e-6 {
            continue;
        }
        let t = (p1 - p0).cross(d1) / denom;
        let hit = Point::new(p0.x + d0.x * t, p0.y + d0.y * t);
        // How far the measured vertex may legitimately sit from the true corner. The
        // anti-aliased boundary of a corner with interior angle theta is a level set of
        // the coverage field, which rounds the corner off: it passes about half a pixel
        // inside a right angle along the bisector and further for sharper ones, and the
        // nearest *sample* of it can be another pixel away. Measured on a 6 px bar at
        // 20 degrees: samples 1.0 px from each true corner, chords 0.12 px too far in.
        // A cap of 3 sigma (0.86 px there) refused every one of those intersections. The
        // allowance grows only with the turn, so two nearly collinear lines — the case
        // the comment above warns about — keep the tight cap.
        let turn = d0.cross(d1).abs().atan2(d0.dot(d1));
        let allowed = if turn >= CORNER_TURN_MIN {
            let half_interior = 0.5 * (std::f64::consts::PI - turn);
            max_shift + (CORNER_CHAMFER / half_interior.sin().max(0.2)).min(3.0)
        } else {
            max_shift
        };
        if hit.dist(out[k]) <= allowed {
            out[k] = hit;
        }
    }
    if closed {
        let first = out[0];
        if let Some(last) = out.last_mut() {
            *last = first;
        }
    }
    out
}

/// Weighted total-least-squares line through points, as (centroid, unit direction).
fn fit_line(pts: &[(Point, f64)]) -> Option<(Point, Vec2)> {
    let w: f64 = pts.iter().map(|p| p.1).sum();
    if pts.len() < 2 || w <= 0.0 {
        return None;
    }
    let mx = pts.iter().map(|(p, k)| p.x * k).sum::<f64>() / w;
    let my = pts.iter().map(|(p, k)| p.y * k).sum::<f64>() / w;
    let (mut cxx, mut cyy, mut cxy) = (0.0, 0.0, 0.0);
    for (p, k) in pts {
        let (dx, dy) = (p.x - mx, p.y - my);
        cxx += k * dx * dx;
        cyy += k * dy * dy;
        cxy += k * dx * dy;
    }
    let tr = cxx + cyy;
    let diff = cxx - cyy;
    let disc = (diff * diff + 4.0 * cxy * cxy).max(0.0).sqrt();
    let major = 0.5 * (tr + disc);
    let (dx, dy) = if cxy.abs() > 1e-12 {
        (major - cyy, cxy)
    } else if cxx >= cyy {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };
    let nrm = dx.hypot(dy);
    if nrm <= 1e-12 {
        return None;
    }
    Some((
        Point::new(mx, my),
        Vec2 {
            x: dx / nrm,
            y: dy / nrm,
        },
    ))
}

/// Perpendicular distance from `p` to the infinite line through `a` and `b`.
pub fn line_distance(p: Point, a: Point, b: Point) -> f64 {
    let d = b - a;
    let len = d.norm();
    if len <= f64::EPSILON {
        return p.dist(a);
    }
    ((p - a).cross(d) / len).abs()
}

/// Largest perpendicular deviation of the source polyline from a segmentation,
/// in units of each point's own sigma. This is the quantity `tau` bounds.
pub fn max_normalized_deviation(poly: &Polyline, seg: &Segmentation) -> f64 {
    let mut worst: f64 = 0.0;
    for pair in seg.vertices.windows(2) {
        let (i, j) = (pair[0], pair[1]);
        let (a, b) = (poly.points[i], poly.points[j]);
        for k in i + 1..j {
            worst = worst.max(line_distance(poly.points[k], a, b) / poly.sigma[k]);
        }
    }
    worst
}

/// A boundary fitted with a mixed alphabet: straight where the shape is straight, cubic
/// where it curves.
#[derive(Debug, Clone)]
pub struct FittedPath {
    /// Starting point of the path.
    pub start: Point,
    /// The path's segments, in order, each running from the previous segment's end
    /// (or `start`, for the first).
    pub segments: Vec<curves::Segment>,
    /// True when the path closes back on `start`.
    pub closed: bool,
}

impl FittedPath {
    /// Total description length, in parameters: the start point plus every segment's own.
    pub fn params(&self) -> f64 {
        2.0 + self.segments.iter().map(|s| s.params()).sum::<f64>()
    }
    /// Number of segments.
    pub fn len(&self) -> usize {
        self.segments.len()
    }
    /// True when the path has no segments.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The path's endpoint: its last segment's end, or `start` if it has no segments.
    pub fn end(&self) -> Point {
        self.segments.last().map(|s| s.end()).unwrap_or(self.start)
    }

    /// The same curve traversed backwards.
    ///
    /// Needed because a boundary is stored once but bounds two faces, and it runs the
    /// opposite way around each of them. Reversing a cubic swaps its control points as
    /// well as its endpoints, so the reversed curve is geometrically identical rather
    /// than merely similar — which is what keeps the two faces sharing an edge exactly.
    pub fn reversed(&self) -> FittedPath {
        let mut pts: Vec<Point> = Vec::with_capacity(self.segments.len() + 1);
        pts.push(self.start);
        for s in &self.segments {
            pts.push(s.end());
        }
        let mut out = Vec::with_capacity(self.segments.len());
        for (i, s) in self.segments.iter().enumerate().rev() {
            let prev = pts[i];
            out.push(match *s {
                curves::Segment::Line(_) => curves::Segment::Line(prev),
                curves::Segment::Cubic(c1, c2, _) => curves::Segment::Cubic(c2, c1, prev),
                // Same circle, same side of the chord, opposite direction of travel.
                curves::Segment::Arc {
                    rx,
                    ry,
                    phi,
                    large_arc,
                    sweep,
                    ..
                } => curves::Segment::Arc {
                    rx,
                    ry,
                    phi,
                    large_arc,
                    sweep: !sweep,
                    end: prev,
                },
            });
        }
        FittedPath {
            start: self.end(),
            segments: out,
            closed: self.closed,
        }
    }
}

/// Turn angle, in degrees, above which a vertex is a corner rather than a smooth join.
pub const CORNER_DEGREES: f64 = 45.0;

/// Fit a measured boundary with lines and cubics, choosing per run by MDL.
///
/// Two stages, in this order for a reason. The line dynamic program runs first and
/// decides — globally, under the same objective as everything else — where the *corners*
/// are. Only then are cubics fitted, to the smooth runs between them.
///
/// Doing it the other way round does not work. Fitting curves first means the curves
/// absorb the corners: a cubic will happily round off a sharp vertex to reduce its own
/// residual, and once it has, no later analysis can tell that a corner was ever there.
/// Corner placement is a global, discrete decision and belongs with the other global,
/// discrete decision.
///
/// Each smooth run is then fitted both ways and the cheaper description wins. A cubic
/// costs three times a line segment (6 parameters against 2), so it is adopted only where
/// it removes at least that much residual — on a circle it removes an enormous amount,
/// on a straight edge none at all, and the objective sorts the two cases out without a
/// special case for either.
pub fn fit_path(poly: &Polyline, cfg: &FitConfig) -> FittedPath {
    let seg = optimal_polygon(poly, cfg);
    let v = &seg.vertices;
    let n_v = v.len();
    let raw: Vec<Point> = v.iter().map(|&i| poly.points[i]).collect();

    if n_v < 2 {
        return FittedPath {
            start: raw.first().copied().unwrap_or(Point::new(0.0, 0.0)),
            segments: Vec::new(),
            closed: poly.closed,
        };
    }

    let closed = poly.closed && v.first() == v.last();
    let n_seg = n_v - 1;

    // Which joins are corners? Measured on the *unadjusted* polygon: the DP has already
    // removed the staircase wobble, and adjustment must not run before this decision,
    // since adjustment is only meaningful at corners in the first place.
    let turn_at = |k: usize| -> f64 {
        let (prev, next) = if closed {
            ((k + n_seg - 1) % n_seg, k % n_seg)
        } else {
            if k == 0 || k + 1 >= n_v {
                return 180.0; // endpoints of an open curve are always breaks
            }
            (k - 1, k)
        };
        let a = raw[prev + 1] - raw[prev];
        let b = raw[next + 1] - raw[next];
        let (na, nb) = (a.norm(), b.norm());
        if na < 1e-9 || nb < 1e-9 {
            return 0.0;
        }
        (a.dot(b) / (na * nb)).clamp(-1.0, 1.0).acos().to_degrees()
    };

    let mut breaks: Vec<usize> = Vec::new();
    for k in 0..n_v {
        // An open curve always breaks at its endpoints; anywhere else, a sharp turn.
        let is_end = !closed && (k == 0 || k == n_v - 1);
        if is_end || turn_at(k) >= CORNER_DEGREES {
            breaks.push(k);
        }
    }
    if breaks.is_empty() {
        breaks.push(0);
        if closed {
            breaks.push(n_v - 1);
        }
    }
    if closed && *breaks.last().unwrap() != n_v - 1 {
        breaks.push(n_v - 1);
    }
    breaks.dedup();

    // Now sharpen the corners, and only the corners.
    let corner_set: std::collections::HashSet<usize> = breaks.iter().copied().collect();
    let max_shift = 3.0 * poly.sigma.iter().copied().fold(0.0, f64::max).max(0.25);
    let adjusted = adjust_vertices_at(poly, &seg, max_shift, |k| corner_set.contains(&k));

    let start = adjusted[breaks[0]];
    let mut segments: Vec<curves::Segment> = Vec::new();

    for w in breaks.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b <= a {
            continue;
        }
        // Build the straight description explicitly, and score it the *same way* the
        // curved one is scored: distance from every measured point to the geometry that
        // would actually be emitted.
        //
        // The previous version costed this branch with `chi2_line` over vertex index
        // ranges, which was wrong twice over. On a closed contour the chosen vertices wrap
        // past the cut point, so `min`/`max` turned a short span into one covering nearly
        // the whole boundary. And even where indices behaved, it measured residual to each
        // span's *best-fit line* rather than to the chord actually drawn — a systematically
        // smaller number. Lines were therefore compared against cubics on favourable
        // terms, and won runs they should have lost.
        let line_segs: Vec<curves::Segment> = (a..b)
            .map(|k| curves::Segment::Line(adjusted[k + 1]))
            .collect();
        let line_params = line_segs.len() as f64 * PARAMS_LINE;
        let run_pts = gather(poly, v[a], v[b]);
        let run_sig = gather_sigma(poly, v[a], v[b]);
        let line_chi2 = curves::chi2(&run_pts, &run_sig, adjusted[a], &line_segs);
        let line_cost = 0.5 * line_chi2 + cfg.lambda * line_params;

        let tol = cfg.tau * run_sig.iter().copied().fold(0.0f64, f64::max).max(0.05);

        // How smooth should the fit be? kurbo fits the source faithfully, and our source
        // is a *measurement*: a polyline carrying about 0.05px of extraction wobble. Asked
        // for 0.1px accuracy it will dutifully chase every wiggle — 92 cubics for a circle
        // that four would describe. Its author flags exactly this, noting the method was
        // never validated on noisy input.
        //
        // Rather than invent a smoothing constant, let the objective decide. Fit at a
        // range of tolerances and keep whichever description is cheapest under the same
        // MDL cost used everywhere else. Chasing noise is then rejected on its own terms:
        // the extra cubics cost more than the residual they remove.
        let mut cubic: Option<Vec<curves::Segment>> = None;
        let mut best_cost = line_cost;
        let mut best: Option<(f64, usize)> = None;
        let run_closed = closed && breaks.len() == 2;
        // Wide search, deliberately. The governing directive is quality over speed, and
        // the (smoothing, tolerance) surface turned out to be sharp: a coarse grid landed
        // a circle on 9 segments and an ellipse on 34, from what is essentially the same
        // problem. Smoothing is capped at a sixth of the run so a window can never span
        // enough of the shape to flatten it.
        let hw_cap = (run_pts.len() / 6).max(1);
        for hw in [0usize, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64] {
            if hw > hw_cap {
                break;
            }
            for mult in [0.25f64, 0.5, 0.75, 1.0, 1.5, 2.0, 4.0, 8.0] {
                let t = tol * mult;
                let Some(segs) = curves::fit_cubics_smoothed(&run_pts, t, hw, run_closed, false)
                else {
                    continue;
                };
                // Always scored against the *original* measurements, never the smoothed
                // copy, so oversmoothing is punished rather than hidden.
                let chi2 = curves::chi2(&run_pts, &run_sig, adjusted[a], &segs);
                let params: f64 = segs.iter().map(|s| s.params()).sum();
                let cost = 0.5 * chi2 + cfg.lambda * params;
                if cost < best_cost {
                    best_cost = cost;
                    best = Some((t, hw));
                    cubic = Some(segs);
                }
            }
        }
        // Pay for kurbo's optimal (and far slower) fitter only at the winning setting.
        if let Some((t, hw)) = best {
            if let Some(segs) = curves::fit_cubics_smoothed(&run_pts, t, hw, run_closed, true) {
                let chi2 = curves::chi2(&run_pts, &run_sig, adjusted[a], &segs);
                let params: f64 = segs.iter().map(|s| s.params()).sum();
                if 0.5 * chi2 + cfg.lambda * params <= best_cost {
                    cubic = Some(segs);
                }
            }
        }

        if std::env::var("INKVEC_DEBUG_FIT").is_ok() {
            eprintln!(
                "    run {a}..{b}: {} pts | LINE {} segs chi2 {:.0} cost {:.0} | CUBIC {} segs cost {:.0}",
                run_pts.len(),
                line_segs.len(),
                line_chi2,
                line_cost,
                cubic.as_ref().map(|c| c.len()).unwrap_or(0),
                best_cost
            );
        }

        match cubic {
            Some(mut segs) => {
                // Pin the run's end to the polygon vertex so neighbouring runs stay joined.
                if let Some(last) = segs.last_mut() {
                    *last = match *last {
                        curves::Segment::Line(_) => curves::Segment::Line(adjusted[b]),
                        curves::Segment::Cubic(c1, c2, _) => {
                            curves::Segment::Cubic(c1, c2, adjusted[b])
                        }
                        curves::Segment::Arc {
                            rx,
                            ry,
                            phi,
                            large_arc,
                            sweep,
                            ..
                        } => curves::Segment::Arc {
                            rx,
                            ry,
                            phi,
                            large_arc,
                            sweep,
                            end: adjusted[b],
                        },
                    };
                }
                segments.extend(segs);
            }
            None => {
                for k in a..b {
                    segments.push(curves::Segment::Line(adjusted[k + 1]));
                }
            }
        }
    }

    FittedPath {
        start,
        segments,
        closed,
    }
}

/// Points of `poly` from index `a` to `b`, following the polyline and wrapping when closed.
///
/// When `a == b` the run is a complete cycle, not an empty one. That distinction was a
/// real bug: a closed contour is cut for the dynamic program, so its first and last chosen
/// vertices are the *same* index, and the naive loop stopped immediately and handed the
/// fitter a single point. Every closed curve therefore reported a zero residual for the
/// straight description and no curved description at all — circles came back as thirty-nine
/// line segments, and the curve fitting looked useless when it was simply never invoked.
fn gather(poly: &Polyline, a: usize, b: usize) -> Vec<Point> {
    let n = poly.len();
    let mut out = vec![poly.points[a]];
    let mut i = a;
    loop {
        i = (i + 1) % n;
        out.push(poly.points[i]);
        if i == b || out.len() > n {
            break;
        }
    }
    out
}

fn gather_sigma(poly: &Polyline, a: usize, b: usize) -> Vec<f64> {
    let n = poly.len();
    let mut out = vec![poly.sigma[a]];
    let mut i = a;
    loop {
        i = (i + 1) % n;
        out.push(poly.sigma[i]);
        if i == b || out.len() > n {
            break;
        }
    }
    out
}
