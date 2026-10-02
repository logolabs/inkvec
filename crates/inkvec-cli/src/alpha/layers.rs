//! `--layers`: translucent layers recovered from the traced faces, and the guard that keeps
//! the layered document from drawing anything the flat one does not.
//!
//! Split out of `alpha.rs` (2026-10-02) when the rules below made that file too long; the
//! module documentation of [`crate::alpha`] still describes where this sits: called at emit
//! time from `pipeline.rs`, its result handed to `write_colour`, which writes the layered
//! document only when it pays.

use inkvec_trace::{gradient, planar};

use crate::args::Args;
use crate::diag;

/// Uncertainty of a face's mean colour, in sRGB units, for the layer hypothesis.
///
/// The module (`inkvec_trace::alpha`) defaults to 1.5/255, the noise of its own tests. Our
/// face colours are medians over evidence pixels of a matted image and carry more than
/// that: on a synthetic stack of three translucent discs, 1.5 finds nothing and 3 finds
/// the layer with a
/// residual of 0.0007. The false-alarm rate grows with the square of this, so it is the
/// smallest value that finds a layer we know is there.
const LAYER_SIGMA_SRGB: f64 = 3.0 / 255.0;

/// Largest colour error, in CIEDE2000, that painting a face as "ground under the layers"
/// may make: the layer stack composited over the face's recovered ground must reproduce
/// the face's own colour this closely, or no layer is written ([`layers_reproduce`]).
///
/// One unit is about a just-noticeable difference, and it is the tolerance the pipeline
/// already spends when it unions a covered face with the uncovered ground of the same
/// colour (`write_colour` in `pipeline.rs`), so a layer that passes changes no face by more
/// than the merge after it may.
const LAYER_MAX_DE00: f32 = 1.0;

/// Translucent layers: one shape at one opacity, seen against several grounds.
///
/// `inkvec_trace::alpha::decompose_with` recovers these from the face partition alone — no
/// alpha channel needed, because the evidence is that the differences between a layer's
/// faces are parallel to the differences between the grounds beneath them, scaled by
/// `1 - a`. A face under a layer of colour `L` and opacity `a` reads
/// `c_f = a·L + (1 − a)·G_f`, with `G_f` the ground it covers, so two covered faces differ
/// by `c_f − c_g = (1 − a)·(G_f − G_g)`. It is what turns three overlapping circles at 85%
/// into three circles instead of five flat patches.
///
/// It is only accepted when it explains the faces to well inside the uncertainty of a
/// face's own colour ([`LAYER_SIGMA_SRGB`]): a missed layer costs parameters, an invented
/// one is a visible error, and the module's own documentation is emphatic about which way
/// to lean.
///
/// Off by default (`--layers`), and the reason is compactness rather than correctness. The
/// layer reproduces the image exactly — the faces beneath it are repainted with the ground
/// and the layer is composited over them — but it only pays when the ground pieces it
/// reunites merge back into fewer shapes. The pipeline does that merge and then keeps the
/// layered document only when it has fewer shapes and no more bytes than the flat one. On
/// real art it is rare besides: two of forty icons in the census.
///
/// Inputs, all indexed by face id: `face_color` (palette index per face), `fills` (each
/// face's fill model) and `traced_labels` (the label map, for each face's pixel area).
/// Adjacency comes from the map's edges. Returns `None` when `--layers` is off, when no
/// layer was found, or when the layers found would not reproduce every face they cover
/// ([`layers_reproduce`]); unless `quiet`, each found layer is described on stderr.
///
/// Three rules decide what the decomposition may see and what it may return, and each one
/// closes a way the layered document used to draw something other than the image. On
/// `noto-emoji/emoji_u1f469_1f3fb_200d_1f52c` (128 px) the first two were at work at once:
/// a black layer at 0.924, recovered in linear light over four hair faces, two of them
/// gradients, repainted the dark grey hair near black, dE00 0.609 -> 2.92 (r2-fidelity,
/// 2026-10-02). With the rules no layer is found there and `--layers` writes the flat
/// document, byte for byte.
///
/// 1. **The space the document composites in.** The layer is written as
///    `fill="C" fill-opacity="a"` over the faces repainted with their ground `G`, and a
///    renderer composites that on the gamma-encoded values: `c = a·C + (1 − a)·G` in sRGB.
///    SVG 1.1 §11.7.1 makes `color-interpolation` the space of alpha compositing, with
///    sRGB its initial value, and the emitter writes no other. A layer recovered in linear
///    light satisfies
///    `lin(c) = a·lin(C) + (1 − a)·lin(G)` instead, and written into the document it draws a
///    different colour: black at 0.924 over the hair's recovered ground comes out at a
///    quarter of the hair's lightness. So only [`inkvec_trace::alpha::Space::Srgb`] is
///    tried. This used to try both and keep whichever found more layers, which is the
///    module's advice for *recognising* a file's compositing space, not for writing one.
/// 2. **Flat faces only.** A face under a layer is painted with one flat ground colour. A
///    gradient face has no such colour -- its palette ink stood in for it here -- and
///    painting it flat throws its shading away whatever the layer is. Gradient faces are
///    therefore given no area, which the decomposition reads as "takes no part", exactly as
///    it treats an anti-aliasing sliver: such a face is neither a layer face nor a ground.
/// 3. **Reproduction.** The decomposition clamps every recovered ground to `[0, 1]`, so a
///    face whose ground would lie outside the gamut is drawn lighter or darker than it was.
///    The layered document is only written when every covered face comes back within
///    [`LAYER_MAX_DE00`] of its own colour ([`layers_reproduce`]).
///
/// Method from: Porter, Duff, "Compositing Digital Images", SIGGRAPH 1984,
/// doi:10.1145/800031.808606 (the "over" operator the layer model inverts); W3C, "SVG 1.1
/// (Second Edition)", 2011, §11.7.1 `color-interpolation` (sRGB, the compositing space of
/// every painted element), <https://www.w3.org/TR/SVG11/painting.html#ColorInterpolationProperty>.
/// Not from the literature: the three rules, because they are the conditions under which
/// this emitter's own layered document equals the flat one; the decomposition itself is
/// `inkvec_trace::alpha`'s. See also: Richardt et al., "Vectorising Bitmaps into
/// Semi-Transparent Gradient Layers", EGSR 2014, doi:10.1111/cgf.12408, which recovers
/// gradient layers jointly and would lift rule 2.
pub(crate) fn recover_layers(
    args: &Args,
    map: &planar::PlanarMap,
    face_color: &[usize],
    fills: &[gradient::FillFit],
    pal: &inkvec_trace::color::Palette,
    traced_labels: &[u16],
) -> Option<inkvec_trace::alpha::AlphaAnalysis> {
    if !args.layers {
        return None;
    }
    let n_faces = face_color.len();
    let flat = |f: usize| {
        fills
            .get(f)
            .is_none_or(|x| matches!(x.model, gradient::FillModel::Flat(_)))
    };
    // Rule 2: a face that is not painted one flat colour takes no part.
    let mut area = vec![0usize; n_faces];
    for &l in traced_labels.iter() {
        if (l as usize) < n_faces {
            area[l as usize] += 1;
        }
    }
    for (f, a) in area.iter_mut().enumerate() {
        if !flat(f) {
            *a = 0;
        }
    }
    // The colour the flat document paints each face: its flat fill, else its palette ink.
    let rgb_of: Vec<[f32; 3]> = (0..n_faces)
        .map(|f| match fills.get(f).map(|x| &x.model) {
            Some(gradient::FillModel::Flat(c)) => *c,
            _ => face_color
                .get(f)
                .and_then(|&ci| pal.rgb.get(ci))
                .copied()
                .unwrap_or([0.0, 0.0, 0.0]),
        })
        .collect();
    let mut adjacency: Vec<(usize, usize)> = map
        .edges
        .iter()
        .filter(|e| e.left != e.right)
        .map(|e| (e.left as usize, e.right as usize))
        .filter(|&(a, b)| a < n_faces && b < n_faces)
        .collect();
    adjacency.sort_unstable();
    adjacency.dedup();
    // Rule 1: the space the written document composites in, and no other.
    let opt = inkvec_trace::alpha::AlphaOptions {
        space: inkvec_trace::alpha::Space::Srgb,
        sigma_srgb: LAYER_SIGMA_SRGB,
        ..Default::default()
    };
    let an = inkvec_trace::alpha::decompose_with(&rgb_of, &area, &adjacency, &opt);
    if an.layers.is_empty() {
        return None;
    }
    // Rule 3: every covered face must come back as it was.
    let worst = layers_reproduce(&an, &rgb_of);
    let kept = worst <= LAYER_MAX_DE00;
    let quiet = args.quiet;
    diag::stage(quiet, || {
        format!(
            "  layers        {} translucent layer(s) over a continuous ground (sRGB), worst face \
             dE00 {worst:.2}: {}",
            an.layers.len(),
            if kept {
                "kept"
            } else {
                "not reproduced, dropped"
            }
        )
    });
    for l in &an.layers {
        diag::stage(quiet, || {
            format!(
                "                {} at {:.3} across {} faces, residual {:.5}",
                inkvec_trace::color::to_hex(l.color),
                l.alpha,
                l.faces.len(),
                l.residual
            )
        });
    }
    kept.then_some(an)
}

/// The largest colour error, in CIEDE2000, with which the layered document would draw a
/// face the layers cover: 0 when no face is covered.
///
/// The document paints a covered face `f` with its ground `G_f = base_rgb[f]` and then every
/// layer over it, back to front, each by the sRGB "over" operator
/// `c ← a_k·C_k + (1 − a_k)·c` (`a_k`, `C_k` the layer's opacity and colour; channels in
/// `[0, 1]`). `an.layers` is in peel order, frontmost first, so the composite starts from the
/// ground and applies the covering layers last to first. The result is compared with
/// `rgb_of[f]`, the colour the flat document paints the face, by
/// [`inkvec_trace::color::de00`].
///
/// Exact algebra gives 0 for every face the decomposition fitted; what this catches is a
/// ground that the decomposition had to clamp into the gamut, and a fit residual larger than
/// a face's own colour noise. Linear in the number of (layer, face) memberships.
fn layers_reproduce(an: &inkvec_trace::alpha::AlphaAnalysis, rgb_of: &[[f32; 3]]) -> f32 {
    let mut worst = 0.0f32;
    let covered: std::collections::BTreeSet<usize> = an
        .layers
        .iter()
        .flat_map(|l| l.faces.iter().copied())
        .collect();
    for f in covered {
        let (Some(&ground), Some(&own)) = (an.base_rgb.get(f), rgb_of.get(f)) else {
            continue;
        };
        let mut c = ground;
        for l in an.layers.iter().rev() {
            if l.faces.binary_search(&f).is_ok() {
                let a = l.alpha;
                c = [
                    a * l.color[0] + (1.0 - a) * c[0],
                    a * l.color[1] + (1.0 - a) * c[1],
                    a * l.color[2] + (1.0 - a) * c[2],
                ];
            }
        }
        worst = worst.max(inkvec_trace::color::de00(c, own));
    }
    worst
}

#[cfg(test)]
mod tests;
