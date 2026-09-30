//! Residual feature carving: recover features quantised away by palette extraction.
//!
//! The `carve` stage of the Quality pipeline, run by `lib.rs` (and by `native.rs` on the
//! alpha path) right after band merging, unless `INKVEC_NO_CARVE` is set. Input: the
//! label map and per-label fills that [`super::bands`] produced, the sRGB image and the
//! palette. Output: the same label map with small unexplained clusters relabelled as new
//! flat regions, `fills` and `ink` extended to match, and the parents refitted.

use super::*;
use crate::color::Palette;

// ---------------------------------------------------------------------------------------
// Features carved out of a region's residual
// ---------------------------------------------------------------------------------------

/// Residual, in sRGB units per channel, beyond which an interior pixel is not explained
/// by its region's fill at all. Eight times the noise, and never below 0.06 (about 15
/// levels): an anti-aliasing fringe the evidence test let through sits below this; a
/// seam, a dot or a stroke that the palette quantised away sits far above it.
const CARVE_RESIDUAL: f64 = 0.06;
/// Most features carved from one image, a guard against a textured region shattering.
const CARVE_MAX: usize = 64;

/// Give clustered residual pixels their own region.
///
/// The palette sees colours, not features: a two-pixel white seam through a black shape,
/// a pair of dots, the end of a thin stroke, all quantise to the ink around them and are
/// labelled as part of it. The fill fitter then sees a solid region with a cluster of
/// pixels it cannot explain, and the only tool it has is a gradient, so it buys one —
/// three radials on solid black (ebox), black lightening to grey at a rim (luanti),
/// a dark core on a flat face (kiss, zebra). The residual cluster is the feature. This
/// pass finds interior pixels whose residual against their region's chosen fill exceeds
/// `CARVE_RESIDUAL`, takes their 4-connected components of at least `min_size`
/// pixels, mints a flat region for each with the cluster's own median colour, and refits
/// the parents without them. Interior only: fringe pixels along a boundary are blends,
/// not features, and belong to the boundary unmixing.
///
/// `fills` and `ink` are indexed by label and grow by one per minted region, the same
/// contract as [`merge_gradient_bands_with_ink`]. Returns the number of regions minted.
/// Nothing is minted for an empty image or an empty `fills`, beyond [`CARVE_MAX`]
/// features, or once the label space (`u16`) is full.
/// As [`merge_gradient_bands_with_ink`]: two of these are `&mut` outputs rather than
/// settings, so a settings struct would not hold them anyway.
#[allow(clippy::too_many_arguments)]
pub fn carve_residual_features(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pal: &Palette,
    fills: &mut Vec<FillFit>,
    ink: &mut Vec<usize>,
    sigma_noise: f64,
    lambda: f64,
    min_size: usize,
) -> usize {
    carve_residual_features_with_detail_noise(
        labels,
        rgb,
        w,
        h,
        pal,
        fills,
        ink,
        sigma_noise,
        lambda,
        min_size,
        None,
    )
}

/// As [`carve_residual_features`], with a separate noise estimate well inside regions.
/// Compression-boundary uncertainty must not suppress interior artwork details.
///
/// `detail_sigma`, when given, replaces `sigma_noise` in the residual threshold for a
/// pixel whose whole 7x7 neighbourhood carries its own label: lossy input is noisiest at
/// edges, and a threshold set by edge noise would hide a faint stroke in the middle of a
/// region. `None` is exactly [`carve_residual_features`].
///
/// The work runs in three stages: [`residual_mask`] marks the candidate pixels,
/// [`mint_features`] turns their clusters into new flat regions, and [`refit_parents`]
/// refits the regions they were cut from. The signature is kept flat rather than
/// grouped into a struct because it is public and called from three places (`lib.rs`
/// twice, `native.rs` once), and its arguments are three different things — the image,
/// three `&mut` outputs, and four settings — that a single struct would only rename.
#[allow(clippy::too_many_arguments)]
pub fn carve_residual_features_with_detail_noise(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pal: &Palette,
    fills: &mut Vec<FillFit>,
    ink: &mut Vec<usize>,
    sigma_noise: f64,
    lambda: f64,
    min_size: usize,
    detail_sigma: Option<f64>,
) -> usize {
    let n = w * h;
    if fills.is_empty() || n == 0 {
        return 0;
    }
    let img = CarveImage {
        rgb,
        w,
        h,
        pal,
        sigma_noise,
        lambda,
    };
    let mask = residual_mask(&img, labels, fills, detail_sigma);
    let (minted, parents) = mint_features(&img, labels, fills, ink, &mask, min_size);
    if minted == 0 {
        return 0;
    }
    refit_parents(&img, labels, fills, parents);
    minted
}

/// The inputs every carving stage reads and none changes: the sRGB image (0..1) of
/// `w`×`h` pixels, its palette, the per-channel noise in sRGB units and the MDL
/// parameter price.
struct CarveImage<'a> {
    /// The composited sRGB image, 0..1 per channel, row-major.
    rgb: &'a [[f32; 3]],
    /// Image width, px.
    w: usize,
    /// Image height, px.
    h: usize,
    /// The palette the labels index (the first `pal.rgb.len()` labels).
    pal: &'a Palette,
    /// Per-channel pixel noise, sRGB units.
    sigma_noise: f64,
    /// Price of one editable number in the MDL cost.
    lambda: f64,
}

impl CarveImage<'_> {
    /// The ink colour of every label: the palette colour for palette labels, the fill's
    /// representative colour for labels minted after the palette.
    fn inks(&self, fills: &[FillFit]) -> Vec<[f32; 3]> {
        (0..fills.len())
            .map(|l| {
                self.pal
                    .rgb
                    .get(l)
                    .copied()
                    .unwrap_or_else(|| fills[l].model.representative())
            })
            .collect()
    }
}

/// Whether pixel `p` is strictly inside its label: not on the picture edge, and all four
/// neighbours carry the same label.
fn interior4(labels: &[u16], w: usize, h: usize, p: usize) -> bool {
    let (x, y) = (p % w, p / w);
    let l = labels[p];
    x > 0
        && x + 1 < w
        && y > 0
        && y + 1 < h
        && labels[p - 1] == l
        && labels[p + 1] == l
        && labels[p - w] == l
        && labels[p + w] == l
}

/// Stage 1: the pixels the chosen fill does not explain.
///
/// A pixel is marked when it is strictly interior to its label ([`interior4`]), pure
/// (not a blend of nearby inks, [`fill_evidence`]), its label has a reliable flat colour
/// ([`region_flat_colours`]), and its residual against the label's fill,
/// `r = max_ch |c_ch − f_ch(x, y)|` in sRGB, exceeds `max(8σ, CARVE_RESIDUAL)`. Where
/// `detail_sigma` is given and the pixel's 7x7 neighbourhood is all one label, `σ` is
/// `detail_sigma` instead of the image noise.
fn residual_mask(
    img: &CarveImage,
    labels: &[u16],
    fills: &[FillFit],
    detail_sigma: Option<f64>,
) -> Vec<bool> {
    let (rgb, w, h) = (img.rgb, img.w, img.h);
    let n = w * h;
    let thr = (8.0 * img.sigma_noise).max(CARVE_RESIDUAL) as f32;
    // Only pixels that are not blends of nearby inks can be features: a two-pixel grey
    // fleck where two black strokes almost touch is anti-aliasing of the gap, not a
    // thing to draw, and minted as a region it paints over the gap (send_and_archive:
    // two such flecks cost 0.19 dE00). A seam *inside* a region has no other ink within
    // reach and stays eligible.
    let ink_rgb0 = img.inks(fills);
    let pure0 = fill_evidence(rgb, w, h, labels, &ink_rgb0, img.sigma_noise);
    let flat_colour = region_flat_colours(img, labels, fills.len(), &pure0);
    // A pixel is a candidate when the fill the fitter chose does not explain it.
    //
    // Two alternatives were tried and are *not* what ships. Testing the residual against
    // the flat colour instead carved hard-edged shading the fitter had legitimately
    // captured with a gradient (noto 1f9d1_1f3fc_200d_1f37c +0.13, 1F3DD +0.10);
    // requiring both residuals to fail is stricter still. The dev set (51 icons)
    // preferred the flat rule with the size and step-edge guards below (0.7797 against
    // 0.7888), but the full 980-icon set reversed it: against the plain fit rule without
    // guards, flat+guards was 82 better / 193 worse in dE00 (+0.0022) and fit+guards
    // 57 / 191 (+0.0031), both at an unchanged objective. The plain rule ships.
    //
    // The flat colour is still computed and still gates: a region with too few pure
    // interior pixels to take a median of has no reliable notion of its own colour, and
    // its pixels are skipped rather than carved.
    let mut mask = vec![false; n];
    for p in 0..n {
        let l = labels[p] as usize;
        let Some(Some(_flat)) = flat_colour.get(l) else {
            continue;
        };
        let Some(f) = fills.get(l) else { continue };
        if !interior4(labels, w, h, p) || !pure0[p] {
            continue;
        }
        let c = rgb[p];
        let pred = f.model.color_at((p % w) as f64, (p / w) as f64);
        let r_fit = (0..3).map(|k| (c[k] - pred[k]).abs()).fold(0f32, f32::max);
        let interior_thr = detail_sigma
            .filter(|_| {
                let (x, y) = (p % w, p / w);
                x >= 3
                    && y >= 3
                    && x + 3 < w
                    && y + 3 < h
                    && (y - 3..=y + 3)
                        .all(|yy| (x - 3..=x + 3).all(|xx| labels[yy * w + xx] == labels[p]))
            })
            .map(|s| (8.0 * s).max(CARVE_RESIDUAL) as f32)
            .unwrap_or(thr);
        if r_fit > interior_thr {
            mask[p] = true;
        }
    }
    mask
}

/// Each label's *flat* colour: the per-channel sRGB median of its pure interior pixels,
/// or `None` when it has fewer than eight of them. `n_labels` is `fills.len()`.
///
/// Taking the residual against this instead of against the fill the fitter chose was
/// the original design, on the argument that the chosen fill may be a gradient bought
/// precisely to explain the feature (ebox's seams) and a residual against that
/// gradient hides it. It needed a size guard to work, because against a flat colour a
/// genuine gradient region has residual everywhere and its cluster is the whole
/// region; and measured on the full set the pair lost more than it won (see the
/// candidate test in [`residual_mask`]). What survives is the gate: a region with fewer
/// than eight pure interior pixels has no reliable colour of its own and is left alone.
fn region_flat_colours(
    img: &CarveImage,
    labels: &[u16],
    n_labels: usize,
    pure0: &[bool],
) -> Vec<Option<[f32; 3]>> {
    let (rgb, w, h) = (img.rgb, img.w, img.h);
    let mut med_samples: Vec<[Vec<f32>; 3]> = (0..n_labels)
        .map(|_| [Vec::new(), Vec::new(), Vec::new()])
        .collect();
    for p in 0..w * h {
        let l = labels[p] as usize;
        if l < med_samples.len() && pure0[p] && interior4(labels, w, h, p) {
            for k in 0..3 {
                med_samples[l][k].push(rgb[p][k]);
            }
        }
    }
    med_samples
        .iter_mut()
        .map(|ch| {
            if ch[0].len() < 8 {
                return None;
            }
            let mut c = [0f32; 3];
            for k in 0..3 {
                let m = ch[k].len() / 2;
                let (_, med, _) = ch[k].select_nth_unstable_by(m, |a, b| a.total_cmp(b));
                c[k] = *med;
            }
            Some(c)
        })
        .collect()
}

/// The 4-connected cluster of marked pixels that `start` belongs to, within `start`'s
/// label, by depth-first flood fill. Visited pixels are marked in `seen` (any value but
/// `u32::MAX`), so every pixel joins at most one cluster.
fn flood_cluster(
    mask: &[bool],
    seen: &mut [u32],
    labels: &[u16],
    w: usize,
    h: usize,
    start: usize,
) -> Vec<usize> {
    let l = labels[start];
    let mut stack = vec![start];
    let mut group = Vec::new();
    seen[start] = 0;
    while let Some(p) = stack.pop() {
        group.push(p);
        let (x, y) = (p % w, p / w);
        let mut visit = |q: usize| {
            if mask[q] && seen[q] == u32::MAX && labels[q] == l {
                seen[q] = 0;
                stack.push(q);
            }
        };
        if x > 0 {
            visit(p - 1);
        }
        if x + 1 < w {
            visit(p + 1);
        }
        if y > 0 {
            visit(p - w);
        }
        if y + 1 < h {
            visit(p + w);
        }
    }
    group
}

/// Per-channel sRGB median of the pixels `group` (the upper median for an even count).
/// `group` must not be empty.
fn median_colour(rgb: &[[f32; 3]], group: &[usize]) -> [f32; 3] {
    let mut colour = [0f32; 3];
    for k in 0..3 {
        let mut v: Vec<f32> = group.iter().map(|&p| rgb[p][k]).collect();
        let m = v.len() / 2;
        let (_, med, _) = v.select_nth_unstable_by(m, |a, b| a.total_cmp(b));
        colour[k] = *med;
    }
    colour
}

/// Stage 2: give every large enough cluster of `mask` its own label and flat fill.
///
/// Clusters are taken in raster order of their first pixel. One of at least
/// `max(min_size, 4)` pixels, while fewer than [`CARVE_MAX`] have been minted and the
/// label space has room, gets the next label id, a [`flat_only`] fill of its median
/// colour, and the palette entry nearest that colour (squared sRGB distance) as its ink.
/// Returns how many were minted and the labels they were cut from, in first-cut order.
fn mint_features(
    img: &CarveImage,
    labels: &mut [u16],
    fills: &mut Vec<FillFit>,
    ink: &mut Vec<usize>,
    mask: &[bool],
    min_size: usize,
) -> (usize, Vec<usize>) {
    let (rgb, w, h, pal) = (img.rgb, img.w, img.h, img.pal);
    let n = w * h;
    // 4-connected components of the mask, within one label.
    let mut comp = vec![u32::MAX; n];
    let mut minted = 0usize;
    let mut parents: Vec<usize> = Vec::new();
    for start in 0..n {
        if !mask[start] || comp[start] != u32::MAX {
            continue;
        }
        let l = labels[start];
        let group = flood_cluster(mask, &mut comp, labels, w, h, start);
        // Rejected: capping a cluster at a fifth of its parent (and at 200 pixels), on
        // the theory that a feature is small against its parent - a seam, a dot, a stroke
        // end - and anything larger is a region the palette lost, which is the palette's
        // problem to fix rather than this pass's. Measured on the full set that guard,
        // with the step-edge test below, lost more small features than the explosions it
        // prevented. Only [`CARVE_MAX`] survives, against a textured region shattering.
        if group.len() < min_size.max(4) || minted >= CARVE_MAX {
            continue;
        }
        // Rejected with it: a step-edge test requiring the mean contrast across the
        // cluster's boundary with the rest of the parent to reach the residual threshold.
        // The reasoning was sound against a *flat* residual - the bright end of a genuine
        // gradient is also a small cluster of large residual, but it is reached by a ramp
        // where a seam is a step (a skin-tone highlight carved this way cost +0.06 on
        // emoji_u1f469_1f3fd_200d_1f4bc; the ebox seam steps by 0.5) - but it only ever
        // ran alongside the flat rule, which does not ship. It cost a four-neighbour
        // sweep of every candidate cluster on the default path, where nothing read it.
        // The feature's colour: per-channel median of its pixels.
        let colour = median_colour(rgb, &group);
        let new_label = fills.len();
        if new_label >= u16::MAX as usize {
            break;
        }
        if inkvec_core::env::flag("INKVEC_EVDBG") {
            let xs = group.iter().map(|&p| p % w);
            let ys = group.iter().map(|&p| p / w);
            const NON_EMPTY: &str = "a cluster holds at least its seed pixel";
            eprintln!(
                "  [carve] parent {} size {} bbox x{}..{} y{}..{} colour {:?}",
                l,
                group.len(),
                xs.clone().min().expect(NON_EMPTY),
                xs.max().expect(NON_EMPTY),
                ys.clone().min().expect(NON_EMPTY),
                ys.max().expect(NON_EMPTY),
                [
                    (colour[0] * 255.0) as u8,
                    (colour[1] * 255.0) as u8,
                    (colour[2] * 255.0) as u8
                ]
            );
        }
        for &p in &group {
            labels[p] = new_label as u16;
        }
        fills.push(flat_only(colour, img.lambda));
        // Name it by the palette entry it is nearest to, as the band merger does.
        let nearest = (0..pal.rgb.len())
            .min_by(|&a, &b| {
                let da: f32 = (0..3).map(|k| (pal.rgb[a][k] - colour[k]).powi(2)).sum();
                let db: f32 = (0..3).map(|k| (pal.rgb[b][k] - colour[k]).powi(2)).sum();
                da.total_cmp(&db)
            })
            .unwrap_or(l as usize);
        ink.push(nearest);
        if !parents.contains(&(l as usize)) {
            parents.push(l as usize);
        }
        minted += 1;
    }
    (minted, parents)
}

/// Stage 3: refit every parent without the pixels that were never its own.
///
/// The evidence mask is recomputed on the new labels (the minted features are inks now,
/// so pixels blending towards them stop counting), and each parent is refitted by full
/// model selection over its remaining pixels.
fn refit_parents(img: &CarveImage, labels: &[u16], fills: &mut [FillFit], parents: Vec<usize>) {
    let (rgb, w, h) = (img.rgb, img.w, img.h);
    let n = w * h;
    let ink_rgb = img.inks(fills);
    let pure = fill_evidence(rgb, w, h, labels, &ink_rgb, img.sigma_noise);
    let refits = parents.len() as u64;
    for (done, l) in parents.into_iter().enumerate() {
        inkvec_core::progress::step("regions refitted", done as u64, refits);
        let pixels: Vec<usize> = (0..n).filter(|&p| labels[p] as usize == l).collect();
        let f = select(fit_pixels(
            rgb,
            w,
            h,
            &pixels,
            |p| labels[p] as usize == l,
            |p| pure[p],
            img.sigma_noise,
            img.lambda,
        ));
        fills[l] = f;
    }
}

#[cfg(test)]
mod lossy_detail_tests {
    use super::*;

    #[test]
    fn boundary_noise_does_not_hide_an_interior_low_contrast_stroke() {
        let (w, h) = (32, 32);
        let mut rgb = vec![[1.0; 3]; w * h];
        for y in 8..24 {
            rgb[y * w + 12] = [0.88, 1.0, 1.0];
            rgb[y * w + 1] = [0.88, 1.0, 1.0];
        }
        let pal = Palette {
            rgb: vec![[1.0; 3]],
            colors: vec![crate::color::rgb_to_oklab([1.0; 3])],
            weight: vec![1.0],
            alpha: vec![1.0],
        };
        let run = |detail| {
            let mut labels = vec![0; w * h];
            let mut fills = vec![flat_only([1.0; 3], 1.0)];
            let mut ink = vec![0];
            carve_residual_features_with_detail_noise(
                &mut labels,
                &rgb,
                w,
                h,
                &pal,
                &mut fills,
                &mut ink,
                8.0 / 255.0,
                1.0,
                2,
                detail,
            );
            labels
        };
        assert_eq!(run(None)[16 * w + 12], 0);
        let detailed = run(Some(0.5 / 255.0));
        assert_ne!(detailed[16 * w + 12], 0);
        assert_eq!(detailed[16 * w + 1], 0);
    }
}
