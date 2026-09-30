//! Transparency carried natively: an ink is a colour *and* an opacity, and the transparent
//! ground is an ink like any other.
//!
//! The classic path composites the image onto a white matte and traces what is left, so a
//! white mark on a transparent ground is one flat colour, a translucent wash is baked into
//! whatever it resembles, and a glow becomes a colour ramp towards the matte. Here nothing
//! is thrown away: every pixel is kept as its colour over white `W` together with its alpha
//! `a`. That pair is an invertible transform of premultiplied RGBA (`W = P + (1 − a)` with
//! `P = s·a` the premultiplied colour), so an anti-aliased pixel between two inks is still
//! a straight-line blend of them in all four channels -- between two opaque inks, between
//! an ink and the clear ground, or at a junction of both -- and every stage that unmixes
//! linearly needs one more channel and nothing else.
//!
//! Perceptual comparisons use the colour over two grounds: `W` over white and
//! `K = W − (1 − a)(1 − g)` over a second, grey ground `g` ([`SECOND_GROUND`], mid-grey;
//! the names `k` and [`over_black`] date from when that ground was black). Two inks are
//! one ink only if they look the same over both. For an opaque colour `K = W` and this is
//! plain OKLab distance; white paint and the clear ground are as far apart as white and
//! mid-grey.
//!
//! Opaque input never reaches this module. `trace_color_full_with_alpha` dispatches here
//! only when the caller asked for native alpha and the source has transparency, so the
//! classic path is untouched by construction.
//!
//! # Mirrors of the classic path, and why they are forks
//!
//! Most of this module is a four-channel (or two-ground) copy of a classic function:
//!
//! | here | classic | what changes |
//! |---|---|---|
//! | [`trace_color`] | [`crate::trace_color_full_with_alpha`] | the stages below, plus `merge_fades` (in `native/fade.rs`) |
//! | [`extract_palette`] | [`color::extract_palette_mdl`] | [`Ink2`] points; the clear ink is not counted against `max_colors`; a translucent candidate needs an interior |
//! | [`label_image`] | [`color::label_image`] | [`Ink2::dist`] |
//! | `frequency_modes` | `color::frequency_modes` | bins over both grounds, `u64` keys, hash map |
//! | `claim_spread` | `color::claim_spread` | [`Ink2::dist`] |
//! | `interior_fraction` | `color::interior_fraction` | [`Ink2::dist`] |
//! | `blend_pairs` | `color::blend_pairs` | six coordinates ([`six`]) |
//! | `straddle_fraction` | `color::straddle_fraction` | six coordinates |
//! | `BlendEvidence` | `color::BlendEvidence` | the translucent-interior rule |
//! | `refine_to_members` | `color::refine_to_members` | means over both grounds |
//! | [`mixture`] | [`crate::regions::mixture`] | `N` channels |
//! | [`absorb_blend_slivers`] | [`crate::regions::absorb_blend_slivers`] | `[W, a]`; [`CLEAR`] for the white backdrop |
//! | [`reassign_blend_pixels`] | [`crate::regions::reassign_blend_pixels`] | `[W, a]`; [`CLEAR`] for the white backdrop |
//!
//! They are deliberately not merged into generic code. The classic functions are the
//! shipped default and are held byte-identical by the gate; making them generic over
//! channel count and distance would put every opaque trace at the mercy of a change made
//! for transparency, and would cost the opaque path the two-ground conversions it does not
//! need. Keeping the fork means a change here cannot move an opaque result. The price is
//! that a fix to one side has to be considered for the other; the table above is the list
//! to check. Only the colour-free label-graph helpers (connected components and contact
//! counts) are shared, from [`crate::regions`].

use std::collections::HashMap;

use rayon::prelude::*;

use crate::color::{
    self, de00, linear_to_srgb, oklab_to_rgb, rgb_to_oklab, srgb_to_linear, Oklab, Palette,
    PaletteEvidence, BLEND_INTERIOR_FRACTION, BLEND_STRADDLE_FRACTION, JND_FLOOR, MIN_INK_WEIGHT,
    PARAMS_PER_INK,
};
use crate::regions::{label_components, tally_contacts};
use crate::{coverage, gradient, regularize, ColorOptions, ColorTrace, Rgba, Stopwatch};

mod fade;
use fade::merge_fades;
pub use fade::Fade;
#[cfg(test)]
use fade::{alpha_params, fade_chi2, fit_colour_stops, fit_opacity, model_stops, solve};

/// Alpha at or above which a pixel or an ink is opaque.
pub const OPAQUE: f32 = 0.999;

/// A colour seen over both grounds: `w` over white, `k` over the second ground
/// ([`SECOND_GROUND`]), in OKLab. For an opaque colour the two are the same point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink2 {
    /// Over white.
    pub w: Oklab,
    /// Over the second ground (historically black, now [`SECOND_GROUND`]).
    pub k: Oklab,
}

impl Ink2 {
    /// An opaque colour: the same point over either ground.
    pub fn opaque(w: Oklab) -> Self {
        Ink2 { w, k: w }
    }

    /// The larger of the two grounds' OKLab distances. Plain OKLab distance when both
    /// colours are opaque.
    ///
    /// The maximum, not a sum or a mean, because the question is "can the two be told
    /// apart over *some* ground": white paint and the clear ground are identical over
    /// white and far apart over grey, and the distance must say they differ.
    #[inline]
    pub fn dist(self, o: Ink2) -> f32 {
        self.w.dist(o.w).max(self.k.dist(o.k))
    }

    /// CIEDE2000 over whichever ground tells the two apart better.
    pub fn de00(self, o: Ink2) -> f32 {
        de00(oklab_to_rgb(self.w), oklab_to_rgb(o.w))
            .max(de00(oklab_to_rgb(self.k), oklab_to_rgb(o.k)))
    }

    /// The opacity the two grounds imply: over white and over the second ground differ by
    /// `(1 - a)(1 - g)`.
    ///
    /// Solved per channel and averaged: `a = 1 − mean_c(W_c − K_c) / (1 − g)`, with `W`,
    /// `K` converted back to sRGB `[0, 1]`, then [`snap_alpha`]. Exact for a colour built
    /// by [`over_black`] unless that clamped a channel at 0.
    pub fn alpha(self) -> f32 {
        let (w, k) = (oklab_to_rgb(self.w), oklab_to_rgb(self.k));
        let d = ((w[0] - k[0]) + (w[1] - k[1]) + (w[2] - k[2])) / 3.0;
        snap_alpha(1.0 - d / (1.0 - second_ground()))
    }
}

/// The second ground colours are compared over, as a grey level. Black separates white
/// paint from the clear ground as well as anything can, but OKLab's lightness is a cube
/// root and near black it is stretched: a 2 % fringe over black sits 0.12 from black, three
/// merge radii, and every faint anti-aliased pixel read as an ink of its own. Mid-grey still
/// puts white paint (white) and the ground (grey) far apart, and a faint fringe next to the
/// ground where it belongs. (It was `INKVEC_NATIVE_GROUND`; nothing set it.)
pub const SECOND_GROUND: f32 = 0.5;

/// [`SECOND_GROUND`].
pub fn second_ground() -> f32 {
    SECOND_GROUND
}

/// Opacity pinned to exactly 0 or 1 when it is within measurement of either.
///
/// Clamped to `[0, 1]` first; above 0.995 becomes 1 and below 0.005 becomes 0, which is
/// about one 8-bit level either side.
pub fn snap_alpha(a: f32) -> f32 {
    let a = a.clamp(0.0, 1.0);
    if a > 0.995 {
        1.0
    } else if a < 0.005 {
        0.0
    } else {
        a
    }
}

/// The colour over the second ground (see [`second_ground`]), from the colour over white
/// and the alpha: `W - (1 - a)(1 - g)`.
///
/// Derivation: a colour `s` at opacity `a` shows `s·a + (1 − a)·G` over a ground `G`, so
/// over white `W = s·a + (1 − a)` and over grey `g` it is `W − (1 − a)(1 − g)`. sRGB
/// `[0, 1]` in and out; `a` is clamped to `[0, 1]` and each channel floored at 0.
#[inline]
pub fn over_black(w: [f32; 3], a: f32) -> [f32; 3] {
    let m = (1.0 - a.clamp(0.0, 1.0)) * (1.0 - second_ground());
    [
        (w[0] - m).max(0.0),
        (w[1] - m).max(0.0),
        (w[2] - m).max(0.0),
    ]
}

/// Every pixel as a two-ground point.
///
/// `rgb` is the image composited onto white (sRGB `[0, 1]`), `alpha` the source alpha.
/// Opaque pixels (`a ≥ OPAQUE`) skip the second conversion and get `k = w`.
pub fn pixel_points(rgb: &[[f32; 3]], alpha: &[f32]) -> Vec<Ink2> {
    rgb.par_iter()
        .zip(alpha.par_iter())
        .map(|(&c, &a)| {
            let w = rgb_to_oklab(c);
            if a >= OPAQUE {
                Ink2::opaque(w)
            } else {
                Ink2 {
                    w,
                    k: rgb_to_oklab(over_black(c, a)),
                }
            }
        })
        .collect()
}

/// Every palette entry as a two-ground point, from its colour over white and its opacity
/// (1 when the palette has no opacity for it).
pub fn ink_points(pal: &Palette) -> Vec<Ink2> {
    (0..pal.len())
        .map(|i| {
            let w = pal.colors[i];
            let a = pal.alpha.get(i).copied().unwrap_or(1.0);
            if a >= OPAQUE {
                Ink2::opaque(w)
            } else {
                Ink2 {
                    w,
                    k: rgb_to_oklab(over_black(pal.rgb[i], a)),
                }
            }
        })
        .collect()
}

/// Each pixel as `[W, a]`, the four channels every linear stage works in.
///
/// `W` is sRGB `[0, 1]` over white, `a` the alpha clamped to `[0, 1]`.
pub fn rgba_w(rgb: &[[f32; 3]], alpha: &[f32]) -> Vec<[f32; 4]> {
    rgb.iter()
        .zip(alpha)
        .map(|(c, &a)| [c[0], c[1], c[2], a.clamp(0.0, 1.0)])
        .collect()
}

/// Each palette entry as `[W, a]` (opacity 1 when the palette has none for it).
pub fn ink_rgba_w(pal: &Palette) -> Vec<[f32; 4]> {
    (0..pal.len())
        .map(|i| {
            let c = pal.rgb[i];
            [c[0], c[1], c[2], pal.alpha.get(i).copied().unwrap_or(1.0)]
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------------------

/// Bins per OKLab axis, as in the classic palette.
const BINS: usize = 24;

/// A colour's OKLab bin, `L_i · 24² + a_i · 24 + b_i`, with the classic palette's ranges
/// (`L` over `[0, 1]`, `a` and `b` over `[−0.4, 0.4]`, clamped).
fn bin(c: Oklab) -> u64 {
    let li = ((c.l.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u64).min(BINS as u64 - 1);
    let ai = (((c.a + 0.4) / 0.8).clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u64;
    let bi = (((c.b + 0.4) / 0.8).clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u64;
    li * (BINS * BINS) as u64 + ai * BINS as u64 + bi
}

/// [`color`]'s `claim_spread` with two-ground distances: how many pixels `c` would claim
/// (strictly nearer to it than to any accepted ink, counted every `stride_px` pixels and
/// scaled back up), and the median [`Ink2::dist`] of the claimed pixels within `tol` of it.
fn claim_spread(
    px: &[Ink2],
    nearest_px: &[f32],
    c: Ink2,
    tol: f32,
    stride_px: usize,
) -> (usize, f32) {
    const MAX_SAMPLES: usize = 8192;
    let stride = (px.len() / MAX_SAMPLES).max(1);
    // One pass for both the territory count and the spread sample: they test the same
    // distance at the same pixels.
    let claimed: Vec<(usize, f32)> = (0..px.len())
        .into_par_iter()
        .step_by(stride_px)
        .with_min_len(color::PAR_MIN_LEN)
        .filter_map(|i| {
            let d = px[i].dist(c);
            (d < nearest_px[i]).then_some((i, d))
        })
        .collect();
    let n = claimed.len();
    let mut d_in: Vec<f32> = claimed
        .iter()
        .filter(|&&(i, d)| d < tol && i % stride == 0)
        .map(|&(_, d)| d)
        .collect();
    let n = n * stride_px;
    if d_in.is_empty() {
        return (n, 0.0);
    }
    d_in.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (n, d_in[d_in.len() / 2])
}

/// [`color`]'s `interior_fraction` with two-ground distances: the share of the pixels `c`
/// would claim whose four in-image neighbours it would claim too (one step of erosion).
/// 1 when the geometry is missing, 0 when `c` claims nothing.
fn interior_fraction(
    px: &[Ink2],
    width: usize,
    height: usize,
    c: Ink2,
    nearest: &[f32],
    stride_px: usize,
) -> f32 {
    if width == 0 || height == 0 || px.len() < width * height {
        return 1.0;
    }
    // Evaluated only where the reduction reads it: the sampled pixels and their four
    // neighbours. Building the whole-image mask first cost a full pass and an allocation
    // per call for a candidate whose territory is usually a thin band.
    let mask = |i: usize| px[i].dist(c) < nearest[i];
    let (total, interior) = (0..width * height)
        .into_par_iter()
        .step_by(stride_px)
        .with_min_len(color::PAR_MIN_LEN)
        .filter(|&i| mask(i))
        .map(|i| {
            let (x, y) = (i % width, i / width);
            let ok = (x == 0 || mask(i - 1))
                && (x + 1 == width || mask(i + 1))
                && (y == 0 || mask(i - width))
                && (y + 1 == height || mask(i + width));
            (1u32, ok as u32)
        })
        .reduce(|| (0u32, 0u32), |a, b| (a.0 + b.0, a.1 + b.1));
    if total == 0 {
        return 0.0;
    }
    interior as f32 / total as f32
}

/// A two-ground point's six blend coordinates: the colour over white and over black, in
/// linear light or in sRGB.
///
/// `[W_r, W_g, W_b, K_r, K_g, K_b]`. Both halves are affine in `(P, a)`, so a coverage
/// blend of two inks is a straight segment in these six numbers, which is what the chord
/// tests need.
fn six(c: Ink2, linear: bool) -> [f32; 6] {
    let (w, k) = (oklab_to_rgb(c.w), oklab_to_rgb(c.k));
    let f = |v: f32| if linear { srgb_to_linear(v) } else { v };
    [f(w[0]), f(w[1]), f(w[2]), f(k[0]), f(k[1]), f(k[2])]
}

/// The inverse of [`six`]: clamp each coordinate to `[0, 1]`, encode if `linear`, and
/// convert both halves back to OKLab.
fn from_six(q: [f32; 6], linear: bool) -> Ink2 {
    let f = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        if linear {
            linear_to_srgb(v)
        } else {
            v
        }
    };
    Ink2 {
        w: rgb_to_oklab([f(q[0]), f(q[1]), f(q[2])]),
        k: rgb_to_oklab([f(q[3]), f(q[4]), f(q[5])]),
    }
}

/// [`color`]'s blend test in six dimensions: `c` lies on the chord between two accepted
/// inks over white *and* over black. An anti-aliased rim between an ink and the clear ground
/// is on such a chord (flat over white for a white ink, a ramp to black over black).
///
/// Same formula as the classic `blend_pairs` with `p`, `A`, `B` the [`six`] coordinates:
/// `t = ((p − A) · (B − A)) / |B − A|²` kept for `tmin ≤ t ≤ 1 − tmin`, and the residual
/// `off = Ink2::dist(c, from_six(A + t (B − A)))`, kept when `off ≤ tol`. Returns
/// `(i, j, linear, off)`, linear-light pairs first.
fn blend_pairs(c: Ink2, accepted: &[Ink2], tol: f32, tmin: f32) -> Vec<(usize, usize, bool, f32)> {
    let mut out = Vec::new();
    if accepted.len() < 2 {
        return out;
    }
    for space in 0..2 {
        let linear = space == 0;
        let p = six(c, linear);
        for i in 0..accepted.len() {
            for j in i + 1..accepted.len() {
                let (a, b) = (six(accepted[i], linear), six(accepted[j], linear));
                let mut dd = 0.0f32;
                let mut dot = 0.0f32;
                for k in 0..6 {
                    let d = b[k] - a[k];
                    dd += d * d;
                    dot += (p[k] - a[k]) * d;
                }
                if dd < 1e-9 {
                    continue;
                }
                let t = dot / dd;
                if !(tmin..=1.0 - tmin).contains(&t) {
                    continue;
                }
                let mut q = [0.0f32; 6];
                for k in 0..6 {
                    q[k] = a[k] + (b[k] - a[k]) * t;
                }
                let off = c.dist(from_six(q, linear));
                if off <= tol {
                    out.push((i, j, linear, off));
                }
            }
        }
    }
    out
}

/// [`color`]'s `straddle_fraction` in six coordinates: the share of the pixels `c` would
/// claim that have, within their 3x3 neighbourhood, one pixel further towards `a` and one
/// further towards `b` along the `a`–`b` axis than `c` is (steps as in the classic version,
/// [`color::STRADDLE_STEP`] clipped to half the room on each side, floor 0.02).
///
/// `px6_srgb` and `px6_lin` are every pixel's [`six`] coordinates, precomputed. Returns 0
/// on a size mismatch or a degenerate axis, 1 when `c` claims nothing.
#[allow(clippy::too_many_arguments)]
fn straddle_fraction(
    px: &[Ink2],
    px6_srgb: &[[f32; 6]],
    px6_lin: &[[f32; 6]],
    width: usize,
    height: usize,
    c: Ink2,
    nearest: &[f32],
    a: Ink2,
    b: Ink2,
    linear: bool,
    stride_px: usize,
) -> f32 {
    if width == 0 || height == 0 || px.len() < width * height {
        return 0.0;
    }
    let (pa, pb) = (six(a, linear), six(b, linear));
    let mut d = [0.0f32; 6];
    let mut dd = 0.0f32;
    for k in 0..6 {
        d[k] = pb[k] - pa[k];
        dd += d[k] * d[k];
    }
    if dd < 1e-9 {
        return 0.0;
    }
    let t_of = |p: &[f32; 6]| -> f32 {
        let mut s = 0.0;
        for k in 0..6 {
            s += (p[k] - pa[k]) * d[k];
        }
        s / dd
    };
    let tc = t_of(&six(c, linear));
    let step_lo = color::STRADDLE_STEP.min(0.5 * tc).max(0.02);
    let step_hi = color::STRADDLE_STEP.min(0.5 * (1.0 - tc)).max(0.02);
    // Both evaluated only where the reduction reads them -- see `interior_fraction`. This
    // was the palette stage's largest cost on gradient art with transparency: two
    // whole-image passes and allocations per candidate pair, 237 calls and 4.6 s on one
    // 512 px noto-emoji, for values read at a thin band of sampled pixels.
    let cache = if linear { px6_lin } else { px6_srgb };
    let (total, straddle) = (0..width * height)
        .into_par_iter()
        .step_by(stride_px)
        .with_min_len(color::PAR_MIN_LEN)
        .filter(|&i| px[i].dist(c) < nearest[i])
        .map(|i| {
            let (x, y) = (i % width, i / width);
            let (mut lower, mut higher) = (false, false);
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                        continue;
                    }
                    let tn = t_of(&cache[ny as usize * width + nx as usize]);
                    lower |= tn < tc - step_lo;
                    higher |= tn > tc + step_hi;
                }
            }
            (1u32, (lower && higher) as u32)
        })
        .reduce(|| (0u32, 0u32), |a, b| (a.0 + b.0, a.1 + b.1));
    if total == 0 {
        return 1.0;
    }
    straddle as f32 / total as f32
}

/// [`color::extract_palette_mdl`], with every colour a two-ground point: the same
/// frequency-ranked mode seeking, rarity floor, perceptual same-ink floor, merge radius,
/// description-length escape and blend tests, asked over white and over black at once.
///
/// The clear ground comes out as an ink of its own -- white over white, black over black,
/// opacity 0 -- and a translucent wash as one with its own opacity, with no alpha splitting
/// after the fact. `Palette::colors`/`rgb` hold each ink over white; `alpha` its opacity.
///
/// Differences from the classic walk, beyond the distance:
///
/// * the clear ink (opacity ≤ [`CLEAR_INK_ALPHA`]) does not count against `max_colors`;
///   once the cap is full the scan continues only to find it, and stops once it is found;
/// * a translucent candidate that is not a blend must have an interior (see
///   `BlendEvidence::measure`);
/// * there is no `INKVEC_MERGE_DE00` experiment and the same-ink floor does not print.
///
/// `rgb` is sRGB `[0, 1]` composited onto white and `alpha` the source alpha, both
/// row-major `width × height`. At least one ink is always returned.
#[allow(clippy::too_many_arguments)]
pub fn extract_palette(
    rgb: &[[f32; 3]],
    alpha: &[f32],
    width: usize,
    height: usize,
    merge_distance: f32,
    max_colors: usize,
    ev: PaletteEvidence,
) -> Palette {
    let PaletteEvidence {
        sigma_noise,
        lambda,
        noise_sigmas,
        same_ink_de00,
    } = ev;
    let view = PixelViews::new(rgb, alpha, width, height);
    let modes = frequency_modes(&view.px);

    let total_px = view.px.len().max(1) as f32;
    let mut colors: Vec<Ink2> = Vec::new();
    let mut nearest_px: Vec<f32> = vec![f32::INFINITY; view.px.len()];
    let paldbg = inkvec_core::env::flag("INKVEC_PALDBG");
    // The clear ground draws nothing, so it is found but not counted against the cap; once
    // the cap is full the scan goes on only to look for it.
    let clear = |c: &Ink2| c.alpha() <= CLEAR_INK_ALPHA;
    for &(n, _key, c) in &modes {
        let full = colors.iter().filter(|p| !clear(p)).count() >= max_colors;
        if full && colors.iter().any(clear) {
            break;
        }
        if full && !clear(&c) {
            continue;
        }
        let (claim, spread) =
            claim_spread(&view.px, &nearest_px, c, merge_distance, view.stride_px);
        if (claim as f32 / total_px) < MIN_INK_WEIGHT && !colors.is_empty() {
            continue;
        }
        let nearest = colors
            .iter()
            .map(|&p| p.dist(c))
            .fold(f32::INFINITY, f32::min);
        let reach = noise_sigmas * spread;
        if same_ink_as_accepted(c, &colors, same_ink_de00) {
            continue;
        }
        if nearest <= merge_distance.max(reach) {
            let worth_it = sigma_noise > 0.0
                && nearest > JND_FLOOR
                && nearest > reach
                && 0.5 * (claim as f64) * ((nearest as f64 / sigma_noise).powi(2))
                    > lambda * PARAMS_PER_INK;
            if !worth_it {
                continue;
            }
        }
        let Some(shape) = BlendEvidence::measure(&view, c, &colors, &nearest_px, merge_distance)
        else {
            continue;
        };
        if paldbg {
            eprintln!(
                "  native cand w={} k={} a={:.3} bin={n} claim={claim} near={nearest:.4} blend={} interior={:.3} straddle={:.3}",
                color::to_hex(oklab_to_rgb(c.w)),
                color::to_hex(oklab_to_rgb(c.k)),
                c.alpha(),
                shape.blend,
                shape.interior,
                shape.straddle
            );
        }
        if shape.is_coverage() {
            continue;
        }
        nearest_px
            .par_iter_mut()
            .zip(view.px.par_iter())
            .for_each(|(d, &q)| *d = d.min(q.dist(c)));
        colors.push(c);
    }
    if colors.is_empty() {
        colors.push(modes.first().map(|m| m.2).unwrap_or(Ink2::opaque(Oklab {
            l: 1.0,
            a: 0.0,
            b: 0.0,
        })));
    }

    let weight = refine_to_members(&view.px, &mut colors, merge_distance, total_px);
    let alpha: Vec<f32> = colors.iter().map(|c| c.alpha()).collect();
    if paldbg {
        for (c, a) in colors.iter().zip(&alpha) {
            eprintln!(
                "  native ink {} alpha {a:.3}",
                color::to_hex(oklab_to_rgb(c.w))
            );
        }
    }
    Palette {
        colors: colors.iter().map(|c| c.w).collect(),
        rgb: colors.iter().map(|c| oklab_to_rgb(c.w)).collect(),
        weight,
        alpha,
    }
}

/// The image as two-ground points and as [`six`] coordinates in both spaces, converted
/// once for the whole palette walk (the classic `PixelViews` in two grounds).
struct PixelViews {
    px: Vec<Ink2>,
    px6_srgb: Vec<[f32; 6]>,
    px6_lin: Vec<[f32; 6]>,
    width: usize,
    height: usize,
    /// Stride of the statistical passes, from [`color::stat_stride`].
    stride_px: usize,
}

impl PixelViews {
    /// Convert the composited image and its alpha on every core.
    fn new(rgb: &[[f32; 3]], alpha: &[f32], width: usize, height: usize) -> Self {
        let px = pixel_points(rgb, alpha);
        let stride_px = color::stat_stride(px.len(), width);
        let px6_srgb: Vec<[f32; 6]> = px.par_iter().map(|&p| six(p, false)).collect();
        let px6_lin: Vec<[f32; 6]> = px.par_iter().map(|&p| six(p, true)).collect();
        PixelViews {
            px,
            px6_srgb,
            px6_lin,
            width,
            height,
            stride_px,
        }
    }
}

/// The palette candidates: occupied two-ground bins, most populous first.
///
/// A pixel's key is `bin(w) · 24³ + bin(k)`, so two colours share a bin only if they share
/// it over both grounds. Each bin yields `(count, key, mean)`, the mean taken over both
/// grounds in `f64`, in pixel order, so the centroids are the same on every run. Sorted by
/// count descending, then key ascending; keys are unique, so the order is total and does
/// not depend on the hash map's iteration order. (A hash map rather than the classic dense
/// table because there are 24⁶ possible keys.)
fn frequency_modes(px: &[Ink2]) -> Vec<(u32, u64, Ink2)> {
    let keys: Vec<u64> = px
        .par_iter()
        .map(|p| bin(p.w) * (BINS * BINS * BINS) as u64 + bin(p.k))
        .collect();
    let mut acc: HashMap<u64, (u32, [f64; 6])> = HashMap::new();
    for (p, &key) in px.iter().zip(keys.iter()) {
        let e = acc.entry(key).or_insert((0, [0.0; 6]));
        e.0 += 1;
        for (s, v) in
            e.1.iter_mut()
                .zip([p.w.l, p.w.a, p.w.b, p.k.l, p.k.a, p.k.b])
        {
            *s += v as f64;
        }
    }
    let mut modes: Vec<(u32, u64, Ink2)> = acc
        .into_iter()
        .map(|(key, (n, s))| {
            let f = n as f64;
            let m = |i: usize| (s[i] / f) as f32;
            (
                n,
                key,
                Ink2 {
                    w: Oklab {
                        l: m(0),
                        a: m(1),
                        b: m(2),
                    },
                    k: Oklab {
                        l: m(3),
                        a: m(4),
                        b: m(5),
                    },
                },
            )
        })
        .collect();
    modes.sort_by_key(|&(n, key, _)| (std::cmp::Reverse(n), key));
    modes
}

/// The perceptual floor in two grounds: is `c` within `same_ink_de00` ([`Ink2::de00`]) of
/// the accepted ink nearest to it by [`Ink2::dist`]? False when nothing is accepted yet.
fn same_ink_as_accepted(c: Ink2, colors: &[Ink2], same_ink_de00: f32) -> bool {
    colors
        .iter()
        .min_by(|&&p, &&q| {
            p.dist(c)
                .partial_cmp(&q.dist(c))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .is_some_and(|&near_ink| c.de00(near_ink) < same_ink_de00)
}

/// Whether a candidate is anti-aliasing rather than an ink, over both grounds: the
/// classic `BlendEvidence` plus the translucent-interior rule.
struct BlendEvidence {
    blend: bool,
    interior: f32,
    straddle: f32,
}

impl BlendEvidence {
    /// Measure candidate `c` against the accepted inks, or `None` when it is rejected
    /// outright as a translucent band with no interior.
    fn measure(
        view: &PixelViews,
        c: Ink2,
        colors: &[Ink2],
        nearest_px: &[f32],
        merge_distance: f32,
    ) -> Option<Self> {
        let pairs = blend_pairs(c, colors, merge_distance * 1.6, color::BLEND_TMIN);
        let blend = !pairs.is_empty();
        // Partly transparent is exactly what an anti-aliased silhouette pixel is, and the
        // chord test above cannot always say so: a rim where shading meets the ground mixes
        // the clear ink with a shade the palette rejected as a blend itself, so no pair of
        // accepted inks explains it (`noto-emoji/emoji_u1f932` minted two inks at 0.77 from
        // 136 scattered rim pixels). The palette's own rule settles it: an ink covers area,
        // anti-aliasing is a band. A translucent candidate is kept only with an interior.
        //
        // Translucent means any opacity the palette does not round to 0 or 1. The rim just
        // inside an opaque silhouette is 0.95-0.99 opaque, and while this asked only between
        // 0.05 and 0.95 that rim was never asked at all: the mode of those pixels sits a
        // step off black, too far to be the same ink (dE00 over the same-ink floor) and too
        // close to be a blend (inside the chord test's 0.04 end margin), so
        // `simple-icons/sagemath`'s black line graph gained a 0.96 ink along every line
        // (dE00 0.62 -> 0.75 over white; the classic path's bins never form that mode).
        //
        // Blends keep the straddle test below instead. Asking them for an interior as well
        // was tried: on its own it traded noto-emoji for twemoji (0.3722 -> 0.3713 against
        // 0.1414 -> 0.1424), and with the wider range above it cost the translucent set
        // (dark 0.0077 -> 0.0079).
        let translucent = {
            let a = c.alpha();
            a > 0.0 && a < 1.0
        };
        let interior = if blend || translucent {
            interior_fraction(
                &view.px,
                view.width,
                view.height,
                c,
                nearest_px,
                view.stride_px,
            )
        } else {
            1.0
        };
        if translucent && !blend && interior < BLEND_INTERIOR_FRACTION {
            return None;
        }
        // The pairs on every core, as in `color`: the max of finite ratios is order-free.
        let straddle = if blend && interior < BLEND_INTERIOR_FRACTION {
            pairs
                .par_iter()
                .map(|&(i, j, linear, _)| {
                    straddle_fraction(
                        &view.px,
                        &view.px6_srgb,
                        &view.px6_lin,
                        view.width,
                        view.height,
                        c,
                        nearest_px,
                        colors[i],
                        colors[j],
                        linear,
                        view.stride_px,
                    )
                })
                .reduce(|| 0.0f32, f32::max)
        } else {
            0.0
        };
        Some(BlendEvidence {
            blend,
            interior,
            straddle,
        })
    }

    /// A blend, thin and straddling: anti-aliasing, not an ink.
    fn is_coverage(&self) -> bool {
        self.blend
            && self.interior < BLEND_INTERIOR_FRACTION
            && self.straddle >= BLEND_STRADDLE_FRACTION
    }
}

/// Refine each ink to the mean of the pixels that chose it, over both grounds, and return
/// each ink's share of the image.
///
/// A pixel chooses its nearest ink by [`Ink2::dist`] (ties to the lower index) only when
/// that ink is within `merge_distance`. An ink no pixel chose keeps its candidate value
/// and gets weight 0.
fn refine_to_members(
    px: &[Ink2],
    colors: &mut [Ink2],
    merge_distance: f32,
    total_px: f32,
) -> Vec<f32> {
    let chosen: Vec<u32> = px
        .par_iter()
        .map(|c| {
            let mut best = (0usize, f32::MAX);
            for (i, &p) in colors.iter().enumerate() {
                let d = c.dist(p);
                if d < best.1 {
                    best = (i, d);
                }
            }
            if best.1 <= merge_distance {
                best.0 as u32
            } else {
                u32::MAX
            }
        })
        .collect();
    let mut sums = vec![([0.0f64; 6], 0u32); colors.len()];
    for (c, &k) in px.iter().zip(chosen.iter()) {
        if k != u32::MAX {
            let e = &mut sums[k as usize];
            for (s, v) in
                e.0.iter_mut()
                    .zip([c.w.l, c.w.a, c.w.b, c.k.l, c.k.a, c.k.b])
            {
                *s += v as f64;
            }
            e.1 += 1;
        }
    }
    let mut weight = Vec::with_capacity(colors.len());
    for (i, (s, n)) in sums.iter().enumerate() {
        if *n > 0 {
            let f = *n as f64;
            let m = |j: usize| (s[j] / f) as f32;
            colors[i] = Ink2 {
                w: Oklab {
                    l: m(0),
                    a: m(1),
                    b: m(2),
                },
                k: Oklab {
                    l: m(3),
                    a: m(4),
                    b: m(5),
                },
            };
        }
        weight.push(*n as f32 / total_px);
    }
    weight
}

/// Every pixel to its nearest ink, over both grounds.
///
/// The two-ground [`color::label_image`]: nearest by [`Ink2::dist`], ties to the lower
/// index; label 0 for an empty palette.
pub fn label_image(rgb: &[[f32; 3]], alpha: &[f32], pal: &Palette) -> Vec<u16> {
    let px = pixel_points(rgb, alpha);
    let inks = ink_points(pal);
    px.par_iter()
        .map(|&c| {
            let mut best = (0usize, f32::MAX);
            for (i, &p) in inks.iter().enumerate() {
                let d = c.dist(p);
                if d < best.1 {
                    best = (i, d);
                }
            }
            best.0 as u16
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Blend absorption, in four channels
// ---------------------------------------------------------------------------------------

/// The clear ground, as `[W, a]`.
pub const CLEAR: [f32; 4] = [1.0, 1.0, 1.0, 0.0];

/// Opacity at or below which an ink is the clear ground, not a colour (for the colour cap).
pub const CLEAR_INK_ALPHA: f32 = 0.02;

/// Squared Euclidean distance in `N` channels.
fn d2<const N: usize>(a: [f32; N], b: [f32; N]) -> f32 {
    let mut s = 0.0;
    for k in 0..N {
        let e = a[k] - b[k];
        s += e * e;
    }
    s
}

/// [`crate::regions::mixture`] in `N` channels: `c` as the nearest convex mixture of two or
/// three of `cols`, returning the residual and the dominant one.
///
/// The same computation in `N` dimensions: segments by clamped projection, triangles by the
/// 2x2 normal equations with only interior solutions offered, dominant ink by largest
/// weight, first candidate winning ties. With `[W, a]` (`N = 4`) the residual is a
/// Euclidean distance over colour-over-white and alpha, both in `[0, 1]`. `None` with
/// fewer than two inks or when all pairs and triples are degenerate.
pub fn mixture<const N: usize>(c: [f32; N], cols: &[[f32; N]]) -> Option<(f32, usize)> {
    let k = cols.len();
    if k < 2 {
        return None;
    }
    let dot = |a: &[f32; N], b: &[f32; N]| -> f32 {
        let mut s = 0.0;
        for i in 0..N {
            s += a[i] * b[i];
        }
        s
    };
    let sub = |a: [f32; N], b: [f32; N]| -> [f32; N] {
        let mut r = [0.0; N];
        for i in 0..N {
            r[i] = a[i] - b[i];
        }
        r
    };
    let mut best: Option<(f32, usize)> = None;
    let mut offer = |r2: f32, who: usize| {
        if best.is_none_or(|(b, _)| r2 < b) {
            best = Some((r2, who));
        }
    };
    for i in 0..k {
        for j in i + 1..k {
            let (a, b) = (cols[i], cols[j]);
            let u = sub(b, a);
            let uu = dot(&u, &u);
            if uu < 1e-12 {
                continue;
            }
            let t = (dot(&sub(c, a), &u) / uu).clamp(0.0, 1.0);
            let mut q = a;
            for m in 0..N {
                q[m] += u[m] * t;
            }
            offer(d2(c, q), if t < 0.5 { i } else { j });
        }
    }
    for i in 0..k {
        for j in i + 1..k {
            for m in j + 1..k {
                let (a, b, e) = (cols[i], cols[j], cols[m]);
                let (u, v, w) = (sub(b, a), sub(e, a), sub(c, a));
                let (uu, vv, uv) = (dot(&u, &u), dot(&v, &v), dot(&u, &v));
                let det = uu * vv - uv * uv;
                if det.abs() < 1e-12 {
                    continue;
                }
                let (wu, wv) = (dot(&w, &u), dot(&w, &v));
                let sc = (vv * wu - uv * wv) / det;
                let tc = (uu * wv - uv * wu) / det;
                if sc < 0.0 || tc < 0.0 || sc + tc > 1.0 {
                    continue;
                }
                let mut q = a;
                for z in 0..N {
                    q[z] += u[z] * sc + v[z] * tc;
                }
                let wa = 1.0 - sc - tc;
                let who = if wa >= sc && wa >= tc {
                    i
                } else if sc >= tc {
                    j
                } else {
                    m
                };
                offer(d2(c, q), who);
            }
        }
    }
    best.map(|(r2, who)| (r2.sqrt(), who))
}

/// [`crate::regions::absorb_blend_slivers`] in four channels. The white backdrop becomes
/// the clear ink, which is what a partly transparent pixel is partly made of.
///
/// `px` and `inks` are `[W, a]` ([`rgba_w`], [`ink_rgba_w`]); `labels` is edited in place.
/// Same tests, tolerance `max(3 σ_noise, 0.025)` and two rounds as the classic version
/// (see [`crate::regions::absorb_blend_slivers`] for the reasoning). The differences:
/// [`CLEAR`] joins the candidate inks when a pixel of the sliver is translucent
/// (`a < 0.99`) and no candidate is already clear (`a < 0.005`), and there are no debug
/// prints. Returns the number of components absorbed.
pub fn absorb_blend_slivers(
    labels: &mut [u16],
    px: &[[f32; 4]],
    w: usize,
    h: usize,
    inks: &[[f32; 4]],
    sigma_noise: f64,
) -> usize {
    let tol = (3.0 * sigma_noise).max(0.025) as f32;
    let mut absorbed = 0usize;
    for _round in 0..2 {
        let (comp, members) = label_components(labels, w, h);
        let mut changed = 0usize;
        let n_labels = labels
            .iter()
            .copied()
            .max()
            .map_or(1, |m| m as usize + 1)
            .max(inks.len());
        let mut contacts: Vec<usize> = vec![0; n_labels];
        for group in &members {
            if absorb_sliver(group, &comp, labels, px, w, h, inks, tol, &mut contacts) {
                changed += 1;
            }
        }
        absorbed += changed;
        if changed == 0 {
            break;
        }
    }
    absorbed
}

/// Try to dissolve one component in four channels; returns whether it was absorbed.
///
/// The thin / few-inks / blend tests of `regions::absorb_sliver`, with [`CLEAR`] as the
/// backdrop pseudo-ink. On success each pixel moves to the dominant ink of its nearest
/// [`mixture`]; when the clear ink dominates, to the dominant real ink (or the most-touched
/// neighbour if no mixture of the real inks exists).
#[allow(clippy::too_many_arguments)]
fn absorb_sliver(
    group: &[usize],
    comp: &[u32],
    labels: &mut [u16],
    px: &[[f32; 4]],
    w: usize,
    h: usize,
    inks: &[[f32; 4]],
    tol: f32,
    contacts: &mut [usize],
) -> bool {
    let area = group.len();
    if area == 0 {
        return false;
    }
    let id = comp[group[0]];
    let (interior, foreign) = tally_contacts(group, id, comp, labels, w, h, contacts);
    if interior * 5 >= area || foreign == 0 {
        return false;
    }
    let mut tally: Vec<(usize, u16)> = contacts
        .iter()
        .enumerate()
        .filter(|(_, &c)| c > 0)
        .map(|(l, &c)| (c, l as u16))
        .collect();
    tally.sort_by_key(|&(c, l)| (std::cmp::Reverse(c), l));
    tally.truncate(3);
    let covered: usize = tally.iter().map(|&(c, _)| c).sum();
    if covered * 5 < foreign * 4 {
        return false;
    }
    let mut labs: Vec<Option<u16>> = tally.iter().map(|&(_, l)| Some(l)).collect();
    let mut cols: Vec<[f32; 4]> = Vec::with_capacity(4);
    for &(_, l) in &tally {
        let Some(&c) = inks.get(l as usize) else {
            continue;
        };
        cols.push(c);
    }
    if cols.len() != labs.len() || cols.len() < 2 {
        return false;
    }
    if group.iter().any(|&p| px[p][3] < 0.99) && !cols.iter().any(|c| c[3] < 0.005) {
        cols.push(CLEAR);
        labs.push(None);
    }
    let mut pass = 0usize;
    let mut dest: Vec<u16> = Vec::with_capacity(area);
    for &p in group {
        let Some((r, who)) = mixture(px[p], &cols) else {
            dest.push(labels[p]);
            continue;
        };
        if r <= tol {
            pass += 1;
        }
        dest.push(match labs[who] {
            Some(l) => l,
            None => match mixture(px[p], &cols[..labs.len() - 1]) {
                Some((_, w2)) => labs[w2]
                    .expect("only the clear entry is None, and it is last, outside the slice"),
                None => tally[0].1,
            },
        });
    }
    if pass * 5 < area * 4 {
        return false;
    }
    for (&p, &l) in group.iter().zip(&dest) {
        labels[p] = l;
    }
    true
}

/// [`crate::regions::reassign_blend_pixels`] in four channels.
///
/// Same rule (move a pixel to the dominant ink of its nearest [`mixture`] of its own and
/// up to three neighbouring inks when the residual is within `max(3 σ_noise, 0.025)` and
/// below half its distance to its own ink; up to four snapshot rounds), with `[W, a]`
/// pixels and inks and [`CLEAR`] as the backdrop pseudo-ink. Returns the number of moves.
pub fn reassign_blend_pixels(
    labels: &mut [u16],
    px: &[[f32; 4]],
    w: usize,
    h: usize,
    inks: &[[f32; 4]],
    sigma_noise: f64,
) -> usize {
    const ROUNDS: usize = 4;
    let n = w * h;
    let tol = (3.0 * sigma_noise).max(0.025) as f32;
    let mut moved_total = 0usize;
    for _ in 0..ROUNDS {
        let snap = labels.to_vec();
        let decide = |p: usize| -> Option<u16> {
            let (x, y) = (p % w, p / w);
            let own = snap[p];
            let mut labs = [own; 4];
            let mut nl = 1usize;
            for (dx, dy) in [
                (-1i32, -1i32),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                let (qx, qy) = (x as i32 + dx, y as i32 + dy);
                if qx < 0 || qy < 0 || qx >= w as i32 || qy >= h as i32 {
                    continue;
                }
                let l = snap[qy as usize * w + qx as usize];
                if !labs[..nl].contains(&l) && nl < 4 {
                    labs[nl] = l;
                    nl += 1;
                }
            }
            if nl < 2 {
                return None;
            }
            let c = px[p];
            let oc = *inks.get(own as usize)?;
            let resid_own = d2(c, oc).sqrt();
            let mut cols: Vec<[f32; 4]> = Vec::with_capacity(5);
            let mut keep: Vec<Option<u16>> = Vec::with_capacity(5);
            for &l in &labs[..nl] {
                if let Some(&cc) = inks.get(l as usize) {
                    cols.push(cc);
                    keep.push(Some(l));
                }
            }
            if c[3] < 0.99 && !cols.iter().any(|q| q[3] < 0.005) {
                cols.push(CLEAR);
                keep.push(None);
            }
            let (r, who) = mixture(c, &cols)?;
            let target = match keep[who] {
                Some(l) => l,
                None => match mixture(c, &cols[..cols.len() - 1]) {
                    Some((_, w2)) => keep[w2]
                        .expect("only the clear entry is None, and it is last, outside the slice"),
                    None => return None,
                },
            };
            (target != own && r <= tol && r < 0.5 * resid_own).then_some(target)
        };
        let decided: Vec<Option<u16>> = (0..n).into_par_iter().map(decide).collect();
        let mut moved = 0usize;
        for (p, d) in decided.into_iter().enumerate() {
            if let Some(t) = d {
                labels[p] = t;
                moved += 1;
            }
        }
        moved_total += moved;
        if moved == 0 {
            break;
        }
    }
    moved_total
}

// ---------------------------------------------------------------------------------------
// The colour path
// ---------------------------------------------------------------------------------------

/// Two palette entries may be merged into one gradient only when they are drawn at the
/// same opacity: a fill here is fitted over white, where the clear ground and white paint
/// are the same colour.
///
/// Returns the gate handed to `merge_gradient_bands_guarded`: true when the two entries'
/// opacities differ by less than 0.05 (a missing opacity counts as 1). No classic mirror;
/// on the opaque path every ink has opacity 1 and the gate would always pass.
pub fn same_opacity(pal: &Palette) -> impl Fn(u16, u16) -> bool + Sync + '_ {
    move |a: u16, b: u16| {
        let fa = pal.alpha.get(a as usize).copied().unwrap_or(1.0);
        let fb = pal.alpha.get(b as usize).copied().unwrap_or(1.0);
        (fa - fb).abs() < 0.05
    }
}

/// The colour path with transparency carried natively. Mirrors
/// [`crate::trace_color_full_with_alpha`] stage for stage; see the module docs for what
/// changes and why.
///
/// `img` is the source (straight RGBA `[0, 1]`), `alpha` its alpha per pixel (the caller
/// has checked that some pixel is below [`OPAQUE`]). The stages, in order:
///
/// 1. **intake**: noise estimate on luminance, soft-intake test (edge width, lossy
///    container, ringing), exactly as the classic path;
/// 2. **palette**: [`extract_palette`] (two grounds);
/// 3. **labels**: [`label_image`], then on a soft intake the measured residual noise
///    raises `σ` (capped at [`color::MEASURED_SIGMA_CAP`]);
/// 4. **despeckle**: [`crate::despeckle`];
/// 5. **blend_absorb**: [`absorb_blend_slivers`] and [`reassign_blend_pixels`] in `[W, a]`,
///    then despeckle again if anything moved (skipped with `INKVEC_NO_ABSORB`);
/// 6. **merge_bands**: the classic gradient-band merge over white, gated by
///    [`same_opacity`] (only with `opts.gradients`);
/// 7. **carve**: the classic residual-feature carve (with `opts.gradients`, unless
///    `INKVEC_NO_CARVE`);
/// 8. **fades**: `merge_fades` joins translucent bands into opacity gradients (with
///    `opts.gradients`);
/// 9. **split**: [`crate::split_components`], then each face's fill, ink, fade and rim
///    opacity;
/// 10. everything after the face map is shared: [`crate::finish_color_trace_alpha`] with
///     the source alpha and each face's opacity, and the fades attached to the result.
pub fn trace_color(img: &Rgba, opts: &ColorOptions, alpha: &[f32]) -> ColorTrace {
    let (w, h) = (img.width, img.height);
    let rgb = img.composited([1.0, 1.0, 1.0]);

    let lum: Vec<f32> = rgb
        .iter()
        .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
        .collect();
    let mut sigma_noise = coverage::estimate_noise(&lum, w, h);
    let min_region = opts.min_region;
    let mut sw = Stopwatch::start();
    inkvec_core::progress::begin("palette");
    let edge_width = coverage::intake_scale(&rgb, w, h);
    let ringing = coverage::ringing_score(&rgb, w, h);
    let ringing_gate = if w.min(h) >= color::RINGING_MIN_DIM {
        color::SOFT_RINGING_LARGE
    } else {
        color::SOFT_RINGING
    };
    let soft_intake =
        edge_width > color::SOFT_INTAKE_EDGE || opts.lossy_intake || ringing > ringing_gate;
    let noise_sigmas = if soft_intake {
        color::SOFT_NOISE_SIGMAS
    } else {
        color::NOISE_SIGMAS
    };
    let same_ink_de00 = if soft_intake {
        color::SOFT_SAME_INK_DE00
    } else {
        color::SAME_INK_DE00
    };
    crate::diag!(
        "intake",
        "native alpha: w={w} h={h} edge_width={edge_width:.3} ringing={ringing:.4} soft={soft_intake}"
    );

    let pal = extract_palette(
        &rgb,
        alpha,
        w,
        h,
        opts.merge_distance,
        opts.max_colors,
        PaletteEvidence {
            sigma_noise,
            lambda: gradient::bic_lambda(w * h),
            noise_sigmas,
            same_ink_de00,
        },
    );
    sw.mark("palette");
    inkvec_core::progress::begin("labels");
    let mut labels = label_image(&rgb, alpha, &pal);
    if soft_intake {
        let cap = color::MEASURED_SIGMA_CAP / 255.0;
        let measured = (regularize::residual_sigma(&rgb, &labels, w, h, &pal)
            * color::MEASURED_SIGMA_SCALE)
            .min(cap);
        sigma_noise = sigma_noise.max(measured);
    }
    sw.mark("labels");
    inkvec_core::progress::begin("despeckle");
    crate::despeckle(&mut labels, w, h, min_region);
    sw.mark("despeckle");
    inkvec_core::progress::begin("blend_absorb");
    if !inkvec_core::env::flag("INKVEC_NO_ABSORB") {
        let px4 = rgba_w(&rgb, alpha);
        let inks4 = ink_rgba_w(&pal);
        let absorbed = absorb_blend_slivers(&mut labels, &px4, w, h, &inks4, sigma_noise);
        let moved = reassign_blend_pixels(&mut labels, &px4, w, h, &inks4, sigma_noise);
        if absorbed > 0 || moved > 0 {
            crate::despeckle(&mut labels, w, h, min_region);
        }
    }
    sw.mark("blend_absorb");
    inkvec_core::progress::begin("merge_bands");
    let (mut fills_by_label, mut label_ink) = if opts.gradients {
        gradient::bands::merge_gradient_bands_guarded(
            &mut labels,
            &rgb,
            w,
            h,
            &pal,
            sigma_noise,
            gradient::bic_lambda(w * h),
            opts.deadline,
            Some(&same_opacity(&pal)),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    sw.mark("merge_bands");
    inkvec_core::progress::begin("carve");
    if opts.gradients && !inkvec_core::env::flag("INKVEC_NO_CARVE") {
        gradient::carve_residual_features_with_detail_noise(
            &mut labels,
            &rgb,
            w,
            h,
            &pal,
            &mut fills_by_label,
            &mut label_ink,
            sigma_noise,
            gradient::bic_lambda(w * h),
            min_region.max(2),
            None,
        );
    }
    sw.mark("carve");
    inkvec_core::progress::begin("fades");
    let fade_of_label = if opts.gradients {
        merge_fades(
            &mut labels,
            &mut fills_by_label,
            &mut label_ink,
            &rgb,
            alpha,
            w,
            h,
            &pal,
            sigma_noise,
            gradient::bic_lambda(w * h),
        )
    } else {
        Vec::new()
    };
    sw.mark("fades");
    inkvec_core::progress::begin("split");
    let (labels, face_src) = crate::split_components(&labels, w, h);
    sw.mark("split");
    let n_faces = face_src.len();
    let face_fill: Vec<gradient::FillFit> = face_src
        .iter()
        .map(|&l| {
            fills_by_label
                .get(l)
                .cloned()
                .unwrap_or_else(|| gradient::FillFit {
                    model: gradient::FillModel::Flat(
                        pal.rgb.get(l).copied().unwrap_or([1.0, 1.0, 1.0]),
                    ),
                    chi2: 0.0,
                    params: gradient::PARAMS_FLAT,
                    cost: 0.0,
                })
        })
        .collect();
    let face_color: Vec<usize> = face_src
        .iter()
        .map(|&l| label_ink.get(l).copied().unwrap_or(l))
        .collect();
    let face_fade: Vec<Option<Fade>> = face_src
        .iter()
        .map(|&l| fade_of_label.get(l).cloned().flatten())
        .collect();
    // Where a face meets the ground the boundary is placed by its opacity there: a fade's
    // rim, a wash's one opacity.
    let face_alpha: Vec<f32> = face_fade
        .iter()
        .zip(&face_color)
        .map(|(fade, &ink)| match fade {
            Some(f) => f.rim_alpha(),
            None => pal.alpha.get(ink).copied().unwrap_or(1.0),
        })
        .collect();
    let mut tr = crate::finish_color_trace_alpha(
        img,
        opts,
        &rgb,
        pal,
        labels,
        face_fill,
        face_color,
        n_faces,
        sigma_noise,
        &mut sw,
        Some(alpha),
        Some(face_alpha),
    );
    if tr.face_color.len() == face_fade.len() {
        tr.face_fade = face_fade;
    }
    tr
}

#[cfg(test)]
mod tests;
