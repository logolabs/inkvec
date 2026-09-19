//! Transparency carried natively: an ink is a colour *and* an opacity, and the transparent
//! ground is an ink like any other.
//!
//! The classic path composites the image onto a white matte and traces what is left, so a
//! white mark on a transparent ground is one flat colour, a translucent wash is baked into
//! whatever it resembles, and a glow becomes a colour ramp towards the matte. Here nothing
//! is thrown away: every pixel is kept as its colour over white `W` together with its alpha
//! `a`. That pair is an invertible transform of premultiplied RGBA, so an anti-aliased pixel
//! between two inks is still a straight-line blend of them in all four channels -- between
//! two opaque inks, between an ink and the clear ground, or at a junction of both -- and
//! every stage that unmixes linearly needs one more channel and nothing else.
//!
//! Perceptual comparisons use the colour over both grounds, `W` and `K = W - (1 - a)` (over
//! black): two inks are one ink only if they look the same on white *and* on black. For an
//! opaque colour `K = W` and this is plain OKLab distance; white paint and the clear ground
//! are as far apart as white and black.
//!
//! Opaque input never reaches this module. `trace_color_full_with_alpha` dispatches here
//! only when the caller asked for native alpha and the source has transparency, so the
//! classic path is untouched by construction.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::color::{
    self, de00, linear_to_srgb, oklab_to_rgb, rgb_to_oklab, srgb_to_linear, Oklab, Palette,
    PaletteEvidence, BLEND_INTERIOR_FRACTION, BLEND_STRADDLE_FRACTION, JND_FLOOR, MIN_INK_WEIGHT,
    PARAMS_PER_INK,
};
use crate::{coverage, diag, gradient, regularize, ColorOptions, ColorTrace, Rgba, Stopwatch};

/// Alpha at or above which a pixel or an ink is opaque.
pub const OPAQUE: f32 = 0.999;

/// A colour seen over both grounds: `w` over white, `k` over black, in OKLab. For an opaque
/// colour the two are the same point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink2 {
    /// Over white.
    pub w: Oklab,
    /// Over black.
    pub k: Oklab,
}

impl Ink2 {
    /// An opaque colour: the same point over either ground.
    pub fn opaque(w: Oklab) -> Self {
        Ink2 { w, k: w }
    }

    /// The larger of the two grounds' OKLab distances. Plain OKLab distance when both
    /// colours are opaque.
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
/// ground where it belongs. `INKVEC_NATIVE_GROUND` overrides it.
pub fn second_ground() -> f32 {
    static G: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *G.get_or_init(|| {
        std::env::var("INKVEC_NATIVE_GROUND")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|g| (0.0..0.95).contains(g))
            .unwrap_or(0.5)
    })
}

/// Opacity pinned to exactly 0 or 1 when it is within measurement of either.
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

/// Every palette entry as a two-ground point.
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
pub fn rgba_w(rgb: &[[f32; 3]], alpha: &[f32]) -> Vec<[f32; 4]> {
    rgb.iter()
        .zip(alpha)
        .map(|(c, &a)| [c[0], c[1], c[2], a.clamp(0.0, 1.0)])
        .collect()
}

/// Each palette entry as `[W, a]`.
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

const BINS: usize = 24;

fn bin(c: Oklab) -> u64 {
    let li = ((c.l.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u64).min(BINS as u64 - 1);
    let ai = (((c.a + 0.4) / 0.8).clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u64;
    let bi = (((c.b + 0.4) / 0.8).clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u64;
    li * (BINS * BINS) as u64 + ai * BINS as u64 + bi
}

fn claim_spread(
    px: &[Ink2],
    nearest_px: &[f32],
    c: Ink2,
    tol: f32,
    stride_px: usize,
) -> (usize, f32) {
    const MAX_SAMPLES: usize = 8192;
    let stride = (px.len() / MAX_SAMPLES).max(1);
    let n = (0..px.len())
        .into_par_iter()
        .step_by(stride_px)
        .filter(|&i| px[i].dist(c) < nearest_px[i])
        .count();
    let mut d_in: Vec<f32> = (0..px.len())
        .into_par_iter()
        .step_by(stride_px)
        .filter_map(|i| {
            let d = px[i].dist(c);
            (d < nearest_px[i] && d < tol && i % stride == 0).then_some(d)
        })
        .collect();
    let n = n * stride_px;
    if d_in.is_empty() {
        return (n, 0.0);
    }
    d_in.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (n, d_in[d_in.len() / 2])
}

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
    let mask: Vec<bool> = px
        .par_iter()
        .zip(nearest.par_iter())
        .map(|(&p, &d)| p.dist(c) < d)
        .collect();
    let (total, interior) = (0..width * height)
        .into_par_iter()
        .step_by(stride_px)
        .filter(|&i| mask[i])
        .map(|i| {
            let (x, y) = (i % width, i / width);
            let ok = (x == 0 || mask[i - 1])
                && (x + 1 == width || mask[i + 1])
                && (y == 0 || mask[i - width])
                && (y + 1 == height || mask[i + width]);
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
fn six(c: Ink2, linear: bool) -> [f32; 6] {
    let (w, k) = (oklab_to_rgb(c.w), oklab_to_rgb(c.k));
    let f = |v: f32| if linear { srgb_to_linear(v) } else { v };
    [f(w[0]), f(w[1]), f(w[2]), f(k[0]), f(k[1]), f(k[2])]
}

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
fn blend_pairs(c: Ink2, accepted: &[Ink2], tol: f32) -> Vec<(usize, usize, bool, f32)> {
    let mut out = Vec::new();
    if accepted.len() < 2 {
        return out;
    }
    let tmin: f32 = std::env::var("INKVEC_BLEND_TMIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.04);
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
    let mask: Vec<bool> = px
        .par_iter()
        .zip(nearest.par_iter())
        .map(|(&p, &n)| p.dist(c) < n)
        .collect();
    let cache = if linear { px6_lin } else { px6_srgb };
    let t: Vec<f32> = cache.par_iter().map(t_of).collect();
    let (total, straddle) = (0..width * height)
        .into_par_iter()
        .step_by(stride_px)
        .filter(|&i| mask[i])
        .map(|i| {
            let (x, y) = (i % width, i / width);
            let (mut lower, mut higher) = (false, false);
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                        continue;
                    }
                    let tn = t[ny as usize * width + nx as usize];
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
    let px = pixel_points(rgb, alpha);
    let stride_px = color::stat_stride(px.len(), width);
    let px6_srgb: Vec<[f32; 6]> = px.par_iter().map(|&p| six(p, false)).collect();
    let px6_lin: Vec<[f32; 6]> = px.par_iter().map(|&p| six(p, true)).collect();

    // Modes in pixel order, so the centroids are the same on every run.
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

    let total_px = px.len().max(1) as f32;
    let mut colors: Vec<Ink2> = Vec::new();
    let mut nearest_px: Vec<f32> = vec![f32::INFINITY; px.len()];
    let paldbg = std::env::var("INKVEC_PALDBG").is_ok();
    for &(n, _key, c) in &modes {
        if colors.len() >= max_colors {
            break;
        }
        let (claim, spread) = claim_spread(&px, &nearest_px, c, merge_distance, stride_px);
        if (claim as f32 / total_px) < MIN_INK_WEIGHT && !colors.is_empty() {
            continue;
        }
        let nearest = colors
            .iter()
            .map(|&p| p.dist(c))
            .fold(f32::INFINITY, f32::min);
        let k = std::env::var("INKVEC_NOISE_SIGMAS")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(noise_sigmas);
        let reach = k * spread;
        if let Some(&near_ink) = colors.iter().min_by(|&&p, &&q| {
            p.dist(c)
                .partial_cmp(&q.dist(c))
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            if c.de00(near_ink) < same_ink_de00 {
                continue;
            }
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
        let pairs = blend_pairs(c, &colors, merge_distance * 1.6);
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
            interior_fraction(&px, width, height, c, &nearest_px, stride_px)
        } else {
            1.0
        };
        if translucent && !blend && interior < BLEND_INTERIOR_FRACTION {
            continue;
        }
        let straddle = if blend && interior < BLEND_INTERIOR_FRACTION {
            pairs
                .iter()
                .map(|&(i, j, linear, _)| {
                    straddle_fraction(
                        &px,
                        &px6_srgb,
                        &px6_lin,
                        width,
                        height,
                        c,
                        &nearest_px,
                        colors[i],
                        colors[j],
                        linear,
                        stride_px,
                    )
                })
                .fold(0.0f32, f32::max)
        } else {
            0.0
        };
        if paldbg {
            eprintln!(
                "  native cand w={} k={} a={:.3} bin={n} claim={claim} near={nearest:.4} blend={blend} interior={interior:.3} straddle={straddle:.3}",
                color::to_hex(oklab_to_rgb(c.w)),
                color::to_hex(oklab_to_rgb(c.k)),
                c.alpha()
            );
        }
        if blend && interior < BLEND_INTERIOR_FRACTION && straddle >= BLEND_STRADDLE_FRACTION {
            continue;
        }
        nearest_px
            .par_iter_mut()
            .zip(px.par_iter())
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

    // Refine each ink to the mean of the pixels that chose it, over both grounds.
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

/// Every pixel to its nearest ink, over both grounds.
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
pub fn absorb_blend_slivers(
    labels: &mut [u16],
    px: &[[f32; 4]],
    w: usize,
    h: usize,
    inks: &[[f32; 4]],
    sigma_noise: f64,
) -> usize {
    let n = w * h;
    let tol = (3.0 * sigma_noise).max(0.025) as f32;
    let mut absorbed = 0usize;
    for _round in 0..2 {
        let mut comp = vec![u32::MAX; n];
        let mut members: Vec<Vec<usize>> = Vec::new();
        for start in 0..n {
            if comp[start] != u32::MAX {
                continue;
            }
            let id = members.len() as u32;
            let lab = labels[start];
            let mut stack = vec![start];
            let mut group = Vec::new();
            comp[start] = id;
            while let Some(p) = stack.pop() {
                group.push(p);
                let (x, y) = (p % w, p / w);
                for q in [
                    (x > 0).then(|| p - 1),
                    (x + 1 < w).then(|| p + 1),
                    (y > 0).then(|| p - w),
                    (y + 1 < h).then(|| p + w),
                ]
                .into_iter()
                .flatten()
                {
                    if comp[q] == u32::MAX && labels[q] == lab {
                        comp[q] = id;
                        stack.push(q);
                    }
                }
            }
            members.push(group);
        }
        let mut changed = 0usize;
        let n_labels = labels
            .iter()
            .copied()
            .max()
            .map_or(1, |m| m as usize + 1)
            .max(inks.len());
        let mut contacts: Vec<usize> = vec![0; n_labels];
        for group in &members {
            let area = group.len();
            if area == 0 {
                continue;
            }
            let id = comp[group[0]];
            let mut interior = 0usize;
            contacts.iter_mut().for_each(|c| *c = 0);
            let mut foreign = 0usize;
            for &p in group {
                let (x, y) = (p % w, p / w);
                let mut inside = true;
                for q in [
                    (x > 0).then(|| p - 1),
                    (x + 1 < w).then(|| p + 1),
                    (y > 0).then(|| p - w),
                    (y + 1 < h).then(|| p + w),
                ]
                .into_iter()
                .flatten()
                {
                    if comp[q] != id {
                        inside = false;
                        contacts[labels[q] as usize] += 1;
                        foreign += 1;
                    }
                }
                if inside {
                    interior += 1;
                }
            }
            if interior * 5 >= area || foreign == 0 {
                continue;
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
                continue;
            }
            let mut labs: Vec<Option<u16>> = tally.iter().map(|&(_, l)| Some(l)).collect();
            let mut cols: Vec<[f32; 4]> = Vec::with_capacity(4);
            for l in &labs {
                let Some(&c) = inks.get(l.unwrap() as usize) else {
                    continue;
                };
                cols.push(c);
            }
            if cols.len() != labs.len() || cols.len() < 2 {
                continue;
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
                        Some((_, w2)) => labs[w2].unwrap(),
                        None => tally[0].1,
                    },
                });
            }
            if pass * 5 < area * 4 {
                continue;
            }
            for (&p, &l) in group.iter().zip(&dest) {
                labels[p] = l;
            }
            changed += 1;
        }
        absorbed += changed;
        if changed == 0 {
            break;
        }
    }
    absorbed
}

/// [`crate::regions::reassign_blend_pixels`] in four channels.
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
                    Some((_, w2)) => keep[w2].unwrap(),
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
// Fades
// ---------------------------------------------------------------------------------------

/// A face whose opacity varies across it: a glow, a soft shadow, a vignette, a flame's
/// halo. Written as one gradient carrying `stop-color` and `stop-opacity` at each stop.
#[derive(Debug, Clone, PartialEq)]
pub struct Fade {
    /// The colour profile, straight (not composited over anything), on the geometry and
    /// at the stop offsets of `alpha`. Most fades in the corpus change colour as they fade
    /// (175 of the 308 files that use `stop-opacity`), so this is a profile, not a colour.
    pub color: gradient::FillModel,
    /// The opacity profile, as a fill model whose every stop is a grey equal to the
    /// opacity there: the geometry and the stops of an ordinary gradient.
    pub alpha: gradient::FillModel,
}

/// A model's stops in order, `(offset, colour)`: one stop for a flat fill.
pub(crate) fn model_stops(m: &gradient::FillModel) -> Vec<(f64, [f32; 3])> {
    match m {
        gradient::FillModel::Flat(c) => vec![(0.0, *c)],
        gradient::FillModel::Linear { c0, c1, mids, .. }
        | gradient::FillModel::Radial { c0, c1, mids, .. } => {
            let mut v = vec![(0.0, *c0)];
            v.extend(mids.iter().copied());
            v.push((1.0, *c1));
            v
        }
    }
}

/// `m` with its stop colours replaced, in order; offsets and geometry kept.
fn restop(m: &gradient::FillModel, cols: &[[f32; 3]]) -> gradient::FillModel {
    match m {
        gradient::FillModel::Flat(_) => gradient::FillModel::Flat(cols[0]),
        gradient::FillModel::Linear { mids, .. } | gradient::FillModel::Radial { mids, .. } => {
            let k = mids.len();
            m.with_stops(
                cols[0],
                mids.iter()
                    .zip(&cols[1..=k])
                    .map(|(&(o, _), &c)| (o, c))
                    .collect(),
                cols[k + 1],
            )
        }
    }
}

impl Fade {
    /// The lowest opacity the profile reaches: at a fade's rim, where it meets the ground.
    pub fn rim_alpha(&self) -> f32 {
        let stops = |m: &gradient::FillModel| -> Vec<f32> {
            match m {
                gradient::FillModel::Flat(c) => vec![c[0]],
                gradient::FillModel::Linear { c0, c1, mids, .. }
                | gradient::FillModel::Radial { c0, c1, mids, .. } => {
                    let mut v = vec![c0[0], c1[0]];
                    v.extend(mids.iter().map(|m| m.1[0]));
                    v
                }
            }
        };
        stops(&self.alpha).into_iter().fold(1.0f32, f32::min)
    }

    /// The same fade as a fill over white, which is how every other stage sees a face:
    /// `W = s·a + (1 - a)`, linear in the gradient coordinate exactly as `a` is.
    pub fn over_white(&self) -> gradient::FillModel {
        let a_stops = model_stops(&self.alpha);
        let c_stops = model_stops(&self.color);
        let cols: Vec<[f32; 3]> = a_stops
            .iter()
            .enumerate()
            .map(|(i, &(_, ag))| {
                let a = ag[0].clamp(0.0, 1.0);
                let s = c_stops.get(i).or(c_stops.last()).map_or([1.0; 3], |c| c.1);
                [s[0] * a + 1.0 - a, s[1] * a + 1.0 - a, s[2] * a + 1.0 - a]
            })
            .collect();
        restop(&self.alpha, &cols)
    }
}

/// Solve the small symmetric system `a·x = b` by Gaussian elimination with partial
/// pivoting; `None` when it is singular.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<[f64; 3]>) -> Option<Vec<[f64; 3]>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            for k in col..n {
                a[row][k] -= f * a[col][k];
            }
            for c in 0..3 {
                b[row][c] -= f * b[col][c];
            }
        }
    }
    let mut x = vec![[0.0f64; 3]; n];
    for row in (0..n).rev() {
        for c in 0..3 {
            let mut s = b[row][c];
            for k in row + 1..n {
                s -= a[row][k] * x[k][c];
            }
            x[row][c] = s / a[row][row];
        }
    }
    Some(x)
}

/// The colour profile of a fade, on the geometry and stops of its opacity profile.
///
/// With the geometry fixed every pixel has its gradient coordinate `t`, and the colour is
/// piecewise linear in `t` between the stops, so the stop colours are a small linear least
/// squares. Weighted by `a²`, which is the residual in premultiplied colour -- what the
/// pixel actually shows -- so a pixel too faint to see cannot steer the colour. A stop no
/// pixel testifies about (a halo's inner stop under an opaque flame) is held to the fade's
/// mean colour by a light ridge.
fn fit_colour_stops(
    alpha_model: &gradient::FillModel,
    px: &[usize],
    rgb: &[[f32; 3]],
    alpha: &[f32],
    w: usize,
) -> gradient::FillModel {
    let offs: Vec<f64> = model_stops(alpha_model).iter().map(|s| s.0).collect();
    let m = offs.len();
    let mut a = vec![vec![0.0f64; m]; m];
    let mut b = vec![[0.0f64; 3]; m];
    let (mut mean, mut msum) = ([0.0f64; 3], 0.0f64);
    for &p in px {
        let ap = alpha[p] as f64;
        if ap < 1e-3 {
            continue;
        }
        let pm = [
            (rgb[p][0] as f64 - (1.0 - ap)),
            (rgb[p][1] as f64 - (1.0 - ap)),
            (rgb[p][2] as f64 - (1.0 - ap)),
        ];
        for k in 0..3 {
            mean[k] += pm[k];
        }
        msum += ap;
        // s(t) = (1-u)·S_j + u·S_{j+1}; residual a·s(t) - pm, so the design row is a·basis.
        let t = alpha_model.t_at((p % w) as f64, (p / w) as f64);
        let j = (0..m - 1).rfind(|&j| t >= offs[j]).unwrap_or(0);
        let span = (offs[j + 1] - offs[j]).max(1e-9);
        let u = ((t - offs[j]) / span).clamp(0.0, 1.0);
        let (r0, r1) = (ap * (1.0 - u), ap * u);
        a[j][j] += r0 * r0;
        a[j][j + 1] += r0 * r1;
        a[j + 1][j] += r0 * r1;
        a[j + 1][j + 1] += r1 * r1;
        for k in 0..3 {
            b[j][k] += r0 * pm[k];
            b[j + 1][k] += r1 * pm[k];
        }
    }
    let mean = if msum > 1e-9 {
        [mean[0] / msum, mean[1] / msum, mean[2] / msum]
    } else {
        [1.0; 3]
    };
    let ridge = 1e-6 + 1e-3 * (0..m).map(|i| a[i][i]).sum::<f64>() / m as f64;
    for i in 0..m {
        a[i][i] += ridge;
        for k in 0..3 {
            b[i][k] += ridge * mean[k];
        }
    }
    let x = solve(a, b).unwrap_or_else(|| vec![mean; m]);
    let cols: Vec<[f32; 3]> = x
        .iter()
        .map(|c| {
            [
                c[0].clamp(0.0, 1.0) as f32,
                c[1].clamp(0.0, 1.0) as f32,
                c[2].clamp(0.0, 1.0) as f32,
            ]
        })
        .collect();
    restop(alpha_model, &cols)
}

/// Chi-square of a model of a region's pixels -- opacity and premultiplied colour, each
/// beyond the half-level quantisation dead zone -- where `model(p)` is `(colour, alpha)`.
fn fade_chi2(
    px: &[usize],
    rgb: &[[f32; 3]],
    alpha: &[f32],
    sigma: f64,
    model: impl Fn(usize) -> ([f32; 3], f32),
) -> f64 {
    const DEAD: f64 = 0.5 / 255.0;
    let r = |e: f64| {
        let e = (e.abs() - DEAD).max(0.0) / sigma;
        e * e
    };
    px.iter()
        .map(|&p| {
            let (s, am) = model(p);
            let (ap, am) = (alpha[p] as f64, am as f64);
            let mut c2 = r(ap - am);
            for k in 0..3 {
                let pm = rgb[p][k] as f64 - (1.0 - ap);
                c2 += r(pm - s[k] as f64 * am);
            }
            c2
        })
        .sum()
}

/// Editable numbers in an opacity profile: the geometry, and one number per stop where a
/// colour stop has three.
fn alpha_params(m: &gradient::FillModel) -> f64 {
    match m {
        gradient::FillModel::Flat(_) => 1.0,
        gradient::FillModel::Linear { mids, .. } => 4.0 + 2.0 + 2.0 * mids.len() as f64,
        gradient::FillModel::Radial { aspect, mids, .. } => {
            let geom = if *aspect == 1.0 { 3.0 } else { 5.0 };
            geom + 2.0 + 2.0 * mids.len() as f64
        }
    }
}

/// An opacity profile fitted by the ordinary fill fitter, run on the alpha as a grey image.
/// Only sRGB-space candidates: `stop-opacity` interpolates linearly in opacity, and a
/// linear-light fit of a grey would be a different curve. The grey repeats the alpha in
/// three channels, so its chi-square counts the evidence three times; the cost here counts
/// it once, and prices each stop at one number.
fn fit_opacity(
    grey: &[[f32; 3]],
    w: usize,
    h: usize,
    pixels: &[usize],
    member: impl Fn(usize) -> bool + Sync,
    sigma: f64,
    lambda: f64,
    flat_only: bool,
) -> (gradient::FillModel, f64) {
    gradient::fit_pixels(grey, w, h, pixels, member, |_| true, sigma, lambda)
        .into_iter()
        .filter(|f| match &f.model {
            gradient::FillModel::Flat(_) => true,
            gradient::FillModel::Linear { interp, .. }
            | gradient::FillModel::Radial { interp, .. } => {
                !flat_only && *interp == gradient::Interp::Srgb
            }
        })
        .map(|f| {
            let cost = 0.5 * f.chi2 / 3.0 + lambda * alpha_params(&f.model);
            (f.model, cost)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((gradient::FillModel::Flat([1.0; 3]), f64::INFINITY))
}

/// Turn each connected run of translucent regions into one fade, where one opacity profile
/// and one colour profile on it cost less than the separate washes.
///
/// The palette finds a fade as bands, one ink per opacity level, and the colour merge
/// cannot join them: over white a white glow is white everywhere, and the difference is all
/// in the alpha. Here translucent regions are gathered into connected clusters; the ordinary
/// fill fitter is asked for the geometry from the alpha alone -- linear, radial or
/// elliptical, with interior stops -- then the colour stops are fitted on that geometry,
/// and the whole is priced against the separate washes by the same description length as
/// every other merge, opacity and premultiplied colour residuals both. A single region
/// whose opacity ramps is a fade on its own.
///
/// Returns, per label id (new ids included), the fade that label is, if any.
#[allow(clippy::too_many_arguments)]
fn merge_fades(
    labels: &mut [u16],
    fills_by_label: &mut Vec<gradient::FillFit>,
    label_ink: &mut Vec<usize>,
    rgb: &[[f32; 3]],
    alpha: &[f32],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma: f64,
    lambda: f64,
) -> Vec<Option<Fade>> {
    /// An opacity at or above this is paint, not a fade.
    const OPAQUE_BAND: f32 = 0.98;
    let n = w * h;
    let n_labels = labels
        .iter()
        .map(|&l| l as usize + 1)
        .max()
        .unwrap_or(0)
        .max(fills_by_label.len())
        .max(label_ink.len())
        .max(pal.len());
    let ink_of = |l: usize, label_ink: &[usize]| label_ink.get(l).copied().unwrap_or(l);

    // Each translucent label's colour, from its own pixels: every pixel of a colour `s` at
    // opacity `a` is `W - (1 - a) = s·a` exactly, so `s = Σ(W - (1 - a)) / Σa` whatever the
    // opacities are. Un-matting the band's one colour by its one opacity instead divides a
    // colour error by `a`: a black shadow's faint bands came out #353535 and #2a2a2a and were
    // never recognised as one colour.
    let mut pm = vec![([0.0f64; 3], 0.0f64); n_labels];
    for p in 0..n {
        let (c, a) = (rgb[p], alpha[p]);
        let e = &mut pm[labels[p] as usize];
        for k in 0..3 {
            e.0[k] += (c[k] - (1.0 - a)) as f64;
        }
        e.1 += a as f64;
    }
    let colour_of = |acc: &([f64; 3], f64)| -> Option<[f32; 3]> {
        (acc.1 > 1e-6).then(|| {
            [
                (acc.0[0] / acc.1).clamp(0.0, 1.0) as f32,
                (acc.0[1] / acc.1).clamp(0.0, 1.0) as f32,
                (acc.0[2] / acc.1).clamp(0.0, 1.0) as f32,
            ]
        })
    };

    // The washes: every translucent label, whatever fill the colour merge gave it. A band
    // of a fade carries a stretch of the ramp inside it, so the merge over white often fits
    // it a colour gradient -- five of the candle halo's did -- and a colour gradient over
    // white is not something a translucent face can be written as: its stops already hold
    // the white, and `fill-opacity` would apply it twice. The fit below starts from the
    // pixels, opacity and colour both, so the fill it replaces does not matter.
    let wash: Vec<bool> = (0..n_labels)
        .map(|l| {
            let a = pal.alpha.get(ink_of(l, label_ink)).copied().unwrap_or(1.0);
            (0.05..OPAQUE_BAND).contains(&a) && colour_of(&pm[l]).is_some()
        })
        .collect();
    if !wash.iter().any(|&b| b) {
        return vec![None; n_labels];
    }

    // Connected clusters of wash pixels. Colour does not split them: two washes of
    // different colours join only if one colour profile explains both, and the cost below
    // says whether it does.
    let mut cluster = vec![u32::MAX; n];
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for start in 0..n {
        if !wash[labels[start] as usize] || cluster[start] != u32::MAX {
            continue;
        }
        let id = clusters.len() as u32;
        let mut stack = vec![start];
        let mut px = Vec::new();
        cluster[start] = id;
        while let Some(p) = stack.pop() {
            px.push(p);
            let (x, y) = (p % w, p / w);
            for q in [
                (x > 0).then(|| p - 1),
                (x + 1 < w).then(|| p + 1),
                (y > 0).then(|| p - w),
                (y + 1 < h).then(|| p + w),
            ]
            .into_iter()
            .flatten()
            {
                if cluster[q] == u32::MAX && wash[labels[q] as usize] {
                    cluster[q] = id;
                    stack.push(q);
                }
            }
        }
        px.sort_unstable();
        clusters.push(px);
    }

    let grey: Vec<[f32; 3]> = alpha.iter().map(|&a| [a, a, a]).collect();
    let mut fade_of: Vec<Option<Fade>> = vec![None; n_labels];
    let mut next = n_labels;
    let dbg = std::env::var_os("INKVEC_FADEDBG").is_some();
    if dbg {
        let translucent_gradient = (0..n_labels)
            .filter(|&l| {
                let a = pal.alpha.get(ink_of(l, label_ink)).copied().unwrap_or(1.0);
                (0.05..OPAQUE_BAND).contains(&a)
                    && fills_by_label.get(l).is_some_and(|f| f.model.is_gradient())
            })
            .count();
        eprintln!(
            "  fades: {} washes, {} translucent labels with a colour gradient, {} clusters",
            wash.iter().filter(|&&b| b).count(),
            translucent_gradient,
            clusters.len()
        );
    }
    for (cid, px) in clusters.iter().enumerate() {
        let mut bands: Vec<u16> = px.iter().map(|&p| labels[p]).collect();
        bands.sort_unstable();
        bands.dedup();
        // One band is enough: a single region whose opacity ramps is a fade on its own.
        if px.len() < 16 || next >= u16::MAX as usize {
            if dbg {
                eprintln!(
                    "  fade cluster {cid}: {} band(s), {} px -- too small",
                    bands.len(),
                    px.len()
                );
            }
            continue;
        }
        let cid = cid as u32;
        // The geometry comes from the opacity, which is what a fade is.
        let (alpha_model, _) =
            fit_opacity(&grey, w, h, px, |p| cluster[p] == cid, sigma, lambda, false);
        if !alpha_model.is_gradient() {
            if dbg {
                eprintln!(
                    "  fade cluster {cid}: {} bands, {} px -- opacity fits flat",
                    bands.len(),
                    px.len()
                );
            }
            continue;
        }
        let color_model = fit_colour_stops(&alpha_model, px, rgb, alpha, w);
        let n_stops = model_stops(&alpha_model).len() as f64;
        let union_chi2 = fade_chi2(px, rgb, alpha, sigma, |p| {
            let (x, y) = ((p % w) as f64, (p / w) as f64);
            (color_model.color_at(x, y), alpha_model.color_at(x, y)[0])
        });
        let union = 0.5 * union_chi2 + lambda * (alpha_params(&alpha_model) + 3.0 * n_stops);
        // The separate washes: each band one opacity and one colour of its own.
        let mut separate = 0.0;
        for &b in &bands {
            let bp: Vec<usize> = px.iter().copied().filter(|&p| labels[p] == b).collect();
            let Some(s) = colour_of(&pm[b as usize]) else {
                continue;
            };
            let mut av: Vec<f32> = bp.iter().map(|&p| alpha[p]).collect();
            let mid = av.len() / 2;
            let a_b = *av.select_nth_unstable_by(mid, |x, y| x.total_cmp(y)).1;
            separate += 0.5 * fade_chi2(&bp, rgb, alpha, sigma, |_| (s, a_b)) + lambda * 4.0;
        }
        if dbg {
            eprintln!(
                "  fade cluster {cid}: {} bands, {} px, {} union {union:.1} vs separate {separate:.1}",
                bands.len(),
                px.len(),
                alpha_model.kind()
            );
        }
        if union >= separate {
            continue;
        }
        let fade = Fade {
            color: color_model,
            alpha: alpha_model,
        };
        let id = next;
        next += 1;
        let first_ink = ink_of(bands[0] as usize, label_ink);
        for &p in px {
            labels[p] = id as u16;
        }
        // Any label without a fill of its own falls back to its palette colour downstream;
        // padding must say the same, not invent one.
        while fills_by_label.len() <= id {
            let l = fills_by_label.len();
            fills_by_label.push(gradient::FillFit {
                model: gradient::FillModel::Flat(pal.rgb.get(l).copied().unwrap_or([1.0; 3])),
                chi2: 0.0,
                params: gradient::PARAMS_FLAT,
                cost: 0.0,
            });
        }
        fills_by_label[id] = gradient::FillFit {
            model: fade.over_white(),
            chi2: union_chi2,
            params: alpha_params(&fade.alpha) + 3.0 * n_stops,
            cost: union,
        };
        if label_ink.len() <= id {
            let len = label_ink.len();
            label_ink.extend(len..=id);
        }
        label_ink[id] = first_ink;
        if fade_of.len() <= id {
            fade_of.resize(id + 1, None);
        }
        fade_of[id] = Some(fade);
    }

    // The washes that stay washes get the same colour estimate. The emitter recovers a
    // wash's colour by un-matting its fill at its opacity, so the fill is written as that
    // colour over white at that opacity, and the division by `a` recovers it exactly.
    let still_present: std::collections::HashSet<u16> = labels.iter().copied().collect();
    for l in 0..n_labels {
        if !wash[l] || !still_present.contains(&(l as u16)) {
            continue;
        }
        let (Some(s), Some(&a)) = (colour_of(&pm[l]), pal.alpha.get(ink_of(l, label_ink))) else {
            continue;
        };
        while fills_by_label.len() <= l {
            let k = fills_by_label.len();
            fills_by_label.push(gradient::FillFit {
                model: gradient::FillModel::Flat(pal.rgb.get(k).copied().unwrap_or([1.0; 3])),
                chi2: 0.0,
                params: gradient::PARAMS_FLAT,
                cost: 0.0,
            });
        }
        fills_by_label[l].model =
            gradient::FillModel::Flat([s[0] * a + 1.0 - a, s[1] * a + 1.0 - a, s[2] * a + 1.0 - a]);
    }
    fade_of
}

// ---------------------------------------------------------------------------------------
// The colour path
// ---------------------------------------------------------------------------------------

/// Two palette entries may be merged into one gradient only when they are drawn at the
/// same opacity: a fill here is fitted over white, where the clear ground and white paint
/// are the same colour.
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
    let mut labels = label_image(&rgb, alpha, &pal);
    if soft_intake && std::env::var_os("INKVEC_NO_MEASURED_SIGMA").is_none() {
        let cap = color::MEASURED_SIGMA_CAP / 255.0;
        let measured = (regularize::residual_sigma(&rgb, &labels, w, h, &pal)
            * color::MEASURED_SIGMA_SCALE)
            .min(cap);
        sigma_noise = sigma_noise.max(measured);
    }
    sw.mark("labels");

    crate::despeckle(&mut labels, w, h, min_region);
    sw.mark("despeckle");

    if std::env::var_os("INKVEC_NO_ABSORB").is_none() {
        let px4 = rgba_w(&rgb, alpha);
        let inks4 = ink_rgba_w(&pal);
        let absorbed = absorb_blend_slivers(&mut labels, &px4, w, h, &inks4, sigma_noise);
        let moved = reassign_blend_pixels(&mut labels, &px4, w, h, &inks4, sigma_noise);
        if absorbed > 0 || moved > 0 {
            crate::despeckle(&mut labels, w, h, min_region);
        }
    }
    sw.mark("blend_absorb");

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

    if opts.gradients && std::env::var_os("INKVEC_NO_CARVE").is_none() {
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

    let fade_of_label = if opts.gradients && std::env::var_os("INKVEC_NO_FADES").is_none() {
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
    let _ = diag::Stop::Floor;
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
