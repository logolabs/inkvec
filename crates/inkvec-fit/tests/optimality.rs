//! Tests for the optimal-polygon dynamic program.
//!
//! Two claims need holding to account, because the whole M1 thesis rests on them:
//!
//! 1. **It really is optimal.** Verified against exhaustive enumeration on small inputs.
//!    A "globally optimal" DP that quietly returns a local optimum would be worse than a
//!    greedy method, because we would trust it.
//! 2. **It is robust to boundary noise.** `docs/M0-BASELINE.md` §5 measured VTracer at
//!    +18.0% median parameter growth under smooth perturbation and Potrace at 0.0%. The
//!    non-local DP is the reason for the difference, so ours has to reproduce it.

use inkvec_core::{Point, Polyline};
use inkvec_fit::{max_normalized_deviation, optimal_polygon, segment_cost, FitConfig};

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

/// Deterministic smooth (low-frequency) displacement — the same kind of perturbation the
/// benchmark's `boundary` variant applies. Deliberately smooth: a well-behaved tracer
/// should follow a gently displaced boundary with the *same* number of segments.
fn perturb_smooth(pts: &[Point], amplitude: f64, seed: u64) -> Vec<Point> {
    let phase = (seed as f64) * 0.7;
    pts.iter()
        .enumerate()
        .map(|(k, p)| {
            let t = k as f64 * 0.35 + phase;
            Point::new(
                p.x + amplitude * (t.sin() + 0.5 * (2.3 * t).sin()),
                p.y + amplitude * (t.cos() + 0.5 * (1.7 * t).cos()),
            )
        })
        .collect()
}

/// Exhaustive minimum over every admissible segmentation. Exponential; small inputs only.
fn brute_force_min(poly: &Polyline, cfg: &FitConfig) -> f64 {
    let n = poly.len();
    assert!(n <= 14, "exponential; keep inputs small");
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

        let total: f64 = verts
            .windows(2)
            .map(|p| segment_cost(poly, p[0], p[1], cfg))
            .sum();
        if total < best {
            best = total;
        }
    }
    best
}

// --- optimality ---------------------------------------------------------------------

#[test]
fn matches_exhaustive_search_on_small_inputs() {
    let cfg = FitConfig {
        tau: 2.0,
        lambda: 1.0,
    };
    let cases: Vec<Vec<Point>> = vec![
        line(10, 0.0, 0.0, 20.0, 6.0),
        circle(12, 0.0, 0.0, 10.0),
        // An L with a genuine corner.
        [line(6, 0.0, 0.0, 10.0, 0.0), line(6, 10.0, 0.0, 10.0, 10.0)].concat(),
        // A shallow staircase: the case Potrace's non-local step exists to straighten.
        (0..12)
            .map(|k| Point::new(k as f64, ((k as f64) / 3.0).floor()))
            .collect(),
    ];

    for (idx, pts) in cases.into_iter().enumerate() {
        let poly = Polyline::with_uniform_sigma(pts, 0.5, false);
        let got = optimal_polygon(&poly, &cfg);
        let want = brute_force_min(&poly, &cfg);
        assert!(
            (got.cost - want).abs() < 1e-9,
            "case {idx}: DP cost {} != exhaustive minimum {}",
            got.cost,
            want
        );
    }
}

#[test]
fn perfect_straight_line_collapses_to_one_segment() {
    let poly = Polyline::with_uniform_sigma(line(50, 0.0, 0.0, 100.0, 37.0), 0.5, false);
    let seg = optimal_polygon(&poly, &FitConfig::default());
    assert_eq!(
        seg.segment_count(),
        1,
        "a straight line needs exactly one segment"
    );
    assert_eq!(seg.vertices, vec![0, 49]);
}

#[test]
fn corner_is_found_exactly() {
    let pts = [
        line(20, 0.0, 0.0, 10.0, 0.0),
        line(21, 10.0, 0.0, 10.0, 10.0),
    ]
    .concat();
    let poly = Polyline::with_uniform_sigma(pts, 0.2, false);
    let seg = optimal_polygon(
        &poly,
        &FitConfig {
            tau: 2.0,
            lambda: 1.0,
        },
    );
    assert_eq!(seg.segment_count(), 2, "an L is two segments");
    assert_eq!(seg.vertices[1], 19, "the corner sits at the join");
}

/// The staircase case from the Potrace paper: a shallow straight line rendered to a
/// grid becomes a stair, and the tracer must return the line, not the stair.
#[test]
fn shallow_staircase_is_straightened() {
    let pts: Vec<Point> = (0..60)
        .map(|k| Point::new(k as f64, ((k as f64) * 0.2).round()))
        .collect();
    let poly = Polyline::with_uniform_sigma(pts, 0.5, false);
    let seg = optimal_polygon(
        &poly,
        &FitConfig {
            tau: 2.0,
            lambda: 1.0,
        },
    );
    assert!(
        seg.segment_count() <= 2,
        "a quantized shallow line should straighten, got {} segments",
        seg.segment_count()
    );
}

#[test]
fn deviation_stays_within_the_tolerance_it_promises() {
    let cfg = FitConfig {
        tau: 2.0,
        lambda: 1.0,
    };
    let poly = Polyline::with_uniform_sigma(circle(200, 0.0, 0.0, 50.0), 0.4, false);
    let seg = optimal_polygon(&poly, &cfg);
    let dev = max_normalized_deviation(&poly, &seg);
    assert!(
        dev <= cfg.tau + 1e-9,
        "max deviation {dev} exceeded tau {}",
        cfg.tau
    );
}

// --- the sigma field actually drives simplification ---------------------------------

#[test]
fn larger_uncertainty_yields_fewer_segments() {
    let pts = circle(400, 0.0, 0.0, 80.0);
    let cfg = FitConfig {
        tau: 2.0,
        lambda: 1.0,
    };
    let tight = optimal_polygon(
        &Polyline::with_uniform_sigma(pts.clone(), 0.05, false),
        &cfg,
    );
    let loose = optimal_polygon(&Polyline::with_uniform_sigma(pts, 0.8, false), &cfg);
    assert!(
        loose.segment_count() < tight.segment_count(),
        "a less certain boundary should be simplified harder: {} vs {}",
        loose.segment_count(),
        tight.segment_count()
    );
}

/// Adaptive simplification, the property Vectorizer.AI advertises, falling out of the
/// measurement model rather than a separate heuristic: the well-localized half of a
/// boundary keeps its detail while the faint half is simplified away.
#[test]
fn per_point_sigma_localizes_simplification() {
    let pts = circle(240, 0.0, 0.0, 60.0);
    let n = pts.len();
    let sigma: Vec<f64> = (0..n).map(|k| if k < n / 2 { 0.05 } else { 1.5 }).collect();
    let poly = Polyline::new(pts, sigma, false);
    let seg = optimal_polygon(
        &poly,
        &FitConfig {
            tau: 2.0,
            lambda: 1.0,
        },
    );

    let confident = seg.vertices.iter().filter(|&&i| i < n / 2).count();
    let faint = seg.vertices.iter().filter(|&&i| i >= n / 2).count();
    assert!(
        confident > 2 * faint,
        "detail should concentrate where the boundary is well localized: {confident} vs {faint}"
    );
}

// --- regression: search pruning must never exclude an optimal segment --------------

/// A polygon with long straight edges and realistic sub-pixel extraction noise must
/// return exactly one segment per edge.
///
/// This is a regression test for a real bug. Pruning was originally an angular cone on
/// the admissible chord direction, and on a ~50px edge it excluded the whole-edge
/// segment before the cost function ever saw it. The dynamic program then correctly
/// returned the best of what remained — splitting straight edges at *collinear* points —
/// and a hexagon came back as 12 segments instead of 6. Nothing looked broken; the
/// output was merely twice the size it should have been.
///
/// The lesson generalizes past this one constant: a search bound that is not derived
/// from the objective can silently change the answer, and an "optimal" algorithm will
/// keep reporting confident, optimal-looking results while it does.
#[test]
fn long_straight_edges_are_not_split_by_search_pruning() {
    let cfg = FitConfig::default();
    for sides in [4usize, 5, 6, 8] {
        let mut pts = Vec::new();
        let r = 50.0;
        for k in 0..sides {
            let a0 = std::f64::consts::TAU * k as f64 / sides as f64;
            let a1 = std::f64::consts::TAU * (k + 1) as f64 / sides as f64;
            let (p0, p1) = (
                Point::new(r * a0.cos(), r * a0.sin()),
                Point::new(r * a1.cos(), r * a1.sin()),
            );
            // Sample the edge densely, with the ~0.05px wobble marching squares produces.
            let n = 48;
            for i in 0..n {
                let t = i as f64 / n as f64;
                let jitter = 0.05 * ((i as f64) * 2.399).sin();
                let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
                let len = dx.hypot(dy).max(1e-9);
                pts.push(Point::new(
                    p0.x + dx * t - dy / len * jitter,
                    p0.y + dy * t + dx / len * jitter,
                ));
            }
        }
        // Corners are genuinely less localized than straight runs, as in the real front end.
        let n = pts.len();
        let per_edge = n / sides;
        let sigma: Vec<f64> = (0..n)
            .map(|k| {
                if k % per_edge < 2 || k % per_edge > per_edge - 3 {
                    0.35
                } else {
                    0.05
                }
            })
            .collect();

        let poly = Polyline::new(pts, sigma, false);
        let seg = optimal_polygon(&poly, &cfg);
        assert!(
            seg.segment_count() <= sides + 1,
            "{sides}-gon returned {} segments; straight edges are being split",
            seg.segment_count()
        );
    }
}

// --- robustness: the M0 claim -------------------------------------------------------

#[test]
fn segment_count_is_stable_under_smooth_boundary_noise() {
    let cfg = FitConfig {
        tau: 2.0,
        lambda: 1.0,
    };
    let sigma = 0.5;
    let base_pts = circle(300, 0.0, 0.0, 90.0);
    let base = Polyline::with_uniform_sigma(base_pts.clone(), sigma, false);
    let clean = optimal_polygon(&base, &cfg).segment_count() as f64;

    let mut growths = Vec::new();
    for seed in 0..8u64 {
        let noisy = Polyline::with_uniform_sigma(
            perturb_smooth(&base_pts, 0.25 * sigma, seed),
            sigma,
            false,
        );
        let n = optimal_polygon(&noisy, &cfg).segment_count() as f64;
        growths.push((n / clean - 1.0) * 100.0);
    }
    growths.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = growths[growths.len() / 2];

    // M0-BASELINE.md §5: VTracer +18.0% median, Potrace 0.0%. DESIGN.md §8 targets <5%.
    assert!(
        median < 5.0,
        "median segment-count growth {median:.1}% exceeds the <5% target \
         (VTracer measured at +18.0%)"
    );
}

// --- degenerate input ---------------------------------------------------------------

#[test]
fn degenerate_inputs_do_not_panic() {
    let cfg = FitConfig::default();
    for pts in [
        vec![],
        vec![Point::new(1.0, 1.0)],
        vec![Point::new(1.0, 1.0), Point::new(2.0, 2.0)],
        // All points coincident.
        vec![Point::new(3.0, 3.0); 8],
    ] {
        let poly = Polyline::with_uniform_sigma(pts, 0.5, false);
        let seg = optimal_polygon(&poly, &cfg);
        assert!(seg.cost.is_finite() || poly.len() < 2 || poly.len() == 8);
    }
}

#[test]
fn closed_loop_round_trips_to_valid_indices() {
    let poly = Polyline::with_uniform_sigma(circle(120, 5.0, 5.0, 40.0), 0.3, true);
    let seg = optimal_polygon(&poly, &FitConfig::default());
    assert!(
        seg.segment_count() >= 3,
        "a closed loop needs at least 3 segments"
    );
    assert!(
        seg.vertices.iter().all(|&i| i < poly.len()),
        "indices must stay in range"
    );
}
