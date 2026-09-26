//! Colour space and palette recovery (DESIGN.md S1).
//!
//! Two decisions here, both of which VTracer gets wrong in ways `M0-BASELINE.md`
//! measures:
//!
//! **Work in OKLab, not RGB.** VTracer's `color_precision` truncates significant bits per
//! RGB channel, and RGB distance is not perceptual distance — so it simultaneously splits
//! colours a viewer cannot tell apart and merges ones they can. In OKLab, Euclidean
//! distance is approximately perceptually uniform by construction, so a single threshold
//! means the same thing everywhere in the space.
//!
//! **Find modes, not bins.** In flat art the palette entries are the *frequent* colours;
//! anti-aliased pixels are blends, individually rare and spread along the lines between
//! palette entries. Picking modes by frequency therefore recovers the artist's palette
//! and leaves the AA pixels to be explained as coverage — which is exactly what S2 then
//! does with them. Quantizing by bin instead turns every anti-aliased ramp into its own
//! set of spurious colours, which is where gradient banding and the 44x parameter blow-up
//! on `gradient_linear` come from.

/// A colour in OKLab. Euclidean distance here is approximately perceptually uniform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklab {
    /// Lightness.
    pub l: f32,
    /// Green-red axis.
    pub a: f32,
    /// Blue-yellow axis.
    pub b: f32,
}

impl Oklab {
    /// Euclidean distance to another colour in this space.
    #[inline]
    pub fn dist(self, o: Oklab) -> f32 {
        let (dl, da, db) = (self.l - o.l, self.a - o.a, self.b - o.b);
        (dl * dl + da * da + db * db).sqrt()
    }
}

#[inline]
pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
pub(crate) fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Cluster in plain sRGB instead of OKLab. Not a shipping option: it is here so the
/// question "what if grouping were done in RGB" can be answered with a measurement
/// rather than an argument. Both transforms honour it, so they stay inverses.
/// `INKVEC_PALETTE_RGB`, in a `research` build only; a constant `false` otherwise.
#[inline]
fn palette_rgb_space() -> bool {
    #[cfg(feature = "research")]
    {
        // Asked once per pixel, so cached here rather than looked up each time.
        static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *F.get_or_init(|| inkvec_core::env::flag("INKVEC_PALETTE_RGB"))
    }
    #[cfg(not(feature = "research"))]
    false
}

/// sRGB in `[0, 1]` to OKLab (Ottosson).
pub fn rgb_to_oklab(rgb: [f32; 3]) -> Oklab {
    if palette_rgb_space() {
        return Oklab {
            l: rgb[0],
            a: rgb[1],
            b: rgb[2],
        };
    }
    let r = srgb_to_linear(rgb[0]);
    let g = srgb_to_linear(rgb[1]);
    let b = srgb_to_linear(rgb[2]);

    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    Oklab {
        l: 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        a: 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        b: 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    }
}

/// OKLab (Ottosson) to sRGB in `[0, 1]`, clamped.
pub fn oklab_to_rgb(c: Oklab) -> [f32; 3] {
    if palette_rgb_space() {
        return [c.l, c.a, c.b];
    }
    let l_ = c.l + 0.396_337_78 * c.a + 0.215_803_76 * c.b;
    let m_ = c.l - 0.105_561_346 * c.a - 0.063_854_17 * c.b;
    let s_ = c.l - 0.089_484_18 * c.a - 1.291_485_5 * c.b;

    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    [
        linear_to_srgb(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s).clamp(0.0, 1.0),
        linear_to_srgb(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s).clamp(0.0, 1.0),
        linear_to_srgb(-0.004_196_086 * l - 0.703_418_6 * m + 1.707_614_7 * s).clamp(0.0, 1.0),
    ]
}

/// Formats an sRGB colour in `[0, 1]` as a `#rrggbb` hex string.
pub fn to_hex(rgb: [f32; 3]) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgb[0] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgb[1] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgb[2] * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

/// Threshold in OKLab below which two colours are treated as the same ink.
///
/// Roughly a small multiple of a just-noticeable difference: tight enough to keep
/// distinguishable colours apart, loose enough that JPEG ringing and dithering inside one
/// flat region do not fracture it.
///
/// Measured, 2026-09-05, on the 980-icon devset. It was 0.055, and that was merging inks
/// the artwork keeps apart. The evidence is an error budget rather than a sweep: for five
/// families of seven, every bit of the colour error already sits on boundary pixels and the
/// interiors are exact, and the exception is a small set of regions -- 1.6 % of noto-emoji's
/// interior pixels carrying half its interior error -- that the model paints flat where the
/// truth varies. Fitting those regions three ways showed the variation is not shading: the
/// best linear ramp removes 13 % of that error and the best *two flat colours* remove 66 %.
/// They are two inks merged into one, which is this constant's job to prevent.
///
/// Sweeping it on the full set then confirmed the diagnosis and located the value:
///
///   0.055 -> objective 0.4960 (as shipped)   0.040 -> 0.4931
///   0.035 -> objective 0.4922 (best)         0.030 -> 0.4941
///
/// At 0.035 the full set improves on all three axes at once -- dE00 0.2005 -> 0.1991,
/// DISTS 0.0296 -> 0.0293, parameters against the artist 1.46 -> 1.44 -- which is why the
/// value moved. Do not tune this on the screen split alone: it reads 0.030 as the optimum
/// there, and held-out set A prefers the old value outright. Only the full set separates
/// them.
pub const DEFAULT_MERGE_DISTANCE: f32 = 0.035;

/// Minimum share of the image a colour must occupy to count as ink.
///
/// Anti-aliased pixels are individually rare: a 128px circle has a few hundred boundary
/// pixels spread across the whole ramp, so no single blend colour accumulates much
/// weight, while each flat region accumulates thousands. This alone removes most spurious
/// entries; `is_blend` removes the rest.
pub const MIN_INK_WEIGHT: f32 = 0.004;

/// A candidate this close to the chord between two accepted inks is a blend whatever
/// *shape* it makes on the page.
///
/// [`BLEND_INTERIOR_FRACTION`] asks whether a colour is a thin band or covers area, and
/// its premise is stated in its own comment: "anti-aliasing is a band one pixel wide lying
/// along a boundary". That is true of a native render and false of anything upscaled. A x4
/// intake spreads the same boundary over four pixels, so the ramp has an interior, covers
/// area, and is kept -- and the shape test vetoes colour-space evidence that is not close
/// to ambiguous. Measured on a real brand mark upscaled x4: seven invented tones sitting
/// 0.0000 to 0.0037 from a chord, several kept because their interior read 0.253 or their
/// straddle 0.458, against real inks 33 to 122 8-bit units clear of any chord.
///
/// A distance of zero in a three-dimensional colour space is not a coincidence; it is the
/// definition of a mixture. So a conclusive chord lifts the *thickness* gate. It does not
/// lift the straddle test, which is the semantic one and stays required: a real ink that
/// happens to be a mixture of two others -- a designer may legitimately pick a 50 % tint --
/// occupies its own region and does not lie spatially *between* them, so it does not
/// straddle and is kept.
///
/// The value is an order of magnitude below the merge tolerance it is measured against
/// (`merge_distance * 1.6`, 0.056 at the default). Across thirty clean corpus icons every
/// one of 1284 blend candidates reads exactly 0.0000 and every one is already dropped, so
/// on native intake this changes nothing by construction.
#[allow(dead_code)]
pub const CONCLUSIVE_CHORD: f32 = 0.006;

/// Below this share of its own pixels being *interior*, a colour that tests as a blend is
/// anti-aliasing rather than ink.
///
/// Abundance (above) was the original discriminator and it is the wrong one, which is why
/// no single value ever satisfied both corpora — swept across them, real content wanted
/// 0.015 and synthetic wanted 0.030, and the two disagreed because they were being asked
/// the wrong question. What actually separates an anti-aliased ramp from a pale ink is
/// not how much of the image it covers but *what shape it is*. Anti-aliasing is a band
/// one pixel wide lying along a boundary. An ink covers area.
///
/// Erosion measures exactly that and nothing else: a pixel is interior when its four
/// orthogonal neighbours share its colour, so a one-pixel band has no interior at all
/// while a solid region is almost entirely interior. A three-pixel ring keeps about a
/// third, which is the case this threshold has to sit below — the ten-concentric-rings
/// image whose five pale hues were wrongly discarded had rings several pixels wide.
pub const BLEND_INTERIOR_FRACTION: f32 = 0.25;

/// A blend-coloured candidate thinner than the interior test can see is still ink when
/// it does not *straddle* the two inks it blends.
///
/// Erosion cannot tell a two-pixel band from anti-aliasing: neither has an interior. But
/// anti-aliasing is a ramp — every pixel of it has, within one step, a neighbour nearer
/// ink A and a neighbour nearer ink B, because that is what a coverage transition is —
/// while a band of ink two pixels wide has pixels touching A and pixels touching B and
/// none touching both. The Vulcan salute's shadow strips, 2–3 px of a brown that is a
/// mix of the palm and the outline, had interior 0.21 and were discarded as coverage;
/// the palm then grew a radial gradient to explain them. A candidate is discarded as
/// coverage only when at least this fraction of its pixels straddle.
pub const BLEND_STRADDLE_FRACTION: f32 = 0.5;
/// How much further along the A–B axis a neighbour must sit, as a fraction of the axis,
/// to count as being on the far side. Above quantisation noise for a pair of inks that
/// differ by more than a few levels.
pub(crate) const STRADDLE_STEP: f32 = 0.12;

/// The fewest strided pixels one rayon task takes in the palette's per-candidate passes.
/// Those passes run hundreds of times per palette over a few thousand samples each, and
/// splitting them finer than this spends more on scheduling than on the pixels. The
/// reductions are integer counts and order-keeping collects, so the split does not
/// change a result.
pub(crate) const PAR_MIN_LEN: usize = 8192;

/// What fraction of the pixels `c` would claim sit between a pixel nearer ink `a` and a
/// pixel nearer ink `b`? See [`BLEND_STRADDLE_FRACTION`].
///
/// Position along the axis is measured in the space the blend was accepted in, by
/// projecting every pixel onto the A–B segment.
#[allow(clippy::too_many_arguments)]
fn straddle_fraction(
    lab: &[Oklab],
    px_srgb: &[[f32; 3]],
    px_lin: &[[f32; 3]],
    width: usize,
    height: usize,
    c: Oklab,
    nearest: &[f32],
    a: Oklab,
    b: Oklab,
    linear: bool,
    stride_px: usize,
) -> f32 {
    if width == 0 || height == 0 || lab.len() < width * height {
        return 0.0;
    }
    let f = |x: Oklab| -> [f32; 3] {
        let r = oklab_to_rgb(x);
        if linear {
            [
                srgb_to_linear(r[0]),
                srgb_to_linear(r[1]),
                srgb_to_linear(r[2]),
            ]
        } else {
            r
        }
    };
    let (pa, pb) = (f(a), f(b));
    let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
    let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if dd < 1e-9 {
        return 0.0;
    }
    let t_of = |x: Oklab| -> f32 {
        let p = f(x);
        ((p[0] - pa[0]) * d[0] + (p[1] - pa[1]) * d[1] + (p[2] - pa[2]) * d[2]) / dd
    };
    let tc = t_of(c);
    // A candidate near one end of the axis has that ink within less than a full step:
    // the far side is judged by the step, the near side by half the room that is left.
    let step_lo = STRADDLE_STEP.min(0.5 * tc).max(0.02);
    let step_hi = STRADDLE_STEP.min(0.5 * (1.0 - tc)).max(0.02);
    // Each pixel's position along the axis, from its *cached* colour rather than
    // by converting it here. `t_of` costs an oklab_to_rgb and, in linear space,
    // three powf calls, and it was being paid per pixel per candidate per pair --
    // 200 million cube roots on a 512 px input, which is where the palette stage's
    // 36 seconds went. The pixel's own colour does not depend on the pair, so it
    // is converted once for the whole image in `extract_palette_mdl`.
    //
    // Territory and axis position are both evaluated only where the reduction reads
    // them -- the sampled pixels inside the candidate's territory and their neighbours
    // -- rather than as two whole-image arrays built per call.
    let px: &[[f32; 3]] = if linear { px_lin } else { px_srgb };
    let t = |j: usize| {
        let p = px[j];
        ((p[0] - pa[0]) * d[0] + (p[1] - pa[1]) * d[1] + (p[2] - pa[2]) * d[2]) / dd
    };
    let (total, straddle) = (0..width * height)
        .into_par_iter()
        .step_by(stride_px)
        .with_min_len(PAR_MIN_LEN)
        .filter(|&i| lab[i].dist(c) < nearest[i])
        .map(|i| {
            let (x, y) = (i % width, i / width);
            let (mut lower, mut higher) = (false, false);
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                        continue;
                    }
                    let tn = t(ny as usize * width + nx as usize);
                    lower |= tn < tc - step_lo;
                    higher |= tn > tc + step_hi;
                }
            }
            (1u32, (lower && higher) as u32)
        })
        .reduce(|| (0u32, 0u32), |a, b| (a.0 + b.0, a.1 + b.1));
    if total == 0 {
        // Nothing of its own to protect: let the interior test's verdict stand.
        return 1.0;
    }
    straddle as f32 / total as f32
}

/// Numbers needed to state one ink.
pub const PARAMS_PER_INK: f64 = 3.0;

// ---------------------------------------------------------------------------------------
// Perceptual floor
// ---------------------------------------------------------------------------------------

/// Below this CIEDE2000 distance two candidate inks are one ink, whatever the pixel
/// count says.
///
/// The merge distance is a fixed radius in OKLab, and OKLab's lightness is a cube root:
/// the first sRGB level above black spans 0.067 of it, thirty times the step at mid-grey.
/// So on a clean render of a one-ink black logo the palette accepted #020202, #040404
/// and #070707 as three more inks -- 0.078, 0.028 and 0.049 from black in OKLab, over
/// twice the merge distance -- and the shape tests could not catch them because a
/// candidate that close to an endpoint has nothing beyond it to straddle. In CIEDE2000,
/// which is what the bench scores with, those three sit at 0.31, 0.63 and 1.11 from
/// black: differences no viewer can see. A description-length argument cannot rescue an
/// ink nobody can distinguish, so this floor is applied before the MDL escape, not
/// inside it. Swept on the screen set against the shipped palette: 1.0 gave 0.4145 ->
/// 0.4140 (6 icons better, 5 worse) and left #070707 standing at 1.11; 1.5 gave
/// 0.4140 -> 0.4124 (10 better, 6 worse, noto-emoji -0.005 dE00) and abra_agency
/// comes back as the two inks it is. Mid-grey pairs 4 levels apart read 1.5, and a
/// pair that close is not something the artwork is saying.
pub const SAME_INK_DE00: f32 = 1.5;
use rayon::prelude::*;

pub(crate) fn srgb_to_lab(rgb: [f32; 3]) -> [f32; 3] {
    let (r, g, b) = (
        srgb_to_linear(rgb[0]) as f64,
        srgb_to_linear(rgb[1]) as f64,
        srgb_to_linear(rgb[2]) as f64,
    );
    let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
    let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    let z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883;
    let f = |t: f64| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [
        (116.0 * fy - 16.0) as f32,
        (500.0 * (fx - fy)) as f32,
        (200.0 * (fy - fz)) as f32,
    ]
}

/// CIEDE2000 between two sRGB colours. Sharma, Wu and Dalal (2005) formulation.
pub fn de00(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (l1, a1, b1) = {
        let v = srgb_to_lab(a);
        (v[0] as f64, v[1] as f64, v[2] as f64)
    };
    let (l2, a2, b2) = {
        let v = srgb_to_lab(b);
        (v[0] as f64, v[1] as f64, v[2] as f64)
    };
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cbar = 0.5 * (c1 + c2);
    let p7 = cbar.powi(7);
    let g = 0.5 * (1.0 - (p7 / (p7 + 25f64.powi(7))).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = a1p.hypot(b1);
    let c2p = a2p.hypot(b2);
    let hue = |ap: f64, bp: f64| -> f64 {
        if ap == 0.0 && bp == 0.0 {
            0.0
        } else {
            let h = bp.atan2(ap).to_degrees();
            if h < 0.0 {
                h + 360.0
            } else {
                h
            }
        }
    };
    let h1p = hue(a1p, b1);
    let h2p = hue(a2p, b2);
    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else {
        let mut d = h2p - h1p;
        if d > 180.0 {
            d -= 360.0;
        } else if d < -180.0 {
            d += 360.0;
        }
        d
    };
    let dhp_big = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();
    let lbp = 0.5 * (l1 + l2);
    let cbp = 0.5 * (c1p + c2p);
    let hbp = if c1p * c2p == 0.0 {
        h1p + h2p
    } else {
        let sum = h1p + h2p;
        if (h1p - h2p).abs() <= 180.0 {
            0.5 * sum
        } else if sum < 360.0 {
            0.5 * (sum + 360.0)
        } else {
            0.5 * (sum - 360.0)
        }
    };
    let t = 1.0 - 0.17 * (hbp - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hbp).to_radians().cos()
        + 0.32 * (3.0 * hbp + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hbp - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-((hbp - 275.0) / 25.0).powi(2)).exp();
    let cb7 = cbp.powi(7);
    let rc = 2.0 * (cb7 / (cb7 + 25f64.powi(7))).sqrt();
    let l50 = (lbp - 50.0).powi(2);
    let sl = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let sc = 1.0 + 0.045 * cbp;
    let sh = 1.0 + 0.015 * cbp * t;
    let rt = -(2.0 * dtheta).to_radians().sin() * rc;
    let (vl, vc, vh) = (dlp / sl, dcp / sc, dhp_big / sh);
    (vl * vl + vc * vc + vh * vh + rt * vc * vh).max(0.0).sqrt() as f32
}

/// An intake whose edges are wider than this many pixels has been resampled, blurred
/// or upscaled, and its anti-aliasing ramps carry colours that are not inks.
///
/// Measured over all 980 corpus rasters (native 8x-supersampled renders): median 1.00,
/// the widest 1.50 (a noto-emoji face with soft shading), then 1.43 (a synthetic radial
/// gradient) and 1.26. A 2x Lanczos round trip reads 1.36-1.40, a 4x one 2.80, 8x
/// reads 4.00. So this sits above everything native and catches upscales of about 3x
/// and more; a 2x resample is indistinguishable from soft artwork by edge width alone
/// and is what the SR pre-pass (`--sr auto`) exists for.
pub const SOFT_INTAKE_EDGE: f64 = 1.75;

/// The noise guard's strength on a soft intake (see [`NOISE_SIGMAS`], which is the
/// clean-intake value and stays zero).
///
/// It has to stay gated. Run unconditionally it costs **10.9 %** on the 246-icon screen
/// set -- objective 0.4005 -> 0.4442, measured 2026-09-08 -- because on a clean intake two
/// colours a whisker apart really are two inks and merging them throws away artwork. So
/// this is only ever switched on by positive evidence that the intake is not clean: a wide
/// edge (`SOFT_INTAKE_EDGE`), or a lossy container (`crate::lossy_container`).
pub const SOFT_NOISE_SIGMAS: f32 = 3.0;

/// Ringing above this counts as positive evidence that the intake is compressed.
///
/// [`crate::coverage::ringing_score`] is zero on clean vector art by construction and rises
/// with compression. The threshold is set from the FALSE-POSITIVE side, because the guard it
/// opens costs 2.8% on the screen set when it fires and the corpus is overwhelmingly clean.
/// Measured over 240 clean corpus rasters at tier 128ss: median 0.0000, p90 0.0062, p99
/// 0.0333, max 0.1023 -- and that maximum is `synthetic/rings_concentric`, a test pattern
/// that genuinely oscillates and arguably should trip it. The highest real artwork is a
/// noto-emoji at 0.0370. So 0.12 sits above everything clean the corpus contains, which is
/// what "only ever switched on by positive evidence" has to mean in practice.
///
/// It is deliberately the *third* way into the soft-intake branch rather than a replacement
/// for either existing one. A wide edge and a lossy container each remain sufficient on their
/// own; this only adds the case both of them miss, which is the common one: a JPEG re-saved
/// as PNG, where the container lies and the edges are still one pixel wide.
pub const SOFT_RINGING: f64 = 0.12;

/// [`SOFT_RINGING`] for an image large enough for the measurement to isolate one boundary.
///
/// The ring band is fixed in pixels (3 to 7 px out) because ringing is: it comes from an 8x8
/// DCT block and does not scale with the picture. That band only isolates a single boundary
/// when features are bigger than it. On the 128 px benchmark tier a glyph stroke is about ten
/// pixels wide, so the ring of one edge lands on the next edge, clean artwork scores up to
/// 0.1023, and the threshold has to sit above that. At 512 px the same artwork scores at most
/// 0.0430 over 64 images, with the compressed versions of those same images at 0.086 median.
///
/// So the gate is not one number, and the reason is geometry rather than tuning. Measured at
/// 512 px, a 0.05 gate has ZERO false positives and catches 88-89% of JPEG at qualities 85,
/// 60 and 40; the conservative gate catches 19-33% of the same files. Below
/// [`RINGING_MIN_DIM`] the conservative number stands and the screen set is bit-identical.
pub const SOFT_RINGING_LARGE: f64 = 0.05;

/// The smallest dimension at which [`SOFT_RINGING_LARGE`] applies. See it for why.
pub const RINGING_MIN_DIM: usize = 256;

/// How much of the measured residual noise to believe.
///
/// `regularize::residual_sigma` measures how far interior pixels sit from their own ink, which
/// includes palette bias as well as compression damage, so taking it at face value overstates
/// the noise a little. Everything downstream divides by sigma, so overstating it buys fewer
/// parameters at the cost of colour. Measured on 78 JPEG-re-encoded-as-PNG traces against the
/// artist's clean render, the endpoints of that trade are: at 1.0 the parameter count falls
/// 31.5% against what ships today and colour error 15.9%; the detector alone (sigma left at
/// the floor) gives 22.0% and 22.2%. The value here is the swept optimum between them.
pub const MEASURED_SIGMA_SCALE: f64 = 1.0;

/// Ceiling on the measured noise, in display levels. See [`MEASURED_SIGMA_SCALE`].
///
/// `regularize::residual_sigma` clamps itself to 8 levels; this is a second ceiling on top,
/// and it exists because an early measurement said diagrams broke above 2.
///
/// **That measurement was wrong, twice, and the way it was wrong is the lesson.** On 22
/// diagrams the high ceiling appeared to double their colour error (+114%) with a sharp cliff
/// between 2 and 3 levels. On 106 diagrams the same setting read +0.7%. On 791 traces across
/// all classes it reads +6% on diagrams while every other class improves, and the cliff does
/// not exist:
///
/// | ceiling | colour | parameters | diagrams colour | diagrams parameters |
/// |---|---|---|---|---|
/// | 2 | -15.6% | -20.3% | -6% | -30% |
/// | 3 | -15.4% | -28.3% | +8% | -36% |
/// | 6 | -16.9% | -32.7% | +6% | -48% |
/// | 8 | **-16.9%** | **-33.7%** | +6% | -52% |
///
/// So the ceiling is 8, which is to say it no longer binds and `residual_sigma`'s own clamp
/// governs. Diagrams pay 6% colour for 52% fewer parameters; every other class is better on
/// both axes, brand logos by 36% and 52%.
///
/// Three separate small samples pointed the wrong way on this one constant, each time with an
/// apparently clean story attached. Nothing about this trade should be decided on fewer than
/// several hundred paired traces, and it must be broken down by source, because the corpus
/// median hid a real regression once and invented a false one twice.
pub const MEASURED_SIGMA_CAP: f64 = 8.0;

/// [`SAME_INK_DE00`] for a soft intake, for the same reason the noise guard rises there.
///
/// A native render puts one blend pixel on an edge, and two colours a whisker apart in it
/// really are two inks. An oversampled or resampled one spreads that edge over several
/// pixels, and the ramp between two inks then supplies a whole family of intermediate
/// colours that are not inks at all -- each of which the palette will otherwise mint,
/// and each of which then becomes its own face, its own boundary, and its own path.
/// Measured on a real brand mark upscaled 4x: 76 distinct fills where the drawing has
/// five, `#030303` alone emitted as 82 separate paths against a single `#000000` at 1x.
///
/// This is the same judgement `SOFT_NOISE_SIGMAS` already makes and keyed to the same
/// threshold, so it changes nothing a native intake does.
pub const SOFT_SAME_INK_DE00: f32 = 5.0;

/// What fraction of the pixels this colour would *claim* are interior to the claim?
///
/// The set has to be the pixels `c` would take from the palette as it currently stands —
/// those nearer to `c` than to anything already accepted — and not simply the pixels
/// within some radius of `c`. A ball is the obvious choice and it is wrong: an
/// anti-aliased colour sits close to one end of its ramp, so a ball around it swallows
/// the solid region as well as the band, and the band then measures as solid. Tried that
/// way, the green-circle case got worse rather than better, 29 faces to 40.
///
/// Near zero for an anti-aliased boundary band; near one for a filled region.
fn interior_fraction(
    lab: &[Oklab],
    width: usize,
    height: usize,
    c: Oklab,
    nearest: &[f32],
    stride_px: usize,
) -> f32 {
    if width == 0 || height == 0 || lab.len() < width * height {
        // Without the geometry there is nothing to measure; claim solidity so the caller
        // falls back on its other evidence rather than discarding the colour.
        return 1.0;
    }
    // Evaluated only where the reduction reads it: the sampled pixels and their four
    // neighbours, not as a whole-image array built per call.
    let mask = |i: usize| lab[i].dist(c) < nearest[i];
    let (total, interior) = (0..width * height)
        .into_par_iter()
        .step_by(stride_px)
        .with_min_len(PAR_MIN_LEN)
        .filter(|&i| mask(i))
        .map(|i| {
            let (x, y) = (i % width, i / width);
            // A border pixel has no neighbour outside the image to disqualify it; treat
            // the outside as matching, so a region touching the edge is not penalised.
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

/// How far inside a chord, as a fraction of it, a blend must lie (was `INKVEC_BLEND_TMIN`;
/// see `docs/algorithm/constants.md`).
pub(crate) const BLEND_TMIN: f32 = 0.04;

/// Is `c` explained as a mixture of two colours already accepted?
///
/// This is the principled test, and the one that matters. An anti-aliased pixel is *by
/// definition* a coverage-weighted blend of the two inks it sits between, so it lies on
/// the segment joining them. A colour that lands near the middle of such a segment is
/// not a new ink no matter how often it occurs — it is evidence about geometry, which is
/// what S2 will use it for.
///
/// Compositing is linear in *linear* light, so the test is done there rather than in
/// OKLab (whose cube root would bend a straight blend line) or in sRGB. Some pipelines do
/// composite in sRGB anyway, so a blend is accepted in either space.
///
/// Returns every pair of inks `c` could be a blend of, each with whether the blend was
/// found in linear light. A pale pink is within tolerance of the white–grey axis as well
/// as the white–red one, and only the pair its pixels actually lie between can say
/// whether it straddles them; the caller tries them all.
///
/// `tmin` is [`BLEND_TMIN`].
fn blend_pairs(
    c: Oklab,
    accepted: &[Oklab],
    tol: f32,
    tmin: f32,
) -> Vec<(usize, usize, bool, f32)> {
    let mut out: Vec<(usize, usize, bool, f32)> = Vec::new();
    if accepted.len() < 2 {
        return out;
    }
    let to_lin = |x: Oklab| {
        let r = oklab_to_rgb(x);
        [
            srgb_to_linear(r[0]),
            srgb_to_linear(r[1]),
            srgb_to_linear(r[2]),
        ]
    };
    let srgb = |x: Oklab| oklab_to_rgb(x);

    for space in 0..2 {
        let f = |x: Oklab| if space == 0 { to_lin(x) } else { srgb(x) };
        let p = f(c);
        for i in 0..accepted.len() {
            for j in i + 1..accepted.len() {
                let (a, b) = (f(accepted[i]), f(accepted[j]));
                let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                if dd < 1e-9 {
                    continue;
                }
                let t = ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1] + (p[2] - a[2]) * d[2]) / dd;
                // Only interior mixtures count; t outside (0, 1) is a different colour,
                // not a blend of these two.
                if !(tmin..=1.0 - tmin).contains(&t) {
                    continue;
                }
                let q = [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t];
                let e = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                // Compare the residual perceptually, in OKLab.
                let back = if space == 0 {
                    rgb_to_oklab([
                        linear_to_srgb(q[0].clamp(0.0, 1.0)),
                        linear_to_srgb(q[1].clamp(0.0, 1.0)),
                        linear_to_srgb(q[2].clamp(0.0, 1.0)),
                    ])
                } else {
                    rgb_to_oklab([
                        q[0].clamp(0.0, 1.0),
                        q[1].clamp(0.0, 1.0),
                        q[2].clamp(0.0, 1.0),
                    ])
                };
                let _ = e;
                let off = c.dist(back);
                if off <= tol {
                    out.push((i, j, space == 0, off));
                }
            }
        }
    }
    out
}

/// Recovered palette plus a per-pixel assignment.
pub struct Palette {
    /// Recovered entries, in OKLab.
    pub colors: Vec<Oklab>,
    /// Recovered entries, in sRGB `[0, 1]`.
    pub rgb: Vec<[f32; 3]>,
    /// Fraction of pixels assigned to each entry.
    pub weight: Vec<f32>,
    /// Opacity of each entry, 1.0 for an opaque ink.
    ///
    /// Extraction works on the image matted opaque, so everything it finds is opaque and
    /// this is all ones. [`split_alpha_inks`] fills it in when the caller knows what the
    /// source's alpha was: a panel drawn at one opacity becomes its own entry, with the
    /// same colour as the opaque ink it would otherwise have merged with.
    pub alpha: Vec<f32>,
}

impl Palette {
    /// Number of palette entries.
    pub fn len(&self) -> usize {
        self.colors.len()
    }
    /// Whether the palette has no entries.
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// Index of the nearest palette entry, with its distance.
    pub fn nearest(&self, c: Oklab) -> (usize, f32) {
        let mut best = (0usize, f32::MAX);
        for (i, &p) in self.colors.iter().enumerate() {
            let d = c.dist(p);
            if d < best.1 {
                best = (i, d);
            }
        }
        best
    }
}

/// Recover the palette by frequency-ranked mode seeking in OKLab.
///
/// Colours are bucketed coarsely only to make counting tractable; the palette entry is
/// then the *weighted mean* of the pixels that fall to it, so the recovered colour is not
/// snapped to a bucket centre. Entries are taken in descending frequency and a candidate
/// is rejected if it is within `merge_distance` of one already accepted, which is what
/// keeps an anti-aliased ramp from contributing entries of its own.
/// Smallest OKLab separation we will ever treat as two inks.
///
/// Below this a viewer cannot tell the colours apart at all, so no amount of evidence
/// makes them two inks rather than one measured twice.
pub const JND_FLOOR: f32 = 0.012;

/// How many times its own spread a candidate must stand clear of the nearest accepted ink
/// before the difference is credited to the artwork rather than to the input.
///
/// **Zero, which is off**, and that is a measured decision rather than caution. The guard
/// does what it was built for: on a logo upscaled with the packaged model's own error of
/// 1.87 levels it takes the output from 5 fills, 77 paths and 12.4 KB back to 1 fill, 3
/// paths and 1.0 KB, which is what a clean trace produces. But it is not free on a clean
/// intake -- the screen set goes from 0.4328 to 0.4451 -- because a region with a real
/// gradient has a real spread, and the guard cannot tell that from noise without knowing
/// which it is looking at.
///
/// So the caller decides, because the caller knows where its raster came from: input that
/// has been upscaled, compressed or resampled gets [`SOFT_NOISE_SIGMAS`] from the soft-intake
/// gate, and an exact-coverage render keeps this. Making it free, by detecting the noise
/// instead of being told about it, is the open problem: `coverage::estimate_noise` cannot
/// see it, because an icon is mostly empty and its median Laplacian is zero however noisy
/// the artwork is.
pub const NOISE_SIGMAS: f32 = 0.0;

/// Recover the palette with the fixed `merge_distance` threshold alone, with no noise
/// evidence to justify the split test. See [`extract_palette_mdl`] for the full form.
pub fn extract_palette(
    rgb: &[[f32; 3]],
    width: usize,
    height: usize,
    merge_distance: f32,
    max_colors: usize,
) -> Palette {
    // No noise estimate: fall back to the fixed threshold alone.
    extract_palette_mdl(
        rgb,
        width,
        height,
        merge_distance,
        max_colors,
        PaletteEvidence::default(),
    )
}

/// Palette extraction that decides how many inks there are by minimum description
/// length, rather than by a fixed distance.
///
/// A fixed threshold cannot be right everywhere in a perceptual space. Saturated inks sit
/// far apart and survive it; pale ones cluster and do not. Ten concentric rings of ten
/// distinct hues came back as six colours, because the five pale rings fell inside the
/// threshold of each other — and the five that vanished then made their regions look like
/// gradients, which is where a DISTS@4x of 0.197 came from.
///
/// The MDL test asks the same question the rest of the pipeline asks. Folding a candidate
/// into the nearest accepted ink saves its three parameters but pays a residual over
/// every one of its pixels: `0.5·n·(d/sigma)²` against `lambda·3`. A mode with thousands
/// of pixels and a separation far above the noise is therefore kept however close the
/// fixed threshold would call it, while a handful of pixels a hair away from an existing
/// ink is folded in — which is exactly the behaviour wanted from both.
///
/// `sigma_noise` is the per-channel pixel noise in sRGB units; passing `0.0` disables the
/// MDL test and leaves only the fixed threshold.
/// Mean distance from a candidate's own pixels to the candidate, in OKLab.
///
/// This is the scale at which the image itself says "these pixels are the same colour",
/// and it is measured rather than assumed, which matters because no single number can
/// stand in for it. The merge threshold is a distance in OKLab and OKLab's lightness is
/// cube-root-like, so one sRGB level near black is a far larger distance than one level
/// near white: on a logo upscaled with under two levels of error, seven separate inks were
/// accepted that were all, in fact, black -- `#000000`, `#010300`, `#000002`, `#020000`,
/// `#000100`, `#010002`, `#030100` -- each claiming tens of thousands of pixels.
///
/// A global noise estimate cannot supply this either. `coverage::estimate_noise` takes a
/// median over the whole image, and an icon is mostly empty, so more than half its pixels
/// are exactly flat and the median is zero however noisy the artwork is. Measured on that
/// same upscaled logo it returned its floor of half a level. Selecting flatter pixels
/// first does not rescue it: the empty background is flat too, and it is the majority.
///
/// The spread of the pixels a candidate actually claims has neither problem. It is local,
/// so it scales with the colour space where the colour is; it is zero on an exact-coverage
/// intake, so the shipped behaviour is unchanged; and it needs nothing to be estimated
/// globally at all.
/// Pixels visited by the palette's statistical passes.
///
/// Claim, spread, interior fraction and straddle fraction are all estimates of a
/// *fraction* of the image, and a fraction does not need every pixel to be
/// measured. Visiting a strided subset bounds the cost of one pass at any input
/// size, which is what stops trace time growing with resolution: these passes run
/// once per palette candidate, so an unbounded pass makes the stage quadratic in
/// everything at once.
///
/// The cap sits above 128 x 128 = 16384 deliberately. Every constant in this
/// module was tuned on a 128 px corpus, and at or below the cap the stride is one
/// and the arithmetic is bit-identical to visiting every pixel. Only inputs
/// larger than the corpus see any change at all, and today those do not finish.
pub const STAT_PIXELS: usize = 1 << 16;

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Stride for the statistical passes over an image of `n` pixels and `width` columns.
pub(crate) fn stat_stride(n: usize, width: usize) -> usize {
    if n <= STAT_PIXELS || width == 0 {
        return 1;
    }
    let mut s = n.div_ceil(STAT_PIXELS);
    // A stride sharing a factor with the row width lands on the same columns of
    // every row, which would sample a few vertical stripes rather than the image.
    while s > 1 && gcd(s, width) != 1 {
        s += 1;
    }
    s
}

fn claim_spread(
    lab: &[Oklab],
    nearest_px: &[f32],
    c: Oklab,
    tol: f32,
    stride_px: usize,
) -> (usize, f32) {
    const MAX_SAMPLES: usize = 8192;
    let stride = (lab.len() / MAX_SAMPLES).max(1);
    // Every core, in one ordered pass: territory count and spread sample test the same
    // distance at the same pixels. rayon's collect keeps sequential order, so `d_in` and
    // therefore its median are exactly what one thread would have produced.
    let claimed: Vec<(usize, f32)> = (0..lab.len())
        .into_par_iter()
        .step_by(stride_px)
        .with_min_len(PAR_MIN_LEN)
        .filter_map(|i| {
            let dist = lab[i].dist(c);
            (dist < nearest_px[i]).then_some((i, dist))
        })
        .collect();
    let n = claimed.len();
    // Members, not territory. Territory is whatever has no closer ink yet, which
    // for the first candidate is the whole image, and a spread measured over that
    // is the mean distance from every pixel to white -- enormous, and it rejected
    // every colour after the first. Measured: the screen set went from 0.4328 to
    // 1.2461 before this was restricted to `tol`.
    let mut d_in: Vec<f32> = claimed
        .iter()
        .filter(|&&(i, dist)| dist < tol && i % stride == 0)
        .map(|&(_, dist)| dist)
        .collect();
    // `n` is compared against absolute pixel counts downstream, so a strided
    // visit is scaled back up to estimate what a full one would have counted.
    let n = n * stride_px;
    if d_in.is_empty() {
        return (n, 0.0);
    }
    // The MEDIAN of those distances, not the mean. An anti-aliased edge puts a ramp of
    // blend pixels inside `tol` on a perfectly clean image, and a mean is pulled up by
    // them: that alone cost the screen set 2 % before this was a median. Blends are a
    // minority of any region's members, so the median ignores them and reads zero on an
    // exact-coverage intake, which is what keeps this from touching the shipped corpus.
    d_in.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (n, d_in[d_in.len() / 2])
}

/// What the palette needs to know about the *measurement*, as opposed to the pixels.
///
/// These three arrived as trailing numbers and were easy to transpose — two `f64` and an
/// `f32`, all plausible in any order, and a swap would have quietly changed how many inks
/// the image was found to have. Naming them makes that impossible.
#[derive(Clone, Copy, Debug, Default)]
pub struct PaletteEvidence {
    /// Per-channel pixel noise in sRGB units, from [`crate::coverage::estimate_noise`].
    /// Zero disables the split test that this noise level would otherwise justify.
    pub sigma_noise: f64,
    /// Nats charged per parameter — the same exchange rate every other stage prices
    /// against. An ink costs `lambda * PARAMS_PER_INK` before it explains anything.
    pub lambda: f64,
    /// How many measured standard deviations two inks must lie apart before they count
    /// as two.
    pub noise_sigmas: f32,
    /// Perceptual distance below which two candidate inks are one ink. Raised on a soft
    /// intake; see [`SOFT_SAME_INK_DE00`].
    pub same_ink_de00: f32,
}

/// Recover the palette by frequency-ranked mode seeking in OKLab, using `ev` to decide
/// whether two nearby candidates are one ink measured twice or genuinely two inks.
pub fn extract_palette_mdl(
    rgb: &[[f32; 3]],
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
    const BINS: usize = 24;

    let lab: Vec<Oklab> = rgb.par_iter().map(|&c| rgb_to_oklab(c)).collect();

    // How many pixels each statistical pass visits. These passes run once per
    // palette candidate, so leaving them unbounded makes the stage grow with
    // resolution on top of everything else: a 512 px input spent 36 s here and a
    // 1024 px one over four minutes. At or below the cap this is 1 and nothing
    // changes, which is every image in the corpus.
    let stride_px = stat_stride(lab.len(), width);

    // Each pixel's sRGB and linear-RGB value, converted once. `straddle_fraction`
    // needs them to place a pixel on a colour axis, the axis changes per candidate
    // pair while the pixel's own colour does not, and the conversion is a cube root
    // plus three powf calls. Converting from `lab` rather than reusing `rgb` keeps
    // the arithmetic bit-identical to what the per-pixel call computed.
    let px_srgb: Vec<[f32; 3]> = lab.par_iter().map(|&p| oklab_to_rgb(p)).collect();
    let px_lin: Vec<[f32; 3]> = px_srgb
        .par_iter()
        .map(|r| {
            [
                srgb_to_linear(r[0]),
                srgb_to_linear(r[1]),
                srgb_to_linear(r[2]),
            ]
        })
        .collect();

    // Bin keys on every core; the accumulation stays sequential and in pixel order so
    // the centroid sums are bit-identical to the single-threaded version. A dense table
    // over the 24^3 bins replaces the hash map, which was most of this pass's cost.
    let keys: Vec<u32> = lab
        .par_iter()
        .map(|c| {
            let li =
                ((c.l.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u32).min(BINS as u32 - 1);
            let ai = (((c.a + 0.4) / 0.8).clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u32;
            let bi = (((c.b + 0.4) / 0.8).clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u32;
            li * (BINS * BINS) as u32 + ai * BINS as u32 + bi
        })
        .collect();
    let mut dense: Vec<(u32, f64, f64, f64)> = vec![(0, 0.0, 0.0, 0.0); BINS * BINS * BINS];
    for (c, &key) in lab.iter().zip(keys.iter()) {
        let e = &mut dense[key as usize];
        e.0 += 1;
        e.1 += c.l as f64;
        e.2 += c.a as f64;
        e.3 += c.b as f64;
    }
    let counts: std::collections::HashMap<u32, (u32, f64, f64, f64)> = dense
        .into_iter()
        .enumerate()
        .filter(|(_, e)| e.0 > 0)
        .map(|(k, e)| (k as u32, e))
        .collect();

    // Carry the bin key through, purely so ties can be broken by it.
    //
    // Sorting modes by pixel count alone leaves equal-frequency colours ordered by
    // whatever order the hash map yielded them in, which differs between runs of the
    // same binary on the same input. Palette order decides which colour is accepted
    // first and therefore what everything downstream sees, so the whole tracer was
    // non-deterministic: the junction accuracy test measured 0.054-0.134px across ten
    // consecutive runs of one binary. An identical input must give an identical file.
    let mut modes: Vec<(u32, u32, Oklab)> = counts
        .into_iter()
        .map(|(key, (n, sl, sa, sb))| {
            let f = n as f64;
            (
                n,
                key,
                Oklab {
                    l: (sl / f) as f32,
                    a: (sa / f) as f32,
                    b: (sb / f) as f32,
                },
            )
        })
        .collect();
    modes.sort_by_key(|&(n, key, _)| (std::cmp::Reverse(n), key));

    let total_px = lab.len().max(1) as f32;
    let mut colors: Vec<Oklab> = Vec::new();
    // Distance from each pixel to the nearest ink accepted so far, so a candidate's own
    // territory can be read off without rescanning the whole palette.
    let mut nearest_px: Vec<f32> = vec![f32::INFINITY; lab.len()];
    let paldbg = inkvec_core::env::flag("INKVEC_PALDBG");
    // The perceptual-merge experiment below, in a `research` build only.
    let de00_radius: Option<f32> = if cfg!(feature = "research") {
        inkvec_core::env::number("INKVEC_MERGE_DE00").map(|v| v as f32)
    } else {
        None
    };
    let blend_tmin = BLEND_TMIN;
    for (n, _key, c) in &modes {
        if colors.len() >= max_colors {
            break;
        }
        // How many pixels would this candidate actually take? Not `n`, which counts one
        // bin of a 24-cubed grid in OKLab.
        //
        // The difference is not cosmetic. A colour whose pixels straddle a bin boundary is
        // split across several bins and every one of them is counted small, so both tests
        // below -- the rarity floor and the description-length escape -- see a fraction of
        // the evidence that exists and refuse an ink the image plainly contains. That is
        // measurable downstream: the regions the tracer paints flat where the artwork
        // varies are two inks merged into one, and fitting them showed the best pair of
        // flat colours removing 66 % of the error there against 13 % for the best linear
        // ramp. Counting the territory
        // costs one pass over the image per candidate and answers the question asked.
        let (claim, spread) = claim_spread(&lab, &nearest_px, *c, merge_distance, stride_px);
        if (claim as f32 / total_px) < MIN_INK_WEIGHT && !colors.is_empty() {
            continue;
        }
        let nearest = colors
            .iter()
            .map(|&p| p.dist(*c))
            .fold(f32::INFINITY, f32::min);
        // Two inks are two inks only if they are further apart than the noise, and the
        // noise has to be measured where they are. On a clean intake `sigma_noise` sits at
        // its floor of half a level and this term is far below `merge_distance`, so
        // nothing changes; on a degraded or upscaled input it grows, and it grows most
        // where the colour space is most stretched, which is where the spurious inks were.
        // Apart by more than their own spread, or they are one ink measured twice.
        let reach = noise_sigmas * spread;
        // Two inks nobody can tell apart are one ink. Decided perceptually, before any
        // description-length argument, because the argument counts pixels and pixels
        // are exactly what an anti-aliasing ramp near an ink has plenty of.
        if let Some(&near_ink) = colors.iter().min_by(|&&p, &&q| {
            p.dist(*c)
                .partial_cmp(&q.dist(*c))
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            let perceptual = de00(oklab_to_rgb(*c), oklab_to_rgb(near_ink));
            if perceptual < same_ink_de00 {
                if paldbg {
                    eprintln!(
                        "  cand {:<9} same ink as {} (dE00 {:.2} < {})",
                        to_hex(oklab_to_rgb(*c)),
                        to_hex(oklab_to_rgb(near_ink)),
                        perceptual,
                        same_ink_de00
                    );
                }
                continue;
            }
        }
        // EXPERIMENT (`INKVEC_MERGE_DE00=<radius>`, research builds only): decide the merge PERCEPTUALLY rather
        // than by Euclidean distance in OKLab.
        //
        // The motivation is the failure documented on `SAME_INK_DE00`: OKLab's lightness is
        // a cube root, so the first sRGB level above black spans thirty times the step at
        // mid-grey, and a one-ink black logo minted #020202, #040404 and #070707 as three
        // more inks at 0.078, 0.028 and 0.049 -- over twice `merge_distance`. In CIEDE2000
        // those sit at 0.31, 0.63 and 1.11, differences no viewer can see. The shipped fix
        // is a dE00 FLOOR applied after the fact; this asks whether using dE00 as the
        // distance itself removes the problem at its root instead of patching it.
        //
        // Measured at 512 px, so a reader can check whether it was worth it:
        // switching the palette to plain sRGB instead (`INKVEC_PALETTE_RGB`, research) cost +15%
        // parameters and +1% colour on JPEG q40 and changed nothing on clean input, which
        // is why the space is not the lever and the metric might be.
        let merged = if let Some(rad) = de00_radius {
            let d = colors
                .iter()
                .map(|&p| de00(oklab_to_rgb(*c), oklab_to_rgb(p)))
                .fold(f32::INFINITY, f32::min);
            // `reach` stays in OKLab units and still applies, so the noise guard behaves
            // exactly as before on a degraded intake.
            d <= rad || nearest <= reach
        } else {
            nearest <= merge_distance.max(reach)
        };
        if merged {
            // Inside the fixed threshold. Keep it anyway if the evidence is overwhelming:
            // enough pixels, separated far enough above the noise, that explaining them
            // with the nearest ink would cost more residual than a new ink costs to state.
            let worth_it = sigma_noise > 0.0
                && nearest > JND_FLOOR
                && nearest > reach
                && 0.5 * (claim as f64) * ((nearest as f64 / sigma_noise).powi(2))
                    > lambda * PARAMS_PER_INK;
            if !worth_it {
                continue;
            }
        }
        let _ = n;
        // Explained as a blend of inks already accepted: coverage evidence, not a new
        // colour — unless it covers too much of the image to be a boundary effect.
        // Explained as a blend of inks already accepted *and* shaped like a boundary
        // band rather than a region: coverage evidence, not a new colour.
        let pairs = blend_pairs(*c, &colors, merge_distance * 1.6, blend_tmin);
        let blend = !pairs.is_empty();
        // How far the candidate sits from the nearest chord between two accepted inks.
        // Zero means it lies exactly on the line between them, which in a three-dimensional
        // colour space is not a coincidence -- it is what a blend *is*.
        let chord_off = pairs
            .iter()
            .map(|&(_, _, _, off)| off)
            .fold(f32::INFINITY, f32::min);
        let interior = if blend {
            interior_fraction(&lab, width, height, *c, &nearest_px, stride_px)
        } else {
            1.0
        };
        // The pairs on every core: each is a count ratio and a max of finite values does
        // not depend on the order it is taken in.
        let straddle = if blend && interior < BLEND_INTERIOR_FRACTION {
            pairs
                .par_iter()
                .map(|&(i, j, linear, _off)| {
                    straddle_fraction(
                        &lab,
                        &px_srgb,
                        &px_lin,
                        width,
                        height,
                        *c,
                        &nearest_px,
                        colors[i],
                        colors[j],
                        linear,
                        stride_px,
                    )
                })
                .reduce(|| 0.0f32, f32::max)
        } else {
            0.0
        };
        // Why every candidate was kept or dropped. A wrong palette does not look like a
        // palette bug downstream — the green-circle case surfaced as a spurious radial
        // gradient and twenty-seven junk paths — so the decision has to be readable
        // directly. `INKVEC_PALDBG=1`.
        if paldbg {
            eprintln!(
                "  cand {:<9} bin={:<6} claim={:<6} w={:.4} sig={:.5} reach={:.4} near={:.4} blend={} chord={:.4} interior={:.3} straddle={:.3}",
                to_hex(oklab_to_rgb(*c)),
                n,
                claim,
                claim as f32 / total_px,
                sigma_noise,
                reach,
                nearest,
                blend,
                if blend { chord_off } else { f32::NAN },
                interior,
                straddle
            );
        }
        // Both spatial tests are required, and the colour-space distance does not override
        // them. **Letting a conclusive chord decide alone was tried on 2026-09-09 and is a
        // 20 % regression** -- screen-set objective 0.4005 -> 0.4826 at 128, and worse on
        // every tier, losing on colour error and parameter count at once.
        //
        // The reason is worth keeping, because the idea is seductive and correct in theory:
        // a colour lying exactly on the chord between two inks *is* a mixture of them, and
        // in a three-dimensional space that is not a coincidence. But a designer may also
        // simply choose that colour, and then it is an ink that happens to be a mixture --
        // and there are far more of those in real artwork than the geometry suggests. The
        // interior test is what protects them: a chosen tint covers area, an anti-aliased
        // ramp does not.
        //
        // The measurement that appeared to clear this was wrong, and the mistake is easy to
        // repeat: it checked that the chord rule agreed with the existing rule on every
        // candidate the existing rule *dropped* (1284 of 1284 across thirty icons) and never
        // asked what the chord rule would newly drop. Agreement on the accepted set says
        // nothing about the rejected set.
        if blend && interior < BLEND_INTERIOR_FRACTION && straddle >= BLEND_STRADDLE_FRACTION {
            continue;
        }
        nearest_px
            .par_iter_mut()
            .zip(lab.par_iter())
            .for_each(|(d, &q)| *d = d.min(q.dist(*c)));
        colors.push(*c);
    }
    if colors.is_empty() {
        colors.push(modes.first().map(|m| m.2).unwrap_or(Oklab {
            l: 1.0,
            a: 0.0,
            b: 0.0,
        }));
    }

    // Refine each entry to the mean of the pixels that actually chose it. Anti-aliased
    // pixels sit far from every entry, so excluding them keeps blends from dragging a
    // palette colour off its true value.
    let mut acc = vec![(0.0f64, 0.0f64, 0.0f64, 0u32); colors.len()];
    // The nearest-entry search is the cost and runs on every core; the sums are
    // taken in pixel order afterwards so the means are bit-identical.
    let chosen: Vec<u32> = lab
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
    for (c, &k) in lab.iter().zip(chosen.iter()) {
        if k != u32::MAX {
            let e = &mut acc[k as usize];
            e.0 += c.l as f64;
            e.1 += c.a as f64;
            e.2 += c.b as f64;
            e.3 += 1;
        }
    }
    let total = total_px;
    let mut weight = Vec::with_capacity(colors.len());
    for (i, e) in acc.iter().enumerate() {
        if e.3 > 0 {
            let f = e.3 as f64;
            colors[i] = Oklab {
                l: (e.0 / f) as f32,
                a: (e.1 / f) as f32,
                b: (e.2 / f) as f32,
            };
        }
        weight.push(e.3 as f32 / total);
    }

    let rgb_out: Vec<[f32; 3]> = colors.iter().map(|&c| oklab_to_rgb(c)).collect();
    let alpha = vec![1.0; colors.len()];
    Palette {
        colors,
        rgb: rgb_out,
        weight,
        alpha,
    }
}

/// Split each ink by the opacity the source drew it at.
///
/// The pipeline traces an image matted opaque, because unmixing a boundary needs two
/// opaque colours. That loses a distinction the source made: a panel at 25% white over
/// nothing and the transparent ground itself both composite to white, so they label as one
/// ink and the panel disappears into the background. This puts it back — after labelling,
/// where it costs one pass and no change to how the palette was found.
///
/// Only *flat* opacity is separated. A face whose alpha varies across it is a glow, no
/// single opacity describes it, and splitting it would mint a band per level; the test is
/// therefore a two-mode one — the label's alphas must fall into groups that are each tight
/// and clearly apart — and anything else is left alone.
///
/// Returns the number of new inks minted.
pub fn split_alpha_inks(labels: &mut [u16], pal: &mut Palette, alpha: &[f32]) -> usize {
    /// Opacity below which the source drew nothing at all.
    const CLEAR: f32 = 0.05;
    /// How far apart two levels must be to count as two, and how tight each must be.
    const LEVEL_GAP: f32 = 0.15;
    const LEVEL_SPREAD: f32 = 0.06;
    /// A level worth an ink of its own.
    const MIN_SHARE: f32 = 0.02;

    if labels.len() != alpha.len() || pal.is_empty() {
        return 0;
    }
    let n_inks = pal.len();
    let mut by_ink: Vec<Vec<f32>> = vec![Vec::new(); n_inks];
    for (p, &l) in labels.iter().enumerate() {
        if (l as usize) < n_inks {
            by_ink[l as usize].push(alpha[p]);
        }
    }

    // For each ink, the opacity levels its pixels were drawn at.
    let mut levels: Vec<Vec<f32>> = vec![Vec::new(); n_inks];
    for (ink, alphas) in by_ink.iter().enumerate() {
        if alphas.len() < 16 {
            continue;
        }
        let mut sorted = alphas.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        // Cut wherever consecutive values jump by more than the gap; that is a histogram
        // split without a histogram, and it is exact on flat art.
        let mut groups: Vec<(f32, f32, usize)> = Vec::new(); // (sum, count as f32, count)
        let mut start = 0usize;
        for k in 1..=sorted.len() {
            let cut = k == sorted.len() || sorted[k] - sorted[k - 1] > LEVEL_GAP;
            if !cut {
                continue;
            }
            let slice = &sorted[start..k];
            let spread = slice[slice.len() - 1] - slice[0];
            let mean = slice.iter().sum::<f32>() / slice.len() as f32;
            groups.push((mean, spread, slice.len()));
            start = k;
        }
        let total = sorted.len() as f32;
        // The transparent group counts as a level of its own: an ink whose pixels are
        // partly "nothing at all" and partly a wash is exactly the case this exists for —
        // over a white matte a 25% white band and the empty ground are the same colour, and
        // without the clear level to split against, the band is simply lost.
        let keep: Vec<f32> = groups
            .iter()
            .filter(|&&(_, spread, n)| spread <= LEVEL_SPREAD && n as f32 / total >= MIN_SHARE)
            .map(|&(mean, _, _)| if mean < CLEAR { 0.0 } else { mean })
            .collect();
        // One level is the ordinary case: the ink is simply that opaque.
        if keep.len() > 1 {
            levels[ink] = keep;
        } else if let Some(&m) = keep.first() {
            pal.alpha[ink] = m;
        }
    }

    // Mint the extra inks and move the pixels onto them.
    let mut minted = 0usize;
    let mut extra: Vec<(usize, f32)> = Vec::new(); // (source ink, opacity)
    for (ink, ls) in levels.iter().enumerate() {
        if ls.is_empty() {
            continue;
        }
        // The most opaque level keeps the original entry; the rest get new ones.
        let mut ordered = ls.clone();
        ordered.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        pal.alpha[ink] = ordered[0];
        for &lvl in &ordered[1..] {
            extra.push((ink, lvl));
            minted += 1;
        }
    }
    if minted == 0 {
        return 0;
    }
    let base = pal.len();
    for &(ink, lvl) in &extra {
        pal.colors.push(pal.colors[ink]);
        pal.rgb.push(pal.rgb[ink]);
        pal.weight.push(0.0);
        pal.alpha.push(lvl);
    }
    for (p, l) in labels.iter_mut().enumerate() {
        let ink = *l as usize;
        if ink >= n_inks || levels[ink].is_empty() {
            continue;
        }
        let a = alpha[p];
        // Nearest level, and the entry that carries it.
        let mut best = (f32::INFINITY, *l);
        if (a - pal.alpha[ink]).abs() < best.0 {
            best = ((a - pal.alpha[ink]).abs(), *l);
        }
        for (k, &(src, lvl)) in extra.iter().enumerate() {
            if src == ink && (a - lvl).abs() < best.0 {
                best = ((a - lvl).abs(), (base + k) as u16);
            }
        }
        *l = best.1;
    }
    minted
}

/// Assign every pixel to its nearest palette entry.
pub fn label_image(rgb: &[[f32; 3]], pal: &Palette) -> Vec<u16> {
    rgb.par_iter()
        .map(|&c| pal.nearest(rgb_to_oklab(c)).0 as u16)
        .collect()
}

#[cfg(test)]
mod de00_tests {
    use super::de00;
    fn c(r: u8, g: u8, b: u8) -> [f32; 3] {
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
    }
    #[test]
    fn matches_reference_values() {
        // skimage.color.deltaE_ciede2000 on the same pairs.
        assert!((de00(c(0, 0, 0), c(2, 2, 2)) - 0.31).abs() < 0.05);
        assert!((de00(c(0, 0, 0), c(7, 7, 7)) - 1.11).abs() < 0.05);
        assert!((de00(c(7, 96, 84), c(15, 103, 91)) - 2.35).abs() < 0.08);
        assert!((de00(c(128, 128, 128), c(136, 136, 136)) - 2.95).abs() < 0.08);
        assert_eq!(de00(c(50, 100, 150), c(50, 100, 150)), 0.0);
    }
}
