//! Smoke test for [`inkvec_fit::curves::fit_cubics`] against clean and noisy circles.
use inkvec_core::Point;
fn main() {
    // A perfect circle, densely sampled, no noise. If kurbo cannot fit this in a handful
    // of cubics through our ParamCurveFit, the wrapper is wrong, not the input.
    for n in [64usize, 256, 880] {
        let pts: Vec<Point> = (0..=n)
            .map(|k| {
                let a = std::f64::consts::TAU * k as f64 / n as f64;
                Point::new(46.0 * a.cos(), 46.0 * a.sin())
            })
            .collect();
        print!("clean circle, {n:4} samples: ");
        for tol in [0.05f64, 0.1, 0.5, 2.0] {
            let segs = inkvec_fit::curves::fit_cubics(&pts, tol);
            print!("tol{tol:<4}={:<4} ", segs.map(|s| s.len()).unwrap_or(0));
        }
        println!();
    }
    // Same circle with realistic extraction wobble.
    let n = 880;
    let pts: Vec<Point> = (0..=n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            let r = 46.0 + 0.05 * ((k as f64) * 2.399).sin();
            Point::new(r * a.cos(), r * a.sin())
        })
        .collect();
    print!("noisy circle (0.05px):     ");
    for tol in [0.05f64, 0.1, 0.5, 2.0] {
        let segs = inkvec_fit::curves::fit_cubics(&pts, tol);
        print!("tol{tol:<4}={:<4} ", segs.map(|s| s.len()).unwrap_or(0));
    }
    println!();
}
