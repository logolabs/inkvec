//! Measures how far extracted contour points deviate from a true straight
//! edge, rendered as a half-plane at a given angle, to quantify edge-fit bias.
use inkvec_trace::coverage::Rgba;

fn render(w: usize, h: usize, inside: impl Fn(f64, f64) -> bool) -> Rgba {
    const SS: usize = 16;
    let mut data = vec![0.0f32; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f64 - 0.5 + (sx as f64 + 0.5) / SS as f64;
                    let py = y as f64 - 0.5 + (sy as f64 + 0.5) / SS as f64;
                    if inside(px, py) {
                        hits += 1;
                    }
                }
            }
            let a = hits as f32 / (SS * SS) as f32;
            let v = 1.0 - a; // black shape on white
            let i = (y * w + x) * 4;
            data[i] = v;
            data[i + 1] = v;
            data[i + 2] = v;
            data[i + 3] = 1.0;
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

fn main() {
    println!(
        "
  Deviation of extracted contour points from a TRUE straight edge"
    );
    println!(
        "  (half-plane at angle theta; residual to the exact line, in pixels)
"
    );
    println!(
        "  {:>7} {:>10} {:>10} {:>10} {:>12}",
        "angle", "rms", "max", "sigma_rep", "max/sigma"
    );
    for deg in [0.0f64, 10.0, 22.5, 30.0, 45.0, 63.4] {
        let th = deg.to_radians();
        let (nx, ny) = (th.cos(), th.sin());
        let c = 31.5;
        // Half-plane through the canvas centre with normal (nx, ny).
        let img = render(96, 96, |x, y| (x - c) * nx + (y - c) * ny <= 0.0);
        let (polys, field) =
            inkvec_trace::trace_bilevel(&img, &inkvec_trace::TraceOptions { min_area: 1.0 });
        let Some(poly) = polys.into_iter().max_by_key(|p| p.len()) else {
            continue;
        };
        // Keep only points on the interior edge, away from the canvas border.
        let pts: Vec<_> = poly
            .points
            .iter()
            .copied()
            .filter(|p| p.x > 4.0 && p.x < 91.0 && p.y > 4.0 && p.y < 91.0)
            .collect();
        if pts.len() < 10 {
            println!("  {deg:>6.1}  (too few interior points)");
            continue;
        }
        let mut sq = 0.0f64;
        let mut mx = 0.0f64;
        for p in &pts {
            let d = ((p.x - c) * nx + (p.y - c) * ny).abs();
            sq += d * d;
            if d > mx {
                mx = d;
            }
        }
        let rms = (sq / pts.len() as f64).sqrt();
        let sig = field.position_sigma(pts[pts.len() / 2]);
        println!(
            "  {:>6.1}  {:>10.4} {:>10.4} {:>10.4} {:>12.1}",
            deg,
            rms,
            mx,
            sig,
            mx / sig
        );
    }
    println!(
        "
  sigma_rep is what the pipeline currently reports. If max/sigma exceeds tau=2,"
    );
    println!("  the fit is being told the boundary is more certain than it actually is, and");
    println!(
        "  it cannot merge points that genuinely lie on one straight line.
"
    );
}
