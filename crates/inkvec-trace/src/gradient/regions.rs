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
