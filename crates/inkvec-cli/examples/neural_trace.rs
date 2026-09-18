//! Trace an image from a label map a neural network produced, instead of from the
//! palette and labelling stages.
//!
//! Everything downstream of labelling is the shipped pipeline: planar map, sub-pixel
//! refinement, junction refinement, the global boundary solve, symmetry, the fitting
//! dynamic program, ring repair, fills and the emitter. Fills are fitted from the raster
//! here as always -- a label map says *where* the regions are, not what colour they are.
//!
//! Usage: `neural_trace <input_image> <output_svg> <guidance.bin> [--lossy on|off|auto]
//! [--lambda-scale <f>] [--no-bopt]`
//!
//! `guidance.bin` is little-endian:
//!
//! ```text
//!   magic  b"NLBL"
//!   u32    version = 1 or 2   (identical layout; 2 may carry bit2)
//!   u32    w
//!   u32    h
//!   u32    n_labels
//!   u32    flags        bit0 = per-pixel sigma map, bit1 = per-pixel subpixel deltas,
//!                       bit2 = per-pixel lambda scale
//!   u16    label        x w*h, row major
//!   f32    sigma        x w*h,   present iff bit0 -- boundary position uncertainty, px
//!   f32    dx, dy       x w*h*2, present iff bit1 -- boundary offset, px
//!   f32    lambda       x w*h,   present iff bit2 -- relative parameter density
//! ```

use inkvec_cli::{trace_color_from_labels_guided, Args};
use inkvec_trace::{load_image, ColorTrace};
use std::{error::Error, path::Path};

/// The smallest positional uncertainty a supplied sigma map is allowed to claim.
///
/// A sigma of zero asks the fitter for infinite confidence in one point, which no
/// measurement of a rasterised boundary can support and which makes the dynamic program's
/// cost blow up around it.
const SIGMA_FLOOR: f64 = 0.02;

/// How much of a predicted offset to apply, and how far a single prediction may move a
/// point. Half, and one pixel: a label map is already a pixel-accurate statement, and the
/// sub-pixel refinement below has read the image, so this nudges rather than overrules.
const DELTA_GAIN: f64 = 0.5;
const DELTA_CLAMP: f64 = 1.0;

/// How far a predicted lambda scale may move the price of a parameter on one boundary.
///
/// The map is a *relative* density -- what the artist spent here against what they spent
/// on average -- so 1.0 is neutral and the useful range is well inside a factor of a few.
/// The clamp exists because a prediction is not a measurement: a rogue pixel reading zero
/// would make parameters free on the boundary that passes through it, and one reading
/// enormous would flatten a boundary to a single chord.
const LAMBDA_FLOOR: f64 = 0.05;
const LAMBDA_CEIL: f64 = 20.0;

struct Guidance {
    w: usize,
    h: usize,
    n_labels: usize,
    labels: Vec<u16>,
    sigma: Option<Vec<f32>>,
    deltas: Option<Vec<f32>>,
    lambda: Option<Vec<f32>>,
}

fn take_u32(b: &[u8], off: &mut usize) -> Result<u32, Box<dyn Error>> {
    let end = *off + 4;
    let slice = b
        .get(*off..end)
        .ok_or("guidance file ends inside its header")?;
    *off = end;
    Ok(u32::from_le_bytes(slice.try_into()?))
}

fn read_guidance(path: &Path) -> Result<Guidance, Box<dyn Error>> {
    let b = std::fs::read(path)?;
    if b.len() < 24 || &b[0..4] != b"NLBL" {
        return Err(format!(
            "{}: not an NLBL guidance file (expected magic \"NLBL\", got {:?})",
            path.display(),
            String::from_utf8_lossy(&b[0..4.min(b.len())])
        )
        .into());
    }
    let mut off = 4usize;
    let version = take_u32(&b, &mut off)?;
    // v2 is v1 plus the option of a lambda-scale plane; the layout is the same and the
    // flags say what is actually there, so both are read by the same code.
    if version != 1 && version != 2 {
        return Err(format!(
            "{}: NLBL version {version}, expected 1 or 2",
            path.display()
        )
        .into());
    }
    let w = take_u32(&b, &mut off)? as usize;
    let h = take_u32(&b, &mut off)? as usize;
    let n_labels = take_u32(&b, &mut off)? as usize;
    let flags = take_u32(&b, &mut off)?;
    let n = w
        .checked_mul(h)
        .ok_or("guidance header claims an impossible size")?;
    if n == 0 {
        return Err(format!("{}: NLBL claims {w}x{h}", path.display()).into());
    }

    let need = n * 2;
    let end = off + need;
    if b.len() < end {
        return Err(format!(
            "{}: NLBL says {w}x{h} = {n} labels ({need} bytes) but only {} bytes follow the header",
            path.display(),
            b.len() - off
        )
        .into());
    }
    let labels: Vec<u16> = b[off..end]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    off = end;

    let mut read_f32s = |count: usize, what: &str| -> Result<Vec<f32>, Box<dyn Error>> {
        let end = off + count * 4;
        if b.len() < end {
            return Err(format!(
                "{}: NLBL flags ask for a {what} of {count} floats ({} bytes) but only {} bytes remain",
                path.display(),
                count * 4,
                b.len().saturating_sub(off)
            )
            .into());
        }
        let v: Vec<f32> = b[off..end]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        off = end;
        Ok(v)
    };
    let sigma = if flags & 1 != 0 {
        Some(read_f32s(n, "sigma map")?)
    } else {
        None
    };
    let deltas = if flags & 2 != 0 {
        Some(read_f32s(n * 2, "delta map")?)
    } else {
        None
    };
    let lambda = if flags & 4 != 0 {
        Some(read_f32s(n, "lambda-scale map")?)
    } else {
        None
    };
    if off != b.len() {
        eprintln!(
            "note: {} bytes of {} were not read (flags {flags:#x})",
            b.len() - off,
            path.display()
        );
    }

    Ok(Guidance {
        w,
        h,
        n_labels,
        labels,
        sigma,
        deltas,
        lambda,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let argv: Vec<String> = std::env::args().collect();
    let mut positional: Vec<String> = Vec::new();
    let mut lossy_flag: Option<String> = None;
    let mut lambda_scale = 1.0f64;
    let mut no_bopt = false;
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--no-bopt" => no_bopt = true,
            "--lossy" => {
                i += 1;
                lossy_flag = Some(
                    argv.get(i)
                        .cloned()
                        .ok_or("--lossy needs one of on|off|auto")?,
                );
            }
            "--lambda-scale" => {
                i += 1;
                lambda_scale = argv
                    .get(i)
                    .ok_or("--lambda-scale needs a value")?
                    .parse()
                    .map_err(|_| "bad --lambda-scale")?;
            }
            a if a.starts_with("--") => return Err(format!("unknown flag {a}").into()),
            a => positional.push(a.to_string()),
        }
        i += 1;
    }
    if positional.len() < 3 {
        eprintln!(
            "Usage: neural_trace <input_image> <output_svg> <guidance.bin> \
             [--lossy on|off|auto] [--lambda-scale <f>] [--no-bopt]"
        );
        std::process::exit(1);
    }
    let (input_path, output_path, guidance_path) = (&positional[0], &positional[1], &positional[2]);

    let img = load_image(Path::new(input_path))?;
    let g = read_guidance(Path::new(guidance_path))?;
    if g.w != img.width || g.h != img.height {
        return Err(format!(
            "guidance is {}x{} but {} is {}x{}",
            g.w, g.h, input_path, img.width, img.height
        )
        .into());
    }

    let lossy = match lossy_flag.as_deref().unwrap_or("auto") {
        "on" => inkvec_sr::Mode::On,
        "off" => inkvec_sr::Mode::Off,
        "auto" => {
            let head = std::fs::read(input_path)?;
            if inkvec_trace::lossy_container(&head).unwrap_or(false) {
                inkvec_sr::Mode::On
            } else {
                inkvec_sr::Mode::Off
            }
        }
        other => return Err(format!("--lossy {other}: expected on, off or auto").into()),
    };
    let cli_args = Args {
        lossy,
        quiet: true,
        lambda_scale,
        ..Args::default()
    };

    if no_bopt {
        // Before the trace, because the tracer reads it once, on the way past.
        std::env::set_var("INKVEC_BOPT", "0");
    }

    let (w, h) = (g.w, g.h);
    let sigma = g.sigma;
    let deltas = g.deltas;
    let lambda = g.lambda;
    let mut n_faces = 0usize;
    let mut n_edges = 0usize;

    let (svg, _) = trace_color_from_labels_guided(
        &img,
        &cli_args,
        &g.labels,
        g.n_labels,
        |t: &mut ColorTrace| {
            for e in t.map.edges.iter_mut() {
                let last = e.points.len().saturating_sub(1);
                // The boundary's own price for a parameter, from the map under it. The
                // fitter charges one lambda for the whole boundary, so the per-pixel
                // prediction has to be pooled into one number, and the geometric mean is
                // the mean of a ratio: a stretch predicted at 2x and one at 0.5x cancel
                // instead of averaging to 1.25x. Accumulated in logs so a long boundary
                // cannot underflow the product.
                let mut log_sum = 0.0f64;
                let mut log_n = 0usize;
                for k in 0..e.points.len() {
                    let p = e.points[k];
                    let px = (p.x.max(0.0).round() as usize).min(w - 1);
                    let py = (p.y.max(0.0).round() as usize).min(h - 1);
                    let q = py * w + px;
                    if let Some(lm) = lambda.as_ref() {
                        // Clamped per sample, not only after the mean: a predicted zero
                        // is a log of minus infinity and one NaN pixel would otherwise
                        // take the whole boundary with it.
                        let v = lm[q] as f64;
                        let v = if v.is_finite() {
                            v.clamp(LAMBDA_FLOOR, LAMBDA_CEIL)
                        } else {
                            1.0
                        };
                        log_sum += v.ln();
                        log_n += 1;
                    }
                    if let Some(sm) = sigma.as_ref() {
                        if let Some(s) = e.sigma.get_mut(k) {
                            *s = (sm[q] as f64).max(SIGMA_FLOOR);
                        }
                    }
                    if let Some(dt) = deltas.as_ref() {
                        // The ends of an open boundary are junction nodes, shared with
                        // every other boundary that meets there. Moving one from a
                        // per-pixel prediction tears the map apart, so they stay put and
                        // only the interior of the boundary is nudged.
                        if !e.closed && (k == 0 || k == last) {
                            continue;
                        }
                        let dx = (dt[q * 2] as f64 * DELTA_GAIN).clamp(-DELTA_CLAMP, DELTA_CLAMP);
                        let dy =
                            (dt[q * 2 + 1] as f64 * DELTA_GAIN).clamp(-DELTA_CLAMP, DELTA_CLAMP);
                        e.points[k].x += dx;
                        e.points[k].y += dy;
                    }
                }
                if log_n > 0 {
                    e.lambda_scale = (log_sum / log_n as f64)
                        .exp()
                        .clamp(LAMBDA_FLOOR, LAMBDA_CEIL);
                }
            }
            n_faces = t.map.n_labels;
            n_edges = t.map.edges.len();
        },
    )?;

    std::fs::write(output_path, &svg)?;
    println!(
        "{{\"n_faces\":{n_faces},\"edges\":{n_edges},\"svg_bytes\":{}}}",
        svg.len()
    );
    Ok(())
}
