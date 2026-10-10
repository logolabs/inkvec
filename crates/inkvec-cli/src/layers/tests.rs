//! The stage on hand-built faces: what it completes, and what it must refuse.

use super::*;

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

/// A closed polygon as one fitted edge of lines.
fn polygon(pts: &[(f64, f64)]) -> FittedPath {
    let mut segs: Vec<Segment> = pts[1..].iter().map(|&(x, y)| Segment::Line(p(x, y))).collect();
    segs.push(Segment::Line(p(pts[0].0, pts[0].1)));
    FittedPath {
        start: p(pts[0].0, pts[0].1),
        segments: segs,
        closed: true,
    }
}

/// Runs the stage on faces given as one polygon each, painted in the given ranks.
fn run(
    faces: &[Vec<(f64, f64)>],
    rank: &[Option<usize>],
    candidate: &[bool],
    strokes: &[StrokeShape],
) -> HashMap<usize, Completion> {
    let fitted: Vec<FittedPath> = faces.iter().map(|f| polygon(f)).collect();
    let order: Vec<FaceRings> = (0..faces.len()).map(|k| vec![vec![(k, false)]]).collect();
    let prims = vec![None; faces.len()];
    let outer = vec![vec![0usize]; faces.len()];
    let holes = vec![Vec::new(); faces.len()];
    let covers = vec![true; faces.len()];
    let decimals = 2;
    let cost = |f: usize| -> f64 {
        let mut d = String::new();
        crate::pathdata::fmt_ring(&order[f][0], &fitted, decimals, &mut d);
        gate_count(&d)
    };
    let inp = Input {
        order: &order,
        fitted: &fitted,
        prims: &prims,
        outer: &outer,
        holes: &holes,
        rank,
        covers: &covers,
        candidate,
        cost_now: &cost,
        strokes,
        decimals,
    };
    complete(&inp)
}

#[test]
fn an_l_under_the_square_in_its_corner_completes_to_the_whole_square() {
    // Face 0 shows an L; face 1, the square in its top-right corner, is painted over it.
    let l = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (20.0, 10.0),
        (20.0, 20.0),
        (0.0, 20.0),
    ];
    let corner = vec![(10.0, 0.0), (20.0, 0.0), (20.0, 10.0), (10.0, 10.0)];
    let got = run(&[l, corner], &[Some(0), Some(1)], &[true, false], &[]);
    let c = got.get(&0).expect("the L is completed");
    assert!(c.after < c.before, "{c:?}");
    assert!(c.after <= 6.0, "a square writes six numbers: {c:?}");
}

#[test]
fn nothing_painted_over_a_face_leaves_it_alone() {
    let a = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let b = vec![(20.0, 0.0), (30.0, 0.0), (30.0, 10.0), (20.0, 10.0)];
    let got = run(&[a, b], &[Some(0), Some(1)], &[true, true], &[]);
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn a_face_painted_over_a_lower_one_cannot_reach_into_it() {
    // Face 1 (upper) is the L; face 0 (lower) the corner square. The L may not grow into
    // the square, which is painted beneath it, and nothing is painted over the L.
    let l = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (20.0, 10.0),
        (20.0, 20.0),
        (0.0, 20.0),
    ];
    let corner = vec![(10.0, 0.0), (20.0, 0.0), (20.0, 10.0), (10.0, 10.0)];
    let got = run(&[corner, l], &[Some(0), Some(1)], &[false, true], &[]);
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn a_face_under_a_stroke_reaches_under_it() {
    // A square whose right side is a staircase under a thick vertical stroke: completed
    // to a square that runs under the stroke.
    let stairs = vec![
        (0.0, 0.0),
        (20.0, 0.0),
        (20.5, 2.0),
        (19.5, 4.0),
        (20.5, 6.0),
        (19.5, 8.0),
        (20.0, 10.0),
        (20.0, 20.0),
        (0.0, 20.0),
    ];
    let stroke = StrokeShape {
        lines: vec![FittedPath {
            start: p(20.0, -2.0),
            segments: vec![Segment::Line(p(20.0, 22.0))],
            closed: false,
        }],
        prims: Vec::new(),
        width: 4.0,
        cap: EndCap::Butt,
        join: JoinKind::Round,
    };
    let got = run(&[stairs], &[Some(0)], &[true], &[stroke]);
    let c = got.get(&0).expect("completed under the stroke");
    assert!(c.after < c.before, "{c:?}");
}

#[test]
fn the_check_refuses_a_shape_that_loses_what_the_face_shows() {
    let v = Region::from_polygons(&[vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)]], Rule::EvenOdd);
    let small = Region::from_polygons(&[vec![p(0.0, 0.0), p(9.0, 0.0), p(9.0, 10.0), p(0.0, 10.0)]], Rule::EvenOdd);
    let big = Region::from_polygons(&[vec![p(-1.0, -1.0), p(11.0, -1.0), p(11.0, 11.0), p(-1.0, 11.0)]], Rule::EvenOdd);
    assert!(!check::certify_completion(&v, &small, &big));
    assert!(check::certify_completion(&v, &v, &big));
    assert!(!check::certify_completion(&v, &big, &v));
}
