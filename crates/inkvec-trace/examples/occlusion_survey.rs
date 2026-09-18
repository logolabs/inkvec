//! Does T-junction analysis, uncorroborated, actually say anything trustworthy on real
//! flat art — or only on the synthetic case built to demonstrate its limit?
//!
//! `occlusion::tests::a_three_way_partition_reads_as_a_single_uncorroborated_junction`
//! proves a single T-junction can be wrong on ordinary flat art with no occlusion in it
//! at all: three wedges meeting at 120 degrees produce one spurious reading, every time,
//! regardless of resolution. `occlusion::aggregate` exists to be the corroboration that
//! catches that case (support 1, never trusted). What this measures is whether real
//! corpus icons look like the synthetic negative (many single-junction pairs, mostly
//! untrustworthy) or like the synthetic positive (occlusion events with agreeing pairs
//! of junctions), and whether raising the corroboration bar above "more than one" buys
//! anything.
//!
//! Run: `cargo run --release -p inkvec-trace --example occlusion_survey -- [n]`

use std::collections::HashMap;

use inkvec_trace::{load_image, occlusion, trace_color_full, ColorOptions};

fn main() {
    let n_images: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    let mut images = 0usize;
    let mut total_junctions = 0usize;
    let mut total_pairs = 0usize;
    let mut agree_1 = 0usize; // pairs backed by exactly one junction (never trustworthy)
    let mut agree_2plus = 0usize; // pairs backed by >=2 junctions, all agreeing
    let mut split = 0usize; // pairs where junctions disagree on direction
    let mut per_image_junctions: Vec<usize> = Vec::new();
    let mut bend_hist = [0usize; 6]; // 0-5,5-10,10-15,15-20,20-25,25-30 deg

    let corpora = [
        "lucide",
        "material-icons",
        "simple-icons",
        "openmoji",
        "twemoji",
        "fluent-emoji",
        "noto-emoji",
    ];

    for corpus in corpora {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../bench/data/corpus_raster")
            .join(corpus)
            .join("128ss");
        let Ok(rd) = std::fs::read_dir(&dir) else {
            println!("  (no rasters for {corpus}, skipping)");
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
            let traced = trace_color_full(&img, &ColorOptions::default());
            images += 1;

            let js = occlusion::find(&traced.map);
            total_junctions += js.len();
            per_image_junctions.push(js.len());
            for j in &js {
                let b = ((j.bend_deg / 5.0).floor() as usize).min(5);
                bend_hist[b] += 1;
            }

            for v in occlusion::aggregate(&js) {
                total_pairs += 1;
                let support = v.for_a + v.for_b;
                if v.occluder.is_none() {
                    split += 1;
                } else if support == 1 {
                    agree_1 += 1;
                } else {
                    agree_2plus += 1;
                }
            }
        }
    }

    println!("\n===== {total_junctions} T-junction claims, {total_pairs} face pairs, {images} images =====\n");
    println!("  per face pair, how the evidence resolves:");
    for (n, label) in [
        (
            split,
            "disagreeing  (junctions for this pair point both ways -- never trust)",
        ),
        (
            agree_1,
            "single junction (support 1 -- the uncorroborated case; do not trust alone)",
        ),
        (agree_2plus, "corroborated (2+ junctions, all agreeing)"),
    ] {
        let pct = 100.0 * n as f64 / total_pairs.max(1) as f64;
        println!("    {n:6}  ({pct:5.1}%)  {label}");
    }

    println!("\n  bend of the winning face's two edges, degrees off dead straight:");
    for (i, n) in bend_hist.iter().enumerate() {
        let pct = 100.0 * *n as f64 / total_junctions.max(1) as f64;
        println!("    {:>2}-{:<2} deg  {n:6}  ({pct:5.1}%)", i * 5, i * 5 + 5);
    }

    per_image_junctions.sort_unstable();
    let median = per_image_junctions
        .get(per_image_junctions.len() / 2)
        .copied()
        .unwrap_or(0);
    let zero = per_image_junctions.iter().filter(|&&n| n == 0).count();
    println!(
        "\n  junctions per image: median {median}, {zero} of {images} images have none ({:.1}%)",
        100.0 * zero as f64 / images.max(1) as f64
    );

    let by_corpus: HashMap<&str, ()> = HashMap::new();
    let _ = by_corpus;

    println!(
        "\n  headline: of {total_pairs} candidate pairs, {:.1}% are corroborated (2+ agreeing \
         junctions) and would survive a caller that refuses single-junction evidence.",
        100.0 * agree_2plus as f64 / total_pairs.max(1) as f64
    );
}
