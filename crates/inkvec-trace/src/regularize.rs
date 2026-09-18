//! Spatial label regularization for explicitly lossy rasters.
//!
//! Reduces the sum of colour residual and a boundary-length cost. Unlike an area
//! cutoff, strong one-pixel features can pay for their boundaries and survive.
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
pub fn residual_incoherence(
    rgb: &[[f32; 3]],
    labels: &[u16],
    w: usize,
    h: usize,
    pal: &Palette,
) -> f64 {
    /// Root of the sum of squared coefficients of the 4-neighbour Laplacian, which is the gain
    /// it applies to independent noise. Derived, not written down, for the reason
    /// `coverage::estimate_noise` records at length.
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
