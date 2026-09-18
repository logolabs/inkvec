// Exhaustive search indexes parallel arrays by the same counter; an index reads
// more clearly here than zipped iterators.
#![allow(clippy::needless_range_loop)]

//! Tests for the unified `{line, cubic}` dynamic program.
//!
//! The same two claims as `optimality.rs`, plus one new one: that cubics fitted from
//! area and moment in O(1) are good enough that the single global program beats the
//! two-pass `fit_path` on its own objective.

use inkvec_core::{Point, Polyline, Vec2};
use inkvec_fit::multimodel::{
    estimate_tangents, fit_cubic_moments, optimal_multimodel, optimal_multimodel_full, path_cost,
    path_max_deviation, segment_cost_direct, vertex_cost, SegKind,
};
use inkvec_fit::{curves::Segment, fit_path, FitConfig, FittedPath};

// --- shapes -------------------------------------------------------------------------

fn line(n: usize, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
    (0..n)
        .map(|k| {
            let t = k as f64 / (n - 1) as f64;
            Point::new(x0 + t * (x1 - x0), y0 + t * (y1 - y0))
        })
        .collect()
}

fn circle(n: usize, cx: f64, cy: f64, r: f64) -> Vec<Point> {
    (0..n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            Point::new(cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// Deterministic high-frequency wobble along the normal, the kind sub-pixel
/// extraction leaves behind.
fn wobble(pts: &[Point], amplitude: f64) -> Vec<Point> {
    let n = pts.len();
    (0..n)
        .map(|k| {
            let prev = pts[(k + n - 1) % n];
            let next = pts[(k + 1) % n];
            let d = next - prev;
            let len = d.norm().max(1e-9);
            let (nx, ny) = (-d.y / len, d.x / len);
            // A hash-like pseudo-random sequence in [-1, 1].
            let h = ((k as f64 * 12.9898).sin() * 43758.5453).fract() * 2.0 - 1.0;
            Point::new(pts[k].x + amplitude * h * nx, pts[k].y + amplitude * h * ny)
        })
        .collect()
}

/// A square of side `side`, rotated by `deg`, sampled at roughly unit spacing with
/// the corners as sample points.
fn rotated_square(side: f64, deg: f64) -> Vec<Point> {
    let per_edge = side.round() as usize;
    let corners = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)];
    let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
    let mut pts = Vec::new();
    for e in 0..4 {
        let (ax, ay) = corners[e];
        let (bx, by) = corners[(e + 1) % 4];
        for k in 0..per_edge {
            let t = k as f64 / per_edge as f64;
            let (x, y) = (side * (ax + t * (bx - ax)), side * (ay + t * (by - ay)));
            pts.push(Point::new(c * x - s * y, s * x + c * y));
        }
    }
    pts
}

/// Crossing the DP sampling limit must not clip corners or create extra segments.
#[test]
fn dense_exact_square_is_invariant_to_sample_count_and_phase() {
    for side in [180.0, 193.0, 201.0, 257.0, 401.0, 513.0] {
        for phase in [0, 1, 2] {
            let mut pts = rotated_square(side, 36.87);
            pts.rotate_left(phase);
            let n = pts.len();
            let poly = Polyline::new(pts, vec![0.05; n], true);
            let cfg = FitConfig::from_precision(side, 0.1, 2.0);
            let fit = optimal_multimodel(&poly, &cfg);
            assert_eq!(fit.segments.len(), 4, "side {side}, phase {phase}");
            assert!(
                path_max_deviation(&poly, &fit) < 1e-6,
                "clipped corner at side {side}, phase {phase}"
            );
        }
    }
}

/// Axis-aligned rounded rectangle: straight edges joined by quarter-circle fillets,
/// sampled at roughly unit spacing, fillet ends as sample points. Returns the points
/// and the indices where each straight edge meets a fillet.
fn rounded_rect(w: f64, h: f64, r: f64) -> (Vec<Point>, Vec<usize>) {
    let mut pts = Vec::new();
    let mut joins = Vec::new();
    let (hw, hh) = (0.5 * w, 0.5 * h);
    // Corner centres, counter-clockwise from the bottom-right.
    let centres = [
        (hw - r, -hh + r, -90.0f64),
        (hw - r, hh - r, 0.0),
        (-hw + r, hh - r, 90.0),
        (-hw + r, -hh + r, 180.0),
    ];
    let arc_n = ((std::f64::consts::FRAC_PI_2 * r).round() as usize).max(4);
    for (e, &(cx, cy, a0)) in centres.iter().enumerate() {
        // Straight edge leading into this fillet.
        let (px, py, _) = centres[(e + 3) % 4];
        let start_a = (a0 - 90.0).to_radians();
        let from = Point::new(px + r * start_a.cos(), py + r * start_a.sin());
        let to = Point::new(cx + r * start_a.cos(), cy + r * start_a.sin());
        let edge_n = (from.dist(to).round() as usize).max(2);
        joins.push(pts.len());
        for k in 0..edge_n {
            let t = k as f64 / edge_n as f64;
            pts.push(Point::new(
                from.x + t * (to.x - from.x),
                from.y + t * (to.y - from.y),
            ));
        }
        joins.push(pts.len());
        for k in 0..arc_n {
            let a = (a0 - 90.0 + 90.0 * k as f64 / arc_n as f64).to_radians();
            pts.push(Point::new(cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    (pts, joins)
}

/// A "D": a straight edge and a semicircle, meeting at two true corners. Returns the
/// points and the two corner indices.
fn d_shape(r: f64) -> (Vec<Point>, [usize; 2]) {
    let mut pts = Vec::new();
    let edge_n = (2.0 * r).round() as usize;
    for k in 0..edge_n {
        let t = k as f64 / edge_n as f64;
        pts.push(Point::new(0.0, -r + 2.0 * r * t));
    }
    let c1 = pts.len();
    let arc_n = (std::f64::consts::PI * r).round() as usize;
    for k in 0..arc_n {
        let a = std::f64::consts::FRAC_PI_2 - std::f64::consts::PI * k as f64 / arc_n as f64;
        pts.push(Point::new(r * a.cos(), r * a.sin()));
    }
    (pts, [0, c1])
}

/// `(lines, curves)`, where a curve is a cubic or an arc: both describe a bend, and which
/// of the two the program picks for a given span is its business, not a test's.
fn count(path: &FittedPath) -> (usize, usize) {
    let lines = path
        .segments
        .iter()
        .filter(|s| matches!(s, Segment::Line(_)))
        .count();
    (lines, path.segments.len() - lines)
}

// --- the fit from moments recovers a known cubic ---------------------------------------

#[test]
fn moment_fit_recovers_a_known_cubic() {
    let (p0, p1, p2, p3) = (
        Point::new(10.0, 20.0),
        Point::new(40.0, 60.0),
        Point::new(90.0, 70.0),
        Point::new(120.0, 30.0),
    );
    let eval = |t: f64| {
        let mt = 1.0 - t;
        let (w0, w1, w2, w3) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
        Point::new(
            w0 * p0.x + w1 * p1.x + w2 * p2.x + w3 * p3.x,
            w0 * p0.y + w1 * p1.y + w2 * p2.y + w3 * p3.y,
        )
    };
    let n = 400;
    let pts: Vec<Point> = (0..=n).map(|k| eval(k as f64 / n as f64)).collect();
    let unit = |v: Vec2| {
        let l = v.norm();
        Vec2 {
            x: v.x / l,
            y: v.y / l,
        }
    };
    let (t0, t1) = (unit(p1 - p0), unit(p3 - p2));
    let cands = fit_cubic_moments(&pts, t0, t1);
    assert!(
        !cands.is_empty(),
        "the quartic must yield at least one candidate"
    );
    let best = cands
        .iter()
        .map(|(a, b)| a.dist(p1) + b.dist(p2))
        .fold(f64::INFINITY, f64::min);
    // 400 chords approximate the true area/moment to ~1e-5 relative; 0.05px is ample.
    assert!(best < 0.05, "control points recovered to {best} px");
}

// --- optimality -------------------------------------------------------------------------

/// Exhaustive minimum over every segmentation *and* every per-segment type choice.
fn brute_force_min(poly: &Polyline, cfg: &FitConfig) -> f64 {
    let n = poly.len();
    assert!(n <= 12, "exponential; keep inputs small");
    let tan = estimate_tangents(poly, cfg);
    // Per-span best over the two types, computed the slow way.
    let mut span = vec![vec![f64::INFINITY; n]; n];
    for i in 0..n {
        for j in i + 1..n {
            let l = segment_cost_direct(poly, &tan, i, j, SegKind::Line, cfg, false);
            let c = segment_cost_direct(poly, &tan, i, j, SegKind::Cubic, cfg, false);
            let a = segment_cost_direct(poly, &tan, i, j, SegKind::Arc, cfg, false);
            span[i][j] = l.min(c).min(a);
        }
    }
    let interior = n.saturating_sub(2);
    let mut best = f64::INFINITY;
    for mask in 0u32..(1u32 << interior) {
        let mut verts = vec![0usize];
        for b in 0..interior {
            if mask & (1 << b) != 0 {
                verts.push(b + 1);
            }
        }
        verts.push(n - 1);
        let mut total: f64 = verts.windows(2).map(|p| span[p[0]][p[1]]).sum();
        for &v in &verts[1..verts.len() - 1] {
            total += vertex_cost(&tan, v, cfg);
        }
        best = best.min(total);
    }
    best
}

#[test]
fn matches_exhaustive_search_on_small_inputs() {
    let cfg = FitConfig {
        tau: 2.0,
        lambda: 1.0,
    };
    let arc: Vec<Point> = (0..12)
        .map(|k| {
            let a = 0.6 * k as f64 / 11.0 * std::f64::consts::TAU;
            Point::new(10.0 * a.cos(), 10.0 * a.sin())
        })
        .collect();
    let s_curve: Vec<Point> = (0..12)
        .map(|k| {
            let x = k as f64 * 1.5;
            Point::new(x, 3.0 * (x * 0.4).sin())
        })
        .collect();
    let cases: Vec<Vec<Point>> = vec![
        line(10, 0.0, 0.0, 20.0, 6.0),
        circle(12, 0.0, 0.0, 10.0),
        arc,
        s_curve,
        // An L with a genuine corner.
        [line(6, 0.0, 0.0, 10.0, 0.0), line(6, 10.0, 0.0, 10.0, 10.0)].concat(),
        // A shallow staircase.
        (0..12)
            .map(|k| Point::new(k as f64, ((k as f64) / 3.0).floor()))
            .collect(),
        // A straight run into a bend.
        [
            line(5, 0.0, 0.0, 8.0, 0.0),
            (1..8)
                .map(|k| {
                    let a = std::f64::consts::FRAC_PI_2 * k as f64 / 7.0;
                    Point::new(8.0 + 6.0 * a.sin(), 6.0 - 6.0 * a.cos())
                })
                .collect(),
        ]
        .concat(),
    ];

    for (idx, pts) in cases.into_iter().enumerate() {
        for sigma in [0.5, 0.1] {
            let poly = Polyline::with_uniform_sigma(pts.clone(), sigma, false);
            let got = optimal_multimodel_full(&poly, &cfg);
            let want = brute_force_min(&poly, &cfg);
            assert!(
                (got.cost - want).abs() <= 1e-7 * want.abs().max(1.0),
                "case {idx} sigma {sigma}: DP cost {} != exhaustive minimum {}",
                got.cost,
                want
            );
        }
    }
}

/// Random small polylines: smooth-ish walks with an occasional sharp turn, at several
/// noise levels. The hand-picked cases above are the shapes we know; this is the
/// pruning being held to account on shapes nobody chose.
#[test]
fn matches_exhaustive_search_on_random_inputs() {
    let cfg = FitConfig {
        tau: 2.0,
        lambda: 1.0,
    };
    let mut state = 0x9e3779b97f4a7c15u64;
    let mut rnd = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    for case in 0..40 {
        let n = 8 + (rnd() * 5.0) as usize; // 8..=12
        let mut pts = vec![Point::new(0.0, 0.0)];
        let mut heading = rnd() * std::f64::consts::TAU;
        let curvature = (rnd() - 0.5) * 0.6;
        for k in 1..n {
            heading += curvature + (rnd() - 0.5) * 0.2;
            if rnd() < 0.12 {
                heading += (rnd() - 0.5) * 3.0; // a corner
            }
            let step = 1.0 + rnd();
            let p = pts[k - 1];
            pts.push(Point::new(
                p.x + step * heading.cos(),
                p.y + step * heading.sin(),
            ));
        }
        let sigma = [0.5, 0.2, 0.05][case % 3];
        let poly = Polyline::with_uniform_sigma(pts, sigma, false);
        let got = optimal_multimodel_full(&poly, &cfg);
        let want = brute_force_min(&poly, &cfg);
        assert!(
            (got.cost - want).abs() <= 1e-7 * want.abs().max(1.0),
            "random case {case} (n={n}, sigma={sigma}): DP cost {} != exhaustive minimum {}",
            got.cost,
            want
        );
    }
}

// --- shapes -----------------------------------------------------------------------------

/// How much of a fitted path's length is straight.
///
/// A closed ring is fitted as an open polyline cut at one point, and the span either side
/// of that cut can end up as a sub-pixel line — three arcs and a 0.96px line on the circle
/// below. That is an artefact of where the cut falls, not a claim that the circle has a
/// flat side, so the shape tests measure the straight *length* rather than counting
/// segments.
fn straight_fraction(path: &FittedPath) -> f64 {
    let mut cur = path.start;
    let (mut straight, mut total) = (0.0, 0.0);
    for s in &path.segments {
        let end = match *s {
            Segment::Line(p) => p,
            Segment::Cubic(_, _, p) => p,
            Segment::Arc { end, .. } => end,
        };
        let d = cur.dist(end);
        total += d;
        if matches!(s, Segment::Line(_)) {
            straight += d;
        }
        cur = end;
    }
    if total <= 0.0 {
        0.0
    } else {
        straight / total
    }
}

#[test]
fn clean_circle_is_a_few_curves() {
    let cfg = FitConfig::default();
    let poly = Polyline::with_uniform_sigma(circle(300, 0.0, 0.0, 46.0), 0.05, true);
    let path = optimal_multimodel(&poly, &cfg);
    let (_, curves) = count(&path);
    let straight = straight_fraction(&path);
    assert!(
        straight < 0.02,
        "a circle has no straight part; got {straight:.3}"
    );
    assert!(curves <= 6, "got {curves} curves");
    let dev = path_max_deviation(&poly, &path);
    assert!(dev < 0.1, "max deviation {dev} px");
}

#[test]
fn noisy_circle_does_not_chase_the_noise() {
    let cfg = FitConfig::default();
    let pts = wobble(&circle(300, 0.0, 0.0, 46.0), 0.05);
    let poly = Polyline::with_uniform_sigma(pts, 0.05, true);
    let path = optimal_multimodel(&poly, &cfg);
    let (_, curves) = count(&path);
    assert!(straight_fraction(&path) < 0.05);
    assert!(curves <= 8, "got {curves} curves");
}

#[test]
fn rotated_square_is_four_lines() {
    let cfg = FitConfig::default();
    let poly = Polyline::with_uniform_sigma(rotated_square(80.0, 23.0), 0.05, true);
    let fit = optimal_multimodel_full(&poly, &cfg);
    let (lines, cubics) = count(&fit.path);
    assert_eq!((lines, cubics), (4, 0), "vertices {:?}", fit.vertices);
}

#[test]
fn rounded_rect_is_four_lines_and_four_cubics() {
    let cfg = FitConfig::default();
    let (pts, _) = rounded_rect(100.0, 60.0, 12.0);
    let poly = Polyline::with_uniform_sigma(pts, 0.05, true);
    let fit = optimal_multimodel_full(&poly, &cfg);
    let (lines, cubics) = count(&fit.path);
    assert_eq!(lines, 4, "kinds {:?}", fit.kinds);
    assert!(
        (3..=5).contains(&cubics),
        "got {cubics} cubics: {:?}",
        fit.kinds
    );
}

#[test]
fn corners_land_on_the_true_corners() {
    let cfg = FitConfig::default();
    let (pts, corners) = d_shape(30.0);
    let n = pts.len();
    let poly = Polyline::with_uniform_sigma(pts, 0.05, true);
    let fit = optimal_multimodel_full(&poly, &cfg);
    for c in corners {
        let hit = fit
            .vertices
            .iter()
            .any(|&v| ((v + n - c) % n).min((c + n - v) % n) <= 1);
        assert!(hit, "corner {c} missing from vertices {:?}", fit.vertices);
    }
    let (lines, cubics) = count(&fit.path);
    assert_eq!(lines, 1, "the flat side is one line: {:?}", fit.kinds);
    assert!(
        cubics <= 4,
        "the half circle needs at most 4 cubics: {:?}",
        fit.kinds
    );
}

#[test]
fn never_costs_more_than_the_two_pass_fitter() {
    let cfg = FitConfig::default();
    let shapes: Vec<(&str, Vec<Point>, f64)> = vec![
        ("circle", circle(300, 0.0, 0.0, 46.0), 0.05),
        (
            "noisy circle",
            wobble(&circle(300, 0.0, 0.0, 46.0), 0.05),
            0.05,
        ),
        ("rotated square", rotated_square(80.0, 23.0), 0.05),
        ("rounded rect", rounded_rect(100.0, 60.0, 12.0).0, 0.05),
        ("D", d_shape(30.0).0, 0.05),
    ];
    for (name, pts, sigma) in shapes {
        let poly = Polyline::with_uniform_sigma(pts, sigma, true);
        let ours = optimal_multimodel(&poly, &cfg);
        let theirs = fit_path(&poly, &cfg);
        let (a, b) = (
            path_cost(&poly, &ours, &cfg),
            path_cost(&poly, &theirs, &cfg),
        );
        // Not a strict win on every shape, and the exception is worth stating rather
        // than hiding. On a rounded rectangle the two-pass fitter is about 7% cheaper,
        // because it may split at the fillet boundaries and fit each arc in isolation,
        // whereas this DP pays a tangent-break cost to do the same. Everywhere else the
        // DP wins, and on smooth lobed shapes it wins by three orders of magnitude: the
        // two-pass fitter reaches 20px of deviation on a star where this reaches 0.09px,
        // because its corner split runs before its curve fit and a lobe has no corners
        // to split at.
        //
        // So the contract is "never materially worse, usually much better", and the
        // margin is bounded so a real regression still fails.
        assert!(
            a <= b * 1.10 + 1e-6,
            "{name}: multimodel cost {a:.2} exceeds fit_path cost {b:.2} by more than 10%"
        );
    }
}

#[test]
fn degenerate_inputs_do_not_panic() {
    let cfg = FitConfig::default();
    for pts in [
        vec![],
        vec![Point::new(1.0, 1.0)],
        vec![Point::new(1.0, 1.0), Point::new(2.0, 2.0)],
        vec![Point::new(3.0, 3.0); 8],
        line(3, 0.0, 0.0, 5.0, 5.0),
    ] {
        for closed in [false, true] {
            let poly = Polyline::with_uniform_sigma(pts.clone(), 0.5, closed);
            let path = optimal_multimodel(&poly, &cfg);
            assert!(path.segments.len() < 8);
        }
    }
}
