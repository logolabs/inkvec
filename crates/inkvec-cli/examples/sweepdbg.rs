//! Debug dump of the tolerance sweep for one edge of a synthetic test image.
use inkvec_fit::{curves, optimal_polygon, FitConfig, PARAMS_LINE};

fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "prim_circle".into());
    let want = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1);
    let path = format!("bench/data/corpus_raster/synthetic/128/{name}.png");
    let img = inkvec_trace::load_image(std::path::Path::new(&path)).unwrap();
    let (map, _) = inkvec_trace::trace_color(&img, &inkvec_trace::ColorOptions::default());
    let cfg = FitConfig::from_precision(128.0, 0.1, 2.0);
    let e = &map.edges[want];
    let poly = e.as_polyline();
    let seg = optimal_polygon(&poly, &cfg);
    let sig_max = poly.sigma.iter().cloned().fold(0.0f64, f64::max);
    let mut sv = poly.sigma.clone();
    sv.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let tol = cfg.tau * sig_max.max(0.05);
    println!(
        "{name} edge {want}: {} pts closed={}  sigma med {:.3} max {:.3}  tol {:.3}",
        poly.len(),
        e.closed,
        sv[sv.len() / 2],
        sig_max,
        tol
    );
    println!(
        "LINE: {} segs, params-cost {:.0}\n",
        seg.segment_count(),
        cfg.lambda * seg.segment_count() as f64 * PARAMS_LINE
    );
    let mut pts = poly.points.clone();
    pts.push(poly.points[0]);
    let mut sig = poly.sigma.clone();
    sig.push(poly.sigma[0]);
    println!(
        "  {:>3} {:>7} {:>7} {:>10} {:>9}",
        "hw", "tol", "cubics", "chi2", "cost"
    );
    for hw in [0usize, 2, 4, 8, 16, 32] {
        for mult in [0.5f64, 1.0, 2.0, 4.0, 8.0] {
            let t = tol * mult;
            if let Some(segs) = curves::fit_cubics_smoothed(&pts, t, hw, true, false) {
                let chi2 = curves::chi2(&pts, &sig, pts[0], &segs);
                let params: f64 = segs.iter().map(|s| s.params()).sum();
                println!(
                    "  {:>3} {:>7.3} {:>7} {:>10.0} {:>9.0}",
                    hw,
                    t,
                    segs.len(),
                    chi2,
                    0.5 * chi2 + cfg.lambda * params
                );
            }
        }
    }
}
