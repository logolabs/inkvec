//! The same input must give the same output.
//!
//! It did not. Palette modes were ordered by pixel count with ties left to hash
//! iteration order, and that order decides which colour is accepted first and therefore
//! everything downstream — labels, faces, boundaries. Two runs of one binary on one file
//! produced different SVGs, and a junction-accuracy test that asserted a fixed bound was
//! quietly measuring a random variable: 0.054 to 0.134px across ten consecutive runs,
//! and 0.29px under parallel load.
//!
//! Rust seeds each `HashMap` differently, so building the same structure twice in one
//! process is enough to catch this; a cross-process check is not required.

use inkvec_trace::coverage::Rgba;
use inkvec_trace::{color, trace_color_full, ColorOptions};

/// An image with many equal-area colours, which is the case ties come from.
fn tie_prone(w: usize, h: usize) -> Rgba {
    let palette = [
        [0.85f32, 0.20, 0.20],
        [0.20, 0.75, 0.30],
        [0.25, 0.35, 0.85],
        [0.90, 0.80, 0.20],
        [0.70, 0.30, 0.80],
        [0.20, 0.75, 0.78],
    ];
    let mut data = vec![0.0f32; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            // Equal-sized blocks, so several colours have identical pixel counts.
            let c = palette[((x * 6) / w + (y * 6) / h) % palette.len()];
            let i = (y * w + x) * 4;
            data[i] = c[0];
            data[i + 1] = c[1];
            data[i + 2] = c[2];
            data[i + 3] = 1.0;
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

#[test]
fn palette_extraction_is_deterministic() {
    let img = tie_prone(96, 96);
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let first = color::extract_palette(
        &rgb,
        img.width,
        img.height,
        color::DEFAULT_MERGE_DISTANCE,
        64,
    );
    for _ in 0..8 {
        let again = color::extract_palette(
            &rgb,
            img.width,
            img.height,
            color::DEFAULT_MERGE_DISTANCE,
            64,
        );
        assert_eq!(again.len(), first.len(), "palette size varied between runs");
        for (a, b) in again.colors.iter().zip(&first.colors) {
            assert_eq!(a, b, "palette order or contents varied between runs");
        }
    }
}

#[test]
fn tracing_is_deterministic() {
    let img = tie_prone(96, 96);
    let opts = ColorOptions::default();
    let first = trace_color_full(&img, &opts);

    for _ in 0..5 {
        let again = trace_color_full(&img, &opts);
        assert_eq!(again.labels, first.labels, "labels varied between runs");
        assert_eq!(
            again.face_color, first.face_color,
            "face colours varied between runs"
        );
        assert_eq!(
            again.map.edges.len(),
            first.map.edges.len(),
            "edge count varied between runs"
        );
        for (a, b) in again.map.edges.iter().zip(&first.map.edges) {
            assert_eq!(a.points.len(), b.points.len(), "edge length varied");
            assert_eq!((a.left, a.right), (b.left, b.right), "edge sides varied");
            for (p, q) in a.points.iter().zip(&b.points) {
                assert_eq!((p.x, p.y), (q.x, q.y), "edge geometry varied between runs");
            }
        }
    }
}

/// Speckle removal picks a neighbour by majority; equally common neighbours must not be
/// resolved by hash order.
#[test]
fn despeckle_ties_are_resolved_deterministically() {
    let (w, h) = (64usize, 64usize);
    let mut data = vec![0.0f32; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            // Two halves, and a single stray pixel exactly on the seam so its two
            // neighbours are equally represented.
            let mut c = if x < w / 2 {
                [0.9f32, 0.2, 0.2]
            } else {
                [0.2, 0.3, 0.9]
            };
            if x == w / 2 && y == h / 2 {
                c = [0.2, 0.8, 0.2];
            }
            let i = (y * w + x) * 4;
            data[i] = c[0];
            data[i + 1] = c[1];
            data[i + 2] = c[2];
            data[i + 3] = 1.0;
        }
    }
    let img = Rgba {
        width: w,
        height: h,
        data,
    };
    let first = trace_color_full(&img, &ColorOptions::default());
    for _ in 0..8 {
        let again = trace_color_full(&img, &ColorOptions::default());
        assert_eq!(again.labels, first.labels, "despeckle varied between runs");
    }
}
