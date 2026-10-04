//! The symmetric fit against boundaries built exactly symmetric, closed and open, crossing
//! the axis at a point or between two, under one mirror and two.

use super::*;

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

fn cfg() -> FitConfig {
    FitConfig::from_precision(32.0, 0.1, 2.0)
}

fn plain(poly: &Polyline) -> FittedPath {
    multimodel::optimal_multimodel(poly, &cfg())
}

/// The closed outline of the axis-aligned rectangle `[x0, x1] × [y0, y1]`, sampled every
/// `step` px clockwise from its top-left corner, then rotated to start `rot` points later
/// (so the pairing's shift is not trivially zero).
fn rect_ring(x0: f64, y0: f64, x1: f64, y1: f64, step: f64, rot: usize) -> Polyline {
    let mut pts = Vec::new();
    let edge = |a: Point, b: Point, pts: &mut Vec<Point>| {
        let n = ((b.x - a.x).abs().max((b.y - a.y).abs()) / step).round() as usize;
        for i in 0..n {
            let t = i as f64 / n as f64;
            pts.push(p(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
        }
    };
    edge(p(x0, y0), p(x1, y0), &mut pts);
    edge(p(x1, y0), p(x1, y1), &mut pts);
    edge(p(x1, y1), p(x0, y1), &mut pts);
    edge(p(x0, y1), p(x0, y0), &mut pts);
    let len = pts.len();
    pts.rotate_left(rot % len);
    let n = pts.len();
    Polyline::new(pts, vec![0.05; n], true)
}

/// The fitted path's anchors: its start and every segment's end.
fn anchors(path: &FittedPath) -> Vec<Point> {
    std::iter::once(path.start)
        .chain(path.segments.iter().map(Segment::end))
        .collect()
}

/// Every anchor's reflection is an anchor too (to `tol`).
/// Segments compared by their debug form (they carry no `PartialEq`).
fn same(a: &Option<Segment>, b: &Option<Segment>) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

fn anchors_symmetric(path: &FittedPath, m: Mirror, tol: f64) -> bool {
    let a = anchors(path);
    a.iter()
        .all(|&q| a.iter().any(|&r| r.dist(m.point(q)) <= tol))
}

#[test]
fn a_symmetric_ring_comes_back_exactly_symmetric_and_whole() {
    // Axis x = 10 (V(20)). The ring crosses it at (10, 2) and (10, 14), both sample points.
    let m = Mirror::V(20);
    for rot in [0, 3, 17, 40] {
        let ring = rect_ring(4.0, 2.0, 16.0, 14.0, 0.5, rot);
        let path = fit(&ring, &[m], &cfg(), &plain).expect("the ring is its own mirror image");
        assert!(path.closed);
        assert!(
            anchors_symmetric(&path, m, 1e-9),
            "rot {rot}: {:?}",
            anchors(&path)
        );
        // A rectangle is four lines: the two lines across the axis were merged back.
        assert_eq!(path.segments.len(), 4, "rot {rot}: {:?}", path.segments);
        assert!(multimodel::path_max_deviation(&ring, &path) < 1e-6);
        // The ring closes on its own start.
        assert!(path.segments.last().unwrap().end().dist(path.start) < 1e-12);
    }
}

#[test]
fn a_ring_crossing_the_axis_between_two_points_is_split_at_the_middle() {
    // Axis x = 10 (V(20)), samples at x = 4.25, 4.75, ...: they straddle the axis, so the
    // ring crosses it in the middle of a piece at the top and at the bottom (a mirror's
    // axis is always at a whole or half pixel; the samples are what fall between).
    let m = Mirror::V(20);
    let ring = rect_ring(4.25, 2.0, 15.75, 14.0, 0.5, 5);
    let h = half(&ring, m).expect("a half");
    assert_eq!(h.points[0].x, 10.0);
    assert_eq!(h.points.last().unwrap().x, 10.0);
    let path = fit(&ring, &[m], &cfg(), &plain).expect("symmetric");
    assert!(anchors_symmetric(&path, m, 1e-9), "{:?}", anchors(&path));
    assert_eq!(path.segments.len(), 4);
}

#[test]
fn a_ring_symmetric_under_both_mirrors_is_fitted_as_a_quarter() {
    let (v, hm) = (Mirror::V(20), Mirror::H(16));
    let ring = rect_ring(4.0, 2.0, 16.0, 14.0, 0.5, 9);
    let path = fit(&ring, &[v, hm], &cfg(), &plain).expect("symmetric");
    assert!(anchors_symmetric(&path, v, 1e-9));
    assert!(anchors_symmetric(&path, hm, 1e-9));
    assert_eq!(path.segments.len(), 4);
}

#[test]
fn a_ring_that_is_not_its_own_mirror_image_is_left_to_the_ordinary_fit() {
    let mut ring = rect_ring(4.0, 2.0, 16.0, 14.0, 0.5, 0);
    ring.points[7].y += 0.01;
    assert!(fit(&ring, &[Mirror::V(20)], &cfg(), &plain).is_none());
    // Nor under a mirror whose axis it does not straddle symmetrically.
    let ring = rect_ring(4.0, 2.0, 16.0, 14.0, 0.5, 0);
    assert!(fit(&ring, &[Mirror::V(22)], &cfg(), &plain).is_none());
    // Too short to have a half worth fitting.
    let tiny = Polyline::new(
        vec![p(9.0, 1.0), p(11.0, 1.0), p(10.0, 2.0)],
        vec![0.05; 3],
        true,
    );
    assert!(fit(&tiny, &[Mirror::V(20)], &cfg(), &plain).is_none());
}

#[test]
fn an_open_symmetric_boundary_keeps_its_junctions_and_is_symmetric() {
    // A parabola from junction (2, 5) to junction (18, 5) through (10, 1): its own mirror
    // image under V(20), reversed. 33 points, so the middle one is on the axis.
    let m = Mirror::V(20);
    let pts: Vec<Point> = (0..33)
        .map(|i| {
            let x = 2.0 + 0.5 * i as f64;
            p(x, 1.0 + (x - 10.0) * (x - 10.0) / 16.0)
        })
        .collect();
    let poly = Polyline::new(pts.clone(), vec![0.05; 33], false);
    let path = fit(&poly, &[m], &cfg(), &plain).expect("symmetric");
    assert!(!path.closed);
    assert_eq!(path.start, pts[0]);
    assert_eq!(path.segments.last().unwrap().end(), pts[32]);
    assert!(anchors_symmetric(&path, m, 1e-9), "{:?}", anchors(&path));
    // Within the fitter's own tolerance of the points (three sigma).
    assert!(multimodel::path_max_deviation(&poly, &path) < 0.15);
    // The ordinary fit of this boundary is already symmetric (the program found the
    // symmetric optimum): `choose` keeps it as it is.
    let ordinary = plain(&poly);
    assert!(anchors_mirror(&ordinary, m));
    let chosen = choose(&poly, &[m], &cfg(), &plain);
    assert_eq!(format!("{chosen:?}"), format!("{ordinary:?}"));
    // An even count crosses between the two middle points.
    let even = Polyline::new(pts[..32].to_vec(), vec![0.05; 32], false);
    assert!(
        fit(&even, &[m], &cfg(), &plain).is_none(),
        "not symmetric about x = 10"
    );
    let shifted: Vec<Point> = (0..32)
        .map(|i| {
            let x = 2.25 + 0.5 * i as f64;
            p(x, 1.0 + (x - 10.0) * (x - 10.0) / 16.0)
        })
        .collect();
    let even = Polyline::new(shifted, vec![0.05; 32], false);
    let h = half(&even, Mirror::V(20)).expect("even count, symmetric about 10");
    assert_eq!(h.points.len(), 17);
    assert_eq!(h.points[16].x, 10.0);
}

#[test]
fn two_mirrored_lines_or_arcs_at_the_axis_merge_into_one() {
    // About the axis x = 10. Lines meeting the axis at a right angle.
    let a = Segment::Line(p(10.0, 3.0));
    let b = Segment::Line(p(16.0, 3.0));
    assert!(same(
        &merged(p(4.0, 3.0), &a, &b),
        &Some(Segment::Line(p(16.0, 3.0)))
    ));
    // At an angle the merged line is still square to the axis, through `from`'s height
    // (whether that is better is priced by the caller).
    let a = Segment::Line(p(10.0, 2.0));
    let b = Segment::Line(p(16.0, 3.0));
    assert!(same(
        &merged(p(4.0, 3.0), &a, &b),
        &Some(Segment::Line(p(16.0, 3.0)))
    ));
    // A quarter circle of radius 6 about (10, 8), from (4, 8) to (10, 2), and its mirror:
    // one half circle from (4, 8) to (16, 8).
    let a = Segment::circular_arc(6.0, false, true, p(10.0, 2.0));
    let b = Segment::circular_arc(6.0, false, true, p(16.0, 8.0));
    let Some(Segment::Arc {
        rx, large_arc, end, ..
    }) = merged(p(4.0, 8.0), &a, &b)
    else {
        panic!("two quarter circles are one half circle");
    };
    assert_eq!((rx, end), (6.0, p(16.0, 8.0)));
    // Exactly half a turn: not the large arc.
    assert!(!large_arc);
    // A line and an arc are not one segment.
    assert!(merged(p(4.0, 3.0), &Segment::Line(p(10.0, 3.0)), &b).is_none());
}

#[test]
fn a_cubic_nearly_square_to_the_axis_is_snapped_and_a_corner_is_not() {
    let m = Mirror::V(20);
    // Ends at (10, 2) arriving from the left, control point a hair above the normal.
    let part = FittedPath {
        start: p(4.0, 6.0),
        segments: vec![Segment::Cubic(p(5.0, 3.0), p(8.0, 2.1), p(10.0, 2.0))],
        closed: false,
    };
    let s = snap_cubic(&part, m, true).expect("within ten degrees");
    assert!(same(
        &Some(s.segments[0].clone()),
        &Some(Segment::Cubic(p(5.0, 3.0), p(8.0, 2.0), p(10.0, 2.0)))
    ));
    let steep = FittedPath {
        segments: vec![Segment::Cubic(p(5.0, 3.0), p(9.0, 1.0), p(10.0, 2.0))],
        ..part.clone()
    };
    assert!(snap_cubic(&steep, m, true).is_none());
    let line = FittedPath {
        segments: vec![Segment::Line(p(10.0, 2.0))],
        ..part
    };
    assert!(snap_cubic(&line, m, true).is_none());
}

#[test]
fn choose_takes_the_symmetric_fit_when_the_ordinary_one_is_asymmetric_and_dearer() {
    // An ordinary fitter that answers the closed ring with one needless, asymmetric vertex
    // (a collinear break on the left side): two parameters dearer and not symmetric.
    let m = Mirror::V(20);
    let ring = rect_ring(4.0, 2.0, 16.0, 14.0, 0.5, 0);
    let lopsided = |poly: &Polyline| -> FittedPath {
        let mut f = plain(poly);
        if poly.closed {
            let i = f
                .segments
                .iter()
                .position(|s| matches!(s, Segment::Line(q) if q.dist(p(4.0, 2.0)) < 1e-6))
                .expect("the left side ends at the top-left corner");
            f.segments.insert(i, Segment::Line(p(4.0, 5.0)));
        }
        f
    };
    let ordinary = lopsided(&ring);
    assert!(!anchors_mirror(&ordinary, m));
    let chosen = choose(&ring, &[m], &cfg(), &lopsided);
    assert!(anchors_mirror(&chosen, m));
    assert_eq!(chosen.segments.len(), 4);
}
