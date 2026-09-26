//! Gradient regions: fitting a gradient to the whole region an artist filled with one,
//! rather than to whichever pair of palette bands happens to be adjacent.

use std::collections::HashMap;

/// Whether the region-level gradient recovery is on: by default, off with
/// `INKVEC_GRAD_REGIONS=0`.
pub(crate) fn enabled() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| inkvec_core::env::switch("INKVEC_GRAD_REGIONS", true))
}

/// Largest colour difference (CIEDE2000) between the inks of two *flat* adjacent regions
/// for them to be tried as bands of one ramp (was `INKVEC_RAMP_STEP`).
pub(crate) const RAMP_STEP_DE00: f32 = 15.0;

/// Largest colour step (CIE76, Lab units) between two 4-neighbouring pixels for the pair
/// to count as inside one smooth region (was `INKVEC_SMOOTH_STEP`).
///
/// A seam between two bands of one ramp is crossed in steps of the ramp's slope, a pixel
/// at a time; a seam between two flat regions is crossed in one anti-aliased pixel, so
/// at least one of the two pixel pairs across it steps half the contrast or more.
const SMOOTH_STEP: f32 = 3.0;

/// Fraction of a seam's pixel pairs that must be smooth steps for the seam to be one
/// inside a region (was `INKVEC_SMOOTH_FRACTION`).
const SMOOTH_FRACTION: f64 = 0.5;

/// Whether the step from pixel colour `p` to `q` (sRGB) is small enough to lie inside a
/// smooth region.
pub(crate) fn smooth_step(p: [f32; 3], q: [f32; 3]) -> bool {
    let (a, b) = (crate::color::srgb_to_lab(p), crate::color::srgb_to_lab(q));
    let d2 = (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
    d2 < SMOOTH_STEP.powi(2)
}

/// Whether the seam between regions `a` and `b` is mostly smooth steps.
pub(crate) fn is_smooth(
    adj: &[HashMap<u32, u32>],
    smooth: &[HashMap<u32, u32>],
    a: usize,
    b: usize,
) -> bool {
    let shared = adj[a].get(&(b as u32)).copied().unwrap_or(0);
    let calm = smooth[a].get(&(b as u32)).copied().unwrap_or(0);
    shared > 0 && calm as f64 >= SMOOTH_FRACTION * shared as f64
}

/// Region `b` was absorbed into `a`: move `b`'s seam counts onto `a`.
pub(crate) fn absorb_counts(counts: &mut [HashMap<u32, u32>], a: usize, b: usize) {
    let taken = std::mem::take(&mut counts[b]);
    let mut taken: Vec<_> = taken.into_iter().collect();
    taken.sort_by_key(|&(c, _)| c);
    for (c, k) in taken {
        if c as usize == a {
            continue;
        }
        *counts[a].entry(c).or_insert(0) += k;
        let e = &mut counts[c as usize];
        e.remove(&(b as u32));
        *e.entry(a as u32).or_insert(0) += k;
    }
    counts[a].remove(&(b as u32));
}

/// Whether a pixel's recorded blend partners (see [`super::blend_partners`]) all satisfy
/// `inside`: a blend is evidence for a fit only when nothing outside the fit could have
/// made it. An anti-aliased pixel on a grey ramp's outline against white lies on the
/// segment between two of the ramp's inks as well as on the one to white.
pub(crate) fn all_inside(partners: &[u32], inside: impl Fn(usize) -> bool) -> bool {
    partners[0] != super::FOREIGN
        && partners
            .iter()
            .take_while(|&&q| q != super::PURE)
            .all(|&q| inside(q as usize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{rgb_to_oklab, Palette};

    fn palette(rgb: Vec<[f32; 3]>) -> Palette {
        Palette {
            colors: rgb.iter().map(|&c| rgb_to_oklab(c)).collect(),
            weight: vec![1.0; rgb.len()],
            alpha: vec![1.0; rgb.len()],
            rgb,
        }
    }

    fn q8(v: f32) -> f32 {
        (v * 255.0).round() / 255.0
    }

    /// Distinct gradient fills among the labels the pixels ended up with.
    fn gradients(labels: &[u16], fills: &[crate::gradient::FillFit]) -> usize {
        let mut seen: Vec<u16> = labels
            .iter()
            .copied()
            .filter(|&l| fills[l as usize].model.is_gradient())
            .collect();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    }

    /// A horizontal ramp quantised into two-pixel bands: every band is too thin to have an
    /// interior, so each is flat, and only region recovery joins them into one gradient.
    #[test]
    fn thin_flat_bands_of_a_ramp_become_one_gradient() {
        let (w, h, band) = (40usize, 20usize, 2usize);
        let n_bands = w / band;
        let ramp = |x: usize| 0.25 + 0.5 * x as f32 / (w - 1) as f32;
        let rgb: Vec<[f32; 3]> = (0..w * h).map(|p| [q8(ramp(p % w)); 3]).collect();
        let inks: Vec<[f32; 3]> = (0..n_bands)
            .map(|b| [q8(ramp(b * band) * 0.5 + ramp(b * band + 1) * 0.5); 3])
            .collect();
        let pal = palette(inks);
        let base: Vec<u16> = (0..w * h).map(|p| ((p % w) / band) as u16).collect();
        let (sigma, lambda) = (0.5 / 255.0, crate::gradient::bic_lambda(w * h));

        let mut off = base.clone();
        let (fills, _) = crate::gradient::bands::merge_bands_with(
            &mut off, &rgb, w, h, &pal, sigma, lambda, None, None, false,
        );
        assert_eq!(
            gradients(&off, &fills),
            0,
            "the legacy merge never joins flat bands"
        );

        let mut on = base.clone();
        let (fills, _) = crate::gradient::bands::merge_bands_with(
            &mut on, &rgb, w, h, &pal, sigma, lambda, None, None, true,
        );
        assert_eq!(gradients(&on, &fills), 1);
        let first = on[0];
        assert!(on.iter().all(|&l| l == first), "one region covers the ramp");
    }

    /// Two flat inks meeting at an anti-aliased edge stay two flat regions.
    #[test]
    fn an_antialiased_edge_between_flats_is_not_a_ramp() {
        let (w, h) = (40usize, 20usize);
        let (a, b) = (0.30f32, 0.55f32);
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|p| {
                let x = p % w;
                let v = if x < 20 {
                    a
                } else if x == 20 {
                    q8(0.5 * (a + b))
                } else {
                    b
                };
                [q8(v); 3]
            })
            .collect();
        let pal = palette(vec![[q8(a); 3], [q8(b); 3]]);
        let mut labels: Vec<u16> = (0..w * h).map(|p| u16::from(p % w > 20)).collect();
        let (sigma, lambda) = (0.5 / 255.0, crate::gradient::bic_lambda(w * h));
        let (fills, _) = crate::gradient::bands::merge_bands_with(
            &mut labels,
            &rgb,
            w,
            h,
            &pal,
            sigma,
            lambda,
            None,
            None,
            true,
        );
        assert_eq!(gradients(&labels, &fills), 0);
        assert_ne!(labels[0], labels[w - 1]);
    }

    #[test]
    fn smooth_steps_and_seams() {
        assert!(smooth_step([0.50; 3], [0.51; 3]));
        assert!(!smooth_step([0.30; 3], [0.42; 3]));
        let mut adj = vec![HashMap::new(); 3];
        let mut calm = vec![HashMap::new(); 3];
        adj[0].insert(1, 10);
        adj[1].insert(0, 10);
        calm[0].insert(1, 6);
        calm[1].insert(0, 6);
        assert!(is_smooth(&adj, &calm, 0, 1));
        calm[0].insert(1, 4);
        assert!(!is_smooth(&adj, &calm, 0, 1));
        assert!(!is_smooth(&adj, &calm, 0, 2));
    }

    #[test]
    fn absorbed_counts_move_to_the_survivor() {
        let mut c = vec![HashMap::new(); 3];
        for (a, b, k) in [(0u32, 1u32, 2u32), (1, 2, 5), (0, 2, 1)] {
            c[a as usize].insert(b, k);
            c[b as usize].insert(a, k);
        }
        absorb_counts(&mut c, 0, 1);
        assert_eq!(c[0].get(&2), Some(&6));
        assert_eq!(c[2].get(&0), Some(&6));
        assert!(c[0].get(&1).is_none() && c[1].is_empty() && c[2].get(&1).is_none());
    }

    #[test]
    fn a_blend_is_inner_only_when_every_partner_is_inside() {
        let p = super::super::PURE;
        assert!(all_inside(&[p, p, p], |_| false));
        assert!(all_inside(&[4, 7, p], |q| q == 4 || q == 7));
        assert!(!all_inside(&[4, 9, p], |q| q == 4 || q == 7));
        assert!(!all_inside(&[super::super::FOREIGN; 3], |_| true));
    }
}
