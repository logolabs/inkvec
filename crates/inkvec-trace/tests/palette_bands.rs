//! A thin band of ink whose colour is a mix of its neighbours is ink, not anti-aliasing.
//!
//! The palette discards colours that lie on the segment between two accepted inks when
//! they are shaped like a boundary band, because that is what an anti-aliased edge looks
//! like. A two-pixel-wide shading strip has the same colour signature and the same lack
//! of interior, and was discarded with it; the parent region then had to grow a gradient
//! to explain the pixels. What separates the two is whether the pixels *straddle* the
//! inks they blend: every pixel of a coverage ramp does, no pixel of a band does.

use inkvec_trace::color::{self, rgb_to_oklab, DEFAULT_MERGE_DISTANCE};
use inkvec_trace::gradient::bic_lambda;

const A: [f32; 3] = [0.65, 0.42, 0.26];
const B: [f32; 3] = [0.26, 0.14, 0.10];

fn mix(t: f32) -> [f32; 3] {
    [
        A[0] + (B[0] - A[0]) * t,
        A[1] + (B[1] - A[1]) * t,
        A[2] + (B[2] - A[2]) * t,
    ]
}

/// Left half `A`, right half `B`, with `band` columns of the 50 % mix between them,
/// then one column of 25 % / 75 % coverage on each side of the band when `aa` is set.
fn image(band: usize, aa: bool) -> (Vec<[f32; 3]>, usize, usize) {
    let (w, h) = (96usize, 64usize);
    let x0 = 48 - band / 2;
    let mut rgb = Vec::with_capacity(w * h);
    for _y in 0..h {
        for x in 0..w {
            let c = if x < x0 {
                if aa && x + 1 == x0 {
                    mix(0.25)
                } else {
                    A
                }
            } else if x < x0 + band {
                mix(0.5)
            } else if aa && x == x0 + band {
                mix(0.75)
            } else {
                B
            };
            rgb.push(c);
        }
    }
    (rgb, w, h)
}

fn palette_has(rgb: &[[f32; 3]], w: usize, h: usize, c: [f32; 3]) -> bool {
    let pal = color::extract_palette_mdl(
        rgb,
        w,
        h,
        DEFAULT_MERGE_DISTANCE,
        64,
        color::PaletteEvidence {
            sigma_noise: 0.004,
            lambda: bic_lambda(w * h),
            noise_sigmas: 0.0,
            same_ink_de00: color::SAME_INK_DE00,
        },
    );
    let target = rgb_to_oklab(c);
    pal.colors.iter().any(|&p| p.dist(target) < 0.02)
}

#[test]
fn two_pixel_band_of_a_mixed_colour_is_ink() {
    let (rgb, w, h) = image(2, false);
    assert!(
        palette_has(&rgb, w, h, mix(0.5)),
        "2 px band discarded as anti-aliasing"
    );
}

#[test]
fn three_pixel_band_with_antialiased_edges_is_ink() {
    let (rgb, w, h) = image(3, true);
    assert!(
        palette_has(&rgb, w, h, mix(0.5)),
        "3 px band discarded as anti-aliasing"
    );
    assert!(
        !palette_has(&rgb, w, h, mix(0.25)),
        "coverage ramp accepted as ink"
    );
}

#[test]
fn one_pixel_coverage_line_is_not_ink() {
    let (rgb, w, h) = image(1, false);
    assert!(
        !palette_has(&rgb, w, h, mix(0.5)),
        "1 px coverage line accepted as ink"
    );
}
