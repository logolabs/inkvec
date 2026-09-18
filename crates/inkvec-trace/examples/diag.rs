//! Compares two ways of measuring area from a coverage field on strokes of
//! known width, by rendering synthetic shapes at high supersampling.
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
  Two ways to measure area from a coverage field, on strokes of known width.
"
    );
    println!(
        "  {:>5}  {:>9}  {:>22}  {:>22}",
        "width", "truth", "coverage integral", "level-set polygon"
    );
    for w in [0.3f64, 0.6, 1.0, 2.0, 4.0, 8.0] {
        let img = render(64, 64, |x, y| {
            (10.0..54.0).contains(&y) && x >= 20.0 && x < 20.0 + w
        });
        let field = inkvec_trace::coverage::bilevel_coverage(&img);
        let integral: f64 = field.data.iter().map(|&a| a as f64).sum();
        let (polys, _) =
            inkvec_trace::trace_bilevel(&img, &inkvec_trace::TraceOptions { min_area: 0.02 });
        let poly_area: f64 = polys
            .iter()
            .map(|p| inkvec_trace::contour::signed_area(p).abs())
            .sum();
        let truth = w * 44.0;
        println!(
            "  {:>5.1}  {:>9.2}  {:>13.2} {:>+7.1}%  {:>13.2} {:>+7.1}%",
            w,
            truth,
            integral,
            (integral / truth - 1.0) * 100.0,
            poly_area,
            (poly_area / truth - 1.0) * 100.0
        );
    }
    println!(
        "
  Resolvedness (fraction of covered pixels that are strictly interior):
"
    );
    for w in [0.3f64, 0.6, 1.0, 1.5, 2.0, 3.0, 4.0, 8.0] {
        let img = render(64, 64, |x, y| {
            (10.0..54.0).contains(&y) && x >= 20.0 && x < 20.0 + w
        });
        let f = inkvec_trace::coverage::bilevel_coverage(&img);
        println!(
            "  w={:4.1}  saturation={:.3}  sigma_alpha={:.5}",
            w, f.saturation, f.sigma_alpha
        );
    }

    println!(
        "
  Estimated foreground/background per case (truth: fg=0.000, bg=1.000):
"
    );
    for w in [0.3f64, 0.6, 1.0, 2.0, 4.0] {
        let img = render(64, 64, |x, y| {
            (10.0..54.0).contains(&y) && x >= 20.0 && x < 20.0 + w
        });
        let rgb = img.composited([1.0, 1.0, 1.0]);
        let lum: Vec<f32> = rgb
            .iter()
            .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
            .collect();
        let mut s2 = lum.clone();
        s2.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "  w={:4.1}  min={:.3}  p0.2%={:.3}  p2%={:.3}  median={:.3}",
            w,
            s2[0],
            s2[(s2.len() as f64 * 0.002) as usize],
            s2[(s2.len() as f64 * 0.02) as usize],
            s2[s2.len() / 2]
        );
    }
    println!(
        "
  A stroke narrower than a pixel never produces a fully-covered pixel, so the"
    );
    println!("  darkest observation is not the foreground colour. Percentile estimation then");
    println!("  reports a foreground far lighter than the truth, and every coverage value");
    println!(
        "  computed against it is inflated.
"
    );
}
