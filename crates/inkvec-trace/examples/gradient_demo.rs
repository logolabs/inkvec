//! Per-region fill model selection on the two synthetic gradient images.
//!
//! Runs the `trace_color` front end (palette + labels), then fits every label with
//! `gradient::fit_candidates` and prints the chosen model against the flat alternative;
//! then merges the bands with `merge_gradient_bands` and prints what survives.
//!
//!     cargo run -p inkvec-trace --example gradient_demo
//!
//! Needs `bench/data/corpus_raster/synthetic/128/`; build it with
//! `cd bench && python -W ignore -m inkvec_bench.cli build --tiers 128`.

use std::path::PathBuf;

use inkvec_trace::color::{extract_palette, label_image, to_hex};
use inkvec_trace::coverage::estimate_noise;
use inkvec_trace::gradient::{
    bic_lambda, fill_to_svg, fit_candidates, merge_gradient_bands, FillModel, Interp,
};
use inkvec_trace::ColorOptions;

fn space(i: Interp) -> &'static str {
    match i {
        Interp::LinearRgb => "[linearRGB]",
        Interp::Srgb => "[sRGB]",
    }
}

fn describe(m: &FillModel) -> String {
    match m {
        FillModel::Flat(c) => format!("Flat   {}", to_hex(*c)),
        FillModel::Linear {
            p0,
            p1,
            c0,
            c1,
            interp,
            ..
        } => format!(
            "Linear ({:.1},{:.1})->({:.1},{:.1}) {}->{} {}",
            p0.0,
            p0.1,
            p1.0,
            p1.1,
            to_hex(*c0),
            to_hex(*c1),
            space(*interp)
        ),
        FillModel::Radial {
            c,
            r,
            c0,
            c1,
            interp,
            ..
        } => format!(
            "Radial c=({:.1},{:.1}) r={:.1} {}->{} {}",
            c.0,
            c.1,
            r,
            to_hex(*c0),
            to_hex(*c1),
            space(*interp)
        ),
    }
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../bench/data/corpus_raster/synthetic/128");
    let opts = ColorOptions::default();

    for name in ["gradient_linear", "gradient_radial"] {
        let path = root.join(format!("{name}.png"));
        let img = match inkvec_trace::load_image(&path) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("{}: {e} (build bench data first)", path.display());
                std::process::exit(1);
            }
        };
        let (w, h) = (img.width, img.height);
        let rgb = img.composited([1.0, 1.0, 1.0]);
        let pal = extract_palette(
            &rgb,
            img.width,
            img.height,
            opts.merge_distance,
            opts.max_colors,
        );
        let mut labels = label_image(&rgb, &pal);
        let lum: Vec<f32> = rgb
            .iter()
            .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
            .collect();
        let sigma = estimate_noise(&lum, w, h);

        println!(
            "\n== {name}  {w}x{h}  palette {} entries  sigma_noise {:.2}/255",
            pal.len(),
            sigma * 255.0
        );
        println!(
            "  {:<5} {:>6}  {:<52} {:>12} {:>12} {:>9}",
            "label", "pixels", "chosen", "cost", "cost(flat)", "ratio"
        );

        for l in 0..pal.len() as u16 {
            let n = labels.iter().filter(|&&x| x == l).count();
            if n == 0 {
                continue;
            }
            let cands = fit_candidates(&rgb, w, h, &labels, l, sigma, bic_lambda(n));
            let flat = cands[0].cost;
            let best = cands
                .iter()
                .min_by(|a, b| a.cost.partial_cmp(&b.cost).unwrap())
                .unwrap();
            println!(
                "  {:<5} {:>6}  {:<52} {:>12.1} {:>12.1} {:>9.3}",
                l,
                n,
                describe(&best.model),
                best.cost,
                flat,
                best.cost / flat
            );
        }

        let fills = merge_gradient_bands(&mut labels, &rgb, w, h, &pal, sigma, bic_lambda(w * h));
        println!("  after merge_gradient_bands:");
        for (l, f) in fills.iter().enumerate() {
            let n = labels.iter().filter(|&&x| x as usize == l).count();
            if n == 0 {
                continue;
            }
            let (defs, fill) = fill_to_svg(&f.model, &format!("f{l}"));
            println!(
                "  {:<5} {:>6}  {:<52} chi2 {:.0}  cost {:.1}  fill={fill}",
                l,
                n,
                describe(&f.model),
                f.chi2,
                f.cost
            );
            if !defs.is_empty() {
                println!("         {defs}");
            }
        }
    }
}
