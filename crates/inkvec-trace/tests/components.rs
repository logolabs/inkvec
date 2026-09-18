//! Faces must be connected regions, not "everywhere this colour appears".
//!
//! Keying a face by palette index alone merges every disconnected shape of the same
//! colour into one face. That is wrong twice over. Structurally, two separate shapes are
//! two objects and an editor should be able to select and move one of them. And
//! numerically, any per-face measurement is then taken over a region that is not one
//! region: concentric rings quantised into a handful of palette entries put several
//! non-adjacent annuli of slightly different colour into a single "region", whose flat
//! residual came out a thousand times larger than pixel noise could explain, so the fill
//! fitter explained the difference as a radial gradient. The gradient was a symptom.

use inkvec_trace::coverage::Rgba;
use inkvec_trace::{trace_color_full, ColorOptions};

/// Paint an image from a closure returning sRGB per pixel, with hard edges.
fn paint(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3]) -> Rgba {
    let mut data = vec![0.0f32; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let c = f(x, y);
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

const RED: [f32; 3] = [0.85, 0.15, 0.15];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

#[test]
fn two_separate_shapes_of_one_colour_are_two_faces() {
    // Two disjoint red squares on white. One palette entry for red, but two objects.
    let img = paint(96, 48, |x, y| {
        let in_a = (8..32).contains(&x) && (8..40).contains(&y);
        let in_b = (64..88).contains(&x) && (8..40).contains(&y);
        if in_a || in_b {
            RED
        } else {
            WHITE
        }
    });
    let t = trace_color_full(&img, &ColorOptions::default());

    assert_eq!(t.palette.len(), 2, "two inks");
    assert_eq!(
        t.face_color.len(),
        3,
        "background plus two separate red squares = three faces"
    );

    let reds = t
        .face_color
        .iter()
        .filter(|&&ci| {
            let c = t.palette.rgb[ci];
            c[0] > 0.5 && c[1] < 0.5
        })
        .count();
    assert_eq!(reds, 2, "both squares must map back to the red ink");
}

#[test]
fn face_ids_partition_the_image() {
    let img = paint(64, 64, |x, y| {
        if (x / 16 + y / 16).is_multiple_of(2) {
            RED
        } else {
            WHITE
        }
    });
    let t = trace_color_full(&img, &ColorOptions::default());

    assert_eq!(t.labels.len(), 64 * 64, "one face id per pixel");
    let max = *t.labels.iter().max().unwrap() as usize;
    assert!(
        max < t.face_color.len(),
        "face id {max} out of range for {} faces",
        t.face_color.len()
    );
    // A 4x4 checkerboard has 16 cells, each its own connected component.
    assert_eq!(
        t.face_color.len(),
        16,
        "checkerboard cells are separate faces"
    );
}

#[test]
fn each_face_is_connected() {
    let img = paint(80, 40, |x, y| {
        let in_a = (4..30).contains(&x) && (4..36).contains(&y);
        let in_b = (50..76).contains(&x) && (4..36).contains(&y);
        if in_a || in_b {
            RED
        } else {
            WHITE
        }
    });
    let t = trace_color_full(&img, &ColorOptions::default());
    let (w, h) = (80usize, 40usize);

    // Flood fill from the first pixel of each face; it must reach every pixel of it.
    for face in 0..t.face_color.len() as u16 {
        let pixels: Vec<usize> = (0..w * h).filter(|&p| t.labels[p] == face).collect();
        if pixels.is_empty() {
            continue;
        }
        let mut seen = vec![false; w * h];
        let mut stack = vec![pixels[0]];
        seen[pixels[0]] = true;
        let mut count = 0usize;
        while let Some(p) = stack.pop() {
            count += 1;
            let (x, y) = (p % w, p / w);
            let go = |q: usize, stack: &mut Vec<usize>, seen: &mut Vec<bool>| {
                if !seen[q] && t.labels[q] == face {
                    seen[q] = true;
                    stack.push(q);
                }
            };
            if x > 0 {
                go(p - 1, &mut stack, &mut seen);
            }
            if x + 1 < w {
                go(p + 1, &mut stack, &mut seen);
            }
            if y > 0 {
                go(p - w, &mut stack, &mut seen);
            }
            if y + 1 < h {
                go(p + w, &mut stack, &mut seen);
            }
        }
        assert_eq!(
            count,
            pixels.len(),
            "face {face} is not connected: reached {count} of {} pixels",
            pixels.len()
        );
    }
}

/// The regression this fix exists for: rings of one colour must not be pooled into a
/// region whose colour then looks like a gradient.
#[test]
fn concentric_rings_are_separate_faces_and_each_is_flat() {
    use inkvec_trace::gradient::{self, FillModel};

    let (w, h) = (128usize, 128usize);
    // Five concentric annuli alternating between two lightnesses, on white.
    let img = paint(w, h, |x, y| {
        let d = ((x as f64 - 63.5).powi(2) + (y as f64 - 63.5).powi(2)).sqrt();
        if d > 60.0 {
            return WHITE;
        }
        let band = (d / 12.0) as usize;
        if band.is_multiple_of(2) {
            [0.20, 0.35, 0.75]
        } else {
            [0.60, 0.72, 0.95]
        }
    });

    let t = trace_color_full(&img, &ColorOptions::default());
    assert!(
        t.face_color.len() >= 5,
        "five annuli plus background should give at least 5 faces, got {}",
        t.face_color.len()
    );

    let rgb = img.composited([1.0, 1.0, 1.0]);
    let mut gradients = 0;
    for fi in 0..t.face_color.len() {
        let f = gradient::fit_fill(&rgb, w, h, &t.labels, fi as u16, t.sigma_noise, 7.15);
        if !matches!(f.model, FillModel::Flat(_)) {
            gradients += 1;
        }
    }
    assert!(
        gradients <= 1,
        "flat annuli should not be fitted as gradients; {gradients} of {} were",
        t.face_color.len()
    );
}
