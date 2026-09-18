//! Dump corroborated T-junction verdicts as `image,occluder_hex,occluded_hex,support`
//! CSV, so a Python script can cross-check them against SVG document order without
//! needing a Rust-side face-to-path correspondence pipeline.
//!
//! This is a checkpoint, not a training input: it exists to answer one question before
//! committing to a labelling pipeline for a learned model -- does document order in the
//! corpus actually predict the *exact*, already-corroborated T-junction evidence? If it
//! does not agree at a high rate, document order is not a trustworthy label source and
//! nothing built on it would be either.
//!
//! Run: `cargo run --release -p inkvec-trace --example zrank_dump -- [n] > out.csv`

use inkvec_trace::{load_image, occlusion, trace_color_full, ColorOptions};

fn to_hex(rgb: [f32; 3]) -> String {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("{:02x}{:02x}{:02x}", c(rgb[0]), c(rgb[1]), c(rgb[2]))
}

fn main() {
    let n_images: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);

    // One row per raw junction (not the aggregated pair), with its pixel position, so a
    // consumer can restrict colour matching to elements actually near that point --
    // colours like black or white recur across many unrelated elements in real icons,
    // and matching by colour alone across the whole document confuses them freely.
    println!("image,x,y,occluder_hex,occluded_hex,support");

    let corpora = [
        "lucide",
        "material-icons",
        "simple-icons",
        "openmoji",
        "twemoji",
        "noto-emoji",
    ];
    for corpus in corpora {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../bench/data/corpus_raster")
            .join(corpus)
            .join("128ss");
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
            let traced = trace_color_full(&img, &ColorOptions::default());
            let js = occlusion::find(&traced.map);
            let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("?");

            // Support per unordered pair, so each row carries the corroboration count its
            // pair achieved overall, not just "this one junction".
            let mut support: std::collections::HashMap<(u16, u16), usize> =
                std::collections::HashMap::new();
            for v in occlusion::aggregate(&js) {
                if v.occluder.is_some() {
                    support.insert((v.a.min(v.b), v.a.max(v.b)), v.for_a + v.for_b);
                }
            }
            for j in &js {
                let key = (j.occluder.min(j.occluded), j.occluder.max(j.occluded));
                let Some(&n) = support.get(&key) else {
                    continue;
                }; // skip disagreeing pairs
                let Some(&occ_rgb) = traced.palette.rgb.get(j.occluder as usize) else {
                    continue;
                };
                let Some(&other_rgb) = traced.palette.rgb.get(j.occluded as usize) else {
                    continue;
                };
                println!(
                    "{corpus}/{stem},{:.2},{:.2},{},{},{n}",
                    j.pos.x,
                    j.pos.y,
                    to_hex(occ_rgb),
                    to_hex(other_rgb)
                );
            }
        }
    }
}
