//! How much does the primitive alphabet buy?
//!
//! For each synthetic boundary (0.05px noise, sigma 0.05) this prints the parameter count
//! and MDL cost of three descriptions of the same points:
//!
//!   lines      — the optimal polygon (`optimal_polygon`), lines only
//!   cubics     — `fit_path`, lines and cubics
//!   primitive  — `fit_primitive_or_arcs`: circle / ellipse / rounded rect / arcs
//!
//! All three are costed identically: `0.5 * chi2 + lambda * params`, chi2 measured from
//! every point to the geometry that would actually be emitted, `params` including the
//! path start point where there is one.
//!
//! Run with:  cargo run --release --example primitives_demo -p inkvec-fit

use inkvec_core::{Point, Polyline};
use inkvec_fit::curves::{self, Segment};
use inkvec_fit::primitives::{fit_primitive_or_arcs, PrimitiveKind};
use inkvec_fit::{fit_path, optimal_polygon, FitConfig, PARAMS_LINE};
use std::f64::consts::{PI, TAU};

struct Rng(u64);

impl Rng {
    fn uniform(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    fn gaussian(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-300);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

const SIGMA: f64 = 0.05;

fn sample(n: usize, t1: f64, seed: u64, curve: impl Fn(f64) -> Point) -> Vec<Point> {
    let mut rng = Rng(seed | 1);
    (0..n)
        .map(|k| {
            let t = t1 * k as f64 / n as f64;
            let p = curve(t);
            let (a, b) = (curve(t - 1e-4), curve(t + 1e-4));
            let tan = b - a;
            let l = tan.norm().max(1e-12);
            let d = SIGMA * rng.gaussian();
            Point::new(p.x - tan.y / l * d, p.y + tan.x / l * d)
        })
        .collect()
}

fn round_rect(n: usize, x: f64, y: f64, w: f64, h: f64, rx: f64, seed: u64) -> Vec<Point> {
    let (sw, sh, arc) = (w - 2.0 * rx, h - 2.0 * rx, rx * PI / 2.0);
    let total = 2.0 * (sw + sh) + 4.0 * arc;
    let corner = |cx: f64, cy: f64, a: f64| Point::new(cx + rx * a.cos(), cy + rx * a.sin());
    sample(n, total, seed, move |s| {
        let mut s = s.rem_euclid(total);
        let pieces = [sw, arc, sh, arc, sw, arc, sh, arc];
        for (i, len) in pieces.iter().enumerate() {
            if s <= *len {
                let u = if *len > 0.0 { s / len } else { 0.0 };
                let q = PI / 2.0;
                return match i {
                    0 => Point::new(x + rx + sw * u, y + h),
                    1 => corner(x + w - rx, y + h - rx, q - u * q),
                    2 => Point::new(x + w, y + h - rx - sh * u),
                    3 => corner(x + w - rx, y + rx, -u * q),
                    4 => Point::new(x + w - rx - sw * u, y),
                    5 => corner(x + rx, y + rx, -q - u * q),
                    6 => Point::new(x, y + rx + sh * u),
                    _ => corner(x + rx, y + h - rx, PI - u * q),
                };
            }
            s -= len;
        }
        Point::new(x + rx, y + h)
    })
}

fn describe(kind: &PrimitiveKind) -> String {
    match *kind {
        PrimitiveKind::Circle { c, r } => format!("circle c=({:.2},{:.2}) r={:.3}", c.x, c.y, r),
        PrimitiveKind::Ellipse { c, rx, ry, angle } => format!(
            "ellipse c=({:.2},{:.2}) rx={:.3} ry={:.3} angle={:.1}deg",
            c.x,
            c.y,
            rx,
            ry,
            angle.to_degrees()
        ),
        PrimitiveKind::RoundRect { x, y, w, h, rx } => {
            format!("rect x={x:.2} y={y:.2} w={w:.2} h={h:.2} rx={rx:.3}")
        }
    }
}

fn main() {
    let cfg = FitConfig::default();
    let shapes: Vec<(&str, Vec<Point>)> = vec![
        (
            "circle r=40",
            sample(320, TAU, 1, |t| {
                Point::new(100.0 + 40.0 * t.cos(), 100.0 + 40.0 * t.sin())
            }),
        ),
        (
            "ellipse 50x30 @25deg",
            sample(360, TAU, 2, |t| {
                let (s, c) = 25f64.to_radians().sin_cos();
                let (x, y) = (50.0 * t.cos(), 30.0 * t.sin());
                Point::new(100.0 + c * x - s * y, 100.0 + s * x + c * y)
            }),
        ),
        (
            "roundrect 90x56 rx=12",
            round_rect(400, 15.0, 22.0, 90.0, 56.0, 12.0, 3),
        ),
        ("square 80", round_rect(320, 20.0, 30.0, 80.0, 80.0, 0.0, 4)),
    ];

    println!(
        "lambda = {:.2} nats/param, sigma = {SIGMA}px, cost = 0.5*chi2 + lambda*params\n",
        cfg.lambda
    );
    println!(
        "{:<24} {:>5} | {:>13} | {:>13} | {:>13} | primitive",
        "shape", "pts", "lines", "cubics", "primitive"
    );
    println!(
        "{:<24} {:>5} | {:>6} {:>6} | {:>6} {:>6} | {:>6} {:>6} |",
        "", "", "params", "cost", "params", "cost", "params", "cost"
    );
    println!("{}", "-".repeat(100));

    for (name, pts) in &shapes {
        let n = pts.len();
        let sigma = vec![SIGMA; n];
        let poly = Polyline::with_uniform_sigma(pts.clone(), SIGMA, true);

        // Lines only: cost the emitted polygon the same way as everything else.
        let seg = optimal_polygon(&poly, &cfg);
        let verts: Vec<Point> = seg.vertices.iter().map(|&i| pts[i]).collect();
        let line_segs: Vec<Segment> = verts[1..].iter().map(|p| Segment::Line(*p)).collect();
        let run: Vec<Point> = {
            let cut = seg.vertices[0];
            (0..=n).map(|k| pts[(cut + k) % n]).collect()
        };
        let run_sigma = vec![SIGMA; run.len()];
        let line_chi2 = curves::chi2(&run, &run_sigma, verts[0], &line_segs);
        let line_params = 2.0 + PARAMS_LINE * line_segs.len() as f64;
        let line_cost = 0.5 * line_chi2 + cfg.lambda * line_params;

        // Cubics. `fit_path` starts a closed path at its own cut point, and `chi2` walks
        // points and path together, so score against the points rotated to that start.
        let path = fit_path(&poly, &cfg);
        let cut = (0..n)
            .min_by(|&a, &b| pts[a].dist(path.start).total_cmp(&pts[b].dist(path.start)))
            .unwrap();
        let run: Vec<Point> = (0..=n).map(|k| pts[(cut + k) % n]).collect();
        let cubic_chi2 = curves::chi2(&run, &run_sigma, path.start, &path.segments);
        let cubic_params = path.params();
        let cubic_cost = 0.5 * cubic_chi2 + cfg.lambda * cubic_params;

        // Primitive.
        let prim = fit_primitive_or_arcs(pts, &sigma, true, &cfg);
        let (prim_params, prim_cost, label) = match &prim {
            Some((segs, Some(p), cost)) => (
                p.params,
                *cost,
                format!("{} | path form: {} segs", describe(&p.kind), segs.len()),
            ),
            Some((segs, None, cost)) => (
                segs.iter().map(|s| s.params()).sum::<f64>(),
                *cost,
                format!("arcs: {} segs", segs.len()),
            ),
            None => (f64::NAN, f64::NAN, "none (lines/cubics win)".to_string()),
        };

        println!(
            "{:<24} {:>5} | {:>6.0} {:>6.0} | {:>6.0} {:>6.0} | {:>6.0} {:>6.0} | {}",
            name,
            n,
            line_params,
            line_cost,
            cubic_params,
            cubic_cost,
            prim_params,
            prim_cost,
            label
        );
    }
}
