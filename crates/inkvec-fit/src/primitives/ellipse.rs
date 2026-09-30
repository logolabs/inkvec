//! Ellipse geometry and fitting (Taubin algebraic + orthogonal Levenberg-Marquardt).
//!
//! Used by the whole-ring primitive search (`super::fit_primitive_or_arcs`, through
//! [`fit_ellipse`]) and by the dynamic program's elliptical-arc candidate
//! (`crate::candidates::try_ellipse`, through [`fit_ellipse_algebraic`] alone, which is
//! cheap enough to run per span). Points and radii are in px, angles in radians.
//!
//! An ellipse here is `c + R(angle)·(rx·cos t, ry·sin t)` for the parametric angle `t`,
//! with `R` the rotation matrix. After fitting, `rx ≥ ry` and `angle ∈ (−π/2, π/2]`.

use inkvec_core::{Point, Vec2};
use std::f64::consts::PI;

use super::solver::{gen_eigen_5, levenberg_marquardt};
use super::{fit_circle, weight_at, weights};

/// A fitted ellipse with its weighted orthogonal-distance chi².
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EllipseFit {
    /// Centre of the ellipse.
    pub c: Point,
    /// Semi-axis along the ellipse's own x-axis.
    pub rx: f64,
    /// Semi-axis along the ellipse's own y-axis.
    pub ry: f64,
    /// Rotation of the `rx` axis, radians, in `(-π/2, π/2]`.
    pub angle: f64,
    /// Weighted sum of squared orthogonal distances from the measured points.
    pub chi2: f64,
}

impl EllipseFit {
    /// Point at parametric angle `t`.
    pub fn at(&self, t: f64) -> Point {
        let (s, c) = self.angle.sin_cos();
        let (x, y) = (self.rx * t.cos(), self.ry * t.sin());
        Point::new(self.c.x + c * x - s * y, self.c.y + s * x + c * y)
    }

    /// Derivative with respect to `t`: `R(angle)·(−rx·sin t, ry·cos t)`, px per radian.
    pub(crate) fn deriv(&self, t: f64) -> Vec2 {
        let (s, c) = self.angle.sin_cos();
        let (x, y) = (-self.rx * t.sin(), self.ry * t.cos());
        Vec2 {
            x: c * x - s * y,
            y: s * x + c * y,
        }
    }

    /// Orthogonal contact: `(signed distance, parametric angle t, unit outward normal
    /// in the ellipse frame)`. Positive distance is outside.
    ///
    /// In the ellipse's own frame the point is `q` and the curve `E(t) = (rx cos t,
    /// ry sin t)`. The foot of the perpendicular satisfies `(q − E(t))·E'(t) = 0`, i.e.
    ///
    /// ```text
    ///     f(t) = −rx·qx·sin t + ry·qy·cos t + (rx² − ry²)·sin t·cos t = 0
    /// ```
    ///
    /// solved by Newton's method from `t = atan2(rx·qy, ry·qx)` (exact on a circle), with
    /// steps clamped to ±0.5 rad and at most 12 iterations. The normal at `E(t)` is
    /// `(ry cos t, rx sin t)` normalised, and the distance is `(q − E(t))·n`. Near the
    /// centre of an eccentric ellipse `f` has several roots and Newton may settle on a
    /// local rather than the nearest foot; fits never have their points there.
    pub fn contact(&self, p: Point) -> (f64, f64, Vec2) {
        let (s, c) = self.angle.sin_cos();
        let (dx, dy) = (p.x - self.c.x, p.y - self.c.y);
        let q = Vec2 {
            x: c * dx + s * dy,
            y: -s * dx + c * dy,
        };
        let (rx, ry) = (self.rx, self.ry);
        let mut t = (rx * q.y).atan2(ry * q.x);
        let k = rx * rx - ry * ry;
        for _ in 0..12 {
            let (st, ct) = t.sin_cos();
            let f = -rx * q.x * st + ry * q.y * ct + k * st * ct;
            let df = -rx * q.x * ct - ry * q.y * st + k * (ct * ct - st * st);
            if df.abs() < 1e-300 {
                break;
            }
            let step = f / df;
            let step = step.clamp(-0.5, 0.5);
            t -= step;
            if step.abs() < 1e-13 {
                break;
            }
        }
        let (st, ct) = t.sin_cos();
        let ex = Vec2 {
            x: rx * ct,
            y: ry * st,
        };
        let mut n = Vec2 {
            x: ry * ct,
            y: rx * st,
        };
        let nn = n.norm();
        if nn < 1e-300 {
            n = Vec2 { x: 1.0, y: 0.0 };
        } else {
            n = Vec2 {
                x: n.x / nn,
                y: n.y / nn,
            };
        }
        let d = (q.x - ex.x) * n.x + (q.y - ex.y) * n.y;
        (d, t, n)
    }
}

/// Weighted sum of squared orthogonal distances from `pts` to the ellipse.
///
/// `χ² = Σ w_k·d_k²`, `d_k` from [`EllipseFit::contact`] and `w_k = 1/σ_k²`
/// (`super::weights`); dimensionless.
pub fn ellipse_chi2(pts: &[Point], sigma: &[f64], e: &EllipseFit) -> f64 {
    let w = weights(sigma, pts.len());
    pts.iter()
        .zip(&w)
        .map(|(p, wk)| {
            let d = e.contact(*p).0;
            wk * d * d
        })
        .sum()
}

/// Geometry of the conic `a x² + b xy + c y² + d x + e y + f = 0`, if it is an ellipse.
///
/// Returns `(centre, r1, r2, angle)`, with `r1` the semi-axis along `angle` and `r2` the
/// one across it (either may be the larger). The centre is where the gradient vanishes,
///
/// ```text
///     x0 = (b·e − 2c·d) / (4ac − b²),     y0 = (b·d − 2a·e) / (4ac − b²),
/// ```
///
/// `f0 = f + ½(d·x0 + e·y0)` is the conic's value there, the axes are rotated by
/// `angle = ½·atan2(b, a − c)`, and with `l1`, `l2` the quadratic form's values along the
/// two axes the semi-axes are `√(−f0/l1)` and `√(−f0/l2)`. `None` unless
/// `b² − 4ac < 0` (an ellipse, not a parabola or hyperbola), `f0 ≠ 0` and both
/// `−f0/l` are positive (a real, non-empty ellipse).
pub(crate) fn conic_to_ellipse(k: [f64; 6]) -> Option<(Point, f64, f64, f64)> {
    let [a, b, c, d, e, f] = k;
    let disc = b * b - 4.0 * a * c;
    if disc >= 0.0 {
        return None;
    }
    let det = 4.0 * a * c - b * b;
    let x0 = (b * e - 2.0 * c * d) / det;
    let y0 = (b * d - 2.0 * a * e) / det;
    let f0 = f + 0.5 * (d * x0 + e * y0);
    if f0 == 0.0 {
        return None;
    }
    let angle = 0.5 * b.atan2(a - c);
    let (s, cs) = angle.sin_cos();
    let l1 = a * cs * cs + b * cs * s + c * s * s;
    let l2 = a * s * s - b * cs * s + c * cs * cs;
    let (r1, r2) = (-f0 / l1, -f0 / l2);
    if r1 <= 0.0 || r2 <= 0.0 {
        return None;
    }
    Some((Point::new(x0, y0), r1.sqrt(), r2.sqrt(), angle))
}

/// Normalize an ellipse axis angle into the range `(-π/2, π/2]`.
pub(crate) fn canonical_angle(mut angle: f64) -> f64 {
    while angle > PI / 2.0 {
        angle -= PI;
    }
    while angle <= -PI / 2.0 {
        angle += PI;
    }
    angle
}

/// Taubin's algebraic conic fit, weighted by `1/σ²`, returned only if it is an ellipse.
///
/// The points are first centred on their weighted centroid and scaled by their RMS
/// distance from it, so the conic's coefficients are of order one. Each point then gives
/// the row `z = (u², u·v, v², u, v)`; the constant term is eliminated by centring the rows
/// on their weighted mean `z̄`. Taubin's fit minimises the algebraic residual
/// `θᵀ·C·θ`, `C = Σ w (z − z̄)(z − z̄)ᵀ`, subject to `θᵀ·N·θ = 1` with
/// `N = Σ w (∂z/∂u ∂z/∂uᵀ + ∂z/∂v ∂z/∂vᵀ)`, the gradient normalisation that makes the
/// residual approximate a squared distance and removes most of the plain algebraic fit's
/// bias. That is the generalised eigenproblem `C·θ = μ·N·θ` for the smallest `μ`
/// (`solver::gen_eigen_5`); `f = −z̄·θ`. The conic is converted with
/// `conic_to_ellipse`, scaled back to px, and given `rx ≥ ry`.
///
/// `None` for fewer than six points, zero weight, coincident points or a conic that is
/// not an ellipse. `chi2` is the orthogonal residual of this algebraic solution.
pub fn fit_ellipse_algebraic(pts: &[Point], sigma: &[f64]) -> Option<EllipseFit> {
    let mut e = taubin_ellipse(pts, sigma)?;
    e.chi2 = ellipse_chi2(pts, sigma, &e);
    Some(e)
}

/// [`fit_ellipse_algebraic`] without its orthogonal χ²: the conic's geometry only, with
/// `chi2` left at infinity ("not scored"), the value the near-circle starts of
/// [`fit_ellipse`] carry too.
///
/// The two internal callers never read that χ². The dynamic program's elliptical-arc
/// candidate (`crate::candidates::try_ellipse`) rebuilds the arc through its end points
/// and scores it by the Sampson distance, and [`fit_ellipse`] hands the algebraic fit to
/// Levenberg–Marquardt as a start, which reads only the centre, radii and angle. The χ²
/// costs a Newton foot-point solve per point (`EllipseFit::contact`), and measured on a
/// 512 px flat logo and a 2048 px icon it was 54% and 63% of the algebraic fit's time
/// (7.6% and 9.1% of the whole trace's CPU), all of it discarded. The geometry is the
/// same computation, so it is bit-identical to [`fit_ellipse_algebraic`]'s.
///
/// Not from the literature: this only removes a quantity nobody reads. For the same
/// reason of cost, the passes below reuse each point's weight and lifted row, and the
/// 5×5 eigen solve runs on stack arrays (`solver::gen_eigen_5`): the arithmetic and its
/// order are those of the original, so the fit is bit-identical.
pub(crate) fn taubin_ellipse(pts: &[Point], sigma: &[f64]) -> Option<EllipseFit> {
    taubin_ellipse_with_residual(pts, sigma).map(|(e, _)| e)
}

/// [`taubin_ellipse`], also returning Taubin's minimised residual in px² as a weighted
/// sum over the points, `μ·s²·Σw`: `μ = Σ w·Q² / Σ w·|∇Q|²` is the smallest generalised
/// eigenvalue in the scaled frame, `s` the frame's scale and `Σw` the total weight. It
/// approximates the Sampson χ² `Σ w·Q²/|∇Q|²` of the conic (Taubin 1991), and so, near
/// the curve, its orthogonal χ² (see [`fit_ellipse_screened`]).
pub(crate) fn taubin_ellipse_with_residual(
    pts: &[Point],
    sigma: &[f64],
) -> Option<(EllipseFit, f64)> {
    let n = pts.len();
    if n < 6 {
        return None;
    }
    // Each point's weight and lifted row are computed once and reused by every pass
    // below, instead of once per pass: the same values, so the same sums.
    let w: Vec<f64> = (0..n).map(|k| weight_at(sigma, k)).collect();
    let mut sw = 0.0;
    let mut mx = 0.0;
    let mut my = 0.0;
    for (p, &wk) in pts.iter().zip(&w) {
        sw += wk;
        mx += p.x * wk;
        my += p.y * wk;
    }
    if sw <= 0.0 {
        return None;
    }
    mx /= sw;
    my /= sw;
    let mut var = 0.0;
    for (p, &wk) in pts.iter().zip(&w) {
        var += wk * ((p.x - mx).powi(2) + (p.y - my).powi(2));
    }
    let scale = (var / sw).sqrt();
    if scale.is_nan() || scale <= 1e-12 {
        return None;
    }

    let rows: Vec<[f64; 5]> = pts
        .iter()
        .map(|p| {
            let (u, v) = ((p.x - mx) / scale, (p.y - my) / scale);
            [u * u, u * v, v * v, u, v]
        })
        .collect();
    let mut mean = [0.0f64; 5];
    for (row, &wk) in rows.iter().zip(&w) {
        for a in 0..5 {
            mean[a] += wk * row[a];
        }
    }
    for m in &mut mean {
        *m /= sw;
    }
    let mut cov = [[0.0f64; 5]; 5];
    let mut nrm = [[0.0f64; 5]; 5];
    for (row, &wk) in rows.iter().zip(&w) {
        let (u, v) = (row[3], row[4]);
        let gx = [2.0 * u, v, 0.0, 1.0, 0.0];
        let gy = [0.0, u, 2.0 * v, 0.0, 1.0];
        let d = [
            row[0] - mean[0],
            row[1] - mean[1],
            row[2] - mean[2],
            row[3] - mean[3],
            row[4] - mean[4],
        ];
        for a in 0..5 {
            for b in 0..5 {
                cov[a][b] += wk * d[a] * d[b];
                nrm[a][b] += wk * (gx[a] * gx[b] + gy[a] * gy[b]);
            }
        }
    }
    let (theta, mu) = gen_eigen_5(&cov, &nrm)?;
    let f = -(0..5).map(|k| mean[k] * theta[k]).sum::<f64>();
    let (c, r1, r2, angle) =
        conic_to_ellipse([theta[0], theta[1], theta[2], theta[3], theta[4], f])?;
    let (rx, ry, angle) = if r1 >= r2 {
        (r1, r2, angle)
    } else {
        (r2, r1, angle + PI / 2.0)
    };
    let fit = EllipseFit {
        c: Point::new(mx + scale * c.x, my + scale * c.y),
        rx: rx * scale,
        ry: ry * scale,
        angle: canonical_angle(angle),
        chi2: f64::INFINITY,
    };
    Some((fit, mu * scale * scale * sw))
}

/// Orthogonal-distance ellipse fit (Ahn et al. 2001).
///
/// Levenberg–Marquardt on the orthogonal distances (`refine_ellipse`) from several
/// starts, keeping the lowest χ²: the algebraic fit when there is one, and four
/// near-circles (radii 1.02·r and 0.98·r about the orthogonal circle fit) at 0°, 45°, 90°
/// and 135°. Several starts because the orthogonal objective has local minima, and the
/// near-circles still give a start where the algebraic fit returns nothing (it offers
/// only ellipses) or lands far off. `None` for fewer than six points or when every start
/// fails.
pub fn fit_ellipse(pts: &[Point], sigma: &[f64]) -> Option<EllipseFit> {
    if pts.len() < 6 {
        return None;
    }
    fit_ellipse_from(pts, sigma, fit_circle(pts, sigma))
}

/// [`fit_ellipse`] with its orthogonal circle fit (`fit_circle(pts, sigma)`, whose centre
/// and radius seed the near-circle starts) supplied by a caller that has already fitted
/// it. `None` for fewer than six points or when every start fails.
pub(crate) fn fit_ellipse_from(
    pts: &[Point],
    sigma: &[f64],
    circle: Option<super::CircleFit>,
) -> Option<EllipseFit> {
    if pts.len() < 6 {
        return None;
    }
    let mut starts: Vec<EllipseFit> = Vec::new();
    // Levenberg–Marquardt reads only the start's geometry, so the algebraic fit's own
    // orthogonal χ² is not computed (see `taubin_ellipse`).
    if let Some(e) = taubin_ellipse(pts, sigma) {
        starts.push(e);
    }
    if let Some(cf) = circle {
        for k in 0..4 {
            starts.push(EllipseFit {
                c: cf.c,
                rx: cf.r * 1.02,
                ry: cf.r * 0.98,
                angle: k as f64 * PI / 4.0,
                chi2: f64::INFINITY,
            });
        }
    }
    let mut best: Option<EllipseFit> = None;
    for s in starts {
        if let Some(e) = refine_ellipse(pts, sigma, s) {
            match best {
                Some(b) if b.chi2 <= e.chi2 => {}
                _ => best = Some(e),
            }
        }
    }
    best
}

/// Taubin's residual, as a share of the orthogonal χ² the Levenberg–Marquardt fit
/// reaches from it, never fell below 0.28 over the 1,131 closed rings of the 246-icon
/// screen set and the 232 of a 1672×941 poster. A residual this multiple of which already
/// exceeds the caller's χ² gate marks an ellipse the gate would refuse.
const SCREEN_SHARE: f64 = 0.25;

/// The orthogonal ellipse fit the whole-ring primitive search offers, or `None` where the
/// χ² `gate` it must then pass would refuse it anyway.
///
/// Two economies over [`fit_ellipse_from`], both taken from the literature on algebraic
/// fits as initialisers. Taubin's normalisation makes the algebraic residual approximate
/// the squared geometric distance: Taubin (1991), "Estimation of planar curves, surfaces,
/// and nonplanar space curves defined by implicit equations with applications to edge and
/// range image segmentation", IEEE TPAMI 13(11):1115–1138, doi:10.1109/34.103273; and
/// Kanatani & Rangarajan (2011), "Hyper least squares fitting of circles and ellipses",
/// Comput. Stat. Data Anal. 55:2197–2208, doi:10.1016/j.csda.2010.12.012, eqs. 14–17 and
/// 23, cast it as the eigenproblem solved here. So:
///
/// * the fit is not run when `SCREEN_SHARE` of Taubin's residual already exceeds `gate`
///   (the gate refused 924 of the screen set's 1,131 closed-ring ellipses; this screen
///   catches 806 of them, and never one the gate passed);
/// * Levenberg–Marquardt starts from the algebraic fit alone, as Halíř & Flusser (1998),
///   "Numerically stable direct least squares fitting of ellipses", WSCG,
///   https://autotrace.sourceforge.net/WSCG98.pdf, recommend for the direct fit ("a fast
///   and robust estimator of a good initial solution"), rather than from it and four
///   near-circles. Over the screen set and the poster, every one of the 93 ellipses chosen
///   came from the algebraic start. The near-circles remain the fallback when there is no
///   algebraic ellipse, or when Levenberg–Marquardt fails from it.
pub(crate) fn fit_ellipse_screened(
    pts: &[Point],
    sigma: &[f64],
    circle: Option<super::CircleFit>,
    gate: f64,
) -> Option<EllipseFit> {
    if pts.len() < 6 {
        return None;
    }
    let Some((alg, residual)) = taubin_ellipse_with_residual(pts, sigma) else {
        return fit_ellipse_from(pts, sigma, circle);
    };
    if SCREEN_SHARE * residual > gate {
        return None;
    }
    refine_ellipse(pts, sigma, alg).or_else(|| fit_ellipse_from(pts, sigma, circle))
}

/// Levenberg–Marquardt on the signed orthogonal distances `d_k` of [`EllipseFit::contact`],
/// over `(cx, cy, rx, ry, angle)`, weighted by `1/σ²`.
///
/// The Jacobian uses the envelope argument of the `primitives` overview: the contact angle is
/// held fixed, so with `n` the unit normal in the ellipse frame, `n_w` the same normal in
/// the world frame and `(ex, ey) = (rx cos t, ry sin t)`,
///
/// ```text
///     ∂d/∂c = −n_w,   ∂d/∂rx = −n_x·cos t,   ∂d/∂ry = −n_y·sin t,   ∂d/∂angle = n_x·ey − n_y·ex
/// ```
///
/// Radii are kept at least 1e-3 px, and at most 200 iterations run. The result is
/// normalised to `rx ≥ ry`, `angle ∈ (−π/2, π/2]`. `None` if a contact distance turns
/// non-finite or the solver fails.
fn refine_ellipse(pts: &[Point], sigma: &[f64], init: EllipseFit) -> Option<EllipseFit> {
    let w = weights(sigma, pts.len());
    let eval = |p: &[f64]| -> Option<(f64, Vec<Vec<f64>>, Vec<f64>)> {
        let e = EllipseFit {
            c: Point::new(p[0], p[1]),
            rx: p[2],
            ry: p[3],
            angle: p[4],
            chi2: 0.0,
        };
        let (s, c) = e.angle.sin_cos();
        let mut jtj = vec![vec![0.0; 5]; 5];
        let mut jtr = vec![0.0; 5];
        let mut chi2 = 0.0;
        for (pt, wk) in pts.iter().zip(&w) {
            let (d, t, n) = e.contact(*pt);
            if !d.is_finite() {
                return None;
            }
            let (st, ct) = t.sin_cos();
            let nw = Vec2 {
                x: c * n.x - s * n.y,
                y: s * n.x + c * n.y,
            };
            let (ex, ey) = (e.rx * ct, e.ry * st);
            let j = [-nw.x, -nw.y, -n.x * ct, -n.y * st, n.x * ey - n.y * ex];
            chi2 += wk * d * d;
            for a in 0..5 {
                jtr[a] += wk * j[a] * d;
                for b in 0..5 {
                    jtj[a][b] += wk * j[a] * j[b];
                }
            }
        }
        Some((chi2, jtj, jtr))
    };
    let project = |p: &mut [f64]| {
        p[2] = p[2].abs().max(1e-3);
        p[3] = p[3].abs().max(1e-3);
    };
    let p0 = vec![init.c.x, init.c.y, init.rx, init.ry, init.angle];
    let (p, chi2) = levenberg_marquardt(p0, 200, eval, project)?;
    let (mut rx, mut ry, mut angle) = (p[2], p[3], p[4]);
    if rx < ry {
        std::mem::swap(&mut rx, &mut ry);
        angle += PI / 2.0;
    }
    Some(EllipseFit {
        c: Point::new(p[0], p[1]),
        rx,
        ry,
        angle: canonical_angle(angle),
        chi2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_angle() {
        assert!((canonical_angle(PI) - 0.0).abs() < 1e-10);
        assert!((canonical_angle(PI / 4.0) - PI / 4.0).abs() < 1e-10);
    }

    /// `fit_ellipse_algebraic` as it was before its passes shared their weights and rows
    /// (its eigen solve is proved bit-identical to the old one in `solver`'s tests).
    fn reference_algebraic(pts: &[Point], sigma: &[f64]) -> Option<EllipseFit> {
        let n = pts.len();
        if n < 6 {
            return None;
        }
        let mut sw = 0.0;
        let mut mx = 0.0;
        let mut my = 0.0;
        for (k, p) in pts.iter().enumerate() {
            let w = weight_at(sigma, k);
            sw += w;
            mx += p.x * w;
            my += p.y * w;
        }
        if sw <= 0.0 {
            return None;
        }
        mx /= sw;
        my /= sw;
        let mut var = 0.0;
        for (k, p) in pts.iter().enumerate() {
            let w = weight_at(sigma, k);
            var += w * ((p.x - mx).powi(2) + (p.y - my).powi(2));
        }
        let scale = (var / sw).sqrt();
        if scale.is_nan() || scale <= 1e-12 {
            return None;
        }

        let row_of = |p: &Point| -> [f64; 5] {
            let (u, v) = ((p.x - mx) / scale, (p.y - my) / scale);
            [u * u, u * v, v * v, u, v]
        };
        let mut mean = [0.0f64; 5];
        for (k, p) in pts.iter().enumerate() {
            let w = weight_at(sigma, k);
            let row = row_of(p);
            for a in 0..5 {
                mean[a] += w * row[a];
            }
        }
        for m in &mut mean {
            *m /= sw;
        }
        let mut cov = [[0.0f64; 5]; 5];
        let mut nrm = [[0.0f64; 5]; 5];
        for (k, p) in pts.iter().enumerate() {
            let w = weight_at(sigma, k);
            let row = row_of(p);
            let (u, v) = (row[3], row[4]);
            let gx = [2.0 * u, v, 0.0, 1.0, 0.0];
            let gy = [0.0, u, 2.0 * v, 0.0, 1.0];
            let d = [
                row[0] - mean[0],
                row[1] - mean[1],
                row[2] - mean[2],
                row[3] - mean[3],
                row[4] - mean[4],
            ];
            for a in 0..5 {
                for b in 0..5 {
                    cov[a][b] += w * d[a] * d[b];
                    nrm[a][b] += w * (gx[a] * gx[b] + gy[a] * gy[b]);
                }
            }
        }
        let (theta, _) = gen_eigen_5(&cov, &nrm)?;
        let f = -(0..5).map(|k| mean[k] * theta[k]).sum::<f64>();
        let (c, r1, r2, angle) =
            conic_to_ellipse([theta[0], theta[1], theta[2], theta[3], theta[4], f])?;
        let (rx, ry, angle) = if r1 >= r2 {
            (r1, r2, angle)
        } else {
            (r2, r1, angle + PI / 2.0)
        };
        let mut e = EllipseFit {
            c: Point::new(mx + scale * c.x, my + scale * c.y),
            rx: rx * scale,
            ry: ry * scale,
            angle: canonical_angle(angle),
            chi2: 0.0,
        };
        e.chi2 = ellipse_chi2(pts, sigma, &e);
        Some(e)
    }

    /// The shared weights and rows change no bit of the fit, χ² included.
    #[test]
    fn algebraic_fit_matches_its_former_self() {
        let mut st = 23u64;
        let mut rnd = || {
            st = st
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (st >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut fitted = 0;
        for case in 0..400 {
            let n = 4 + case % 90;
            let (rx, ry, rot, sweep) = (
                1.0 + 60.0 * rnd(),
                1.0 + 30.0 * rnd(),
                3.0 * rnd(),
                0.2 + 6.1 * rnd(),
            );
            let noise = [0.0, 0.02, 0.3, 2.0][case % 4];
            let pts: Vec<Point> = (0..n)
                .map(|k| {
                    let t = sweep * k as f64 / n as f64;
                    let (x, y) = (rx * t.cos(), ry * t.sin());
                    let (s, c) = rot.sin_cos();
                    Point::new(
                        300.0 + c * x - s * y + noise * (rnd() - 0.5),
                        -120.0 + s * x + c * y + noise * (rnd() - 0.5),
                    )
                })
                .collect();
            // Short sigma slices too: missing sigmas default to 0.5.
            let sigma: Vec<f64> = (0..n - case % 3).map(|_| 0.01 + rnd()).collect();
            let got = fit_ellipse_algebraic(&pts, &sigma);
            let want = reference_algebraic(&pts, &sigma);
            assert_eq!(got.is_some(), want.is_some(), "case {case}");
            if let (Some(g), Some(w)) = (got, want) {
                let bits =
                    |e: EllipseFit| [e.c.x, e.c.y, e.rx, e.ry, e.angle, e.chi2].map(f64::to_bits);
                assert_eq!(bits(g), bits(w), "case {case}");
                fitted += 1;
            }
        }
        assert!(fitted > 100);
    }

    /// The screened, single-start fit reaches the whole-ring search's verdict on the full
    /// five-start fit (sane radii, the χ² gate, a full turn round the centre, as
    /// `super::offer_ellipse` asks) on ellipses, noisy ellipses, rounded squares, stars and
    /// slivers, and where both are offered it is no worse.
    #[test]
    fn screened_fit_agrees_with_the_full_search_at_the_gate() {
        let mut st = 77u64;
        let mut rnd = || {
            st = st
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (st >> 11) as f64 / (1u64 << 53) as f64
        };
        let (mut passed, mut refused) = (0, 0);
        for case in 0..240 {
            let n = 24 + case % 150;
            let (rx, ry, rot) = (3.0 + 40.0 * rnd(), 2.0 + 25.0 * rnd(), 3.0 * rnd());
            let noise = [0.0, 0.1, 0.4, 1.0][case % 4];
            // 0: ellipse; 1: superellipse (rounded square); 2: star; 3: thin sliver.
            let shape = (case / 4) % 4;
            let pts: Vec<Point> = (0..n)
                .map(|k| {
                    let t = std::f64::consts::TAU * k as f64 / n as f64;
                    let (c, s) = (t.cos(), t.sin());
                    let (x, y) = match shape {
                        0 => (rx * c, ry * s),
                        1 => (
                            rx * c.signum() * c.abs().sqrt(),
                            ry * s.signum() * s.abs().sqrt(),
                        ),
                        2 => {
                            let r = 1.0 + 0.3 * (5.0 * t).cos();
                            (rx * r * c, rx * r * s)
                        }
                        _ => (rx * c, 0.05 * ry * s),
                    };
                    let (sr, cr) = rot.sin_cos();
                    Point::new(
                        cr * x - sr * y + noise * (rnd() - 0.5),
                        sr * x + cr * y + noise * (rnd() - 0.5),
                    )
                })
                .collect();
            let sigma = vec![0.25; n];
            let gate = 4.0 * n as f64;
            let full = fit_ellipse(&pts, &sigma);
            let screened = fit_ellipse_screened(&pts, &sigma, fit_circle(&pts, &sigma), gate);
            let span = pts
                .iter()
                .map(|p| p.dist(pts[0]))
                .fold(0.0f64, f64::max)
                .max(1.0);
            let offered = |e: &EllipseFit| {
                e.rx.is_finite()
                    && e.ry.is_finite()
                    && e.rx.max(e.ry) <= 1e3 * span
                    && e.chi2 <= gate
                    && super::super::total_sweep(&pts, e.c, true).abs() > 1.9 * PI
            };
            let full_ok = full.as_ref().is_some_and(offered);
            let screened_ok = screened.as_ref().is_some_and(offered);
            let res = taubin_ellipse_with_residual(&pts, &sigma)
                .map(|(e, r)| (r, refine_ellipse(&pts, &sigma, e).map(|f| f.chi2)));
            assert_eq!(
                full_ok,
                screened_ok,
                "case {case}, shape {shape}: gate {gate} full {:?} screened {:?} residual/refined {:?}",
                full.map(|e| (e.chi2, e.rx, e.ry)),
                screened.map(|e| (e.chi2, e.rx, e.ry)),
                res
            );
            if let (true, Some(f), Some(s)) = (full_ok, full, screened) {
                assert!(s.chi2 <= f.chi2 * (1.0 + 1e-6) + 1e-9, "case {case}");
                passed += 1;
            } else {
                refused += 1;
            }
        }
        assert!(
            passed > 20 && refused > 20,
            "{passed} passed, {refused} refused"
        );
    }

    /// The unscored fit is the scored one minus its χ², bit for bit, on arcs, full rings,
    /// noisy points and inputs too few or too degenerate to fit.
    #[test]
    fn taubin_ellipse_is_the_algebraic_fit_without_its_chi2() {
        let mut st = 11u64;
        let mut rnd = || {
            st = st
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (st >> 11) as f64 / (1u64 << 53) as f64
        };
        for case in 0..300 {
            let n = 3 + case % 40;
            let (rx, ry, rot) = (2.0 + 30.0 * rnd(), 1.0 + 20.0 * rnd(), 3.0 * rnd());
            let sweep = 0.3 + 6.0 * rnd();
            let noise = [0.0, 0.05, 0.5][case % 3];
            let pts: Vec<Point> = (0..n)
                .map(|k| {
                    let t = sweep * k as f64 / n as f64;
                    let (x, y) = (rx * t.cos(), ry * t.sin());
                    let (s, c) = rot.sin_cos();
                    Point::new(
                        40.0 + c * x - s * y + noise * (rnd() - 0.5),
                        -7.0 + s * x + c * y + noise * (rnd() - 0.5),
                    )
                })
                .collect();
            let sigma: Vec<f64> = (0..n).map(|_| 0.1 + rnd()).collect();
            let lean = taubin_ellipse(&pts, &sigma);
            let full = fit_ellipse_algebraic(&pts, &sigma);
            assert_eq!(lean.is_some(), full.is_some(), "case {case}");
            if let (Some(l), Some(f)) = (lean, full) {
                assert!(l.chi2.is_infinite());
                for (a, b) in [(l.c.x, f.c.x), (l.c.y, f.c.y), (l.rx, f.rx), (l.ry, f.ry)] {
                    assert_eq!(a.to_bits(), b.to_bits(), "case {case}");
                }
                assert_eq!(l.angle.to_bits(), f.angle.to_bits(), "case {case}");
                let chi2 = ellipse_chi2(&pts, &sigma, &l);
                assert_eq!(chi2.to_bits(), f.chi2.to_bits(), "case {case}");
            }
        }
        // Collinear and coincident points fit nothing, either way.
        let line: Vec<Point> = (0..10)
            .map(|k| Point::new(k as f64, 2.0 * k as f64))
            .collect();
        let same = vec![Point::new(3.0, 4.0); 10];
        for pts in [line, same] {
            let sigma = vec![0.5; pts.len()];
            assert_eq!(
                taubin_ellipse(&pts, &sigma).is_some(),
                fit_ellipse_algebraic(&pts, &sigma).is_some()
            );
        }
    }

    #[test]
    fn test_ellipse_contact() {
        let e = EllipseFit {
            c: Point::new(0.0, 0.0),
            rx: 10.0,
            ry: 5.0,
            angle: 0.0,
            chi2: 0.0,
        };
        let (d, t, n) = e.contact(Point::new(12.0, 0.0));
        assert!((d - 2.0).abs() < 1e-6);
        assert!(t.abs() < 1e-6);
        assert!((n.x - 1.0).abs() < 1e-6);
    }
}
