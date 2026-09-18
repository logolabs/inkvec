//! Debug dump of the polygon and curve fit for each edge of a synthetic test image.
use inkvec_fit::{curves::Segment, fit_path, optimal_polygon, FitConfig};

fn main() {
    for name in ["rings_concentric", "logo_like", "prim_circle"] {
        let path = format!("bench/data/corpus_raster/synthetic/128/{name}.png");
        let img = inkvec_trace::load_image(std::path::Path::new(&path)).unwrap();
        let (map, _) = inkvec_trace::trace_color(&img, &inkvec_trace::ColorOptions::default());
        let cfg = FitConfig::from_precision(128.0, 0.1, 2.0);
        println!("\n{name}  (lambda {:.2})", cfg.lambda);
        println!(
            "  {:>4} {:>6} {:>7} {:>8} {:>7} {:>7}",
            "edge", "pts", "polygon", "breaks", "lines", "cubics"
        );
        for (k, e) in map.edges.iter().enumerate() {
            let poly = e.as_polyline();
            let seg = optimal_polygon(&poly, &cfg);
            let f = fit_path(&poly, &cfg);
            let nl = f
                .segments
                .iter()
                .filter(|s| matches!(s, Segment::Line(..)))
                .count();
            let nc = f
                .segments
                .iter()
                .filter(|s| matches!(s, Segment::Cubic(..)))
                .count();
            println!(
                "  {k:>4} {:>6} {:>7} {:>8} {:>7} {:>7}",
                poly.len(),
                seg.segment_count(),
                "-",
                nl,
                nc
            );
        }
    }
}
