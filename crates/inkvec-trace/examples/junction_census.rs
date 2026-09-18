//! How many junctions are tangential, where placing them by intersection cannot work?
//!
//! A junction is placed by intersecting the boundaries that meet there. That is well posed
//! when they cross at an angle and unusable when they are tangent, and the error is large:
//! on a traced rounded square the junction landed 3.2px past the true tangent point, which
//! is the kink. `taper::fit` can recover those, but recovering them means re-cutting the
//! contour between edges — real work in the planar map. So count them first.
//!
//! What matters is not that *some* pair continues each other — that is just a T junction,
//! the ordinary case in a partition, and its stem crosses the through boundary at a healthy
//! angle so the intersection is well posed. The ill-posed case is when the **third**
//! boundary is *also* nearly parallel to the through pair: then a region tapers to nothing
//! and there is no well-conditioned intersection to be had.
//!
//! So each junction is classified by how far the third boundary is from the through
//! direction. Near 0 or 180 degrees it is a taper; near 90 it is an ordinary T.
//!
//! What this measures is a *lower bound on suspicion*, not a count of tangencies, and the
//! difference matters. The angle is measured at the node as currently placed, so where the
//! node is wrong the angle is wrong too, in the direction that makes a tangency look like
//! a crossing. It cannot be otherwise: the quantity being used to detect the error is
//! itself corrupted by that error.
//!
//! So take the 24.9% below as a reason to look, not as a result. What settled the question
//! was fitting the tangency and counting the fits that hold up against the measurement's
//! own noise — 568 junctions over these 80 images, which `planar::taper_junction` records.
//!
//! Run: `cargo run --release -p inkvec-trace --example junction_census -- [n]`

use std::collections::HashMap;

use inkvec_core::Point;
use inkvec_trace::{load_image, trace_color_full, ColorOptions};

/// Direction a boundary leaves a node, from a few points in so the estimate is not one
/// pixel of staircase.
fn leaving(points: &[Point], from_start: bool) -> Option<(f64, f64)> {
    let n = points.len();
    if n < 2 {
        return None;
    }
    let step = 3.min(n - 1);
    let (a, b) = if from_start {
        (points[0], points[step])
    } else {
        (points[n - 1], points[n - 1 - step])
    };
    let v = (b.x - a.x, b.y - a.y);
    let l = (v.0 * v.0 + v.1 * v.1).sqrt();
    (l > 1e-9).then(|| (v.0 / l, v.1 / l))
}

fn angle_between(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 * b.1 - a.1 * b.0)
        .atan2(a.0 * b.0 + a.1 * b.1)
        .to_degrees()
        .abs()
}

fn main() {
    let n_images: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);

    let mut buckets = [0usize; 6]; // by how close the straightest pair is to 180
    let mut total = 0usize;
    let mut degree: HashMap<usize, usize> = HashMap::new();
    let mut images = 0usize;

    for corpus in ["noto-emoji", "twemoji"] {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../bench/data/corpus_raster")
            .join(corpus)
            .join("128");
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<_> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "png"))
            .collect();
        files.sort();
        files.truncate(n_images);

        for f in files {
            let Ok(img) = load_image(&f) else { continue };
            let traced = trace_color_full(
                &img,
                &ColorOptions {
                    min_region: 2,
                    ..ColorOptions::default()
                },
            );
            images += 1;

            let mut at: HashMap<u32, Vec<(f64, f64)>> = HashMap::new();
            for e in &traced.map.edges {
                if e.closed {
                    continue;
                }
                if let Some(d) = leaving(&e.points, true) {
                    at.entry(e.start_node).or_default().push(d);
                }
                if let Some(d) = leaving(&e.points, false) {
                    at.entry(e.end_node).or_default().push(d);
                }
            }

            for (_, dirs) in at {
                if dirs.len() < 2 {
                    continue;
                }
                *degree.entry(dirs.len()).or_default() += 1;
                total += 1;
                // The through pair: the two that most nearly continue each other.
                let (mut best, mut pair) = (0.0f64, (0usize, 1usize));
                for i in 0..dirs.len() {
                    for j in i + 1..dirs.len() {
                        let a = angle_between(dirs[i], dirs[j]);
                        if a > best {
                            best = a;
                            pair = (i, j);
                        }
                    }
                }
                // How far the remaining boundaries sit from that through direction. A
                // stem at 90 degrees is an ordinary T; one lying along it is a taper.
                let through = dirs[pair.0];
                let mut off = 90.0f64;
                for (k, d) in dirs.iter().enumerate() {
                    if k == pair.0 || k == pair.1 {
                        continue;
                    }
                    let a = angle_between(through, *d);
                    // Distance from parallel, in either sense.
                    off = off.min(a.min(180.0 - a));
                }
                let b = match off {
                    o if o < 10.0 => 0,
                    o if o < 20.0 => 1,
                    o if o < 30.0 => 2,
                    o if o < 45.0 => 3,
                    o if o < 70.0 => 4,
                    _ => 5,
                };
                buckets[b] += 1;
            }
        }
    }

    println!("\n===== {total} junctions over {images} images =====");
    println!("  how far the branching boundary lies from the through direction:");
    let labels = [
        "  under 10 deg from the through line  (a taper)",
        "  10 to 20 deg                        (a taper in practice)",
        "  20 to 30 deg",
        "  30 to 45 deg",
        "  45 to 70 deg",
        "  over 70 deg                         (an ordinary T junction)",
    ];
    for (i, l) in labels.iter().enumerate() {
        let pct = 100.0 * buckets[i] as f64 / total.max(1) as f64;
        println!("  {l:<52}{:>7}{:>8.1}%", buckets[i], pct);
    }
    let tang = buckets[0] + buckets[1];
    println!(
        "\n  tangential or nearly so: {tang} of {total}  ({:.1}%)",
        100.0 * tang as f64 / total.max(1) as f64
    );
    let mut d: Vec<_> = degree.into_iter().collect();
    d.sort();
    println!("  junction degree: {d:?}");
}
