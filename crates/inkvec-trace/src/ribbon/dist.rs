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

use inkvec_core::Point;
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
}
