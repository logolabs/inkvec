//! Primitive fitting: circles, ellipses, circular arcs and rounded rectangles
//! (DESIGN.md §2.8, S4).
//!
//! The cubic alphabet has a description-length floor of its own. A circle is four cubics
//! — 26 parameters with the start point — where `<circle>` is three. Under the MDL
//! objective that gap is worth about 23·λ ≈ 165 nats, which is far more than the fidelity
//! term can ever recover, so the only reason primitives are not chosen is that they are
//! not *offered*. This module offers them, scored by exactly the objective everything
//! else is scored by: `0.5·chi² + λ·params`, with chi² the weighted sum of squared
//! **orthogonal** distances from the measured points to the primitive.
//!
//! Orthogonal, not algebraic. Algebraic circle and conic fits minimize a residual in the
//! wrong units (`x² + y² − r²` rather than distance) and carry a documented bias toward
//! smaller radii on partial arcs — the "high-curvature bias" DESIGN.md S4 warns about.
//! Every fit here is therefore two-stage: Taubin's algebraic fit, whose normalization
//! removes most of the bias, gives a starting point, and Levenberg–Marquardt on the true
//! orthogonal distance (Ahn, Rauh & Warnecke 2001) finishes. The refinement's Jacobian
//! uses the standard envelope argument: the derivative of the distance to a curve with
//! respect to the curve's parameters is the normal component of the contact point's
//! motion, and the contact parameter itself contributes nothing to first order.
//!
//! What is *not* done here: refinement against the image (S4 says candidate primitives
//! should be refined against pixels, not the polyline). The polyline is what this crate
//! receives; that refinement belongs in the analysis-by-synthesis stage.
//!
//! # Where this sits
//!
//! Beside the dynamic program rather than inside it: `inkvec-cli`'s pipeline and
//! `inkvec-svgmin` call [`fit_primitive_or_arcs`] on a whole ring (points in px, sigma in
//! px) and compare its cost with the multimodel fit of the same ring; the cheaper wins.
//! The sub-modules hold the ellipse fitter ([`ellipse`]), the rounded-rectangle fitter
//! ([`round_rect`]) and the small dense solvers both use (`solver`). This file holds the
//! circle fits, the path forms (arcs, four-cubic ellipses, rounded-rectangle outlines)
//! and the entry point.

use crate::curves::{self, Segment};
use crate::{optimal_polygon, FitConfig};
use inkvec_core::{Point, Polyline};
use std::f64::consts::{PI, TAU};

/// `<circle cx cy r>`.
pub const PARAMS_CIRCLE: f64 = 3.0;
/// `<ellipse cx cy rx ry>` plus a rotation.
pub const PARAMS_ELLIPSE: f64 = 5.0;
/// `<rect x y width height rx ry>` — charged as six even though we force `ry = rx`,
/// because the emitted element carries both and a designer editing it sees both.
pub const PARAMS_ROUND_RECT: f64 = 6.0;
/// `<rect x y width height>`, the corner radius having collapsed to zero.
pub const PARAMS_RECT: f64 = 4.0;

/// Longest sweep emitted as a single [`Segment::Arc`], in degrees.
///
/// SVG arcs are endpoint-parametrized, and near 180° that parametrization is badly
/// conditioned: the centre offset from the chord midpoint is `sqrt(r² − h²)` in the
/// half-chord `h`, whose derivative diverges as `h → r`. Shortening the chord of a 40px
/// half-circle by 0.05px — one sigma of extraction noise on an endpoint — moves its
/// centre by 2px. At 120° the same derivative is 0.58, so the arc shape is as stable as
/// its endpoints. The cost is one extra arc on a full circle, and a full circle is
/// emitted as `<circle>` anyway.
pub const MAX_ARC_DEGREES: f64 = 120.0;

/// A primitive that describes a whole closed boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrimitiveKind {
    /// Centre `c` and radius `r`.
    Circle {
        /// Centre of the circle.
        c: Point,
        /// Radius of the circle.
        r: f64,
    },
    /// Semi-axes `rx`, `ry` and the angle of the `rx` axis in radians.
    Ellipse {
        /// Centre of the ellipse.
        c: Point,
        /// Semi-axis along the ellipse's own x-axis.
        rx: f64,
        /// Semi-axis along the ellipse's own y-axis.
        ry: f64,
        /// Rotation of the `rx` axis, in radians.
        angle: f64,
    },
    /// Axis-aligned, equal corner radii. `rx == 0` is a plain rectangle.
    RoundRect {
        /// Left edge.
        x: f64,
        /// Top edge.
        y: f64,
        /// Width.
        w: f64,
        /// Height.
        h: f64,
        /// Corner radius, equal on both axes.
        rx: f64,
    },
}

/// A fitted primitive with its cost components.
#[derive(Debug, Clone, Copy)]
pub struct PrimitiveFit {
    /// The primitive itself, and its parameters.
    pub kind: PrimitiveKind,
    /// Weighted sum of squared orthogonal distances from the measured points.
    pub chi2: f64,
    /// Description length of the primitive, in parameters.
    pub params: f64,
}

impl PrimitiveFit {
    /// `0.5·chi² + λ·params`, the same objective every other description is scored by.
    pub fn cost(&self, cfg: &FitConfig) -> f64 {
        0.5 * self.chi2 + cfg.lambda * self.params
    }
}

pub mod ellipse;
pub mod round_rect;
pub(crate) mod solver;

pub use ellipse::{ellipse_chi2, fit_ellipse, fit_ellipse_algebraic, EllipseFit};
pub use round_rect::{fit_round_rect, round_rect_distance, RoundRectFit};
pub(crate) use solver::{levenberg_marquardt, solve};

/// Inverse-variance weights `1/σ²` for the first `n` points (px⁻²); a missing sigma is
/// taken as 0.5 px and every sigma is floored at 1e-3 px.
pub(crate) fn weights(sigma: &[f64], n: usize) -> Vec<f64> {
    (0..n)
        .map(|k| {
            let s = sigma.get(k).copied().unwrap_or(0.5).max(1e-3);
            1.0 / (s * s)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Circles
// ---------------------------------------------------------------------------------------

/// A fitted circle with its weighted orthogonal-distance chi².
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircleFit {
    /// Centre of the circle.
    pub c: Point,
    /// Radius of the circle.
    pub r: f64,
    /// Weighted sum of squared orthogonal distances from the measured points.
    pub chi2: f64,
}

/// The weight of one sample: inverse variance, with the same floor `weights` applies.
#[inline]
pub(crate) fn weight_at(sigma: &[f64], k: usize) -> f64 {
    let s = sigma.get(k).copied().unwrap_or(0.5).max(1e-3);
    1.0 / (s * s)
}

/// Weighted sum of squared orthogonal distances from `pts` to the circle.
///
/// `χ² = Σ w_k·(|p_k − c| − r)²` with `w_k = 1/σ_k²` (`weights`); dimensionless.
pub fn circle_chi2(pts: &[Point], sigma: &[f64], c: Point, r: f64) -> f64 {
    let w = weights(sigma, pts.len());
    pts.iter()
        .zip(&w)
        .map(|(p, wk)| {
            let d = p.dist(c) - r;
            wk * d * d
        })
        .sum()
}

/// The plain algebraic circle fit (Kåsa): linear least squares on `x² + y² + Bx + Cy + D`.
///
/// Kept as the reference point for what "algebraic-only" costs. Its residual is
/// `|p − c|² − r²`, which weights a point by how far it is from the centre rather than
/// from the circle, and on a partial arc that systematically pulls the fit toward a
/// smaller circle. Not used as an initializer; see [`fit_circle_algebraic`].
pub fn fit_circle_kasa(pts: &[Point], sigma: &[f64]) -> Option<CircleFit> {
    let n = pts.len();
    if n < 3 {
        return None;
    }
    let w = weights(sigma, n);
    let sw: f64 = w.iter().sum();
    let mx = pts.iter().zip(&w).map(|(p, w)| p.x * w).sum::<f64>() / sw;
    let my = pts.iter().zip(&w).map(|(p, w)| p.y * w).sum::<f64>() / sw;
    let mut a = vec![vec![0.0; 3]; 3];
    let mut b = vec![0.0; 3];
    for (p, wk) in pts.iter().zip(&w) {
        let (x, y) = (p.x - mx, p.y - my);
        let row = [x, y, 1.0];
        let rhs = -(x * x + y * y);
        for i in 0..3 {
            b[i] += wk * row[i] * rhs;
            for j in 0..3 {
                a[i][j] += wk * row[i] * row[j];
            }
        }
    }
    let x = solve(a, b)?;
    let (cx, cy) = (-0.5 * x[0], -0.5 * x[1]);
    let r2 = cx * cx + cy * cy - x[2];
    if r2.is_nan() || r2 <= 0.0 {
        return None;
    }
    let c = Point::new(cx + mx, cy + my);
    let r = r2.sqrt();
    Some(CircleFit {
        c,
        r,
        chi2: circle_chi2(pts, sigma, c, r),
    })
}

/// Taubin's algebraic circle fit (Chernov's Newton formulation), weighted by `1/σ²`.
///
/// Minimizes `Σ w (|p−c|² − r²)²` under Taubin's gradient normalization, which is what
/// removes most of the small-arc radius bias of the plain (Kåsa) algebraic fit. It is not
/// bias-free — no algebraic fit is — but it is a good enough start that the geometric
/// refinement converges in a handful of steps. Reported `chi2` is the *orthogonal*
/// residual of this algebraic solution, so it can be compared with [`fit_circle`].
///
/// The steps, all in coordinates centred on the weighted centroid: accumulate the
/// weighted mean moments `Mxx, Myy, Mxy, Mxz, Myz, Mzz` of `(x, y, z = x² + y²)`; form
/// the cubic characteristic polynomial `P(η) = a3·η³ + a2·η² + a1·η + a0` of Taubin's
/// generalised eigenproblem; find its smallest non-negative root by Newton's method from
/// `η = 0` (at most 40 steps, stopping once `|P|` no longer decreases); then
///
/// ```text
///     c = ( (Mxz·(Myy − η) − Myz·Mxy) / 2D,  (Myz·(Mxx − η) − Mxz·Mxy) / 2D ),
///     D = η² − η·Mz + (Mxx·Myy − Mxy²),   r = √(|c|² + Mz),   Mz = Mxx + Myy
/// ```
///
/// `None` for fewer than three points, a vanishing `D` (collinear points) or a
/// non-finite radius.
pub fn fit_circle_algebraic(pts: &[Point], sigma: &[f64]) -> Option<CircleFit> {
    let n = pts.len();
    if n < 3 {
        return None;
    }
    let w = weights(sigma, n);
    let sw: f64 = w.iter().sum();
    let mx = pts.iter().zip(&w).map(|(p, w)| p.x * w).sum::<f64>() / sw;
    let my = pts.iter().zip(&w).map(|(p, w)| p.y * w).sum::<f64>() / sw;

    let (mut mxx, mut myy, mut mxy, mut mxz, mut myz, mut mzz) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, wk) in pts.iter().zip(&w) {
        let (xi, yi) = (p.x - mx, p.y - my);
        let zi = xi * xi + yi * yi;
        mxx += wk * xi * xi;
        myy += wk * yi * yi;
        mxy += wk * xi * yi;
        mxz += wk * xi * zi;
        myz += wk * yi * zi;
        mzz += wk * zi * zi;
    }
    for m in [&mut mxx, &mut myy, &mut mxy, &mut mxz, &mut myz, &mut mzz] {
        *m /= sw;
    }

    let mz = mxx + myy;
    let cov_xy = mxx * myy - mxy * mxy;
    let var_z = mzz - mz * mz;
    let a3 = 4.0 * mz;
    let a2 = -3.0 * mz * mz - mzz;
    let a1 = var_z * mz + 4.0 * cov_xy * mz - mxz * mxz - myz * myz;
    let a0 = mxz * (mxz * myy - myz * mxy) + myz * (myz * mxx - mxz * mxy) - var_z * cov_xy;
    let a22 = a2 + a2;
    let a33 = a3 + a3 + a3;

    // Newton on the characteristic polynomial from x = 0, which Chernov shows converges
    // to the smallest positive root.
    let mut x = 0.0;
    let mut y = a0;
    for _ in 0..40 {
        let dy = a1 + x * (a22 + x * a33);
        if dy.abs() < 1e-300 {
            break;
        }
        let xnew = x - y / dy;
        if xnew == x || !xnew.is_finite() {
            break;
        }
        let ynew = a0 + xnew * (a1 + xnew * (a2 + xnew * a3));
        if ynew.abs() >= y.abs() {
            break;
        }
        x = xnew;
        y = ynew;
    }
    let x = x.max(0.0);
    let det = x * x - x * mz + cov_xy;
    if det.abs() < 1e-300 {
        return None;
    }
    let cx = (mxz * (myy - x) - myz * mxy) / det / 2.0;
    let cy = (myz * (mxx - x) - mxz * mxy) / det / 2.0;
    let r = (cx * cx + cy * cy + mz).sqrt();
    if !r.is_finite() || r <= 0.0 {
        return None;
    }
    let c = Point::new(cx + mx, cy + my);
    Some(CircleFit {
        c,
        r,
        chi2: circle_chi2(pts, sigma, c, r),
    })
}

/// Orthogonal-distance circle fit: Taubin initialization, Levenberg–Marquardt on the
/// true geometric residual `(|p − c| − r) / σ`.
pub fn fit_circle(pts: &[Point], sigma: &[f64]) -> Option<CircleFit> {
    let init = fit_circle_algebraic(pts, sigma)?;
    refine_circle(pts, sigma, init)
}

/// Levenberg–Marquardt on the orthogonal residuals `e_k = |p_k − c| − r`, weighted by
/// `1/σ²`, from `init`.
///
/// The Jacobian of `e_k` with respect to `(cx, cy, r)` is `(−(p_k − c)/|p_k − c|, −1)`;
/// a point sitting on the centre has no defined direction and is skipped. The radius is
/// kept positive (at least 1e-6 px) after each step, and at most 100 iterations run (see
/// `solver::levenberg_marquardt`). Returns whichever of the refined and initial circles
/// has the smaller χ².
fn refine_circle(pts: &[Point], sigma: &[f64], init: CircleFit) -> Option<CircleFit> {
    let w = weights(sigma, pts.len());
    let eval = |p: &[f64]| -> Option<(f64, Vec<Vec<f64>>, Vec<f64>)> {
        let (cx, cy, r) = (p[0], p[1], p[2]);
        let mut jtj = vec![vec![0.0; 3]; 3];
        let mut jtr = vec![0.0; 3];
        let mut chi2 = 0.0;
        for (pt, wk) in pts.iter().zip(&w) {
            let (dx, dy) = (pt.x - cx, pt.y - cy);
            let d = dx.hypot(dy);
            if d < 1e-12 {
                continue;
            }
            let res = d - r;
            let j = [-dx / d, -dy / d, -1.0];
            chi2 += wk * res * res;
            for a in 0..3 {
                jtr[a] += wk * j[a] * res;
                for b in 0..3 {
                    jtj[a][b] += wk * j[a] * j[b];
                }
            }
        }
        Some((chi2, jtj, jtr))
    };
    let project = |p: &mut [f64]| {
        p[2] = p[2].abs().max(1e-6);
    };
    let (p, chi2) = levenberg_marquardt(vec![init.c.x, init.c.y, init.r], 100, eval, project)?;
    let out = CircleFit {
        c: Point::new(p[0], p[1]),
        r: p[2],
        chi2,
    };
    // Refinement can only be accepted if it did not make things worse — it cannot, by
    // construction, but a degenerate input can make the LM loop exit on its first step.
    Some(if out.chi2 <= init.chi2 { out } else { init })
}

// ---------------------------------------------------------------------------------------
// Arcs and path forms
// ---------------------------------------------------------------------------------------

/// Total signed angle swept about `c` by the run, with the closing step included when
/// `closed`. Positive means increasing angle (SVG `sweep = 1`).
fn total_sweep(pts: &[Point], c: Point, closed: bool) -> f64 {
    let mut total = 0.0;
    let mut prev = (pts[0] - c).angle();
    let n = pts.len();
    let last = if closed { n } else { n - 1 };
    for k in 1..=last {
        let a = (pts[k % n] - c).angle();
        let mut d = a - prev;
        if d > PI {
            d -= TAU;
        } else if d < -PI {
            d += TAU;
        }
        total += d;
        prev = a;
    }
    total
}

/// Emit the arc of `circle` from angle `a0` sweeping `delta`, starting at the caller's
/// current point and ending exactly at `end`, split into pieces of at most
/// [`MAX_ARC_DEGREES`]. Intermediate split points lie exactly on the circle.
///
/// The piece count is `ceil(|delta| / MAX_ARC_DEGREES)` (at least one; the `1e-9` keeps
/// an exact multiple from rounding up), each piece sweeping `delta / pieces`. Angles in
/// radians, `c` and `r` in px.
fn arc_segments(c: Point, r: f64, a0: f64, delta: f64, end: Point) -> Vec<Segment> {
    let pieces = ((delta.abs() / MAX_ARC_DEGREES.to_radians()) - 1e-9)
        .ceil()
        .max(1.0) as usize;
    let sweep = delta > 0.0;
    (1..=pieces)
        .map(|i| {
            let e = if i == pieces {
                end
            } else {
                let a = a0 + delta * i as f64 / pieces as f64;
                Point::new(c.x + r * a.cos(), c.y + r * a.sin())
            };
            Segment::circular_arc(r, (delta.abs() / pieces as f64) > PI, sweep, e)
        })
        .collect()
}

/// Reduced chi² above which a primitive is not offered at all, regardless of how it
/// costs against lines. `tau²` with the default `tau = 2`: an rms residual beyond two
/// sigma is a model the measurement rejects, not a description of it.
const MAX_REDUCED_CHI2: f64 = 4.0;

/// Fit the run with circular arc segments, if a circle explains it.
///
/// The run is emitted from its first measured point to its last (or back to its first
/// when `closed`), so it joins its neighbours exactly as a line or cubic run would. The
/// arcs' radius and interior split points come from the orthogonal-distance circle fit;
/// the endpoints are the measurements, and the small discrepancy between a measured
/// endpoint and the fitted circle is absorbed by SVG's centre reconstruction, which is
/// well conditioned at the sweep this function emits (see [`MAX_ARC_DEGREES`]).
///
/// `None` when there are too few points, the run is straight enough that a circle is
/// meaningless, or the reduced chi² of the fit exceeds `MAX_REDUCED_CHI2`.
pub fn fit_arcs(pts: &[Point], sigma: &[f64], closed: bool) -> Option<Vec<Segment>> {
    fit_arcs_inner(pts, sigma, closed).map(|(segs, _)| segs)
}

/// [`fit_arcs`], also returning the circle the arcs lie on.
///
/// Refuses (with `None`) fewer than four points, a fit whose χ² exceeds
/// `MAX_REDUCED_CHI2·n`, a radius over a thousand times the run's extent (a straight run
/// in disguise) and a total sweep under 1e-3 rad.
fn fit_arcs_inner(pts: &[Point], sigma: &[f64], closed: bool) -> Option<(Vec<Segment>, CircleFit)> {
    if pts.len() < 4 {
        return None;
    }
    arcs_on_circle(pts, closed, fit_circle(pts, sigma)?)
}

/// [`fit_arcs_inner`] on a circle already fitted to `pts` ([`fit_circle`]), so that
/// [`fit_primitive_or_arcs`] can fit the circle once and hand the same fit to the ellipse
/// search as its near-circle starts.
fn arcs_on_circle(pts: &[Point], closed: bool, cf: CircleFit) -> Option<(Vec<Segment>, CircleFit)> {
    let n = pts.len();
    if cf.chi2 > MAX_REDUCED_CHI2 * n as f64 {
        return None;
    }
    // Straight runs fit a huge circle perfectly; that is a line's job.
    let span = pts.iter().map(|p| p.dist(pts[0])).fold(0.0f64, f64::max);
    if !cf.r.is_finite() || cf.r > 1e3 * span.max(1.0) {
        return None;
    }
    let delta = total_sweep(pts, cf.c, closed);
    if delta.abs() < 1e-3 {
        return None;
    }
    let a0 = (pts[0] - cf.c).angle();
    let end = if closed { pts[0] } else { pts[n - 1] };
    let end = if closed && pts[n - 1].dist(pts[0]) < 1e-9 {
        pts[n - 1]
    } else {
        end
    };
    Some((arc_segments(cf.c, cf.r, a0, delta, end), cf))
}

/// Cubic approximation of the ellipse, four pieces from the point nearest `start`,
/// traversed in the direction of `sign`, ending exactly at `end`.
///
/// Each piece spans a quarter of the parametric angle, `Δ = ±π/2`, and uses the standard
/// circular-arc construction carried through the ellipse's affine map: the control points
/// are `P(t1) + α·P'(t1)` and `P(t2) − α·P'(t2)` with `α = 4/3·tan(|Δ|/4)` and `P'` the
/// derivative with respect to the parametric angle. The error of that construction is
/// about 2.7e-4 of the radius per quarter. The last piece is pinned to `end`.
fn ellipse_cubics(e: &EllipseFit, start: Point, sign: f64, end: Point) -> Vec<Segment> {
    let (_, t0, _) = e.contact(start);
    let step = sign * PI / 2.0;
    let alpha = 4.0 / 3.0 * (step.abs() / 4.0).tan();
    (0..4)
        .map(|i| {
            let (t1, t2) = (t0 + step * i as f64, t0 + step * (i + 1) as f64);
            let (p0, p3) = (e.at(t1), if i == 3 { end } else { e.at(t2) });
            let (d1, d2) = (e.deriv(t1), e.deriv(t2));
            let c1 = Point::new(p0.x + sign * alpha * d1.x, p0.y + sign * alpha * d1.y);
            let c2 = Point::new(p3.x - sign * alpha * d2.x, p3.y - sign * alpha * d2.y);
            Segment::Cubic(c1, c2, p3)
        })
        .collect()
}

/// One piece of a rounded rectangle's outline, in increasing-angle order.
#[derive(Clone, Copy)]
enum Piece {
    /// A straight side, from the first point to the second.
    Line(Point, Point),
    /// Centre, radius, start angle; always a quarter turn of increasing angle.
    Arc(Point, f64, f64),
}

impl Piece {
    /// Length of the piece, px.
    fn len(&self) -> f64 {
        match *self {
            Piece::Line(a, b) => a.dist(b),
            Piece::Arc(_, r, _) => r * PI / 2.0,
        }
    }
    /// Parameter in `[0, 1]` of the point on this piece nearest `p`, and its distance.
    fn nearest(&self, p: Point) -> (f64, f64) {
        match *self {
            Piece::Line(a, b) => {
                let d = b - a;
                let l2 = d.dot(d);
                let u = if l2 > 0.0 {
                    ((p - a).dot(d) / l2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let q = Point::new(a.x + d.x * u, a.y + d.y * u);
                (u, p.dist(q))
            }
            Piece::Arc(c, r, a) => {
                let mut rel = (p - c).angle() - a;
                while rel < 0.0 {
                    rel += TAU;
                }
                while rel >= TAU {
                    rel -= TAU;
                }
                // Beyond the quarter turn, the nearer endpoint wins.
                let u = if rel <= PI / 2.0 {
                    rel / (PI / 2.0)
                } else if rel < PI / 2.0 + 3.0 * PI / 4.0 {
                    1.0
                } else {
                    0.0
                };
                let ang = a + u * PI / 2.0;
                let q = Point::new(c.x + r * ang.cos(), c.y + r * ang.sin());
                (u, p.dist(q))
            }
        }
    }
    /// The sub-piece from parameter `u0` to `u1`, as a segment ending at `end_override`
    /// (or the sub-piece's own end). `u1 < u0` traverses backwards.
    fn segment(&self, u0: f64, u1: f64, end_override: Option<Point>) -> Segment {
        match *self {
            Piece::Line(a, b) => {
                let e = end_override
                    .unwrap_or_else(|| Point::new(a.x + (b.x - a.x) * u1, a.y + (b.y - a.y) * u1));
                Segment::Line(e)
            }
            Piece::Arc(c, r, a) => {
                let ang = a + u1 * PI / 2.0;
                let e = end_override
                    .unwrap_or_else(|| Point::new(c.x + r * ang.cos(), c.y + r * ang.sin()));
                Segment::circular_arc(r, false, u1 > u0, e)
            }
        }
    }
}

/// The eight pieces of the outline, in increasing-angle order, zero-length ones dropped.
fn round_rect_pieces(rr: &RoundRectFit) -> Vec<Piece> {
    let (x0, y0, x1, y1, r) = (rr.x, rr.y, rr.x + rr.w, rr.y + rr.h, rr.rx);
    let pieces = [
        Piece::Line(Point::new(x1, y0 + r), Point::new(x1, y1 - r)),
        Piece::Arc(Point::new(x1 - r, y1 - r), r, 0.0),
        Piece::Line(Point::new(x1 - r, y1), Point::new(x0 + r, y1)),
        Piece::Arc(Point::new(x0 + r, y1 - r), r, PI / 2.0),
        Piece::Line(Point::new(x0, y1 - r), Point::new(x0, y0 + r)),
        Piece::Arc(Point::new(x0 + r, y0 + r), r, PI),
        Piece::Line(Point::new(x0 + r, y0), Point::new(x1 - r, y0)),
        Piece::Arc(Point::new(x1 - r, y0 + r), r, 3.0 * PI / 2.0),
    ];
    pieces.into_iter().filter(|p| p.len() > 1e-9).collect()
}

/// Path form of a rounded rectangle from the outline point nearest `start`, traversed in
/// the direction of `sign`, ending exactly at `end`.
///
/// The outline is the eight pieces of [`round_rect_pieces`] (four for a plain
/// rectangle). The path runs from the nearest point on the nearest piece to that piece's
/// end, through every other piece in turn, and back along the first piece to `end`; a
/// closing segment of zero length is folded into the one before it. Empty for a
/// degenerate rectangle with no pieces.
fn round_rect_segments(rr: &RoundRectFit, start: Point, sign: f64, end: Point) -> Vec<Segment> {
    let pieces = round_rect_pieces(rr);
    if pieces.is_empty() {
        return Vec::new();
    }
    let m = pieces.len();
    let (k0, u0) = pieces
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let (u, d) = p.nearest(start);
            (k, u, d)
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(k, u, _)| (k, u))
        .expect("pieces is non-empty: the empty case returned above");
    let forward = sign >= 0.0;
    let mut out = Vec::with_capacity(m + 1);
    // Rest of the starting piece, then every other piece, then the starting piece back
    // to the start point.
    let (first_to, last_from) = if forward { (1.0, 0.0) } else { (0.0, 1.0) };
    if (u0 - first_to).abs() > 1e-9 {
        out.push(pieces[k0].segment(u0, first_to, None));
    }
    for i in 1..m {
        let k = if forward {
            (k0 + i) % m
        } else {
            (k0 + m - i) % m
        };
        let (a, b) = if forward { (0.0, 1.0) } else { (1.0, 0.0) };
        out.push(pieces[k].segment(a, b, None));
    }
    out.push(pieces[k0].segment(last_from, u0, Some(end)));
    // Drop a zero-length closing segment if the start sat exactly on a piece boundary.
    if (u0 - last_from).abs() <= 1e-9 && out.len() > 1 {
        out.pop();
        if let Some(last) = out.last_mut() {
            *last = match last.clone() {
                Segment::Line(_) => Segment::Line(end),
                Segment::Cubic(a, b, _) => Segment::Cubic(a, b, end),
                Segment::Arc {
                    rx,
                    ry,
                    phi,
                    large_arc,
                    sweep,
                    ..
                } => Segment::Arc {
                    rx,
                    ry,
                    phi,
                    large_arc,
                    sweep,
                    end,
                },
            };
        }
    }
    out
}

/// Signed area of the closed polygon; positive when the angle about the interior increases.
///
/// The shoelace formula `½·Σ (x_i·y_{i+1} − x_{i+1}·y_i)`, px², wrapping from the last
/// point to the first.
fn signed_area(pts: &[Point]) -> f64 {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        * 0.5
}

// ---------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------

/// The cheapest primitive-or-arc description of a run under the MDL objective.
///
/// Returns `(segments, primitive, cost)`:
///
/// * `segments` — a path form of the description, beginning at `pts[0]` (the caller
///   supplies that start, as for any run) and ending at the run's last point, or back at
///   `pts[0]` when `closed`. Circles and arcs become [`Segment::Arc`]s; an ellipse
///   becomes four cubics; a rounded rectangle becomes lines and quarter arcs.
/// * `primitive` — `Some` only for a closed run described by a whole `<circle>`,
///   `<ellipse>` or `<rect>`, so the emitter can write the element instead of a path.
/// * `cost` — `0.5·chi² + λ·params` of what will be emitted: the primitive's own
///   parameter count and orthogonal-distance chi² when `primitive` is `Some`, otherwise
///   the arc path's parameters (excluding the shared start point, as `fit_path` costs a
///   run) and its sampled chi².
///
/// `None` when no candidate is both statistically acceptable (reduced chi² within
/// `tau²`) and cheaper than describing the run with straight segments alone — the
/// line-only optimum from [`optimal_polygon`], which is the "not fitting" baseline this
/// function can compute for itself. The caller compares the returned cost against its
/// cubic description; both are in the same units.
///
/// # Measured: this is the largest parameter lever in the tree, and the comparison is
/// # not the weak point DESIGN.md §S4 expected
///
/// `docs/algorithm/00-overview.md` flags the caller's structure — one whole-boundary
/// primitive fit scored against one DP fit, rather than primitives as states inside the
/// DP — as "a real architectural gap", on DESIGN.md's reasoning that "once cubics are
/// fitted they have already absorbed the error a primitive would have explained", so
/// primitives would lose contests they deserve to win. Counted over the 246-icon gate
/// set (6898 boundaries): a candidate is offered on 606 of them and **wins 595, or 98.2%
/// of offers**. Eleven lose, and nine of those lose by more than 5% (median loser costs
/// 1.28x the DP). There is no population of near-misses for a unified DP to rescue, so
/// whatever that gap costs, it is not this.
///
/// Winners: 386 rounded rects, 107 circles, 69 ellipses, 33 arc chains. Only 16.4% of
/// boundaries are closed at all, and a whole-shape primitive needs a closed one, so the
/// offer rate among eligible boundaries is 53.5%.
///
/// Ablating the path entirely (the `INKVEC_NO_PRIMITIVE` switch, since removed) costs **30.52% of the parameter
/// ratio** (1.4818 -> 1.9341) and 9.01% of dE00, with turning unchanged (-0.32%). For
/// scale, every other parameter lever measured on this tree moves the ratio by 1-2%.
///
/// What the count cannot see: a boundary that is only *partly* a primitive is never
/// offered one, and shows up here only as a silent non-offer. That residue is the real
/// remainder of the S4 gap — though circular arcs, the common case, are already states
/// in the DP alphabet ([`crate::multimodel`] fits `{line, cubic, arc}`, not the
/// `{line, cubic}` the overview describes), which bounds how much of it is left.
pub fn fit_primitive_or_arcs(
    pts: &[Point],
    sigma: &[f64],
    closed: bool,
    cfg: &FitConfig,
) -> Option<(Vec<Segment>, Option<PrimitiveFit>, f64)> {
    let n = pts.len();
    if n < 4 || sigma.len() < n {
        return None;
    }
    let sigma = &sigma[..n];
    let run = PrimRun {
        pts,
        sigma,
        closed,
        cfg,
        gate: cfg.tau * cfg.tau * n as f64,
        start: pts[0],
        end: if closed && pts[n - 1].dist(pts[0]) >= 1e-9 {
            pts[0]
        } else {
            pts[n - 1]
        },
    };

    // One orthogonal circle fit serves both the arcs and the ellipse search's starts,
    // which used to fit the same circle to the same points a second time.
    let circle = fit_circle(pts, sigma);
    let mut best = Cheapest::new(pts);
    offer_arcs(&run, circle, &mut best);
    if closed {
        let sign = if signed_area(pts) >= 0.0 { 1.0 } else { -1.0 };
        offer_ellipse(&run, circle, sign, &mut best);
        offer_round_rects(&run, sign, &mut best);
    }
    let best = best.best?;

    // Not fitting: the line-only optimum over the same points.
    let poly = Polyline::new(pts.to_vec(), sigma.to_vec(), closed);
    let baseline = optimal_polygon(&poly, cfg).cost;
    if best.2 < baseline {
        Some(best)
    } else {
        None
    }
}

/// The run [`fit_primitive_or_arcs`] is describing, and what every candidate is judged
/// against.
struct PrimRun<'a> {
    /// The measured points, px.
    pts: &'a [Point],
    /// Their uncertainties, px; exactly as many as `pts`.
    sigma: &'a [f64],
    closed: bool,
    cfg: &'a FitConfig,
    /// The acceptance gate on a whole primitive's χ²: `τ²·n`, a reduced χ² of `τ²`.
    gate: f64,
    /// Where the path form starts: the first point.
    start: Point,
    /// Where it ends: the last point, or the first again for a closed run whose last
    /// point is not already a repeat of it.
    end: Point,
}

/// A candidate description: path form, the whole-ring primitive if it is one, cost.
type Candidate = (Vec<Segment>, Option<PrimitiveFit>, f64);

/// The cheapest sane candidate offered so far.
///
/// No candidate may place geometry far outside the points it claims to describe. Each
/// fitter has its own conditioning guards; this is the one place that catches a
/// divergence none of them anticipated, before it reaches an emitter or a renderer. The
/// allowed box is the points' bounding box grown by eight times its larger side (at
/// least 8 px) on every side; an arc's radii must also stay under that margin.
struct Cheapest {
    lo_x: f64,
    lo_y: f64,
    hi_x: f64,
    hi_y: f64,
    slack: f64,
    best: Option<Candidate>,
}

impl Cheapest {
    /// An empty pool, with the bounds taken from `pts`.
    fn new(pts: &[Point]) -> Self {
        let (mut lo_x, mut lo_y, mut hi_x, mut hi_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in pts {
            lo_x = lo_x.min(p.x);
            lo_y = lo_y.min(p.y);
            hi_x = hi_x.max(p.x);
            hi_y = hi_y.max(p.y);
        }
        let slack = 8.0 * (hi_x - lo_x).max(hi_y - lo_y).max(1.0);
        Cheapest {
            lo_x,
            lo_y,
            hi_x,
            hi_y,
            slack,
            best: None,
        }
    }

    /// Whether `p` is finite and inside the allowed box.
    fn in_bounds(&self, p: Point) -> bool {
        p.x.is_finite()
            && p.y.is_finite()
            && p.x >= self.lo_x - self.slack
            && p.x <= self.hi_x + self.slack
            && p.y >= self.lo_y - self.slack
            && p.y <= self.hi_y + self.slack
    }

    /// Keep this candidate if it is non-empty, finite, inside the box and strictly
    /// cheaper than the best so far (ties keep the earlier one).
    fn consider(&mut self, segs: Vec<Segment>, prim: Option<PrimitiveFit>, cost: f64) {
        if segs.is_empty() || !cost.is_finite() {
            return;
        }
        let sane = segs.iter().all(|s| match *s {
            Segment::Line(p) => self.in_bounds(p),
            Segment::Cubic(a, b, p) => self.in_bounds(a) && self.in_bounds(b) && self.in_bounds(p),
            Segment::Arc { rx, ry, end, .. } => {
                rx.is_finite() && ry.is_finite() && rx.max(ry) <= self.slack && self.in_bounds(end)
            }
        });
        if !sane {
            return;
        }
        match self.best {
            Some((_, _, c)) if c <= cost => {}
            _ => self.best = Some((segs, prim, cost)),
        }
    }
}

/// Offer the circular-arc description: the only candidate for an open run, costed as a
/// path (`½·χ²` sampled against the arcs, plus the arcs' parameters), and the path form
/// of a whole `<circle>` for a closed run that goes more than 1.9π round the centre and
/// passes the χ² gate, costed as the circle's three parameters.
fn offer_arcs(run: &PrimRun<'_>, circle: Option<CircleFit>, best: &mut Cheapest) {
    let (pts, sigma, cfg) = (run.pts, run.sigma, run.cfg);
    let arcs = circle.and_then(|cf| arcs_on_circle(pts, run.closed, cf));
    if let Some((segs, cf)) = &arcs {
        if !run.closed {
            let chi2 = curves::chi2(pts, sigma, run.start, segs);
            let params: f64 = segs.iter().map(|s| s.params()).sum();
            best.consider(segs.clone(), None, 0.5 * chi2 + cfg.lambda * params);
        } else {
            let sweep = total_sweep(pts, cf.c, true);
            if sweep.abs() > 1.9 * PI && cf.chi2 <= run.gate {
                let prim = PrimitiveFit {
                    kind: PrimitiveKind::Circle { c: cf.c, r: cf.r },
                    chi2: cf.chi2,
                    params: PARAMS_CIRCLE,
                };
                best.consider(segs.clone(), Some(prim), prim.cost(cfg));
            }
        }
    }
}

/// Offer a whole `<ellipse>` for a closed run: orthogonal-distance fit, sane radii,
/// the χ² gate, and more than 1.9π of sweep round its centre. `sign` is the run's
/// orientation (+1 when its signed area is positive), which the four-cubic path form
/// follows.
fn offer_ellipse(run: &PrimRun<'_>, circle: Option<CircleFit>, sign: f64, best: &mut Cheapest) {
    let pts = run.pts;
    // The same bound the circle fit applies (see `fit_arcs_inner`), which the
    // ellipse lacked. A thin sliver is described *well* by an absurdly eccentric
    // ellipse - two nearly parallel edges are a good local fit to one - so chi2 is
    // small, the sweep around a centre inside the sliver is a full turn, and every
    // gate passes while `rx` runs to 1e9. The cubics that approximate it then carry
    // control points a billion units away, which is not a shape any renderer is
    // obliged to survive: tiny-skia panics on it.
    //
    // Found when blend absorption started producing slivers this fitter had never
    // been offered before. The defect was always here; nothing upstream had reached
    // it.
    let span = pts
        .iter()
        .map(|p| p.dist(pts[0]))
        .fold(0.0f64, f64::max)
        .max(1.0);
    if let Some(e) = ellipse::fit_ellipse_from(pts, run.sigma, circle) {
        let sane = e.rx.is_finite()
            && e.ry.is_finite()
            && e.c.x.is_finite()
            && e.c.y.is_finite()
            && e.rx.max(e.ry) <= 1e3 * span;
        if sane && e.chi2 <= run.gate && total_sweep(pts, e.c, true).abs() > 1.9 * PI {
            let prim = PrimitiveFit {
                kind: PrimitiveKind::Ellipse {
                    c: e.c,
                    rx: e.rx,
                    ry: e.ry,
                    angle: e.angle,
                },
                chi2: e.chi2,
                params: PARAMS_ELLIPSE,
            };
            best.consider(
                ellipse_cubics(&e, run.start, sign, run.end),
                Some(prim),
                prim.cost(run.cfg),
            );
        }
    }
}

/// Offer the rounded rectangle, and the plain rectangle it collapses to, for a closed
/// run. Both are costed and the objective picks; a corner radius the noise cannot
/// resolve is not worth two parameters. Each must pass the χ² gate and have a positive
/// width and height.
fn offer_round_rects(run: &PrimRun<'_>, sign: f64, best: &mut Cheapest) {
    let rr_free = fit_round_rect(run.pts, run.sigma, None);
    let rr_zero = fit_round_rect(run.pts, run.sigma, Some(0.0));
    for (rr, params) in [(rr_free, PARAMS_ROUND_RECT), (rr_zero, PARAMS_RECT)] {
        let Some(rr) = rr else { continue };
        if rr.chi2 > run.gate || rr.w <= 0.0 || rr.h <= 0.0 {
            continue;
        }
        let prim = PrimitiveFit {
            kind: PrimitiveKind::RoundRect {
                x: rr.x,
                y: rr.y,
                w: rr.w,
                h: rr.h,
                rx: rr.rx,
            },
            chi2: rr.chi2,
            params,
        };
        let segs = round_rect_segments(&rr, run.start, sign, run.end);
        best.consider(segs, Some(prim), prim.cost(run.cfg));
    }
}
