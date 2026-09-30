//! Spatial label regularization for explicitly lossy rasters.
//!
//! Reduces the sum of colour residual and a boundary-length cost. Unlike an area
//! cutoff, strong one-pixel features can pay for their boundaries and survive.
//!
//! # Where this sits
//!
//! Between labelling and the planar map, in the crate root's colour trace. Every pixel
//! already carries the index of its nearest palette ink; this module measures how far
//! the pixels sit from their inks ([`residual_sigma`], [`residual_incoherence`], which
//! feed the noise estimate and its diagnostics on every trace) and, for explicitly lossy
//! input in a research build, relabels pixels to trade colour error against boundary
//! length ([`labels`], a Potts-model descent). Colours are sRGB `0..1`; label maps are
//! row-major `u16` indices into the palette.

use crate::color::Palette;

/// How much of the colour residual is INCOHERENT, in display levels: a codec-free damage signal.
///
/// **Why neither existing signal is enough.** [`residual_sigma`] measures how far interior
/// pixels sit from their own ink, and every degradation inflates it, so it is general. But
/// artwork that genuinely is not flat inflates it too: measured on clean renders it reads a
/// median of 1.41 levels on brand logos and 2.09 on diagrams against 0.50 on flat classes, and
/// a threshold sensitive enough to catch 98% of VAE output fires on 36% of clean images.
/// `crate::coverage::ringing_score` has the opposite problem: zero false positives on clean
/// input, but it looks for Gibbs ringing, which is a JPEG artefact, so it catches 86% of JPEG
/// and only 16% of VAE.
///
/// **What separates them is smoothness, not size.** A gradient's residual varies slowly across
/// a region; compression damage, latent-decoder error and resampling error are all incoherent
/// from pixel to pixel. So take the Laplacian of the residual field rather than the residual
/// itself: a smooth ramp annihilates, and anything incoherent survives. That is the same
/// argument [`crate::coverage::estimate_noise`] makes about the image, applied to a field where
/// the ink has already been subtracted, which is what rescues it from the degeneracy that makes
/// the image version useless here (90% of an icon's pixels are exactly flat, so its median
/// Laplacian is zero however damaged the file is).
///
/// Boundary pixels are excluded outright rather than projected, because a one-pixel
/// anti-aliasing ramp is incoherent by construction at this scale and would dominate.
///
/// The scale is an RMS rather than a median, deliberately: on clean artwork the residual field
/// is exactly zero over most of its area, and a median over that population is zero whatever
/// the tail does, which is precisely the trap the image-domain estimator fell into. Outliers
/// are already excluded by the boundary mask.
///
/// Returns display levels (0 to 255), so it can be compared with a threshold written in the
/// same units a designer would use.
///
/// In symbols, with `r_i = sqrt(|I_i − c(L_i)|² / 3)` each pixel's RMS-over-channels
/// residual against its own ink and `L` the 4-neighbour Laplacian of `r`:
///
/// ```text
///     incoherence = 255 · sqrt(mean_{interior i}(L_i²) / 20)
/// ```
///
/// over pixels at least two in from the border whose four neighbours share their label.
/// Zero for images under 5x5, mismatched buffers, or fewer than 256 such pixels.
pub fn residual_incoherence(
    rgb: &[[f32; 3]],
    labels: &[u16],
    w: usize,
    h: usize,
    pal: &Palette,
) -> f64 {
    /// Sum of squared coefficients of the 4-neighbour Laplacian, `4² + 4·1² = 20`: the
    /// factor by which it multiplies the *variance* of independent noise. The mean square
    /// is divided by it before the square root, which divides the RMS by `sqrt(20)`, the
    /// kernel's amplitude gain. (Written as a literal here; `coverage` derives the same
    /// number from its `LAPLACIAN_KERNEL`.)
    const GAIN: f64 = 20.0;
    /// Too few interior samples and the statistic is meaningless.
    const MIN_SAMPLES: usize = 256;

    if w < 5 || h < 5 || rgb.len() < w * h || labels.len() < w * h {
        return 0.0;
    }
    // Residual of every pixel against its own ink, one channel-averaged magnitude per pixel.
    let mut res = vec![0f32; w * h];
    for i in 0..w * h {
        let ink = pal.rgb[labels[i] as usize];
        let mut e = 0f32;
        for c in 0..3 {
            let d = rgb[i][c] - ink[c];
            e += d * d;
        }
        res[i] = (e / 3.0).sqrt();
    }
    // A pixel is usable when it and all four neighbours carry the same label, so no
    // anti-aliasing ramp enters the Laplacian.
    let mut acc = 0f64;
    let mut n = 0usize;
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            let i = y * w + x;
            let l = labels[i];
            if labels[i - 1] != l || labels[i + 1] != l || labels[i - w] != l || labels[i + w] != l
            {
                continue;
            }
            let lap = 4.0 * res[i] - res[i - 1] - res[i + 1] - res[i - w] - res[i + w];
            acc += (lap as f64) * (lap as f64);
            n += 1;
        }
    }
    if n < MIN_SAMPLES {
        return 0.0;
    }
    ((acc / n as f64) / GAIN).sqrt() * 255.0
}

/// Estimate active-region residual noise, excluding label boundaries and exact flats.
/// JPEG error is spatially heterogeneous: an empty background must not dominate it.
///
/// Two populations, each summarised by its median (when it has at least 32 samples,
/// otherwise [`crate::coverage::NOISE_FLOOR`]):
///
/// * interior pixels (all four neighbours share the label): the RMS-over-channels
///   residual `sqrt(|I − c|² / 3)` against their own ink, counting only pixels off it by
///   more than half a level, so exactly flat areas do not vote;
/// * boundary pixels: the distance from the pixel to the segment between its own ink and
///   a neighbouring ink (the best such neighbour), as `sqrt(|residual⊥|² / 2)`. The
///   component along the segment is what anti-aliasing explains and is removed, leaving
///   two degrees of freedom.
///
/// Returns the larger of the two, in sRGB units, clamped to `[NOISE_FLOOR, 8/255]`.
/// Images under 3x3 return the floor.
pub fn residual_sigma(rgb: &[[f32; 3]], labels: &[u16], w: usize, h: usize, pal: &Palette) -> f64 {
    let mut errors = Vec::new();
    let mut edge_errors = Vec::new();
    if w < 3 || h < 3 {
        return crate::coverage::NOISE_FLOOR;
    }
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let l = labels[i];
            let ink = pal.rgb[l as usize];
            let neighbours = [i - 1, i + 1, i - w, i + w];
            if neighbours.iter().any(|&j| labels[j] != l) {
                // Coverage can explain an arbitrary point on the two-ink colour line.
                // Only the orthogonal residual is evidence of compression, not the
                // anti-aliasing ramp itself. Take the best available neighbouring ink.
                let mut best = f64::INFINITY;
                for &j in &neighbours {
                    if labels[j] == l {
                        continue;
                    }
                    let other = pal.rgb[labels[j] as usize];
                    let delta = std::array::from_fn::<_, 3, _>(|c| other[c] - ink[c]);
                    let norm = delta.iter().map(|v| v * v).sum::<f32>();
                    if norm < 1e-8 {
                        continue;
                    }
                    let t = ((0..3).map(|c| (rgb[i][c] - ink[c]) * delta[c]).sum::<f32>() / norm)
                        .clamp(0.0, 1.0);
                    let e = (0..3)
                        .map(|c| (rgb[i][c] - ink[c] - t * delta[c]).powi(2) as f64)
                        .sum::<f64>()
                        / 2.0;
                    best = best.min(e.sqrt());
                }
                if best.is_finite() {
                    edge_errors.push(best);
                }
                continue;
            }
            let e = (0..3)
                .map(|c| (rgb[i][c] - ink[c]).powi(2) as f64)
                .sum::<f64>()
                / 3.0;
            if e > (0.5f64 / 255.0).powi(2) {
                errors.push(e.sqrt());
            }
        }
    }
    errors.sort_by(f64::total_cmp);
    edge_errors.sort_by(f64::total_cmp);
    // Residual includes palette bias, so limit its influence to 8 display levels.
    let interior = if errors.len() >= 32 {
        errors[errors.len() / 2]
    } else {
        crate::coverage::NOISE_FLOOR
    };
    let edge = if edge_errors.len() >= 32 {
        edge_errors[edge_errors.len() / 2]
    } else {
        crate::coverage::NOISE_FLOOR
    };
    interior
        .max(edge)
        .clamp(crate::coverage::NOISE_FLOOR, 8.0 / 255.0)
}

/// Deterministic four-neighbour Potts descent. Every change strictly lowers energy.
///
/// Relabels `labels` (row-major `w x h`, indices into `pal`) in place to lower
///
/// ```text
///     E(L) = Σ_i |I_i − c(L_i)|²  +  β · #{4-neighbour pairs i~j : L_i ≠ L_j}
///     β    = 2σ² · ln(max(N, 3)),   N = w·h
/// ```
///
/// where `I_i` is pixel `i`'s sRGB colour, `c(l)` ink `l`'s colour and `σ` the noise
/// estimate in sRGB units. The data term is squared sRGB distance summed over channels;
/// `β` prices one unit of boundary in the same units, scaled by the noise variance so a
/// boundary costs the same number of noise units at any noise level, and growing slowly
/// with the image.
///
/// Two stages, both greedy descents on `E` (so neither can raise it):
///
/// 1. [`pixel_descent`]: iterated conditional modes, one pixel at a time, each pixel
///    trying only the labels of its 4-neighbours.
/// 2. [`merge_components`]: whole 4-connected components relabelled at once, which is
///    what removes a weak island whose pixels all agree with each other.
///
/// Returns the number of pixel label changes. Called only on explicitly lossy intake, in
/// a research build (`INKVEC_LOSSY_REGULARIZE`), from the crate root.
pub fn labels(
    rgb: &[[f32; 3]],
    labels: &mut [u16],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma: f64,
) -> usize {
    if w == 0 || h == 0 {
        return 0;
    }
    let penalty = (2.0 * sigma * sigma * ((w * h).max(3) as f64).ln()) as f32;
    let mut changes = pixel_descent(rgb, labels, w, h, pal, penalty);
    changes += merge_components(rgb, labels, w, h, pal, penalty);
    changes
}

/// The in-image 4-neighbours of pixel `i = y·w + x`, in the order left, right, up, down,
/// and how many there are.
fn neighbours4(i: usize, x: usize, y: usize, w: usize, h: usize) -> ([usize; 4], usize) {
    let mut neighbours = [i; 4];
    let mut n = 0;
    if x > 0 {
        neighbours[n] = i - 1;
        n += 1;
    }
    if x + 1 < w {
        neighbours[n] = i + 1;
        n += 1;
    }
    if y > 0 {
        neighbours[n] = i - w;
        n += 1;
    }
    if y + 1 < h {
        neighbours[n] = i + w;
        n += 1;
    }
    (neighbours, n)
}

/// Stage 1 of [`labels`]: single-pixel moves.
///
/// Each pixel takes whichever of its own label and its neighbours' labels minimises its
/// local energy `|I_i − c(l)|² + β·#{neighbours j with L_j ≠ l}`, switching only on a
/// decrease of more than `1e-9`. Because every boundary pair touching `i` is in that local
/// sum, a switch lowers the global energy by the same amount. Pixels are visited in two
/// checkerboard half-sweeps (red–black ordering), so each half-sweep's decisions read
/// neighbours that half-sweep does not change. At most 12 sweeps; stops early on a sweep
/// with no change. Returns the number of changes.
fn pixel_descent(
    rgb: &[[f32; 3]],
    labels: &mut [u16],
    w: usize,
    h: usize,
    pal: &Palette,
    penalty: f32,
) -> usize {
    let mut changes = 0;
    for _ in 0..12 {
        let mut moved = 0;
        for parity in 0..2 {
            for y in 0..h {
                for x in 0..w {
                    if (x + y) % 2 != parity {
                        continue;
                    }
                    let i = y * w + x;
                    let (neighbours, n) = neighbours4(i, x, y, w, h);
                    let energy = |l: u16| {
                        let ink = pal.rgb[l as usize];
                        (0..3).map(|c| (rgb[i][c] - ink[c]).powi(2)).sum::<f32>()
                            + penalty
                                * neighbours[..n].iter().filter(|&&j| labels[j] != l).count() as f32
                    };
                    let mut best = labels[i];
                    let mut cost = energy(best);
                    for &j in &neighbours[..n] {
                        let l = labels[j];
                        let e = energy(l);
                        if e + 1e-9 < cost {
                            cost = e;
                            best = l;
                        }
                    }
                    if best != labels[i] {
                        labels[i] = best;
                        moved += 1;
                    }
                }
            }
        }
        changes += moved;
        if moved == 0 {
            break;
        }
    }
    changes
}

/// Stage 2 of [`labels`]: joint moves of whole components.
///
/// Single-pixel descent cannot escape a weak island whose interior pixels all agree with
/// one another. So each 4-connected component `C` (label `a`) is tested against every
/// label `b` it touches:
///
/// ```text
///     ΔE = Σ_{i∈C} (|I_i − c(b)|² − |I_i − c(a)|²) − β·shared(C, b) − 9β
/// ```
///
/// `shared` counts the pixel sides between `C` and `b`, which stop being boundary; the
/// `9β` is the component's own description, at least a closed three-point path plus an
/// RGB fill (9 scalars), which removing it saves. The most negative `ΔE`, if any, is
/// applied. Components are found by breadth-first flood from each unseen pixel in raster
/// order, so later components see earlier merges. Up to 4 passes, stopping early on a
/// pass with no merge. Returns the number of pixels relabelled.
fn merge_components(
    rgb: &[[f32; 3]],
    labels: &mut [u16],
    w: usize,
    h: usize,
    pal: &Palette,
    penalty: f32,
) -> usize {
    let mut changes = 0;
    // Single-pixel descent cannot escape a weak island whose interior pixels all
    // agree with one another. Test whole connected components as joint moves.
    // A component costs at least a closed three-point path plus an RGB fill (9
    // scalar parameters). Removing one must pay for any additional raster error.
    let region_cost = 9.0 * penalty;
    for _ in 0..4 {
        let mut seen = vec![false; w * h];
        let mut merged = 0;
        for seed in 0..w * h {
            if seen[seed] {
                continue;
            }
            let current = labels[seed];
            let (pixels, contacts) = flood_component(labels, &mut seen, seed, w, h);
            let mut best = current;
            let mut gain = 0.0;
            for (target, shared) in contacts {
                let delta = pixels
                    .iter()
                    .map(|&i| {
                        (0..3)
                            .map(|c| {
                                (rgb[i][c] - pal.rgb[target as usize][c]).powi(2)
                                    - (rgb[i][c] - pal.rgb[current as usize][c]).powi(2)
                            })
                            .sum::<f32>()
                    })
                    .sum::<f32>()
                    - penalty * shared as f32
                    - region_cost;
                if delta < gain {
                    gain = delta;
                    best = target;
                }
            }
            if best != current {
                for i in pixels {
                    labels[i] = best;
                    changes += 1;
                }
                merged += 1;
            }
        }
        if merged == 0 {
            break;
        }
    }
    changes
}

/// The 4-connected component of same-label pixels containing `seed` (breadth-first, in
/// visiting order), marking them in `seen`, and for each other label it touches the number
/// of pixel sides it shares with that label, in label order.
fn flood_component(
    labels: &[u16],
    seen: &mut [bool],
    seed: usize,
    w: usize,
    h: usize,
) -> (Vec<usize>, std::collections::BTreeMap<u16, usize>) {
    let current = labels[seed];
    let mut pixels = vec![seed];
    seen[seed] = true;
    let mut head = 0;
    let mut contacts = std::collections::BTreeMap::<u16, usize>::new();
    while head < pixels.len() {
        let i = pixels[head];
        head += 1;
        let (x, y) = (i % w, i / w);
        for j in [
            if x > 0 { Some(i - 1) } else { None },
            if x + 1 < w { Some(i + 1) } else { None },
            if y > 0 { Some(i - w) } else { None },
            if y + 1 < h { Some(i + w) } else { None },
        ]
        .into_iter()
        .flatten()
        {
            if labels[j] != current {
                *contacts.entry(labels[j]).or_default() += 1;
            } else if !seen[j] {
                seen[j] = true;
                pixels.push(j);
            }
        }
    }
    (pixels, contacts)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn palette(rgb: Vec<[f32; 3]>) -> Palette {
        let n = rgb.len();
        Palette {
            colors: rgb
                .iter()
                .copied()
                .map(crate::color::rgb_to_oklab)
                .collect(),
            rgb,
            weight: vec![1.0 / n as f32; n],
            alpha: vec![1.0; n],
        }
    }
    #[test]
    fn weak_island_removed_but_high_contrast_one_pixel_stroke_survives() {
        let p = palette(vec![[0.0; 3], [0.025; 3], [1.0; 3]]);
        let mut rgb = vec![[0.0; 3]; 81];
        let mut lab = vec![0; 81];
        rgb[20] = [0.025; 3];
        lab[20] = 1;
        for y in 0..9 {
            rgb[y * 9 + 6] = [1.0; 3];
            lab[y * 9 + 6] = 2;
        }
        labels(&rgb, &mut lab, 9, 9, &p, 3.0 / 255.0);
        assert_eq!(lab[20], 0);
        for y in 0..9 {
            assert_eq!(lab[y * 9 + 6], 2);
        }
    }
    #[test]
    fn exact_flats_do_not_invent_noise() {
        let p = palette(vec![[0.5; 3]]);
        assert_eq!(
            residual_sigma(&vec![[0.5; 3]; 100], &[0; 100], 10, 10, &p),
            crate::coverage::NOISE_FLOOR
        );
    }
    #[test]
    fn label_changes_reduce_the_stated_global_energy() {
        let p = palette(vec![[0.2; 3], [0.3; 3]]);
        let rgb = vec![[0.24; 3]; 64];
        let mut lab: Vec<u16> = (0..64).map(|i| (i % 3 == 0) as u16).collect();
        let sigma = 0.025;
        let penalty = 2.0 * sigma * sigma * 64f64.ln();
        let energy = |l: &[u16]| {
            let mut total = 0.0;
            for i in 0..64 {
                total += (0..3)
                    .map(|c| (rgb[i][c] - p.rgb[l[i] as usize][c]).powi(2) as f64)
                    .sum::<f64>();
                if i % 8 < 7 && l[i] != l[i + 1] {
                    total += penalty;
                }
                if i / 8 < 7 && l[i] != l[i + 8] {
                    total += penalty;
                }
            }
            total
        };
        let before = energy(&lab);
        labels(&rgb, &mut lab, 8, 8, &p, sigma);
        assert!(energy(&lab) < before);
    }
}
