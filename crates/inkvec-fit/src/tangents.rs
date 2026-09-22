//! Tangent estimation and turn cost models (DESIGN.md S4).

use crate::FitConfig;
use inkvec_core::{Point, Polyline, Vec2};

/// Tangent break, in degrees, at which a join is charged the full cost of a corner
/// (one parameter, `λ`). Below it the charge ramps quadratically.
pub const G1_BREAK_DEGREES: f64 = 10.0;

/// Widest tangent window, in points on each side.
pub const TANGENT_WINDOW_MAX: usize = 16;

/// Break angle threshold in radians: 10 degrees unless the trace in progress asked for another
/// (see [`crate::cost`]), or an experiment set `INKVEC_G1_BREAK`.
pub(crate) fn g1_break_radians() -> f64 {
    crate::cost::g1_break_radians()
}

/// One-sided unit tangents at every vertex: `incoming[k]` is the direction the boundary
/// arrives at `k` with, `outgoing[k]` the direction it leaves with. Both point forward.
#[derive(Debug, Clone)]
pub struct Tangents {
    /// Unit tangent arriving at each vertex.
    pub incoming: Vec<Vec2>,
    /// Unit tangent leaving each vertex.
    pub outgoing: Vec<Vec2>,
}

/// Cumulative arc length at each vertex.
pub(crate) fn arc_lengths(pts: &[Point]) -> Vec<f64> {
    let mut s = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    s.push(0.0);
    for k in 1..pts.len() {
        acc += pts[k].dist(pts[k - 1]);
        s.push(acc);
    }
    s
}

/// Solve a symmetric 3x3 system by Cramer's rule. `m` is row-major.
pub(crate) fn solve3(m: [[f64; 3]; 3], r: [f64; 3]) -> Option<[f64; 3]> {
    let det = |a: [[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det(m);
    if d.abs() < 1e-18 {
        return None;
    }
    let mut out = [0.0; 3];
    for (col, o) in out.iter_mut().enumerate() {
        let mut mm = m;
        for row in 0..3 {
            mm[row][col] = r[row];
        }
        *o = det(mm) / d;
    }
    Some(out)
}

/// Weighted least-squares quadratic `a + b·u + c·u²` through `(u, x, y, w)` samples,
/// returning the derivative `(b_x, b_y)` at `u = 0` and the normalized residual.
fn quadratic_tangent(samples: &[(f64, Point, f64)]) -> Option<(Vec2, f64)> {
    let mut s = [0.0f64; 5];
    let mut tx = [0.0f64; 3];
    let mut ty = [0.0f64; 3];
    for &(u, p, w) in samples {
        let mut up = 1.0;
        for (k, sk) in s.iter_mut().enumerate() {
            *sk += w * up;
            if k < 3 {
                tx[k] += w * up * p.x;
                ty[k] += w * up * p.y;
            }
            up *= u;
        }
    }
    let m = [[s[0], s[1], s[2]], [s[1], s[2], s[3]], [s[2], s[3], s[4]]];
    let cx = solve3(m, tx)?;
    let cy = solve3(m, ty)?;
    let mut resid = 0.0;
    for &(u, p, w) in samples {
        let rx = p.x - (cx[0] + cx[1] * u + cx[2] * u * u);
        let ry = p.y - (cy[0] + cy[1] * u + cy[2] * u * u);
        resid += w * (rx * rx + ry * ry);
    }
    let d = Vec2 { x: cx[1], y: cy[1] };
    let n = d.norm();
    if n < 1e-12 || !n.is_finite() {
        return None;
    }
    Some((
        Vec2 {
            x: d.x / n,
            y: d.y / n,
        },
        resid,
    ))
}

/// Tangent at vertex `k` from the points on one side of it only.
fn one_sided_tangent(poly: &Polyline, k: usize, forward: bool, cfg: &FitConfig) -> Option<Vec2> {
    let n = poly.len();
    let avail = if poly.closed {
        n - 1
    } else if forward {
        n - 1 - k
    } else {
        k
    };
    let wmax = TANGENT_WINDOW_MAX.min(avail);
    if wmax < 1 {
        return None;
    }
    let idx = |m: usize| -> usize {
        if forward {
            (k + m) % n
        } else {
            (k + n - m % n) % n
        }
    };
    let mut samples: Vec<(f64, Point, f64)> = Vec::with_capacity(wmax + 1);
    let mut u = 0.0;
    for m in 0..=wmax {
        let i = idx(m);
        if m > 0 {
            let step = poly.points[i].dist(poly.points[idx(m - 1)]);
            u += if forward { step } else { -step };
        }
        samples.push((u, poly.points[i], 1.0 / (poly.sigma[i] * poly.sigma[i])));
    }

    let tau2 = cfg.tau * cfg.tau;
    for w in (2..=wmax).rev() {
        let Some((t, resid)) = quadratic_tangent(&samples[..=w]) else {
            continue;
        };
        let dof = (2 * (w + 1)).saturating_sub(6) as f64;
        if dof <= 0.0 || resid <= tau2 * dof {
            return Some(t);
        }
    }
    let d = poly.points[idx(1)] - poly.points[k];
    let nrm = d.norm();
    if nrm < 1e-12 {
        return None;
    }
    let sign = if forward { 1.0 } else { -1.0 };
    Some(Vec2 {
        x: sign * d.x / nrm,
        y: sign * d.y / nrm,
    })
}

/// Symmetric tangent at vertex `k` from the widest window of at most `half` points each side.
pub(crate) fn symmetric_tangent(
    poly: &Polyline,
    k: usize,
    half: usize,
    cfg: &FitConfig,
) -> Option<Vec2> {
    let n = poly.len();
    let avail = if poly.closed {
        (n - 1) / 2
    } else {
        k.min(n - 1 - k)
    };
    let half = half.min(TANGENT_WINDOW_MAX).min(avail);
    if half < 2 {
        return None;
    }
    let idx = |d: i64| -> usize { (k as i64 + d).rem_euclid(n as i64) as usize };
    let tau2 = cfg.tau * cfg.tau;
    for w in (2..=half).rev() {
        let mut samples = Vec::with_capacity(2 * w + 1);
        let mut u = 0.0;
        let mut back = Vec::with_capacity(w);
        for m in 1..=w {
            let (a, b) = (idx(-(m as i64)), idx(-(m as i64) + 1));
            u -= poly.points[a].dist(poly.points[b]);
            back.push((u, poly.points[a], 1.0 / (poly.sigma[a] * poly.sigma[a])));
        }
        samples.extend(back.into_iter().rev());
        samples.push((0.0, poly.points[k], 1.0 / (poly.sigma[k] * poly.sigma[k])));
        let mut u = 0.0;
        for m in 1..=w {
            let (a, b) = (idx(m as i64), idx(m as i64 - 1));
            u += poly.points[a].dist(poly.points[b]);
            samples.push((u, poly.points[a], 1.0 / (poly.sigma[a] * poly.sigma[a])));
        }
        let Some((t, resid)) = quadratic_tangent(&samples) else {
            continue;
        };
        let dof = (2 * (2 * w + 1) - 6) as f64;
        if resid <= tau2 * dof {
            return Some(t);
        }
    }
    None
}

/// Estimate the tangents at every vertex.
pub fn estimate_tangents(poly: &Polyline, cfg: &FitConfig) -> Tangents {
    let n = poly.len();
    let fallback = Vec2 { x: 1.0, y: 0.0 };
    let mut incoming = vec![fallback; n];
    let mut outgoing = vec![fallback; n];
    for k in 0..n {
        if let Some(t) = symmetric_tangent(poly, k, TANGENT_WINDOW_MAX, cfg) {
            incoming[k] = t;
            outgoing[k] = t;
            continue;
        }
        let fwd = one_sided_tangent(poly, k, true, cfg);
        let bwd = one_sided_tangent(poly, k, false, cfg);
        let (o, i) = match (fwd, bwd) {
            (Some(f), Some(b)) => (f, b),
            (Some(f), None) => (f, f),
            (None, Some(b)) => (b, b),
            (None, None) => (fallback, fallback),
        };
        outgoing[k] = o;
        incoming[k] = i;
    }
    Tangents { incoming, outgoing }
}

/// Unsigned angle between two directions, in radians.
pub(crate) fn turn_angle(a: Vec2, b: Vec2) -> f64 {
    let (na, nb) = (a.norm(), b.norm());
    if na < 1e-12 || nb < 1e-12 {
        return 0.0;
    }
    (a.dot(b) / (na * nb)).clamp(-1.0, 1.0).acos()
}

/// Cost of a tangent break between directions `a` and `b`.
pub fn break_cost(a: Vec2, b: Vec2, lambda: f64) -> f64 {
    let r = turn_angle(a, b) / g1_break_radians();
    let exp = break_exponent();
    let p = if exp == 2.0 { r * r } else { r.powf(exp) };
    lambda * p.min(1.0)
}

fn break_exponent() -> f64 {
    static V: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("INKVEC_BREAK_EXP")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(2.0)
    })
}

/// Turn cost charged when vertex `k` is chosen as a segment boundary.
pub fn vertex_cost(tan: &Tangents, k: usize, cfg: &FitConfig) -> f64 {
    break_cost(tan.incoming[k], tan.outgoing[k], cfg.lambda)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tangent_turn_angle_and_break_cost() {
        let a = Vec2 { x: 1.0, y: 0.0 };
        let b = Vec2 { x: 0.0, y: 1.0 };
        let angle = turn_angle(a, b);
        assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-6);
        let cost = break_cost(a, b, 1.0);
        assert_eq!(cost, 1.0);
    }
}
