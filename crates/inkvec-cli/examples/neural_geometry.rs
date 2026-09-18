//! Research: raster-derived shared boundaries + bounded neural localization proposals.
//! Usage: neural_geometry input.png fields.f32 baseline.svg guided.svg [lossy]
//! fields.f32: exactly 26*H*W little-endian f32, channel-major, native input size.
use inkvec_cli::{trace_color_guided, Args};
use inkvec_trace::{load_image, ColorTrace};
use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() < 5 {
        return Err("input fields.f32 baseline.svg guided.svg [lossy]".into());
    }
    let img = load_image(Path::new(&a[1]))?;
    let (w, h) = (img.width, img.height);
    let bytes = std::fs::read(&a[2])?;
    if bytes.len() != 26 * w * h * 4 {
        return Err("field dimensions do not match image".into());
    }
    let f: Vec<f64> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()) as f64)
        .collect();
    if f.iter().any(|v| !v.is_finite()) {
        return Err("nonfinite fields".into());
    }
    let args = Args {
        lossy: if a.get(5).is_some_and(|s| s == "lossy") {
            inkvec_sr::Mode::On
        } else {
            inkvec_sr::Mode::Off
        },
        ..Args::default()
    };
    let mut saved = None;
    let (baseline, _) = trace_color_guided(&img, &args, |t| {
        saved = Some(t.map.clone());
    })?;
    std::fs::write(&a[3], baseline)?;
    let original = saved.unwrap();
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let mut proposed = 0;
    let mut accepted = 0;
    let mut before_points = Vec::new();
    let mut after_points = Vec::new();
    let (guided, _) = trace_color_guided(&img, &args, |t: &mut ColorTrace| {
        // Determinism check prevents comparing different raster topologies.
        assert_eq!(original.edges.len(), t.map.edges.len());
        let at = |c: usize, x: usize, y: usize| f[c * w * h + y * w + x];
        // Face embeddings identify existing raster regions; they do not create new ones.
        let mut means = vec![[0.0; 16]; t.map.n_labels];
        for (i, &label) in t.labels.iter().enumerate() {
            for c in 0..16 {
                means[label as usize][c] += f[(10 + c) * w * h + i];
            }
        }
        for m in &mut means {
            let norm = m.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-9);
            for v in m {
                *v /= norm;
            }
        }
        let sample = |x: f64, y: f64| -> [f64; 3] {
            let x = x.clamp(0.0, (w - 1) as f64);
            let y = y.clamp(0.0, (h - 1) as f64);
            let (ix, iy) = (x.floor() as usize, y.floor() as usize);
            let (fx, fy) = (x - ix as f64, y - iy as f64);
            let mut c = [0.0; 3];
            for (xx, wx) in [(ix, 1.0 - fx), ((ix + 1).min(w - 1), fx)] {
                for (yy, wy) in [(iy, 1.0 - fy), ((iy + 1).min(h - 1), fy)] {
                    for k in 0..3 {
                        c[k] += rgb[yy * w + xx][k] as f64 * wx * wy;
                    }
                }
            }
            c
        };
        for (ei, e) in t.map.edges.iter_mut().enumerate() {
            let old = &original.edges[ei];
            assert_eq!(old.points, e.points);
            if e.left as usize >= means.len() || e.right as usize >= means.len() {
                continue;
            }
            let n = e.points.len();
            if n < 5 {
                continue;
            }
            for i in 0..n {
                // Junctions stay fixed, so both faces retain exactly shared endpoints.
                if !e.closed && (i < 2 || i + 2 >= n) {
                    continue;
                }
                let p = old.points[i];
                let prev = old.points[(i + n - 1) % n];
                let next = old.points[(i + 1) % n];
                let (ax, ay) = (p.x - prev.x, p.y - prev.y);
                let (bx, by) = (next.x - p.x, next.y - p.y);
                let norm = ((ax * ax + ay * ay) * (bx * bx + by * by)).sqrt();
                if norm < 1e-9 || (ax * bx + ay * by) / norm < 0.866 {
                    continue;
                }
                let (tx, ty) = (next.x - prev.x, next.y - prev.y);
                let len = (tx * tx + ty * ty).sqrt();
                if len < 1e-9 {
                    continue;
                }
                let (tx, ty) = (tx / len, ty / len);
                let (nx, ny) = (-ty, tx);
                let mut best = None;
                let mut best_score = f64::INFINITY;
                for yy in (p.y.floor() as isize - 2)..=(p.y.ceil() as isize + 2) {
                    for xx in (p.x.floor() as isize - 2)..=(p.x.ceil() as isize + 2) {
                        if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize {
                            continue;
                        }
                        let (x, y) = (xx as usize, yy as usize);
                        let label = t.labels[y * w + x];
                        if label != e.left && label != e.right {
                            continue;
                        }
                        let d = at(0, x, y);
                        let (dx, dy) = (at(1, x, y), at(2, x, y));
                        if !(0.0..0.9).contains(&d)
                            || ((dx * dx + dy * dy).sqrt() - 1.5 * d).abs() > 0.25
                        {
                            continue;
                        }
                        if at(3, x, y) > 0.25 || at(4, x, y) > 0.25 {
                            continue;
                        }
                        let affinity = (0..16)
                            .map(|c| at(10 + c, x, y) * means[label as usize][c])
                            .sum::<f64>();
                        if affinity < 0.8 {
                            continue;
                        }
                        let agreement =
                            at(5, x, y) * (tx * tx - ty * ty) + at(6, x, y) * 2.0 * tx * ty;
                        if agreement < 0.5 {
                            continue;
                        }
                        if at(9, x, y) > 0.5 && (at(7, x, y) * nx + at(8, x, y) * ny).abs() < 0.7 {
                            continue;
                        }
                        // Internal tracer pixel centres are integers; external contract
                        // uses x+.5, so only offsets transfer, not the absolute origin.
                        let (qx, qy) = (x as f64 + dx, y as f64 + dy);
                        let dist = ((qx - p.x).powi(2) + (qy - p.y).powi(2)).sqrt();
                        if dist > 1.0 {
                            continue;
                        }
                        let shift = (qx - p.x) * nx + (qy - p.y) * ny;
                        if shift.abs() > 0.5 {
                            continue;
                        }
                        let score = dist + 0.25 * (1.0 - affinity);
                        if score < best_score {
                            best_score = score;
                            best = Some(shift * 0.5);
                        }
                    }
                }
                if let Some(shift) = best {
                    proposed += 1;
                    // Raster remains an independent veto. Compare the two side
                    // samples with their existing inks, allowing either orientation.
                    let residual = |x: f64, y: f64| {
                        let a = sample(x + nx * 0.75, y + ny * 0.75);
                        let b = sample(x - nx * 0.75, y - ny * 0.75);
                        let ca = t.face_rgb[e.left as usize];
                        let cb = t.face_rgb[e.right as usize];
                        let cost = |u: [f64; 3], v: [f64; 3], l: [f32; 3], r: [f32; 3]| {
                            (0..3)
                                .map(|k| {
                                    (u[k] - l[k] as f64).powi(2) + (v[k] - r[k] as f64).powi(2)
                                })
                                .sum::<f64>()
                        };
                        cost(a, b, ca, cb).min(cost(a, b, cb, ca))
                    };
                    let (qx, qy) = (p.x + shift * nx, p.y + shift * ny);
                    if residual(qx, qy) > residual(p.x, p.y) + 1e-6 {
                        continue;
                    }
                    e.points[i].x = qx;
                    e.points[i].y = qy;
                    accepted += 1;
                    before_points.push([p.x + 0.5, p.y + 0.5]);
                    after_points.push([qx + 0.5, qy + 0.5]);
                }
            }
            assert_eq!(old.sigma, e.sigma);
            assert_eq!(
                (old.left, old.right, old.start_node, old.end_node),
                (e.left, e.right, e.start_node, e.end_node)
            );
        }
    })?;
    std::fs::write(&a[4], guided)?;
    std::fs::write(format!("{}.points.json",a[4]),format!("{{\"proposed\":{proposed},\"accepted\":{accepted},\"before\":{before_points:?},\"after\":{after_points:?}}}"))?;
    eprintln!("neural geometry: {accepted}/{proposed} proposals accepted; max displacement 0.25px");
    Ok(())
}
