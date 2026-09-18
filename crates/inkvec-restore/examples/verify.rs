//! Runs the in-process restorer on one PNG, times it, writes the output PNG, and, given a raw
//! float reference (little-endian f32, interleaved RGB, from onnxruntime on the same ONNX file),
//! reports how far the output is from it.
//!
//!     cargo run --release -p inkvec-restore --features onnxruntime,flex,ndarray --example verify -- \
//!         <in.png> <out.png> [--backend ort|flex|ndarray] [--weights <file>] \
//!         [--reference <ref.f32>] [--runs N]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use inkvec_restore::Restore;

fn main() -> Result<(), Box<dyn Error>> {
    let mut positional = Vec::new();
    let mut backend = String::from("ort");
    let mut weights: Option<PathBuf> = None;
    let mut reference = None;
    let mut runs = 3usize;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--backend" => backend = args.next().ok_or("--backend needs a value")?,
            "--weights" => {
                weights = Some(PathBuf::from(args.next().ok_or("--weights needs a value")?))
            }
            "--reference" => {
                reference = Some(PathBuf::from(
                    args.next().ok_or("--reference needs a value")?,
                ))
            }
            "--runs" => runs = args.next().ok_or("--runs needs a value")?.parse()?,
            _ => positional.push(a),
        }
    }
    let [input, output] = positional.as_slice() else {
        return Err("usage: verify <in.png> <out.png> [--backend ort|flex|ndarray] [--weights f] [--reference f] [--runs n]".into());
    };

    let img = inkvec_trace::load_image(Path::new(input))?;
    let (w, h) = (img.width, img.height);
    let mut rgb = vec![0f32; w * h * 3];
    for i in 0..w * h {
        rgb[i * 3..i * 3 + 3].copy_from_slice(&img.data[i * 4..i * 4 + 3]);
    }

    let t = Instant::now();
    let net = load(&backend, weights.as_deref())?;
    eprintln!(
        "{w}x{h}: {} loaded in {:.0} ms",
        net.describe(),
        t.elapsed().as_secs_f64() * 1e3
    );
    let mut out = Vec::new();
    for i in 0..runs.max(1) {
        let t = Instant::now();
        out = net.restore(&rgb, w, h)?;
        eprintln!("{w}x{h}: run {i} {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    }

    if let Some(path) = reference {
        let bytes = std::fs::read(&path)?;
        let refv: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        if refv.len() != out.len() {
            return Err(
                format!("reference has {} floats, output {}", refv.len(), out.len()).into(),
            );
        }
        let mut d: Vec<f32> = out.iter().zip(&refv).map(|(a, b)| (a - b).abs()).collect();
        let mean = d.iter().map(|&v| v as f64).sum::<f64>() / d.len() as f64;
        d.sort_by(f32::total_cmp);
        let p99 = d[d.len() * 99 / 100];
        let max = d[d.len() - 1];
        eprintln!(
            "vs reference: mean {mean:.2e}, p99 {p99:.2e}, max {max:.2e} ({:.3} levels max)",
            max * 255.0
        );
    }

    let buf: Vec<u8> = out
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    image::RgbImage::from_raw(w as u32, h as u32, buf)
        .ok_or("output buffer has the wrong size")?
        .save(output)?;
    Ok(())
}

fn load(backend: &str, weights: Option<&Path>) -> Result<Box<dyn Restore>, Box<dyn Error>> {
    match backend {
        #[cfg(feature = "onnxruntime")]
        "ort" => {
            let path = match weights {
                Some(w) => w.to_path_buf(),
                None => inkvec_restore::default_onnx()?,
            };
            Ok(Box::new(inkvec_restore::onnx::OnnxRestorer::load(&path)?))
        }
        #[cfg(feature = "flex")]
        "flex" => {
            let path = weights
                .map(Path::to_path_buf)
                .unwrap_or_else(inkvec_restore::default_weights);
            Ok(Box::new(inkvec_restore::model::Restorer::<
                burn::backend::Flex,
            >::load(&path, Default::default())?))
        }
        #[cfg(feature = "ndarray")]
        "ndarray" => {
            let path = weights
                .map(Path::to_path_buf)
                .unwrap_or_else(inkvec_restore::default_weights);
            Ok(Box::new(inkvec_restore::model::Restorer::<
                burn::backend::NdArray,
            >::load(&path, Default::default())?))
        }
        other => {
            let _ = weights;
            Err(format!("backend {other:?} is not compiled in").into())
        }
    }
}
