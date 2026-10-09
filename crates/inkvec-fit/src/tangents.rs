//! Tangent estimation and turn cost models (DESIGN.md S4).
//!
//! Stage 3 of the shipping fit (see the crate overview): before the multimodel dynamic
//! program runs, every measured point gets an incoming and an outgoing unit tangent
//! ([`estimate_tangents`]). The program's cubics inherit these directions at their ends
//! (only the arm lengths are fitted), and every chosen vertex is charged for the turn
//! between its two tangents ([`vertex_cost`], [`break_cost`]). Called from
//! `crate::multimodel` (and its `scan` and `refine` steps) and from `crate::candidates`.
//!
//! The tangent at a point is the derivative of a weighted least-squares quadratic fitted
//! to the points around it, parametrized by arc length. The window is the widest (up to
//! [`TANGENT_WINDOW_MAX`] points a side) whose quadratic is still consistent with the
//! measurement model, `χ² ≤ τ²·dof`. A window centred on the point is tried first; it
//! passes on smooth runs, where averaging both sides beats noise, and fails across a
//! true corner, where no quadratic fits, and there the two sides are estimated
//! separately so each keeps its own direction instead of their bisector.

use crate::FitConfig;
use inkvec_core::{Point, Polyline, Vec2};

/// Tangent break, in degrees, at which a join is charged the full cost of a corner
/// (one parameter, `λ`). Below it the charge ramps quadratically.
pub const G1_BREAK_DEGREES: f64 = 10.0;

/// Widest tangent window, in points on each side.
pub const TANGENT_WINDOW_MAX: usize = 16;

/// Break angle threshold in radians: 10 degrees unless the trace in progress asked for another
/// (see [`crate::cost`]).
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

/// Cumulative arc length at each vertex, in px: `s[0] = 0`, `s[k] = s[k−1] + |p_k − p_{k−1}|`.
pub fn arc_lengths(pts: &[Point]) -> Vec<f64> {
    if pts.is_empty() {
        return Vec::new();
    }
    let mut s = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    s.push(0.0);
    for k in 1..pts.len() {
        acc += pts[k].dist(pts[k - 1]);
        s.push(acc);
    }
    #[cfg(debug_assertions)]
    debug_assert!(
        s.windows(2).all(|w| w[1] >= w[0]),
        "arc lengths must be non-decreasing"
    );
    s
}

/// Solve a symmetric 3x3 system by Cramer's rule. `m` is row-major.
///
/// `x_c = det(M with column c replaced by r) / det(M)`. `None` when `|det(M)| < 1e-18`,
/// i.e. the system is (numerically) singular; the threshold is absolute, so callers pass
/// systems whose entries are of order one or larger. Cramer's rule is fine at 3x3 and
/// avoids pivoting logic; nothing here needs better conditioning.
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
///
/// Each sample is `(u, p, w)`: its signed arc-length offset from the point of interest
/// (px), its position (px) and its weight `1/σ²`. The normal equations
///
/// ```text
///     [S0 S1 S2] [a]   [Σw·x    ]
///     [S1 S2 S3] [b] = [Σw·u·x  ]      S_m = Σ w·u^m
///     [S2 S3 S4] [c]   [Σw·u²·x ]
/// ```
///
/// are solved for x and y separately ([`solve3`]). The returned direction is `(b_x, b_y)`
/// normalised, which points towards increasing `u`; the residual is
/// `Σ w·|p − q(u)|²`, a χ² in units of sigma, with `2·len − 6` degrees of freedom.
/// `None` for a singular system or a zero or non-finite derivative.
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
    #[cfg(debug_assertions)]
    debug_assert!(
        resid >= 0.0 && resid.is_finite(),
        "chi-squared residual must be non-negative and finite: {resid}"
    );
    Some((
        Vec2 {
            x: d.x / n,
            y: d.y / n,
        },
        resid,
    ))
}

/// Tangent at vertex `k` from the points on one side of it only.
///
/// `forward` uses `k, k+1, …` (the outgoing tangent), otherwise `k, k−1, …` (the incoming
/// one, with negative `u` so the direction still points along the boundary). Windows of
/// `w = wmax..2` points beyond `k` are tried widest first and the first whose quadratic
/// satisfies `χ² ≤ τ²·(2(w+1) − 6)` is taken; at `w = 2` the fit has no spare degrees of
/// freedom and is always accepted. `wmax` is [`TANGENT_WINDOW_MAX`], less near the end of
/// an open polyline; a closed one wraps. With only one point available on that side, or
/// if every fit is singular, the chord to the neighbour is used. `None` when there is no
/// neighbour on that side (the end of an open polyline) or it coincides with `k`.
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
///
/// Windows of `w = half..2` points on each side (capped by [`TANGENT_WINDOW_MAX`] and by
/// what the polyline has) are fitted with a quadratic in signed arc length, widest first;
/// the first with `χ² ≤ τ²·(2(2w+1) − 6)` wins. `None` when fewer than two points are
/// available on a side or no window is consistent, which is what happens across a
/// corner: that `None` is the signal to fall back to one-sided estimates.
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
///
/// Where a symmetric window fits (`symmetric_tangent`), incoming and outgoing are the
/// same direction: the point is on a smooth run. Otherwise each side is estimated on its
/// own (`one_sided_tangent`); if only one side exists (the ends of an open polyline) both
/// take it, and with neither the x axis stands in. All tangents are unit vectors pointing
/// along the polyline's direction of travel.
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
    #[cfg(debug_assertions)]
    for t in incoming.iter().chain(outgoing.iter()) {
        debug_assert!(
            t.x.is_finite() && t.y.is_finite() && (t.norm() - 1.0).abs() < 1e-6,
            "tangent must be finite unit vector, got {t:?}"
        );
    }
    Tangents { incoming, outgoing }
}

/// Unsigned angle between two directions, in radians, in `[0, π]`: `acos(a·b / (|a||b|))`.
/// A zero vector has no direction and gives 0.
pub fn turn_angle(a: Vec2, b: Vec2) -> f64 {
    let (na, nb) = (a.norm(), b.norm());
    if !na.is_finite() || !nb.is_finite() || na < 1e-12 || nb < 1e-12 {
        return 0.0;
    }
    let cos_theta = (a.dot(b) / (na * nb)).clamp(-1.0, 1.0);
    if !cos_theta.is_finite() {
        return 0.0;
    }
    cos_theta.acos()
}

/// Cost of a tangent break between directions `a` and `b`, in nats.
///
/// ```text
///     cost = λ · min(1, (θ / θ_break)²)
/// ```
///
/// with `θ` the `turn_angle` and `θ_break` the break angle in force
/// ([`G1_BREAK_DEGREES`] unless a [`crate::cost::CostModel`] says otherwise). A corner is
/// one extra free parameter (the outgoing direction is no longer implied by the incoming
/// one), hence the saturation at `λ`; the quadratic ramp below it keeps tangent-estimate
/// noise on a smooth join nearly free.
pub fn break_cost(a: Vec2, b: Vec2, lambda: f64) -> f64 {
    // Quadratic in the turn, saturating at a full corner (the exponent was once
    // `INKVEC_BREAK_EXP`; nothing measured another).
    let r = turn_angle(a, b) / g1_break_radians();
    lambda * (r * r).min(1.0)
}

/// Turn cost charged when vertex `k` is chosen as a segment boundary: the
/// [`break_cost`] between its incoming and outgoing tangents, in nats.
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

    #[test]
    fn test_arc_lengths_monotonicity_and_invariants() {
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(3.0, 4.0),
            Point::new(3.0, 4.0), // zero step
            Point::new(6.0, 8.0),
            Point::new(10.0, 8.0),
        ];
        let lens = arc_lengths(&pts);
        assert_eq!(lens.len(), 5);
        assert_eq!(lens[0], 0.0);
        assert_eq!(lens[1], 5.0);
        assert_eq!(lens[2], 5.0);
        assert_eq!(lens[3], 10.0);
        assert_eq!(lens[4], 14.0);
        assert!(lens.windows(2).all(|w| w[1] >= w[0]));
        assert!(arc_lengths(&[]).is_empty());
    }

    #[test]
    fn test_quadratic_tangent_invariants() {
        // Samples along a parabola y = 0.5 * u^2, x = u
        let samples = vec![
            (-1.0, Point::new(-1.0, 0.5), 1.0),
            (0.0, Point::new(0.0, 0.0), 1.0),
            (1.0, Point::new(1.0, 0.5), 1.0),
        ];
        let (tan, resid) = quadratic_tangent(&samples).expect("quadratic fit");
        assert!(resid >= 0.0 && resid.is_finite());
        assert!(resid < 1e-12); // exact quadratic fit
        assert!((tan.norm() - 1.0).abs() < 1e-6);
        assert!((tan.x - 1.0).abs() < 1e-6 && tan.y.abs() < 1e-6); // tangent at u=0 is (1, 0)
    }

    #[test]
    fn test_estimate_tangents_all_unit_norm() {
        let poly = Polyline::with_uniform_sigma(
            vec![
                Point::new(0.0, 0.0),
                Point::new(1.0, 2.0),
                Point::new(4.0, 5.0),
                Point::new(10.0, 2.0),
            ],
            0.05,
            false,
        );
        let cfg = FitConfig::default();
        let tans = estimate_tangents(&poly, &cfg);
        for i in 0..poly.len() {
            assert!((tans.incoming[i].norm() - 1.0).abs() < 1e-6);
            assert!((tans.outgoing[i].norm() - 1.0).abs() < 1e-6);
        }
    }
}
