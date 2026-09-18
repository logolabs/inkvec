//! Ellipse geometry and fitting (Taubin algebraic + orthogonal Levenberg-Marquardt).

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

    /// Derivative with respect to `t`.
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
pub fn fit_ellipse_algebraic(pts: &[Point], sigma: &[f64]) -> Option<EllipseFit> {
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
    let cov: Vec<Vec<f64>> = cov.iter().map(|r| r.to_vec()).collect();
    let nrm: Vec<Vec<f64>> = nrm.iter().map(|r| r.to_vec()).collect();
    let theta = gen_eigen_5(&cov, &nrm)?;
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

/// Orthogonal-distance ellipse fit (Ahn et al. 2001).
pub fn fit_ellipse(pts: &[Point], sigma: &[f64]) -> Option<EllipseFit> {
    if pts.len() < 6 {
        return None;
    }
    let mut starts: Vec<EllipseFit> = Vec::new();
    if let Some(e) = fit_ellipse_algebraic(pts, sigma) {
        starts.push(e);
    }
    if let Some(cf) = fit_circle(pts, sigma) {
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
