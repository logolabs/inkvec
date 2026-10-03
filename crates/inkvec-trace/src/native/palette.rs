//! The two-ground palette walk of [`super::extract_palette`], evaluated per distinct
//! (colour, opacity) point: the transparent-image mirror of `color/mdl.rs`.
//!
//! Every per-pixel quantity the walk reads -- the two-ground point, its six blend
//! coordinates, its distance to a candidate or to the nearest accepted ink -- is a function
//! of the pixel's colour and alpha bits, so it is computed once per distinct pair and read
//! back through the pixel's id. The literature and the exactness argument are the classic
//! path's; see `crate::color::distinct` (Celebi 2011's unique-colour reduction, Swain and
//! Ballard 1991's backprojection, Korn and Muthukrishnan 2000's influence sets). As there,
//! the candidate means and the refined inks are still summed over the pixels in order.

use rayon::prelude::*;

use super::{
    bin, from_six, ink_points, over_black, same_ink_as_accepted, six, Ink2, BINS, CLEAR_INK_ALPHA,
    OPAQUE,
};
use crate::color::distinct::{
    side_of, BitsMap, Claim, ColourIds, DistinctImage, Neighbourhoods, PAR_COLOURS,
};
use crate::color::{
    self, oklab_to_rgb, rgb_to_oklab, Oklab, Palette, PaletteEvidence, BLEND_INTERIOR_FRACTION,
    BLEND_STRADDLE_FRACTION, JND_FLOOR, MIN_INK_WEIGHT, PARAMS_PER_INK,
};

/// Distinct points per parallel task in the per-point conversions.
const PAR_MIN_POINTS: usize = 1024;

/// One pixel's two-ground point, exactly as [`super::pixel_points`] computes it.
fn point_of(c: [f32; 3], a: f32) -> Ink2 {
    let w = rgb_to_oklab(c);
    if a >= OPAQUE {
        Ink2::opaque(w)
    } else {
        Ink2 {
            w,
            k: rgb_to_oklab(over_black(c, a)),
        }
    }
}

/// Every distinct (colour, opacity) point as a two-ground point and as [`six`] coordinates
/// in both spaces, converted once.
pub(crate) struct NativeView<'a> {
    /// The ids, their visited pixels and the geometry.
    img: DistinctImage<'a>,
    /// Each point over both grounds.
    pts: Vec<Ink2>,
    /// Each point's blend coordinates in sRGB and in linear light.
    six_srgb: Vec<[f32; 6]>,
    six_lin: Vec<[f32; 6]>,
}

impl<'a> NativeView<'a> {
    /// Convert the distinct points of `rgb` (over white) and `alpha`, numbered by `ids`.
    pub(crate) fn new(
        rgb: &[[f32; 3]],
        alpha: &[f32],
        ids: &'a ColourIds,
        width: usize,
        height: usize,
    ) -> Self {
        let pts: Vec<Ink2> = ids
            .reps
            .par_iter()
            .with_min_len(PAR_MIN_POINTS)
            .map(|&i| point_of(rgb[i], alpha[i]))
            .collect();
        let six_srgb = pts
            .par_iter()
            .with_min_len(PAR_MIN_POINTS)
            .map(|&p| six(p, false))
            .collect();
        let six_lin = pts
            .par_iter()
            .with_min_len(PAR_MIN_POINTS)
            .map(|&p| six(p, true))
            .collect();
        NativeView {
            img: DistinctImage::new(ids, width, height),
            pts,
            six_srgb,
            six_lin,
        }
    }
}

/// Each accepted ink's [`six`] coordinates, converted once when it is accepted.
#[derive(Default)]
pub(crate) struct InkSix {
    /// In sRGB.
    pub(crate) srgb: Vec<[f32; 6]>,
    /// In linear light.
    pub(crate) lin: Vec<[f32; 6]>,
}

impl InkSix {
    /// The inks `accepted`, converted.
    #[cfg(test)]
    pub(crate) fn of(accepted: &[Ink2]) -> Self {
        let mut s = InkSix::default();
        for &a in accepted {
            s.push(a);
        }
        s
    }

    /// Convert and append one ink.
    pub(crate) fn push(&mut self, ink: Ink2) {
        self.srgb.push(six(ink, false));
        self.lin.push(six(ink, true));
    }
}

/// [`super::blend_pairs`] with the accepted inks' six coordinates already converted: every
/// pair `(i, j)` and space whose chord `c` lies on (`tmin ≤ t ≤ 1 − tmin`, residual
/// `Ink2::dist(c, from_six(A + t (B − A))) ≤ tol`), as `(i, j, linear, off)`, linear-light
/// pairs first. Empty with fewer than two inks.
pub(crate) fn blend_pairs_cached(
    c: Ink2,
    inks: &InkSix,
    tol: f32,
    tmin: f32,
) -> Vec<(usize, usize, bool, f32)> {
    let mut out = Vec::new();
    if inks.srgb.len() < 2 {
        return out;
    }
    for space in 0..2 {
        let linear = space == 0;
        let p = six(c, linear);
        let coords = if linear { &inks.lin } else { &inks.srgb };
        for i in 0..coords.len() {
            for j in i + 1..coords.len() {
                let (a, b) = (coords[i], coords[j]);
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

/// [`super::extract_palette`] on a prepared view. See there for every decision.
pub(crate) fn extract(
    view: &NativeView,
    merge_distance: f32,
    max_colors: usize,
    ev: PaletteEvidence,
) -> Palette {
    let modes = frequency_modes(view);
    let points = view.pts.len();
    let mut walk = Walk {
        view,
        ev,
        merge_distance,
        total_px: view.img.pixels().max(1) as f32,
        colors: Vec::new(),
        six: InkSix::default(),
        nearest: vec![f32::INFINITY; points],
        claim: Claim::new(points),
        scratch: vec![0u8; points + 1],
        paldbg: inkvec_core::env::flag("INKVEC_PALDBG"),
    };
    // The clear ground draws nothing, so it is found but not counted against the cap; once
    // the cap is full the scan goes on only to look for it.
    let clear = |c: &Ink2| c.alpha() <= CLEAR_INK_ALPHA;
    for &(n, _key, c) in &modes {
        let full = walk.colors.iter().filter(|p| !clear(p)).count() >= max_colors;
        if full && walk.colors.iter().any(clear) {
            break;
        }
        if full && !clear(&c) {
            continue;
        }
        if walk.accepts(n, c) {
            walk.accept(c);
        }
    }
    let paldbg = walk.paldbg;
    let mut colors = walk.colors;
    if colors.is_empty() {
        colors.push(modes.first().map(|m| m.2).unwrap_or(Ink2::opaque(Oklab {
            l: 1.0,
            a: 0.0,
            b: 0.0,
        })));
    }
    let weight = refine_to_members(view, &mut colors, merge_distance);
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

/// Whether candidate `c` skips the rarity gate (`MIN_INK_WEIGHT`): it is the first ink
/// the walk would accept, or the first one that draws anything.
///
/// The rarity gate rejects a candidate that claims less than `MIN_INK_WEIGHT` (0.4 %) of
/// the image, because anti-aliased colours are individually rare and an ink is not. The
/// classic walk exempts its first ink, so the palette is never empty. Here the first ink
/// is nearly always the clear ground (on a transparent canvas it is the commonest
/// colour), and the clear ground draws nothing. With only the classic exemption, an image
/// whose only paint covers less than 0.4 % of the canvas got a palette of the clear ink
/// alone: a lone 50 px² disc on a 128 px transparent canvas (0.31 %) was traced to an
/// empty SVG. The carve stage still cut the disc out as a face, but it named the face by
/// the nearest palette entry over white, which was the clear ink, so the face was emitted
/// with opacity 0.
///
/// So the exemption goes to the first ink that draws something: `c` is exempt when it is
/// not clear (opacity above `CLEAR_INK_ALPHA`) and no accepted ink is, or, as in the
/// classic walk, when nothing is accepted yet. The clear ground is treated the same way
/// by the colour cap, which it does not count against. Every other gate still applies to
/// the exempt candidate, so a translucent anti-aliased rim without an interior is still
/// rejected (`BlendEvidence::measure`).
///
/// It changes a palette only when a rare visible candidate comes up while no visible ink
/// has been accepted, that is, when no more frequent visible candidate passed the gates:
/// a transparent canvas whose paint is all rare. O(accepted inks).
///
/// Not from the literature: the rarity gate and its exemption are rules of this walk,
/// because the published quantisers have no clear ink that draws nothing. See also:
/// Heckbert, P. (1982), "Color image quantization for frame buffer display", *ACM
/// SIGGRAPH Computer Graphics* 16(3):297-307, doi:10.1145/965145.801294, whose
/// popularity algorithm keeps the most frequent colours and drops rare ones. Rare colours
/// that matter, such as a small isolated shape, are the known weakness of that rule.
fn rarity_exempt(accepted: &[Ink2], c: Ink2) -> bool {
    let clear = |p: &Ink2| p.alpha() <= CLEAR_INK_ALPHA;
    accepted.is_empty() || (!clear(&c) && accepted.iter().all(clear))
}

/// The walk's state between candidates.
struct Walk<'v, 'a> {
    view: &'v NativeView<'a>,
    ev: PaletteEvidence,
    merge_distance: f32,
    total_px: f32,
    /// Accepted inks, in acceptance order.
    colors: Vec<Ink2>,
    /// The same inks' six coordinates.
    six: InkSix,
    /// Per point, the [`Ink2::dist`] to the nearest accepted ink.
    nearest: Vec<f32>,
    /// The current candidate's claimed points.
    claim: Claim,
    /// One byte per point for the straddle test.
    scratch: Vec<u8>,
    paldbg: bool,
}

impl Walk<'_, '_> {
    /// Whether candidate `c` (from a bin of `n` pixels) passes every gate.
    fn accepts(&mut self, n: u32, c: Ink2) -> bool {
        let PaletteEvidence {
            sigma_noise,
            lambda,
            noise_sigmas,
            same_ink_de00,
        } = self.ev;
        let view = self.view;
        let claim = view
            .img
            .claim(&mut self.claim, &self.nearest, |d| view.pts[d].dist(c));
        // Read only through `noise_sigmas * spread`, which is zero with the guard off.
        let spread = if noise_sigmas == 0.0 {
            0.0
        } else {
            view.img.spread(&self.claim, self.merge_distance)
        };
        if (claim as f32 / self.total_px) < MIN_INK_WEIGHT && !rarity_exempt(&self.colors, c) {
            return false;
        }
        let nearest = self
            .colors
            .iter()
            .map(|&p| p.dist(c))
            .fold(f32::INFINITY, f32::min);
        let reach = noise_sigmas * spread;
        if same_ink_as_accepted(c, &self.colors, same_ink_de00) {
            return false;
        }
        // Inside the merge radius: kept only if the escape pays, and then (below) only with
        // an interior (`color::escape_needs_interior`).
        let escaped = nearest <= self.merge_distance.max(reach);
        if escaped {
            let worth_it = sigma_noise > 0.0
                && nearest > JND_FLOOR
                && nearest > reach
                && 0.5 * (claim as f64) * ((nearest as f64 / sigma_noise).powi(2))
                    > lambda * PARAMS_PER_INK;
            if !worth_it {
                return false;
            }
        }
        let Some(shape) = BlendEvidence::measure(self, c, escaped) else {
            return false;
        };
        if self.paldbg {
            eprintln!(
                "  native cand w={} k={} a={:.3} bin={n} claim={claim} near={nearest:.4} blend={} interior={:.3} straddle={:.3} escaped={escaped}",
                color::to_hex(oklab_to_rgb(c.w)),
                color::to_hex(oklab_to_rgb(c.k)),
                c.alpha(),
                shape.blend,
                shape.interior,
                shape.straddle
            );
        }
        !shape.is_thin_escape() && !shape.is_coverage()
    }

    /// Accept `c`: lower every point's nearest-ink distance and record the ink.
    fn accept(&mut self, c: Ink2) {
        let pts = &self.view.pts;
        if pts.len() >= PAR_COLOURS {
            self.nearest
                .par_iter_mut()
                .zip(pts.par_iter())
                .for_each(|(d, &q)| *d = d.min(q.dist(c)));
        } else {
            for (d, &q) in self.nearest.iter_mut().zip(pts) {
                *d = d.min(q.dist(c));
            }
        }
        self.six.push(c);
        self.colors.push(c);
    }
}

/// Whether a candidate is anti-aliasing rather than an ink, over both grounds: the classic
/// blend, thin and straddling test, the escape rule (`color::escape_needs_interior`), plus
/// the translucent-interior rule.
struct BlendEvidence {
    blend: bool,
    /// Inside the merge radius of an accepted ink, kept so far only by the MDL escape.
    escaped: bool,
    /// Share of the claimed pixels that are interior; 1.0 (not measured) unless the
    /// candidate is a blend, translucent or escaped.
    interior: f32,
    straddle: f32,
}

impl BlendEvidence {
    /// Measure candidate `c`, or `None` when it is rejected outright as a translucent band
    /// with no interior.
    ///
    /// Partly transparent is exactly what an anti-aliased silhouette pixel is, and the chord
    /// test cannot always say so: a rim where shading meets the ground mixes the clear ink
    /// with a shade the palette rejected as a blend itself, so no pair of accepted inks
    /// explains it (`noto-emoji/emoji_u1f932` minted two inks at 0.77 from 136 scattered rim
    /// pixels). An ink covers area, anti-aliasing is a band: a translucent candidate (any
    /// opacity strictly between 0 and 1) that is not a blend is kept only with an interior.
    /// Blends keep the straddle test instead; asking them for an interior as well traded
    /// noto-emoji for twemoji and cost the translucent set.
    ///
    /// The escape rule (`color::escape_needs_interior`) extends the translucent rule to an
    /// *opaque* non-blend, but only inside the merge radius, where the description-length
    /// escape admitted it (`escaped`): its interior is measured for that. Blends are
    /// measured exactly as before, and a candidate that is neither a blend, translucent nor
    /// escaped is not measured at all.
    fn measure(walk: &mut Walk, c: Ink2, escaped: bool) -> Option<Self> {
        let pairs = blend_pairs_cached(c, &walk.six, walk.merge_distance * 1.6, color::BLEND_TMIN);
        let blend = !pairs.is_empty();
        let translucent = {
            let a = c.alpha();
            a > 0.0 && a < 1.0
        };
        let img = &walk.view.img;
        let interior = if blend || translucent || escaped {
            img.interior(&walk.claim)
        } else {
            1.0
        };
        if translucent && !blend && interior < BLEND_INTERIOR_FRACTION {
            return None;
        }
        let straddle = if blend && interior < BLEND_INTERIOR_FRACTION {
            let hoods = img.neighbourhoods(&walk.claim);
            pairs
                .iter()
                .map(|&(i, j, linear, _)| {
                    straddle(
                        walk.view,
                        &hoods,
                        c,
                        (i, j),
                        &walk.six,
                        linear,
                        &mut walk.scratch,
                    )
                })
                .fold(0.0f32, f32::max)
        } else {
            0.0
        };
        Some(BlendEvidence {
            blend,
            escaped,
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

    /// Admitted inside the merge radius by the description-length escape, not a blend, and
    /// thin: the crest's overshoot rim. See `color::escape_needs_interior`.
    fn is_thin_escape(&self) -> bool {
        color::escape_needs_interior(self.escaped, self.blend, self.interior)
    }
}

/// The classic straddle test in [`six`] coordinates: the share of the pixels `c` claims
/// that have, within their 3x3 neighbourhood, one pixel further towards ink `pair.0` and one
/// further towards ink `pair.1` along their axis than `c` is (steps as in the classic
/// version, [`color::STRADDLE_STEP`] clipped to half the room on each side, floor 0.02).
/// 0 without a full grid or for a degenerate axis, 1 when `c` claims nothing.
fn straddle(
    view: &NativeView,
    hoods: &Neighbourhoods,
    c: Ink2,
    pair: (usize, usize),
    inks: &InkSix,
    linear: bool,
    scratch: &mut [u8],
) -> f32 {
    if !hoods.has_geometry() {
        return 0.0;
    }
    let coords = if linear { &inks.lin } else { &inks.srgb };
    let (pa, pb) = (coords[pair.0], coords[pair.1]);
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
    let (lo, hi) = (tc - step_lo, tc + step_hi);
    let px = if linear {
        &view.six_lin
    } else {
        &view.six_srgb
    };
    hoods.straddle(|k| side_of(t_of(&px[k]), lo, hi), scratch)
}

/// The palette candidates: occupied two-ground bins (`bin(w) · 24³ + bin(k)`), each with
/// its pixel count and its pixels' mean over both grounds (summed in `f64`, in pixel order),
/// sorted by count descending then key ascending. The bin is found per point; the sums walk
/// the pixels in order, so the means are bit-identical to a per-pixel pass.
fn frequency_modes(view: &NativeView) -> Vec<(u32, u64, Ink2)> {
    let mut slot_of: BitsMap<u64, u32> = BitsMap::default();
    let mut keys: Vec<u64> = Vec::new();
    let slot: Vec<u32> = view
        .pts
        .iter()
        .map(|p| {
            let key = bin(p.w) * (BINS * BINS * BINS) as u64 + bin(p.k);
            *slot_of.entry(key).or_insert_with(|| {
                keys.push(key);
                (keys.len() - 1) as u32
            })
        })
        .collect();
    let mut acc: Vec<(u32, [f64; 6])> = vec![(0, [0.0; 6]); keys.len()];
    for &id in view.img.cid {
        let p = view.pts[id as usize];
        let e = &mut acc[slot[id as usize] as usize];
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
        .zip(keys)
        .map(|((n, s), key)| (n, key, mean_of(&s, n)))
        .collect();
    modes.sort_by_key(|&(n, key, _)| (std::cmp::Reverse(n), key));
    modes
}

/// A two-ground point from six `f64` sums over `n` pixels.
fn mean_of(s: &[f64; 6], n: u32) -> Ink2 {
    let f = n as f64;
    let m = |i: usize| (s[i] / f) as f32;
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
    }
}

/// Refine each ink to the mean of the pixels that chose it (nearest by [`Ink2::dist`], ties
/// to the lower index, within `merge_distance`), over both grounds, and return each ink's
/// share of the image. The choice is made per point; the sums walk the pixels in order.
fn refine_to_members(view: &NativeView, colors: &mut [Ink2], merge_distance: f32) -> Vec<f32> {
    let total_px = view.img.pixels().max(1) as f32;
    let chosen: Vec<u32> = view
        .pts
        .par_iter()
        .with_min_len(PAR_MIN_POINTS)
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
    for &id in view.img.cid {
        let k = chosen[id as usize];
        if k != u32::MAX {
            let c = view.pts[id as usize];
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
            colors[i] = mean_of(s, *n);
        }
        weight.push(*n as f32 / total_px);
    }
    weight
}

/// Every pixel's nearest ink over both grounds ([`Ink2::dist`], ties to the lower index),
/// found once per distinct point and read back through the pixel's id.
pub(crate) fn label(rgb: &[[f32; 3]], alpha: &[f32], ids: &ColourIds, pal: &Palette) -> Vec<u16> {
    let inks = ink_points(pal);
    let per_point: Vec<u16> = ids
        .reps
        .par_iter()
        .with_min_len(PAR_MIN_POINTS)
        .map(|&i| {
            let c = point_of(rgb[i], alpha[i]);
            let mut best = (0usize, f32::MAX);
            for (k, &p) in inks.iter().enumerate() {
                let d = c.dist(p);
                if d < best.1 {
                    best = (k, d);
                }
            }
            best.0 as u16
        })
        .collect();
    ids.cid
        .par_iter()
        .with_min_len(1 << 16)
        .map(|&d| per_point[d as usize])
        .collect()
}
#[cfg(test)]
pub(crate) use adapters::{claim_spread, interior_fraction, straddle_fraction};

/// The per-pixel entry points the native palette's unit tests were written against,
/// answered by the per-point code. Ids are keyed on every per-pixel input.
#[cfg(test)]
mod adapters {
    use super::*;

    fn bits6(v: Option<&[f32; 6]>) -> [u32; 6] {
        v.map_or([0; 6], |v| v.map(f32::to_bits))
    }

    fn ids_of(px: &[Ink2], s6: &[[f32; 6]], l6: &[[f32; 6]], nearest: &[f32]) -> ColourIds {
        let n = px.len().min(nearest.len());
        ColourIds::build(n, |i| {
            let p = px[i];
            (
                [p.w.l, p.w.a, p.w.b, p.k.l, p.k.a, p.k.b].map(f32::to_bits),
                bits6(s6.get(i)),
                bits6(l6.get(i)),
                nearest[i].to_bits(),
            )
        })
    }

    /// A view over given points and caches (one entry per id, from its first pixel).
    fn view_of<'a>(
        ids: &'a ColourIds,
        px: &[Ink2],
        s6: &[[f32; 6]],
        l6: &[[f32; 6]],
        width: usize,
        height: usize,
        stride_px: usize,
    ) -> NativeView<'a> {
        NativeView {
            img: DistinctImage::with_stride(ids, width, height, stride_px),
            pts: ids.reps.iter().map(|&i| px[i]).collect(),
            six_srgb: ids
                .reps
                .iter()
                .map(|&i| s6.get(i).copied().unwrap_or([0.0; 6]))
                .collect(),
            six_lin: ids
                .reps
                .iter()
                .map(|&i| l6.get(i).copied().unwrap_or([0.0; 6]))
                .collect(),
        }
    }

    /// The old `claim_spread(px, nearest_px, c, tol, stride_px)`.
    pub(crate) fn claim_spread(
        px: &[Ink2],
        nearest_px: &[f32],
        c: Ink2,
        tol: f32,
        stride_px: usize,
    ) -> (usize, f32) {
        let ids = ids_of(px, &[], &[], nearest_px);
        let view = view_of(&ids, px, &[], &[], px.len(), 1, stride_px);
        let near: Vec<f32> = ids.reps.iter().map(|&i| nearest_px[i]).collect();
        let mut claim = Claim::new(ids.len());
        let n = view.img.claim(&mut claim, &near, |d| view.pts[d].dist(c));
        (n, view.img.spread(&claim, tol))
    }

    /// The old `interior_fraction(px, width, height, c, nearest, stride_px)`.
    pub(crate) fn interior_fraction(
        px: &[Ink2],
        width: usize,
        height: usize,
        c: Ink2,
        nearest: &[f32],
        stride_px: usize,
    ) -> f32 {
        let ids = ids_of(px, &[], &[], nearest);
        let view = view_of(&ids, px, &[], &[], width, height, stride_px);
        let near: Vec<f32> = ids.reps.iter().map(|&i| nearest[i]).collect();
        let mut claim = Claim::new(ids.len());
        view.img.claim(&mut claim, &near, |d| view.pts[d].dist(c));
        view.img.interior(&claim)
    }

    /// The old `straddle_fraction(px, px6_srgb, px6_lin, width, height, c, nearest, a, b,
    /// linear, stride_px)`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn straddle_fraction(
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
        let ids = ids_of(px, px6_srgb, px6_lin, nearest);
        let view = view_of(&ids, px, px6_srgb, px6_lin, width, height, stride_px);
        let near: Vec<f32> = ids.reps.iter().map(|&i| nearest[i]).collect();
        let mut claim = Claim::new(ids.len());
        view.img.claim(&mut claim, &near, |d| view.pts[d].dist(c));
        let hoods = view.img.neighbourhoods(&claim);
        let mut scratch = vec![0u8; ids.len() + 1];
        straddle(
            &view,
            &hoods,
            c,
            (0, 1),
            &InkSix::of(&[a, b]),
            linear,
            &mut scratch,
        )
    }
}
