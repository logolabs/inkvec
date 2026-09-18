//! Prints palette size, edge count and edge-length distribution for a fixed
//! set of synthetic corpus images traced with [`inkvec_trace::trace_color`].
fn main() {
    for name in [
        "mosaic_grid4",
        "rings_concentric",
        "logo_like",
        "prim_circle",
    ] {
        let path = format!("bench/data/corpus_raster/synthetic/128/{name}.png");
        let img = inkvec_trace::load_image(std::path::Path::new(&path)).unwrap();
        let (map, pal) = inkvec_trace::trace_color(&img, &inkvec_trace::ColorOptions::default());
        let mut lens: Vec<usize> = map.edges.iter().map(|e| e.points.len()).collect();
        lens.sort_unstable();
        let n = lens.len();
        let tiny = lens.iter().filter(|&&l| l <= 3).count();
        let total: usize = lens.iter().sum();
        println!(
            "{:18} palette {:2}  edges {:4}  pts {:5}  median-len {:3}  edges<=3pts {:4} ({:.0}%)",
            name,
            pal.len(),
            n,
            total,
            if n > 0 { lens[n / 2] } else { 0 },
            tiny,
            100.0 * tiny as f64 / n.max(1) as f64
        );
    }
}
