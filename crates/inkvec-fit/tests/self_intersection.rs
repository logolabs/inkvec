//! A fitted boundary must not cross itself.
//!
//! This is an editability property, not a fidelity one, and that is exactly why it needs
//! its own tests: the objective cannot see a crossing. Both curves still pass through
//! their measured points and the rendered pixels barely change, so nothing in
//! `0.5·chi² + λ·params` pushes back — while the resulting ring is invalid, resolved
//! arbitrarily by whichever fill rule applies, and unpleasant to edit.
//!
//! Measured over 180 real emoji, 53 (29%) emitted at least one self-intersecting ring
//! against VTracer's 10 (5.6%). Classified over 2126 rings: **zero** were a single cubic
//! looping, and all 40 were crossings between two different segments. Both cases are
//! covered here, the second because it is the one that occurs and the first because a
//! test for "does this path cross itself" that could not see a loop would be incomplete.

use inkvec_core::{Point, Polyline};
use inkvec_fit::curves::{cubic_self_intersects, Segment};
use inkvec_fit::simple::{fit_simple, self_crossing};
use inkvec_fit::{FitConfig, FittedPath};

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

// --- the single-cubic loop test ------------------------------------------------------

#[test]
fn a_plain_s_curve_is_clean() {
    assert!(!cubic_self_intersects(
        p(0.0, 0.0),
        p(30.0, 0.0),
        p(70.0, 100.0),
        p(100.0, 100.0)
    ));
}

#[test]
fn a_quarter_arc_is_clean() {
    const K: f64 = 0.552_284_749_830_793_4;
    let r = 40.0;
    assert!(!cubic_self_intersects(
        p(r, 0.0),
        p(r, K * r),
        p(K * r, r),
        p(0.0, r)
    ));
}

/// Cross-checked against dense sampling before being written down.
#[test]
fn crossed_arms_make_a_loop() {
    assert!(cubic_self_intersects(
        p(0.0, 0.0),
        p(80.0, 50.0),
        p(-70.0, 50.0),
        p(10.0, 0.0)
    ));
}

/// Arms that cross in the control polygon without folding the *curve* over itself.
/// Rejecting on the control polygon alone would discard good fits.
#[test]
fn crossed_control_arms_without_a_loop_are_kept() {
    assert!(!cubic_self_intersects(
        p(0.0, 0.0),
        p(100.0, 20.0),
        p(0.0, 20.0),
        p(100.0, 0.0)
    ));
}

/// A cubic whose endpoints coincide is a closed teardrop, and in this pipeline an edge may
/// legitimately start and end at the same junction. The crossing the algebra finds there
/// is at `t = 0, s = 1`, which is why the test is strict about being *inside* the span.
#[test]
fn a_closed_teardrop_is_not_a_defect() {
    assert!(!cubic_self_intersects(
        p(0.0, 0.0),
        p(120.0, 60.0),
        p(-120.0, 60.0),
        p(0.0, 0.0)
    ));
}

#[test]
fn the_closed_form_agrees_with_sampling() {
    fn brute(p0: Point, p1: Point, p2: Point, p3: Point) -> bool {
        const N: usize = 400;
        let at = |t: f64| {
            let u = 1.0 - t;
            let b = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            Point::new(
                b[0] * p0.x + b[1] * p1.x + b[2] * p2.x + b[3] * p3.x,
                b[0] * p0.y + b[1] * p1.y + b[2] * p2.y + b[3] * p3.y,
            )
        };
        let pts: Vec<Point> = (0..=N).map(|k| at(k as f64 / N as f64)).collect();
        for a in 0..pts.len() - 1 {
            for b in a + 2..pts.len() - 1 {
                if inkvec_core::predicates::segments_intersect(
                    pts[a],
                    pts[a + 1],
                    pts[b],
                    pts[b + 1],
                ) {
                    return true;
                }
            }
        }
        false
    }

    // The sweep must span both sides of the boundary, or it proves nothing.
    let mut loops = 0;
    for k in 0..16 {
        let arm = 20.0 + 30.0 * k as f64;
        let (p0, p1, p2, p3) = (
            p(0.0, 0.0),
            p(arm, 40.0),
            p(100.0 - arm, 40.0),
            p(100.0, 0.0),
        );
        assert_eq!(
            cubic_self_intersects(p0, p1, p2, p3),
            brute(p0, p1, p2, p3),
            "arm {arm}"
        );
        loops += cubic_self_intersects(p0, p1, p2, p3) as usize;
    }
    assert!(
        loops > 0,
        "the sweep never produced a loop, so it proved nothing about detecting one"
    );
}

// --- the crossing that actually occurs -----------------------------------------------

#[test]
fn a_simple_path_reports_no_crossing() {
    let path = FittedPath {
        start: p(0.0, 0.0),
        segments: vec![
            Segment::Line(p(10.0, 0.0)),
            Segment::Line(p(10.0, 10.0)),
            Segment::Line(p(0.0, 10.0)),
            Segment::Line(p(0.0, 0.0)),
        ],
        closed: true,
    };
    assert_eq!(self_crossing(&path), None);
}

/// Neighbouring segments meet at a shared endpoint by construction, and on a closed path
/// so do the first and last. Reporting those would make every path look invalid.
#[test]
fn touching_at_shared_endpoints_is_not_a_crossing() {
    let path = FittedPath {
        start: p(0.0, 0.0),
        segments: vec![
            Segment::Line(p(10.0, 0.0)),
            Segment::Line(p(5.0, 8.0)),
            Segment::Line(p(0.0, 0.0)),
        ],
        closed: true,
    };
    assert_eq!(self_crossing(&path), None);
}

/// A figure-of-eight: the two non-adjacent legs cross in the middle.
#[test]
fn two_segments_crossing_is_found() {
    let path = FittedPath {
        start: p(0.0, 0.0),
        segments: vec![
            Segment::Line(p(10.0, 10.0)),
            Segment::Line(p(0.0, 10.0)),
            Segment::Line(p(10.0, 0.0)),
        ],
        closed: false,
    };
    assert!(self_crossing(&path).is_some());
}

/// The repair has to terminate on any input, because at a span cap of one the fit
/// reproduces the measured contour, and a contour traced on a partition is simple.
///
/// The neck here is the case the corpus actually produces: two stretches of boundary
/// running close together, where independently fitted curves can bulge across each other.
#[test]
fn a_thin_neck_is_fitted_without_crossing() {
    // A narrow hairpin: out along y≈0, round the end, back along y≈1.2.
    let mut pts = Vec::new();
    for k in 0..=40 {
        pts.push(p(k as f64 * 2.0, 0.0));
    }
    for k in 1..8 {
        let a = std::f64::consts::PI * (k as f64 / 8.0) - std::f64::consts::FRAC_PI_2;
        pts.push(p(80.0 + 0.6 * a.cos(), 0.6 + 0.6 * a.sin()));
    }
    for k in (0..=40).rev() {
        pts.push(p(k as f64 * 2.0, 1.2));
    }
    let n = pts.len();
    let poly = Polyline {
        points: pts,
        sigma: vec![0.35; n],
        closed: false,
    };
    let cfg = FitConfig::from_precision(128.0, 0.1, 2.0);
    let fit = fit_simple(&poly, &cfg);
    assert_eq!(
        self_crossing(&fit),
        None,
        "the repair returned a path that still crosses itself"
    );
    assert!(!fit.segments.is_empty());
}

/// A boundary that is already simple must come back untouched — the repair must not cost
/// parameters on the overwhelming majority of inputs that never had a problem.
#[test]
fn an_already_simple_fit_is_not_disturbed() {
    let mut pts = Vec::new();
    for k in 0..=60 {
        let t = k as f64 / 60.0 * std::f64::consts::TAU;
        pts.push(p(40.0 * t.cos(), 40.0 * t.sin()));
    }
    let n = pts.len();
    let poly = Polyline {
        points: pts,
        sigma: vec![0.3; n],
        closed: true,
    };
    let cfg = FitConfig::from_precision(128.0, 0.1, 2.0);
    let plain = inkvec_fit::multimodel::optimal_multimodel(&poly, &cfg);
    let repaired = fit_simple(&poly, &cfg);
    assert_eq!(plain.segments.len(), repaired.segments.len());
    assert_eq!(self_crossing(&repaired), None);
}
