//! A lossy intake's codec damage taken out of a palette label image: Quality mode's use of
//! Fast's halo pass ([`super::faces`]), with one test of its own. Called from
//! [`crate::trace_color_full_with_alpha`] after the labels are despeckled and before blend
//! absorption, on an opaque lossy intake only, and not on an image that came out of the
//! restorer (`ColorOptions::restored`): the restorer took the codec's damage out, and on the
//! restored path (`quality-web`, `--restore on`, the gate's 0.3 sample) the clean-up read dE00
//! +0.24 %, `openmoji/E30B`'s brown border strip, a real ink, going to the black outline.
//!
//! # The problem
//!
//! Quality's palette (the MDL walk, `color/mdl.rs`) admits what the codec made: on
//! `openmoji/E056` (`web` tier) it keeps three tinted blacks, `#000109`, `#080100` and
//! `#030801`, beside `#000000`, and the trace came back with 674 faces, 41 colours and 250
//! subpaths where the artist drew four colours. A JPEG stores chroma at half resolution, so
//! a black stroke beside a blue fill takes a blue tint over its whole width without moving
//! its luma, and the walk sees a colour with thousands of interior pixels, more than 5 dE00
//! from black when it is weighed (`#00010f`); its mean then settles nearer black. Along every
//! edge the codec also leaves the halos Fast's pass was written for.
//!
//! # The clean-up
//!
//! 1. **Codec variants** ([`variant_map`]): an ink whose colour lies within
//!    [`CODEC_VARIANT_LEVELS`] of a heavier ink in luma-weighted YCbCr (each chroma channel
//!    at a quarter of luma's weight on squares, `faces::halos::coverage`'s metric: the
//!    chroma is at half resolution and bleeds), and within the soft floor
//!    (`color::SOFT_SAME_INK_DE00`), is that ink, everywhere. The tinted blacks above lie 2.7
//!    to 5.9 levels from black; the mosaic's neighbouring fills differ in luma and lie
//!    farther (`synthetic/mosaic_grid6` is unchanged by the clean-up).
//! 2. **Halos**: [`super::faces::RunLabels::absorb_halos_held`] with every thin component
//!    held to the strip rule and the segment test (`strict`), not only one of an ink that
//!    owns an area elsewhere as in Fast. Quality's palette admits a thin ink on evidence, and
//!    a real strip can lie on the segment between its neighbours: `noto-emoji/emoji_u1f9ed`'s
//!    needle shadow, a strip three to four pixels wide between the teal needle and the white
//!    dial, went with the hull test alone.
//! 3. **Thin variants**: [`super::faces::RunLabels::merge_same_inks`] at the soft floor for
//!    thin components, as in Fast.
//!
//! The labels come back as palette indices; the palette itself is not changed, so an ink
//! nothing is labelled with any more simply draws no face.
//!
//! # Literature
//!
//! - Method from: ITU-R BT.601 (JPEG's colour space) and ITU-T T.81's 4:2:0 subsampling, as
//!   in the halo pass.
//! - Not from the literature: the luma-weighted variant test and its reach.

use super::faces::{self, Pixels, RunLabels};
use crate::Palette;

/// How far (8-bit levels, luma-weighted YCbCr) an ink may lie from a heavier one and still be
/// the codec's variant of it ([`variant_map`]): the tinted blacks of `openmoji/E056` lie
/// 2.7-5.9 levels from black. The clean-up as a whole was measured on `quality-web` against
/// the build before it (246 icons, 2026-10-11, the gate's distance axes): dE00 −6.83 %,
/// `turning_gap` −36.57 %, `ratio_gap` −11.89 %, geom −3.73 %. No sweep of this value.
pub(crate) const CODEC_VARIANT_LEVELS: f32 = 8.0;

/// Take the codec's variants, halos and thin variants out of the palette labels `labels` of
/// `img` (only its size is read), `rgb` being the image over white and `pal` the palette they index; nothing when
/// `alpha` (the source's, when known) holds a pixel below [`crate::native::OPAQUE`], as the
/// tests read JPEG's colour space, which has no opacity. See the module documentation. Says
/// what it did under `INKVEC_DIAG`.
pub(crate) fn clean_labels(
    labels: &mut [u16],
    img: &crate::Rgba,
    rgb: &[[f32; 3]],
    pal: &Palette,
    alpha: Option<&[f32]>,
) {
    if alpha.is_some_and(|a| a.iter().any(|&v| v < crate::native::OPAQUE)) {
        return;
    }
    let (w, h) = (img.width, img.height);
    let to = variant_map(pal);
    let variants = to
        .iter()
        .enumerate()
        .filter(|&(i, &t)| t as usize != i)
        .count();
    if variants > 0 {
        for l in labels.iter_mut() {
            if let Some(&t) = to.get(*l as usize) {
                *l = t;
            }
        }
    }
    let inks: Vec<[f32; 4]> = pal.rgb.iter().map(|c| [c[0], c[1], c[2], 1.0]).collect();
    let mut runs = RunLabels::new(labels, w, h);
    let px = Pixels { rgb, alpha: None };
    let halos = runs.absorb_halos_held(px, &inks, true);
    runs.merge_same_inks(&inks, Some(crate::color::SOFT_SAME_INK_DE00));
    let face_color = runs.write_faces(labels);
    for l in labels.iter_mut() {
        *l = face_color[*l as usize] as u16;
    }
    crate::diag!(
        "slivers",
        "lossy intake: {variants} codec variants, {} halos and {} halo specks given back",
        halos.0,
        halos.1
    );
}

/// For each ink of `pal`, the ink it is ([`CODEC_VARIANT_LEVELS`], module docs, step 1):
/// itself, or the heaviest ink before it (by weight, ties to the lower index) that is not a
/// variant itself and lies within reach. One pass in descending weight, so a chain of
/// variants maps to its heaviest member, never through a variant.
pub(crate) fn variant_map(pal: &Palette) -> Vec<u16> {
    let n = pal.rgb.len();
    let ycc = |c: [f32; 3]| faces::halos_ycc([c[0], c[1], c[2], 1.0]);
    let reach = CODEC_VARIANT_LEVELS / 255.0;
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let (wa, wb) = (
            pal.weight.get(a).copied().unwrap_or(0.0),
            pal.weight.get(b).copied().unwrap_or(0.0),
        );
        wb.total_cmp(&wa).then(a.cmp(&b))
    });
    let mut to: Vec<u16> = (0..n as u16).collect();
    for (i, &v) in order.iter().enumerate() {
        let cv = ycc(pal.rgb[v]);
        for &a in &order[..i] {
            if to[a] as usize != a {
                continue;
            }
            let ca = ycc(pal.rgb[a]);
            let d2 = (cv[0] - ca[0]).powi(2)
                + 0.25 * ((cv[1] - ca[1]).powi(2) + (cv[2] - ca[2]).powi(2));
            if d2 <= reach * reach
                && crate::color::de00(pal.rgb[v], pal.rgb[a]) < crate::color::SOFT_SAME_INK_DE00
            {
                to[v] = a as u16;
                break;
            }
        }
    }
    to
}

#[cfg(test)]
mod tests;
