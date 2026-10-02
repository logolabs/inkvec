//! The floor against the cost it bounds, and [`describe`] against the plain choice.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// A small deterministic generator (PCG-style LCG), so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The image frame of a `w × h` raster as the planar map writes it: a closed ring of
/// pixel-corner lattice points around the border, clockwise from the top-left corner,
/// σ = 0.5 px.
fn frame(w: usize, h: usize) -> Polyline {
    let (x1, y1) = (w as f64 - 0.5, h as f64 - 0.5);
    let mut pts = Vec::new();
    for i in 0..w {
        pts.push(Point::new(i as f64 - 0.5, -0.5));
    }
    for j in 0..h {
        pts.push(Point::new(x1, j as f64 - 0.5));
    }
    for i in (1..=w).rev() {
        pts.push(Point::new(i as f64 - 0.5, y1));
    }
    for j in (1..=h).rev() {
        pts.push(Point::new(-0.5, j as f64 - 0.5));
    }
    let n = pts.len();
    Polyline::new(pts, vec![0.5; n], true)
}

/// Rings and runs of every kind the choice sees: circles, squares, a wobbly blob, a
/// straight run, collinear and coincident points, and frames from 1×1 up.
fn shapes(rng: &mut Rng) -> Vec<Polyline> {
    let mut out = vec![
        frame(1, 1),
        frame(1, 6),
        frame(3, 3),
        frame(16, 16),
        frame(40, 7),
    ];
    for case in 0..10 {
        let n = 6 + 9 * case;
        let r = 1.0 + 6.0 * rng.next();
        let pts: Vec<Point> = (0..n)
            .map(|k| {
                let t = std::f64::consts::TAU * k as f64 / n as f64;
                let wobble = [0.0, 0.1, 0.8][case % 3] * (rng.next() - 0.5);
                Point::new(20.0 + (r + wobble) * t.cos(), 9.0 + (r + wobble) * t.sin())
            })
            .collect();
        let sigma = (0..n).map(|_| 0.05 + 0.4 * rng.next()).collect();
        out.push(Polyline::new(pts, sigma, case % 4 != 3));
    }
    let line: Vec<Point> = (0..12)
        .map(|k| Point::new(k as f64, 0.5 * k as f64))
        .collect();
    out.push(Polyline::new(line, vec![0.2; 12], false));
    out.push(Polyline::new(
        vec![Point::new(3.0, 3.0); 5],
        vec![0.3; 5],
        true,
    ));
    out
}

/// A random path with `segs` segments of random kinds near the points of `poly`.
fn random_path(rng: &mut Rng, poly: &Polyline, segs: usize) -> FittedPath {
    let near = |rng: &mut Rng| {
        let p = poly.points[(rng.next() * poly.points.len() as f64) as usize % poly.points.len()];
        Point::new(
            p.x + 2.0 * (rng.next() - 0.5),
            p.y + 2.0 * (rng.next() - 0.5),
        )
    };
    let start = near(rng);
    let segments = (0..segs)
        .map(|_| match (rng.next() * 4.0) as usize {
            0 => Segment::Line(near(rng)),
            1 => Segment::Cubic(near(rng), near(rng), near(rng)),
            k => {
                let r = 0.5 + 10.0 * rng.next();
                Segment::Arc {
                    rx: r,
                    ry: if k == 2 { r } else { r * 1.5 },
                    phi: 0.0,
                    large_arc: false,
                    sweep: rng.next() < 0.5,
                    end: near(rng),
                }
            }
        })
        .collect();
    FittedPath {
        start,
        segments,
        closed: poly.closed,
    }
}

/// The best single line through `poly`: its principal axis, drawn long enough to cover
/// every point's foot. The tightest case of the single-line floor.
fn principal_line(poly: &Polyline) -> FittedPath {
    let n = poly.points.len() as f64;
    let (cx, cy) = poly
        .points
        .iter()
        .fold((0.0, 0.0), |(x, y), p| (x + p.x / n, y + p.y / n));
    let (mut a, mut b, mut d) = (0.0, 0.0, 0.0);
    for p in &poly.points {
        let (dx, dy) = (p.x - cx, p.y - cy);
        a += dx * dx;
        b += dx * dy;
        d += dy * dy;
    }
    let angle = 0.5 * (2.0 * b).atan2(a - d);
    let (ux, uy) = (angle.cos(), angle.sin());
    let reach = 4.0 * (a + d + 1.0).sqrt();
    FittedPath {
        start: Point::new(cx - reach * ux, cy - reach * uy),
        segments: vec![Segment::Line(Point::new(cx + reach * ux, cy + reach * uy))],
        closed: poly.closed,
    }
}

/// No path, of one segment or many, of any kind, costs less than the floor; and the floor
/// is not vacuous on a frame.
#[test]
fn no_path_costs_less_than_the_floor() {
    let mut rng = Rng(11);
    let mut checked = 0;
    for poly in shapes(&mut rng) {
        for lambda in [0.0, 1e-3, 1.0, 7.85, 12.3, 1e3] {
            let cfg = FitConfig {
                lambda,
                ..FitConfig::default()
            };
            let floor = cost_floor(&poly, &cfg);
            let mut paths = vec![principal_line(&poly)];
            for segs in 1..=5 {
                for _ in 0..12 {
                    paths.push(random_path(&mut rng, &poly, segs));
                }
            }
            for path in &paths {
                let cost = boundary_cost(&poly, path, &cfg);
                assert!(
                    cost >= floor,
                    "{} pts, lambda {lambda}: cost {cost} < floor {floor}",
                    poly.points.len()
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 5000);
    // A 16 px frame: every single line misses the frame badly, so the floor is the
    // two-segment price.
    let cfg = FitConfig::default();
    assert_eq!(cost_floor(&frame(16, 16), &cfg), 6.0 * cfg.lambda);
}

/// The floor proves nothing when it cannot: negative or NaN λ, and NaN coordinates.
#[test]
fn the_floor_is_safe_on_degenerate_input() {
    let poly = frame(4, 4);
    for lambda in [-1.0, f64::NAN] {
        let cfg = FitConfig {
            lambda,
            ..FitConfig::default()
        };
        assert_eq!(cost_floor(&poly, &cfg), f64::NEG_INFINITY);
    }
    let mut bad = frame(4, 4);
    bad.points[2] = Point::new(f64::NAN, 0.0);
    assert_eq!(line_chi2_floor(&bad), 0.0);
    assert_eq!(line_chi2_floor(&Polyline::new(vec![], vec![], true)), 0.0);
}

/// `describe` returns what the plain choice returns, on frames (where it may skip the
/// program) and on everything else (where it runs the two side by side); and on a frame
/// large enough for the rectangle to be provably cheapest, the program is not run.
#[test]
fn describe_is_the_plain_choice() {
    let mut rng = Rng(5);
    let cfg = FitConfig::default();
    let mut skipped = 0;
    for poly in shapes(&mut rng) {
        let plain = choose(
            &poly,
            crate::multimodel::optimal_multimodel(&poly, &cfg),
            primitive_offer(&poly, &cfg),
            &cfg,
        );
        for frame_first in [false, true] {
            let ran = AtomicBool::new(false);
            let got = describe(&poly, &cfg, frame_first, || {
                ran.store(true, Ordering::Relaxed);
                crate::multimodel::optimal_multimodel(&poly, &cfg)
            });
            assert_eq!(format!("{got:?}"), format!("{plain:?}"));
            if frame_first && !ran.load(Ordering::Relaxed) {
                skipped += 1;
            }
        }
    }
    // Every frame of at least 3×3 has its program skipped.
    assert!(skipped >= 3, "skipped only {skipped}");
}

/// The frame test: every point on one of the four border lines, nothing else.
#[test]
fn frame_points_are_recognised() {
    assert!(lies_on_frame(&frame(5, 3).points, 5, 3));
    assert!(!lies_on_frame(&frame(5, 3).points, 5, 4));
    assert!(!lies_on_frame(&[], 5, 3));
    let mut inside = frame(5, 3);
    inside.points[3] = Point::new(1.0, 1.0);
    assert!(!lies_on_frame(&inside.points, 5, 3));
    // An open run along two sides is on the frame too; the caller asks about closed rings.
    assert!(lies_on_frame(
        &[
            Point::new(2.5, -0.5),
            Point::new(4.5, -0.5),
            Point::new(4.5, 1.5)
        ],
        5,
        3
    ));
}
