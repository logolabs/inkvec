//! Unit tests for the multimodel program that need its private pieces: known optimal
//! segmentations with hand-computed costs, a brute-force oracle over closed loops, the
//! decimation contract, the arm polish, the continuous refinement on hand-built
//! solutions, and exact values of the path evaluators.
//!
//! Reference values here are derived by hand from the objective `0.5·chi² + λ·params`
//! plus the break terms (`λ·min(1, (angle/10°)²)`), not read back from the code.

use super::*;

const DEG: f64 = std::f64::consts::PI / 180.0;

fn cfg(lambda: f64) -> FitConfig {
    FitConfig { tau: 2.0, lambda }
}

fn dir(deg: f64) -> Vec2 {
    Vec2 {
        x: (deg * DEG).cos(),
        y: (deg * DEG).sin(),
    }
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// `count + 1` points from `a` to `b` inclusive, evenly spaced.
fn run(a: Point, b: Point, count: usize) -> Vec<Point> {
    (0..=count)
        .map(|k| {
            let t = k as f64 / count as f64;
            Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y))
        })
        .collect()
}

/// `count` points of a left-turning circular arc of radius `r` that leaves `start` in
/// direction `dir_deg` and sweeps `sweep_deg`; `start` itself is not included.
fn arc_after(start: Point, dir_deg: f64, r: f64, sweep_deg: f64, count: usize) -> Vec<Point> {
    let d = dir_deg * DEG;
    let c = Point::new(start.x - r * d.sin(), start.y + r * d.cos());
    (1..=count)
        .map(|k| {
            let th = d + sweep_deg * DEG * k as f64 / count as f64;
            Point::new(c.x + r * th.sin(), c.y - r * th.cos())
        })
        .collect()
}

/// Tangents set by hand: `(incoming, outgoing)` per point.
fn tangents(t: Vec<(Vec2, Vec2)>) -> Tangents {
    Tangents {
        incoming: t.iter().map(|p| p.0).collect(),
        outgoing: t.iter().map(|p| p.1).collect(),
    }
}

fn end_of(s: &Segment) -> Point {
    match *s {
        Segment::Line(p) | Segment::Cubic(_, _, p) => p,
        Segment::Arc { end, .. } => end,
    }
}

fn unit_of(v: Vec2) -> Vec2 {
    unit(v).expect("non-degenerate")
}

// --- known optimal segmentations -------------------------------------------------

/// An exact open L: 10 unit steps along x, then 10 up. The optimum is two lines with a
/// vertex on the corner. Both lines fit exactly (chi² = 0) and the one-sided tangents
/// at the corner are the edge directions, so the price is `2λ + 2λ` for the lines and
/// `λ` for the 90° turn (saturated break): 5λ, nothing else.
#[test]
fn open_l_is_two_lines_costing_exactly_five_lambda() {
    let pts = [
        run(Point::new(0.0, 0.0), Point::new(10.0, 0.0), 10),
        run(Point::new(10.0, 0.0), Point::new(10.0, 10.0), 10)[1..].to_vec(),
    ]
    .concat();
    for lambda in [1.0, 3.5] {
        let poly = Polyline::with_uniform_sigma(pts.clone(), 0.05, false);
        let fit = optimal_multimodel_impl(&poly, &cfg(lambda), usize::MAX, false);
        assert_eq!(fit.vertices, vec![0, 10, 20]);
        assert_eq!(fit.kinds, vec![SegKind::Line, SegKind::Line]);
        assert!(
            close(fit.cost, 5.0 * lambda, 1e-9),
            "λ {lambda}: cost {}",
            fit.cost
        );
        // The emitted path goes through the true corner.
        assert_eq!(fit.path.segments.len(), 2);
        assert!(fit.path.start.dist(Point::new(0.0, 0.0)) < 1e-9);
        assert!(end_of(&fit.path.segments[0]).dist(Point::new(10.0, 0.0)) < 1e-9);
        assert!(end_of(&fit.path.segments[1]).dist(Point::new(10.0, 10.0)) < 1e-9);
    }
}

/// A closed axis-aligned square with its corners on samples: four exact lines (8λ)
/// and four 90° corners (4λ, the cut's included) = 12λ.
#[test]
fn closed_square_is_four_lines_costing_exactly_twelve_lambda() {
    let c = [(-6.0, -6.0), (6.0, -6.0), (6.0, 6.0), (-6.0, 6.0)];
    let mut pts = Vec::new();
    for e in 0..4 {
        let (a, b) = (c[e], c[(e + 1) % 4]);
        let side = run(Point::new(a.0, a.1), Point::new(b.0, b.1), 12);
        pts.extend_from_slice(&side[..12]);
    }
    let poly = Polyline::with_uniform_sigma(pts, 0.05, true);
    let fit = optimal_multimodel_impl(&poly, &cfg(2.0), usize::MAX, false);
    let mut corners: Vec<usize> = fit.vertices.clone();
    corners.sort_unstable();
    corners.dedup();
    assert_eq!(corners, vec![0, 12, 24, 36], "vertices {:?}", fit.vertices);
    assert_eq!(fit.kinds, vec![SegKind::Line; 4]);
    assert!(close(fit.cost, 24.0, 1e-9), "cost {}", fit.cost);
}

/// A quarter circle sampled exactly is one arc, priced at its five parameters and
/// nothing else: the residual about the true circle is zero and an open run's ends are
/// not joins, so no break is charged.
#[test]
fn open_quarter_circle_is_one_arc_costing_five_lambda() {
    let mut pts = vec![Point::new(0.0, 0.0)];
    pts.extend(arc_after(Point::new(0.0, 0.0), 0.0, 10.0, 90.0, 12));
    let poly = Polyline::with_uniform_sigma(pts, 0.02, false);
    let lambda = 2.0;
    let fit = optimal_multimodel_impl(&poly, &cfg(lambda), usize::MAX, false);
    assert_eq!(fit.vertices, vec![0, 12]);
    assert_eq!(fit.kinds, vec![SegKind::Arc]);
    assert!(close(fit.cost, 5.0 * lambda, 1e-6), "cost {}", fit.cost);
    match fit.path.segments[..] {
        [Segment::Arc {
            rx,
            ry,
            sweep,
            large_arc,
            end,
            ..
        }] => {
            assert!(
                close(rx, 10.0, 1e-9) && close(ry, 10.0, 1e-9),
                "r {rx} {ry}"
            );
            assert!(sweep && !large_arc);
            assert!(end.dist(Point::new(10.0, 10.0)) < 1e-9);
        }
        ref s => panic!("expected one arc, got {s:?}"),
    }
}

/// A straight run into a tangent quarter circle breaks exactly at the tangent point:
/// one sample either way puts a point 0.05 px off the other model, which at
/// sigma 0.005 costs 50 nats against a join that is nearly free.
#[test]
fn line_into_tangent_arc_breaks_at_the_tangent_point() {
    let mut pts = run(Point::new(-10.0, 0.0), Point::new(0.0, 0.0), 10);
    pts.extend(arc_after(Point::new(0.0, 0.0), 0.0, 10.0, 90.0, 16));
    let poly = Polyline::with_uniform_sigma(pts, 0.005, false);
    let lambda = 2.0;
    let fit = optimal_multimodel_impl(&poly, &cfg(lambda), usize::MAX, false);
    assert_eq!(fit.vertices, vec![0, 10, 26]);
    assert_eq!(fit.kinds, vec![SegKind::Line, SegKind::Arc]);
    // 2λ + 5λ plus whatever the tangent estimator's small disagreement at a curvature
    // jump charges; it is a small fraction of one corner.
    assert!(
        fit.cost >= 7.0 * lambda - 1e-9 && fit.cost < 7.3 * lambda,
        "cost {}",
        fit.cost
    );
}

// --- brute force over closed loops ----------------------------------------------

/// Every cyclic segmentation of a closed polyline, each span priced by
/// `segment_cost_direct` on the loop opened at the span's start, plus the turn cost of
/// every chosen vertex. Independent of the program's cut heuristics.
fn brute_force_closed(poly: &Polyline, cfg: &FitConfig) -> f64 {
    let n = poly.len();
    assert!(n <= 12, "exponential; keep inputs small");
    let tan = estimate_tangents(poly, cfg);
    // span[a][len]: cheapest model for the span from vertex a, len edges long.
    let mut span = vec![vec![f64::INFINITY; n + 1]; n];
    for (a, row) in span.iter_mut().enumerate() {
        let (opened, otan) = open_at(poly, &tan, a);
        for len in 1..=n {
            row[len] = [SegKind::Line, SegKind::Cubic, SegKind::Arc]
                .into_iter()
                .map(|k| segment_cost_direct(&opened, &otan, 0, len, k, cfg, true))
                .fold(f64::INFINITY, f64::min);
        }
    }
    let mut best = f64::INFINITY;
    for mask in 1u32..(1u32 << n) {
        let verts: Vec<usize> = (0..n).filter(|&b| mask & (1 << b) != 0).collect();
        let m = verts.len();
        let mut total = 0.0;
        for q in 0..m {
            let (a, b) = (verts[q], verts[(q + 1) % m]);
            let len = if m == 1 { n } else { (b + n - a) % n };
            total += span[a][len] + vertex_cost(&tan, a, cfg);
        }
        best = best.min(total);
    }
    best
}

/// A bean: an ellipse elongated along x with its top pushed in to a concave corner. The
/// sharpest turn (the dent) is not the point farthest from the centroid (the ends), so
/// the choice of first cut is exercised, not just the program from a lucky cut.
fn bean(n: usize, dent: f64) -> Vec<Point> {
    (0..n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            let (x, y) = (10.0 * a.cos(), 4.0 * a.sin());
            // Index n/4 is the top of the ellipse.
            if k == n / 4 {
                Point::new(x, y - dent)
            } else {
                Point::new(x, y)
            }
        })
        .collect()
}

fn small_square() -> Vec<Point> {
    let c = [(0.0, 0.0), (6.0, 0.0), (6.0, 6.0), (0.0, 6.0)];
    (0..4)
        .flat_map(|e| {
            let (a, b) = (c[e], c[(e + 1) % 4]);
            run(Point::new(a.0, a.1), Point::new(b.0, b.1), 3)[..3].to_vec()
        })
        .collect()
}

/// `(program, exhaustive)` costs of one closed case.
fn closed_costs(pts: &[Point], sigma: f64, lambda: f64) -> (f64, f64) {
    let poly = Polyline::with_uniform_sigma(pts.to_vec(), sigma, true);
    let c = cfg(lambda);
    let want = brute_force_closed(&poly, &c);
    assert!(want.is_finite(), "no finite segmentation");
    (
        optimal_multimodel_impl(&poly, &c, usize::MAX, false).cost,
        want,
    )
}

/// The closed program (cut at the sharpest corner, solve, re-cut once) reaches the
/// exhaustive optimum over every cyclic segmentation on these loops, and never goes
/// below it on any.
#[test]
fn closed_program_matches_brute_force_over_cyclic_segmentations() {
    let cases: Vec<(&str, Vec<Point>, f64, &[f64])> = vec![
        ("square", small_square(), 0.1, &[1.0, 2.5, 4.0]),
        ("bean", bean(12, 3.0), 0.1, &[1.0, 2.5, 4.0]),
        ("bean fine", bean(12, 3.0), 0.05, &[1.0, 2.5, 4.0]),
        ("bean deep", bean(11, 5.0), 0.2, &[1.0, 2.5, 4.0]),
        ("bean shallow", bean(12, 2.0), 0.1, &[1.0, 2.5, 4.0]),
        ("bean 10", bean(10, 3.0), 0.1, &[1.0, 2.5, 4.0]),
        ("bean 4", bean(12, 4.0), 0.2, &[1.0, 2.5, 4.0]),
        ("bean coarse", bean(12, 3.0), 0.3, &[1.0, 2.5]),
    ];
    for (name, pts, sigma, lambdas) in cases {
        for &lambda in lambdas {
            let (got, want) = closed_costs(&pts, sigma, lambda);
            assert!(
                close(got, want, 1e-7 * want.abs().max(1.0)),
                "{name} λ {lambda}: program {got} vs exhaustive {want}"
            );
        }
    }
    // Where the heuristic cut misses (see the ignored test below) it is still an
    // admissible segmentation, so never cheaper than the optimum.
    for (sigma, lambda) in [(0.5, 1.0), (0.2, 4.0)] {
        let (got, want) = closed_costs(&bean(12, 3.0), sigma, lambda);
        assert!(
            got >= want - 1e-7 * want,
            "below the optimum: {got} < {want}"
        );
    }
}

/// Where the two cuts both land off every optimal cycle the program pays for it. The
/// module documents the cut as a heuristic (the exact cyclic program is future work);
/// this pins how far it is from the optimum on small loops, and that it is never below.
#[test]
#[ignore = "LIMITATION: two-cut heuristic misses the cyclic optimum by up to 14 nats on a 12-point loop, multimodel.rs:1123-1142"]
fn closed_program_reaches_the_cyclic_optimum_everywhere() {
    for (sigma, lambda) in [
        (0.5, 1.0),
        (0.5, 2.5),
        (0.5, 4.0),
        (0.2, 2.5),
        (0.2, 4.0),
        (0.3, 4.0),
    ] {
        let (got, want) = closed_costs(&bean(12, 3.0), sigma, lambda);
        assert!(
            got >= want - 1e-7 * want.max(1.0),
            "below the optimum: {got} < {want}"
        );
        assert!(
            close(got, want, 1e-7 * want.max(1.0)),
            "sigma {sigma} λ {lambda}: program {got} vs exhaustive {want}"
        );
    }
}

// --- decimation above the point cap ----------------------------------------------

/// Above the program's point cap the boundary is decimated to every `stride`-th sample.
/// With the cap at 16, 40 points decimate at stride 3 to indices 0, 3, …, 39. Those
/// points are placed exactly on the x-axis and every other one 0.05 off it, so the fit
/// on the decimated grid is one exact line — cost `2λ` to the last bit of the residual —
/// while a fit on all 40 points would pay a residual for the off-axis ones.
#[test]
fn above_the_point_cap_the_program_runs_on_the_decimated_grid() {
    let pts: Vec<Point> = (0..40)
        .map(|k| Point::new(k as f64, if k % 3 == 0 { 0.0 } else { 0.05 }))
        .collect();
    let poly = Polyline::with_uniform_sigma(pts, 0.02, false);
    let lambda = 1.5;
    let fit = with_dp_max_points(16, || {
        optimal_multimodel_impl(&poly, &cfg(lambda), usize::MAX, false)
    });
    assert_eq!(fit.vertices, vec![0, 39]);
    assert_eq!(fit.kinds, vec![SegKind::Line]);
    assert!(close(fit.cost, 2.0 * lambda, 1e-9), "cost {}", fit.cost);
    // Without the cap the same input is not one free line: the off-axis points cost.
    let full = optimal_multimodel_impl(&poly, &cfg(lambda), usize::MAX, false);
    assert!(full.cost > 2.0 * lambda + 0.1, "full cost {}", full.cost);
}

/// A span cap exempts the program from decimation: at `max_span = 1` every edge is its
/// own segment, whatever the point cap, so the measured contour is reproduced.
#[test]
fn a_span_cap_is_never_decimated() {
    let pts: Vec<Point> = (0..40)
        .map(|k| Point::new(k as f64, 2.0 * ((k as f64) * 0.7).sin()))
        .collect();
    let poly = Polyline::with_uniform_sigma(pts.clone(), 0.1, false);
    let fit = with_dp_max_points(16, || optimal_multimodel_impl(&poly, &cfg(1.0), 1, false));
    assert_eq!(fit.vertices, (0..40).collect::<Vec<_>>());
    assert_eq!(fit.path.segments.len(), 39);
    for (s, p) in fit.path.segments.iter().zip(&pts[1..]) {
        assert!(end_of(s).dist(*p) < 1e-9);
    }
}

// --- segment_cost_direct -----------------------------------------------------------

/// Hand-priced spans: an exact line costs `2λ`; a join end whose polyline tangent is 5°
/// off the chord adds `λ·(5/10)²`; a span out of range or of zero length is infinite,
/// as is a cubic without an interior point; an exact circular arc costs `5λ`.
#[test]
fn segment_cost_direct_prices_hand_built_spans() {
    let pts = run(Point::new(0.0, 0.0), Point::new(8.0, 0.0), 8);
    let poly = Polyline::with_uniform_sigma(pts, 0.1, false);
    let lambda = 1.7;
    let c = cfg(lambda);
    let mut t = vec![(dir(0.0), dir(0.0)); 9];
    t[2].1 = dir(5.0);
    t[6].0 = dir(-10.0);
    let tan = tangents(t);
    let line = |i, j, joins| segment_cost_direct(&poly, &tan, i, j, SegKind::Line, &c, joins);
    assert!(close(line(0, 8, false), 2.0 * lambda, 1e-12));
    // Both ends are joins when the span is interior; only 2's and 6's tangents are off.
    assert!(close(line(0, 5, false), 2.0 * lambda, 1e-12));
    assert!(close(line(2, 5, false), 2.25 * lambda, 1e-9));
    assert!(close(line(3, 6, false), 3.0 * lambda, 1e-9));
    assert!(close(line(2, 6, false), 3.25 * lambda, 1e-9));
    for (i, j) in [(0, 9), (3, 3), (5, 2)] {
        assert_eq!(line(i, j, false), f64::INFINITY, "span {i}..{j}");
    }
    let cubic = |i, j| segment_cost_direct(&poly, &tan, i, j, SegKind::Cubic, &c, false);
    assert_eq!(cubic(3, 4), f64::INFINITY);
    assert_eq!(cubic(0, 9), f64::INFINITY);

    let mut apts = vec![Point::new(0.0, 0.0)];
    apts.extend(arc_after(Point::new(0.0, 0.0), 0.0, 6.0, 60.0, 8));
    let apoly = Polyline::with_uniform_sigma(apts.clone(), 0.05, false);
    let atan = tangents(
        (0..apts.len())
            .map(|k| {
                let d = dir(60.0 * k as f64 / 8.0);
                (d, d)
            })
            .collect(),
    );
    let arc = segment_cost_direct(&apoly, &atan, 0, 8, SegKind::Arc, &c, true);
    assert!(close(arc, 5.0 * lambda, 1e-6), "arc {arc}");
}

// --- polish_arms ---------------------------------------------------------------------

struct KnownCubic {
    pts: Vec<Point>,
    sigma: Vec<f64>,
    s: Vec<f64>,
    t0: Vec2,
    t1: Vec2,
    arms: (f64, f64),
}

/// Points sampled exactly on the G1 cubic with arms `arms` along `t0`, `t1`.
fn known_cubic(arms: (f64, f64), t0_deg: f64, t1_deg: f64, samples: usize) -> KnownCubic {
    let (p0, p3) = (Point::new(0.0, 0.0), Point::new(10.0, 0.0));
    let (t0, t1) = (dir(t0_deg), dir(t1_deg));
    let cb = Cubic::from_arms(p0, p3, t0, t1, 10.0, arms.0, arms.1);
    let pts: Vec<Point> = (0..=samples)
        .map(|k| {
            let t = k as f64 / samples as f64;
            let mt = 1.0 - t;
            let w = [mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t];
            Point::new(
                w[0] * cb.p0.x + w[1] * cb.p1.x + w[2] * cb.p2.x + w[3] * cb.p3.x,
                w[0] * cb.p0.y + w[1] * cb.p1.y + w[2] * cb.p2.y + w[3] * cb.p3.y,
            )
        })
        .collect();
    let sigma = vec![0.05; pts.len()];
    let s = arc_lengths(&pts);
    KnownCubic {
        pts,
        sigma,
        s,
        t0,
        t1,
        arms,
    }
}

impl KnownCubic {
    fn chi2(&self, d: (f64, f64)) -> f64 {
        let n = self.pts.len() - 1;
        let cb = Cubic::from_arms(self.pts[0], self.pts[n], self.t0, self.t1, 10.0, d.0, d.1);
        chi2_cubic(&self.pts, &self.sigma, &self.s, 0, n, &cb, false)
    }
    fn polish(&self, start: (f64, f64)) -> ((f64, f64), f64) {
        let n = self.pts.len() - 1;
        polish_arms(
            &self.pts,
            &self.sigma,
            &self.s,
            0,
            n,
            self.t0,
            self.t1,
            start,
        )
    }
}

/// From a start near the truth the Newton polish recovers the arms the points were
/// drawn from, and reports the residual of the arms it returns.
#[test]
fn polish_arms_recovers_the_arms_of_an_exact_cubic() {
    for (arms, a0, a1, start) in [
        ((0.30, 0.45), 40.0, -35.0, (0.36, 0.40)),
        ((0.25, 0.20), 30.0, -60.0, (0.20, 0.28)),
        ((0.50, 0.30), 20.0, 10.0, (0.45, 0.36)),
    ] {
        let k = known_cubic(arms, a0, a1, 24);
        assert!(k.chi2(k.arms) < 1e-12, "the truth fits: {}", k.chi2(k.arms));
        let before = k.chi2(start);
        assert!(before > 1.0, "the start is visibly wrong: {before}");
        let (d, c) = k.polish(start);
        assert!(
            close(d.0, arms.0, 1e-5) && close(d.1, arms.1, 1e-5),
            "{arms:?}: polished to {d:?}"
        );
        assert!(c < 1e-6, "residual {c}");
        assert_eq!(
            c.to_bits(),
            k.chi2(d).to_bits(),
            "reported residual is f(d)"
        );
    }
}

/// Arms already optimal come back unchanged with their residual; arms outside the
/// admissible range are clamped into it before anything else.
#[test]
fn polish_arms_keeps_an_optimum_and_clamps_its_start() {
    let k = known_cubic((0.3, 0.4), 45.0, -45.0, 20);
    let (d, c) = k.polish(k.arms);
    assert!(close(d.0, 0.3, 1e-9) && close(d.1, 0.4, 1e-9), "{d:?}");
    assert!(c < 1e-12);
    // A start far outside the box is clamped and the polish improves on the clamped
    // point: never worse than where it was allowed to start.
    let (d, c) = k.polish((9.0, -3.0));
    assert!(
        (1e-3..=1.5).contains(&d.0) && (1e-3..=1.5).contains(&d.1),
        "{d:?}"
    );
    assert!(c < k.chi2((1.5, 1e-3)), "{c} vs {}", k.chi2((1.5, 1e-3)));
    assert_eq!(c.to_bits(), k.chi2(d).to_bits());
}

// --- refine on hand-built solutions -------------------------------------------------

fn solution(vertices: Vec<usize>, kinds: Vec<SegKind>, arms: Vec<Option<(f64, f64)>>) -> Solution {
    let nseg = kinds.len();
    Solution {
        vertices,
        kinds,
        arms,
        tans: vec![None; nseg],
        arcs: vec![None; nseg],
        cost: 0.0,
    }
}

/// Moment-matched arms for span `i..j`, as the program would hand them to `refine`.
fn g1_arms(poly: &Polyline, tan: &Tangents, i: usize, j: usize) -> Option<(f64, f64)> {
    let s = arc_lengths(&poly.points);
    let raw = raw_moments_direct(&poly.points, i, j);
    let t = (tan.outgoing[i], tan.incoming[j]);
    best_cubic(&poly.points, &poly.sigma, &s, i, j, t.0, t.1, raw, false).map(|b| (b.1, b.2))
}

fn refine_hand(poly: &Polyline, tan: &Tangents, sol: &Solution, lambda: f64) -> FittedPath {
    let pre = Prefix::new(&poly.points, &poly.sigma);
    refine(poly, tan, &pre, sol, &cfg(lambda))
}

/// An L whose corner sample is cut off by a chamfer: the vertical edge is at x = `x`,
/// the last horizontal sample at x = 10. Vertices at 0, 10 and 20.
fn chamfered_l(x: f64) -> Polyline {
    let mut pts = run(Point::new(0.0, 0.0), Point::new(10.0, 0.0), 10);
    pts.extend((1..=10).map(|k| Point::new(x, k as f64)));
    Polyline::with_uniform_sigma(pts, 0.05, false)
}

/// A line–line corner moves to the intersection of the two fitted lines when that is
/// within reach: `3·max(σ, 0.25)` plus the chamfer allowance of a 90° corner,
/// `1/sin 45°`, i.e. 0.75 + 1.414 px here. One pixel moves; three do not.
#[test]
fn refine_moves_a_corner_to_the_line_intersection_only_within_reach() {
    let sol = solution(vec![0, 10, 20], vec![SegKind::Line; 2], vec![None; 2]);
    let tan = tangents(vec![(dir(0.0), dir(0.0)); 21]);

    let near = chamfered_l(11.0);
    let path = refine_hand(&near, &tan, &sol, 1.0);
    assert!(path.start.dist(Point::new(0.0, 0.0)) < 1e-12);
    let corner = end_of(&path.segments[0]);
    assert!(
        corner.dist(Point::new(11.0, 0.0)) < 1e-9,
        "corner {corner:?}"
    );
    assert!(end_of(&path.segments[1]).dist(Point::new(11.0, 10.0)) < 1e-12);
    assert!(!path.closed);

    let far = chamfered_l(13.0);
    let path = refine_hand(&far, &tan, &sol, 1.0);
    let corner = end_of(&path.segments[0]);
    assert!(
        corner.dist(Point::new(10.0, 0.0)) < 1e-12,
        "corner {corner:?}"
    );
}

/// A straight run from (-10, 3) to (0, 3), then a left arc of radius 10 leaving (0, 3)
/// at `arc_dir` degrees and sweeping 80° in 10 samples. Exact tangents, except the
/// outgoing one at the join, which is `start_err` degrees off the arc's.
fn line_then_arc(arc_dir: f64, start_err: f64, sigma: f64) -> (Polyline, Tangents) {
    let join = Point::new(0.0, 3.0);
    let mut pts = run(Point::new(-10.0, 3.0), join, 10);
    pts.extend(arc_after(join, arc_dir, 10.0, 80.0, 10));
    let mut t: Vec<(Vec2, Vec2)> = (0..=10).map(|_| (dir(0.0), dir(0.0))).collect();
    t.extend((1..=10).map(|k| {
        let d = dir(arc_dir + 8.0 * k as f64);
        (d, d)
    }));
    t[10].1 = dir(arc_dir + start_err);
    (Polyline::with_uniform_sigma(pts, sigma, false), tangents(t))
}

/// The start direction of the cubic that follows a line.
fn cubic_start_dir(path: &FittedPath, q: usize) -> Vec2 {
    let from = end_of(&path.segments[q - 1]);
    match path.segments[q] {
        Segment::Cubic(p1, _, _) => unit_of(p1 - from),
        ref s => panic!("segment {q} is not a cubic: {s:?}"),
    }
}

/// A smooth line→cubic join whose estimated tangent is 5° off: the cubic is re-solved
/// with the line's own direction, which fits the tangent arc better and removes the
/// break, so the emitted path is exactly G1 at the join.
#[test]
fn refine_snaps_a_cubic_to_the_line_before_it() {
    let (poly, tan) = line_then_arc(0.0, 5.0, 0.01);
    let arms = g1_arms(&poly, &tan, 10, 20);
    let sol = solution(
        vec![0, 10, 20],
        vec![SegKind::Line, SegKind::Cubic],
        vec![None, arms],
    );
    let path = refine_hand(&poly, &tan, &sol, 1.0);
    let d = cubic_start_dir(&path, 1);
    assert!(
        close(d.x, 1.0, 1e-12) && d.y.abs() < 1e-12,
        "start dir {d:?}"
    );
    // And the snapped cubic is the arc to a few thousandths of a pixel.
    assert!(path_max_deviation(&poly, &path) < 0.01);
}

/// The snap is priced: it is taken when the residual it adds is worth less than the
/// break it removes, and refused when it is worth more. The arc here truly leaves the
/// line at 5°, so forcing G1 moves the curve off the points: cheap at sigma 1 (the
/// break, `λ/4`, pays for it), ruinous at sigma 0.01.
#[test]
fn refine_snaps_a_true_kink_only_when_the_break_pays_for_it() {
    for (sigma, snapped) in [(1.0, true), (0.01, false)] {
        let (poly, tan) = line_then_arc(5.0, 0.0, sigma);
        let arms = g1_arms(&poly, &tan, 10, 20);
        let sol = solution(
            vec![0, 10, 20],
            vec![SegKind::Line, SegKind::Cubic],
            vec![None, arms],
        );
        let path = refine_hand(&poly, &tan, &sol, 1.0);
        let d = cubic_start_dir(&path, 1);
        let want = if snapped { dir(0.0) } else { dir(5.0) };
        assert!(
            close(d.x, want.x, 1e-12) && close(d.y, want.y, 1e-12),
            "sigma {sigma}: start dir {d:?}, want {want:?}"
        );
    }
}

/// A cubic→cubic join where either cubic spans three points or fewer has no room for a
/// symmetric tangent window (half a window of one), so the two meet at the bisector of
/// their own end tangents: the one arriving is 4° off the arc, the one leaving −2°, so
/// both are turned to +1°. Checked with the short cubic on either side of the join,
/// and on both.
#[test]
fn refine_joins_short_cubics_at_the_bisector_of_their_tangents() {
    for (mid, last) in [(7usize, 10usize), (10, 13), (7, 13)] {
        let join = Point::new(0.0, 0.0);
        let mut pts = run(Point::new(-4.0, 0.0), join, 4);
        pts.extend(arc_after(join, 0.0, 8.0, 7.0 * (last - 4) as f64, last - 4));
        let truth = |k: usize| dir(if k <= 4 { 0.0 } else { 7.0 * (k - 4) as f64 });
        let mut t: Vec<(Vec2, Vec2)> = (0..pts.len()).map(|k| (truth(k), truth(k))).collect();
        let at = 7.0 * (mid - 4) as f64;
        t[mid].0 = dir(at + 4.0);
        t[mid].1 = dir(at - 2.0);
        let tan = tangents(t);
        let poly = Polyline::with_uniform_sigma(pts, 1.0, false);
        let arms: Vec<Option<(f64, f64)>> = vec![
            None,
            g1_arms(&poly, &tan, 4, mid),
            g1_arms(&poly, &tan, mid, last),
        ];
        assert!(arms[1].is_some() && arms[2].is_some());
        let sol = solution(
            vec![0, 4, mid, last],
            vec![SegKind::Line, SegKind::Cubic, SegKind::Cubic],
            arms,
        );
        let path = refine_hand(&poly, &tan, &sol, 1.0);
        let want = dir(at + 1.0);
        let (arrive, leave) = match (&path.segments[1], &path.segments[2]) {
            (&Segment::Cubic(_, p2, end), &Segment::Cubic(q1, _, _)) => {
                (unit_of(end - p2), unit_of(q1 - end))
            }
            s => panic!("expected two cubics, got {s:?}"),
        };
        for d in [arrive, leave] {
            assert!(
                close(d.x, want.x, 1e-9) && close(d.y, want.y, 1e-9),
                "vertices 4, {mid}, {last}: join dir {d:?}, want {want:?}"
            );
        }
    }
}

/// A square opened at its bottom-left corner, whose sample is chamfered to (0.5, 0.5).
/// The side arriving at the cut is labelled a cubic, so the cut vertex joins a cubic
/// to a line and is not a corner: it stays on its sample. The line–line corners move to
/// the exact intersections. The emitted path is closed and ends where it starts.
#[test]
fn refine_on_an_opened_loop_moves_only_line_line_corners() {
    let c = [(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)];
    let mut pts = Vec::new();
    for e in 0..4 {
        let (a, b) = (c[e], c[(e + 1) % 4]);
        pts.extend_from_slice(&run(Point::new(a.0, a.1), Point::new(b.0, b.1), 20)[..20]);
    }
    pts[0] = Point::new(0.5, 0.5);
    pts[20] = Point::new(19.5, 0.5);
    pts.push(pts[0]);
    let poly = Polyline::with_uniform_sigma(pts, 0.05, true);
    let side = [dir(0.0), dir(90.0), dir(180.0), dir(270.0)];
    let tan = tangents(
        (0..=80)
            .map(|k| (side[(k / 20) % 4], side[(k / 20) % 4]))
            .collect(),
    );
    let arms = g1_arms(&poly, &tan, 60, 80);
    let sol = solution(
        vec![0, 20, 40, 60, 80],
        vec![SegKind::Line, SegKind::Line, SegKind::Line, SegKind::Cubic],
        vec![None, None, None, arms],
    );
    let path = refine_hand(&poly, &tan, &sol, 1.0);
    assert!(
        path.start.dist(Point::new(0.5, 0.5)) < 1e-12,
        "start {:?}",
        path.start
    );
    let ends: Vec<Point> = path.segments.iter().map(end_of).collect();
    assert!(ends[0].dist(Point::new(20.0, 0.0)) < 1e-9, "{:?}", ends[0]);
    assert!(ends[1].dist(Point::new(20.0, 20.0)) < 1e-9, "{:?}", ends[1]);
    assert!(ends[3].dist(Point::new(0.5, 0.5)) < 1e-12, "{:?}", ends[3]);
}

/// A rounded square opened at the start of its bottom edge: line, quarter circle, line,
/// quarter circle, ..., every join smooth. The estimated start tangents of the first and
/// third corners are 5° off. Each such line→cubic join is snapped to the line's exact
/// direction, and the cubics' ends meet the next line's direction.
#[test]
fn refine_on_an_opened_loop_snaps_every_line_cubic_join() {
    let (side, arc_n) = (10usize, 8usize);
    let mut pts = vec![Point::new(0.0, 0.0)];
    let mut heading = 0.0f64;
    let mut v = vec![0usize];
    let mut t = vec![(dir(0.0), dir(0.0))];
    for _ in 0..4 {
        let from = *pts.last().unwrap();
        let to = Point::new(
            from.x + side as f64 * (heading * DEG).cos(),
            from.y + side as f64 * (heading * DEG).sin(),
        );
        pts.extend_from_slice(&run(from, to, side)[1..]);
        t.extend((0..side).map(|_| (dir(heading), dir(heading))));
        v.push(pts.len() - 1);
        pts.extend(arc_after(to, heading, 4.0, 90.0, arc_n));
        t.extend((1..=arc_n).map(|k| {
            let d = dir(heading + 90.0 * k as f64 / arc_n as f64);
            (d, d)
        }));
        v.push(pts.len() - 1);
        heading += 90.0;
    }
    let n = pts.len();
    assert_eq!(n, 4 * (side + arc_n) + 1);
    assert!(pts[n - 1].dist(pts[0]) < 1e-9);
    pts[n - 1] = pts[0];
    t[v[1]].1 = dir(5.0);
    t[v[5]].1 = dir(185.0);
    let tan = tangents(t);
    let poly = Polyline::with_uniform_sigma(pts, 0.05, true);
    let kinds: Vec<SegKind> = (0..8)
        .map(|q| {
            if q % 2 == 0 {
                SegKind::Line
            } else {
                SegKind::Cubic
            }
        })
        .collect();
    let arms = (0..8)
        .map(|q| {
            (q % 2 == 1)
                .then(|| g1_arms(&poly, &tan, v[q], v[q + 1]))
                .flatten()
        })
        .collect();
    let sol = solution(v, kinds, arms);
    let path = refine_hand(&poly, &tan, &sol, 1.0);
    for (q, want) in [(1, dir(0.0)), (5, dir(180.0))] {
        let d = cubic_start_dir(&path, q);
        assert!(
            close(d.x, want.x, 1e-12) && close(d.y, want.y, 1e-12),
            "cubic {q} start {d:?}"
        );
    }
    for (q, want) in [
        (1, dir(90.0)),
        (3, dir(180.0)),
        (5, dir(270.0)),
        (7, dir(0.0)),
    ] {
        let (p2, end) = match path.segments[q] {
            Segment::Cubic(_, p2, end) => (p2, end),
            ref s => panic!("{s:?}"),
        };
        let d = unit_of(end - p2);
        assert!(
            close(d.x, want.x, 1e-9) && close(d.y, want.y, 1e-9),
            "cubic {q} end {d:?}"
        );
    }
    assert!(path_max_deviation(&poly, &path) < 0.01);
}

/// An arc segment is emitted from its fitted circle; one the program left without a
/// circle falls back to a line to its end vertex.
#[test]
fn refine_emits_arcs_and_falls_back_to_lines_without_one() {
    let mut pts = vec![Point::new(0.0, 0.0)];
    pts.extend(arc_after(Point::new(0.0, 0.0), 0.0, 5.0, 90.0, 6));
    pts.extend(run(Point::new(5.0, 5.0), Point::new(5.0, 9.0), 4)[1..].to_vec());
    let poly = Polyline::with_uniform_sigma(pts, 0.1, false);
    let tan = tangents(vec![(dir(0.0), dir(0.0)); 11]);
    let mut sol = solution(
        vec![0, 6, 10],
        vec![SegKind::Arc, SegKind::Arc],
        vec![None; 2],
    );
    sol.arcs[0] = Some((5.0, 5.0, 0.0, false, true));
    let path = refine_hand(&poly, &tan, &sol, 1.0);
    match path.segments[0] {
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            assert_eq!(
                (rx, ry, phi, large_arc, sweep),
                (5.0, 5.0, 0.0, false, true)
            );
            assert!(end.dist(Point::new(5.0, 5.0)) < 1e-9);
        }
        ref s => panic!("{s:?}"),
    }
    match path.segments[1] {
        Segment::Line(p) => assert!(p.dist(Point::new(5.0, 9.0)) < 1e-12, "{p:?}"),
        ref s => panic!("{s:?}"),
    }
}

// --- path evaluators -------------------------------------------------------------------

/// Exact nearest distances, hand-computed: each point is scored against the closest
/// segment, beyond an end the distance is to the endpoint, and each term is divided by
/// its own sigma².
#[test]
fn path_chi2_cost_and_deviation_have_exact_values() {
    let path = FittedPath {
        start: Point::new(0.0, 0.0),
        segments: vec![
            Segment::Line(Point::new(10.0, 0.0)),
            // A straight cubic along x = 10, from y = 0 to y = 9.
            Segment::Cubic(
                Point::new(10.0, 3.0),
                Point::new(10.0, 6.0),
                Point::new(10.0, 9.0),
            ),
        ],
        closed: false,
    };
    // (point, sigma, squared distance)
    let data = [
        (Point::new(2.0, 1.0), 0.5, 1.0),   // above the line
        (Point::new(5.0, -2.0), 1.0, 4.0),  // below the line
        (Point::new(-3.0, 4.0), 2.0, 25.0), // beyond the start: 3-4-5 to (0, 0)
        (Point::new(8.0, 5.0), 0.25, 4.0),  // nearer the cubic (2) than the line (5)
        (Point::new(10.0, 12.0), 1.0, 9.0), // beyond the cubic's end
    ];
    let poly = Polyline::new(
        data.iter().map(|d| d.0).collect(),
        data.iter().map(|d| d.1).collect(),
        false,
    );
    let want: f64 = data.iter().map(|d| d.2 / (d.1 * d.1)).sum();
    assert!(close(want, 4.0 + 4.0 + 6.25 + 64.0 + 9.0, 1e-12));
    let chi2 = path_chi2(&poly, &path);
    assert!(close(chi2, want, 1e-6), "chi2 {chi2} vs {want}");
    assert!(close(path_max_deviation(&poly, &path), 5.0, 1e-6));
    for lambda in [0.5, 2.0] {
        // A line is 2 parameters, a cubic `params_cubic()` (6 by default).
        let want_cost = 0.5 * want + lambda * (2.0 + params_cubic());
        let got = path_cost(&poly, &path, &cfg(lambda));
        assert!(
            close(got, want_cost, 1e-6),
            "λ {lambda}: {got} vs {want_cost}"
        );
    }

    // An arc is scored as the curve it draws, not as its chord: the centre of a quarter
    // circle is one radius away from it, a point on it is zero.
    let arc = FittedPath {
        start: Point::new(5.0, 0.0),
        segments: vec![Segment::Arc {
            rx: 5.0,
            ry: 5.0,
            phi: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(0.0, 5.0),
        }],
        closed: false,
    };
    let s = std::f64::consts::FRAC_1_SQRT_2 * 5.0;
    let apoly = Polyline::new(
        vec![Point::new(0.0, 0.0), Point::new(s, s)],
        vec![1.0, 1.0],
        false,
    );
    assert!(close(path_chi2(&apoly, &arc), 25.0, 1e-3));
    assert!(close(path_max_deviation(&apoly, &arc), 5.0, 1e-4));

    // Nothing to measure against is infinitely far.
    let empty = FittedPath {
        start: Point::new(0.0, 0.0),
        segments: Vec::new(),
        closed: false,
    };
    assert_eq!(path_chi2(&poly, &empty), f64::INFINITY);
    assert_eq!(path_max_deviation(&poly, &empty), f64::INFINITY);
}
