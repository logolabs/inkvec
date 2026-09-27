//! Fast mode must not invent colour: every face's fill comes from its own pixels.
//!
//! Black text beside a red mark came out with red rims on the glyphs, and at small sizes
//! as red text: the grey anti-aliased rim of a black glyph on white paper lies nearer the
//! red (in OKLab) than either black or white, and fast mode's labelling gave a blend its
//! nearest ink whatever it was; and text too thin for a flat pixel never had a black ink
//! at all, so all of it took the nearest one there was.
//!
//! The check is on the traced faces: a face none of whose pixels is near its fill (OKLab)
//! or a blend of its fill and another ink (sRGB) is paint the image does not have there.
//! A small black face over the grey pixels of thin text is a coverage call and passes; a
//! red face over them does not.

use inkvec_trace::color::rgb_to_oklab;
use inkvec_trace::{trace_color_full, ColorOptions, ColorTrace, Rgba};

/// OKLab distance from every pixel of the face beyond which its fill is invented.
const FAR: f32 = 0.12;

const PAPER: [f32; 3] = [0.99, 0.98, 0.97];
const INK: [f32; 3] = [0.04, 0.04, 0.04];
const RED: [f32; 3] = [0.94, 0.14, 0.12];

/// Signed coverage tests for the shapes, in units of the 96 px design grid.
enum Shape {
    /// A segment from (x0, y0) to (x1, y1), `w` wide: a stroke of a glyph.
    Stroke(f32, f32, f32, f32, f32),
    /// A ring at (cx, cy), radius r, `w` wide: an "o".
    Ring(f32, f32, f32, f32),
    /// A filled triangle.
    Tri([(f32, f32); 3]),
}

impl Shape {
    fn covers(&self, x: f32, y: f32) -> bool {
        match *self {
            Shape::Stroke(x0, y0, x1, y1, w) => {
                let (dx, dy) = (x1 - x0, y1 - y0);
                let t = (((x - x0) * dx + (y - y0) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                (x - x0 - t * dx).hypot(y - y0 - t * dy) <= w / 2.0
            }
            Shape::Ring(cx, cy, r, w) => ((x - cx).hypot(y - cy) - r).abs() <= w / 2.0,
            Shape::Tri(p) => {
                let s = |a: (f32, f32), b: (f32, f32)| {
                    (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)
                };
                let (a, b, c) = (s(p[0], p[1]), s(p[1], p[2]), s(p[2], p[0]));
                (a >= 0.0 && b >= 0.0 && c >= 0.0) || (a <= 0.0 && b <= 0.0 && c <= 0.0)
            }
        }
    }
}

/// Lettering-like black strokes of several widths at sub-pixel offsets, and a red
/// triangle, drawn with 8x8 supersampled coverage at `scale` times a 96 px grid.
fn black_text_and_red(scale: usize) -> Rgba {
    let mut black = Vec::new();
    for (row, w) in [(10.0f32, 2.6f32), (30.0, 1.7), (46.0, 1.2)] {
        let h = 10.0 * w / 2.6;
        for k in 0..5 {
            let x = 6.0 + k as f32 * (w * 4.0 + 1.3) + 0.37 * k as f32;
            black.push(Shape::Stroke(x, row, x, row + h, w));
            black.push(Shape::Stroke(x, row + h, x + w * 2.5, row + h, w));
            black.push(Shape::Ring(x + w * 2.0, row + h / 2.0, w * 1.4, w * 0.8));
            black.push(Shape::Stroke(x + w * 3.2, row, x + w * 1.4, row + h, w));
        }
    }
    black.push(Shape::Stroke(4.0, 80.3, 92.0, 80.3, 1.0));
    raster(scale, &black)
}

/// Separate black strokes 1.2 to 1.9 px wide -- vertical, horizontal and slanted, none
/// touching another, so no black pixel anywhere is flat -- and the red triangle.
fn thin_strokes_and_red() -> Rgba {
    let mut black = Vec::new();
    for k in 0..12 {
        let w = 1.2 + 0.7 * (k % 4) as f32 / 3.0;
        let x = 4.0 + k as f32 * 3.7 + 0.29 * k as f32;
        black.push(Shape::Stroke(x, 6.0, x, 30.0, w));
        black.push(Shape::Stroke(x - 1.0, 36.0, x + 1.2, 58.0, w));
    }
    for k in 0..4 {
        let (y, w) = (64.0 + k as f32 * 4.3, 1.2 + 0.2 * k as f32);
        black.push(Shape::Stroke(4.0, y, 44.0, y, w));
    }
    raster(1, &black)
}

/// `black` shapes in ink and a red triangle on paper, with 8x8 supersampled coverage, at
/// `scale` times the 96 px design grid.
fn raster(scale: usize, black: &[Shape]) -> Rgba {
    let red = Shape::Tri([(58.0, 8.0), (92.0, 72.0), (50.0, 70.0)]);
    let n = 96 * scale;
    let ss = 8;
    let mut data = Vec::with_capacity(n * n * 4);
    for py in 0..n {
        for px in 0..n {
            let (mut kb, mut kr) = (0u32, 0u32);
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = (px as f32 + (sx as f32 + 0.5) / ss as f32) / scale as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / ss as f32) / scale as f32;
                    if red.covers(x, y) {
                        kr += 1;
                    } else if black.iter().any(|s| s.covers(x, y)) {
                        kb += 1;
                    }
                }
            }
            let (b, r) = (kb as f32 / 64.0, kr as f32 / 64.0);
            for c in 0..3 {
                data.push(PAPER[c] * (1.0 - b - r) + INK[c] * b + RED[c] * r);
            }
            data.push(1.0);
        }
    }
    Rgba {
        width: n,
        height: n,
        data,
    }
}

/// sRGB distance from the line between two inks within which a pixel is their blend
/// (generous: the masthead is a noisy scan-like render).
const BLEND: f32 = 0.08;

/// Squared sRGB distance from `c` to the segment between `a` and `b`.
fn to_chord(c: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let ab: [f32; 3] = std::array::from_fn(|k| b[k] - a[k]);
    let l2: f32 = ab.iter().map(|v| v * v).sum();
    let t = if l2 < 1e-9 {
        0.0
    } else {
        ((0..3).map(|k| (c[k] - a[k]) * ab[k]).sum::<f32>() / l2).clamp(0.0, 1.0)
    };
    (0..3).map(|k| (c[k] - a[k] - t * ab[k]).powi(2)).sum()
}

/// Faces none of whose pixels is within [`FAR`] of the fill or a blend of the fill and a
/// palette ink: (face, fill, its nearest pixel's OKLab distance, pixel count).
fn invented(img: &Rgba, t: &ColorTrace) -> Vec<(usize, [f32; 3], f32, usize)> {
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let n_faces = t.face_fill.len();
    let mut near = vec![f32::INFINITY; n_faces];
    let mut explained = vec![false; n_faces];
    let mut count = vec![0usize; n_faces];
    let fills: Vec<[f32; 3]> = t
        .face_fill
        .iter()
        .map(|f| f.model.representative())
        .collect();
    for (p, &f) in t.labels.iter().enumerate() {
        let f = f as usize;
        if f >= n_faces {
            continue;
        }
        count[f] += 1;
        let d = rgb_to_oklab(fills[f]).dist(rgb_to_oklab(rgb[p]));
        near[f] = near[f].min(d);
        explained[f] |= d <= FAR
            || t.palette
                .rgb
                .iter()
                .any(|&ink| to_chord(rgb[p], fills[f], ink) <= BLEND * BLEND);
    }
    (0..n_faces)
        .filter(|&f| count[f] > 0 && !explained[f])
        .map(|f| (f, fills[f], near[f], count[f]))
        .collect()
}

fn fast() -> ColorOptions {
    ColorOptions {
        fast: true,
        ..ColorOptions::default()
    }
}

#[test]
fn black_text_beside_red_gets_no_invented_fill() {
    for scale in [2, 4] {
        let img = black_text_and_red(scale);
        let t = trace_color_full(&img, &fast());
        let bad = invented(&img, &t);
        assert!(
            bad.is_empty(),
            "scale {scale}: faces with invented fills {bad:?}"
        );
    }
}

#[test]
fn text_too_thin_for_a_flat_pixel_still_gets_its_black() {
    // Strokes under two pixels wide have no flat pixel: the black must still be an ink,
    // and no dark grey pixel may be filled red.
    let img = thin_strokes_and_red();
    let t = trace_color_full(&img, &fast());
    let rgb = img.composited([1.0, 1.0, 1.0]);
    assert!(
        t.palette.rgb.iter().any(|c| c.iter().all(|&v| v < 0.15)),
        "no black ink: {:?}",
        t.palette.rgb
    );
    let red = rgb_to_oklab(RED);
    let reddened = t
        .labels
        .iter()
        .zip(&rgb)
        .filter(|&(&f, &c)| {
            let fill = rgb_to_oklab(t.face_fill[f as usize].model.representative());
            let src = rgb_to_oklab(c);
            fill.dist(red) < 0.1 && src.l < 0.5 && (src.a * src.a + src.b * src.b).sqrt() < 0.05
        })
        .count();
    assert_eq!(reddened, 0, "dark grey pixels filled red");
}

#[test]
fn the_masthead_crop_gets_no_invented_fill() {
    // A 300 px crop of the LogoLabs "AGATE" masthead (black serif and sans lettering, a
    // hairline and the foot of a big red A) where the bug was reported.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("masthead_black_text_red_a.png");
    let img = inkvec_trace::load_image(&path).expect("fixture");
    let t = trace_color_full(&img, &fast());
    let bad = invented(&img, &t);
    assert!(bad.is_empty(), "faces with invented fills {bad:?}");
}
