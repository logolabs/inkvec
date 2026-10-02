//! The per-pixel two-ground palette passes as they shipped before the per-point rewrite,
//! kept verbatim as the oracle the rewrite is tested against (compiled only for tests).
//! The one later change is the rarity exemption in `extract_palette`, made in both.
#![allow(dead_code, clippy::too_many_arguments, unreachable_pub)]

use std::collections::HashMap;

use super::*;
use crate::color::{
    BLEND_INTERIOR_FRACTION, BLEND_STRADDLE_FRACTION, JND_FLOOR, MIN_INK_WEIGHT, PARAMS_PER_INK,
};

/// The shipped per-candidate rayon grain.
const PAR_MIN_LEN: usize = 8192;

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
        .with_min_len(PAR_MIN_LEN)
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
        .with_min_len(PAR_MIN_LEN)
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
        .with_min_len(PAR_MIN_LEN)
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

/// The shipped `extract_palette`, per pixel.
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
        // The rarity exemption of 2026-10-02 (`palette::rarity_exempt`: the first ink that
        // draws something is exempt, not only the first ink), applied here as well, so this
        // oracle still tests the per-point rewrite and nothing else.
        let exempt = colors.is_empty() || (!clear(&c) && colors.iter().all(|p: &Ink2| clear(p)));
        if (claim as f32 / total_px) < MIN_INK_WEIGHT && !exempt {
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
// The per-point rewrite against this reference
// ---------------------------------------------------------------------------------------

use crate::color::reference_tests::{random_art, same_palette, Lcg};

/// Random art with transparency: a clear ground, opaque and translucent shapes and
/// anti-aliased rims, as straight 8-bit RGBA composited over white the way the tracer does.
fn random_transparent(rng: &mut Lcg, w: usize, h: usize) -> (Vec<[f32; 3]>, Vec<f32>) {
    let paint = random_art(rng, w, h);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() / 255.0;
    let (cx, cy) = (rng.below(w as u64) as f32, rng.below(h as u64) as f32);
    let r = 1.0 + rng.unit() * w.max(h) as f32 * 0.5;
    let wash = q(0.1 + rng.unit() * 0.8);
    let mut alpha = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            alpha[y * w + x] = if d < r - 1.0 {
                if (x / 3 + y / 5) % 4 == 0 {
                    wash
                } else {
                    1.0
                }
            } else if d < r {
                q(r - d)
            } else {
                0.0
            };
        }
    }
    let rgb = paint
        .iter()
        .zip(&alpha)
        .map(|(p, &a)| p.map(|v| v * a + 1.0 * (1.0 - a)))
        .collect();
    (rgb, alpha)
}

#[test]
fn the_per_point_palette_and_labels_equal_the_per_pixel_ones() {
    let mut rng = Lcg(0xa1fa);
    for case in 0..120 {
        let (w, h) = match case % 12 {
            0 => (1, 1),
            1 => (300 + rng.below(30) as usize, 228),
            _ => (2 + rng.below(60) as usize, 2 + rng.below(60) as usize),
        };
        let (rgb, alpha) = random_transparent(&mut rng, w, h);
        let soft = rng.below(3) == 0;
        let ev = PaletteEvidence {
            sigma_noise: [0.0, 0.5 / 255.0, 3.0 / 255.0][rng.below(3) as usize],
            lambda: gradient::bic_lambda(w * h),
            noise_sigmas: if soft {
                color::SOFT_NOISE_SIGMAS
            } else {
                color::NOISE_SIGMAS
            },
            same_ink_de00: if soft {
                color::SOFT_SAME_INK_DE00
            } else {
                color::SAME_INK_DE00
            },
        };
        let max_colors = [1, 2, 64][rng.below(3) as usize];
        let md = color::DEFAULT_MERGE_DISTANCE;
        let old = extract_palette(&rgb, &alpha, w, h, md, max_colors, ev);
        let new = super::extract_palette(&rgb, &alpha, w, h, md, max_colors, ev);
        assert!(
            same_palette(&old, &new),
            "case {case}: {w}x{h} palettes differ"
        );
        assert_eq!(
            label_image(&rgb, &alpha, &old),
            super::label_image(&rgb, &alpha, &new),
            "case {case}"
        );
    }
}

#[test]
fn the_per_point_rewrite_matches_on_degenerate_shapes() {
    let mut rng = Lcg(3);
    let (rgb, alpha) = random_transparent(&mut rng, 9, 7);
    let ev = PaletteEvidence {
        sigma_noise: 0.002,
        lambda: 3.0,
        noise_sigmas: color::SOFT_NOISE_SIGMAS,
        same_ink_de00: color::SAME_INK_DE00,
    };
    let md = color::DEFAULT_MERGE_DISTANCE;
    for (n_rgb, n_a, w, h) in [
        (0, 0, 0, 0),
        (63, 40, 9, 7),
        (40, 63, 9, 7),
        (63, 63, 5, 7),
        (63, 63, 0, 7),
    ] {
        let (r, a) = (&rgb[..n_rgb], &alpha[..n_a]);
        let old = extract_palette(r, a, w, h, md, 64, ev);
        let new = super::extract_palette(r, a, w, h, md, 64, ev);
        assert!(
            same_palette(&old, &new),
            "{w}x{h} over {n_rgb}/{n_a} pixels"
        );
        assert_eq!(label_image(r, a, &old), super::label_image(r, a, &new));
    }
}

/// The shipped four-channel `reassign_blend_pixels`: every pixel, every round.
fn old_reassign4(
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

#[test]
fn four_channel_active_set_reassignment_equals_every_pixel_every_round() {
    let mut rng = Lcg(0x4ea5);
    for case in 0..100 {
        let (w, h) = (2 + rng.below(50) as usize, 2 + rng.below(50) as usize);
        let (rgb, alpha) = random_transparent(&mut rng, w, h);
        let pal = super::extract_palette(
            &rgb,
            &alpha,
            w,
            h,
            color::DEFAULT_MERGE_DISTANCE,
            64,
            PaletteEvidence::default(),
        );
        let mut labels = super::label_image(&rgb, &alpha, &pal);
        for l in labels.iter_mut() {
            if rng.below(12) == 0 {
                *l = rng.below(pal.len() as u64) as u16;
            }
        }
        let (px, inks) = (rgba_w(&rgb, &alpha), ink_rgba_w(&pal));
        let sigma = [0.0, 0.004, 0.02][case % 3];
        let (mut a, mut b) = (labels.clone(), labels);
        let na = old_reassign4(&mut a, &px, w, h, &inks, sigma);
        let nb = reassign_blend_pixels(&mut b, &px, w, h, &inks, sigma);
        assert_eq!((na, &a), (nb, &b), "case {case} {w}x{h}");
    }
}
