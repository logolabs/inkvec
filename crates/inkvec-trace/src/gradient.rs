//! Per-region fill model selection (DESIGN.md §2.7 and S1).
//!
//! A region proposal from [`crate::color::label_image`] carries one palette colour. That
//! is the right description for flat art and the wrong one for a gradient, where the
//! palette step turns a smooth ramp into bands: `gradient_linear` costs 58 parameters as
//! four flat layers where the ground truth is one gradient with 12. The design calls for
//! per-region *model selection* — flat / linear / radial — chosen by minimum description
//! length on the residual, so that a gradient is not spent on a flat region and a
//! gradient is not banded into layers. This module is that selection.
//!
//! Four decisions worth stating:
//!
//! * **Fit in linear light, and also in sRGB.** Compositing and physically rendered
//!   gradients are linear in linear RGB, so that is the primary fitting space. But SVG
//!   itself interpolates gradient stops in sRGB unless told otherwise, and so does every
//!   renderer that produced the corpus — a gradient that came out of an SVG is linear
//!   in sRGB and *gamma-curved* in linear light. Both interpolations are therefore
//!   fitted and MDL picks the one that explains the pixels; the chosen space is part
//!   of the model ([`Interp`]) and is emitted with it.
//! * **The residual is measured in sRGB, and 8-bit quantisation is part of the
//!   observation model.** The residual is taken in sRGB whichever space the fit was made
//!   in, because that is the domain in which `sigma_noise` is estimated and in which the
//!   pixels were rounded. An 8-bit value `v` means the true value lies in
//!   `[v - ½LSB, v + ½LSB]`, so a prediction inside that interval is fully consistent
//!   with the observation: the residual carries a dead zone of half an LSB and is
//!   Gaussian in `sigma_noise` beyond it. That is the exact likelihood of a quantised
//!   Gaussian observation when `sigma_noise` is below the LSB, and a mildly lenient
//!   approximation above it. Without it, a plain Gaussian at the noise floor treats
//!   sub-LSB rounding as signal, and two gradients with fourteen parameters "explain"
//!   the rounding better than one gradient with seven — which is precisely the banding
//!   this module exists to remove.
//! * **Only strictly interior pixels** (all four neighbours in the region) enter a fit.
//!   Anti-aliased edge pixels are blends with the neighbouring region; they are evidence
//!   about geometry (S2), not about the fill.
//! * **Be conservative.** `cost = 0.5·chi² + λ·params` with params counted in *editable
//!   numbers* (flat 3, linear 10, radial 9). On a noisy flat region a gradient can only
//!   lower chi² by a few units, which λ·7 outweighs; a gradient must also produce a
//!   visible contrast across the region, otherwise it is fitting noise no matter what
//!   chi² says.
//!
//! [`merge_gradient_bands`] closes the loop with the palette: adjacent regions whose union
//! is described more cheaply by one gradient than by two flats are merged back into one.
//!
//! # Where this sits in the pipeline
//!
//! The Quality pipeline in `lib.rs` runs palette → labels → despeckle → blend absorption,
//! then this module's two passes: [`bands::merge_gradient_bands_with_ink`] (the
//! `merge_bands` stage) and [`carve::carve_residual_features_with_detail_noise`] (the
//! `carve` stage). The native-alpha path in `native.rs` calls the same two passes through
//! [`bands::merge_gradient_bands_guarded`]. What comes in is the composited sRGB image
//! (0..1 per channel), the label map, the palette and the noise estimate; what goes out is
//! an updated label map and one [`FillFit`] per label, which `planar.rs` reads to unmix
//! boundary pixels ([`unmix_pair`]) and the CLI emitter writes with [`fill_to_svg`] and
//! [`fade_to_svg`]. `regroup.rs` calls [`fit_candidates`] directly.
//!
//! # Files
//!
//! * this file: the model types, colour helpers, sample gathering, scoring (chi², ramp
//!   support, contrast), the ramp-or-step test and model selection;
//! * `fit`: the individual fitters (flat, linear, circular and elliptical radial);
//! * `stops`: interior stops for a fitted ramp;
//! * `eval`: the hoisted per-pixel evaluator every scoring loop uses;
//! * `evidence`: which pixels testify about a fill and which are blends;
//! * `bands`: the band-merging agglomeration; `regions`: its region-recovery switches;
//! * `gregions`, `segments`, `proposals`: research prototype A10 (`INKVEC_GREGIONS`, a
//!   `research` build only) -- spline-scored radial centres, a step-profile guard, and
//!   smooth-segment region proposals accepted by MDL;
//! * `carve`: residual features carved out as their own regions;
//! * `budget`: sampling caps and timing counters; `debug`: `INKVEC_GRADDBG` output;
//! * `svg`: the SVG writer for fills and fades.
//!
//! Coordinates are pixel centres throughout: pixel `(x, y)` is centred at `(x, y)`, in
//! the tracer's `-0.5 -0.5 w h` viewBox.

use std::collections::HashMap;

use crate::color::{linear_to_srgb, srgb_to_linear};

/// Description length of a flat fill, in editable numbers: one colour.
pub const PARAMS_FLAT: f64 = 3.0;
/// A linear gradient: two axis points and two stop colours.
pub const PARAMS_LINEAR: f64 = 10.0;
/// A radial gradient: centre, radius and two stop colours.
pub const PARAMS_RADIAL: f64 = 9.0;
/// An elliptical radial gradient: the circular one plus an aspect ratio and an angle.
pub const PARAMS_RADIAL_ELLIPTIC: f64 = 11.0;
/// Each interior stop of a gradient: an offset and a colour.
pub const PARAMS_STOP: f64 = 4.0;
/// Most interior stops a gradient is given. Of the 79 multi-stop gradients in the corpus
/// 58 have one interior stop and 8 have two; the rest are approximated by two.
pub(crate) const MAX_MID_STOPS: usize = 2;

/// Fewer interior pixels than this and a gradient cannot be told from noise.
pub(crate) const MIN_GRADIENT_PIXELS: usize = 16;
/// How much better two flat colours must fit than the best GRADIENT before a region is
/// judged a step rather than a ramp and the gradient is declined. Comparing against the
/// flat fit instead is wrong and was measured to be: two flats beat one flat on any
/// varying region, ramps included.
const BIMODAL_MARGIN: f64 = 0.85;

/// A gradient must change the fill by at least this much (sRGB, some channel) across the
/// region, in addition to a noise-relative bound. Below this it is invisible.
const MIN_VISIBLE_CONTRAST: f64 = 1.5 / 255.0;
/// Least fraction of a region's samples a gradient must visibly shade for it to be a
/// fill at all. A ramp that is flat over nine tenths of its samples and lightens only at
/// one extreme is explaining a *feature* — a lost dot, a stroke end, a chamfered tip —
/// not shading the region; the pixels it fits belong to something else (luanti: black
/// lightening to #6a6a6a over the last tenth of an elliptical radius; ebox: three
/// radials on solid black). A real gradient shades most of what it covers.
const MIN_RAMP_SUPPORT: f64 = 0.10;

/// Half an 8-bit quantisation step: the residual dead zone (see the module docs).
const QUANT_HALF_STEP: f64 = 0.5 / 255.0;

/// The space in which a gradient interpolates between its stops.
///
/// SVG's default is `sRGB`; `linearRGB` is what physical compositing does and what the
/// `color-interpolation` property selects. The two differ visibly across a wide ramp, so
/// the space is a model parameter, not a convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interp {
    /// Interpolate in linear-light RGB, what physical compositing does.
    LinearRgb,
    /// Interpolate in gamma-encoded sRGB, SVG's default.
    Srgb,
}

const INTERPS: [Interp; 2] = [Interp::LinearRgb, Interp::Srgb];

/// The fill model chosen for one region.
#[derive(Debug, Clone, PartialEq)]
pub enum FillModel {
    /// One sRGB colour.
    Flat([f32; 3]),
    /// `c0` at `p0`, `c1` at `p1`, interpolated in `interp` along the axis and padded
    /// beyond it. Points are pixel-centre coordinates (pixel `(x, y)` is centred at
    /// `(x, y)`, matching the `-0.5 -0.5 w h` viewBox the tracer emits).
    Linear {
        /// Pixel-centre position where the axis starts, at offset 0.
        p0: (f64, f64),
        /// Pixel-centre position where the axis ends, at offset 1.
        p1: (f64, f64),
        /// Colour at `p0`.
        c0: [f32; 3],
        /// Colour at `p1`.
        c1: [f32; 3],
        /// Colour space the interpolation is done in.
        interp: Interp,
        /// Interior stops `(offset, colour)`, offsets ascending in `(0, 1)`. The profile
        /// is piecewise linear between consecutive stops.
        mids: Vec<(f64, [f32; 3])>,
    },
    /// `c0` at the centre `c`, `c1` at radius `r`, padded beyond it.
    ///
    /// The iso-colour curves are ellipses: semi-axis `r` along the direction `angle`
    /// (radians, from +x towards +y) and `r / aspect` across it. `aspect == 1` is the
    /// plain circular gradient. Illustrator writes nearly every radial gradient with a
    /// `gradientTransform`, and in the Noto corpus 217 of 279 of them are elliptical with
    /// an aspect above 1.1; a circle fitted to any of those is wrong in a way no colour
    /// tolerance can hide, and the bands it fails to merge are the visible result.
    Radial {
        /// Centre, at offset 0.
        c: (f64, f64),
        /// Radius along `angle`, at offset 1.
        r: f64,
        /// Colour at the centre.
        c0: [f32; 3],
        /// Colour at the ellipse.
        c1: [f32; 3],
        /// Colour space the interpolation is done in.
        interp: Interp,
        /// Ratio of `r` to the semi-axis across `angle`: that semi-axis is `r / aspect`.
        /// `1.0` is a plain circular gradient; the elliptical fitter only produces values
        /// in `[1.02, 8]`.
        aspect: f64,
        /// Direction of the `r` semi-axis, in radians from +x towards +y.
        angle: f64,
        /// Interior stops, as for [`FillModel::Linear`].
        mids: Vec<(f64, [f32; 3])>,
    },
}

/// Normalised axial coordinate of `(x, y)`: 0 at `p0`, 1 at `p1`, padded beyond.
///
/// The projection of the point onto the axis, as a fraction of the axis length:
/// `t = clamp(((P − p0)·d) / |d|², 0, 1)` with `d = p1 − p0`, all in pixel-centre units.
/// The clamp is SVG's `spreadMethod="pad"`: beyond either end the end colour holds. A
/// degenerate axis (`p0 == p1`) gives 0 everywhere, the first stop.
#[inline]
fn linear_t(x: f64, y: f64, p0: (f64, f64), p1: (f64, f64)) -> f64 {
    let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
    let dd = dx * dx + dy * dy;
    if dd <= 0.0 {
        0.0
    } else {
        (((x - p0.0) * dx + (y - p0.1) * dy) / dd).clamp(0.0, 1.0)
    }
}

/// Normalised radial coordinate of `(x, y)`: 0 at the centre, 1 on the ellipse.
///
/// See [`eval::radial_t_rot`] for the formula. For a circle (`aspect == 1`) or a
/// non-positive radius the rotation is irrelevant and `sin_cos` is not computed.
#[inline]
fn radial_t(x: f64, y: f64, c: (f64, f64), r: f64, aspect: f64, angle: f64) -> f64 {
    if r <= 0.0 || aspect == 1.0 {
        return eval::radial_t_rot(x, y, c, r, aspect, (0.0, 1.0));
    }
    eval::radial_t_rot(x, y, c, r, aspect, angle.sin_cos())
}

impl FillModel {
    /// Description length in editable numbers: the base count of the model family
    /// ([`PARAMS_FLAT`], [`PARAMS_LINEAR`], [`PARAMS_RADIAL`] or
    /// [`PARAMS_RADIAL_ELLIPTIC`]) plus [`PARAMS_STOP`] per interior stop. This is the
    /// `params` term of the MDL cost `0.5·chi² + λ·params`.
    pub fn params(&self) -> f64 {
        match self {
            FillModel::Flat(_) => PARAMS_FLAT,
            FillModel::Linear { mids, .. } => PARAMS_LINEAR + PARAMS_STOP * mids.len() as f64,
            FillModel::Radial { aspect, mids, .. } => {
                let base = if *aspect == 1.0 {
                    PARAMS_RADIAL
                } else {
                    PARAMS_RADIAL_ELLIPTIC
                };
                base + PARAMS_STOP * mids.len() as f64
            }
        }
    }

    /// A short name for diagnostics.
    pub fn kind(&self) -> &'static str {
        match self {
            FillModel::Flat(_) => "flat",
            FillModel::Linear {
                interp: Interp::Srgb,
                ..
            } => "linear/srgb",
            FillModel::Linear { .. } => "linear/lin",
            FillModel::Radial {
                interp: Interp::Srgb,
                aspect,
                ..
            } if *aspect == 1.0 => "radial/srgb",
            FillModel::Radial {
                interp: Interp::Srgb,
                ..
            } => "ellipse/srgb",
            FillModel::Radial { aspect, .. } if *aspect == 1.0 => "radial/lin",
            FillModel::Radial { .. } => "ellipse/lin",
        }
    }

    /// Whether this is a `Linear` or `Radial` model, as opposed to `Flat`.
    pub fn is_gradient(&self) -> bool {
        !matches!(self, FillModel::Flat(_))
    }

    /// The fill colour (sRGB, 0..1) this model predicts at a pixel-centre position.
    ///
    /// The position is mapped to the gradient coordinate `t` ([`linear_t`] or
    /// [`radial_t`], padded to `[0, 1]`) and the stop profile is evaluated there
    /// ([`eval_stops`]). For many positions of one model use [`FillModel::eval`], which
    /// returns the same bits with the per-model work hoisted out.
    pub fn color_at(&self, x: f64, y: f64) -> [f32; 3] {
        match *self {
            FillModel::Flat(c) => c,
            FillModel::Linear {
                p0,
                p1,
                c0,
                c1,
                interp,
                ref mids,
            } => eval_stops(c0, mids, c1, linear_t(x, y, p0, p1), interp),
            FillModel::Radial {
                c,
                r,
                c0,
                c1,
                interp,
                aspect,
                angle,
                ref mids,
            } => eval_stops(c0, mids, c1, radial_t(x, y, c, r, aspect, angle), interp),
        }
    }

    /// One colour standing in for the whole fill: the flat colour, or the midpoint of a
    /// gradient's end stops. Used where a region only needs to be told apart from its
    /// neighbour, not rendered.
    pub fn representative(&self) -> [f32; 3] {
        let mid = |a: [f32; 3], b: [f32; 3]| {
            [
                0.5 * (a[0] + b[0]),
                0.5 * (a[1] + b[1]),
                0.5 * (a[2] + b[2]),
            ]
        };
        match *self {
            FillModel::Flat(c) => c,
            FillModel::Linear { c0, c1, .. } => mid(c0, c1),
            FillModel::Radial { c0, c1, .. } => mid(c0, c1),
        }
    }
}

/// A fitted model with the numbers the selection was made on.
#[derive(Debug, Clone)]
pub struct FillFit {
    /// The fitted model itself.
    pub model: FillModel,
    /// Sum over interior pixels and channels of `(residual / sigma_noise)²`, residual in
    /// sRGB beyond the half-LSB quantisation dead zone.
    pub chi2: f64,
    /// Description length in editable numbers; see [`FillModel::params`].
    pub params: f64,
    /// `0.5·chi2 + lambda·params`.
    pub cost: f64,
}

/// The BIC choice of `lambda` for `n` observations, `0.5·ln(n)`.
///
/// This is the value at which the MDL cost is the Bayesian information criterion, and a
/// sensible default: it grows slowly with region size, so a large region has to earn a
/// gradient with proportionally more evidence than a small one does.
pub fn bic_lambda(n: usize) -> f64 {
    0.5 * (n.max(2) as f64).ln()
}

// ---------------------------------------------------------------------------------------
// Colour helpers
// ---------------------------------------------------------------------------------------

/// An sRGB colour (0..1, gamma-encoded) in linear light, widened to `f64` for fitting.
/// Uses the sRGB transfer function of [`crate::color::srgb_to_linear`].
pub(crate) fn to_lin(c: [f32; 3]) -> [f64; 3] {
    [
        srgb_to_linear(c[0]) as f64,
        srgb_to_linear(c[1]) as f64,
        srgb_to_linear(c[2]) as f64,
    ]
}

/// A linear-light colour back to sRGB (0..1). Each channel is clamped to `[0, 1]` first,
/// because a least-squares stop can overshoot the gamut and the transfer function is
/// only defined on that range.
fn to_srgb(c: [f64; 3]) -> [f32; 3] {
    [
        linear_to_srgb(c[0].clamp(0.0, 1.0) as f32),
        linear_to_srgb(c[1].clamp(0.0, 1.0) as f32),
        linear_to_srgb(c[2].clamp(0.0, 1.0) as f32),
    ]
}

/// A colour in the fitting space `space`, as an sRGB stop: converted from linear light
/// for [`Interp::LinearRgb`], taken as is for [`Interp::Srgb`]; clamped to `[0, 1]`
/// either way, since fitted stops may overshoot.
pub(crate) fn from_space(c: [f64; 3], space: Interp) -> [f32; 3] {
    match space {
        Interp::LinearRgb => to_srgb(c),
        Interp::Srgb => [
            c[0].clamp(0.0, 1.0) as f32,
            c[1].clamp(0.0, 1.0) as f32,
            c[2].clamp(0.0, 1.0) as f32,
        ],
    }
}

/// Interpolate two sRGB stops in the given space: `c(t) = c0 + t·(c1 − c0)`, taken on
/// the sRGB values for [`Interp::Srgb`] and on their linear-light values (then converted
/// back) for [`Interp::LinearRgb`]. `t` is expected in `[0, 1]`.
fn lerp_stops(c0: [f32; 3], c1: [f32; 3], t: f64, interp: Interp) -> [f32; 3] {
    match interp {
        Interp::LinearRgb => eval::lerp_lin(to_lin(c0), to_lin(c1), t),
        Interp::Srgb => {
            let t = t as f32;
            [
                c0[0] + (c1[0] - c0[0]) * t,
                c0[1] + (c1[1] - c0[1]) * t,
                c0[2] + (c1[2] - c0[2]) * t,
            ]
        }
    }
}

/// Evaluate a multi-stop profile: `c0` at 0, `c1` at 1, `mids` between, piecewise linear
/// in `interp`.
///
/// [`eval::segment`] finds the piece `k` that `t` falls in and the position `u` along
/// it; stop 0 is `c0`, stop `i` (1 ≤ i ≤ mids.len()) is `mids[i − 1]`, and the last is
/// `c1`. The two stops bounding the piece are then lerped at `u`. `mids` must be sorted
/// by offset.
pub(super) fn eval_stops(
    c0: [f32; 3],
    mids: &[(f64, [f32; 3])],
    c1: [f32; 3],
    t: f64,
    interp: Interp,
) -> [f32; 3] {
    let (k, u) = eval::segment(mids, t);
    let stop = |i: usize| match i {
        0 => c0,
        i if i <= mids.len() => mids[i - 1].1,
        _ => c1,
    };
    lerp_stops(stop(k), stop(k + 1), u, interp)
}

// ---------------------------------------------------------------------------------------
// Samples
// ---------------------------------------------------------------------------------------

/// The interior pixels of one region, in raster order: the observations every fill fit
/// is made on. The five vectors are parallel, one entry per sample.
pub(crate) struct Samples {
    /// Pixel indices (`y·w + x`), sorted ascending.
    pub(crate) px: Vec<usize>,
    /// Pixel-centre x coordinate of each sample, px.
    pub(crate) x: Vec<f64>,
    /// Pixel-centre y coordinate of each sample, px.
    pub(crate) y: Vec<f64>,
    /// Observed sRGB.
    pub(crate) srgb: Vec<[f32; 3]>,
    /// The same in linear light, converted once at gather time: every fit used to
    /// reconvert all of its samples per candidate space, which was most of a fit.
    pub(crate) lin: Vec<[f64; 3]>,
}

impl Samples {
    fn len(&self) -> usize {
        self.px.len()
    }

    /// The sample index of pixel `p`, if `p` is a sample (binary search: `px` is sorted).
    fn sample_at(&self, p: usize) -> Option<usize> {
        self.px.binary_search(&p).ok()
    }

    /// Mean sample position, px. With no samples the sums are divided by 1 and the
    /// centroid is `(0, 0)`.
    fn centroid(&self) -> (f64, f64) {
        let n = self.len().max(1) as f64;
        (
            self.x.iter().sum::<f64>() / n,
            self.y.iter().sum::<f64>() / n,
        )
    }

    /// The colours in the fitting space.
    fn colors(&self, space: Interp) -> Vec<[f64; 3]> {
        match space {
            Interp::LinearRgb => self.lin.clone(),
            Interp::Srgb => self
                .srgb
                .iter()
                .map(|c| [c[0] as f64, c[1] as f64, c[2] as f64])
                .collect(),
        }
    }
}

/// Per-channel mean of `cols`; `[0, 0, 0]` for an empty slice (the divisor is floored
/// at 1).
fn mean3(cols: &[[f64; 3]]) -> [f64; 3] {
    let n = cols.len().max(1) as f64;
    let mut m = [0.0; 3];
    for c in cols {
        for k in 0..3 {
            m[k] += c[k];
        }
    }
    [m[0] / n, m[1] / n, m[2] / n]
}

/// How many of `pixels` are strictly interior (every in-image 4-neighbour satisfies
/// `member`).
fn interior_count(pixels: &[usize], w: usize, h: usize, member: impl Fn(usize) -> bool) -> usize {
    pixels
        .iter()
        .filter(|&&p| {
            // The picture edge is a boundary: a pixel on it is half-covered by
            // whatever the icon does off-canvas and testifies to nothing about the
            // fill. Treated as interior, 64 border pixels of luminance 0.50 on a solid
            // black shape (luanti) outweighed 550 pure ones and bought a radial that
            // lightens at the rim.
            let (x, y) = (p % w, p / w);
            x > 0
                && x + 1 < w
                && y > 0
                && y + 1 < h
                && member(p - 1)
                && member(p + 1)
                && member(p - w)
                && member(p + w)
        })
        .count()
}

/// Gather the pixels of `pixels` that are strictly interior: not on the picture edge, and
/// every 4-neighbour satisfies `member`.
///
/// With `strict == false` every member pixel is taken. That is the fallback for regions
/// too thin to have an interior at all (a 1px line), which can only ever be flat.
///
/// Either way a pixel must also pass `evidence` (see [`fill_evidence`]): blends towards
/// a neighbouring ink say nothing about this region's fill. `rgb` is the sRGB image
/// (0..1), `w`×`h` its size; the returned [`Samples`] are sorted by pixel index whatever
/// order `pixels` came in, so the fit does not depend on how the caller built the list.
fn collect_samples(
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pixels: &[usize],
    member: impl Fn(usize) -> bool + Sync,
    evidence: impl Fn(usize) -> bool + Sync,
    strict: bool,
) -> Samples {
    // The filter and the gather are the per-pixel cost of every fill fit and every
    // merge refit; rayon's ordered collect keeps `px` in the same order as before.
    use rayon::prelude::*;
    let mut px: Vec<usize> = pixels
        .par_iter()
        .copied()
        .filter(|&p| {
            if !evidence(p) {
                return false;
            }
            if !strict {
                return true;
            }
            // Interior: all four in-image neighbours belong to the region, and the pixel
            // is not on the picture edge. The edge is a boundary - a pixel on it is
            // half-covered by whatever the icon does off-canvas and testifies to nothing
            // about the fill; treated as interior, 64 border pixels of luminance 0.50
            // on a solid black shape (luanti) outweighed 550 pure ones and bought a
            // radial that lightens at the rim. (Eight neighbours was tried and measured
            // worse: it starves genuine gradients of samples, dev objective +0.007.)
            let (x, y) = (p % w, p / w);
            x > 0
                && x + 1 < w
                && y > 0
                && y + 1 < h
                && member(p - 1)
                && member(p + 1)
                && member(p - w)
                && member(p + w)
        })
        .collect();
    px.par_sort_unstable();
    let x: Vec<f64> = px.par_iter().map(|&p| (p % w) as f64).collect();
    let y: Vec<f64> = px.par_iter().map(|&p| (p / w) as f64).collect();
    let srgb: Vec<[f32; 3]> = px.par_iter().map(|&p| rgb[p]).collect();
    let lin: Vec<[f64; 3]> = srgb.par_iter().map(|&c| to_lin(c)).collect();
    Samples {
        px,
        x,
        y,
        srgb,
        lin,
    }
}

/// The pair of colours to unmix a boundary pixel at `(x, y)` between fills `a` and `b`
/// against, and their separation (sRGB, Euclidean).
///
/// Two descriptions of the faces are on offer: what each fill predicts *at the point*,
/// and one representative colour per face. A real edge wants the first — a black eye on
/// a radial skin fitted with a dark centre unmixed against the skin's mid-stop read its
/// anti-aliasing as mostly skin, and the eye collapsed. A quantisation seam inside one
/// ramp, each band fitted as a gradient, wants the second: the two fits agree at the
/// seam to within noise, so the local projection divides by nothing and the vertex
/// jumps a pixel either way, while the band representatives straddle the seam and put
/// it where the ramp crosses their midpoint — the quantisation threshold itself.
/// Whichever pair separates the faces more is the better-conditioned estimator.
pub fn unmix_pair(a: &FillModel, b: &FillModel, x: f64, y: f64) -> ([f32; 3], [f32; 3], f64) {
    fn sep(p: [f32; 3], q: [f32; 3]) -> f64 {
        let d = [
            (p[0] - q[0]) as f64,
            (p[1] - q[1]) as f64,
            (p[2] - q[2]) as f64,
        ];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }
    let (la, lb) = (a.color_at(x, y), b.color_at(x, y));
    let local = sep(la, lb);
    if !a.is_gradient() && !b.is_gradient() {
        return (la, lb, local);
    }
    let (ra, rb) = (a.representative(), b.representative());
    let rep = sep(ra, rb);
    if local >= rep {
        (la, lb, local)
    } else {
        (ra, rb, rep)
    }
}

/// The principal axis of the samples' sRGB colours about `mean`, by eight rounds of
/// power iteration on their scatter matrix `S = Σ_i d_i d_iᵀ`, `d_i = c_i − mean`, over
/// the sample indices `idx`.
///
/// Each round computes `v ← S v / |S v|` without forming `S` (as `Σ_i (d_i·v) d_i`),
/// starting from the grey direction `(1, 1, 1)`. Eight rounds suffice because only the
/// split direction for a two-cluster test is wanted, not a precise eigenvector. `None`
/// when the samples have no spread along the iterate (all one colour, or a spread
/// exactly orthogonal to grey that the start vector cannot see).
fn dominant_color_axis(s: &Samples, idx: &[usize], mean: [f64; 3]) -> Option<[f64; 3]> {
    let mut axis = [1.0f64, 1.0, 1.0];
    for _ in 0..8 {
        let mut next = [0.0f64; 3];
        for &i in idx {
            let mut d = [0.0f64; 3];
            let mut dot = 0.0;
            for c in 0..3 {
                d[c] = s.srgb[i][c] as f64 - mean[c];
                dot += d[c] * axis[c];
            }
            for c in 0..3 {
                next[c] += dot * d[c];
            }
        }
        let norm = (next[0] * next[0] + next[1] * next[1] + next[2] * next[2]).sqrt();
        if norm < 1e-12 {
            return None;
        }
        for c in 0..3 {
            axis[c] = next[c] / norm;
        }
    }
    Some(axis)
}

/// Residual of the best *two* flat colours for these samples, on the same scale as
/// [`Predictions::chi2`]. This is not a model the emitter can write, and it is not meant
/// to be: it exists only to answer a question about the alternative, which is whether
/// the variation in a region is a ramp or a step (see [`BIMODAL_MARGIN`]).
///
/// The method is one-dimensional k-means with k = 2 (Lloyd's iteration) along the
/// samples' dominant colour axis ([`dominant_color_axis`]): project each sRGB colour to
/// `t_i = (c_i − mean)·v`, start the split at the midpoint of the range and move it to
/// the midpoint of the two cluster means, 24 times. The two clusters' mean colours
/// `μ_0`, `μ_1` are then scored like any model:
///
/// `chi² = (n/m) · Σ_i Σ_ch max(|c_i,ch − μ_g(i),ch| − ½LSB, 0)² / σ²`
///
/// over a strided subsample of `m` of the `n` samples, scaled back to `n`. Returns
/// infinity when there is no split to make: fewer than two samples, fewer than four in
/// the subsample, no colour axis, no spread along it, or one cluster empty.
fn chi2_two_flats(s: &Samples, sigma: f64) -> f64 {
    let n = s.len();
    if n < 2 {
        return f64::INFINITY;
    }
    let stride = (n / fit_cap()).max(1);
    let idx: Vec<usize> = (0..n).step_by(stride).collect();
    let m = idx.len();
    if m < 4 {
        return f64::INFINITY;
    }
    // Mean, then the dominant axis by power iteration on the covariance.
    let mut mean = [0.0f64; 3];
    for &i in &idx {
        for c in 0..3 {
            mean[c] += s.srgb[i][c] as f64;
        }
    }
    for c in 0..3 {
        mean[c] /= m as f64;
    }
    let axis = match dominant_color_axis(s, &idx, mean) {
        Some(a) => a,
        None => return f64::INFINITY,
    };
    let t: Vec<f64> = idx
        .iter()
        .map(|&i| {
            (0..3)
                .map(|c| (s.srgb[i][c] as f64 - mean[c]) * axis[c])
                .sum::<f64>()
        })
        .collect();
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for &v in &t {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if hi - lo < 1e-12 {
        return f64::INFINITY;
    }
    let mut split = 0.5 * (lo + hi);
    for _ in 0..24 {
        let (mut sa, mut na, mut sb, mut nb) = (0.0, 0usize, 0.0, 0usize);
        for &v in &t {
            if v < split {
                sa += v;
                na += 1;
            } else {
                sb += v;
                nb += 1;
            }
        }
        if na == 0 || nb == 0 {
            return f64::INFINITY;
        }
        split = 0.5 * (sa / na as f64 + sb / nb as f64);
    }
    // The two means, in colour, and the residual against them.
    let mut sum = [[0.0f64; 3]; 2];
    let mut cnt = [0usize; 2];
    for (k, &i) in idx.iter().enumerate() {
        let g = if t[k] < split { 0 } else { 1 };
        cnt[g] += 1;
        for c in 0..3 {
            sum[g][c] += s.srgb[i][c] as f64;
        }
    }
    if cnt[0] == 0 || cnt[1] == 0 {
        return f64::INFINITY;
    }
    let mut mu = [[0.0f64; 3]; 2];
    for g in 0..2 {
        for c in 0..3 {
            mu[g][c] = sum[g][c] / cnt[g] as f64;
        }
    }
    let mut acc = 0.0;
    for (k, &i) in idx.iter().enumerate() {
        let g = if t[k] < split { 0 } else { 1 };
        for c in 0..3 {
            let d = ((s.srgb[i][c] as f64 - mu[g][c]).abs() - QUANT_HALF_STEP).max(0.0);
            acc += d * d;
        }
    }
    acc / (sigma * sigma) * (n as f64 / m as f64)
}

/// The linear, radial and elliptic ramps for the samples in one interpolation space, in
/// that order, with the samples' colours in that space. The radial fit (and the elliptic
/// one it seeds) runs beside the linear one.
fn ramp_models(s: &Samples, w: usize, space: Interp) -> (Interp, Vec<[f64; 3]>, Vec<FillModel>) {
    let cols = s.colors(space);
    let ((radial, elliptic), linear) = rayon::join(
        || {
            let t_r = inkvec_core::clock::Instant::now();
            let radial = fit_radial(s, &cols, space, w);
            tick(&FIT_NS_RADIAL, t_r);
            let t_e = inkvec_core::clock::Instant::now();
            let elliptic = radial
                .as_ref()
                .and_then(|r| fit_radial_elliptic(s, &cols, space, r));
            tick(&FIT_NS_ELLIPTIC, t_e);
            (radial, elliptic)
        },
        || {
            let t_l = inkvec_core::clock::Instant::now();
            let linear = fit_linear(s, &cols, space);
            tick(&FIT_NS_LINEAR, t_l);
            linear
        },
    );
    let cands = [linear, radial, elliptic].into_iter().flatten().collect();
    (space, cols, cands)
}

/// The ramp candidates of every interpolation space ([`ramp_models`], one space per entry,
/// in [`INTERPS`] order), each space's list followed by the research prototype A10's extra
/// radial geometries when its part `profile` is on: found once for both spaces under the
/// piecewise-linear profile score ([`profile_scored_radials`], run beside the line-scored
/// fits) and appended to each space's list with the stops refitted there
/// ([`restop_radial`]). With the part off there are none and the lists are as they were.
fn ramp_candidates(s: &Samples, w: usize) -> Vec<(Interp, Vec<[f64; 3]>, Vec<FillModel>)> {
    use rayon::prelude::*;
    let (geometry, mut per_space): (Vec<FillModel>, Vec<_>) = rayon::join(
        || {
            if gregions::parts().profile {
                profile_scored_radials(s, &s.colors(Interp::Srgb), Interp::Srgb, w)
            } else {
                Vec::new()
            }
        },
        || {
            INTERPS
                .par_iter()
                .map(|&space| ramp_models(s, w, space))
                .collect()
        },
    );
    for (space, cols, cands) in per_space.iter_mut() {
        cands.extend(
            geometry
                .iter()
                .filter_map(|g| restop_radial(s, cols, *space, g)),
        );
    }
    per_space
}

/// Research prototype A10, part `profile` ([`gregions`]): the circular and elliptical
/// radial geometries searched again under the piecewise-linear profile score
/// ([`fit::ProfileScore::Spline`]), the elliptical one seeded from the circular one, as
/// [`ramp_models`] seeds its own pair.
///
/// They become *extra* candidates, appended after the line-scored three in each space by
/// `fit_samples`:
/// nothing is replaced, so a region whose profile is straight keeps the fit it had (the
/// line-scored candidate comes first and wins ties), and model selection prices the
/// newcomers like any other. Replacing the line search instead was measured worse in an
/// earlier attempt (4304ba3).
///
/// The search runs once, in sRGB, and its geometry serves both interpolation spaces:
/// a free piecewise-linear profile absorbs the per-channel transfer curve between the
/// spaces, so the level sets it finds, and with them the geometry, do not depend on the
/// space; only the stops do, and `fit_samples` refits those per space. That halves the
/// cost of the part (the round-2 research ran it per space: 1.9x trace time on the
/// gradient icons, under load). Not from the literature: a cost reduction.
fn profile_scored_radials(
    s: &Samples,
    cols: &[[f64; 3]],
    space: Interp,
    w: usize,
) -> Vec<FillModel> {
    let t_r = inkvec_core::clock::Instant::now();
    let radial = fit_radial_scored(s, cols, space, w, ProfileScore::Spline);
    tick(&FIT_NS_RADIAL, t_r);
    let t_e = inkvec_core::clock::Instant::now();
    let elliptic = radial
        .as_ref()
        .and_then(|r| fit_radial_elliptic_scored(s, cols, space, r, ProfileScore::Spline));
    tick(&FIT_NS_ELLIPTIC, t_e);
    [radial, elliptic].into_iter().flatten().collect()
}

/// Every admissible candidate for the samples, flat first.
///
/// The model-selection core. Each candidate is scored `cost = 0.5·chi² + λ·params`
/// ([`Predictions::chi2`], [`FillModel::params`]); `sigma` is the per-channel noise in sRGB (floored
/// at 0.5/255 when not positive) and `lambda` the price of one editable number. The
/// steps, in order:
///
/// 1. the flat fit ([`fit_flat`]), always first, so [`select`]'s tie rule prefers it;
/// 2. an early exit when `strict` is off, the region is below [`MIN_GRADIENT_PIXELS`],
///    or flat already costs no more than the cheapest possible gradient;
/// 3. linear, radial and elliptical ramps in both interpolation spaces, each gated on a
///    visible contrast of `max(3σ, 1.5/255)` and a [`MIN_RAMP_SUPPORT`], plus their
///    multi-stop variants ([`fit_mid_stops`]) under the same gates;
/// 4. the ramp-or-step test: when two flat colours ([`chi2_two_flats`]) fit better than
///    [`BIMODAL_MARGIN`] times the best gradient's chi², every gradient is dropped.
///
/// `w` is the image width, needed to find a sample's 4-neighbours by pixel index. The
/// work is parallel (rayon) but the output order is the serial one, so ties are broken
/// the same way on any number of threads.
fn fit_samples(s: &Samples, w: usize, strict: bool, sigma: f64, lambda: f64) -> Vec<FillFit> {
    let sigma = if sigma > 0.0 { sigma } else { 0.5 / 255.0 };
    // A candidate and its predictions at the scored samples, scored.
    let scored = |model: FillModel, preds: &Predictions| {
        let c2 = preds.chi2(s, sigma);
        let params = model.params();
        FillFit {
            model,
            chi2: c2,
            params,
            cost: 0.5 * c2 + lambda * params,
        }
    };
    let score = |model: FillModel| {
        let preds = Predictions::new(&model, s);
        scored(model, &preds)
    };
    let t_f = inkvec_core::clock::Instant::now();
    let flat = fit_flat(s);
    tick(&FIT_NS_FLAT, t_f);
    let flat_c = match flat {
        FillModel::Flat(c) => c,
        _ => [0.0; 3],
    };
    let mut out = vec![score(flat)];
    if !strict || s.len() < MIN_GRADIENT_PIXELS {
        return out;
    }

    // If the flat fit already costs less than the cheapest gradient can possibly
    // cost, stop here. Every gradient model pays `lambda * params` before it
    // explains anything, and chi-squared is a sum of squares, so no gradient's
    // cost can fall below `lambda * PARAMS_RADIAL`. `select` keeps the earliest
    // candidate on a tie and flat is always first, so `<=` is exact: this cannot
    // change which model is chosen, only how long it takes to find out.
    //
    // It matters because the search it skips is the expensive one -- the radial
    // centre alone is six hundred residual evaluations over every sample -- and
    // on flat-colour artwork most regions take this exit. `merge_bands` refits
    // unions once per agglomeration round, so the saving compounds.
    if out[0].cost <= lambda * PARAMS_RADIAL {
        return out;
    }
    let n_flat_only = out.len();
    // Measured (h-series, noto-emoji): on the icons carrying the family's error, 98 of
    // 100 refused gradient candidates on regions of 200+ pixels fail *this* floor and
    // not the support one, by a hair -- contrast 0.0039-0.0055 against 0.0059 -- while
    // the MDL below would accept them by a wide margin. (`INKVEC_MIN_CONTRAST` scaled the
    // floor so the full set could price it; the default never moved.)
    let min_contrast = (3.0 * sigma).max(MIN_VISIBLE_CONTRAST);
    // Research prototype A10, part `guard`: a candidate whose profile changes visibly
    // within the width of an anti-aliased edge is drawing that edge, not shading, and is
    // refused like one below the contrast floor (see `gregions::step_like`). Off, the
    // test is never made.
    let guard = gregions::parts().guard;
    let edge = |m: &FillModel| guard && gregions::step_like(m, min_contrast);
    // Every candidate and its multi-stop variants, per interpolation space and per model, are
    // independent of one another, and the stop search is serial within one candidate: fit
    // them side by side, then take them in the order the one-at-a-time loop did, which is
    // the order `select` breaks ties in.
    use rayon::prelude::*;
    let per_space = ramp_candidates(s, w);
    let jobs: Vec<(Interp, &[[f64; 3]], &FillModel)> = per_space
        .iter()
        .flat_map(|(space, cols, cands)| cands.iter().map(move |c| (*space, &cols[..], c)))
        .collect();
    let fitted: Vec<Vec<FillFit>> = jobs
        .par_iter()
        .map(|&(space, cols, cand)| {
            let cand = cand.clone();
            let mut out = Vec::new();
            // One evaluation of the candidate per scored sample, read by all three
            // scores (see `score`).
            let preds = Predictions::new(&cand, s);
            let contrast = preds.contrast();
            let support = preds.support(flat_c, contrast);
            if contrast < min_contrast || support < MIN_RAMP_SUPPORT || edge(&cand) {
                // Which gate refused a candidate is otherwise invisible: a region that
                // ends up "cands 1" looks identical whether no ramp was ever tried or
                // every ramp was thrown away here. `INKVEC_EVDBG=1`.
                if inkvec_core::env::flag("INKVEC_EVDBG") || debug::verbose() {
                    // The data's own per-channel range, so a refusal can be read as
                    // "the truth is that subtle" or "the fit missed it".
                    let (mut lo, mut hi) = ([1f32; 3], [0f32; 3]);
                    for c in &s.srgb {
                        for k in 0..3 {
                            lo[k] = lo[k].min(c[k]);
                            hi[k] = hi[k].max(c[k]);
                        }
                    }
                    let data_range = (0..3).map(|k| hi[k] - lo[k]).fold(0f32, f32::max);
                    eprintln!(
                        "  [ev]   refused {} over {} px: contrast {:.4} (min {:.4}) support {:.3} (min {:.2}) data-range {:.4}",
                        cand.kind(), s.len(), contrast, min_contrast, support, MIN_RAMP_SUPPORT, data_range
                    );
                }
                return out;
            }
            let multi = fit_mid_stops(&cand, s, cols, space);
            out.push(scored(cand, &preds));
            for m in multi {
                let pm = Predictions::new(&m, s);
                let c = pm.contrast();
                if c >= min_contrast && pm.support(flat_c, c) >= MIN_RAMP_SUPPORT && !edge(&m) {
                    out.push(scored(m, &pm));
                }
            }
            out
        })
        .collect();
    out.extend(fitted.into_iter().flatten());
    // Is the variation in this region a ramp, or a step?
    //
    // A gradient is the right model for shading and the wrong one for two inks the palette
    // merged into one, and both raise the residual of a single flat colour -- so the flat
    // residual cannot tell them apart, and an earlier version of this test that compared
    // against it declined gradients everywhere and cost the corpus a fifth of its quality.
    // What separates them is how the *alternatives* rank: a step is fitted well by two
    // flat colours and badly by a ramp, and a ramp the other way round.
    //
    // Measured over the regions this tracer paints flat where the artwork varies, the best
    // linear ramp removes 13 % of the error there and the best pair of flat colours
    // removes 66 %: those are steps
    // wearing a ramp's clothes. So price the step, and where it beats the best gradient
    // outright, decline the gradient. The region stays flat and its real problem -- that
    // it should have been two regions -- is left for the palette to solve rather than
    // papered over with a paint the artist never used.
    //
    // Where the variation genuinely is a ramp, the gradient wins this comparison and
    // nothing changes.
    if out.len() > n_flat_only {
        let best_grad = out[n_flat_only..]
            .iter()
            .map(|f| f.chi2)
            .fold(f64::INFINITY, f64::min);
        let two = chi2_two_flats(s, sigma);
        let margin = BIMODAL_MARGIN;
        debug::candidates(s.len(), &out, two, best_grad);
        if two.is_finite() && best_grad.is_finite() && two < margin * best_grad {
            out.truncate(n_flat_only);
        }
    }
    out
}

/// The cheapest candidate; on a tie the earlier (simpler) one.
///
/// # Panics
///
/// On an empty list. Every producer ([`fit_samples`], [`fit_pixels`]) returns at least
/// the flat fit.
pub(crate) fn select(mut candidates: Vec<FillFit>) -> FillFit {
    let mut best = 0;
    for (i, c) in candidates.iter().enumerate() {
        if c.cost < candidates[best].cost {
            best = i;
        }
    }
    candidates.swap_remove(best)
}

/// A flat fill of sRGB colour `c` scored as if it fitted perfectly: chi² 0, cost
/// `λ·PARAMS_FLAT`. Used where a fill is assigned rather than fitted (a palette label
/// with no pixels, a carved feature) or where there is nothing to fit.
pub(crate) fn flat_only(c: [f32; 3], lambda: f64) -> FillFit {
    FillFit {
        model: FillModel::Flat(c),
        chi2: 0.0,
        params: PARAMS_FLAT,
        cost: lambda * PARAMS_FLAT,
    }
}

/// Fit the pixels `pixels` (region membership given by `member`): interior pixels if
/// there are any, otherwise all of them and flat only.
///
/// Three tiers of samples, taken in order until one is non-empty: strictly interior
/// pixels that pass `evidence` (all candidates are fitted); any pixel that passes
/// `evidence`; any pixel at all (both flat only). An empty `pixels` gives a black flat
/// fill at the cost of its parameters. Returns every candidate, flat first; the caller
/// picks with [`select`]. `sigma` is the sRGB noise, `lambda` the parameter price.
///
/// With `INKVEC_EVDBG` set, the samples and the chosen model are printed to stderr.
///
/// Two of the arguments are closures the caller builds per region; they cannot live
/// in a struct shared between calls.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fit_pixels(
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pixels: &[usize],
    member: impl Fn(usize) -> bool + Sync,
    evidence: impl Fn(usize) -> bool + Sync,
    sigma: f64,
    lambda: f64,
) -> Vec<FillFit> {
    if pixels.is_empty() {
        return vec![flat_only([0.0; 3], lambda)];
    }
    // Interior pixels that are evidence for the fill; then any evidence pixel; then,
    // for a region made entirely of blends (a one-pixel sliver), every pixel, which can
    // only ever come back flat.
    let t_c = inkvec_core::clock::Instant::now();
    let s = collect_samples(rgb, w, h, pixels, &member, &evidence, true);
    tick(&FIT_NS_COLLECT, t_c);
    if inkvec_core::env::flag("INKVEC_EVDBG") {
        let all = collect_samples(rgb, w, h, pixels, &member, |_| true, true);
        let fits = if s.len() > 0 {
            fit_samples(&s, w, true, sigma, lambda)
        } else {
            Vec::new()
        };
        let best = if fits.is_empty() {
            flat_only([0.0; 3], lambda)
        } else {
            select(fits.clone())
        };
        let flat_c = match fit_flat(&s) {
            FillModel::Flat(c) => c,
            _ => [0.0; 3],
        };
        let lum: Vec<f32> = s.srgb.iter().map(|c| (c[0] + c[1] + c[2]) / 3.0).collect();
        let (mn, mx) = lum
            .iter()
            .fold((1f32, 0f32), |(a, b), &v| (a.min(v), b.max(v)));
        let mean = lum.iter().sum::<f32>() / lum.len().max(1) as f32;
        let n_hi = lum.iter().filter(|&&v| v > 0.1).count();
        eprintln!(
            "  [ev]   samples lum min {:.3} mean {:.3} max {:.3}, >0.1: {}/{}, flat {:?}",
            mn,
            mean,
            mx,
            n_hi,
            lum.len(),
            flat_c
        );
        let hi: Vec<String> = (0..s.len())
            .filter(|&i| lum[i] > 0.1)
            .take(40)
            .map(|i| format!("({},{}:{:.2})", s.x[i], s.y[i], lum[i]))
            .collect();
        eprintln!("  [ev]   light samples: {}", hi.join(" "));
        let contrast = visible_contrast(&best.model, &s);
        eprintln!(
            "  [ev] fit: {} pixels, {} strict interior, {} of them evidence -> {} (cands {}) contrast {:.3} support {:.3} chi2 {:.1} vs flat {:.1} sigma {:.4}",
            pixels.len(), all.len(), s.len(),
            match &best.model { FillModel::Flat(_) => "flat".to_string(), m => format!("{:?}", m).chars().take(100).collect() },
            fits.len(), contrast, ramp_support(&best.model, &s, flat_c, contrast), best.chi2,
            fits.first().map(|f| f.chi2).unwrap_or(0.0), sigma
        );
    }
    if s.len() > 0 {
        return fit_samples(&s, w, true, sigma, lambda);
    }
    let s = collect_samples(rgb, w, h, pixels, &member, &evidence, false);
    if s.len() > 0 {
        return fit_samples(&s, w, false, sigma, lambda);
    }
    let s = collect_samples(rgb, w, h, pixels, &member, |_| true, false);
    fit_samples(&s, w, false, sigma, lambda)
}

/// Every admissible fill model for region `label`, scored; the flat model comes first.
///
/// This is [`fit_fill`] without the selection, for callers that want to report or
/// inspect the alternatives. A gradient candidate is omitted when the region has no
/// interior, is too small, or the gradient's contrast across the region is below the
/// noise (`max(3·sigma_noise, 1.5/255)` in sRGB). Each gradient family appears once per
/// interpolation space.
pub fn fit_candidates(
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    labels: &[u16],
    label: u16,
    sigma_noise: f64,
    lambda: f64,
) -> Vec<FillFit> {
    let pixels: Vec<usize> = (0..w * h).filter(|&p| labels[p] == label).collect();
    fit_pixels(
        rgb,
        w,
        h,
        &pixels,
        |p| labels[p] == label,
        |_| true,
        sigma_noise,
        lambda,
    )
}

/// Choose the fill model for region `label` by minimum description length.
///
/// `rgb` is the composited sRGB image, `labels` the per-pixel region assignment (as from
/// [`crate::color::label_image`]). Only pixels strictly interior to the region enter the
/// fit. `sigma_noise` is the per-channel pixel noise in sRGB units (as from
/// [`crate::coverage::estimate_noise`]); `lambda` is the description-length weight, for
/// which [`bic_lambda`] is a principled default.
///
/// `cost = 0.5·chi² + lambda·params` with params 3 / 10 / 9 for flat / linear / radial.
/// Ties go to the simpler model. A region with no pixels yields a black flat fill.
pub fn fit_fill(
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    labels: &[u16],
    label: u16,
    sigma_noise: f64,
    lambda: f64,
) -> FillFit {
    select(fit_candidates(
        rgb,
        w,
        h,
        labels,
        label,
        sigma_noise,
        lambda,
    ))
}

pub mod bands;
mod budget;
pub mod carve;
mod debug;
pub(crate) mod eval;
mod evidence;
mod fit;
mod gregions;
mod proposals;
pub(crate) mod regions;
mod score;
mod segments;
pub(crate) mod stops;
pub mod svg;

pub use bands::{merge_gradient_bands, merge_gradient_bands_with_ink};
pub(crate) use budget::*;
pub use carve::{carve_residual_features, carve_residual_features_with_detail_noise};
pub(crate) use evidence::*;
use fit::*;
use score::{ramp_support, visible_contrast, Predictions};
pub(crate) use stops::fit_mid_stops;
pub use svg::{fade_to_svg, fill_to_svg};

#[cfg(test)]
mod tests;
