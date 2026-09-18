//! Comprehensive unit tests for structural simplification (crates/inkvec-fit/src/structural.rs).

use inkvec_core::{Point, Polyline};
use inkvec_fit::curves::{eval_cubic, Segment};
use inkvec_fit::structural::{
    generate_replacements, max_deviation_to_samples, sample_run_uniform,
    simplify_path_structural, simplify_with_poly, vector_angle, StructuralConfig,
};
use inkvec_fit::{FitConfig, FittedPath};

#[test]
fn test_vector_angle() {
    use inkvec_core::Vec2;
    let right = Vec2 { x: 1.0, y: 0.0 };
    let up = Vec2 { x: 0.0, y: 1.0 };
    let left = Vec2 { x: -1.0, y: 0.0 };

    let ang90 = vector_angle(right, up);
    assert!((ang90 - std::f64::consts::FRAC_PI_2).abs() < 1e-9);

    let ang180 = vector_angle(right, left);
    assert!((ang180 - std::f64::consts::PI).abs() < 1e-9);

    let ang0 = vector_angle(right, right);
    assert!(ang0.abs() < 1e-9);
}

#[test]
fn test_collinear_chords_simplify_to_one_line() {
    // 5 short line segments along y = 2x from (0,0) to (50, 100)
    let segs = vec![
        Segment::Line(Point::new(10.0, 20.0)),
        Segment::Line(Point::new(20.0, 40.0)),
        Segment::Line(Point::new(30.0, 60.0)),
        Segment::Line(Point::new(40.0, 80.0)),
        Segment::Line(Point::new(50.0, 100.0)),
    ];

    let mut path = FittedPath {
        start: Point::new(0.0, 0.0),
        segments: segs,
        closed: false,
    };

    let cfg = StructuralConfig::default();
    let eliminated = simplify_path_structural(&mut path, &cfg);

    assert!(eliminated >= 3, "Eliminated {eliminated} segments");
    assert_eq!(path.segments.len(), 1, "Should collapse into single line");
    assert_eq!(path.start, Point::new(0.0, 0.0));
    assert_eq!(path.end(), Point::new(50.0, 100.0));
    assert!(matches!(path.segments[0], Segment::Line(_)));
}

#[test]
fn test_circular_arc_recovery() {
    // Quarter circle arc from (50, 0) to (0, 50), radius 50, center (0, 0)
    // Discretized into 6 small chords
    let mut segs = Vec::new();
    let steps = 6;
    for i in 1..=steps {
        let theta = (i as f64 / steps as f64) * std::f64::consts::FRAC_PI_2;
        let x = 50.0 * theta.cos();
        let y = 50.0 * theta.sin();
        segs.push(Segment::Line(Point::new(x, y)));
    }

    let mut path = FittedPath {
        start: Point::new(50.0, 0.0),
        segments: segs,
        closed: false,
    };

    let cfg = StructuralConfig {
        max_dist: 0.6,
        lambda: 1.0,
        ..Default::default()
    };

    let eliminated = simplify_path_structural(&mut path, &cfg);
    assert!(eliminated >= 4, "Eliminated {eliminated} segments");
    assert!(path.segments.len() <= 2, "Length is {}", path.segments.len());

    // Should have fitted a circular arc or a high-accuracy cubic
    let has_arc = path.segments.iter().any(|s| matches!(s, Segment::Arc { .. }));
    let has_cubic = path.segments.iter().any(|s| matches!(s, Segment::Cubic(..)));
    assert!(has_arc || has_cubic);
}

#[test]
fn test_s_curve_c1_cubic_fit() {
    // S-curve made of 8 short cubic or line fragments
    // Ground truth: cubic from (0,0) with handles (0, 20) and (40, 20) to (40, 40)
    let p0 = Point::new(0.0, 0.0);
    let p1 = Point::new(0.0, 20.0);
    let p2 = Point::new(40.0, 20.0);
    let p3 = Point::new(40.0, 40.0);

    let mut segs = Vec::new();
    let count = 8;
    for i in 1..=count {
        let t = i as f64 / count as f64;
        let pt = eval_cubic([p0, p1, p2, p3], t);
        segs.push(Segment::Line(pt));
    }

    let mut path = FittedPath {
        start: p0,
        segments: segs,
        closed: false,
    };

    let cfg = StructuralConfig {
        max_dist: 0.6,
        lambda: 1.0,
        ..Default::default()
    };

    let eliminated = simplify_path_structural(&mut path, &cfg);
    assert!(eliminated >= 5, "Eliminated {eliminated} segments");
    assert!(path.segments.len() <= 2, "Path segments: {}", path.segments.len());

    // Verify max deviation from original endpoints
    assert!((path.end().x - p3.x).abs() < 1e-6);
    assert!((path.end().y - p3.y).abs() < 1e-6);
}

#[test]
fn test_sharp_corner_is_not_flattened() {
    // A path that makes a sharp 90-degree turn:
    // (0, 0) -> (10, 0) -> (20, 0) -> (20, 10) -> (20, 20)
    let segs = vec![
        Segment::Line(Point::new(10.0, 0.0)),
        Segment::Line(Point::new(20.0, 0.0)),
        Segment::Line(Point::new(20.0, 10.0)),
        Segment::Line(Point::new(20.0, 20.0)),
    ];

    let mut path = FittedPath {
        start: Point::new(0.0, 0.0),
        segments: segs,
        closed: false,
    };

    let cfg = StructuralConfig {
        max_dist: 0.6, // tight tolerance prevents cutting across the 20x20 corner!
        ..Default::default()
    };

    simplify_path_structural(&mut path, &cfg);

    // The two horizontal segments should collapse to 1, and the two vertical segments to 1.
    // But the corner must remain! Length should be exactly 2.
    assert_eq!(path.segments.len(), 2, "Expected 2 segments (one per side of corner)");
    assert_eq!(path.segments[0].end(), Point::new(20.0, 0.0));
    assert_eq!(path.segments[1].end(), Point::new(20.0, 20.0));
}

#[test]
fn test_simplify_with_poly_contour() {
    // Test exact chi2 simplification on a contour
    let mut pts = Vec::new();
    let mut sig = Vec::new();
    for i in 0..=30 {
        let t = i as f64 / 30.0;
        pts.push(Point::new(t * 30.0, (t * 30.0) * 0.5));
        sig.push(0.05);
    }
    let poly = Polyline {
        points: pts,
        sigma: sig,
        closed: false,
    };

    let segs = vec![
        Segment::Line(Point::new(10.0, 5.0)),
        Segment::Line(Point::new(20.0, 10.0)),
        Segment::Line(Point::new(30.0, 15.0)),
    ];
    let mut path = FittedPath {
        start: Point::new(0.0, 0.0),
        segments: segs,
        closed: false,
    };
    let vertices = vec![0, 10, 20, 30];
    let fit_cfg = FitConfig {
        tau: 2.0,
        lambda: 1.0,
    };

    let elim = simplify_with_poly(&mut path, &poly, &vertices, &fit_cfg);
    assert_eq!(elim, 2);
    assert_eq!(path.segments.len(), 1);
    assert_eq!(path.end(), Point::new(30.0, 15.0));
}

#[test]
fn test_generate_replacements_and_sampling() {
    let p0 = Point::new(0.0, 0.0);
    let p1 = Point::new(10.0, 0.0);
    let p2 = Point::new(20.0, 0.0);
    let segs = vec![Segment::Line(p1), Segment::Line(p2)];

    let (samples, len) = sample_run_uniform(p0, &segs, 50).expect("Sampling should succeed");
    assert_eq!(samples.len(), 50);
    assert!((len - 20.0).abs() < 1e-6);

    let dev = max_deviation_to_samples(&samples, &samples);
    assert!(dev < 1e-9);

    let cfg = StructuralConfig::default();
    let repls = generate_replacements(p0, &segs, None, None, &cfg);
    assert!(!repls.is_empty(), "Should generate at least a line replacement");
    assert!(matches!(repls[0], Segment::Line(_)));
}

#[test]
fn missing_contour_correspondence_does_not_change_the_objective() {
    let mut path = FittedPath {
        start: Point::new(0.0, 0.0),
        segments: vec![Segment::Line(Point::new(10.0, 0.0)),
                       Segment::Line(Point::new(20.0, 0.0))],
        closed: false,
    };
    let poly = Polyline { points: vec![path.start, path.end()],
                          sigma: vec![0.01; 2], closed: false };
    // The two indices no longer correspond to the two current segments.
    // Previously this silently selected a geometry-only simplifier instead.
    let eliminated = simplify_with_poly(&mut path, &poly, &[0, 1],
                                        &FitConfig { tau: 2.0, lambda: 1.0 });
    assert_eq!(eliminated, 0);
    assert_eq!(path.segments.len(), 2);
    assert_eq!(path.end(), Point::new(20.0, 0.0));
}

#[test]
fn closed_contour_wrap_does_not_disable_other_valid_spans() {
    let mut path = FittedPath {
        start: Point::new(0.0, 0.0),
        segments: vec![Segment::Line(Point::new(10.0, 0.0)),
                       Segment::Line(Point::new(20.0, 0.0)),
                       Segment::Line(Point::new(20.0, 20.0)),
                       Segment::Line(Point::new(0.0, 0.0))],
        closed: true,
    };
    let poly = Polyline { points: vec![path.start, Point::new(10.0, 0.0),
                                      Point::new(20.0, 0.0), Point::new(20.0, 20.0)],
                          sigma: vec![0.01; 4], closed: true };
    let eliminated = simplify_with_poly(&mut path, &poly, &[0, 1, 2, 3, 0],
                                        &FitConfig { tau: 2.0, lambda: 1.0 });
    assert!(eliminated >= 1);
    assert_eq!(path.start, path.end());
    assert!(path.closed);
}

#[test]
fn one_sided_distance_cannot_erase_a_protrusion() {
    // The candidate follows the bottom of the contour perfectly, but omits
    // the narrow upward excursion. Both directions of evidence are required.
    use inkvec_fit::structural::is_deviation_within;
    let contour = vec![Point::new(0.0,0.0), Point::new(9.0,0.0),
        Point::new(10.0,3.0), Point::new(11.0,0.0), Point::new(20.0,0.0)];
    let line = vec![Point::new(0.0,0.0), Point::new(20.0,0.0)];
    assert!(is_deviation_within(&line, &contour, 0.85));
    assert!(!is_deviation_within(&contour, &line, 0.85));
}

#[test]
fn geometrically_closed_path_has_the_same_seam_guard_as_flagged_closed() {
    let points = [Point::new(0.0,0.0), Point::new(10.0,0.1),
        Point::new(20.0,0.0), Point::new(20.0,20.0),
        Point::new(10.0,20.1), Point::new(0.0,20.0), Point::new(0.0,0.0)];
    let mut flagged = FittedPath { start:points[0], segments:points[1..].iter().copied().map(Segment::Line).collect(), closed:true };
    let mut geometric = flagged.clone(); geometric.closed=false;
    let cfg=StructuralConfig::default();
    simplify_path_structural(&mut flagged,&cfg);
    simplify_path_structural(&mut geometric,&cfg);
    assert_eq!(format!("{:?}",flagged.segments),format!("{:?}",geometric.segments));
}
