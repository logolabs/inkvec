//! Compares the fixed-threshold and MDL palette extractors on one corpus
//! image, printing the resulting colours and the true distinct-colour count.
use inkvec_trace::color;
fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "rings_concentric".into());
    let img = inkvec_trace::load_image(std::path::Path::new(&format!(
        "bench/data/corpus_raster/synthetic/128/{name}.png"
    )))
    .unwrap();
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let lum: Vec<f32> = rgb
        .iter()
        .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
        .collect();
    let sigma = inkvec_trace::coverage::estimate_noise(&lum, img.width, img.height);
    let lambda = inkvec_trace::gradient::bic_lambda(img.width * img.height);
    println!("sigma_noise {sigma:.5}, lambda {lambda:.2}");
    let fixed = color::extract_palette(&rgb, img.width, img.height, 0.055, 64);
    println!("fixed threshold   -> {} colours", fixed.len());
    let mdl = color::extract_palette_mdl(
        &rgb,
        img.width,
        img.height,
        0.055,
        64,
        color::PaletteEvidence {
            sigma_noise: sigma,
            lambda,
            noise_sigmas: 0.0,
            same_ink_de00: color::SAME_INK_DE00,
        },
    );
    let hexes: Vec<String> = mdl.rgb.iter().map(|&c| color::to_hex(c)).collect();
    println!(
        "MDL               -> {} colours: {}",
        mdl.len(),
        hexes.join(" ")
    );
    // How many distinct colours are actually present, by pixel count?
    let mut counts: std::collections::HashMap<[u8; 3], usize> = std::collections::HashMap::new();
    for c in &rgb {
        let k = [
            (c[0] * 255.0) as u8,
            (c[1] * 255.0) as u8,
            (c[2] * 255.0) as u8,
        ];
        *counts.entry(k).or_insert(0) += 1;
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    println!("\ntop source colours by pixel count:");
    for (c, n) in v.iter().take(12) {
        println!(
            "  #{:02x}{:02x}{:02x}  {n:6} px ({:.2}%)",
            c[0],
            c[1],
            c[2],
            100.0 * *n as f64 / rgb.len() as f64
        );
    }
}
