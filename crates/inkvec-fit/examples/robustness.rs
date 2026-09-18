//! Does the non-local dynamic program cause the noise robustness, or is the input
//! just easy?
//!
//! `docs/M0-BASELINE.md` §5 measured Potrace at 0.0% median parameter growth under
//! smooth boundary perturbation and VTracer at +18.0%, and attributed the difference to
//! the optimal-polygon step VTracer dropped. That is a causal claim inferred from two
//! programs that differ in many ways at once, so it is worth testing directly.
//!
//! This runs the *same* admissibility rule, the *same* cost function and the *same*
//! inputs through two segmenters that differ in exactly one respect:
//!
//!   greedy  — extend the current segment while the chord stays admissible, then commit
//!             and start again. One local decision at a time, never revisited.
//!   dp      — the global optimum over all admissible segmentations.
//!
//! Any difference in noise stability is then attributable to non-locality alone.
//!
//! Run with:  cargo run --release --example robustness -p inkvec-fit

use inkvec_core::{Point, Polyline};
use inkvec_fit::{is_admissible, optimal_polygon, FitConfig};

fn circle(n: usize, r: f64) -> Vec<Point> {
    (0..n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            Point::new(r * a.cos(), r * a.sin())
        })
        .collect()
}

fn rounded_rect(n: usize, w: f64, h: f64, r: f64) -> Vec<Point> {
    // Sampled by perimeter so corners and fillets both get points.
    let mut pts = Vec::with_capacity(n);
    for k in 0..n {
        let t = k as f64 / n as f64 * std::f64::consts::TAU;
        let (c, s) = (t.cos(), t.sin());
        let m = c.abs().max(s.abs()).max(1e-9);
        let sq = Point::new(w * 0.5 * c / m, h * 0.5 * s / m);
        let cr = Point::new(r * c, r * s);
        let blend = 0.72;
        pts.push(Point::new(
            sq.x * blend + cr.x * (1.0 - blend),
            sq.y * blend + cr.y * (1.0 - blend),
        ));
    }
    pts
}

fn star(n: usize, points: usize, r0: f64, r1: f64) -> Vec<Point> {
    (0..n)
        .map(|k| {
            let t = k as f64 / n as f64 * std::f64::consts::TAU;
            let lobe = (t * points as f64).cos();
            let r = r0 + (r1 - r0) * 0.5 * (1.0 + lobe);
            Point::new(r * t.cos(), r * t.sin())
        })
        .collect()
}

/// Local segmenter: commit to the longest admissible segment at every step.
fn greedy(poly: &Polyline, cfg: &FitConfig) -> usize {
    let n = poly.len();
    if n < 2 {
        return 0;
    }
    let mut segments = 0;
    let mut i = 0;
    while i < n - 1 {
        let mut j = i + 1;
        while j + 1 < n && is_admissible(poly, i, j + 1, cfg) {
            j += 1;
        }
        segments += 1;
        i = j;
    }
    segments
}

fn perturb(pts: &[Point], amplitude: f64, seed: u64) -> Vec<Point> {
    let phase = seed as f64 * 0.7 + 0.13;
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

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        return f64::NAN;
    }
    v[v.len() / 2]
}

fn main() {
    // Calibrated, not guessed. An earlier version of this example ran at lambda = 1.0
    // and produced a table that looked like the DP losing to greedy on segment count;
    // see examples/lambda_sweep.rs for why that comparison was meaningless.
    let cfg = FitConfig::default();
    let sigma = 0.5;
    const TRIALS: u64 = 16;

    let shapes: Vec<(&str, Vec<Point>)> = vec![
        ("circle", circle(300, 90.0)),
        ("rounded_rect", rounded_rect(300, 180.0, 130.0, 70.0)),
        ("star5", star(300, 5, 45.0, 95.0)),
    ];

    println!("\nSegment-count growth under smooth boundary perturbation (amplitude 0.25 sigma)");
    println!("identical admissibility rule and cost; the only difference is locality\n");
    println!(
        "{:<14} {:>10} {:>10} {:>14} {:>14}",
        "shape", "greedy n", "dp n", "greedy growth", "dp growth"
    );
    println!("{}", "-".repeat(66));

    let mut all_greedy = Vec::new();
    let mut all_dp = Vec::new();

    for (name, pts) in &shapes {
        let base = Polyline::with_uniform_sigma(pts.clone(), sigma, false);
        let g0 = greedy(&base, &cfg) as f64;
        let d0 = optimal_polygon(&base, &cfg).segment_count() as f64;

        let mut gg = Vec::new();
        let mut dg = Vec::new();
        for seed in 0..TRIALS {
            let noisy =
                Polyline::with_uniform_sigma(perturb(pts, 0.25 * sigma, seed), sigma, false);
            gg.push((greedy(&noisy, &cfg) as f64 / g0 - 1.0) * 100.0);
            dg.push((optimal_polygon(&noisy, &cfg).segment_count() as f64 / d0 - 1.0) * 100.0);
        }
        let (gm, dm) = (median(gg.clone()), median(dg.clone()));
        all_greedy.extend(gg);
        all_dp.extend(dg);

        println!(
            "{:<14} {:>10.0} {:>10.0} {:>13.1}% {:>13.1}%",
            name, g0, d0, gm, dm
        );
    }

    println!("{}", "-".repeat(66));
    println!(
        "{:<14} {:>10} {:>10} {:>13.1}% {:>13.1}%",
        "MEDIAN",
        "",
        "",
        median(all_greedy),
        median(all_dp)
    );

    println!("\nreference points from docs/M0-BASELINE.md §5 and the literature:");
    println!("  potrace    (has the DP)        0.0%   measured");
    println!("  vtracer    (dropped the DP)  +18.0%   measured");
    println!("  AnchorFlow (learned anchors)  +2.9%   published");
    println!("  AdaVec                       +20.7%   published");
    println!("  VTracer                     +106.7%   published\n");
}
