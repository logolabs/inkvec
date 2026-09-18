//! Fit existing gradient candidates on a supplied region mask, without palette merging.
//! Usage: gradient_region_probe image.png mask.png output_directory
use inkvec_trace::gradient::{fill_to_svg, fit_candidates};
use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: gradient_region_probe image mask outdir".into());
    }
    let image = image::open(&args[1])?.to_rgb8();
    let mask = image::open(&args[2])?.to_luma8();
    if image.dimensions() != mask.dimensions() {
        return Err("image/mask size mismatch".into());
    }
    let (w, h) = (image.width() as usize, image.height() as usize);
    let rgb: Vec<[f32; 3]> = image
        .pixels()
        .map(|p| p.0.map(|x| x as f32 / 255.0))
        .collect();
    let labels: Vec<u16> = mask.pixels().map(|p| u16::from(p[0] >= 128)).collect();
    let out = Path::new(&args[3]);
    std::fs::create_dir(out)?;
    for (i, fit) in fit_candidates(&rgb, w, h, &labels, 1, 0.5 / 255.0, 6.0)
        .iter()
        .enumerate()
    {
        let (defs, fill) = fill_to_svg(&fit.model, "proposal");
        let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 {w} {h}\" width=\"{w}\" height=\"{h}\"><defs>{defs}</defs><rect x=\"-0.5\" y=\"-0.5\" width=\"{w}\" height=\"{h}\" fill=\"{fill}\"/></svg>");
        std::fs::write(out.join(format!("candidate_{i}.svg")), svg)?;
        println!(
            "{i}\t{}\t{}\t{}\t{}",
            fit.model.kind(),
            fit.params,
            fit.chi2,
            fit.cost
        );
    }
    Ok(())
}
