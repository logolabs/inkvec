//! Tests for primitive fitting (circles, ellipses, arcs, rounded rectangles).
//!
//! The claims held to account:
//!
//! 1. **Recovery.** With 0.05px noise a circle's radius and centre come back within
//!    0.02px, an ellipse's within 0.02px, a rounded rectangle's corner radius within 0.1px.
//! 2. **The objective prefers them.** The primitive's MDL cost is strictly below the
//!    cubic description `fit_path` produces for the same points.
//! 3. **They are not hallucinated.** A square is not reported as a circle.
//! 4. **The refinement earns its keep.** Algebraic-only fitting is measurably worse than
//!    orthogonal-distance fitting on a partial arc — the case DESIGN.md S4 singles out.

use inkvec_core::{Point, Polyline};
use inkvec_fit::curves::{self, arc_center, Segment};
use inkvec_fit::primitives::{
    fit_arcs, fit_circle, fit_circle_algebraic, fit_ellipse, fit_ellipse_algebraic,
    fit_primitive_or_arcs, fit_round_rect, PrimitiveKind, MAX_ARC_DEGREES,
};
use inkvec_fit::{fit_path, FitConfig, FittedPath};
use std::f64::consts::{PI, TAU};

/// xorshift64* — deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn gaussian(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-300);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

const SIGMA: f64 = 0.05;

/// Points on `curve(t)` for `t` in `[t0, t1]`, displaced along the unit normal by
/// Gaussian noise of standard deviation `sigma`.
fn sample(
    n: usize,
    t0: f64,
    t1: f64,
    closed: bool,
    sigma: f64,
    seed: u64,
    curve: impl Fn(f64) -> Point,
) -> Vec<Point> {
    let mut rng = Rng(seed | 1);
    let denom = if closed { n } else { n - 1 } as f64;
    (0..n)
        .map(|k| {
            let t = t0 + (t1 - t0) * k as f64 / denom;
            let p = curve(t);
            let (a, b) = (curve(t - 1e-4), curve(t + 1e-4));
            let tan = b - a;
            let l = tan.norm().max(1e-12);
            let (nx, ny) = (-tan.y / l, tan.x / l);
            let d = sigma * rng.gaussian();
            Point::new(p.x + nx * d, p.y + ny * d)
        })
        .collect()
}

fn circle_pts(n: usize, c: Point, r: f64, sigma: f64, seed: u64) -> Vec<Point> {
    sample(n, 0.0, TAU, true, sigma, seed, |t| {
        Point::new(c.x + r * t.cos(), c.y + r * t.sin())
    })
}

fn ellipse_pts(
    n: usize,
    c: Point,
    rx: f64,
    ry: f64,
    angle: f64,
    sigma: f64,
    seed: u64,
) -> Vec<Point> {
    let (s, co) = angle.sin_cos();
    sample(n, 0.0, TAU, true, sigma, seed, |t| {
        let (x, y) = (rx * t.cos(), ry * t.sin());
        Point::new(c.x + co * x - s * y, c.y + s * x + co * y)
    })
}

/// Rounded rectangle traced by perimeter, corners as true quarter arcs.
fn round_rect_pts(n: usize, rect: [f64; 5], sigma: f64, seed: u64) -> Vec<Point> {
    let [x, y, w, h, rx] = rect;
    let straight_w = w - 2.0 * rx;
    let straight_h = h - 2.0 * rx;
    let arc = rx * PI / 2.0;
    let total = 2.0 * (straight_w + straight_h) + 4.0 * arc;
    let pieces = [
        (straight_w, 0),
        (arc, 1),
        (straight_h, 2),
        (arc, 3),
        (straight_w, 4),
        (arc, 5),
        (straight_h, 6),
        (arc, 7),
    ];
    let curve = |s: f64| -> Point {
        let mut s = s.rem_euclid(total);
        for (len, which) in pieces {
            if s <= len {
                let u = if len > 0.0 { s / len } else { 0.0 };
                return match which {
                    // Bottom side (y = y + h), left to right; then anticlockwise in y-up
                    // terms, which is a consistent orientation for the shoelace test.
                    0 => Point::new(x + rx + straight_w * u, y + h),
                    1 => {
                        let a = PI / 2.0 - u * PI / 2.0;
                        Point::new(x + w - rx + rx * a.cos(), y + h - rx + rx * a.sin())
                    }
                    2 => Point::new(x + w, y + h - rx - straight_h * u),
                    3 => {
                        let a = -u * PI / 2.0;
                        Point::new(x + w - rx + rx * a.cos(), y + rx + rx * a.sin())
                    }
                    4 => Point::new(x + w - rx - straight_w * u, y),
                    5 => {
                        let a = -PI / 2.0 - u * PI / 2.0;
                        Point::new(x + rx + rx * a.cos(), y + rx + rx * a.sin())
                    }
                    6 => Point::new(x, y + rx + straight_h * u),
                    _ => {
                        let a = PI - u * PI / 2.0;
                        Point::new(x + rx + rx * a.cos(), y + h - rx + rx * a.sin())
                    }
                };
            }
            s -= len;
        }
        Point::new(x + rx, y + h)
    };
    sample(n, 0.0, total, true, sigma, seed, curve)
}

/// MDL cost of a `fit_path` description, scored exactly as the primitive is: distance
/// from every measured point to the emitted geometry, plus lambda per parameter.
///
/// `curves::chi2` walks the points and the path together, so a closed path — which
/// `fit_path` starts at its own cut point — has to be scored against the points rotated
/// to begin where the path begins.
fn cubic_cost(pts: &[Point], sigma: &[f64], path: &FittedPath, cfg: &FitConfig) -> f64 {
    let n = pts.len();
    let (run, run_sigma): (Vec<Point>, Vec<f64>) = if path.closed {
        let cut = (0..n)
            .min_by(|&a, &b| pts[a].dist(path.start).total_cmp(&pts[b].dist(path.start)))
            .unwrap();
        (
            (0..=n).map(|k| pts[(cut + k) % n]).collect(),
            (0..=n).map(|k| sigma[(cut + k) % n]).collect(),
        )
    } else {
        (pts.to_vec(), sigma.to_vec())
    };
    let chi2 = curves::chi2(&run, &run_sigma, path.start, &path.segments);
    0.5 * chi2 + cfg.lambda * path.params()
}

/// A boundary is stored once but bounds two faces, and runs the opposite way around each
/// of them; the path form has to follow the run's own direction, from a start anywhere
/// on the outline.
fn assert_path_form_follows_reversed_run(pts: &[Point], sigma: &[f64], cfg: &FitConfig) {
    let mut rev: Vec<Point> = pts.to_vec();
    rev.reverse();
    rev.rotate_left(pts.len() / 3);
    let (segs, prim, _) = fit_primitive_or_arcs(&rev, sigma, true, cfg).expect("primitive");
    assert!(prim.is_some());
    assert_eq!(
        segs.last().unwrap().end(),
        rev[0],
        "closed path returns to its start"
    );
    let dev = curves::max_deviation(&rev, rev[0], &segs);
    assert!(dev < 4.0 * SIGMA, "reversed path form deviates {dev}");
}

#[test]
fn circle_is_recovered_and_beats_cubics() {
    let cfg = FitConfig::default();
    let (c0, r0) = (Point::new(120.3, 77.1), 40.0);
    let pts = circle_pts(320, c0, r0, SIGMA, 7);
    let sigma = vec![SIGMA; pts.len()];

    let cf = fit_circle(&pts, &sigma).expect("circle fit");
    assert!((cf.r - r0).abs() < 0.02, "radius {} vs {}", cf.r, r0);
    assert!(cf.c.dist(c0) < 0.02, "centre {:?} vs {:?}", cf.c, c0);

    let (segs, prim, cost) = fit_primitive_or_arcs(&pts, &sigma, true, &cfg).expect("primitive");
    let prim = prim.expect("a closed circle should come back as a primitive");
    match prim.kind {
        PrimitiveKind::Circle { c, r } => {
            assert!((r - r0).abs() < 0.02);
            assert!(c.dist(c0) < 0.02);
        }
        other => panic!("expected a circle, got {other:?}"),
    }
    assert_eq!(prim.params, 3.0);
    assert!(segs.iter().all(|s| matches!(s, Segment::Arc { .. })));
    assert_eq!(segs.len(), (360.0 / MAX_ARC_DEGREES).ceil() as usize);

    // The path form must trace the same circle: every measured point within a few sigma.
    let dev = curves::max_deviation(&pts, pts[0], &segs);
    assert!(dev < 4.0 * SIGMA, "arc path deviates {dev}");
    assert_path_form_follows_reversed_run(&pts, &sigma, &cfg);

    let poly = Polyline::with_uniform_sigma(pts.clone(), SIGMA, true);
    let path = fit_path(&poly, &cfg);
    let cubic = cubic_cost(&pts, &sigma, &path, &cfg);
    assert!(
        cost < cubic,
        "primitive cost {cost:.1} should beat cubic cost {cubic:.1} ({} segments)",
        path.len()
    );
}

#[test]
fn ellipse_is_recovered_and_beats_cubics() {
    let cfg = FitConfig::default();
    let (c0, rx0, ry0, ang0) = (Point::new(90.0, 64.5), 50.0, 30.0, 0.4);
    let pts = ellipse_pts(360, c0, rx0, ry0, ang0, SIGMA, 11);
    let sigma = vec![SIGMA; pts.len()];

    let e = fit_ellipse(&pts, &sigma).expect("ellipse fit");
    assert!((e.rx - rx0).abs() < 0.02, "rx {} vs {}", e.rx, rx0);
    assert!((e.ry - ry0).abs() < 0.02, "ry {} vs {}", e.ry, ry0);
    assert!(e.c.dist(c0) < 0.02, "centre {:?} vs {:?}", e.c, c0);
    assert!(
        (e.angle - ang0).abs() < 1e-3,
        "angle {} vs {}",
        e.angle,
        ang0
    );

    let (segs, prim, cost) = fit_primitive_or_arcs(&pts, &sigma, true, &cfg).expect("primitive");
    let prim = prim.expect("a closed ellipse should come back as a primitive");
    assert!(
        matches!(prim.kind, PrimitiveKind::Ellipse { .. }),
        "{:?}",
        prim.kind
    );
    assert_eq!(prim.params, 5.0);
    let dev = curves::max_deviation(&pts, pts[0], &segs);
    assert!(dev < 4.0 * SIGMA, "cubic path form deviates {dev}");
    assert_path_form_follows_reversed_run(&pts, &sigma, &cfg);

    let poly = Polyline::with_uniform_sigma(pts.clone(), SIGMA, true);
    let path = fit_path(&poly, &cfg);
    let cubic = cubic_cost(&pts, &sigma, &path, &cfg);
    assert!(cost < cubic, "primitive {cost:.1} vs cubic {cubic:.1}");
}

#[test]
fn a_square_is_not_a_circle() {
    let cfg = FitConfig::default();
    let pts = round_rect_pts(320, [20.0, 30.0, 80.0, 80.0, 0.0], SIGMA, 3);
    let sigma = vec![SIGMA; pts.len()];
    if let Some((_, Some(p), _)) = fit_primitive_or_arcs(&pts, &sigma, true, &cfg) {
        assert!(
            !matches!(
                p.kind,
                PrimitiveKind::Circle { .. } | PrimitiveKind::Ellipse { .. }
            ),
            "square reported as {:?}",
            p.kind
        );
        if let PrimitiveKind::RoundRect { x, y, w, h, rx } = p.kind {
            assert!((x - 20.0).abs() < 0.05 && (y - 30.0).abs() < 0.05);
            assert!((w - 80.0).abs() < 0.1 && (h - 80.0).abs() < 0.1);
            assert!(rx.abs() < 0.1, "square came back with rx {rx}");
        }
    }
    // And a circle fit forced onto it must be rejected by its own residual.
    assert!(fit_arcs(&pts, &sigma, true).is_none());
}

#[test]
fn rounded_rect_is_recovered() {
    let cfg = FitConfig::default();
    let (x0, y0, w0, h0, rx0) = (15.0, 22.0, 90.0, 56.0, 12.0);
    let pts = round_rect_pts(400, [x0, y0, w0, h0, rx0], SIGMA, 5);
    let sigma = vec![SIGMA; pts.len()];

    let rr = fit_round_rect(&pts, &sigma, None).expect("round rect fit");
    assert!((rr.rx - rx0).abs() < 0.1, "rx {} vs {}", rr.rx, rx0);
    assert!(
        (rr.x - x0).abs() < 0.05 && (rr.y - y0).abs() < 0.05,
        "{rr:?}"
    );
    assert!((rr.w - w0).abs() < 0.1 && (rr.h - h0).abs() < 0.1, "{rr:?}");

    let (segs, prim, cost) = fit_primitive_or_arcs(&pts, &sigma, true, &cfg).expect("primitive");
    let prim = prim.expect("primitive");
    match prim.kind {
        PrimitiveKind::RoundRect { rx, .. } => assert!((rx - rx0).abs() < 0.1),
        other => panic!("expected a rounded rect, got {other:?}"),
    }
    assert_eq!(prim.params, 6.0);
    // Path form: four lines and four quarter arcs (the start may split one piece in two).
    let arcs = segs
        .iter()
        .filter(|s| matches!(s, Segment::Arc { .. }))
        .count();
    let lines = segs
        .iter()
        .filter(|s| matches!(s, Segment::Line(_)))
        .count();
    assert!(
        (4..=5).contains(&arcs) && (4..=5).contains(&lines),
        "{arcs} arcs, {lines} lines"
    );
    let dev = curves::max_deviation(&pts, pts[0], &segs);
    assert!(dev < 4.0 * SIGMA, "round-rect path form deviates {dev}");
    assert_path_form_follows_reversed_run(&pts, &sigma, &cfg);

    let poly = Polyline::with_uniform_sigma(pts.clone(), SIGMA, true);
    let path = fit_path(&poly, &cfg);
    let cubic = cubic_cost(&pts, &sigma, &path, &cfg);
    assert!(cost < cubic, "primitive {cost:.1} vs cubic {cubic:.1}");
}

#[test]
fn algebraic_fit_is_measurably_worse_than_orthogonal_on_a_partial_arc() {
    // A short arc with realistic (not tiny) noise is where algebraic fitting is biased:
    // the residual it minimizes, x² + y² − r², is not a distance, and its minimizer
    // shrinks the radius. Averaged over seeds so this is a claim about the estimator,
    // not one draw. Three estimators: the plain algebraic fit (Kåsa), Taubin's
    // normalized algebraic fit (our initializer), and the orthogonal-distance fit.
    let (c0, r0) = (Point::new(50.0, 50.0), 30.0);
    let noise = 0.3;
    let trials = 60;
    let mut err = [0.0f64; 3];
    let mut chi2 = [0.0f64; 3];
    for seed in 0..trials {
        let pts = sample(
            30,
            0.0,
            60f64.to_radians(),
            false,
            noise,
            1000 + seed,
            |t| Point::new(c0.x + r0 * t.cos(), c0.y + r0 * t.sin()),
        );
        let sigma = vec![noise; pts.len()];
        let kasa = inkvec_fit::primitives::fit_circle_kasa(&pts, &sigma).unwrap();
        let taubin = fit_circle_algebraic(&pts, &sigma).unwrap();
        let odf = fit_circle(&pts, &sigma).unwrap();
        assert!(
            odf.chi2 <= taubin.chi2 + 1e-9,
            "refinement must not increase chi2"
        );
        for (k, f) in [kasa, taubin, odf].iter().enumerate() {
            err[k] += f.r - r0;
            chi2[k] += f.chi2;
        }
    }
    let t = trials as f64;
    eprintln!(
        "circle 60deg arc, sigma {noise}: mean radius bias kasa {:+.3} taubin {:+.3} odf {:+.3}; \
         mean chi2 kasa {:.2} taubin {:.2} odf {:.2}",
        err[0] / t,
        err[1] / t,
        err[2] / t,
        chi2[0] / t,
        chi2[1] / t,
        chi2[2] / t
    );
    // The plain algebraic fit is biased toward small radii and fits measurably worse.
    assert!(err[0] / t < -0.05, "kasa bias {:+.3}", err[0] / t);
    assert!(
        chi2[2] < 0.98 * chi2[0],
        "odf chi2 {} vs kasa {}",
        chi2[2] / t,
        chi2[0] / t
    );
    assert!(err[2].abs() < err[0].abs());

    // Taubin's normalization removes most of that bias, which is why it is the
    // initializer: the refinement's gain over it is small in chi2 (about one percent
    // here) but it is never worse, and it is what removes the bias that remains.
    assert!(chi2[2] <= chi2[1] + 1e-9);

    // Ellipses. Same estimators (Taubin conic against orthogonal distance) on a partial
    // arc of a 60x25 ellipse.
    let (rx0, ry0) = (60.0, 25.0);
    let ellipse_arc = |deg: f64, noise: f64, n: usize, seed: u64| -> Vec<Point> {
        sample(n, 0.3, 0.3 + deg.to_radians(), false, noise, seed, |t| {
            Point::new(c0.x + rx0 * t.cos(), c0.y + ry0 * t.sin())
        })
    };
    let (mut alg_err, mut odf_err, mut alg_chi2, mut odf_chi2) = (0.0, 0.0, 0.0, 0.0);
    for seed in 0..trials {
        let pts = ellipse_arc(120.0, 0.3, 40, 2000 + seed);
        let sigma = vec![0.3; pts.len()];
        let odf = fit_ellipse(&pts, &sigma).unwrap();
        let alg = fit_ellipse_algebraic(&pts, &sigma).expect("120 degrees is enough");
        assert!(odf.chi2 <= alg.chi2 + 1e-9);
        odf_err += (odf.rx - rx0).abs() + (odf.ry - ry0).abs();
        odf_chi2 += odf.chi2;
        alg_err += (alg.rx - rx0).abs() + (alg.ry - ry0).abs();
        alg_chi2 += alg.chi2;
    }
    eprintln!(
        "ellipse 120deg arc, sigma 0.3: algebraic axis err {:.3} chi2 {:.2}; odf axis err {:.3} chi2 {:.2}",
        alg_err / t,
        alg_chi2 / t,
        odf_err / t,
        odf_chi2 / t
    );
    assert!(odf_err < alg_err, "refinement should reduce the axis error");
    assert!(odf_chi2 < alg_chi2);

    // On a short, noisy arc the unconstrained conic fit is not even guaranteed to be an
    // ellipse; the orthogonal-distance fit, started from a circle when it is not, always
    // returns one. That robustness is the other half of the refinement's justification.
    let mut alg_failed = 0;
    for seed in 0..trials {
        let pts = ellipse_arc(80.0, 0.5, 30, 3000 + seed);
        let sigma = vec![0.5; pts.len()];
        assert!(fit_ellipse(&pts, &sigma).is_some());
        if fit_ellipse_algebraic(&pts, &sigma).is_none() {
            alg_failed += 1;
        }
    }
    eprintln!("ellipse 80deg arc, sigma 0.5: algebraic fit not an ellipse in {alg_failed}/{trials} trials");
    assert!(alg_failed > 0);
}

#[test]
fn open_arc_run_becomes_arc_segments() {
    let cfg = FitConfig::default();
    let (c0, r0) = (Point::new(60.0, 60.0), 45.0);
    let pts = sample(200, 0.2, 0.2 + 250f64.to_radians(), false, SIGMA, 21, |t| {
        Point::new(c0.x + r0 * t.cos(), c0.y + r0 * t.sin())
    });
    let sigma = vec![SIGMA; pts.len()];
    let segs = fit_arcs(&pts, &sigma, false).expect("arcs");
    // 250 degrees at <= 120 each: three arcs.
    assert_eq!(segs.len(), 3);
    for s in &segs {
        match s {
            Segment::Arc {
                rx, ry, phi, sweep, ..
            } => {
                assert!((rx - r0).abs() < 0.05);
                assert!((rx - ry).abs() < 1e-9, "a circle's arc has equal radii");
                assert_eq!(*phi, 0.0, "a circle's arc has no tilt");
                assert!(*sweep, "sampled in increasing angle");
            }
            other => panic!("expected an arc, got {other:?}"),
        }
    }
    assert_eq!(segs.last().unwrap().end(), *pts.last().unwrap());
    let dev = curves::max_deviation(&pts, pts[0], &segs);
    assert!(dev < 4.0 * SIGMA, "arc run deviates {dev}");

    let (segs2, prim, cost) = fit_primitive_or_arcs(&pts, &sigma, false, &cfg).expect("arcs");
    assert!(prim.is_none(), "an open run is never a whole primitive");
    assert_eq!(segs2.len(), 3);
    let poly = Polyline::with_uniform_sigma(pts.clone(), SIGMA, false);
    let path = fit_path(&poly, &cfg);
    let cubic = cubic_cost(&pts, &sigma, &path, &cfg) - 2.0 * cfg.lambda;
    assert!(cost < cubic, "arcs {cost:.1} vs cubics {cubic:.1}");
}

#[test]
fn a_straight_run_is_not_an_arc() {
    let pts: Vec<Point> = (0..50)
        .map(|k| Point::new(10.0 + k as f64, 20.0 + 0.5 * k as f64))
        .collect();
    let sigma = vec![SIGMA; pts.len()];
    assert!(fit_arcs(&pts, &sigma, false).is_none());
    assert!(fit_primitive_or_arcs(&pts, &sigma, false, &FitConfig::default()).is_none());
}

#[test]
fn arc_center_round_trips_and_reversal_preserves_geometry() {
    let c = Point::new(30.0, 40.0);
    let r = 25.0;
    for (a0, delta) in [
        (0.3f64, 1.2f64),
        (2.0, -1.9),
        (-1.0, 2.9),
        (0.0, -0.5),
        (1.0, 4.0),
    ] {
        let start = Point::new(c.x + r * a0.cos(), c.y + r * a0.sin());
        let end = Point::new(c.x + r * (a0 + delta).cos(), c.y + r * (a0 + delta).sin());
        let large = delta.abs() > PI;
        let sweep = delta > 0.0;
        let (cc, rr, th, d) = arc_center(start, r, large, sweep, end);
        assert!(cc.dist(c) < 1e-9, "{cc:?} vs {c:?}");
        assert!((rr - r).abs() < 1e-9);
        assert!(((th - a0 + PI).rem_euclid(TAU) - PI).abs() < 1e-9);
        assert!((d - delta).abs() < 1e-9, "sweep {d} vs {delta}");

        let seg = Segment::circular_arc(r, large, sweep, end);
        let fwd = FittedPath {
            start,
            segments: vec![seg],
            closed: false,
        };
        let rev = fwd.reversed();
        assert_eq!(rev.start, end);
        let Segment::Arc {
            rx: rr2,
            large_arc: la2,
            sweep: sw2,
            end: end2,
            ..
        } = rev.segments[0].clone()
        else {
            panic!("reversed arc is not an arc");
        };
        assert_eq!(end2, start);
        let (cc2, rr2, _, d2) = arc_center(rev.start, rr2, la2, sw2, end2);
        assert!(cc2.dist(c) < 1e-9, "reversed centre {cc2:?} vs {c:?}");
        assert!((rr2 - r).abs() < 1e-9);
        assert!(
            (d2 + delta).abs() < 1e-9,
            "reversed sweep {d2} vs {}",
            -delta
        );

        // Sample the forward arc densely and check both directions pass through it.
        let samples: Vec<Point> = (0..=50)
            .map(|i| {
                let a = a0 + delta * i as f64 / 50.0;
                Point::new(c.x + r * a.cos(), c.y + r * a.sin())
            })
            .collect();
        let mut back = samples.clone();
        back.reverse();
        assert!(curves::max_deviation(&back, rev.start, &rev.segments) < 1e-3);
        assert!(curves::max_deviation(&samples, fwd.start, &fwd.segments) < 1e-3);
    }
}
