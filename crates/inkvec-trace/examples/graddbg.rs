//! Prints, per palette label of a synthetic corpus image, the gradient fit
//! candidates and how they compare to the flat fit, to debug fill selection.
use inkvec_trace::color;
use inkvec_trace::gradient::{self, FillModel};

fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "rings_concentric".into());
    let path = format!("bench/data/corpus_raster/synthetic/128/{name}.png");
    let img = inkvec_trace::load_image(std::path::Path::new(&path)).unwrap();
    let (w, h) = (img.width, img.height);
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let pal = color::extract_palette(
        &rgb,
        img.width,
        img.height,
        color::DEFAULT_MERGE_DISTANCE,
        64,
    );
    let labels = color::label_image(&rgb, &pal);
    let lum: Vec<f32> = rgb
        .iter()
        .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
        .collect();
    let sigma = inkvec_trace::coverage::estimate_noise(&lum, w, h);
    println!("{name}: {} colours, sigma_noise {:.5}\n", pal.len(), sigma);
    println!(
        "{:>3} {:>7} {:>9} {:>10} {:>10} {:>10} {:>9} {:>8}",
        "lbl", "px", "chosen", "chi2_flat", "chi2_best", "d_chi2", "d_params", "stopdist"
    );
    for li in 0..pal.len() {
        let cands = gradient::fit_candidates(&rgb, w, h, &labels, li as u16, sigma, 7.15);
        if cands.is_empty() {
            continue;
        }
        let px = (0..w * h).filter(|&p| labels[p] == li as u16).count();
        let flat = cands.iter().find(|c| matches!(c.model, FillModel::Flat(_)));
        let best = cands
            .iter()
            .min_by(|a, b| a.cost.partial_cmp(&b.cost).unwrap())
            .unwrap();
        let (kind, sd) = match best.model {
            FillModel::Flat(_) => ("flat", 0.0),
            FillModel::Linear { c0, c1, .. } => (
                "linear",
                color::rgb_to_oklab(c0).dist(color::rgb_to_oklab(c1)),
            ),
            FillModel::Radial { c0, c1, .. } => (
                "radial",
                color::rgb_to_oklab(c0).dist(color::rgb_to_oklab(c1)),
            ),
        };
        let fc = flat.map(|f| f.chi2).unwrap_or(f64::NAN);
        println!(
            "{:>3} {:>7} {:>9} {:>10.0} {:>10.0} {:>10.0} {:>9.0} {:>8.4}",
            li,
            px,
            kind,
            fc,
            best.chi2,
            fc - best.chi2,
            (best.params - 3.0) * 7.15,
            sd
        );
    }
}
