//! Adversarial empirical challenge suite for degenerate geometry and numerical singularities.
//!
//! Stress-tests:
//! 1. Coincident Bézier control points, retrograde segments, cusps, and extreme vectors.
//! 2. Zero-length, subnormal, overflowing, and non-finite tangent angles (`turn_angle`, `vector_angle`).
//! 3. Empty, single-point, identical, and massive point polyline decimation (`decimate::indices`).
//! 4. Empty, single-point, and non-finite arc length cumulative sums (`arc_lengths`).
//! 5. Collinear total-least-squares, extreme coordinates, and singular eigensolves (`scatter_min_eigen`).
//! 6. Levien quartic moment solves under extreme/singular inputs (`arms_from_moments`).
//! 7. Singular elliptical arc center parameterizations (`arc_ellipse_center`).
//! 8. End-to-end fitting pipelines (`fit_path`, `optimal_multimodel`) on degenerate polylines.

use crate::candidates::{arms_from_moments, scatter_min_eigen};
use crate::curves::{
    arc_ellipse_center, cubic_self_intersects, cubic_tangent, eval_cubic, Segment,
};
use crate::decimate;
use crate::multimodel::optimal_multimodel;
use crate::structural::vector_angle;
use crate::tangents::{arc_lengths, turn_angle};
use crate::{fit_path, FitConfig};
use inkvec_core::{Point, Polyline, Vec2};

#[test]
fn challenge_bezier_eval_and_intersections_stress() {
    // 1. All identical control points
    let p_zero = Point::new(0.0, 0.0);
    assert!(!cubic_self_intersects(p_zero, p_zero, p_zero, p_zero));
    for step in 0..=10 {
        let t = step as f64 / 10.0;
        let pt = eval_cubic([p_zero, p_zero, p_zero, p_zero], t);
        let tan = cubic_tangent([p_zero, p_zero, p_zero, p_zero], t);
        assert_eq!(pt, p_zero);
        assert_eq!(tan, Vec2 { x: 0.0, y: 0.0 });
    }

    // 2. Collinear points along diagonal
    let p0 = Point::new(0.0, 0.0);
    let p1 = Point::new(10.0, 10.0);
    let p2 = Point::new(20.0, 20.0);
    let p3 = Point::new(30.0, 30.0);
    assert!(!cubic_self_intersects(p0, p1, p2, p3));
    let mid = eval_cubic([p0, p1, p2, p3], 0.5);
    assert!((mid.x - 15.0).abs() < 1e-12 && (mid.y - 15.0).abs() < 1e-12);

    // 3. Interior loop with self-intersection
    let l0 = Point::new(0.0, 0.0);
    let l1 = Point::new(100.0, 50.0);
    let l2 = Point::new(-50.0, 50.0);
    let l3 = Point::new(50.0, 0.0);
    assert!(cubic_self_intersects(l0, l1, l2, l3));

    // Endpoints touching (closed loop) is intentionally NOT reported as self-intersection
    let cl0 = Point::new(0.0, 0.0);
    let cl1 = Point::new(20.0, 40.0);
    let cl2 = Point::new(-20.0, 40.0);
    let cl3 = Point::new(0.0, 0.0);
    assert!(!cubic_self_intersects(cl0, cl1, cl2, cl3));

    // 4. Extreme coordinate values
    let huge_p = Point::new(1e150, 1e150);
    let pt_huge = eval_cubic([huge_p, huge_p, huge_p, huge_p], 0.5);
    assert!(pt_huge.x.is_finite() && pt_huge.y.is_finite());
    let tan_huge = cubic_tangent([huge_p, huge_p, huge_p, huge_p], 0.5);
    assert!(tan_huge.x.is_finite() && tan_huge.y.is_finite());
}

#[test]
fn challenge_extreme_vector_angle_singularities() {
    // 1. Extreme magnitude vectors: overflowing hypot
    let huge1 = Vec2 { x: 1e200, y: 1e200 };
    let huge2 = Vec2 {
        x: 1e200,
        y: -1e200,
    };
    let ang_huge_turn = turn_angle(huge1, huge2);
    let ang_huge_vec = vector_angle(huge1, huge2);
    assert!(
        ang_huge_turn.is_finite(),
        "turn_angle produced non-finite on huge vectors"
    );
    assert!(
        ang_huge_vec.is_finite(),
        "vector_angle produced non-finite on huge vectors"
    );
    assert!((0.0..=std::f64::consts::PI).contains(&ang_huge_turn));
    assert!((0.0..=std::f64::consts::PI).contains(&ang_huge_vec));

    // 2. Subnormal floats near zero
    let subnormal1 = Vec2 {
        x: 1e-310,
        y: 1e-310,
    };
    let subnormal2 = Vec2 {
        x: f64::MIN_POSITIVE,
        y: 0.0,
    };
    assert_eq!(turn_angle(subnormal1, subnormal2), 0.0);
    assert_eq!(vector_angle(subnormal1, subnormal2), 0.0);

    // 3. Parallel and anti-parallel unit vectors
    let right = Vec2 { x: 1.0, y: 0.0 };
    let left = Vec2 { x: -1.0, y: 0.0 };
    let up = Vec2 { x: 0.0, y: 1.0 };
    assert_eq!(turn_angle(right, right), 0.0);
    assert_eq!(vector_angle(right, right), 0.0);
    assert!((turn_angle(right, left) - std::f64::consts::PI).abs() < 1e-12);
    assert!((vector_angle(right, left) - std::f64::consts::PI).abs() < 1e-12);
    assert!((turn_angle(right, up) - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    assert!((vector_angle(right, up) - std::f64::consts::FRAC_PI_2).abs() < 1e-12);

    // 4. Floating point rounding overshoot (dot product slightly > 1.0 or < -1.0)
    let v_round1 = Vec2 {
        x: 1.0 + 1e-16,
        y: 0.0,
    };
    let v_round2 = Vec2 { x: 1.0, y: 0.0 };
    let ang_round = turn_angle(v_round1, v_round2);
    assert!(ang_round.is_finite());
    assert!((0.0..=1e-12).contains(&ang_round));

    // 5. Infinities and NaNs
    let nans = [
        Vec2 {
            x: f64::NAN,
            y: 0.0,
        },
        Vec2 {
            x: 0.0,
            y: f64::NAN,
        },
        Vec2 {
            x: f64::NAN,
            y: f64::NAN,
        },
        Vec2 {
            x: f64::INFINITY,
            y: 0.0,
        },
        Vec2 {
            x: 0.0,
            y: f64::NEG_INFINITY,
        },
        Vec2 {
            x: f64::INFINITY,
            y: f64::INFINITY,
        },
        Vec2 {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        },
    ];
    for &bad in &nans {
        assert_eq!(turn_angle(bad, right), 0.0);
        assert_eq!(turn_angle(right, bad), 0.0);
        assert_eq!(turn_angle(bad, bad), 0.0);
        assert_eq!(vector_angle(bad, right), 0.0);
        assert_eq!(vector_angle(right, bad), 0.0);
        assert_eq!(vector_angle(bad, bad), 0.0);
    }
}

#[test]
fn challenge_decimate_and_arc_lengths_stress() {
    let cfg = FitConfig::default();

    // 1. Single point polyline
    let single_open = Polyline::new(vec![Point::new(10.0, 20.0)], vec![0.5], false);
    let idx_single = decimate::indices(&single_open, 1, &cfg);
    assert_eq!(idx_single, vec![0]);
    let idx_single_stride10 = decimate::indices(&single_open, 10, &cfg);
    assert_eq!(idx_single_stride10, vec![0]);

    // 2. Two point polyline with large stride
    let two_pts = Polyline::new(
        vec![Point::new(0.0, 0.0), Point::new(10.0, 10.0)],
        vec![0.5, 0.5],
        false,
    );
    let idx_two = decimate::indices(&two_pts, 50, &cfg);
    assert_eq!(idx_two, vec![0, 1]);

    // 3. 5000 identical coincident points
    let n = 5000;
    let coinc_pts = vec![Point::new(12.34, 56.78); n];
    let coinc_poly = Polyline::new(coinc_pts.clone(), vec![0.5; n], false);
    let idx_coinc = decimate::indices(&coinc_poly, 16, &cfg);
    assert!(!idx_coinc.is_empty());
    assert_eq!(*idx_coinc.first().unwrap(), 0);
    assert_eq!(*idx_coinc.last().unwrap(), n - 1);
    for w in idx_coinc.windows(2) {
        assert!(w[0] < w[1], "decimate indices must be strictly ascending");
    }

    // 4. Arc lengths: 5000 identical points
    let s_coinc = arc_lengths(&coinc_pts);
    assert_eq!(s_coinc.len(), n);
    for &val in &s_coinc {
        assert_eq!(val, 0.0);
    }

    // 5. Arc lengths: large distances
    let huge_p0 = Point::new(0.0, 0.0);
    let huge_p1 = Point::new(1e100, 1e100);
    let s_huge = arc_lengths(&[huge_p0, huge_p1]);
    assert_eq!(s_huge.len(), 2);
    assert_eq!(s_huge[0], 0.0);
    assert!(s_huge[1].is_finite());
    assert!(s_huge[1] > 1e100);
}

#[test]
fn challenge_scatter_min_eigen_adversarial_stability() {
    // 1. Coordinates at huge offset (tests catastrophic cancellation: sxx - sx*sx/w)
    let offset_x = 1e8;
    let offset_y = 1e8;
    let pts = [
        Point::new(offset_x + 0.0, offset_y + 0.0),
        Point::new(offset_x + 10.0, offset_y + 0.0),
        Point::new(offset_x + 20.0, offset_y + 0.0),
        Point::new(offset_x + 30.0, offset_y + 0.0),
    ];
    let w = pts.len() as f64;
    let sx: f64 = pts.iter().map(|p| p.x).sum();
    let sy: f64 = pts.iter().map(|p| p.y).sum();
    let sxx: f64 = pts.iter().map(|p| p.x * p.x).sum();
    let syy: f64 = pts.iter().map(|p| p.y * p.y).sum();
    let sxy: f64 = pts.iter().map(|p| p.x * p.y).sum();

    let chi2 = scatter_min_eigen(w, sx, sy, sxx, syy, sxy);
    assert!(chi2.is_finite(), "chi2 must be finite");
    assert!(chi2 >= 0.0, "chi2 must be non-negative");
    assert!(
        chi2 < 1e-4,
        "collinear segment with large coordinate offset must have near-zero residual, got {}",
        chi2
    );

    // 2. Collinear line along steep angle with tiny epsilon jitter
    let n = 200;
    let mut jitter_pts = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64;
        let jitter = if i % 2 == 0 { 1e-12 } else { -1e-12 };
        jitter_pts.push(Point::new(t, 2.5 * t + jitter));
    }
    let w_j = n as f64;
    let sx_j: f64 = jitter_pts.iter().map(|p| p.x).sum();
    let _sy_j: f64 = jitter_pts.iter().map(|p| p.y).sum();
    let sxx_j: f64 = jitter_pts.iter().map(|p| p.x * p.x).sum();
    let sy_j: f64 = jitter_pts.iter().map(|p| p.y * p.y).sum();
    let sxy_j: f64 = jitter_pts.iter().map(|p| p.x * p.y).sum();

    let chi2_j = scatter_min_eigen(w_j, sx_j, sy_j, sxx_j, sy_j, sxy_j);
    assert!(chi2_j.is_finite());
    assert!(chi2_j >= 0.0);
    assert!(chi2_j < 1e-10);

    // 3. Epsilon weight
    let chi2_tiny_w = scatter_min_eigen(1e-15, 0.0, 0.0, 0.0, 0.0, 0.0);
    assert!(chi2_tiny_w.is_finite());
    assert!(chi2_tiny_w >= 0.0);
}

#[test]
fn challenge_arms_from_moments_and_ellipse_center() {
    // 1. arms_from_moments under extreme angle configurations
    let test_cases = [
        (0.0, 0.0, 0.0, 0.0),
        (std::f64::consts::PI, -std::f64::consts::PI, 0.0, 0.0),
        (
            std::f64::consts::FRAC_PI_2,
            -std::f64::consts::FRAC_PI_2,
            1.0,
            0.5,
        ),
        (10.0, -10.0, 100.0, 50.0),
        (0.001, -0.001, 0.0001, 0.00001),
        (1e-9, 1e-9, 0.0, 0.0),
        (std::f64::consts::PI * 2.0, 0.0, 1.0, 1.0),
    ];

    for &(th0, th1, area, mx) in &test_cases {
        let arms = arms_from_moments(th0, th1, area, mx);
        assert!(arms.len <= 4);
        for (d0, d1) in arms.iter() {
            assert!(d0.is_finite(), "d0 must be finite");
            assert!(d1.is_finite(), "d1 must be finite");
            assert!(d0 >= 0.0, "d0 must be non-negative");
            assert!(d1 >= 0.0, "d1 must be non-negative");
        }
    }

    // 2. arc_ellipse_center singularities
    let p_origin = Point::new(0.0, 0.0);
    let p_far = Point::new(100.0, 0.0);

    // Radii too small to bridge chord (distance = 100, rx = 10, ry = 10)
    let frame_too_small = arc_ellipse_center(p_origin, 10.0, 10.0, 0.0, false, false, p_far);
    assert!(frame_too_small.c.x.is_finite());
    assert!(frame_too_small.c.y.is_finite());
    assert!(frame_too_small.delta.is_finite());

    // Negative radii
    let frame_neg_r = arc_ellipse_center(p_origin, -20.0, -20.0, 0.0, false, false, p_far);
    assert!(frame_neg_r.c.x.is_finite());
    assert!(frame_neg_r.c.y.is_finite());

    // Rotated ellipse with zero angle
    let frame_rot = arc_ellipse_center(
        p_origin,
        60.0,
        40.0,
        std::f64::consts::FRAC_PI_3,
        true,
        false,
        p_far,
    );
    assert!(frame_rot.c.x.is_finite());
    assert!(frame_rot.c.y.is_finite());
    assert!(frame_rot.delta.is_finite());
}

#[test]
fn challenge_end_to_end_fit_pipelines_on_degenerate_geometry() {
    let cfg = FitConfig::default();

    // 1. Empty polyline
    let empty_poly = Polyline::new(Vec::new(), Vec::new(), false);
    let fit_empty = fit_path(&empty_poly, &cfg);
    assert_eq!(fit_empty.segments.len(), 0);
    assert_eq!(fit_empty.start, Point::new(0.0, 0.0));

    let multi_empty = optimal_multimodel(&empty_poly, &cfg);
    assert_eq!(multi_empty.segments.len(), 0);

    // 2. Single point polyline
    let p_single = Point::new(15.0, 25.0);
    let single_poly = Polyline::new(vec![p_single], vec![0.5], false);
    let fit_single = fit_path(&single_poly, &cfg);
    assert_eq!(fit_single.segments.len(), 0);
    assert_eq!(fit_single.start, p_single);

    let multi_single = optimal_multimodel(&single_poly, &cfg);
    assert_eq!(multi_single.segments.len(), 0);
    assert_eq!(multi_single.start, p_single);

    // 3. Two coincident points
    let coinc_two = Polyline::new(vec![p_single, p_single], vec![0.5, 0.5], false);
    let fit_c2 = fit_path(&coinc_two, &cfg);
    for seg in &fit_c2.segments {
        assert!(seg.end().x.is_finite() && seg.end().y.is_finite());
    }

    let multi_c2 = optimal_multimodel(&coinc_two, &cfg);
    for seg in &multi_c2.segments {
        assert!(seg.end().x.is_finite() && seg.end().y.is_finite());
    }

    // 4. Perfectly straight collinear line of 50 points
    let line_pts: Vec<Point> = (0..50).map(|i| Point::new(i as f64 * 2.0, 0.0)).collect();
    let line_poly = Polyline::new(line_pts, vec![0.2; 50], false);
    let fit_line = fit_path(&line_poly, &cfg);
    assert!(!fit_line.segments.is_empty());
    for seg in &fit_line.segments {
        assert!(seg.end().x.is_finite() && seg.end().y.is_finite());
        if let Segment::Line(end) = seg {
            assert!((end.y).abs() < 1e-9);
        }
    }

    let multi_line = optimal_multimodel(&line_poly, &cfg);
    assert!(!multi_line.segments.is_empty());
    for seg in &multi_line.segments {
        assert!(seg.end().x.is_finite() && seg.end().y.is_finite());
    }
}
