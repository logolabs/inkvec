//! Integration tests for degenerate geometry and numerical singularity hardening.
//!
//! Verifies zero panics, zero unhandled zero divisions, and zero NaN propagation across:
//! - Coincident Bézier control points (p0=p1=p2=p3, p0=p1, cusps, retrograde segments)
//! - Zero-length and non-finite tangent angles (`turn_angle`, `vector_angle`)
//! - Empty polyline decimation (`decimate::indices`)
//! - Empty and single-point arc length cumulative sums (`arc_lengths`)
//! - Collinear and degenerate solves (`scatter_min_eigen`, `arms_from_moments`, `arc_ellipse_center`)

use inkvec_core::{Point, Polyline, Vec2};
use inkvec_fit::candidates::{arms_from_moments, scatter_min_eigen};
use inkvec_fit::curves::{
    arc_ellipse_center, cubic_self_intersects, cubic_tangent, eval_cubic, Segment,
};
use inkvec_fit::decimate;
use inkvec_fit::structural::vector_angle;
use inkvec_fit::tangents::{arc_lengths, turn_angle};
use inkvec_fit::FitConfig;

#[test]
fn test_coincident_bezier_control_points() {
    // Case 1: All 4 control points identical (completely collapsed cubic)
    let p = Point::new(10.0, 20.0);
    let pts = [p, p, p, p];

    let p_start = eval_cubic(pts, 0.0);
    let p_mid = eval_cubic(pts, 0.5);
    let p_end = eval_cubic(pts, 1.0);
    assert_eq!(p_start, p);
    assert_eq!(p_mid, p);
    assert_eq!(p_end, p);

    let tan_start = cubic_tangent(pts, 0.0);
    let tan_mid = cubic_tangent(pts, 0.5);
    let tan_end = cubic_tangent(pts, 1.0);
    assert_eq!(tan_start, Vec2 { x: 0.0, y: 0.0 });
    assert_eq!(tan_mid, Vec2 { x: 0.0, y: 0.0 });
    assert_eq!(tan_end, Vec2 { x: 0.0, y: 0.0 });
    assert!(tan_start.x.is_finite() && tan_start.y.is_finite());
    assert!(tan_mid.x.is_finite() && tan_mid.y.is_finite());
    assert!(tan_end.x.is_finite() && tan_end.y.is_finite());

    assert!(!cubic_self_intersects(p, p, p, p));

    let seg = Segment::Cubic(p, p, p);
    assert_eq!(seg.params(), 6.0);
    assert_eq!(seg.end(), p);

    // Case 2: p0 = p1 (start endpoint coincides with first control point)
    let p0 = Point::new(0.0, 0.0);
    let p1 = Point::new(0.0, 0.0);
    let p2 = Point::new(5.0, 5.0);
    let p3 = Point::new(10.0, 10.0);
    let tan0 = cubic_tangent([p0, p1, p2, p3], 0.0);
    assert_eq!(tan0, Vec2 { x: 0.0, y: 0.0 });
    assert!(tan0.x.is_finite() && tan0.y.is_finite());
    assert_eq!(eval_cubic([p0, p1, p2, p3], 0.0), p0);
    assert!(!cubic_self_intersects(p0, p1, p2, p3));

    // Case 3: p2 = p3 (end endpoint coincides with second control point)
    let p0 = Point::new(0.0, 0.0);
    let p1 = Point::new(5.0, 5.0);
    let p2 = Point::new(10.0, 10.0);
    let p3 = Point::new(10.0, 10.0);
    let tan1 = cubic_tangent([p0, p1, p2, p3], 1.0);
    assert_eq!(tan1, Vec2 { x: 0.0, y: 0.0 });
    assert!(tan1.x.is_finite() && tan1.y.is_finite());
    assert_eq!(eval_cubic([p0, p1, p2, p3], 1.0), p3);
    assert!(!cubic_self_intersects(p0, p1, p2, p3));

    // Case 4: Cusp / retrograding segment
    let c0 = Point::new(0.0, 0.0);
    let c1 = Point::new(10.0, 0.0);
    let c2 = Point::new(0.0, 0.0);
    let c3 = Point::new(10.0, 0.0);
    let _ = cubic_self_intersects(c0, c1, c2, c3); // must not panic or NaN
    for step in 0..=10 {
        let t = step as f64 / 10.0;
        let pt = eval_cubic([c0, c1, c2, c3], t);
        let tan = cubic_tangent([c0, c1, c2, c3], t);
        assert!(pt.x.is_finite() && pt.y.is_finite());
        assert!(tan.x.is_finite() && tan.y.is_finite());
    }

    // Case 5: Fully reversed retrograde segment
    let r0 = Point::new(10.0, 0.0);
    let r1 = Point::new(0.0, 0.0);
    let r2 = Point::new(10.0, 0.0);
    let r3 = Point::new(0.0, 0.0);
    assert!(!cubic_self_intersects(r0, r1, r2, r3));
    for step in 0..=10 {
        let t = step as f64 / 10.0;
        let pt = eval_cubic([r0, r1, r2, r3], t);
        let tan = cubic_tangent([r0, r1, r2, r3], t);
        assert!(pt.x.is_finite() && pt.y.is_finite());
        assert!(tan.x.is_finite() && tan.y.is_finite());
    }
}

#[test]
fn test_zero_length_tangent_angle_robustness() {
    let zero = Vec2 { x: 0.0, y: 0.0 };
    let right = Vec2 { x: 1.0, y: 0.0 };
    let up = Vec2 { x: 0.0, y: 1.0 };
    let nan_vec = Vec2 {
        x: f64::NAN,
        y: 0.0,
    };
    let nan_both = Vec2 {
        x: f64::NAN,
        y: f64::NAN,
    };
    let inf_vec = Vec2 {
        x: f64::INFINITY,
        y: 0.0,
    };
    let neg_inf_vec = Vec2 {
        x: 0.0,
        y: f64::NEG_INFINITY,
    };
    let subnormal = Vec2 { x: 1e-15, y: 1e-15 };

    // Valid vectors produce standard angle
    let ang90_turn = turn_angle(right, up);
    assert!((ang90_turn - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    let ang90_vec = vector_angle(right, up);
    assert!((ang90_vec - std::f64::consts::FRAC_PI_2).abs() < 1e-9);

    // Zero-length inputs must return 0.0, never NaN
    assert_eq!(turn_angle(zero, right), 0.0);
    assert_eq!(turn_angle(right, zero), 0.0);
    assert_eq!(turn_angle(zero, zero), 0.0);
    assert_eq!(vector_angle(zero, right), 0.0);
    assert_eq!(vector_angle(right, zero), 0.0);
    assert_eq!(vector_angle(zero, zero), 0.0);

    // Subnormal inputs (< 1e-12) must return 0.0
    assert_eq!(turn_angle(subnormal, right), 0.0);
    assert_eq!(vector_angle(subnormal, right), 0.0);

    // NaN inputs must return 0.0, never NaN
    assert_eq!(turn_angle(nan_vec, right), 0.0);
    assert_eq!(turn_angle(right, nan_vec), 0.0);
    assert_eq!(turn_angle(nan_both, nan_both), 0.0);
    assert_eq!(vector_angle(nan_vec, right), 0.0);
    assert_eq!(vector_angle(right, nan_vec), 0.0);
    assert_eq!(vector_angle(nan_both, nan_both), 0.0);

    // Inf inputs must return 0.0, never NaN
    assert_eq!(turn_angle(inf_vec, right), 0.0);
    assert_eq!(turn_angle(right, neg_inf_vec), 0.0);
    assert_eq!(vector_angle(inf_vec, right), 0.0);
    assert_eq!(vector_angle(right, neg_inf_vec), 0.0);

    // Mixed NaN and zero
    assert_eq!(turn_angle(zero, nan_both), 0.0);
    assert_eq!(vector_angle(zero, nan_both), 0.0);
}

#[test]
fn test_decimate_indices_empty_polyline() {
    let cfg = FitConfig::default();

    // Empty open polyline
    let empty_open = Polyline::new(Vec::new(), Vec::new(), false);
    let idx_stride1 = decimate::indices(&empty_open, 1, &cfg);
    assert!(idx_stride1.is_empty());
    let idx_stride4 = decimate::indices(&empty_open, 4, &cfg);
    assert!(idx_stride4.is_empty());

    // Empty closed polyline
    let empty_closed = Polyline::new(Vec::new(), Vec::new(), true);
    let idx_closed1 = decimate::indices(&empty_closed, 1, &cfg);
    assert!(idx_closed1.is_empty());
    let idx_closed4 = decimate::indices(&empty_closed, 4, &cfg);
    assert!(idx_closed4.is_empty());
}

#[test]
fn test_arc_lengths_empty_and_single_point() {
    // Empty slice returns empty Vec
    let empty_res = arc_lengths(&[]);
    assert!(empty_res.is_empty());

    // Single point returns [0.0]
    let single_p = Point::new(10.0, 25.0);
    let single_res = arc_lengths(&[single_p]);
    assert_eq!(single_res, vec![0.0]);

    // Two points
    let p0 = Point::new(0.0, 0.0);
    let p1 = Point::new(3.0, 4.0);
    let two_res = arc_lengths(&[p0, p1]);
    assert_eq!(two_res, vec![0.0, 5.0]);

    // All coincident points
    let c0 = Point::new(7.0, 7.0);
    let c1 = Point::new(7.0, 7.0);
    let c2 = Point::new(7.0, 7.0);
    let coinc_res = arc_lengths(&[c0, c1, c2]);
    assert_eq!(coinc_res, vec![0.0, 0.0, 0.0]);
}

#[test]
fn test_collinear_and_degenerate_solves() {
    // 1. scatter_min_eigen: coincident points
    let w = 5.0;
    let sx = 15.0;
    let sy = 20.0;
    let sxx = 45.0;
    let syy = 80.0;
    let sxy = 60.0;
    let chi2_coinc = scatter_min_eigen(w, sx, sy, sxx, syy, sxy);
    assert!(chi2_coinc.is_finite());
    assert!(chi2_coinc.abs() < 1e-12);

    // scatter_min_eigen: perfectly collinear points along x-axis
    let w = 4.0;
    let sx = 6.0;
    let sy = 0.0;
    let sxx = 14.0;
    let syy = 0.0;
    let sxy = 0.0;
    let chi2_collin_x = scatter_min_eigen(w, sx, sy, sxx, syy, sxy);
    assert!(chi2_collin_x.is_finite());
    assert!(chi2_collin_x.abs() < 1e-12);

    // scatter_min_eigen: perfectly collinear points along y=x diagonal
    let w = 3.0;
    let sx = 3.0;
    let sy = 3.0;
    let sxx = 5.0;
    let syy = 5.0;
    let sxy = 5.0;
    let chi2_collin_diag = scatter_min_eigen(w, sx, sy, sxx, syy, sxy);
    assert!(chi2_collin_diag.is_finite());
    assert!(chi2_collin_diag.abs() < 1e-12);

    // 2. arms_from_moments: zero area and moment (straight chord)
    let arms_flat = arms_from_moments(0.0, 0.0, 0.0, 0.0);
    assert!(arms_flat.len > 0);
    for (d0, d1) in arms_flat.iter() {
        assert!(d0.is_finite() && d1.is_finite());
        assert!(d0 >= 0.0 && d1 >= 0.0);
    }

    // arms_from_moments: cusp / opposite tangents
    let arms_cusp = arms_from_moments(0.0, std::f64::consts::PI, 0.0, 0.0);
    for (d0, d1) in arms_cusp.iter() {
        assert!(d0.is_finite() && d1.is_finite());
        assert!(d0 >= 0.0 && d1 >= 0.0);
    }

    // 3. arc_ellipse_center: coincident endpoints
    let p_same = Point::new(10.0, 15.0);
    let frame_coinc = arc_ellipse_center(p_same, 5.0, 5.0, 0.0, false, false, p_same);
    assert_eq!(frame_coinc.delta, 0.0);
    assert!(frame_coinc.c.x.is_finite() && frame_coinc.c.y.is_finite());

    // arc_ellipse_center: zero radii
    let p_start = Point::new(0.0, 0.0);
    let p_end = Point::new(10.0, 0.0);
    let frame_zero_r = arc_ellipse_center(p_start, 0.0, 0.0, 0.0, false, false, p_end);
    assert_eq!(frame_zero_r.delta, 0.0);
    assert!(frame_zero_r.c.x.is_finite() && frame_zero_r.c.y.is_finite());

    // arc_ellipse_center: near-coincident endpoints
    let p_near = Point::new(1e-15, 1e-15);
    let frame_near = arc_ellipse_center(p_start, 1.0, 1.0, 0.0, false, false, p_near);
    assert_eq!(frame_near.delta, 0.0);
    assert!(frame_near.c.x.is_finite() && frame_near.c.y.is_finite());
}
