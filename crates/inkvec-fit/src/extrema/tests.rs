//! The extremum re-segmentation on curves whose answer is known: ellipses with their nodes
//! off the extrema, rings that are already right, straight runs, and a curve leaving a line
//! at a kink.

use super::*;

/// The standard cubic for the arc of the ellipse `(a cos θ, b sin θ) + c` from `t0` to `t1`
/// (radians, `|t1 − t0| ≤ π/2`): the circle's approximation, scaled.
fn ellipse_cubic(c: Point, a: f64, b: f64, t0: f64, t1: f64) -> Segment {
    let k = 4.0 / 3.0 * ((t1 - t0) / 4.0).tan();
    let at = |t: f64| Point::new(c.x + a * t.cos(), c.y + b * t.sin());
    let d = |t: f64| (-a * t.sin(), b * t.cos());
    let (p0, p3) = (at(t0), at(t1));
    let (d0, d1) = (d(t0), d(t1));
    Segment::Cubic(
        Point::new(p0.x + k * d0.0, p0.y + k * d0.1),
        Point::new(p3.x - k * d1.0, p3.y - k * d1.1),
        p3,
    )
}

/// `n` points round the ellipse, starting at angle `t_start`, uniform sigma.
fn ellipse_points(c: Point, a: f64, b: f64, n: usize, t_start: f64, sigma: f64) -> Polyline {
    let pts = (0..n)
        .map(|k| {
            let t = t_start + std::f64::consts::TAU * k as f64 / n as f64;
            Point::new(c.x + a * t.cos(), c.y + b * t.sin())
        })
        .collect();
    Polyline::with_uniform_sigma(pts, sigma, true)
}

/// The ellipse as four cubics whose nodes sit at 45, 135, 225 and 315 degrees: smooth, and
/// none of them at an extremum.
fn ellipse_off_extrema(c: Point, a: f64, b: f64) -> FittedPath {
    let q = std::f64::consts::FRAC_PI_4;
    let start = Point::new(c.x + a * q.cos(), c.y + b * q.sin());
    let segments = (0..4)
        .map(|k| {
            let t0 = q + k as f64 * 2.0 * q;
            ellipse_cubic(c, a, b, t0, t0 + 2.0 * q)
        })
        .collect();
    FittedPath {
        start,
        segments,
        closed: true,
    }
}

fn nodes_of(fp: &FittedPath) -> Vec<Point> {
    std::iter::once(fp.start)
        .chain(fp.segments.iter().map(Segment::end))
        .collect()
}

/// Every handle of every cubic, as `(node, control point)`.
fn handles(fp: &FittedPath) -> Vec<(Point, Point)> {
    let mut out = Vec::new();
    let mut cur = fp.start;
    for s in &fp.segments {
        if let Segment::Cubic(c1, c2, e) = *s {
            out.push((cur, c1));
            out.push((e, c2));
        }
        cur = s.end();
    }
    out
}

fn cfg() -> FitConfig {
    FitConfig::from_precision(512.0, 0.1, 2.0)
}

#[test]
fn an_ellipse_noded_off_its_extrema_is_renoded_on_them() {
    let c = Point::new(10.0, -5.0);
    let (a, b) = (60.0, 40.0);
    let poly = ellipse_points(c, a, b, 400, 0.3, 0.05);
    let mut fp = ellipse_off_extrema(c, a, b);
    let runs = anchor_extrema(&mut fp, &poly, &cfg(), FIT);
    assert_eq!(runs, 1, "the whole ring is one smooth run");
    assert_eq!(
        fp.segments.len(),
        4,
        "four cubics, as before: {:?}",
        fp.segments
    );
    assert!(fp.segments.iter().all(|s| matches!(s, Segment::Cubic(..))));
    // Every node at one of the four extrema, every handle on an axis.
    let extrema = [
        Point::new(c.x + a, c.y),
        Point::new(c.x, c.y + b),
        Point::new(c.x - a, c.y),
        Point::new(c.x, c.y - b),
    ];
    for p in nodes_of(&fp) {
        let d = extrema
            .iter()
            .map(|e| e.dist(p))
            .fold(f64::INFINITY, f64::min);
        assert!(d < 0.2, "node {p:?} is {d:.3} px from the nearest extremum");
    }
    for (node, h) in handles(&fp) {
        let (dx, dy) = (h.x - node.x, h.y - node.y);
        assert!(
            dx.abs() < 1e-9 || dy.abs() < 1e-9,
            "handle {dx:.4},{dy:.4} at {node:?} is off the axes"
        );
    }
    // And it still draws the ellipse.
    let dev = poly
        .points
        .iter()
        .map(|p| {
            let mut best = f64::INFINITY;
            let mut cur = fp.start;
            for s in &fp.segments {
                best = best.min(dist2_to(s, cur, *p));
                cur = s.end();
            }
            best.sqrt()
        })
        .fold(0.0, f64::max);
    assert!(dev < 0.05, "strays {dev:.4} px from the ellipse");
}

#[test]
fn a_ring_already_on_its_extrema_keeps_its_nodes() {
    let c = Point::new(0.0, 0.0);
    let (a, b) = (50.0, 30.0);
    let poly = ellipse_points(c, a, b, 360, 0.0, 0.05);
    let half = std::f64::consts::FRAC_PI_2;
    let mut fp = FittedPath {
        start: Point::new(a, 0.0),
        segments: (0..4)
            .map(|k| ellipse_cubic(c, a, b, k as f64 * half, (k + 1) as f64 * half))
            .collect(),
        closed: true,
    };
    let before = nodes_of(&fp);
    anchor_extrema(&mut fp, &poly, &cfg(), FIT);
    let after = nodes_of(&fp);
    assert_eq!(after.len(), before.len());
    for p in after {
        let d = before
            .iter()
            .map(|q| q.dist(p))
            .fold(f64::INFINITY, f64::min);
        assert!(d < 0.2, "node moved {d:.3} px");
    }
}

#[test]
fn straight_runs_are_left_alone() {
    let pts: Vec<Point> = (0..50)
        .map(|k| Point::new(k as f64, 0.5 * k as f64))
        .collect();
    let poly = Polyline::with_uniform_sigma(pts, 0.05, false);
    let mut fp = FittedPath {
        start: Point::new(0.0, 0.0),
        segments: vec![
            Segment::Line(Point::new(20.0, 10.0)),
            Segment::Line(Point::new(49.0, 24.5)),
        ],
        closed: false,
    };
    assert_eq!(anchor_extrema(&mut fp, &poly, &cfg(), FIT), 0);
    assert_eq!(fp.segments.len(), 2);
}

#[test]
fn a_curve_leaving_a_line_at_a_kink_is_made_to_leave_along_it() {
    // A horizontal line into a quarter circle tangent to it: smooth in truth. The fitted
    // cubic leaves the line 6 degrees off.
    let r = 40.0;
    let mut pts: Vec<Point> = (0..40).map(|k| Point::new(-40.0 + k as f64, 0.0)).collect();
    let n_arc = 63;
    for k in 0..=n_arc {
        let t = std::f64::consts::FRAC_PI_2 * k as f64 / n_arc as f64;
        pts.push(Point::new(r * t.sin(), r - r * t.cos()));
    }
    let poly = Polyline::with_uniform_sigma(pts, 0.05, false);
    let (k, off) = (0.5523 * r, 6f64.to_radians());
    let good = Segment::Cubic(Point::new(k, 0.0), Point::new(r, r - k), Point::new(r, r));
    let kinked = Segment::Cubic(
        Point::new(k * off.cos(), k * off.sin()),
        Point::new(r, r - k),
        Point::new(r, r),
    );
    let mut fp = FittedPath {
        start: Point::new(-40.0, 0.0),
        segments: vec![Segment::Line(Point::new(0.0, 0.0)), kinked],
        closed: false,
    };
    anchor_extrema(&mut fp, &poly, &cfg(), FIT);
    let Segment::Cubic(c1, _, _) = fp.segments[1] else {
        panic!("the curve stays a cubic: {:?}", fp.segments);
    };
    assert!(c1.y.abs() < 1e-9, "leaves the line along it now: {c1:?}");
    let Segment::Cubic(g1, ..) = good else {
        unreachable!()
    };
    assert!(
        (c1.x - g1.x).abs() < 1.0,
        "with about the arc's own handle: {c1:?}"
    );
}

#[test]
fn nodes_map_to_increasing_measured_points_round_a_closed_ring() {
    let c = Point::new(0.0, 0.0);
    let poly = ellipse_points(c, 30.0, 20.0, 200, 1.0, 0.05);
    let fp = ellipse_off_extrema(c, 30.0, 20.0);
    let starts = nodes_of(&fp);
    let u = node_indices(&poly, &fp, &starts).expect("every node is on the ring");
    assert_eq!(u.len(), 5);
    assert!(u.windows(2).all(|w| w[1] > w[0]));
    assert_eq!(u[4], u[0] + poly.len(), "the ring closes one turn later");
    // A node nowhere near the points maps to nothing.
    let mut far = fp.clone();
    far.start = Point::new(500.0, 500.0);
    assert!(node_indices(&poly, &far, &nodes_of(&far)).is_none());
}

#[test]
fn extrema_inside_a_cubic_and_an_arc() {
    // A cubic from (0,0) bulging to x = 3/4 at t = 1/2 and back: one x extremum, mid-way.
    let cub = Segment::Cubic(
        Point::new(1.0, 1.0),
        Point::new(1.0, 2.0),
        Point::new(0.0, 3.0),
    );
    let ext = interior_extrema(&cub, Point::new(0.0, 0.0));
    assert_eq!(ext.len(), 1);
    assert!((ext[0].0 - 0.5).abs() < 1e-12 && ext[0].1 == Axis::X);
    // A half circle from the bottom through the right to the top: its x extremum (at the
    // right) is the one inside; the ends are the y extrema.
    let arc = Segment::circular_arc(10.0, false, false, Point::new(0.0, -10.0));
    let ext = interior_extrema(&arc, Point::new(0.0, 10.0));
    assert_eq!(ext.len(), 1, "{ext:?}");
    assert_eq!(ext[0].1, Axis::X);
    assert!((ext[0].0 - 0.5).abs() < 1e-9);
    // And the roots themselves.
    assert_eq!(quadratic_roots(0.0, 0.0, 1.0), Vec::<f64>::new());
    assert_eq!(quadratic_roots(0.0, 2.0, -1.0), vec![0.5]);
    let mut r = quadratic_roots(1.0, -3.0, 2.0);
    r.sort_by(f64::total_cmp);
    assert_eq!(r, vec![1.0, 2.0]);
}

#[test]
fn distance_to_an_arc_is_through_its_centre_inside_and_to_its_ends_outside() {
    // The upper half of the circle of radius 10 round the origin, from (10, 0) to (-10, 0).
    let arc = Segment::circular_arc(10.0, false, true, Point::new(-10.0, 0.0));
    let start = Point::new(10.0, 0.0);
    let mid = eval_segment(&arc, start, 0.5);
    let outward = Point::new(mid.x * 1.2, mid.y * 1.2);
    assert!((dist2_to(&arc, start, outward).sqrt() - 2.0).abs() < 1e-9);
    // Opposite the middle, the nearest point of the arc is an end.
    let opposite = Point::new(-mid.x, -mid.y);
    let d = dist2_to(&arc, start, opposite).sqrt();
    assert!(
        (d - opposite
            .dist(start)
            .min(opposite.dist(Point::new(-10.0, 0.0))))
        .abs()
            < 1e-9
    );
}
