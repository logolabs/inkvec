//! Oracle guidance research harness for JPEG restoration and topology experiments.
//! Zero-dependency binary reader.
//! Usage: oracle_guidance <input> <output_svg> [guidance.bin] [dump.txt]

use inkvec_cli::{trace_color_guided, Args};
use inkvec_trace::{load_image, ColorTrace};
use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let args_env: Vec<String> = std::env::args().collect();
    if args_env.len() < 3 {
        eprintln!("Usage: oracle_guidance <input_image> <output_svg> [guidance.bin] [dump.txt]");
        std::process::exit(1);
    }
    let input_path = &args_env[1];
    let output_path = &args_env[2];
    let guidance_path = args_env.get(3);
    let dump_path = args_env.get(4);

    let img = load_image(Path::new(input_path))?;
    let input_bytes = std::fs::read(input_path)?;
    let is_lossy = inkvec_trace::lossy_container(&input_bytes).unwrap_or(false);
    let cli_args = Args {
        lossy: if is_lossy {
            inkvec_sr::Mode::On
        } else {
            inkvec_sr::Mode::Off
        },
        quiet: true,
        ..Args::default()
    };

    let mut remap_table: Option<Vec<u16>> = None;
    let mut edge_sigmas: Option<Vec<Vec<f64>>> = None;
    let mut neural_data: Option<(
        usize,
        usize,
        usize,
        Vec<u16>,
        Vec<[f32; 3]>,
        Option<Vec<f32>>,
        Option<Vec<f32>>,
        Option<Vec<inkvec_trace::gradient::FillModel>>,
    )> = None;
    let mut global_sigma_scale = 1.0f64;

    if let Some(gp) = guidance_path {
        if Path::new(gp).exists() {
            let bytes = std::fs::read(gp)?;
            if bytes.len() >= 24 && &bytes[0..4] == b"ORCL" {
                let mode = u32::from_le_bytes(bytes[4..8].try_into()?);
                global_sigma_scale = f64::from_le_bytes(bytes[8..16].try_into()?);
                let n_remap = u32::from_le_bytes(bytes[16..20].try_into()?) as usize;
                let mut offset = 20;

                if n_remap > 0 && offset + n_remap * 2 <= bytes.len() {
                    let mut r = Vec::with_capacity(n_remap);
                    for _ in 0..n_remap {
                        r.push(u16::from_le_bytes(bytes[offset..offset + 2].try_into()?));
                        offset += 2;
                    }
                    if mode & 1 != 0 {
                        remap_table = Some(r);
                    }
                }

                if offset + 4 <= bytes.len() {
                    let n_edges =
                        u32::from_le_bytes(bytes[offset..offset + 4].try_into()?) as usize;
                    offset += 4;
                    let mut es = Vec::with_capacity(n_edges);
                    for _ in 0..n_edges {
                        if offset + 4 > bytes.len() {
                            break;
                        }
                        let n_pts =
                            u32::from_le_bytes(bytes[offset..offset + 4].try_into()?) as usize;
                        offset += 4;
                        let mut s_vec = Vec::with_capacity(n_pts);
                        for _ in 0..n_pts {
                            if offset + 8 > bytes.len() {
                                break;
                            }
                            s_vec.push(f64::from_le_bytes(bytes[offset..offset + 8].try_into()?));
                            offset += 8;
                        }
                        es.push(s_vec);
                    }
                    if mode & 2 != 0 {
                        edge_sigmas = Some(es);
                    }
                }

                if mode & 4 != 0 && offset + 16 <= bytes.len() {
                    let nw = u32::from_le_bytes(bytes[offset..offset + 4].try_into()?) as usize;
                    let nh = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
                    let n_colors =
                        u32::from_le_bytes(bytes[offset + 8..offset + 12].try_into()?) as usize;
                    let has_sm = u32::from_le_bytes(bytes[offset + 12..offset + 16].try_into()?);
                    offset += 16;
                    let n_pix = nw * nh;
                    if offset + n_pix * 2 + n_colors * 12 <= bytes.len() {
                        let mut labels = Vec::with_capacity(n_pix);
                        for _ in 0..n_pix {
                            labels.push(u16::from_le_bytes(bytes[offset..offset + 2].try_into()?));
                            offset += 2;
                        }
                        let mut colors = Vec::with_capacity(n_colors);
                        for _ in 0..n_colors {
                            let r = f32::from_le_bytes(bytes[offset..offset + 4].try_into()?);
                            let g = f32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?);
                            let b = f32::from_le_bytes(bytes[offset + 8..offset + 12].try_into()?);
                            offset += 12;
                            colors.push([r, g, b]);
                        }
                        let mut sm_vec = None;
                        if has_sm & 1 != 0 && offset + n_pix * 4 <= bytes.len() {
                            let mut sm = Vec::with_capacity(n_pix);
                            for _ in 0..n_pix {
                                sm.push(f32::from_le_bytes(bytes[offset..offset + 4].try_into()?));
                                offset += 4;
                            }
                            sm_vec = Some(sm);
                        }

                        let mut deltas_vec = None;
                        if has_sm & 2 != 0 && offset + n_pix * 8 <= bytes.len() {
                            let mut dt = Vec::with_capacity(n_pix * 2);
                            for _ in 0..n_pix * 2 {
                                dt.push(f32::from_le_bytes(bytes[offset..offset + 4].try_into()?));
                                offset += 4;
                            }
                            deltas_vec = Some(dt);
                        }

                        let mut custom_fills = None;
                        if has_sm & 4 != 0 {
                            let mut cf = Vec::with_capacity(n_colors);
                            for _ in 0..n_colors {
                                if offset + 4 > bytes.len() {
                                    break;
                                }
                                let kind =
                                    u32::from_le_bytes(bytes[offset..offset + 4].try_into()?);
                                offset += 4;
                                match kind {
                                    0 => {
                                        let r = f32::from_le_bytes(
                                            bytes[offset..offset + 4].try_into()?,
                                        );
                                        let g = f32::from_le_bytes(
                                            bytes[offset + 4..offset + 8].try_into()?,
                                        );
                                        let b = f32::from_le_bytes(
                                            bytes[offset + 8..offset + 12].try_into()?,
                                        );
                                        offset += 12;
                                        cf.push(inkvec_trace::gradient::FillModel::Flat([r, g, b]));
                                    }
                                    1 => {
                                        let p0x = f64::from_le_bytes(
                                            bytes[offset..offset + 8].try_into()?,
                                        );
                                        let p0y = f64::from_le_bytes(
                                            bytes[offset + 8..offset + 16].try_into()?,
                                        );
                                        let p1x = f64::from_le_bytes(
                                            bytes[offset + 16..offset + 24].try_into()?,
                                        );
                                        let p1y = f64::from_le_bytes(
                                            bytes[offset + 24..offset + 32].try_into()?,
                                        );
                                        offset += 32;
                                        let c0r = f32::from_le_bytes(
                                            bytes[offset..offset + 4].try_into()?,
                                        );
                                        let c0g = f32::from_le_bytes(
                                            bytes[offset + 4..offset + 8].try_into()?,
                                        );
                                        let c0b = f32::from_le_bytes(
                                            bytes[offset + 8..offset + 12].try_into()?,
                                        );
                                        offset += 12;
                                        let c1r = f32::from_le_bytes(
                                            bytes[offset..offset + 4].try_into()?,
                                        );
                                        let c1g = f32::from_le_bytes(
                                            bytes[offset + 4..offset + 8].try_into()?,
                                        );
                                        let c1b = f32::from_le_bytes(
                                            bytes[offset + 8..offset + 12].try_into()?,
                                        );
                                        offset += 12;
                                        cf.push(inkvec_trace::gradient::FillModel::Linear {
                                            p0: (p0x, p0y),
                                            p1: (p1x, p1y),
                                            c0: [c0r, c0g, c0b],
                                            c1: [c1r, c1g, c1b],
                                            interp: inkvec_trace::gradient::Interp::Srgb,
                                            mids: Vec::new(),
                                        });
                                    }
                                    2 => {
                                        let cx = f64::from_le_bytes(
                                            bytes[offset..offset + 8].try_into()?,
                                        );
                                        let cy = f64::from_le_bytes(
                                            bytes[offset + 8..offset + 16].try_into()?,
                                        );
                                        let r = f64::from_le_bytes(
                                            bytes[offset + 16..offset + 24].try_into()?,
                                        );
                                        let aspect = f64::from_le_bytes(
                                            bytes[offset + 24..offset + 32].try_into()?,
                                        );
                                        let angle = f64::from_le_bytes(
                                            bytes[offset + 32..offset + 40].try_into()?,
                                        );
                                        offset += 40;
                                        let c0r = f32::from_le_bytes(
                                            bytes[offset..offset + 4].try_into()?,
                                        );
                                        let c0g = f32::from_le_bytes(
                                            bytes[offset + 4..offset + 8].try_into()?,
                                        );
                                        let c0b = f32::from_le_bytes(
                                            bytes[offset + 8..offset + 12].try_into()?,
                                        );
                                        offset += 12;
                                        let c1r = f32::from_le_bytes(
                                            bytes[offset..offset + 4].try_into()?,
                                        );
                                        let c1g = f32::from_le_bytes(
                                            bytes[offset + 4..offset + 8].try_into()?,
                                        );
                                        let c1b = f32::from_le_bytes(
                                            bytes[offset + 8..offset + 12].try_into()?,
                                        );
                                        offset += 12;
                                        cf.push(inkvec_trace::gradient::FillModel::Radial {
                                            c: (cx, cy),
                                            r,
                                            c0: [c0r, c0g, c0b],
                                            c1: [c1r, c1g, c1b],
                                            interp: inkvec_trace::gradient::Interp::Srgb,
                                            aspect,
                                            angle,
                                            mids: Vec::new(),
                                        });
                                    }
                                    _ => {
                                        cf.push(inkvec_trace::gradient::FillModel::Flat([
                                            0.0, 0.0, 0.0,
                                        ]));
                                    }
                                }
                            }
                            custom_fills = Some(cf);
                        }

                        neural_data = Some((
                            nw,
                            nh,
                            n_colors,
                            labels,
                            colors,
                            sm_vec,
                            deltas_vec,
                            custom_fills,
                        ));
                    }
                }
            }
        }
    }

    let mut edges_before = 0;
    let mut edges_active = 0;
    let mut n_faces = 0;

    let (svg, _) = trace_color_guided(&img, &cli_args, |t: &mut ColorTrace| {
        // 0. If neural multi-task inking is provided, rebuild planar map and palette directly!
        if let Some((
            nw,
            nh,
            n_colors,
            ref n_labels,
            ref n_colors_vec,
            ref s_map,
            ref dt_map,
            ref cf_opt,
        )) = neural_data
        {
            let mut new_map = inkvec_trace::planar::build(n_labels, nw, nh, n_colors);
            let rgb = img.composited([1.0, 1.0, 1.0]);
            let face_model: Vec<inkvec_trace::gradient::FillModel> = if let Some(ref cf) = cf_opt {
                cf.clone()
            } else {
                n_colors_vec
                    .iter()
                    .map(|&c| inkvec_trace::gradient::FillModel::Flat(c))
                    .collect()
            };
            inkvec_trace::planar::refine_subpixel(
                &mut new_map,
                &rgb,
                &face_model,
                t.sigma_noise,
                false,
            );
            inkvec_trace::planar::refine_junctions(&mut new_map);

            for e in new_map.edges.iter_mut() {
                for (k, p) in e.points.iter_mut().enumerate() {
                    let px = (p.x.max(0.0).round() as usize).min(nw - 1);
                    let py = (p.y.max(0.0).round() as usize).min(nh - 1);
                    if let Some(ref sm) = s_map {
                        let s = sm[py * nw + px] as f64;
                        if let Some(edge_s) = e.sigma.get_mut(k) {
                            *edge_s = s;
                        }
                    }
                    if let Some(ref dt) = dt_map {
                        let idx = (py * nw + px) * 2;
                        if idx + 1 < dt.len() {
                            let dx = dt[idx] as f64;
                            let dy = dt[idx + 1] as f64;
                            p.x += dx.clamp(-1.5, 1.5) * 0.4;
                            p.y += dy.clamp(-1.5, 1.5) * 0.4;
                        }
                    }
                }
            }

            t.map = new_map;
            t.labels = n_labels.clone();
            t.face_rgb = n_colors_vec.clone();
            let oklab_colors: Vec<inkvec_trace::color::Oklab> = n_colors_vec
                .iter()
                .map(|&c| inkvec_trace::color::rgb_to_oklab(c))
                .collect();
            t.palette = inkvec_trace::color::Palette {
                colors: oklab_colors,
                rgb: n_colors_vec.clone(),
                weight: vec![1.0; n_colors],
                alpha: vec![1.0; n_colors],
            };
            t.face_color = (0..n_colors).collect();
            t.face_fill = face_model
                .into_iter()
                .map(|model| {
                    let params = model.params();
                    inkvec_trace::gradient::FillFit {
                        model,
                        chi2: 0.0,
                        params,
                        cost: 1.0,
                    }
                })
                .collect();
        }

        edges_before = t.map.edges.len();
        n_faces = t.map.n_labels;

        // Optionally dump map info
        if let Some(dp) = dump_path {
            use std::io::Write;
            if let Ok(mut f) = std::fs::File::create(dp) {
                let _ = writeln!(
                    f,
                    "{} {} {} {}",
                    t.map.width,
                    t.map.height,
                    t.map.n_labels,
                    t.map.edges.len()
                );
                for (i, rgb) in t.face_rgb.iter().enumerate() {
                    let _ = writeln!(f, "FACE {} {:.4} {:.4} {:.4}", i, rgb[0], rgb[1], rgb[2]);
                }
                let _ = writeln!(f, "LABELS {}", t.labels.len());
                for (idx, &l) in t.labels.iter().enumerate() {
                    let _ = write!(f, "{} ", l);
                    if (idx + 1) % 64 == 0 {
                        let _ = writeln!(f);
                    }
                }
                let _ = writeln!(f);
                for (i, e) in t.map.edges.iter().enumerate() {
                    let _ = writeln!(
                        f,
                        "EDGE {} {} {} {} {}",
                        i,
                        e.left,
                        e.right,
                        e.closed,
                        e.points.len()
                    );
                    for (k, p) in e.points.iter().enumerate() {
                        let s = e.sigma.get(k).copied().unwrap_or(0.1);
                        let _ = writeln!(f, "{:.4} {:.4} {:.4}", p.x, p.y, s);
                    }
                }
            }
            // Label generation needs only the planar map, which exists here, before the
            // curve fit that costs the other half of the run. `INKVEC_DUMP_ONLY` stops now.
            if std::env::var_os("INKVEC_DUMP_ONLY").is_some() {
                std::process::exit(0);
            }
        }

        // 1. Apply face remapping (dissolve internal edges)
        if let Some(ref remap) = remap_table {
            for e in t.map.edges.iter_mut() {
                let l = remap.get(e.left as usize).copied().unwrap_or(e.left);
                let r = remap.get(e.right as usize).copied().unwrap_or(e.right);
                if l == r {
                    e.left = u16::MAX;
                    e.right = u16::MAX;
                } else {
                    e.left = l;
                    e.right = r;
                }
            }
        }

        // 2. Apply calibrated per-edge or per-point sigma
        if let Some(ref es) = edge_sigmas {
            for (ei, e) in t.map.edges.iter_mut().enumerate() {
                if let Some(new_sigmas) = es.get(ei) {
                    for (k, s) in e.sigma.iter_mut().enumerate() {
                        if let Some(&new_s) = new_sigmas.get(k) {
                            *s = new_s;
                        }
                    }
                }
            }
        }

        // 3. Apply global sigma scale
        if (global_sigma_scale - 1.0).abs() > 1e-6 {
            for e in t.map.edges.iter_mut() {
                for s in e.sigma.iter_mut() {
                    *s *= global_sigma_scale;
                }
            }
        }

        edges_active = t
            .map
            .edges
            .iter()
            .filter(|e| (e.left as usize) < t.map.n_labels || (e.right as usize) < t.map.n_labels)
            .count();
    })?;

    std::fs::write(output_path, &svg)?;

    println!(
        "{{\"edges_before\":{},\"edges_active\":{},\"n_labels\":{},\"svg_bytes\":{}}}",
        edges_before,
        edges_active,
        n_faces,
        svg.len()
    );

    Ok(())
}
