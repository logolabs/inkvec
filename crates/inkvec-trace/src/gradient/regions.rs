//! Gradient regions: fitting a gradient to the whole region an artist filled with one,
//! rather than to whichever pair of palette bands happens to be adjacent.

use std::collections::HashMap;

/// Whether the region-level gradient recovery is on (`INKVEC_GRAD_REGIONS=1`).
pub(crate) fn enabled() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("INKVEC_GRAD_REGIONS").is_ok_and(|v| v != "0"))
}

/// Largest colour difference (CIEDE2000) between the inks of two *flat* adjacent regions
/// for them to be tried as bands of one ramp. Overridable with `INKVEC_RAMP_STEP`.
pub(crate) static RAMP_STEP_DE00: std::sync::LazyLock<f32> = std::sync::LazyLock::new(|| {
    std::env::var("INKVEC_RAMP_STEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15.0)
});

/// Largest colour step (CIE76, Lab units) between two 4-neighbouring pixels for the pair
/// to count as inside one smooth region. Overridable with `INKVEC_SMOOTH_STEP`.
///
/// A seam between two bands of one ramp is crossed in steps of the ramp's slope, a pixel
/// at a time; a seam between two flat regions is crossed in one anti-aliased pixel, so
/// at least one of the two pixel pairs across it steps half the contrast or more.
static SMOOTH_STEP: std::sync::LazyLock<f32> = std::sync::LazyLock::new(|| {
    std::env::var("INKVEC_SMOOTH_STEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3.0)
});

/// Fraction of a seam's pixel pairs that must be smooth steps for the seam to be one
/// inside a region. Overridable with `INKVEC_SMOOTH_FRACTION`.
static SMOOTH_FRACTION: std::sync::LazyLock<f64> = std::sync::LazyLock::new(|| {
    std::env::var("INKVEC_SMOOTH_FRACTION")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.5)
});

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
    shared > 0 && calm as f64 >= *SMOOTH_FRACTION * shared as f64
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

/// The sets of live regions connected by `linked` seams, each sorted, ids ascending.
pub(crate) fn ramp_sets(
    alive: &[bool],
    adj: &[HashMap<u32, u32>],
    linked: impl Fn(usize, usize) -> bool,
) -> Vec<Vec<u32>> {
    let n = alive.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for a in 0..n {
        if !alive[a] {
            continue;
        }
        let mut nb: Vec<u32> = adj[a].keys().copied().filter(|&b| b as usize > a).collect();
        nb.sort_unstable();
        for b in nb {
            let b = b as usize;
            if alive[b] && linked(a, b) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                if ra != rb {
                    parent[ra.max(rb)] = ra.min(rb);
                }
            }
        }
    }
    let mut sets: HashMap<usize, Vec<u32>> = HashMap::new();
    for a in 0..n {
        if alive[a] {
            let r = root(&mut parent, a);
            sets.entry(r).or_default().push(a as u32);
        }
    }
    let mut out: Vec<Vec<u32>> = sets.into_values().filter(|s| s.len() > 1).collect();
    out.sort();
    out
}

/// What fitting the pixels `pixels` with one fill `union` instead of their current fills
/// (`split(p)` is the colour the current fill predicts at pixel `p`) saves, both priced on
/// the same evidence pixels: `0.5 * chi2` difference plus `lambda` times the parameters
/// released. `None` when there is no evidence to price on.
#[allow(clippy::too_many_arguments)]
pub(crate) fn set_gain(
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pixels: &[usize],
    split: &(dyn Fn(usize) -> [f32; 3] + Sync),
    member: &(dyn Fn(usize) -> bool + Sync),
    evidence: &(dyn Fn(usize) -> bool + Sync),
    union: &super::FillFit,
    split_params: f64,
    sigma: f64,
    lambda: f64,
) -> Option<f64> {
    let s = super::collect_samples(rgb, w, h, pixels, member, evidence, true);
    if s.len() == 0 {
        return None;
    }
    let stride = (s.len() / super::fit_cap()).max(1);
    let (mut diff, mut used) = (0.0f64, 0usize);
    for i in (0..s.len()).step_by(stride) {
        let old = split(s.px[i]);
        let new = union.model.color_at(s.x[i], s.y[i]);
        for c in 0..3 {
            let o = ((s.srgb[i][c] - old[c]).abs() as f64 - super::QUANT_HALF_STEP).max(0.0);
            let n = ((s.srgb[i][c] - new[c]).abs() as f64 - super::QUANT_HALF_STEP).max(0.0);
            diff += o * o - n * n;
        }
        used += 1;
    }
    let gain = 0.5 * diff / (sigma * sigma) * s.len() as f64 / used as f64
        + lambda * (split_params - union.params);
    gain.is_finite().then_some(gain)
}

/// Narrowest a gradient segment may be, in pixels, when it carries most of the gradient's
/// colour change. Overridable with `INKVEC_EDGE_PX`.
static EDGE_PX: std::sync::LazyLock<f64> = std::sync::LazyLock::new(|| {
    std::env::var("INKVEC_EDGE_PX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3.0)
});

/// Whether a gradient is an edge wearing a gradient's clothes: flat, then most of its
/// colour change within a few pixels, then flat. With interior stops a gradient can draw
/// any soft step, and two flat regions whose edge the intake blurred (a JPEG's halved
/// chroma, an upscale) are then fitted as one "gradient" that is really their edge.
pub(crate) fn edge_like(m: &super::FillModel) -> bool {
    use super::FillModel;
    let (scale, c0, mids, c1) = match m {
        FillModel::Flat(_) => return false,
        FillModel::Linear {
            p0,
            p1,
            c0,
            c1,
            mids,
            ..
        } => (
            ((p1.0 - p0.0).powi(2) + (p1.1 - p0.1).powi(2)).sqrt(),
            c0,
            mids,
            c1,
        ),
        FillModel::Radial {
            r,
            aspect,
            c0,
            c1,
            mids,
            ..
        } => (r / aspect.sqrt(), c0, mids, c1),
    };
    if mids.is_empty() {
        return false;
    }
    let mut stops: Vec<(f64, [f32; 3])> = vec![(0.0, *c0)];
    stops.extend(mids.iter().copied());
    stops.push((1.0, *c1));
    let (mut total, mut steep) = (0.0f64, (0.0f64, 0.0f64));
    for s in stops.windows(2) {
        let d = (0..3)
            .map(|k| ((s[1].1[k] - s[0].1[k]) as f64).powi(2))
            .sum::<f64>()
            .sqrt();
        total += d;
        if d > steep.0 {
            steep = (d, (s[1].0 - s[0].0) * scale);
        }
    }
    total > 0.0 && steep.0 >= 0.7 * total && steep.1 <= *EDGE_PX
}

/// Remove the edge-like gradients (see [`edge_like`]) from a candidate list.
pub(crate) fn drop_edges(cands: &mut Vec<super::FillFit>) {
    cands.retain(|f| !edge_like(&f.model));
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
    fn a_narrow_step_between_plateaus_is_an_edge() {
        use crate::gradient::{FillModel, Interp};
        let lin = |mids: Vec<(f64, [f32; 3])>, len: f64| FillModel::Linear {
            p0: (0.0, 0.0),
            p1: (len, 0.0),
            c0: [0.2; 3],
            c1: [0.8; 3],
            interp: Interp::Srgb,
            mids,
        };
        // Flat to 0.48, the whole change by 0.52, flat after: 1.6 px on a 40 px axis.
        let step = vec![(0.48, [0.21; 3]), (0.52, [0.79; 3])];
        assert!(edge_like(&lin(step.clone(), 40.0)));
        // The same profile drawn over 400 px ramps over 16 px: a gradient.
        assert!(!edge_like(&lin(step, 400.0)));
        // A plateau then a long ramp.
        assert!(!edge_like(&lin(vec![(0.5, [0.2; 3])], 40.0)));
        assert!(!edge_like(&lin(Vec::new(), 2.0)));
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
