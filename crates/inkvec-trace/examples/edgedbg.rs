//! Prints per-edge point counts and `sigma` statistics for a few synthetic
//! corpus images traced with [`inkvec_trace::trace_color`].
fn main() {
    for name in ["logo_like", "mosaic_pie6", "rings_concentric"] {
        let path = format!("bench/data/corpus_raster/synthetic/128/{name}.png");
        let img = inkvec_trace::load_image(std::path::Path::new(&path)).unwrap();
        let (map, _) = inkvec_trace::trace_color(&img, &inkvec_trace::ColorOptions::default());
        println!("\n{name}: {} edges", map.edges.len());
        for (k, e) in map.edges.iter().enumerate().take(10) {
            let sig_max = e.sigma.iter().cloned().fold(0.0f64, f64::max);
            let sig_med = {
                let mut v = e.sigma.clone();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                v[v.len() / 2]
            };
            println!(
                "  edge {k:2}: {:4} pts  closed={}  sigma med {:.3} max {:.3}  L={} R={}",
                e.points.len(),
                e.closed,
                sig_med,
                sig_max,
                e.left,
                e.right
            );
        }
    }
}
