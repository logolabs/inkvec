//! Export reproducible conflict evidence without changing the detector or its threshold.
//! Usage: cargo run --release -p inkvec-trace --example occlusion_conflicts -- [per-corpus=200] [output=out/occlusion-conflicts]
use inkvec_trace::{load_image, occlusion, planar, trace_color_full, ColorOptions};
use rayon::prelude::*;
use std::{fmt::Write as _, path::Path};

fn claims(map: &planar::PlanarMap) -> String {
    let rows: Vec<_> = occlusion::find(map)
        .iter()
        .map(|j| {
            format!(
                "[{},{},{},{:.8},{:.8},{:.8}]",
                j.node, j.occluder, j.occluded, j.pos.x, j.pos.y, j.bend_deg
            )
        })
        .collect();
    format!("[{}]", rows.join(","))
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let n = args
        .get(1)
        .map(|s| s.parse::<usize>().expect("image count"))
        .unwrap_or(200);
    let out = args
        .get(2)
        .map(String::as_str)
        .unwrap_or("out/occlusion-conflicts");
    std::fs::create_dir_all(out).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/data/corpus_raster");
    let mut files = Vec::new();
    for corpus in [
        "lucide",
        "material-icons",
        "simple-icons",
        "openmoji",
        "twemoji",
        "noto-emoji",
    ] {
        let mut paths: Vec<_> = std::fs::read_dir(root.join(corpus).join("128ss"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|s| s == "png"))
            .collect();
        paths.sort();
        paths.truncate(n);
        files.extend(paths.into_iter().map(|p| (corpus, p)));
    }
    let summaries: Vec<_> = files
        .par_iter()
        .map(|(corpus, path)| {
            let img = load_image(path).unwrap();
            let t = trace_color_full(&img, &ColorOptions::default());
            let js = occlusion::find(&t.map);
            let vs = occlusion::aggregate(&js);
            let splits = vs.iter().filter(|v| v.occluder.is_none()).count();
            let single = vs.iter().filter(|v| v.for_a + v.for_b == 1).count();
            let stem = path.file_stem().unwrap().to_str().unwrap();
            // Export all images: this permits comparing the original and perturbed detector
            // on the same population, rather than only on already-selected conflicts.
            let mut s = format!(
                "{{\"image\":\"{corpus}/{stem}\",\"width\":{},\"height\":{},\"claims\":{},",
                img.width,
                img.height,
                claims(&t.map)
            );
            let raw = planar::build(&t.labels, img.width, img.height, t.map.n_labels);
            write!(
                s,
                "\"grid_claims\":{},\"face_rgb\":{:?},\"labels\":{:?},\"edges\":[",
                claims(&raw),
                t.face_rgb,
                t.labels
            )
            .unwrap();
            for (i, e) in t.map.edges.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write!(
                    s,
                    "{{\"left\":{},\"right\":{},\"start\":{},\"end\":{},\"closed\":{},\"points\":[",
                    e.left, e.right, e.start_node, e.end_node, e.closed
                )
                .unwrap();
                for (k, p) in e.points.iter().enumerate() {
                    if k > 0 {
                        s.push(',');
                    }
                    write!(s, "[{:.8},{:.8}]", p.x, p.y).unwrap();
                }
                s.push_str("]}");
            }
            s.push_str("]}");
            std::fs::write(Path::new(out).join(format!("{corpus}--{stem}.json")), s).unwrap();
            format!(
                "{corpus}/{stem},{},{},{},{},{}",
                js.len(),
                vs.len(),
                splits,
                single,
                vs.len() - splits - single
            )
        })
        .collect();
    std::fs::write(
        Path::new(out).join("summary.csv"),
        format!(
            "image,claims,pairs,split,single,corroborated\n{}\n",
            summaries.join("\n")
        ),
    )
    .unwrap();
    println!("Exported {} images to {out}", summaries.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_abutting_rectangles_disagree_with_perfectly_straight_arms() {
        // Disjoint rectangles touch along x=6, y=6..10. At one end A continues
        // straight; at the other B does. Neither rectangle covers the other.
        for scale in [1, 2, 4] {
            let (w, h) = (16 * scale, 18 * scale);
            let mut labels = vec![0; w * h];
            for y in 0..h {
                for x in 0..w {
                    let (x0, y0) = (x / scale, y / scale);
                    labels[y * w + x] = if (2..6).contains(&x0) && (2..10).contains(&y0) {
                        1
                    } else if (6..10).contains(&x0) && (6..14).contains(&y0) {
                        2
                    } else {
                        0
                    };
                }
            }
            let map = planar::build(&labels, w, h, 3);
            let js = occlusion::find(&map);
            let between: Vec<_> = js
                .iter()
                .copied()
                .filter(|j| (j.occluder.min(j.occluded), j.occluder.max(j.occluded)) == (1, 2))
                .collect();
            assert_eq!(between.len(), 2);
            assert!(between.iter().all(|j| j.bend_deg.abs() < 1e-9));
            let verdicts = occlusion::aggregate(&between);
            assert_eq!(verdicts[0].occluder, None);
            assert_eq!((verdicts[0].for_a, verdicts[0].for_b), (1, 1));
        }
    }
}
