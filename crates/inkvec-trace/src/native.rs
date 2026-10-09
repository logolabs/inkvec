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
//! | [`extract_palette`] | [`color::extract_palette_mdl`] | [`Ink2`] points; the clear ink is not counted against `max_colors` and does not use up the rarity exemption; a translucent candidate needs an interior; the representation test measures in six coordinates with the clear ground always among the inks |
//! | [`label_image`] | [`color::label_image`] | [`Ink2::dist`] |
//! | `palette::frequency_modes` | `color::mdl::frequency_modes` | bins over both grounds, `u64` keys |
//! | `palette::Walk` (claim, spread) | `color::mdl::Walk` | [`Ink2::dist`] |
//! | `palette::blend_pairs_cached` | `color::blend_pairs_cached` | six coordinates ([`six`]) |
//! | `palette::straddle` | `color::mdl::straddle` | six coordinates |
//! | `palette::BlendEvidence` | `color::mdl::BlendEvidence` | the translucent-interior rule |
//! | `palette::refine_to_members` | `color::mdl::refine_to_members` | means over both grounds |
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
//! to check. Only the colour-free helpers are shared: the label-graph ones (connected
//! components and contact counts) from [`crate::regions`], and the distinct-colour index
//! and claimed-set tests (`color::distinct`), which read colour ids, not colours.

use rayon::prelude::*;

use crate::color::distinct::ColourIds;
use crate::color::{
    self, de00, linear_to_srgb, oklab_to_rgb, rgb_to_oklab, srgb_to_linear, Oklab, Palette,
    PaletteEvidence,
};
use crate::regions::{tally_contacts, SliverRound};
use crate::{coverage, gradient, regularize, ColorOptions, ColorTrace, Rgba, Stopwatch};

mod fade;
mod palette;
use fade::merge_fades;
pub use fade::Fade;
#[cfg(test)]
use fade::{alpha_params, fade_chi2, fit_colour_stops, fit_opacity, model_stops, solve};
#[cfg(test)]
use palette::{claim_spread, interior_fraction, straddle_fraction};

/// Alpha at or above which a pixel or an ink is opaque.
pub const OPAQUE: f32 = 0.999;

/// Opacity below which a palette candidate that is not a blend has to show an interior to
/// be kept as an ink (the translucent-interior rule of `palette::BlendEvidence::measure`).
///
/// The rule asks every opacity it does not round to 1, so that the 0.95-0.99 rim just
/// inside an opaque silhouette is questioned (see the reference copy in
/// `reference_tests.rs`). A stroke narrower than about 1.5 px never reaches 1 either: box
/// filtering peaks at 0.990 on the 1.2 px bar of `bench/cases` `glyph_ring_bar`, and with
/// no interior to show, the ink was rejected and the glyph traced from its 0.28 fringe with
/// both counters lost. From 0.98 up a candidate is treated as opaque. On all 1386 icons of
/// `bench/alpha_eval.py` (128ss), 18 traces change, 11 better and 7 worse, and the three
/// means are unchanged to six places.
pub(crate) const TRANSLUCENT_BELOW: f32 = 0.98;

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

/// A two-ground point's six blend coordinates: the colour over white and over the second
/// ground ([`SECOND_GROUND`], mid-grey), in linear light or in sRGB.
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
/// inks over white *and* over grey. An anti-aliased rim between an ink and the clear ground
/// is on such a chord (flat over white for a white ink, a ramp to grey over grey).
///
/// Same formula as the classic `blend_pairs` with `p`, `A`, `B` the [`six`] coordinates:
/// `t = ((p − A) · (B − A)) / |B − A|²` kept for `tmin ≤ t ≤ 1 − tmin`, and the residual
/// `off = Ink2::dist(c, from_six(A + t (B − A)))`, kept when `off ≤ tol`. Returns
/// `(i, j, linear, off)`, linear-light pairs first. The walk calls
/// `palette::blend_pairs_cached` with the inks' coordinates converted once.
#[cfg(test)]
fn blend_pairs(c: Ink2, accepted: &[Ink2], tol: f32, tmin: f32) -> Vec<(usize, usize, bool, f32)> {
    palette::blend_pairs_cached(c, &palette::InkSix::of(accepted), tol, tmin)
}

/// [`color::extract_palette_mdl`], with every colour a two-ground point: the same
/// frequency-ranked mode seeking, perceptual same-ink floor, merge radius,
/// description-length escape and its interior rule, representation test for rare
/// candidates (`color::represent`, with the clear ground always among the inks a pixel may
/// be a mixture of) and blend tests, asked over white and over grey at once.
///
/// The clear ground comes out as an ink of its own -- white over white, grey over grey,
/// opacity 0 -- and a translucent wash as one with its own opacity, with no alpha splitting
/// after the fact. `Palette::colors`/`rgb` hold each ink over white; `alpha` its opacity.
///
/// Differences from the classic walk, beyond the distance:
///
/// * the clear ink (opacity ≤ [`CLEAR_INK_ALPHA`]) does not count against `max_colors`;
///   once the cap is full the scan continues only to find it, and stops once it is found;
/// * nor does it use up the rarity exemption: the first ink that draws something is never
///   rare, as the first ink is not, so a lone small shape on a clear canvas is an ink
///   without having to be represented (see `palette::rarity_exempt`);
/// * a translucent candidate that is not a blend must have an interior (see
///   `palette::BlendEvidence::measure`);
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
    let ids = ColourIds::of_rgba(rgb, alpha);
    extract_palette_ids(
        rgb,
        alpha,
        &ids,
        width,
        height,
        merge_distance,
        max_colors,
        ev,
    )
}

/// [`extract_palette`] with the image's (colour, opacity) ids already numbered
/// ([`ColourIds::of_rgba`]), so the caller can share them with the labelling.
#[allow(clippy::too_many_arguments)]
fn extract_palette_ids(
    rgb: &[[f32; 3]],
    alpha: &[f32],
    ids: &ColourIds,
    width: usize,
    height: usize,
    merge_distance: f32,
    max_colors: usize,
    ev: PaletteEvidence,
) -> Palette {
    let view = palette::NativeView::new(rgb, alpha, ids, width, height);
    palette::extract(&view, merge_distance, max_colors, ev)
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

/// Every pixel to its nearest ink, over both grounds.
///
/// The two-ground [`color::label_image`]: nearest by [`Ink2::dist`], ties to the lower
/// index; label 0 for an empty palette. Found once per distinct (colour, opacity) point.
pub fn label_image(rgb: &[[f32; 3]], alpha: &[f32], pal: &Palette) -> Vec<u16> {
    palette::label(rgb, alpha, &ColourIds::of_rgba(rgb, alpha), pal)
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
        let round = SliverRound::of(labels, w, h);
        let mut changed = 0usize;
        let n_labels = labels
            .iter()
            .copied()
            .max()
            .map_or(1, |m| m as usize + 1)
            .max(inks.len());
        let mut contacts: Vec<usize> = vec![0; n_labels];
        for group in (0..round.comps.len()).filter_map(|id| round.thin(id)) {
            let comp = &round.comps.comp;
            if absorb_sliver(group, comp, labels, px, w, h, inks, tol, &mut contacts) {
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
/// below half its distance to its own ink; up to four snapshot rounds, run by
/// [`crate::regions::relabel_rounds`]), with `[W, a]`
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
    let tol = (3.0 * sigma_noise).max(0.025) as f32;
    let decide = |snap: &[u16], p: usize| -> Option<u16> {
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
    crate::regions::relabel_rounds(labels, w, h, ROUNDS, decide)
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

/// Mean opacity at or above which a feature the carve stage cut out is paint
/// ([`name_carved_paint`]): most of its coverage is drawn.
///
/// Measured 2026-10-02 on every feature the carve named by the clear ink in the screen
/// and held_a icons at 128 px: anti-aliasing residue left in the clear ground's interior
/// (light grey flecks in `simple-icons/mysql`, pale blue in `twemoji/1faa3`) reads 0.14 to
/// 0.36, and painting those opaque cost dE00 on five of the eight icons; paint reads 1.00
/// (white details in two openmoji icons) and the repro's pale disc, rim and all, 0.80 to
/// 0.92. The half sits between the two groups.
const CARVED_PAINT_ALPHA: f32 = 0.5;

/// Name each feature the carve stage minted by an ink that draws something, when the carve
/// named it by the clear ground although its own pixels are paint.
///
/// The carve (`gradient::carve_residual_features_with_detail_noise`) is shared with the
/// classic path and names a minted feature by the palette entry nearest its median colour
/// over white (squared sRGB distance). Over white the clear ground *is* white, so a light
/// feature -- pale yellow, white paint -- is named by the clear ink whenever no light paint
/// ink is nearer, and on this path a face's opacity is its ink's: the feature was cut out
/// as a face and then drawn at opacity 0. The 2026-10-02 repro: a 30 px² pale-yellow disc
/// beside a black disc on a clear 128 px canvas (too rare to be an ink itself then, see
/// `palette::rarity_exempt`; since `color::represent` a rare shape that clears the
/// representation floor is an ink, and this naming covers what stays under it) was traced
/// to nothing.
///
/// For each minted label `l` (`from..label_ink.len()`) whose ink is clear (opacity ≤
/// [`CLEAR_INK_ALPHA`]): take the feature's mean colour over white `W̄` and mean opacity `ā`
/// over its pixels. When `ā ≥` [`CARVED_PAINT_ALPHA`] the feature is paint, and the label
/// is renamed to the visible ink nearest the two-ground point of `(W̄, ā)` by
/// [`Ink2::dist`], ties to the lower index. A feature the carve named by a visible ink, and
/// a feature that is mostly see-through, keeps its name, so a trace with neither is
/// unchanged in every byte. Its fill (the feature's own median colour) is not touched.
///
/// Cost: one O(w·h) pass summing the renamed features, and only when a minted label is
/// named clear; nothing at all without a visible ink. Edge cases: `from` past the end (the
/// carve minted nothing) and an out-of-range ink index (treated as clear) are no-ops.
///
/// Not from the literature: a naming rule for this pipeline's carve stage, because the
/// carve's over-white comparison cannot tell white paint from the clear ground, which is
/// the distinction this module exists to keep (two grounds, see the module docs). See also:
/// the classic carve in `gradient/carve.rs`, which keeps its sRGB naming, since on an
/// opaque image every ink is paint.
fn name_carved_paint(
    labels: &[u16],
    rgb: &[[f32; 3]],
    alpha: &[f32],
    pal: &Palette,
    label_ink: &mut [usize],
    from: usize,
) {
    let clear = |i: usize| pal.alpha.get(i).is_none_or(|&a| a <= CLEAR_INK_ALPHA);
    let renamed: Vec<usize> = (from..label_ink.len())
        .filter(|&l| clear(label_ink[l]))
        .collect();
    let visible: Vec<usize> = (0..pal.len()).filter(|&i| !clear(i)).collect();
    if renamed.is_empty() || visible.is_empty() {
        return;
    }
    // Per renamed label: Σ colour over white, Σ opacity, pixel count. `slot[l - from]` is
    // the label's row in `sums`, or `usize::MAX` when it is not being renamed.
    let mut slot = vec![usize::MAX; label_ink.len() - from];
    for (k, &l) in renamed.iter().enumerate() {
        slot[l - from] = k;
    }
    let mut sums = vec![([0.0f64; 3], 0.0f64, 0usize); renamed.len()];
    for (p, &l) in labels.iter().enumerate() {
        let l = l as usize;
        if l < from || l >= label_ink.len() || slot[l - from] == usize::MAX {
            continue;
        }
        let s = &mut sums[slot[l - from]];
        for c in 0..3 {
            s.0[c] += rgb[p][c] as f64;
        }
        s.1 += alpha[p] as f64;
        s.2 += 1;
    }
    let inks = ink_points(pal);
    for (k, &l) in renamed.iter().enumerate() {
        let (sum_w, sum_a, n) = sums[k];
        if n == 0 {
            continue;
        }
        let a_mean = (sum_a / n as f64) as f32;
        if a_mean < CARVED_PAINT_ALPHA {
            continue;
        }
        let w_mean = sum_w.map(|v| (v / n as f64) as f32);
        let point = pixel_points(&[w_mean], &[a_mean])[0];
        // `visible` is non-empty, so there is a nearest; ties keep the lower index.
        let mut best = visible[0];
        let mut best_d = point.dist(inks[best]);
        for &i in &visible[1..] {
            let d = point.dist(inks[i]);
            if d < best_d {
                best = i;
                best_d = d;
            }
        }
        label_ink[l] = best;
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
///    `INKVEC_NO_CARVE`), then [`name_carved_paint`] so a feature of paint is never
///    named by the clear ink;
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

    let ids = ColourIds::of_rgba(&rgb, alpha);
    let pal = extract_palette_ids(
        &rgb,
        alpha,
        &ids,
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
    let mut labels = palette::label(&rgb, alpha, &ids, &pal);
    drop(ids);
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
    if opts.absorb_blends && !inkvec_core::env::flag("INKVEC_NO_ABSORB") {
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
        let carved_from = fills_by_label.len();
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
        name_carved_paint(&labels, &rgb, alpha, &pal, &mut label_ink, carved_from);
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
    // One artist ink, one hex (`color::snap`): over white, as every fill here is fitted.
    // A fade's fill is its colour profile and keeps it.
    let mut face_fill = face_fill;
    let snapped =
        color::snap::snap_flat_fills(&labels, w, h, &mut face_fill, &face_color, &pal, |f| {
            face_fade.get(f).is_some_and(Option::is_some)
        });
    crate::diag!(
        "fills",
        "native alpha: snapped to their ink's colour: {snapped}"
    );
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
mod reference_tests;
#[cfg(test)]
mod tests;
