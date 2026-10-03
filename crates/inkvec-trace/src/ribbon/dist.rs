//! Exact distances from a point to one centreline segment -- line, cubic Bézier, circular
//! or elliptical arc -- for the stroke solve ([`super::refine`]) and the stroke score
//! ([`super::score`]).
//!
//! The solve measures every boundary point against the one segment its nearest flattened
//! piece belongs to ([`super::refine`]'s rows), so each routine here starts from that
//! piece's parameter and polishes it: a line's foot is a clamped projection, a circular
//! arc's the radial projection when it lies in the sweep (else the nearer endpoint), a
//! cubic's a few Newton steps on the foot condition `(C(t) - p)·C'(t) = 0`, and an
//! elliptical arc's (rare: only primitives make them) a sampled search refined by golden
//! section. Coordinates are the traced raster's pixels.
//!
//! Not from the literature as a method: these are the textbook nearest-point computations
//! for each curve type (the cubic's is Newton's method on the stationarity condition of the
//! squared distance). See also: Schneider (1990), Solving the nearest-point-on-curve
//! problem, Graphics Gems, Academic Press, pp. 607-611,
//! doi:10.1016/b978-0-08-050753-8.50131-5, which roots the same degree-5
//! condition by Bézier subdivision instead; a hint from the flattened piece makes Newton's
//! few steps enough here.

use inkvec_core::{Point, Vec2};
use inkvec_fit::curves::{arc_ellipse_center, Segment};

use super::grid::point_segment;

/// Distance from `p` to segment `s` starting at `a`, and the foot's parameter (for a
/// cubic, the Newton-polished `t` started from `t_hint`; otherwise unused).
///
/// Lines and circular arcs are closed-form: an arc's nearest point is the radial
/// projection when that lies within the sweep, else the nearer endpoint. A cubic's foot
/// solves `(C(t) - p)·C'(t) = 0` by four Newton steps, clamped to `[0, 1]`, against both
/// endpoints. An elliptical arc takes the best of 64 samples refined by golden section.
pub(super) fn dist_to(p: Point, a: Point, s: &Segment, t_hint: f64) -> (f64, f64) {
    match *s {
        Segment::Line(b) => {
            let (d, u, _) = point_segment(p, a, b);
            (d, u)
        }
        Segment::Cubic(c1, c2, b) => cubic_dist(p, [a, c1, c2, b], t_hint),
        Segment::Arc { .. } if s.is_circular() => (arc_dist(p, a, s), 0.0),
        Segment::Arc { .. } => (ellipse_arc_dist(p, a, s), 0.0),
    }
}

/// Cubic Bézier `q` at `t`, and its first and second derivatives.
pub(super) fn cubic_eval(q: &[Point; 4], t: f64) -> (Point, Point, Point) {
    let mt = 1.0 - t;
    let p = Point::new(
        mt * mt * mt * q[0].x
            + 3.0 * mt * mt * t * q[1].x
            + 3.0 * mt * t * t * q[2].x
            + t * t * t * q[3].x,
        mt * mt * mt * q[0].y
            + 3.0 * mt * mt * t * q[1].y
            + 3.0 * mt * t * t * q[2].y
            + t * t * t * q[3].y,
    );
    let d1 = |i: usize, j: usize| 3.0 * (q[j].x - q[i].x);
    let d1y = |i: usize, j: usize| 3.0 * (q[j].y - q[i].y);
    let dp = Point::new(
        mt * mt * d1(0, 1) + 2.0 * mt * t * d1(1, 2) + t * t * d1(2, 3),
        mt * mt * d1y(0, 1) + 2.0 * mt * t * d1y(1, 2) + t * t * d1y(2, 3),
    );
    let ddp = Point::new(
        6.0 * (mt * (q[2].x - 2.0 * q[1].x + q[0].x) + t * (q[3].x - 2.0 * q[2].x + q[1].x)),
        6.0 * (mt * (q[2].y - 2.0 * q[1].y + q[0].y) + t * (q[3].y - 2.0 * q[2].y + q[1].y)),
    );
    (p, dp, ddp)
}

/// Distance from `p` to cubic `q` and the foot's `t`: Newton on `(C - p)·C' = 0` from
/// `t_hint`, clamped, compared with both endpoints.
pub(super) fn cubic_dist(p: Point, q: [Point; 4], t_hint: f64) -> (f64, f64) {
    let mut t = t_hint.clamp(0.0, 1.0);
    for _ in 0..4 {
        let (c, d1, d2) = cubic_eval(&q, t);
        let (ex, ey) = (c.x - p.x, c.y - p.y);
        let f = ex * d1.x + ey * d1.y;
        let fp = d1.x * d1.x + d1.y * d1.y + ex * d2.x + ey * d2.y;
        if fp.abs() < 1e-12 {
            break;
        }
        t = (t - f / fp).clamp(0.0, 1.0);
    }
    let mut best = (cubic_eval(&q, t).0.dist(p), t);
    for (tt, e) in [(0.0, q[0]), (1.0, q[3])] {
        let d = e.dist(p);
        if d < best.0 {
            best = (d, tt);
        }
    }
    best
}

/// Distance from `p` to the circular arc `s` starting at `a`.
pub(super) fn arc_dist(p: Point, a: Point, s: &Segment) -> f64 {
    let Segment::Arc {
        rx,
        ry,
        phi,
        large_arc,
        sweep,
        end,
    } = *s
    else {
        return f64::INFINITY;
    };
    let f = arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
    let v = p - f.c;
    let rho = v.norm();
    if f.delta == 0.0 || rho < 1e-12 {
        return a.dist(p).min(end.dist(p));
    }
    let ang = v.y.atan2(v.x);
    let tau = std::f64::consts::TAU;
    // How far round the sweep the point's angle lies, in the sweep's own direction.
    let s_ang = if f.delta > 0.0 {
        (ang - f.theta1).rem_euclid(tau)
    } else {
        (f.theta1 - ang).rem_euclid(tau)
    };
    if s_ang <= f.delta.abs() {
        (rho - f.rx).abs()
    } else {
        a.dist(p).min(end.dist(p))
    }
}

/// Distance from `p` to the elliptical arc `s` starting at `a`: best of 64 samples in the
/// sweep, refined by 24 golden-section steps on the bracketing interval.
pub(super) fn ellipse_arc_dist(p: Point, a: Point, s: &Segment) -> f64 {
    let Segment::Arc {
        rx,
        ry,
        phi,
        large_arc,
        sweep,
        end,
    } = *s
    else {
        return f64::INFINITY;
    };
    let f = arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
    let d = |u: f64| f.at(f.theta1 + f.delta * u).dist(p);
    let n = 64;
    let mut best = (d(0.0), 0usize);
    for i in 1..=n {
        let v = d(i as f64 / n as f64);
        if v < best.0 {
            best = (v, i);
        }
    }
    let (mut lo, mut hi) = (
        best.1.saturating_sub(1) as f64 / n as f64,
        ((best.1 + 1).min(n)) as f64 / n as f64,
    );
    let g = 0.618_033_988_749_895;
    for _ in 0..24 {
        let (m1, m2) = (hi - g * (hi - lo), lo + g * (hi - lo));
        if d(m1) < d(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    d(0.5 * (lo + hi)).min(best.0)
}

// ---------------------------------------------------------------------------------------
// Gradients. The stroke solve needs, for every boundary point `p`, the derivative of its
// distance `d(p, C)` to the nearest segment with respect to that segment's variables.
//
// **The envelope theorem.** `d(θ) = min_t |p - C(t; θ)|`. At the minimiser `t*` the
// derivative of `|p - C(t; θ)|` in `t` vanishes (or `t*` sits on a bound of `[0, 1]`, where
// it stays to first order), so `∂d/∂θ = -n·∂C(t*; θ)/∂θ` with `n = (p - C(t*))/d` the unit
// vector from the foot to the point: the foot's own movement adds nothing to first order.
// For a Bézier segment `C(t) = Σ_i B_i(t)·P_i`, so `∂d/∂P_i = -B_i(t*)·n`; for a line,
// `B = (1 - u, u)`; for a circle (or a circular arc's in-sweep part) `d = |ρ - R|` with
// `ρ = |p - c|`, so `∂d = sgn(ρ - R)·(-u·∂c - ∂R)` with `u = (p - c)/ρ`, and the arc's centre
// `c` depends on its endpoints and radius through SVG's endpoint-to-centre conversion.
//
// These replace the central differences the solve first used (two exact distance
// evaluations per variable per point, each a Newton solve for a cubic: 16 per cubic row).
// They agree with them to the differences' own truncation and round-off;
// `analytic_gradients_match_central_differences` checks every case.
//
// Method from: the envelope theorem of parametric optimisation (the derivative of an
// optimal value is the partial derivative of the objective at the optimiser), as stated
// in Milgrom, Segal (2002), Envelope theorems for arbitrary choice sets, Econometrica
// 70(2), 583-601, doi:10.1111/1468-0262.00296; and the SVG 1.1 implementation notes,
// appendix F.6.5 (endpoint to centre parametrisation of an arc),
// <https://www.w3.org/TR/SVG11/implnote.html#ArcConversionEndpointToCenter>,
// differentiated by hand.
// ---------------------------------------------------------------------------------------

/// `a·x + b·y` as a vector.
fn lin(a: f64, x: Vec2, b: f64, y: Vec2) -> Vec2 {
    Vec2 {
        x: a * x.x + b * y.x,
        y: a * x.y + b * y.y,
    }
}

/// `s·v`.
fn scaled(s: f64, v: Vec2) -> Vec2 {
    Vec2 {
        x: s * v.x,
        y: s * v.y,
    }
}

/// The unit vector from `foot` to `p`, or `None` when they coincide (the distance has no
/// derivative there).
fn away(p: Point, foot: Point) -> Option<Vec2> {
    let v = p - foot;
    let l = v.norm();
    (l > 1e-12).then(|| scaled(1.0 / l, v))
}

/// The distance from `p` to the line segment `a -> b` and its gradients with respect to
/// `a` and `b`: `-(1 - u)·n` and `-u·n`, `u` the foot's clamped position. `None` when `p`
/// lies on the segment.
pub(super) fn line_grad(p: Point, a: Point, b: Point) -> Option<(f64, [Vec2; 2])> {
    let (d, u, foot) = point_segment(p, a, b);
    let n = away(p, foot)?;
    Some((d, [scaled(-(1.0 - u), n), scaled(-u, n)]))
}

/// The distance from `p` to the cubic Bézier `q` (foot found by [`cubic_dist`] from
/// `t_hint`, exactly as the solve measures it) and its gradients with respect to the four
/// control points, `-B_i(t*)·n` with `B_i` the cubic Bernstein polynomials. `None` when
/// `p` lies on the curve.
pub(super) fn cubic_grad(p: Point, q: [Point; 4], t_hint: f64) -> Option<(f64, [Vec2; 4])> {
    let (d, t) = cubic_dist(p, q, t_hint);
    let n = away(p, cubic_eval(&q, t).0)?;
    let mt = 1.0 - t;
    let b = [mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t];
    Some((d, b.map(|w| scaled(-w, n))))
}

/// The distance from `p` to a circle of centre `c` and radius `r` (`| |p - c| - r |`) and
/// its gradients with respect to `c` and `r`: `-sgn·u` and `-sgn`, `u = (p - c)/|p - c|`.
/// `None` at the centre or on the circle.
pub(super) fn circle_grad(p: Point, c: Point, r: f64) -> Option<(f64, Vec2, f64)> {
    let u = away(p, c)?;
    let rho = p.dist(c);
    if rho == r {
        return None;
    }
    let sgn = (rho - r).signum();
    Some(((rho - r).abs(), scaled(-sgn, u), -sgn))
}

/// The distance from `p` to the circular arc from `a` to `e` of radius `r` with SVG flags
/// `large` and `sweep` -- exactly [`arc_dist`]'s -- and its gradients with respect to `a`,
/// `e` and `r`.
///
/// Outside the sweep the distance is to the nearer endpoint, `-n` for that endpoint. Inside
/// it, `d = |ρ - R|` and the centre is `c = m + s·k·n̂` (SVG F.6.5 for a circle): `m` the
/// chord's midpoint, `hv = (a - e)/2` the half-chord of length `L` and direction `ĥ`,
/// `n̂ = (hv.y, -hv.x)/L`, `k = sqrt(R² - L²)`, `s = ±1` from the flags. Differentiating,
/// with `∂k/∂hv = -(L/k)·ĥ` and `∂n̂/∂hv = -ĥ·n̂ᵀ/L`:
///
/// `∂c/∂a = I/2 + (s/2)·(-(L/k)·n̂ ĥᵀ - (k/L)·ĥ n̂ᵀ)`, `∂c/∂e = I/2 - (same)`,
/// `∂c/∂R = s·(R/k)·n̂`, `∂R_drawn/∂R = 1`.
///
/// When the radius is too small for the chord (`L > R`) SVG draws the half circle on the
/// chord (`R_drawn = L`, `c = m`): `∂c/∂a = ∂c/∂e = I/2`, `∂R_drawn/∂a = ĥ/2`,
/// `∂R_drawn/∂e = -ĥ/2`, and the radius variable has no effect. `None` -- the caller then
/// uses finite differences -- for a non-positive radius, a degenerate chord, a half circle
/// to within `k < 1e-6·L` (where `∂c` grows without bound) and a point on the arc.
pub(super) fn circular_arc_grad(
    p: Point,
    a: Point,
    r: f64,
    large: bool,
    sweep: bool,
    e: Point,
) -> Option<(f64, [Vec2; 2], f64)> {
    if r <= 0.0 {
        return None;
    }
    let f = arc_ellipse_center(a, r, r, 0.0, large, sweep, e);
    let v = p - f.c;
    let rho = v.norm();
    let zero = Vec2 { x: 0.0, y: 0.0 };
    let in_sweep = f.delta != 0.0 && rho >= 1e-12 && {
        let ang = v.y.atan2(v.x);
        let tau = std::f64::consts::TAU;
        let s_ang = if f.delta > 0.0 {
            (ang - f.theta1).rem_euclid(tau)
        } else {
            (f.theta1 - ang).rem_euclid(tau)
        };
        s_ang <= f.delta.abs()
    };
    if !in_sweep {
        // The nearer endpoint, as `arc_dist` takes it (`min` keeps `a` on a tie).
        let (da, de) = (a.dist(p), e.dist(p));
        return if da <= de {
            Some((da, [scaled(-1.0, away(p, a)?), zero], 0.0))
        } else {
            Some((de, [zero, scaled(-1.0, away(p, e)?)], 0.0))
        };
    }
    if rho == f.rx {
        return None;
    }
    let sgn = (rho - f.rx).signum();
    let u = scaled(1.0 / rho, v);
    let (hx, hy) = ((a.x - e.x) * 0.5, (a.y - e.y) * 0.5);
    let l = hx.hypot(hy);
    if l < 1e-12 {
        return None;
    }
    let h_hat = Vec2 {
        x: hx / l,
        y: hy / l,
    };
    let half_u = scaled(-0.5, u);
    // Scaled up to the chord, as `arc_ellipse_center` decides it (phi = 0, rx = ry = r).
    if (hx * hx) / (r * r) + (hy * hy) / (r * r) > 1.0 {
        let ga = lin(sgn, half_u, -0.5 * sgn, h_hat);
        let ge = lin(sgn, half_u, 0.5 * sgn, h_hat);
        return Some(((rho - f.rx).abs(), [ga, ge], 0.0));
    }
    let k = (r * r - l * l).max(0.0).sqrt();
    if k < 1e-6 * l {
        return None;
    }
    let n_hat = Vec2 {
        x: hy / l,
        y: -hx / l,
    };
    let mid = Point::new((a.x + e.x) * 0.5, (a.y + e.y) * 0.5);
    let s = if (f.c - mid).dot(n_hat) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    // -u·∂c/∂a = -u/2 + (s/2)·((L/k)(u·n̂)·ĥ + (k/L)(u·ĥ)·n̂); the e side flips the bracket.
    let bracket = lin((l / k) * u.dot(n_hat), h_hat, (k / l) * u.dot(h_hat), n_hat);
    let ga = lin(sgn, half_u, 0.5 * s * sgn, bracket);
    let ge = lin(sgn, half_u, -0.5 * s * sgn, bracket);
    let gr = sgn * (-s * (r / k) * u.dot(n_hat) - 1.0);
    Some(((rho - f.rx).abs(), [ga, ge], gr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_and_cubic_distances_are_exact() {
        // Upper half circle of radius 5 round the origin, from (5,0) to (-5,0).
        let s = Segment::circular_arc(5.0, false, false, Point::new(-5.0, 0.0));
        let a = Point::new(5.0, 0.0);
        let d_in =
            arc_dist(Point::new(0.0, -7.0), a, &s).min(arc_dist(Point::new(0.0, 7.0), a, &s));
        assert!((d_in - 2.0).abs() < 1e-9, "{d_in}");
        // A straight cubic along x: distance is the height.
        let q = [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(3.0, 0.0),
        ];
        let (d, t) = cubic_dist(Point::new(1.5, 2.0), q, 0.3);
        assert!((d - 2.0).abs() < 1e-9 && (t - 0.5).abs() < 1e-6, "{d} {t}");
    }

    /// Central differences of `f` at `x` in each of its coordinates, step `h`.
    fn numeric_h(f: &dyn Fn(&[f64]) -> f64, x: &[f64], h: f64) -> Vec<f64> {
        (0..x.len())
            .map(|i| {
                let (mut p, mut m) = (x.to_vec(), x.to_vec());
                p[i] += h;
                m[i] -= h;
                (f(&p) - f(&m)) / (2.0 * h)
            })
            .collect()
    }

    /// Central differences of `f` at `x`, step 1e-6.
    fn numeric(f: &dyn Fn(&[f64]) -> f64, x: &[f64]) -> Vec<f64> {
        numeric_h(f, x, 1e-6)
    }

    /// `a` and `b` agree to `tol` relatively.
    fn close_to(a: &[f64], b: &[f64], tol: f64, what: &str) {
        for (x, y) in a.iter().zip(b) {
            assert!(
                (x - y).abs() < tol * (1.0 + y.abs()),
                "{what}: {a:?} vs {b:?}"
            );
        }
    }

    /// `a` and `b` agree to 1e-5 relatively.
    fn close(a: &[f64], b: &[f64], what: &str) {
        close_to(a, b, 1e-5, what);
    }

    #[test]
    fn analytic_gradients_match_central_differences() {
        let pt = |x: &[f64], i: usize| Point::new(x[i], x[i + 1]);
        let points = [
            Point::new(3.0, 4.5),
            Point::new(-2.0, 1.0),
            Point::new(12.0, -3.0),
            Point::new(5.5, 9.0),
        ];
        for &p in &points {
            // A line, its foot inside and clamped.
            let x = [0.0, 0.0, 10.0, 2.0];
            let f = |x: &[f64]| point_segment(p, pt(x, 0), pt(x, 2)).0;
            let (d, g) = line_grad(p, pt(&x, 0), pt(&x, 2)).expect("off the line");
            assert!((d - f(&x)).abs() < 1e-12);
            close(&[g[0].x, g[0].y, g[1].x, g[1].y], &numeric(&f, &x), "line");
            // A cubic.
            let x = [0.0, 0.0, 3.0, 6.0, 8.0, 6.0, 10.0, 0.0];
            let q = |x: &[f64]| [pt(x, 0), pt(x, 2), pt(x, 4), pt(x, 6)];
            let f = |x: &[f64]| cubic_dist(p, q(x), 0.5).0;
            let (d, g) = cubic_grad(p, q(&x), 0.5).expect("off the curve");
            assert!((d - f(&x)).abs() < 1e-12);
            let flat: Vec<f64> = g.iter().flat_map(|v| [v.x, v.y]).collect();
            close(&flat, &numeric(&f, &x), "cubic");
            // A circle.
            let x = [4.0, 3.0, 2.5];
            let f = |x: &[f64]| (pt(x, 0).dist(p) - x[2]).abs();
            let (_, gc, gr) = circle_grad(p, pt(&x, 0), x[2]).expect("off the circle");
            close(&[gc.x, gc.y, gr], &numeric(&f, &x), "circle");
            // Circular arcs: every flag pair, a radius that spans the chord and one that
            // SVG scales up to it. Scaled up, the drawn centre is the chord's midpoint only
            // up to `sqrt` of a rounding error (F.6.5's radicand is clamped at zero), which
            // makes differences at 1e-6 noisy at 1e-2; there they step 1e-3 and are held to
            // 1e-3.
            for (large, sweep) in [(false, false), (false, true), (true, false), (true, true)] {
                for (radius, step, tol) in [(7.0, 1e-6, 1e-5), (3.0, 1e-3, 1e-3)] {
                    let x = [1.0, 2.0, 9.0, 4.0, radius];
                    let seg = |x: &[f64]| Segment::circular_arc(x[4], large, sweep, pt(x, 2));
                    let f = |x: &[f64]| arc_dist(p, pt(x, 0), &seg(x));
                    let (d, g, gr) = circular_arc_grad(p, pt(&x, 0), x[4], large, sweep, pt(&x, 2))
                        .expect("a regular arc");
                    assert!((d - f(&x)).abs() < 1e-12, "{d} vs {}", f(&x));
                    close_to(
                        &[g[0].x, g[0].y, g[1].x, g[1].y, gr],
                        &numeric_h(&f, &x, step),
                        tol,
                        &format!("arc {large} {sweep} r {radius} p {p:?}"),
                    );
                }
            }
        }
        // Degenerate cases ask for finite differences.
        let (o, q) = (Point::new(0.0, 0.0), Point::new(2.0, 0.0));
        assert!(circular_arc_grad(points[0], o, 0.0, false, true, points[1]).is_none());
        assert!(line_grad(Point::new(1.0, 0.0), o, q).is_none());
    }
}
