//! Two-pass `fit_path` against the single global `{line, cubic}` program.
//!
//! Both are scored with the same evaluator — exact nearest distance from every measured
//! point to the emitted geometry, `0.5·chi² + λ·params` — under the same `FitConfig`.
//!
//! Run with:  cargo run --release --example multimodel_demo -p inkvec-fit

use inkvec_core::{Point, Polyline};
use inkvec_fit::curves::Segment;
use inkvec_fit::multimodel::{optimal_multimodel, path_chi2, path_cost, path_max_deviation};
use inkvec_fit::{fit_path, FitConfig, FittedPath};
use std::time::Instant;

fn circle(n: usize, r: f64) -> Vec<Point> {
    (0..n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            Point::new(r * a.cos(), r * a.sin())
        })
        .collect()
}

fn wobble(pts: &[Point], amplitude: f64) -> Vec<Point> {
    let n = pts.len();
    (0..n)
        .map(|k| {
            let d = pts[(k + 1) % n] - pts[(k + n - 1) % n];
            let len = d.norm().max(1e-9);
            let h = ((k as f64 * 12.9898).sin() * 43758.5453).fract() * 2.0 - 1.0;
            Point::new(
                pts[k].x - amplitude * h * d.y / len,
                pts[k].y + amplitude * h * d.x / len,
            )
        })
        .collect()
}

fn rotated_square(side: f64, deg: f64) -> Vec<Point> {
    let per_edge = side.round() as usize;
    let corners = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)];
    let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
    let mut pts = Vec::new();
    for e in 0..4 {
        let (ax, ay) = corners[e];
        let (bx, by) = corners[(e + 1) % 4];
        for k in 0..per_edge {
            let t = k as f64 / per_edge as f64;
            let (x, y) = (side * (ax + t * (bx - ax)), side * (ay + t * (by - ay)));
            pts.push(Point::new(c * x - s * y, s * x + c * y));
        }
    }
    pts
}

fn rounded_rect(w: f64, h: f64, r: f64) -> Vec<Point> {
    let mut pts = Vec::new();
    let (hw, hh) = (0.5 * w, 0.5 * h);
    let centres = [
        (hw - r, -hh + r, -90.0f64),
        (hw - r, hh - r, 0.0),
        (-hw + r, hh - r, 90.0),
        (-hw + r, -hh + r, 180.0),
    ];
    let arc_n = ((std::f64::consts::FRAC_PI_2 * r).round() as usize).max(4);
    for (e, &(cx, cy, a0)) in centres.iter().enumerate() {
        let (px, py, _) = centres[(e + 3) % 4];
        let a = (a0 - 90.0).to_radians();
        let from = Point::new(px + r * a.cos(), py + r * a.sin());
        let to = Point::new(cx + r * a.cos(), cy + r * a.sin());
        let edge_n = (from.dist(to).round() as usize).max(2);
        for k in 0..edge_n {
            let t = k as f64 / edge_n as f64;
            pts.push(Point::new(
                from.x + t * (to.x - from.x),
                from.y + t * (to.y - from.y),
            ));
        }
        for k in 0..arc_n {
            let a = (a0 - 90.0 + 90.0 * k as f64 / arc_n as f64).to_radians();
            pts.push(Point::new(cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    pts
}

fn d_shape(r: f64) -> Vec<Point> {
    let mut pts = Vec::new();
    let edge_n = (2.0 * r).round() as usize;
    for k in 0..edge_n {
        pts.push(Point::new(0.0, -r + 2.0 * r * k as f64 / edge_n as f64));
    }
    let arc_n = (std::f64::consts::PI * r).round() as usize;
    for k in 0..arc_n {
        let a = std::f64::consts::FRAC_PI_2 - std::f64::consts::PI * k as f64 / arc_n as f64;
        pts.push(Point::new(r * a.cos(), r * a.sin()));
    }
    pts
}

fn star(n: usize, points: usize, r0: f64, r1: f64) -> Vec<Point> {
    (0..n)
        .map(|k| {
            let t = k as f64 / n as f64 * std::f64::consts::TAU;
            let r = r0 + (r1 - r0) * 0.5 * (1.0 + (t * points as f64).cos());
            Point::new(r * t.cos(), r * t.sin())
        })
        .collect()
}

fn describe(path: &FittedPath) -> (usize, usize) {
    let lines = path
        .segments
        .iter()
        .filter(|s| matches!(s, Segment::Line(_)))
        .count();
    (lines, path.segments.len() - lines)
}

fn main() {
    let cfg = FitConfig::default();
    let shapes: Vec<(&str, Vec<Point>, f64)> = vec![
        ("circle r46", circle(300, 46.0), 0.05),
        ("noisy circle", wobble(&circle(300, 46.0), 0.05), 0.05),
        ("circle sigma .5", circle(300, 90.0), 0.5),
        ("rotated square", rotated_square(80.0, 23.0), 0.05),
        ("rounded rect", rounded_rect(100.0, 60.0, 12.0), 0.05),
        ("D", d_shape(30.0), 0.05),
        ("star5 smooth", star(400, 5, 30.0, 60.0), 0.05),
        ("noisy star5", wobble(&star(400, 5, 30.0, 60.0), 0.05), 0.05),
        (
            "ellipse 1000",
            {
                (0..1000)
                    .map(|k| {
                        let a = std::f64::consts::TAU * k as f64 / 1000.0;
                        Point::new(120.0 * a.cos(), 50.0 * a.sin())
                    })
                    .collect()
            },
            0.05,
        ),
    ];

    println!("lambda = {:.3}, tau = {}", cfg.lambda, cfg.tau);
    println!();
    println!(
        "| {:<16} | {:>4} | {:<10} | {:>5} | {:>5} | {:>9} | {:>8} | {:>7} | {:>8} |",
        "shape", "n", "method", "lines", "cubic", "chi2", "cost", "maxdev", "time"
    );
    println!(
        "|{}|{}|{}|{}|{}|{}|{}|{}|{}|",
        "-".repeat(18),
        "-".repeat(6),
        "-".repeat(12),
        "-".repeat(7),
        "-".repeat(7),
        "-".repeat(11),
        "-".repeat(10),
        "-".repeat(9),
        "-".repeat(10)
    );
    for (name, pts, sigma) in shapes {
        let n = pts.len();
        let poly = Polyline::with_uniform_sigma(pts, sigma, true);
        let t = Instant::now();
        let fp = fit_path(&poly, &cfg);
        let t_fp = t.elapsed();
        let t = Instant::now();
        let mm = optimal_multimodel(&poly, &cfg);
        let t_mm = t.elapsed();
        for (method, path, dt) in [("fit_path", &fp, t_fp), ("multimodel", &mm, t_mm)] {
            let (l, c) = describe(path);
            println!(
                "| {:<16} | {:>4} | {:<10} | {:>5} | {:>5} | {:>9.1} | {:>8.1} | {:>7.3} | {:>6.1}ms |",
                name,
                n,
                method,
                l,
                c,
                path_chi2(&poly, path),
                path_cost(&poly, path, &cfg),
                path_max_deviation(&poly, path),
                dt.as_secs_f64() * 1e3
            );
        }
    }
}
