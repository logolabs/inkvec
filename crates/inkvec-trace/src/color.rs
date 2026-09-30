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
//!
//! # Where this sits
//!
//! This is the first colour stage of the Quality pipeline
//! ([`crate::trace_color_full_with_alpha`]): **palette**, then **labels**.
//!
//! * [`extract_palette_mdl`] takes the image as sRGB `[0, 1]` (composited onto white) and
//!   returns a [`Palette`]: the inks in OKLab and sRGB, each with its share of the pixels.
//! * [`label_image`] assigns every pixel its nearest ink in OKLab, giving the label map
//!   that [`crate::regions`] cleans and the gradient stages build on.
//! * [`split_alpha_inks`] runs after labelling when the source had an alpha channel, and
//!   gives an ink drawn at two flat opacities one entry per opacity.
//!
//! The transparent-image path ([`crate::native`]) has four-channel copies of the palette
//! tests below; see that module for which function mirrors which.
//!
//! Three colour spaces appear, each for a reason:
//!
//! * **OKLab** for "is this the same colour?": Euclidean distance there is roughly
//!   perceptual, so one merge radius means the same thing everywhere.
//! * **linear RGB and sRGB** for "is this a blend?": light mixes linearly in linear RGB,
//!   and many renderers mix the encoded sRGB values instead, so a blend is looked for in
//!   both and the residual is then judged back in OKLab.
//! * **CIELAB with CIEDE2000** ([`de00`]) for "could anybody tell them apart?", because
//!   that is what the benchmark scores with and OKLab's cube-root lightness makes the
//!   first few levels above black look far apart when nobody can see them.

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

/// The sRGB decoding curve (IEC 61966-2-1): an encoded channel in `[0, 1]` to linear light.
///
/// `c / 12.92` below `0.04045`, `((c + 0.055) / 1.055)^2.4` above. Values outside
/// `[0, 1]` are not clamped; the power branch returns NaN for inputs below `-0.055`.
#[inline]
pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The sRGB encoding curve, inverse of [`srgb_to_linear`]: linear light to an encoded
/// channel. `12.92 c` below `0.0031308`, `1.055 c^(1/2.4) − 0.055` above. Not clamped;
/// callers clamp where the input may leave `[0, 1]`.
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
///
/// Decode to linear RGB, multiply by Ottosson's `M1` to get cone-like `(l, m, s)`, take the
/// cube root of each, and multiply by `M2` to get `(L, a, b)`. `L` is about 0 for black and
/// 1 for white; `a` and `b` stay within about ±0.4 for sRGB colours. The cube root is
/// defined for negative input, so out-of-gamut values do not produce NaN.
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
///
/// The inverse of [`rgb_to_oklab`] with Ottosson's published inverse matrices: `M2⁻¹`,
/// cube each component, `M1⁻¹`, encode. A colour outside the sRGB gamut is clamped per
/// channel after encoding, so the round trip holds (to float rounding) only inside the
/// gamut.
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
pub(crate) mod distinct;
mod mdl;
#[cfg(test)]
use mdl::{claim_spread, interior_fraction, straddle_fraction};

/// sRGB in `[0, 1]` to CIELAB (D65 white), as `[L*, a*, b*]` with `L*` in `[0, 100]`.
///
/// Decode to linear RGB, convert to XYZ with the sRGB (D65) matrix, divide by the white
/// point `(0.95047, 1, 1.08883)`, then `L* = 116 f(Y) − 16`, `a* = 500 (f(X) − f(Y))`,
/// `b* = 200 (f(Y) − f(Z))` with `f(t) = t^(1/3)` above `0.008856` and
/// `7.787 t + 16/116` below (the classic rounded CIE constants). Computed in `f64`,
/// returned as `f32`. This is the input to [`de00`].
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
    de00_lab(srgb_to_lab(a).map(f64::from), srgb_to_lab(b).map(f64::from)) as f32
}

/// [`de00`] on CIELAB values (D65 white). Split out so the formula can be held to Sharma,
/// Wu and Dalal's 34 published reference pairs, which are given in Lab (`color/tests.rs`).
///
/// The standard CIEDE2000 steps, with `k_L = k_C = k_H = 1`:
///
/// 1. Stretch `a*` by `1 + G`, `G = ½ (1 − √(C̄⁷ / (C̄⁷ + 25⁷)))`, which matters only
///    for near-neutral colours; recompute chroma `C'` and hue `h'` (degrees in `[0, 360)`,
///    0 for a neutral colour).
/// 2. Differences `ΔL'`, `ΔC'`, and `ΔH' = 2 √(C'₁C'₂) sin(Δh'/2)` with `Δh'` wrapped to
///    `[−180, 180]` (zero when either colour is neutral).
/// 3. Weights from the means: `S_L = 1 + 0.015 (L̄' − 50)² / √(20 + (L̄' − 50)²)`,
///    `S_C = 1 + 0.045 C̄'`, `S_H = 1 + 0.015 C̄' T` with `T` the four-cosine hue term, and
///    the rotation `R_T = −sin(2 Δθ) R_C` that corrects the blue region.
/// 4. `ΔE00 = √((ΔL'/S_L)² + (ΔC'/S_C)² + (ΔH'/S_H)² + R_T (ΔC'/S_C)(ΔH'/S_H))`.
///
/// The mean hue `h̄'` follows Sharma's rule for pairs more than 180° apart, and is the plain
/// sum when either colour is neutral. The radicand is clamped at zero before the square
/// root so rounding can never produce NaN.
pub(crate) fn de00_lab(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;
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
    (vl * vl + vc * vc + vh * vh + rt * vc * vh).max(0.0).sqrt()
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

/// How far inside a chord, as a fraction of it, a blend must lie (was `INKVEC_BLEND_TMIN`;
/// see `docs/algorithm/constants.md`).
pub(crate) const BLEND_TMIN: f32 = 0.04;

/// [`blend_pairs_cached`] with the accepted inks converted here (the tests' entry point).
#[cfg(test)]
fn blend_pairs(
    c: Oklab,
    accepted: &[Oklab],
    tol: f32,
    tmin: f32,
) -> Vec<(usize, usize, bool, f32)> {
    blend_pairs_cached(c, &mdl::InkAxes::of(accepted), tol, tmin)
}

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
/// `tmin` is [`BLEND_TMIN`]. `axes` holds the accepted inks already converted to both
/// spaces, once per ink rather than once per candidate pair.
///
/// # The formula
///
/// For each pair `(A, B)` of accepted inks and each space (linear RGB first, then sRGB),
/// with `p` the candidate in that space: `t = ((p − A) · (B − A)) / |B − A|²`, the nearest
/// point on the chord `q = A + t (B − A)`, and the residual
/// `off = |c − OKLab(clamp(q, 0, 1))|` measured in OKLab. The pair is returned as
/// `(i, j, linear, off)` when `tmin ≤ t ≤ 1 − tmin` and `off ≤ tol`. Pairs whose inks
/// coincide in that space (`|B − A|² < 1e-9`) are skipped; fewer than two inks give an
/// empty list. A pair can appear twice, once per space.
fn blend_pairs_cached(
    c: Oklab,
    axes: &mdl::InkAxes,
    tol: f32,
    tmin: f32,
) -> Vec<(usize, usize, bool, f32)> {
    let mut out: Vec<(usize, usize, bool, f32)> = Vec::new();
    if axes.srgb.len() < 2 {
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
        let inks = if space == 0 { &axes.lin } else { &axes.srgb };
        let p = f(c);
        for i in 0..inks.len() {
            for j in i + 1..inks.len() {
                let (a, b) = (inks[i], inks[j]);
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

    /// Index of the nearest palette entry, with its OKLab distance.
    ///
    /// A linear scan; ties go to the lower index. An empty palette returns
    /// `(0, f32::MAX)`, so callers must not index with the result without checking.
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

/// Greatest common divisor by Euclid's algorithm; `gcd(a, 0) = a`.
fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Stride for the statistical passes over an image of `n` pixels and `width` columns.
///
/// 1 up to [`STAT_PIXELS`] pixels (and for a zero width); above it the smallest
/// `s ≥ ⌈n / STAT_PIXELS⌉` coprime with `width`, so that visiting pixels `0, s, 2s, …` of
/// the row-major image walks diagonally through the columns instead of revisiting the
/// same few.
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
///
/// # The idea
///
/// Colours are bucketed coarsely only to make counting tractable; the palette entry is
/// then the *weighted mean* of the pixels that fall to it, so the recovered colour is not
/// snapped to a bucket centre. Entries are taken in descending frequency and a candidate
/// is rejected if it is within `merge_distance` of one already accepted, which is what
/// keeps an anti-aliased ramp from contributing entries of its own.
///
/// How many inks there are is decided by minimum description length, not by that fixed
/// distance alone. A fixed threshold cannot be right everywhere in a perceptual space.
/// Saturated inks sit far apart and survive it; pale ones cluster and do not. Ten
/// concentric rings of ten distinct hues came back as six colours, because the five pale
/// rings fell inside the threshold of each other — and the five that vanished then made
/// their regions look like gradients, which is where a DISTS@4x of 0.197 came from.
///
/// The MDL test asks the same question the rest of the pipeline asks. Folding a candidate
/// into the nearest accepted ink saves its three parameters but pays a residual over
/// every one of its pixels: `0.5·n·(d/sigma)²` against `lambda·3`. A mode with thousands
/// of pixels and a separation far above the noise is therefore kept however close the
/// fixed threshold would call it, while a handful of pixels a hair away from an existing
/// ink is folded in — which is exactly the behaviour wanted from both.
///
/// # The stages
///
/// 1. Number the image's distinct colours and convert each once to OKLab, sRGB and linear
///    RGB (`mdl::ClassicView`; every later per-pixel quantity is read through the pixel's
///    colour id, see [`distinct`]).
/// 2. Bin in OKLab and rank the bins by pixel count (`mdl::frequency_modes`).
/// 3. Walk the candidates in that order and accept one only if it passes every gate, in
///    this order (stopping once `max_colors` are accepted):
///    * **rarity**: it would claim at least [`MIN_INK_WEIGHT`] of the image (the first
///      ink is exempt);
///    * **perceptual floor**: CIEDE2000 to its nearest accepted ink is at least
///      `same_ink_de00` ([`same_ink_as_accepted`]);
///    * **separation**: its OKLab distance `d` to the nearest accepted ink exceeds
///      `max(merge_distance, reach)`, `reach = noise_sigmas · spread`; or, failing that,
///      the MDL escape `0.5 · claim · (d / σ)² > λ · PARAMS_PER_INK` with `d` above both
///      [`JND_FLOOR`] and `reach`;
///    * **not coverage**: it is not a blend of two accepted inks that is also thin and
///      straddling (`mdl::BlendEvidence`, over the candidate's claimed set computed once).
/// 4. Move each accepted ink to the mean of the pixels that chose it and record its share
///    (`mdl::refine_to_members`).
///
/// # Units and edge cases
///
/// `rgb` is sRGB `[0, 1]`, row-major `width × height`; `merge_distance`, `d`, `spread` and
/// `reach` are OKLab distances; `σ = ev.sigma_noise` is per-channel noise in sRGB units and
/// `λ = ev.lambda` nats per parameter. The escape's `d / σ` therefore divides an OKLab
/// distance by an sRGB one; both scales run over about `[0, 1]`, and the test treats the
/// ratio as a number of standard deviations. `σ = 0` disables the escape.
///
/// The result always has at least one ink: with `max_colors == 0` it is the most frequent
/// mode, and an empty image gives white with weight 0. Palette order is acceptance order,
/// which is deterministic (ties in frequency are broken by bin index).
pub fn extract_palette_mdl(
    rgb: &[[f32; 3]],
    width: usize,
    height: usize,
    merge_distance: f32,
    max_colors: usize,
    ev: PaletteEvidence,
) -> Palette {
    let ids = distinct::ColourIds::of_rgb(rgb);
    extract_palette_mdl_ids(rgb, &ids, width, height, merge_distance, max_colors, ev)
}

/// [`extract_palette_mdl`] with the image's colour ids already numbered
/// ([`distinct::ColourIds::of_rgb`]), so the caller can share them with [`label_image_ids`].
pub(crate) fn extract_palette_mdl_ids(
    rgb: &[[f32; 3]],
    ids: &distinct::ColourIds,
    width: usize,
    height: usize,
    merge_distance: f32,
    max_colors: usize,
    ev: PaletteEvidence,
) -> Palette {
    let view = mdl::ClassicView::new(rgb, ids, width, height);
    mdl::extract(&view, merge_distance, max_colors, ev)
}

/// The perceptual floor: is `c` within `same_ink_de00` (CIEDE2000) of the accepted ink
/// nearest to it in OKLab? False when nothing is accepted yet.
///
/// Only the OKLab-nearest ink is compared (ties to the earlier one), not the ink nearest
/// in CIEDE2000. `paldbg` prints the verdict (`INKVEC_PALDBG`).
fn same_ink_as_accepted(c: Oklab, colors: &[Oklab], same_ink_de00: f32, paldbg: bool) -> bool {
    let Some(&near_ink) = colors.iter().min_by(|&&p, &&q| {
        p.dist(c)
            .partial_cmp(&q.dist(c))
            .unwrap_or(std::cmp::Ordering::Equal)
    }) else {
        return false;
    };
    let perceptual = de00(oklab_to_rgb(c), oklab_to_rgb(near_ink));
    if perceptual < same_ink_de00 {
        if paldbg {
            eprintln!(
                "  cand {:<9} same ink as {} (dE00 {:.2} < {})",
                to_hex(oklab_to_rgb(c)),
                to_hex(oklab_to_rgb(near_ink)),
                perceptual,
                same_ink_de00
            );
        }
        return true;
    }
    false
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
/// # The procedure
///
/// For each ink with at least 16 pixels, sort its pixels' source alphas and cut the sorted
/// list wherever two neighbours differ by more than `LEVEL_GAP` (0.15). A group is a
/// *level* when its range is at most `LEVEL_SPREAD` (0.06) and it holds at least
/// `MIN_SHARE` (2 %) of the ink's pixels; its opacity is the group mean, snapped to 0 below
/// `CLEAR`. One level just sets that ink's `pal.alpha`. Two or more: the most opaque keeps
/// the original entry and each other level becomes a new entry with the same colour and
/// weight 0, and every pixel of the ink moves to the entry whose opacity is nearest its own
/// alpha (ties keep the original entry). Inks with no level are left as they are.
///
/// `labels` and `pal` are edited in place; `alpha` is the source alpha in `[0, 1]`, one
/// per pixel. A length mismatch or an empty palette changes nothing.
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
        let mut groups: Vec<(f32, f32, usize)> = Vec::new(); // (mean, spread, count)
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
///
/// `rgb` is sRGB `[0, 1]`; the distance is Euclidean in OKLab ([`Palette::nearest`], ties
/// to the lower index). This is hard nearest-ink labelling: an anti-aliased pixel gets
/// whichever ink is closest, often a third colour, which is what
/// [`crate::regions::absorb_blend_slivers`] later repairs. Returns one `u16` label per
/// pixel, in the same order. The nearest entry is found once per distinct colour (see
/// [`distinct`]) and read back through each pixel's colour id.
pub fn label_image(rgb: &[[f32; 3]], pal: &Palette) -> Vec<u16> {
    label_image_ids(rgb, &distinct::ColourIds::of_rgb(rgb), pal)
}

/// [`label_image`] with the colour ids already numbered.
pub(crate) fn label_image_ids(
    rgb: &[[f32; 3]],
    ids: &distinct::ColourIds,
    pal: &Palette,
) -> Vec<u16> {
    mdl::label(rgb, ids, pal)
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

#[cfg(test)]
pub(crate) mod reference_tests;
#[cfg(test)]
mod tests;
