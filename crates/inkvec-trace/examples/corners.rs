//! Traces a hexagon and prints the contour window around its sharpest corner,
//! with per-point turn angle and `sigma`, to check corner handling by eye.
fn main() {
    let img = inkvec_trace::load_image(std::path::Path::new(
        "bench/data/corpus_raster/synthetic/128/prim_hex.png",
    ))
    .unwrap();
    let (polys, _) = inkvec_trace::trace_bilevel(&img, &inkvec_trace::TraceOptions::default());
    let p = &polys[0];
    let n = p.len();
    // Find the point of sharpest turn (a corner) and print a window around it.
    let turn = |k: usize| -> f64 {
        let a = p.points[k] - p.points[(k + n - 1) % n];
        let b = p.points[(k + 1) % n] - p.points[k];
        let (na, nb) = (a.norm(), b.norm());
        if na < 1e-9 || nb < 1e-9 {
            return 0.0;
        }
        (a.dot(b) / (na * nb)).clamp(-1.0, 1.0).acos().to_degrees()
    };
    let mut best = (0usize, 0.0f64);
    for k in 0..n {
        let t = turn(k);
        if t > best.1 {
            best = (k, t);
        }
    }
    println!(
        "\n  contour points: {n}   sharpest turn {:.1} deg at index {}",
        best.1, best.0
    );
    println!(
        "  sigma: min {:.4}  max {:.4}  mean {:.4}",
        p.sigma.iter().cloned().fold(f64::MAX, f64::min),
        p.sigma.iter().cloned().fold(0.0, f64::max),
        p.sigma.iter().sum::<f64>() / n as f64
    );
    println!(
        "\n  {:>6} {:>9} {:>9} {:>9} {:>10}",
        "idx", "x", "y", "turn(deg)", "sigma"
    );
    let c = best.0;
    for d in -4i64..=4 {
        let k = ((c as i64 + d).rem_euclid(n as i64)) as usize;
        println!(
            "  {:>6} {:>9.3} {:>9.3} {:>9.1} {:>10.4}",
            k,
            p.points[k].x,
            p.points[k].y,
            turn(k),
            p.sigma[k]
        );
    }
    println!();
}
