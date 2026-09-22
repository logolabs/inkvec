//! Cubic Bézier fitting, via `kurbo` (DESIGN.md S4, §2.6).
//!
//! The line-only alphabet has a hard floor that no amount of segmentation quality can
//! lift. A circle approximated by chords within tolerance `e` needs about
//! `pi / sqrt(e / 2r)` segments — 36 of them for a 46px radius at 0.1px — while four
//! cubics track the same circle to about 0.02% error. The segmentation was already
//! optimal; it was optimal over the wrong alphabet.
//!
//! We do not implement the fitting. `kurbo::fit_to_bezpath_opt` computes optimal
//! subdivision points and is, by its author's assessment, the best implementation in
//! either the literature or shipping products: it works to an approximate Fréchet
//! distance (orientation-preserving, unlike Hausdorff), and it solves the G1 control-arm
//! problem in closed form via a quartic rather than iterating — which sidesteps the three
//! local minima that make Schneider's Graphics Gems algorithm get stuck.
//!
//! What we supply is the *source curve*: the measured boundary, parametrized by arc
//! length, with corners already removed by splitting. Corners are handled here rather
//! than through kurbo's cusp callback because we have better information than curve
//! geometry alone — the line dynamic program has already decided, under the MDL
//! objective, where the corners are.

use inkvec_core::{Point, Vec2};
use kurbo::{
    fit_to_bezpath_opt, BezPath, CurveFitSample, ParamCurve, ParamCurveDeriv, ParamCurveFit,
    PathEl, Point as KPoint, Vec2 as KVec2,
};

/// A measured polyline presented to kurbo as a smooth source curve.
///
/// Parametrized by normalized arc length so that sampling is uniform in space rather than
/// in vertex index — vertex spacing along a traced boundary is irregular, and an
/// index-based parametrization would make the fit chase the sampling instead of the shape.
struct PolylineCurve<'a> {
    pts: &'a [Point],
    /// Cumulative arc length at each vertex; `cum[n-1]` is the total.
    cum: Vec<f64>,
}

impl<'a> PolylineCurve<'a> {
    fn new(pts: &'a [Point]) -> Option<Self> {
        if pts.len() < 2 {
            return None;
        }
        let mut cum = Vec::with_capacity(pts.len());
        let mut acc = 0.0;
        cum.push(0.0);
        for k in 1..pts.len() {
            acc += pts[k].dist(pts[k - 1]);
            cum.push(acc);
        }
        if acc <= 1e-12 {
            return None;
        }
        Some(Self { pts, cum })
    }

    fn total(&self) -> f64 {
        *self.cum.last().unwrap_or(&0.0)
    }

    /// Position at normalized arc length `t`, by linear interpolation.
    fn pos(&self, t: f64) -> KPoint {
        let s = t.clamp(0.0, 1.0) * self.total();
        let mut lo = 0usize;
        let mut hi = self.cum.len() - 1;
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if self.cum[mid] <= s {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let seg_len = (self.cum[hi] - self.cum[lo]).max(1e-12);
        let u = ((s - self.cum[lo]) / seg_len).clamp(0.0, 1.0);
        let (a, b) = (self.pts[lo], self.pts[hi]);
        KPoint::new(a.x + (b.x - a.x) * u, a.y + (b.y - a.y) * u)
    }

    /// Position and unit tangent at normalized arc length `t`.
    ///
    /// The tangent is a **central difference over a window**, not the direction of the
    /// containing segment. That distinction turned out to matter enormously. A polyline's
    /// tangent is discontinuous at every single vertex, so a fitter that samples tangents
    /// sees a curve composed entirely of corners and subdivides at each — measured effect,
    /// 92 cubics for a circle that four describe, and no tolerance setting changed it
    /// because the subdivision was driven by apparent corners rather than by error.
    ///
    /// Smoothing only the tangent leaves position exact, so the fit still passes through
    /// the measurements; it just stops treating sampling artefacts as shape.
    fn at(&self, t: f64) -> (KPoint, KVec2) {
        let p = self.pos(t);
        let e = 1e-4;
        let (a, b) = (self.pos((t - e).max(0.0)), self.pos((t + e).min(1.0)));
        let d = KVec2::new(b.x - a.x, b.y - a.y);
        let n = d.hypot();
        if n < 1e-12 {
            return (p, KVec2::new(1.0, 0.0));
        }
        (p, d / n)
    }
}

impl ParamCurveFit for PolylineCurve<'_> {
    fn sample_pt_tangent(&self, t: f64, _sign: f64) -> CurveFitSample {
        let (p, tan) = self.at(t);
        CurveFitSample { p, tangent: tan }
    }

    fn sample_pt_deriv(&self, t: f64) -> (KPoint, KVec2) {
        let (p, tan) = self.at(t);
        // Arc-length parametrization: speed is constant and equal to the total length.
        (p, tan * self.total())
    }

    fn break_cusp(&self, _range: std::ops::Range<f64>) -> Option<f64> {
        // Corners are split before we get here, using the MDL segmentation rather than
        // local curve geometry — a better-informed decision than this callback can make.
        None
    }
}

/// A fitted span of the boundary: straight, one or more cubics, or a circular arc.
#[derive(Debug, Clone)]
pub enum Segment {
    /// A straight line to this endpoint.
    Line(Point),
    /// Control points and endpoint of a cubic.
    Cubic(Point, Point, Point),
    /// An arc in SVG `A` endpoint parametrization: `A rx ry phi large_arc sweep x y`.
    ///
    /// The centre is not stored; it is recovered from the two endpoints, the radii, the
    /// rotation and the flags exactly as an SVG renderer does it (see [`arc_center`]).
    /// `sweep` is true when the arc runs in the direction of increasing angle in the raw
    /// coordinate system, i.e. clockwise on a y-down screen — SVG's `sweep-flag = 1`.
    ///
    /// `rx == ry` with `phi == 0` is a circular arc, which is what
    /// [`Segment::circular_arc`] builds and what most of the tracer emits.
    Arc {
        /// Radius along the ellipse's own x-axis.
        rx: f64,
        /// Radius along the ellipse's own y-axis.
        ry: f64,
        /// Rotation of the ellipse's x-axis, in radians.
        phi: f64,
        /// SVG's `large-arc-flag`: true when the arc sweeps the larger of the two
        /// possible angles between the endpoints.
        large_arc: bool,
        /// SVG's `sweep-flag`: true when the arc runs in the direction of increasing
        /// angle in the raw coordinate system.
        sweep: bool,
        /// Endpoint of the arc.
        end: Point,
    },
}

/// Parameters charged for a circular arc.
///
/// SVG writes seven numbers (`rx ry rotation large-arc sweep x y`), but for a *circular*
/// arc `rx == ry` and the rotation is meaningless, so the document carries one radius,
/// two one-bit flags and an endpoint. We charge the flags a full parameter each — they
/// are a choice the designer has to make when editing — which gives 1 + 2 + 2 = 5. It is
/// deliberately not 3 (radius + endpoint): a description length that ignored the flags
/// would let an arc undercut a cubic on every short run where the two are
/// indistinguishable, which is not the compactness the objective is meant to reward.
pub const PARAMS_ARC: f64 = 5.0;

/// Parameters charged for an elliptical arc.
///
/// The document carries all seven numbers SVG writes: two radii, a rotation, two flags
/// and an endpoint. One more than a cubic, so an ellipse has to be a materially better
/// description of its span, not merely an equal one.
pub const PARAMS_ELLIPTICAL_ARC: f64 = 7.0;

impl Segment {
    /// A circular arc: equal radii, no rotation.
    pub fn circular_arc(radius: f64, large_arc: bool, sweep: bool, end: Point) -> Segment {
        Segment::Arc {
            rx: radius,
            ry: radius,
            phi: 0.0,
            large_arc,
            sweep,
            end,
        }
    }

    /// Whether this arc is a circle's (equal radii, no rotation). True for everything
    /// else, which has no radii to compare.
    pub fn is_circular(&self) -> bool {
        match *self {
            Segment::Arc { rx, ry, phi, .. } => {
                (rx - ry).abs() <= 1e-9 * rx.abs().max(1.0) && phi == 0.0
            }
            _ => true,
        }
    }

    /// The endpoint this segment ends at.
    pub fn end(&self) -> Point {
        match self {
            Segment::Line(p) => *p,
            Segment::Cubic(_, _, p) => *p,
            Segment::Arc { end, .. } => *end,
        }
    }

    /// Numeric parameters this segment adds to the document.
    ///
    /// Every arm reads the constant that names the count rather than restating it. A line
    /// really is two numbers and a cubic really is six, so literals here would be correct
    /// today and silently stale the moment either constant moved -- and this function and
    /// `merge::segment_params`, which does reference them, would then disagree about the
    /// price of the same segment.
    pub fn params(&self) -> f64 {
        match self {
            Segment::Line(_) => crate::PARAMS_LINE,
            Segment::Cubic(..) => crate::multimodel::params_cubic(),
            Segment::Arc { .. } => {
                if self.is_circular() {
                    PARAMS_ARC
                } else {
                    PARAMS_ELLIPTICAL_ARC
                }
            }
        }
    }
}

/// Cubic Bernstein basis at `t`.
#[inline]
pub fn bernstein(t: f64) -> [f64; 4] {
    let u = 1.0 - t;
    [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t]
}

/// A cubic Bezier at `t`.
///
/// One copy, because three modules had grown their own and a curve evaluated two
/// different ways is two different curves once the results are compared bit for bit.
#[inline]
pub fn eval_cubic(p: [Point; 4], t: f64) -> Point {
    let b = bernstein(t);
    Point::new(
        b[0] * p[0].x + b[1] * p[1].x + b[2] * p[2].x + b[3] * p[3].x,
        b[0] * p[0].y + b[1] * p[1].y + b[2] * p[2].y + b[3] * p[3].y,
    )
}

/// The derivative of a cubic Bezier at `t`: a tangent, unnormalised.
///
/// `merge::cubic_tangent_at` computes the same quantity but factors the 3 out, which is
/// a different rounding, so it is deliberately not folded in here.
#[inline]
pub fn cubic_tangent(p: [Point; 4], t: f64) -> Vec2 {
    let u = 1.0 - t;
    let (w0, w1, w2) = (3.0 * u * u, 6.0 * u * t, 3.0 * t * t);
    Vec2 {
        x: w0 * (p[1].x - p[0].x) + w1 * (p[2].x - p[1].x) + w2 * (p[3].x - p[2].x),
        y: w0 * (p[1].y - p[0].y) + w1 * (p[2].y - p[1].y) + w2 * (p[3].y - p[2].y),
    }
}

/// Does this cubic cross itself somewhere strictly inside its own span?
///
/// A loop is nearly invisible to a fidelity objective — the curve still passes through
/// every measured point, the rendered pixels barely change — and it is a genuine defect
/// in the thing we are actually producing: dragging a control point on a looped cubic in
/// an editor does not do what anyone expects. Measured over 180 real emoji, 53 of them
/// (29%) contained at least one self-intersecting ring, against VTracer's 10 (5.6%), so
/// this is not a rare pathology to be caught by a fallback but a systematic one to be
/// excluded by construction.
///
/// The test is exact and closed form. Writing the curve in the power basis
/// `B(t) = a t³ + b t² + c t + d`, a crossing needs `B(t) = B(s)` with `t != s`, and the
/// difference factors:
///
/// ```text
///     B(t) - B(s) = (t - s) · [ a(t² + ts + s²) + b(t + s) + c ]
/// ```
///
/// With `u = t + s` and `v = ts`, the bracket is `a(u² - v) + b·u + c`. Substituting
/// `w = u² - v` makes the two coordinate equations **linear** in `(w, u)`, so one 2x2
/// solve gives both; `t` and `s` are then the roots of `z² - u·z + v`. No iteration, no
/// tolerance on a subdivision depth.
///
/// A degenerate system (the linear part is singular) means the cubic has degenerated
/// towards a conic or a line, neither of which crosses itself, so it is reported as
/// clean.
pub fn cubic_self_intersects(p0: Point, p1: Point, p2: Point, p3: Point) -> bool {
    // Power basis.
    let ax = -p0.x + 3.0 * p1.x - 3.0 * p2.x + p3.x;
    let ay = -p0.y + 3.0 * p1.y - 3.0 * p2.y + p3.y;
    let bx = 3.0 * (p0.x - 2.0 * p1.x + p2.x);
    let by = 3.0 * (p0.y - 2.0 * p1.y + p2.y);
    let cx = 3.0 * (p1.x - p0.x);
    let cy = 3.0 * (p1.y - p0.y);

    // [ax bx][w]   [-cx]
    // [ay by][u] = [-cy]
    let det = ax * by - ay * bx;
    if det.abs() < 1e-12 {
        return false;
    }
    let w = (-cx * by + cy * bx) / det;
    let u = (ax * -cy - ay * -cx) / det;
    let v = u * u - w;

    // t and s are roots of z² - u z + v.
    let disc = u * u - 4.0 * v;
    if disc <= 0.0 {
        return false;
    }
    let r = disc.sqrt();
    let (t, sroot) = (0.5 * (u - r), 0.5 * (u + r));
    // Strictly inside, and distinct. An endpoint touching is a closed loop, not a defect.
    const EPS: f64 = 1e-9;
    t > EPS && t < 1.0 - EPS && sroot > EPS && sroot < 1.0 - EPS && (sroot - t).abs() > EPS
}

/// Centre parametrization of an SVG arc.
///
/// This is the endpoint-to-centre conversion of the SVG specification (appendix F.6.5).
/// When the chord does not fit inside the ellipse the radii are scaled up together, as
/// the specification requires — so a slightly perturbed endpoint yields a slightly
/// different ellipse rather than an invalid one. The radii returned are the ones actually
/// drawn.
///
/// A point of the arc at parameter `t` is
/// `centre + R(phi) · (rx cos t, ry sin t)`, and `delta` is signed: positive for
/// `sweep_flag = true`.
#[derive(Debug, Clone, Copy)]
pub struct ArcFrame {
    /// Centre of the ellipse.
    pub c: Point,
    /// Radius along the ellipse's own x-axis, after scaling to fit the chord.
    pub rx: f64,
    /// Radius along the ellipse's own y-axis, after scaling to fit the chord.
    pub ry: f64,
    /// Rotation of the ellipse's x-axis, in radians.
    pub phi: f64,
    /// Parameter `t` at the arc's start.
    pub theta1: f64,
    /// Signed sweep, in radians; positive for `sweep_flag = true`.
    pub delta: f64,
}

impl ArcFrame {
    /// The point at parameter `t`.
    pub fn at(&self, t: f64) -> Point {
        let (sp, cp) = self.phi.sin_cos();
        let (x, y) = (self.rx * t.cos(), self.ry * t.sin());
        Point::new(self.c.x + cp * x - sp * y, self.c.y + sp * x + cp * y)
    }

    /// A length that bounds the arc from above, for choosing a sample count.
    pub fn span(&self) -> f64 {
        (self.rx.abs().max(self.ry.abs()) * self.delta).abs()
    }
}

/// Endpoint-to-centre conversion of an SVG arc (elliptical case). See [`ArcFrame`].
pub fn arc_ellipse_center(
    start: Point,
    rx: f64,
    ry: f64,
    phi: f64,
    large_arc: bool,
    sweep: bool,
    end: Point,
) -> ArcFrame {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    let mid = Point::new((start.x + end.x) * 0.5, (start.y + end.y) * 0.5);
    let hx = (start.x - end.x) * 0.5;
    let hy = (start.y - end.y) * 0.5;
    if hx * hx + hy * hy <= 1e-24 || rx <= 1e-12 || ry <= 1e-12 {
        // Degenerate: coincident endpoints, or no ellipse at all. SVG omits the arc; we
        // return a zero-sweep frame at the start so sampling produces nothing surprising.
        return ArcFrame {
            c: Point::new(start.x - rx, start.y),
            rx,
            ry,
            phi,
            theta1: 0.0,
            delta: 0.0,
        };
    }
    let (sp, cp) = phi.sin_cos();
    // The half-chord in the ellipse's own frame.
    let x1 = cp * hx + sp * hy;
    let y1 = -sp * hx + cp * hy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den > 0.0 { (num / den).sqrt() } else { 0.0 };
    if large_arc == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1 / ry;
    let cyp = -coef * ry * x1 / rx;
    let c = Point::new(cp * cxp - sp * cyp + mid.x, sp * cxp + cp * cyp + mid.y);
    let v1 = Vec2 {
        x: (x1 - cxp) / rx,
        y: (y1 - cyp) / ry,
    };
    let v2 = Vec2 {
        x: (-x1 - cxp) / rx,
        y: (-y1 - cyp) / ry,
    };
    let theta1 = v1.y.atan2(v1.x);
    let mut delta = v2.y.atan2(v2.x) - theta1;
    if !sweep && delta > 0.0 {
        delta -= std::f64::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f64::consts::TAU;
    }
    ArcFrame {
        c,
        rx,
        ry,
        phi,
        theta1,
        delta,
    }
}

/// The circular case: `(centre, radius, start_angle, sweep)`.
pub fn arc_center(
    start: Point,
    radius: f64,
    large_arc: bool,
    sweep: bool,
    end: Point,
) -> (Point, f64, f64, f64) {
    let f = arc_ellipse_center(start, radius, radius, 0.0, large_arc, sweep, end);
    (f.c, f.rx, f.theta1, f.delta)
}

/// Fit cubics to a run of measured points, at a tolerance derived from their uncertainty.
///
/// Returns `None` when the run is too short or degenerate to fit, in which case the
/// caller should keep the straight segmentation.
pub fn fit_cubics(pts: &[Point], accuracy: f64) -> Option<Vec<Segment>> {
    fit_cubics_with(pts, accuracy, true)
}

/// Low-pass the measured boundary before fitting.
///
/// kurbo fits its source faithfully, and our source is a measurement carrying about
/// 0.05px of extraction wobble. Faithfulness to noise is the whole problem: asked for
/// tight accuracy the fitter tracks every wiggle and returns dozens of cubics, and the
/// only way to make it return four is to loosen the tolerance so far that the four are
/// placed badly. Measured on a 45px circle, a four-cubic fit obtained that way deviated
/// 0.65px where the theoretical four-cubic approximation of a circle deviates 0.012px.
///
/// Neither end of that trade is the answer, because the trade itself is an artefact. The
/// noise is not signal, so it should be removed from the *source* rather than tolerated
/// in the *fit*. Levien notes this case explicitly as unexplored, observing that the
/// method was never validated on noisy input.
///
/// The window is bounded by what it costs: smoothing over an arc `s` on a curve of radius
/// `r` pulls the curve inward by about `s^2 / 8r`, so a window is only safe while that
/// bias stays below the measurement uncertainty it is removing. Chi-squared is always
/// evaluated against the *original* points, so a window that oversmooths is rejected by
/// the objective rather than quietly accepted.
fn smooth(pts: &[Point], half_window: usize, closed: bool) -> Vec<Point> {
    let n = pts.len();
    let h = half_window as i64;
    if half_window == 0 || n < 2 * half_window + 3 {
        return pts.to_vec();
    }

    // Savitzky-Golay: fit a local quadratic and evaluate it at the centre, rather than
    // averaging the window.
    //
    // A moving average is the obvious choice and the wrong one. Averaging a curve pulls
    // it toward its chord, shrinking a circle of radius `r` by about `s^2 / 8r` over a
    // window of arc `s` — 0.13px for a nine-point window on a 46px circle, which is
    // larger than the 0.05px noise being removed. The fit then inherits a bias it cannot
    // undo, and the objective correctly rejects it: a four-cubic circle came out 17 times
    // worse than the theoretical four-cubic approximation, so lines kept winning.
    //
    // A quadratic reproduces constant curvature exactly, so it removes the noise and
    // leaves the shape where it was.
    (0..n)
        .map(|k| {
            let idx = |d: i64| -> Point {
                let i = k as i64 + d;
                let i = if closed {
                    i.rem_euclid(n as i64)
                } else {
                    i.clamp(0, n as i64 - 1)
                };
                pts[i as usize]
            };
            // Least squares for a + b*t + c*t^2 with t = offset, evaluated at t = 0,
            // which reduces to a weighted combination of the window's moments.
            let mut s0 = 0.0;
            let mut s2 = 0.0;
            let mut s4 = 0.0;
            let (mut sx, mut sx2, mut sy, mut sy2) = (0.0, 0.0, 0.0, 0.0);
            for d in -h..=h {
                let t = d as f64;
                let (t2, t4) = (t * t, t * t * t * t);
                let p = idx(d);
                s0 += 1.0;
                s2 += t2;
                s4 += t4;
                sx += p.x;
                sx2 += p.x * t2;
                sy += p.y;
                sy2 += p.y * t2;
            }
            // Solve [[s0, s2], [s2, s4]] [a, c]^T = [sum, sum*t^2]^T for `a` (the value
            // at the centre). The odd-power terms decouple and do not affect it.
            let det = s0 * s4 - s2 * s2;
            if det.abs() < 1e-12 {
                return pts[k];
            }
            let ax = (sx * s4 - sx2 * s2) / det;
            let ay = (sy * s4 - sy2 * s2) / det;
            Point::new(ax, ay)
        })
        .collect()
}

/// Fit cubics to a denoised copy of the run.
pub fn fit_cubics_smoothed(
    pts: &[Point],
    accuracy: f64,
    half_window: usize,
    closed: bool,
    optimal: bool,
) -> Option<Vec<Segment>> {
    let src = smooth(pts, half_window, closed);
    fit_cubics_with(&src, accuracy, optimal)
}

/// As [`fit_cubics`], choosing between kurbo's two fitters.
///
/// `optimal` uses `fit_to_bezpath_opt`, which searches for subdivision points that
/// equalize error and is near-minimal in segment count — and roughly fifty times slower.
/// The tolerance sweep uses the fast fitter to rank candidates and pays for the optimal
/// one only on the winner.
pub fn fit_cubics_with(pts: &[Point], accuracy: f64, optimal: bool) -> Option<Vec<Segment>> {
    let curve = PolylineCurve::new(pts)?;
    let acc = accuracy.max(1e-4);
    let path: BezPath = if optimal {
        fit_to_bezpath_opt(&curve, acc)
    } else {
        kurbo::fit_to_bezpath(&curve, acc)
    };

    let mut out = Vec::new();
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(_) => {}
            PathEl::LineTo(p) => out.push(Segment::Line(Point::new(p.x, p.y))),
            PathEl::CurveTo(a, b, c) => out.push(Segment::Cubic(
                Point::new(a.x, a.y),
                Point::new(b.x, b.y),
                Point::new(c.x, c.y),
            )),
            PathEl::QuadTo(a, b) => {
                // Elevate to cubic so the output alphabet stays uniform.
                let p0 = out.last().map(|s: &Segment| s.end()).unwrap_or(pts[0]);
                let c1 = Point::new(
                    p0.x + 2.0 / 3.0 * (a.x - p0.x),
                    p0.y + 2.0 / 3.0 * (a.y - p0.y),
                );
                let c2 = Point::new(b.x + 2.0 / 3.0 * (a.x - b.x), b.y + 2.0 / 3.0 * (a.y - b.y));
                out.push(Segment::Cubic(c1, c2, Point::new(b.x, b.y)));
            }
            PathEl::ClosePath => {}
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Densely sample a fitted run, at roughly uniform spacing in *space*.
///
/// Spacing has to follow arc length rather than a fixed count per segment. A fixed count
/// gives a long cubic coarse samples — a quarter-circle of a 46px radius sampled 24 times
/// lands them 3px apart — so nearest-sample distance acquires an error floor of about
/// half that. The floor then scales with segment length, which silently penalises exactly
/// the long segments the fit is trying to find: measured effect, the objective preferring
/// 68 cubics for a circle over 4, because the 4 looked inaccurate when they were not.
pub(crate) fn sample_run(start: Point, segs: &[Segment], spacing: f64) -> Vec<Point> {
    let mut out = Vec::new();
    let mut cur = start;
    out.push(cur);
    for s in segs {
        match *s {
            Segment::Line(p) => {
                let n = ((cur.dist(p) / spacing).ceil() as usize).clamp(1, 4096);
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    out.push(Point::new(
                        cur.x + (p.x - cur.x) * t,
                        cur.y + (p.y - cur.y) * t,
                    ));
                }
                cur = p;
            }
            Segment::Cubic(a, b, p) => {
                // Control polygon length bounds the arc length from above.
                let approx = cur.dist(a) + a.dist(b) + b.dist(p);
                let n = ((approx / spacing).ceil() as usize).clamp(4, 4096);
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let mt = 1.0 - t;
                    let (w0, w1, w2, w3) =
                        (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
                    out.push(Point::new(
                        w0 * cur.x + w1 * a.x + w2 * b.x + w3 * p.x,
                        w0 * cur.y + w1 * a.y + w2 * b.y + w3 * p.y,
                    ));
                }
                cur = p;
            }
            Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end,
            } => {
                let f = arc_ellipse_center(cur, rx, ry, phi, large_arc, sweep, end);
                let n = ((f.span() / spacing).ceil() as usize).clamp(2, 4096);
                for i in 1..n {
                    out.push(f.at(f.theta1 + f.delta * i as f64 / n as f64));
                }
                // Land exactly on the stored endpoint so joins stay watertight even when
                // the radius had to be scaled to span the chord.
                out.push(end);
                cur = end;
            }
        }
    }
    out
}

/// Distance from `p` to the segment `a -> b`.
fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    if l2 <= 1e-24 {
        return p.dist(a);
    }
    let t = ((p - a).dot(d) / l2).clamp(0.0, 1.0);
    p.dist(Point::new(a.x + d.x * t, a.y + d.y * t))
}

/// Distance from each measured point to the fitted run.
///
/// Both sequences run along the same boundary in the same direction, so the match is
/// monotone: a cursor advances through the samples and only a local window is searched.
/// That makes this linear rather than quadratic, which matters because it is evaluated
/// once per candidate tolerance per run.
///
/// The distance is to the *polyline through the samples*, not to the nearest sample. The
/// nearest-sample distance has a floor of up to half the sample spacing — 0.125px at the
/// 0.25px spacing used here — for a point lying exactly on the curve, which at
/// `sigma = 0.05` is a spurious chi² contribution of about two per point, charged to
/// every line and cubic description alike and to nothing that is scored analytically.
/// Measuring to the chord between adjacent samples removes it: the chord's own error is
/// `s² / 8r`, under 0.001px for any radius above 8px.
fn distances(pts: &[Point], samples: &[Point]) -> Vec<f64> {
    let mut out = Vec::with_capacity(pts.len());
    if samples.is_empty() {
        return vec![f64::INFINITY; pts.len()];
    }
    let m = samples.len();
    let mut cursor = 0usize;
    // Window wide enough to absorb non-uniform vertex spacing without going quadratic.
    let window = (m / 8).clamp(16, 512);

    for (k, p) in pts.iter().enumerate() {
        // Expected position of this point along the run.
        let guess = if pts.len() > 1 {
            (k * (m - 1)) / (pts.len() - 1)
        } else {
            0
        };
        let centre = guess.max(cursor.saturating_sub(window / 2));
        let lo = centre.saturating_sub(window);
        let hi = (centre + window).min(m - 1);
        let mut best = f64::INFINITY;
        let mut best_i = cursor;
        for (i, q) in samples[lo..=hi].iter().enumerate() {
            let d = p.dist(*q);
            if d < best {
                best = d;
                best_i = lo + i;
            }
        }
        cursor = best_i;
        let mut d = best;
        if best_i > 0 {
            d = d.min(segment_distance(*p, samples[best_i - 1], samples[best_i]));
        }
        if best_i + 1 < m {
            d = d.min(segment_distance(*p, samples[best_i], samples[best_i + 1]));
        }
        out.push(d);
    }
    out
}

/// Maximum distance from `pts` to a fitted run.
pub fn max_deviation(pts: &[Point], start: Point, segs: &[Segment]) -> f64 {
    if segs.is_empty() || pts.len() < 2 {
        return f64::INFINITY;
    }
    let samples = sample_run(start, segs, 0.25);
    distances(pts, &samples)
        .into_iter()
        .fold(0.0f64, |a, b| if b > a { b } else { a })
}

/// Weighted chi-squared of a fitted run against the measured points.
pub fn chi2(pts: &[Point], sigma: &[f64], start: Point, segs: &[Segment]) -> f64 {
    if segs.is_empty() {
        return f64::INFINITY;
    }
    let samples = sample_run(start, segs, 0.25);
    distances(pts, &samples)
        .into_iter()
        .enumerate()
        .map(|(k, d)| {
            let s = sigma.get(k).copied().unwrap_or(0.5).max(1e-3);
            (d / s) * (d / s)
        })
        .sum()
}

// Silence unused-import warnings from the trait bounds kurbo requires.
#[allow(unused)]
fn _assert_traits(p: &kurbo::CubicBez) {
    let _ = p.eval(0.5);
    let _ = p.deriv();
}
