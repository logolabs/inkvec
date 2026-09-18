//! Why is a rounded corner fitted with straight chords instead of one cubic?
//!
//! Traced output for a rounded square comes back as three line segments per corner. That
//! is suspicious on its face: three lines and one cubic cost the *same* six parameters, so
//! lambda cannot be deciding it, and a cubic through a quarter arc is accurate to about
//! 0.02% while three chords across an 11px radius leave 0.375px of sagitta. The cubic
//! should win on residual and win again on tangent-break cost, which the chords pay twice.
//!
//! This builds the contour analytically — a rounded rectangle, sampled at one point per
//! pixel of arc length — so the question is put to the fitter alone, with none of the
//! tracer's palette, coverage or sigma estimation in the way.
//!
//! Run: `cargo run --release -p inkvec-fit --example corner_fit`

use inkvec_core::{Point, Polyline};
use inkvec_fit::curves::Segment;
use inkvec_fit::multimodel::optimal_multimodel;
use inkvec_fit::{FitConfig, FittedPath};

/// A rounded rectangle traced anticlockwise, one sample per pixel of arc length.
fn rounded_rect(w: f64, h: f64, r: f64) -> Vec<Point> {
    let mut p = Vec::new();
    let push_line = |p: &mut Vec<Point>, a: Point, b: Point| {
        let n = ((b.x - a.x).hypot(b.y - a.y)).round().max(1.0) as usize;
        for k in 0..n {
            let t = k as f64 / n as f64;
            p.push(Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
        }
    };
    let push_arc = |p: &mut Vec<Point>, cx: f64, cy: f64, a0: f64, a1: f64| {
        let n = (r * (a1 - a0).abs()).round().max(2.0) as usize;
        for k in 0..n {
            let a = a0 + (a1 - a0) * (k as f64 / n as f64);
            p.push(Point::new(cx + r * a.cos(), cy + r * a.sin()));
        }
    };
    use std::f64::consts::FRAC_PI_2 as Q;
    push_line(&mut p, Point::new(r, 0.0), Point::new(w - r, 0.0));
    push_arc(&mut p, w - r, r, -Q, 0.0);
    push_line(&mut p, Point::new(w, r), Point::new(w, h - r));
    push_arc(&mut p, w - r, h - r, 0.0, Q);
    push_line(&mut p, Point::new(w - r, h), Point::new(r, h));
    push_arc(&mut p, r, h - r, Q, 2.0 * Q);
    push_line(&mut p, Point::new(0.0, h - r), Point::new(0.0, r));
    push_arc(&mut p, r, r, 2.0 * Q, 3.0 * Q);
    p
}

/// Which samples lie on a corner arc rather than a straight run.
fn arc_mask(w: f64, h: f64, r: f64, pts: &[Point]) -> Vec<bool> {
    pts.iter()
        .map(|p| {
            let inside_x = p.x > r - 1e-9 && p.x < w - r + 1e-9;
            let inside_y = p.y > r - 1e-9 && p.y < h - r + 1e-9;
            !(inside_x || inside_y)
        })
        .collect()
}

fn describe(path: &FittedPath) -> (usize, usize) {
    let l = path
        .segments
        .iter()
        .filter(|s| matches!(s, Segment::Line(_)))
        .count();
    let c = path
        .segments
        .iter()
        .filter(|s| matches!(s, Segment::Cubic(..)))
        .count();
    (l, c)
}

/// Worst distance from the fitted path back to the measured contour.
fn worst_deviation(path: &FittedPath, pts: &[Point]) -> f64 {
    let mut flat = vec![path.start];
    let mut cur = path.start;
    for s in &path.segments {
        match *s {
            Segment::Line(p) => {
                let n = (cur.dist(p).round().max(1.0)) as usize;
                for k in 1..=n {
                    let t = k as f64 / n as f64;
                    flat.push(Point::new(
                        cur.x + (p.x - cur.x) * t,
                        cur.y + (p.y - cur.y) * t,
                    ));
                }
                cur = p;
            }
            Segment::Cubic(c1, c2, p) => {
                let q = [cur, c1, c2, p];
                for k in 1..=24 {
                    let t = k as f64 / 24.0;
                    let u = 1.0 - t;
                    let b = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
                    flat.push(Point::new(
                        b[0] * q[0].x + b[1] * q[1].x + b[2] * q[2].x + b[3] * q[3].x,
                        b[0] * q[0].y + b[1] * q[1].y + b[2] * q[2].y + b[3] * q[3].y,
                    ));
                }
                cur = p;
            }
            Segment::Arc { end, .. } => {
                flat.push(end);
                cur = end;
            }
        }
    }
    pts.iter()
        .map(|p| {
            flat.iter()
                .map(|q| p.dist(*q))
                .fold(f64::INFINITY, f64::min)
        })
        .fold(0.0f64, f64::max)
}

fn main() {
    let cfg = FitConfig::from_precision(128.0, 0.6, 2.0);
    println!("  lambda = {:.2}\n", cfg.lambda);
    println!(
        "  {:>7}{:>7}{:>7}{:>10}{:>8}{:>8}{:>12}",
        "radius", "flat", "arc", "segments", "lines", "cubics", "worst dev"
    );

    // `curved` marks the samples that lie on an arc, so the sweep can give those points a
    // different sigma from the straight runs — which is what `inflate_for_curvature` does
    // in the tracer, and the thing under suspicion here.
    for r in [6.0, 11.0, 20.0, 32.0] {
        for (sigma, arc_sigma) in [(0.15, 0.15), (0.35, 0.35), (0.10, 0.35), (0.10, 0.45)] {
            let pts = rounded_rect(120.0, 120.0, r);
            let curved = arc_mask(120.0, 120.0, r, &pts);
            let n = pts.len();
            let poly = Polyline {
                points: pts.clone(),
                sigma: (0..n)
                    .map(|i| if curved[i] { arc_sigma } else { sigma })
                    .collect(),
                closed: true,
            };
            let fit = optimal_multimodel(&poly, &cfg);
            let (l, c) = describe(&fit);
            println!(
                "  {:>7.0}{:>8.2}{:>10}{:>8}{:>8}{:>12.3}",
                r,
                sigma,
                fit.segments.len(),
                l,
                c,
                worst_deviation(&fit, &pts)
            );
        }
    }

    println!(
        "\n  A quarter arc wants one cubic per corner: 4 cubics and 4 lines for the whole\n\
         \x20 shape. Chords instead mean the fitter is not seeing the cubic as cheaper,\n\
         \x20 even though three lines and one cubic both cost six parameters."
    );
}
