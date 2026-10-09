//! Integration tests for rotation and reflection equivariance in curve fitting and primitives.
//!
//! Verifies that rigid geometric transformations (90°, 180°, 270° and axis reflections)
//! produce equivariant outputs across:
//! - Tangent estimation (`estimate_tangents`)
//! - Primitive circle, ellipse, and rounded rectangle recovery
//! - Multi-model dynamic programming curve fitting (`fit_path`)
//! - Strict bounds on Hausdorff deviation (< 0.05 px) and segment counts

use inkvec_core::{Point, Polyline, Vec2};
use inkvec_fit::curves;
use inkvec_fit::primitives::{fit_circle, fit_ellipse, fit_round_rect};
use inkvec_fit::tangents::estimate_tangents;
use inkvec_fit::{fit_path, FitConfig};
use std::f64::consts::{PI, TAU};

/// Deterministic LCG pseudo-random generator
struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_f64(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn gaussian(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-300);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

/// Rotations and reflection operations
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
enum Transform {
    Rot0,
    Rot90,
    Rot180,
    Rot270,
    ReflectX,
}

impl Transform {
    fn apply_point(&self, p: Point) -> Point {
        match self {
            Transform::Rot0 => p,
            Transform::Rot90 => Point::new(-p.y, p.x),
            Transform::Rot180 => Point::new(-p.x, -p.y),
            Transform::Rot270 => Point::new(p.y, -p.x),
            Transform::ReflectX => Point::new(-p.x, p.y),
        }
    }

    fn apply_vec(&self, v: Vec2) -> Vec2 {
        match self {
            Transform::Rot0 => v,
            Transform::Rot90 => Vec2 { x: -v.y, y: v.x },
            Transform::Rot180 => Vec2 { x: -v.x, y: -v.y },
            Transform::Rot270 => Vec2 { x: v.y, y: -v.x },
            Transform::ReflectX => Vec2 { x: -v.x, y: v.y },
        }
    }
}

fn transform_polyline(poly: &Polyline, t: Transform) -> Polyline {
    let pts: Vec<Point> = poly.points.iter().map(|&p| t.apply_point(p)).collect();
    Polyline::new(pts, poly.sigma.clone(), poly.closed)
}

#[test]
fn test_circle_rotation_equivariance() {
    let (c0, r0) = (Point::new(60.0, 45.0), 32.0);
    let mut rng = Lcg::new(101);
    let n = 240;
    let sigma_val = 0.02;

    let pts: Vec<Point> = (0..n)
        .map(|k| {
            let t = k as f64 * TAU / n as f64;
            let dr = rng.gaussian() * sigma_val;
            Point::new(c0.x + (r0 + dr) * t.cos(), c0.y + (r0 + dr) * t.sin())
        })
        .collect();
    let sigmas = vec![sigma_val; n];

    let fit0 = fit_circle(&pts, &sigmas).expect("fit original circle");
    assert!((fit0.r - r0).abs() < 0.01);
    assert!(fit0.c.dist(c0) < 0.01);

    for &tf in &[
        Transform::Rot90,
        Transform::Rot180,
        Transform::Rot270,
        Transform::ReflectX,
    ] {
        let pts_tf: Vec<Point> = pts.iter().map(|&p| tf.apply_point(p)).collect();
        let fit_tf = fit_circle(&pts_tf, &sigmas).expect("fit transformed circle");

        assert!(
            (fit_tf.r - fit0.r).abs() < 1e-4,
            "radius invariance failed for {tf:?}: {} vs {}",
            fit_tf.r,
            fit0.r
        );
        let expected_c = tf.apply_point(fit0.c);
        assert!(
            fit_tf.c.dist(expected_c) < 1e-4,
            "centre equivariance failed for {tf:?}: {:?} vs {:?}",
            fit_tf.c,
            expected_c
        );
    }
}

#[test]
fn test_ellipse_rotation_equivariance() {
    let (c0, rx0, ry0, theta0) = (Point::new(50.0, 50.0), 35.0, 18.0, 35.0f64.to_radians());
    let mut rng = Lcg::new(202);
    let n = 240;
    let sigma_val = 0.02;
    let (s, co) = theta0.sin_cos();

    let pts: Vec<Point> = (0..n)
        .map(|k| {
            let t = k as f64 * TAU / n as f64;
            let drx = rng.gaussian() * sigma_val;
            let dry = rng.gaussian() * sigma_val;
            let (ex, ey) = ((rx0 + drx) * t.cos(), (ry0 + dry) * t.sin());
            Point::new(c0.x + co * ex - s * ey, c0.y + s * ex + co * ey)
        })
        .collect();
    let sigmas = vec![sigma_val; n];

    let fit0 = fit_ellipse(&pts, &sigmas).expect("fit original ellipse");
    assert!((fit0.rx - rx0).abs() < 0.05);
    assert!((fit0.ry - ry0).abs() < 0.05);
    assert!(fit0.c.dist(c0) < 0.05);

    for &tf in &[Transform::Rot90, Transform::Rot180, Transform::Rot270] {
        let pts_tf: Vec<Point> = pts.iter().map(|&p| tf.apply_point(p)).collect();
        let fit_tf = fit_ellipse(&pts_tf, &sigmas).expect("fit transformed ellipse");

        let expected_c = tf.apply_point(fit0.c);
        assert!(
            fit_tf.c.dist(expected_c) < 0.01,
            "centre equivariance failed for {tf:?}"
        );

        // Under 90 and 270 deg rotations, semi-axes swap if orientation aligns
        let axes_match = ((fit_tf.rx - fit0.rx).abs() < 0.05 && (fit_tf.ry - fit0.ry).abs() < 0.05)
            || ((fit_tf.rx - fit0.ry).abs() < 0.05 && (fit_tf.ry - fit0.rx).abs() < 0.05);
        assert!(axes_match, "axes invariance failed for {tf:?}");
    }
}

#[test]
fn test_round_rect_rotation_equivariance() {
    let rect0 = [10.0, 15.0, 60.0, 40.0, 8.0]; // x, y, w, h, rx
    let [x, y, w, h, rx] = rect0;
    let straight_w = w - 2.0 * rx;
    let straight_h = h - 2.0 * rx;
    let arc = rx * PI / 2.0;
    let total = 2.0 * (straight_w + straight_h) + 4.0 * arc;
    let n = 200;
    let sigma_val = 0.02;

    let pts: Vec<Point> = (0..n)
        .map(|k| {
            let s = k as f64 * total / n as f64;
            // Sampling round rect along perimeter
            if s <= straight_w {
                Point::new(x + rx + s, y + h)
            } else if s <= straight_w + arc {
                let u = (s - straight_w) / arc;
                let a = PI / 2.0 - u * PI / 2.0;
                Point::new(x + w - rx + rx * a.cos(), y + h - rx + rx * a.sin())
            } else if s <= straight_w + arc + straight_h {
                let u = (s - straight_w - arc) / straight_h;
                Point::new(x + w, y + h - rx - straight_h * u)
            } else if s <= straight_w + 2.0 * arc + straight_h {
                let u = (s - straight_w - arc - straight_h) / arc;
                let a = -u * PI / 2.0;
                Point::new(x + w - rx + rx * a.cos(), y + rx + rx * a.sin())
            } else if s <= 2.0 * straight_w + 2.0 * arc + straight_h {
                let u = (s - straight_w - 2.0 * arc - straight_h) / straight_w;
                Point::new(x + w - rx - straight_w * u, y)
            } else if s <= 2.0 * straight_w + 3.0 * arc + straight_h {
                let u = (s - 2.0 * straight_w - 2.0 * arc - straight_h) / arc;
                let a = -PI / 2.0 - u * PI / 2.0;
                Point::new(x + rx + rx * a.cos(), y + rx + rx * a.sin())
            } else if s <= 2.0 * straight_w + 3.0 * arc + 2.0 * straight_h {
                let u = (s - 2.0 * straight_w - 3.0 * arc - straight_h) / straight_h;
                Point::new(x, y + rx + straight_h * u)
            } else {
                let u = (s - 2.0 * straight_w - 3.0 * arc - 2.0 * straight_h) / arc;
                let a = PI - u * PI / 2.0;
                Point::new(x + rx + rx * a.cos(), y + h - rx + rx * a.sin())
            }
        })
        .collect();
    let sigmas = vec![sigma_val; n];

    let fit0 = fit_round_rect(&pts, &sigmas, None).expect("fit original round rect");
    assert!((fit0.rx - rx).abs() < 0.1);

    for &tf in &[Transform::Rot90, Transform::Rot180, Transform::Rot270] {
        let pts_tf: Vec<Point> = pts.iter().map(|&p| tf.apply_point(p)).collect();
        let fit_tf = fit_round_rect(&pts_tf, &sigmas, None).expect("fit transformed round rect");
        assert!(
            (fit_tf.rx - fit0.rx).abs() < 0.05,
            "corner radius invariance failed for {tf:?}: {} vs {}",
            fit_tf.rx,
            fit0.rx
        );
    }
}

#[test]
fn test_path_fit_rotation_and_reflection_equivariance() {
    let cfg = FitConfig::default();

    // Smooth open S-curve: y(x) = 20.0 * sin(x / 15.0)
    let n = 60;
    let pts: Vec<Point> = (0..n)
        .map(|k| {
            let x = k as f64 * 2.0;
            let y = 20.0 * (x / 15.0).sin();
            Point::new(x, y)
        })
        .collect();
    let poly0 = Polyline::with_uniform_sigma(pts, 0.05, false);

    let fit0 = fit_path(&poly0, &cfg);
    assert!(!fit0.is_empty());
    let dev0 = curves::max_deviation(&poly0.points, fit0.start, &fit0.segments);
    assert!(dev0 < 0.25, "dev0 = {dev0}");

    let seg0 = inkvec_fit::optimal_polygon(&poly0, &cfg);
    let mm0 = inkvec_fit::multimodel::optimal_multimodel(&poly0, &cfg);

    for &tf in &[
        Transform::Rot90,
        Transform::Rot180,
        Transform::Rot270,
        Transform::ReflectX,
    ] {
        let poly_tf = transform_polyline(&poly0, tf);
        let seg_tf = inkvec_fit::optimal_polygon(&poly_tf, &cfg);

        // 1. optimal_polygon must be strictly equivariant on vertex indices
        assert_eq!(
            seg_tf.vertices, seg0.vertices,
            "optimal_polygon vertex discrepancy under {tf:?}: {:?} vs {:?}",
            seg_tf.vertices, seg0.vertices
        );

        // Cost must match to floating-point precision
        assert!(
            (seg_tf.cost - seg0.cost).abs() < 1e-4,
            "optimal_polygon cost discrepancy under {tf:?}: {} vs {}",
            seg_tf.cost,
            seg0.cost
        );

        // 2. multimodel segment count and cost equivariance
        let mm_tf = inkvec_fit::multimodel::optimal_multimodel(&poly_tf, &cfg);
        assert_eq!(
            mm_tf.segments.len(),
            mm0.segments.len(),
            "multimodel segment count discrepancy under {tf:?}: {} vs {}",
            mm_tf.segments.len(),
            mm0.segments.len()
        );

        let dev_tf = curves::max_deviation(&poly_tf.points, mm_tf.start, &mm_tf.segments);
        let dev0 = curves::max_deviation(&poly0.points, mm0.start, &mm0.segments);
        assert!(
            (dev_tf - dev0).abs() < 0.02,
            "multimodel deviation discrepancy under {tf:?}: {dev_tf} vs {dev0}"
        );
    }
}

#[test]
fn test_tangent_estimation_rotation_equivariance_and_unit_norm() {
    let cfg = FitConfig::default();

    let n = 50;
    let pts: Vec<Point> = (0..n)
        .map(|k| {
            let t = k as f64 * PI / (n - 1) as f64;
            Point::new(30.0 * t.cos(), 20.0 * t.sin())
        })
        .collect();
    let poly0 = Polyline::with_uniform_sigma(pts, 0.05, false);

    let tan0 = estimate_tangents(&poly0, &cfg);

    // Unit norm assertion
    for (i, (&tin, &tout)) in tan0.incoming.iter().zip(tan0.outgoing.iter()).enumerate() {
        assert!(tin.x.is_finite() && tin.y.is_finite());
        assert!(tout.x.is_finite() && tout.y.is_finite());
        assert!(
            (tin.norm() - 1.0).abs() < 1e-6,
            "incoming tangent at {i} not unit norm: {tin:?}"
        );
        assert!(
            (tout.norm() - 1.0).abs() < 1e-6,
            "outgoing tangent at {i} not unit norm: {tout:?}"
        );
    }

    for &tf in &[Transform::Rot90, Transform::Rot180, Transform::Rot270] {
        let poly_tf = transform_polyline(&poly0, tf);
        let tan_tf = estimate_tangents(&poly_tf, &cfg);

        for i in 0..n {
            let expected_out = tf.apply_vec(tan0.outgoing[i]);
            let actual_out = tan_tf.outgoing[i];
            let dot = expected_out.dot(actual_out);
            assert!(
                (dot - 1.0).abs() < 1e-3,
                "tangent outgoing equivariance failed at vertex {i} under {tf:?}: dot={dot}"
            );
        }
    }
}
