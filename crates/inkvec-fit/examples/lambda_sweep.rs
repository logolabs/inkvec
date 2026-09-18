//! Calibrating lambda, and comparing greedy against the DP *fairly*.
//!
//! A first attempt at this comparison was misleading, and the way it misled is worth
//! recording. Run at `lambda = 1.0`, the greedy segmenter emitted 22 segments for a
//! circle and the "optimal" DP emitted 43. That looks like the DP losing. It is not:
//! the two are minimizing different things. Greedy minimizes segment *count* subject to
//! admissibility; the DP minimizes `0.5*chi^2 + lambda*params`. At `lambda = 1` a
//! segment costs 2.0 nats, so the DP will happily buy a segment whenever it removes more
//! than 2.0 nats of residual. It was not producing worse output — it was answering a
//! different question, and the question had a badly chosen exchange rate.
//!
//! That is precisely the failure mode flagged in DESIGN.md §9.6: a mis-set lambda looks
//! like a pipeline defect while actually being a units problem.
//!
//! **A principled starting value.** MDL measures description length in nats. A
//! coordinate confined to a range `R` and stored to precision `delta` costs
//! `ln(R/delta)` nats. For a 256px canvas at 0.1px precision that is `ln(2560) ~ 7.8`.
//! So `lambda ~ 8`, not 1 — and the right way to set it is from the coordinate range and
//! output precision actually in use, not by taste.
//!
//! The fair comparison is then at *matched segment count*: force both segmenters to the
//! same budget and ask which places its vertices better. That is what optimality should
//! buy, and it is what this measures.
//!
//! Run with:  cargo run --release --example lambda_sweep -p inkvec-fit

use inkvec_core::{Point, Polyline};
use inkvec_fit::{is_admissible, max_normalized_deviation, optimal_polygon, FitConfig};

fn circle(n: usize, r: f64) -> Vec<Point> {
    (0..n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            Point::new(r * a.cos(), r * a.sin())
        })
        .collect()
}

fn star(n: usize, points: usize, r0: f64, r1: f64) -> Vec<Point> {
    (0..n)
        .map(|k| {
            let t = k as f64 / n as f64 * std::f64::consts::TAU;
            let r = r0 + (r1 - r0) * 0.5 * (1.0 + (t * points as f64).cos());
            Point::new(r * t.cos(), r * t.sin())
        })
        .collect()
}

/// Local segmenter: always commit to the longest admissible segment.
fn greedy_vertices(poly: &Polyline, cfg: &FitConfig) -> Vec<usize> {
    let n = poly.len();
    if n < 2 {
        return (0..n).collect();
    }
    let mut v = vec![0usize];
    let mut i = 0;
    while i < n - 1 {
        let mut j = i + 1;
        while j + 1 < n && is_admissible(poly, i, j + 1, cfg) {
            j += 1;
        }
        v.push(j);
        i = j;
    }
    v
}

/// Total squared deviation of the source points from a chosen vertex sequence,
/// normalized by each point's own sigma. This is the fidelity term, comparable across
/// segmenters because it does not include the description-length charge.
fn chi2_of(poly: &Polyline, verts: &[usize]) -> f64 {
    let mut total = 0.0;
    for pair in verts.windows(2) {
        let (i, j) = (pair[0], pair[1]);
        let (a, b) = (poly.points[i], poly.points[j]);
        for k in i + 1..j {
            let d = inkvec_fit::line_distance(poly.points[k], a, b);
            let z = d / poly.sigma[k];
            total += z * z;
        }
    }
    total
}

/// Binary-search lambda until the DP emits `target` segments (or as close as it gets).
fn dp_at_target_count(poly: &Polyline, tau: f64, target: usize) -> (Vec<usize>, f64) {
    let (mut lo, mut hi): (f64, f64) = (1e-3, 1e6);
    let mut best = (Vec::new(), f64::NAN);
    for _ in 0..60 {
        let mid = (lo * hi).sqrt();
        let cfg = FitConfig { tau, lambda: mid };
        let seg = optimal_polygon(poly, &cfg);
        let n = seg.segment_count();
        best = (seg.vertices.clone(), mid);
        if n == target {
            return best;
        }
        // More segments than wanted -> segments are too cheap -> raise lambda.
        if n > target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    best
}

fn main() {
    let tau = 2.0;
    let sigma = 0.5;
    let shapes: Vec<(&str, Vec<Point>)> = vec![
        ("circle", circle(300, 90.0)),
        ("star5", star(300, 5, 45.0, 95.0)),
    ];

    // --- 1. how lambda moves the operating point ---------------------------------
    println!("\n1. Segment count against lambda   (tau = {tau}, sigma = {sigma})\n");
    print!("{:<10}", "shape");
    let lambdas = [0.5, 1.0, 2.0, 4.0, 7.8, 16.0, 32.0, 64.0];
    for l in lambdas {
        print!("{:>8.1}", l);
    }
    println!("\n{}", "-".repeat(10 + 8 * lambdas.len()));
    for (name, pts) in &shapes {
        let poly = Polyline::with_uniform_sigma(pts.clone(), sigma, false);
        print!("{:<10}", name);
        for l in lambdas {
            let seg = optimal_polygon(&poly, &FitConfig { tau, lambda: l });
            print!("{:>8}", seg.segment_count());
        }
        println!();
    }
    println!("\n   lambda ~ 7.8 = ln(256px / 0.1px): the MDL cost of one coordinate");
    println!("   stored at 0.1px precision on a 256px canvas.");

    // --- 2. the fair comparison: matched segment count ----------------------------
    println!("\n\n2. Greedy vs DP at MATCHED segment count");
    println!("   both segmenters given the same budget; lower chi^2 and lower max");
    println!("   deviation are better. This is what optimality is supposed to buy.\n");
    println!(
        "{:<10} {:>8} {:>12} {:>12} {:>9} {:>10} {:>10}",
        "shape", "segments", "greedy chi2", "dp chi2", "chi2 red.", "greedy dev", "dp dev"
    );
    println!("{}", "-".repeat(76));

    for (name, pts) in &shapes {
        let poly = Polyline::with_uniform_sigma(pts.clone(), sigma, false);
        let cfg = FitConfig { tau, lambda: 1.0 };
        let gv = greedy_vertices(&poly, &cfg);
        let target = gv.len() - 1;

        let (dv, _) = dp_at_target_count(&poly, tau, target);
        let (gc, dc) = (chi2_of(&poly, &gv), chi2_of(&poly, &dv));

        let g_seg = inkvec_fit::Segmentation {
            vertices: gv.clone(),
            cost: 0.0,
        };
        let d_seg = inkvec_fit::Segmentation {
            vertices: dv.clone(),
            cost: 0.0,
        };
        let gd = max_normalized_deviation(&poly, &g_seg);
        let dd = max_normalized_deviation(&poly, &d_seg);

        println!(
            "{:<10} {:>8} {:>12.1} {:>12.1} {:>8.1}% {:>10.2} {:>10.2}",
            name,
            format!("{}/{}", target, d_seg.segment_count()),
            gc,
            dc,
            (1.0 - dc / gc.max(1e-12)) * 100.0,
            gd,
            dd
        );
    }
    println!("\n   chi2 red. = how much residual the DP removes at the same segment count.");
}
