//! Demonstration and benchmark of structural simplification in Rust.
//!
//! Compares segment count and runtime before and after structural simplification.
//!
//! Run: `cargo run --release -p inkvec-fit --example structural_demo`

use inkvec_core::Point;
use inkvec_fit::curves::{eval_cubic, Segment};
use inkvec_fit::structural::{simplify_path_structural, StructuralConfig};
use inkvec_fit::FittedPath;
use std::time::Instant;

fn build_circle_chords(r: f64, num_chords: usize) -> FittedPath {
    let mut segs = Vec::new();
    for i in 1..=num_chords {
        let theta = (i as f64 / num_chords as f64) * std::f64::consts::TAU;
        segs.push(Segment::Line(Point::new(r * theta.cos(), r * theta.sin())));
    }
    FittedPath {
        start: Point::new(r, 0.0),
        segments: segs,
        closed: true,
    }
}

fn build_rounded_rect_chords(w: f64, h: f64, r: f64, chords_per_corner: usize) -> FittedPath {
    let mut segs = Vec::new();
    use std::f64::consts::FRAC_PI_2 as Q;

    // Top straight
    segs.push(Segment::Line(Point::new(w - r, 0.0)));
    // Top-right corner
    for i in 1..=chords_per_corner {
        let a = -Q + Q * (i as f64 / chords_per_corner as f64);
        segs.push(Segment::Line(Point::new(
            w - r + r * a.cos(),
            r + r * a.sin(),
        )));
    }
    // Right straight
    segs.push(Segment::Line(Point::new(w, h - r)));
    // Bottom-right corner
    for i in 1..=chords_per_corner {
        let a = Q * (i as f64 / chords_per_corner as f64);
        segs.push(Segment::Line(Point::new(
            w - r + r * a.cos(),
            h - r + r * a.sin(),
        )));
    }
    // Bottom straight
    segs.push(Segment::Line(Point::new(r, h)));
    // Bottom-left corner
    for i in 1..=chords_per_corner {
        let a = Q + Q * (i as f64 / chords_per_corner as f64);
        segs.push(Segment::Line(Point::new(
            r + r * a.cos(),
            h - r + r * a.sin(),
        )));
    }
    // Left straight
    segs.push(Segment::Line(Point::new(0.0, r)));
    // Top-left corner
    for i in 1..=chords_per_corner {
        let a = 2.0 * Q + Q * (i as f64 / chords_per_corner as f64);
        segs.push(Segment::Line(Point::new(r + r * a.cos(), r + r * a.sin())));
    }

    FittedPath {
        start: Point::new(r, 0.0),
        segments: segs,
        closed: true,
    }
}

fn build_s_curve_chords(steps: usize) -> FittedPath {
    let p0 = Point::new(0.0, 0.0);
    let p1 = Point::new(0.0, 50.0);
    let p2 = Point::new(100.0, 50.0);
    let p3 = Point::new(100.0, 100.0);

    let mut segs = Vec::new();
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        segs.push(Segment::Line(eval_cubic([p0, p1, p2, p3], t)));
    }
    FittedPath {
        start: p0,
        segments: segs,
        closed: false,
    }
}

fn main() {
    println!("=== Inkvec Native Rust Structural Simplification ===");
    println!();

    let cfg = StructuralConfig {
        max_dist: 0.6,
        lambda: 1.0,
        ..Default::default()
    };

    // Test 1: Circle with 64 chords
    {
        let mut path = build_circle_chords(64.0, 64);
        let before_segs = path.segments.len();
        let before_params = path.params();

        let t0 = Instant::now();
        let iters = 50;
        for _ in 0..iters {
            let mut clone = path.clone();
            let _ = simplify_path_structural(&mut clone, &cfg);
        }
        let elapsed = t0.elapsed();
        let per_op_us = elapsed.as_secs_f64() * 1e6 / iters as f64;

        let elim = simplify_path_structural(&mut path, &cfg);
        println!("Test 1: Circle (r=64, 64 chords)");
        println!(
            "  Segments:   {} -> {} (eliminated {})",
            before_segs,
            path.segments.len(),
            elim
        );
        println!(
            "  Parameters: {:.1} -> {:.1} (saved {:.1})",
            before_params,
            path.params(),
            before_params - path.params()
        );
        println!(
            "  Time/path:  {:.2} us ({:.3} ms)",
            per_op_us,
            per_op_us / 1000.0
        );
        println!();
    }

    // Test 2: Rounded Rectangle (32 segments)
    {
        let mut path = build_rounded_rect_chords(128.0, 128.0, 24.0, 8);
        let before_segs = path.segments.len();
        let before_params = path.params();

        let t0 = Instant::now();
        let iters = 50;
        for _ in 0..iters {
            let mut clone = path.clone();
            let _ = simplify_path_structural(&mut clone, &cfg);
        }
        let elapsed = t0.elapsed();
        let per_op_us = elapsed.as_secs_f64() * 1e6 / iters as f64;

        let elim = simplify_path_structural(&mut path, &cfg);
        println!("Test 2: Rounded Rectangle (128x128, r=24, 36 chords)");
        println!(
            "  Segments:   {} -> {} (eliminated {})",
            before_segs,
            path.segments.len(),
            elim
        );
        println!(
            "  Parameters: {:.1} -> {:.1} (saved {:.1})",
            before_params,
            path.params(),
            before_params - path.params()
        );
        println!(
            "  Time/path:  {:.2} us ({:.3} ms)",
            per_op_us,
            per_op_us / 1000.0
        );
        println!();
    }

    // Test 3: S-Curve (24 chord steps)
    {
        let mut path = build_s_curve_chords(24);
        let before_segs = path.segments.len();
        let before_params = path.params();

        let t0 = Instant::now();
        let iters = 50;
        for _ in 0..iters {
            let mut clone = path.clone();
            let _ = simplify_path_structural(&mut clone, &cfg);
        }
        let elapsed = t0.elapsed();
        let per_op_us = elapsed.as_secs_f64() * 1e6 / iters as f64;

        let elim = simplify_path_structural(&mut path, &cfg);
        println!("Test 3: Cubic S-Curve (24 chords)");
        println!(
            "  Segments:   {} -> {} (eliminated {})",
            before_segs,
            path.segments.len(),
            elim
        );
        println!(
            "  Parameters: {:.1} -> {:.1} (saved {:.1})",
            before_params,
            path.params(),
            before_params - path.params()
        );
        println!(
            "  Time/path:  {:.2} us ({:.3} ms)",
            per_op_us,
            per_op_us / 1000.0
        );
        println!();
    }
}
