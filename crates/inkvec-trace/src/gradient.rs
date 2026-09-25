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
/// varying region, ramps included. Overridable with `INKVEC_BIMODAL`.
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
        /// Ratio of the semi-axis across `angle` to `r`. `1.0` is a plain circular gradient.
        aspect: f64,
        /// Direction of the `r` semi-axis, in radians from +x towards +y.
        angle: f64,
        /// Interior stops, as for [`FillModel::Linear`].
        mids: Vec<(f64, [f32; 3])>,
    },
}

/// Normalised axial coordinate of `(x, y)`: 0 at `p0`, 1 at `p1`, padded beyond.
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
#[inline]
fn radial_t(x: f64, y: f64, c: (f64, f64), r: f64, aspect: f64, angle: f64) -> f64 {
    if r <= 0.0 {
        return 0.0;
    }
    let (dx, dy) = (x - c.0, y - c.1);
    let rho = if aspect == 1.0 {
        (dx * dx + dy * dy).sqrt()
    } else {
        let (sn, cs) = angle.sin_cos();
        let u = dx * cs + dy * sn;
        let v = (-dx * sn + dy * cs) * aspect;
        (u * u + v * v).sqrt()
    };
    (rho / r).clamp(0.0, 1.0)
}

impl FillModel {
    /// Description length in editable numbers.
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

    /// The fill colour (sRGB) this model predicts at a pixel-centre position.
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

pub(crate) fn to_lin(c: [f32; 3]) -> [f64; 3] {
    [
        srgb_to_linear(c[0]) as f64,
        srgb_to_linear(c[1]) as f64,
        srgb_to_linear(c[2]) as f64,
    ]
}

fn to_srgb(c: [f64; 3]) -> [f32; 3] {
    [
        linear_to_srgb(c[0].clamp(0.0, 1.0) as f32),
        linear_to_srgb(c[1].clamp(0.0, 1.0) as f32),
        linear_to_srgb(c[2].clamp(0.0, 1.0) as f32),
    ]
}

/// A colour in the fitting space `space`, as an sRGB stop.
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

/// Interpolate two sRGB stops in the given space.
fn lerp_stops(c0: [f32; 3], c1: [f32; 3], t: f64, interp: Interp) -> [f32; 3] {
    match interp {
        Interp::LinearRgb => {
            let (a, b) = (to_lin(c0), to_lin(c1));
            to_srgb([
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ])
        }
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
fn eval_stops(
    c0: [f32; 3],
    mids: &[(f64, [f32; 3])],
    c1: [f32; 3],
    t: f64,
    interp: Interp,
) -> [f32; 3] {
    if mids.is_empty() {
        return lerp_stops(c0, c1, t, interp);
    }
    let (mut lo_t, mut lo_c) = (0.0, c0);
    for &(off, col) in mids {
        if t <= off {
            let u = if off > lo_t {
                (t - lo_t) / (off - lo_t)
            } else {
                0.0
            };
            return lerp_stops(lo_c, col, u, interp);
        }
        lo_t = off;
        lo_c = col;
    }
    let u = if lo_t < 1.0 {
        (t - lo_t) / (1.0 - lo_t)
    } else {
        1.0
    };
    lerp_stops(lo_c, c1, u, interp)
}

// ---------------------------------------------------------------------------------------
// Samples
// ---------------------------------------------------------------------------------------

/// The interior pixels of one region, in raster order.
pub(crate) struct Samples {
    /// Pixel indices, sorted ascending.
    pub(crate) px: Vec<usize>,
    pub(crate) x: Vec<f64>,
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

    fn sample_at(&self, p: usize) -> Option<usize> {
        self.px.binary_search(&p).ok()
    }

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

// ---------------------------------------------------------------------------------------
// Fitting
// ---------------------------------------------------------------------------------------

/// Least squares of `colour = a + g·t` per channel.
///
/// Returns the residual sum of squares over all channels and the coefficients.
fn fit_1d(cols: &[[f64; 3]], t: &[f64]) -> (f64, [f64; 3], [f64; 3]) {
    let n = cols.len() as f64;
    let tbar = t.iter().sum::<f64>() / n;
    let cbar = mean3(cols);
    let mut stt = 0.0;
    let mut stc = [0.0; 3];
    let mut scc = [0.0; 3];
    for (c, &ti) in cols.iter().zip(t) {
        let dt = ti - tbar;
        stt += dt * dt;
        for k in 0..3 {
            let dc = c[k] - cbar[k];
            stc[k] += dt * dc;
            scc[k] += dc * dc;
        }
    }
    let mut g = [0.0; 3];
    let mut a = [0.0; 3];
    let mut resid = 0.0;
    for k in 0..3 {
        g[k] = if stt > 1e-12 { stc[k] / stt } else { 0.0 };
        a[k] = cbar[k] - g[k] * tbar;
        resid += (scc[k] - g[k] * g[k] * stt).max(0.0);
    }
    (resid, a, g)
}

/// The half of [`fit_1d`] a centre search recomputes for nothing.
///
/// `fit_1d` forms five sums. Two of them -- the colour mean and the per-channel colour
/// variance -- are properties of the samples alone and do not mention `t`, so a search
/// that moves a centre, an angle or an aspect around recomputes the same two numbers on
/// every evaluation. There are up to 600 of those for a circular gradient and 836 for an
/// elliptical one, on a subsample of a thousand pixels: the elliptical fit was 663 ms of
/// the 683 ms `merge_bands` spent on one emoji.
///
/// So they are formed once, here, over the same slice in the same order -- and the sums
/// that do depend on the geometry are formed exactly as `fit_1d` forms them. Every number
/// is bit for bit the one the search would have arrived at anyway; only the arithmetic
/// that was redundant is gone. The distances land in a buffer the struct owns, which also
/// retires one allocation per evaluation.
struct Resid1d<'a> {
    /// The sample coordinates, gathered contiguous: the search reads them once per
    /// evaluation and the stride made every read a scattered one.
    xs: Vec<f64>,
    ys: Vec<f64>,
    cols: &'a [[f64; 3]],
    cbar: [f64; 3],
    scc: [f64; 3],
    t: Vec<f64>,
}

impl<'a> Resid1d<'a> {
    fn new(s: &Samples, idx: &[usize], cols: &'a [[f64; 3]]) -> Self {
        let cbar = mean3(cols);
        let mut scc = [0.0; 3];
        for c in cols {
            for k in 0..3 {
                let dc = c[k] - cbar[k];
                scc[k] += dc * dc;
            }
        }
        Self {
            xs: idx.iter().map(|&i| s.x[i]).collect(),
            ys: idx.iter().map(|&i| s.y[i]).collect(),
            cols,
            cbar,
            scc,
            t: vec![0.0; idx.len()],
        }
    }

    /// Distance to a point.
    fn radial(&mut self, c: (f64, f64)) -> f64 {
        for j in 0..self.t.len() {
            self.t[j] = ((self.xs[j] - c.0).powi(2) + (self.ys[j] - c.1).powi(2)).sqrt();
        }
        self.finish()
    }

    /// Distance in the frame `(angle, aspect)` about a point.
    fn elliptic(&mut self, c: (f64, f64), sn: f64, cs: f64, k: f64) -> f64 {
        for j in 0..self.t.len() {
            let (dx, dy) = (self.xs[j] - c.0, self.ys[j] - c.1);
            let u = dx * cs + dy * sn;
            let v = (-dx * sn + dy * cs) * k;
            self.t[j] = (u * u + v * v).sqrt();
        }
        self.finish()
    }

    /// The residual of the least-squares line through `(t, colour)`, the `.0` of `fit_1d`.
    fn finish(&self) -> f64 {
        let n = self.cols.len() as f64;
        let tbar = self.t.iter().sum::<f64>() / n;
        let mut stt = 0.0;
        let mut stc = [0.0; 3];
        for (c, &ti) in self.cols.iter().zip(&self.t) {
            let dt = ti - tbar;
            stt += dt * dt;
            for k in 0..3 {
                stc[k] += dt * (c[k] - self.cbar[k]);
            }
        }
        let mut resid = 0.0;
        for k in 0..3 {
            let g = if stt > 1e-12 { stc[k] / stt } else { 0.0 };
            resid += (self.scc[k] - g * g * stt).max(0.0);
        }
        resid
    }
}

/// The flat fill is the mean in linear light: the average of the light the region emits.
/// The flat model is scored by the same sRGB residual as every other candidate, so it has
/// to be the colour that minimises that residual, not the linear-light mean converted
/// back: for a dark region with a few light outliers (blend pixels the evidence test
/// let through, a lost dot) the linear mean lands at a grey no pixel has, every pure
/// pixel then pays for the outliers, and a gradient that puts the true colour in the
/// middle and the outliers at its rim wins on solid black (luanti: flat fitted at 0.16
/// on a region whose interior is 0.00). The per-channel median is the residual's own
/// robust optimum; on a clean region it is the mean.
fn fit_flat(s: &Samples) -> FillModel {
    let n = s.len();
    if n == 0 {
        return FillModel::Flat([0.0; 3]);
    }
    let stride = (n / fit_cap()).max(1);
    let mut c = [0.0f32; 3];
    for k in 0..3 {
        let mut v: Vec<f32> = (0..n).step_by(stride).map(|i| s.srgb[i][k]).collect();
        let m = v.len() / 2;
        let (_, med, _) = v.select_nth_unstable_by(m, |a, b| a.total_cmp(b));
        c[k] = *med;
    }
    FillModel::Flat(c)
}

/// Linear gradient: axis by PCA of the colour-versus-position slope, direction refined
/// by golden-section search on the exact 1-D residual, stops by least squares.
///
/// The full affine model `colour = a + B·(x, y)` is a 3x2 slope matrix `B`; a linear
/// gradient is the rank-1 case where every channel varies along one direction. The top
/// right-singular vector of `B` is that direction's PCA estimate. Given the direction,
/// the residual of the 1-D fit is a closed form in the second moments, so refining the
/// angle costs nothing per step: a coarse scan around the PCA angle guards against the
/// estimate being poor when the channels disagree, and golden section finishes it.
fn fit_linear(s: &Samples, cols: &[[f64; 3]], space: Interp) -> Option<FillModel> {
    let n = s.len();
    if n < MIN_GRADIENT_PIXELS {
        return None;
    }
    let (xc, yc) = s.centroid();
    let cbar = mean3(cols);
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    let mut sxc = [0.0; 3];
    let mut syc = [0.0; 3];
    let mut scc = [0.0; 3];
    for (i, c) in cols.iter().enumerate() {
        let (dx, dy) = (s.x[i] - xc, s.y[i] - yc);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
        for k in 0..3 {
            let dc = c[k] - cbar[k];
            sxc[k] += dx * dc;
            syc[k] += dy * dc;
            scc[k] += dc * dc;
        }
    }
    let det = sxx * syy - sxy * sxy;
    if det <= 1e-9 * (sxx + syy).powi(2) {
        return None; // positions collinear: no 2-D axis to find
    }

    // PCA of the affine slope matrix.
    let (mut m00, mut m01, mut m11) = (0.0, 0.0, 0.0);
    for k in 0..3 {
        let bx = (syy * sxc[k] - sxy * syc[k]) / det;
        let by = (sxx * syc[k] - sxy * sxc[k]) / det;
        m00 += bx * bx;
        m01 += bx * by;
        m11 += by * by;
    }
    let theta0 = 0.5 * (2.0 * m01).atan2(m00 - m11);

    // Exact residual of the 1-D fit along direction theta, from the moments.
    let resid = |theta: f64| -> f64 {
        let (c, sn) = (theta.cos(), theta.sin());
        let sss = c * c * sxx + 2.0 * c * sn * sxy + sn * sn * syy;
        if sss <= 1e-12 {
            return f64::MAX;
        }
        let mut explained = 0.0;
        for k in 0..3 {
            let ssc = c * sxc[k] + sn * syc[k];
            explained += ssc * ssc / sss;
        }
        scc.iter().sum::<f64>() - explained
    };

    let deg = std::f64::consts::PI / 180.0;
    let mut best = (theta0, resid(theta0));
    for k in -45..=45 {
        let th = theta0 + k as f64 * deg;
        let r = resid(th);
        if r < best.1 {
            best = (th, r);
        }
    }
    let (mut lo, mut hi) = (best.0 - deg, best.0 + deg);
    let phi = 0.5 * (5.0f64.sqrt() - 1.0);
    let (mut a, mut b) = (hi - phi * (hi - lo), lo + phi * (hi - lo));
    let (mut fa, mut fb) = (resid(a), resid(b));
    for _ in 0..40 {
        if fa < fb {
            hi = b;
            b = a;
            fb = fa;
            a = hi - phi * (hi - lo);
            fa = resid(a);
        } else {
            lo = a;
            a = b;
            fa = fb;
            b = lo + phi * (hi - lo);
            fb = resid(b);
        }
    }
    let theta = 0.5 * (lo + hi);
    let (dc, ds) = (theta.cos(), theta.sin());

    let t: Vec<f64> = (0..n)
        .map(|i| (s.x[i] - xc) * dc + (s.y[i] - yc) * ds)
        .collect();
    let smin = t.iter().cloned().fold(f64::MAX, f64::min);
    let smax = t.iter().cloned().fold(f64::MIN, f64::max);
    if smax - smin < 1e-6 {
        return None;
    }
    let (_, a, g) = fit_1d(cols, &t);
    let at = |sv: f64| [a[0] + g[0] * sv, a[1] + g[1] * sv, a[2] + g[2] * sv];
    Some(FillModel::Linear {
        p0: (xc + smin * dc, yc + smin * ds),
        p1: (xc + smax * dc, yc + smax * ds),
        c0: from_space(at(smin), space),
        c1: from_space(at(smax), space),
        interp: space,
        mids: Vec::new(),
    })
}

/// Principal direction of the colours, by power iteration on their covariance.
fn color_axis(cols: &[[f64; 3]]) -> Option<[f64; 3]> {
    let cbar = mean3(cols);
    let mut cov = [[0.0f64; 3]; 3];
    for c in cols {
        let d = [c[0] - cbar[0], c[1] - cbar[1], c[2] - cbar[2]];
        for (i, row) in cov.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v += d[i] * d[j];
            }
        }
    }
    let trace = cov[0][0] + cov[1][1] + cov[2][2];
    if trace <= 1e-12 {
        return None;
    }
    let mut v = [1.0 / 3.0f64.sqrt(); 3];
    for _ in 0..30 {
        let mut nv = [0.0; 3];
        for i in 0..3 {
            for j in 0..3 {
                nv[i] += cov[i][j] * v[j];
            }
        }
        let norm = (nv[0] * nv[0] + nv[1] * nv[1] + nv[2] * nv[2]).sqrt();
        if norm <= 1e-15 {
            return None;
        }
        v = [nv[0] / norm, nv[1] / norm, nv[2] / norm];
    }
    Some(v)
}

/// Radial gradient: centre by a weighted least-squares intersection of the gradient
/// lines, refined by pattern search on the exact 1-D residual; stops by least squares.
///
/// In a radial gradient the spatial gradient of any scalar function of colour points at
/// (or away from) the centre, so every interior pixel with a measurable gradient
/// contributes a line the centre must lie on. The centre that minimises the weighted
/// squared distance to all of those lines is a 2x2 linear solve. That estimate is then
/// polished by pattern search on the residual of `colour = a + g·|P - C|`, which is a
/// 1-D least squares per candidate centre.
///
/// The centre is confined to within one bounding-box width of the region. Any radial
/// gradient with its centre farther out than that is, across this region, a linear
/// gradient — and should be described as one.
fn fit_radial(s: &Samples, cols: &[[f64; 3]], space: Interp, w: usize) -> Option<FillModel> {
    let n = s.len();
    if n < MIN_GRADIENT_PIXELS {
        return None;
    }
    let axis = color_axis(cols)?;
    let f: Vec<f64> = cols
        .iter()
        .map(|c| c[0] * axis[0] + c[1] * axis[1] + c[2] * axis[2])
        .collect();

    // Weighted least squares for the point nearest all gradient lines.
    let (mut a00, mut a01, mut a11, mut b0, mut b1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let stride = (n / fit_cap()).max(1);
    for i in (0..n).step_by(stride) {
        let p = s.px[i];
        let x = p % w;
        if x == 0 || p < w {
            continue;
        }
        let (Some(l), Some(r), Some(u), Some(d)) = (
            s.sample_at(p - 1),
            s.sample_at(p + 1),
            s.sample_at(p - w),
            s.sample_at(p + w),
        ) else {
            continue;
        };
        let gx = 0.5 * (f[r] - f[l]);
        let gy = 0.5 * (f[d] - f[u]);
        let mag = (gx * gx + gy * gy).sqrt();
        if mag <= 1e-9 {
            continue;
        }
        // Perpendicular to the gradient: distance of C from the line through P along g.
        let (nx, ny) = (-gy / mag, gx / mag);
        let rhs = nx * s.x[i] + ny * s.y[i];
        a00 += mag * nx * nx;
        a01 += mag * nx * ny;
        a11 += mag * ny * ny;
        b0 += mag * nx * rhs;
        b1 += mag * ny * rhs;
    }
    let (xc, yc) = s.centroid();
    let det = a00 * a11 - a01 * a01;
    let mut c = if det > 1e-9 * (a00 + a11).powi(2) {
        ((a11 * b0 - a01 * b1) / det, (a00 * b1 - a01 * b0) / det)
    } else {
        (xc, yc)
    };

    let xmin = s.x.iter().cloned().fold(f64::MAX, f64::min);
    let xmax = s.x.iter().cloned().fold(f64::MIN, f64::max);
    let ymin = s.y.iter().cloned().fold(f64::MAX, f64::min);
    let ymax = s.y.iter().cloned().fold(f64::MIN, f64::max);
    let (bw, bh) = ((xmax - xmin).max(4.0), (ymax - ymin).max(4.0));
    let clamp = |p: (f64, f64)| {
        (
            p.0.clamp(xmin - bw, xmax + bw),
            p.1.clamp(ymin - bh, ymax + bh),
        )
    };
    c = clamp(c);

    let radii = |c: (f64, f64)| -> Vec<f64> {
        (0..n)
            .map(|i| ((s.x[i] - c.0).powi(2) + (s.y[i] - c.1).powi(2)).sqrt())
            .collect()
    };
    // The centre search evaluates its residual up to six hundred times; on a strided
    // subsample each evaluation is O(MAX_FIT_SAMPLES) instead of O(n). The final fit
    // below still uses every pixel.
    let cstride = (n / CENTRE_SEARCH_SAMPLES).max(1);
    let idx: Vec<usize> = (0..n).step_by(cstride).collect();
    let cols_sub: Vec<[f64; 3]> = idx.iter().map(|&i| cols[i]).collect();
    let mut rz = Resid1d::new(s, &idx, &cols_sub);

    let mut best = rz.radial(c);
    let mut step = 4.0;
    let mut evals = 0;
    const DIRS: [(f64, f64); 8] = [
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (1.0, 1.0),
        (1.0, -1.0),
        (-1.0, 1.0),
        (-1.0, -1.0),
    ];
    while step > 0.03 && evals < 600 {
        let mut improved = false;
        for (dx, dy) in DIRS {
            let cand = clamp((c.0 + dx * step, c.1 + dy * step));
            let r = rz.radial(cand);
            evals += 1;
            if r < best - 1e-12 {
                best = r;
                c = cand;
                improved = true;
                break;
            }
        }
        if !improved {
            step *= 0.5;
        }
    }

    let rho = radii(c);
    let r = rho.iter().cloned().fold(f64::MIN, f64::max);
    if r < 0.5 {
        return None;
    }
    let (_, a, g) = fit_1d(cols, &rho);
    Some(FillModel::Radial {
        c,
        r,
        c0: from_space(a, space),
        c1: from_space([a[0] + g[0] * r, a[1] + g[1] * r, a[2] + g[2] * r], space),
        interp: space,
        aspect: 1.0,
        angle: 0.0,
        mids: Vec::new(),
    })
}

/// Largest aspect ratio an elliptical gradient may take. Beyond this the gradient is,
/// across any region it could plausibly fill, a linear one.
const MAX_ASPECT: f64 = 8.0;

/// Elliptical radial gradient: the circular fit's centre, then a pattern search over
/// centre, orientation and aspect on the exact 1-D residual.
///
/// The centre search of [`fit_radial`] is a good start even when the truth is
/// elliptical - the weighted line intersection lands near the middle of the ellipse -
/// but its residual is left with the whole anisotropy. Four coordinates, `(cx, cy,
/// angle, ln aspect)`, are then searched together: with the aspect free the centre
/// often moves, so the two cannot be settled one after the other. Each evaluation is a
/// 1-D least squares on the strided subsample; the final stops use every pixel.
fn fit_radial_elliptic(
    s: &Samples,
    cols: &[[f64; 3]],
    space: Interp,
    circular: &FillModel,
) -> Option<FillModel> {
    let FillModel::Radial { c: c_start, .. } = *circular else {
        return None;
    };
    let n = s.len();
    if n < 2 * MIN_GRADIENT_PIXELS {
        return None;
    }
    let cstride = (n / CENTRE_SEARCH_SAMPLES).max(1);
    let idx: Vec<usize> = (0..n).step_by(cstride).collect();
    let cols_sub: Vec<[f64; 3]> = idx.iter().map(|&i| cols[i]).collect();
    let mut rz = Resid1d::new(s, &idx, &cols_sub);

    let xmin = s.x.iter().cloned().fold(f64::MAX, f64::min);
    let xmax = s.x.iter().cloned().fold(f64::MIN, f64::max);
    let ymin = s.y.iter().cloned().fold(f64::MAX, f64::min);
    let ymax = s.y.iter().cloned().fold(f64::MIN, f64::max);
    let (bw, bh) = ((xmax - xmin).max(4.0), (ymax - ymin).max(4.0));

    // State: centre, angle, ln(aspect). Radius is not searched: the residual of a
    // 1-D fit on unnormalised elliptical distance is what the search minimises, and
    // the radius falls out of the final fit as the largest distance seen.
    let mut st = [c_start.0, c_start.1, 0.0, 0.0];
    let clamp = |st: [f64; 4]| -> [f64; 4] {
        [
            st[0].clamp(xmin - bw, xmax + bw),
            st[1].clamp(ymin - bh, ymax + bh),
            st[2],
            st[3].clamp(0.0, MAX_ASPECT.ln()),
        ]
    };
    let resid = |rz: &mut Resid1d, st: [f64; 4]| -> f64 {
        let (sn, cs) = st[2].sin_cos();
        rz.elliptic((st[0], st[1]), sn, cs, st[3].exp())
    };

    // Seed the orientation: a coarse sweep at a moderate aspect is cheap and keeps the
    // pattern search out of the wrong local minimum of the angle.
    let mut best = resid(&mut rz, st);
    for deg in (0..180).step_by(15) {
        // Aspects 2 and 3. There is no need for the reciprocals: the angle sweep
        // already reaches a 1:2 ellipse as a 2:1 one turned ninety degrees. (A fourth
        // seed used to sit here written `0.5f64.ln() * -1.0`, which is ln 2 — the same
        // candidate as the second, scored twice for nothing.)
        for &la in &[2.0f64.ln(), 3.0f64.ln()] {
            let cand = clamp([st[0], st[1], (deg as f64).to_radians(), la]);
            let r = resid(&mut rz, cand);
            if r < best - 1e-12 {
                best = r;
                st = cand;
            }
        }
    }
    let scale = [1.0, 1.0, 10f64.to_radians(), 0.25];
    let mut step = 4.0;
    let mut evals = 0;
    while step > 0.03 && evals < 800 {
        let mut improved = false;
        'axes: for ax in 0..4 {
            for sign in [1.0, -1.0] {
                let mut cand = st;
                cand[ax] += sign * step * scale[ax];
                let cand = clamp(cand);
                let r = resid(&mut rz, cand);
                evals += 1;
                if r < best - 1e-12 {
                    best = r;
                    st = cand;
                    improved = true;
                    break 'axes;
                }
            }
        }
        if !improved {
            step *= 0.5;
        }
    }
    let aspect = st[3].exp();
    if aspect < 1.02 {
        return None; // the circle already has it
    }
    let (sn, cs) = st[2].sin_cos();
    let rho: Vec<f64> = (0..n)
        .map(|i| {
            let (dx, dy) = (s.x[i] - st[0], s.y[i] - st[1]);
            let u = dx * cs + dy * sn;
            let v = (-dx * sn + dy * cs) * aspect;
            (u * u + v * v).sqrt()
        })
        .collect();
    let r = rho.iter().cloned().fold(f64::MIN, f64::max);
    if r < 0.5 {
        return None;
    }
    let (_, a, g) = fit_1d(cols, &rho);
    Some(FillModel::Radial {
        c: (st[0], st[1]),
        r,
        c0: from_space(a, space),
        c1: from_space([a[0] + g[0] * r, a[1] + g[1] * r, a[2] + g[2] * r], space),
        interp: space,
        aspect,
        angle: st[2],
        mids: Vec::new(),
    })
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

/// chi² of a model against the samples: per channel in sRGB, the part of the residual
/// beyond the half-LSB quantisation dead zone, in units of `sigma`.
/// Residual of the best *two* flat colours for these samples, on the same scale as
/// [`chi2`].
///
/// One-dimensional k-means along the samples' dominant colour axis. This is not a fill the
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

/// A two-flat-colour fit of the samples, split along their dominant colour axis. This is not a model the
/// emitter can write, and it is not meant to be: it exists only to answer a question about
/// the alternative, which is whether the variation in a region is a ramp or a step.
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
    // Mean, then the dominant axis by one round of power iteration on the covariance.
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

fn chi2(model: &FillModel, s: &Samples, sigma: f64) -> f64 {
    let inv = 1.0 / (sigma * sigma);
    let n = s.len();
    let stride = (n / fit_cap()).max(1);
    let mut sum = 0.0;
    let mut used = 0usize;
    let mut i = 0;
    while i < n {
        let p = model.color_at(s.x[i], s.y[i]);
        for (obs, pred) in s.srgb[i].iter().zip(p.iter()) {
            let d = ((obs - pred).abs() as f64 - QUANT_HALF_STEP).max(0.0);
            sum += d * d;
        }
        used += 1;
        i += stride;
    }
    if used == 0 {
        return 0.0;
    }
    // Scaled back to the full pixel count so the MDL cost stays comparable with
    // parameter costs and with fits of other regions.
    sum * inv * (n as f64 / used as f64)
}

/// Largest per-channel sRGB range the model spans over the samples.
/// Fraction of the samples at which `model` predicts a colour at least a quarter of its
/// own contrast away from the region's mean colour — how much of the region the ramp
/// actually shades. Half for a linear ramp; near zero for a ramp that is flat except at
/// one end.
fn ramp_support(model: &FillModel, s: &Samples, mean: [f32; 3], contrast: f64) -> f64 {
    let thr = (0.25 * contrast) as f32;
    let thr2 = thr * thr;
    let stride = (s.len() / fit_cap()).max(1);
    let (mut n, mut k) = (0usize, 0usize);
    for i in (0..s.len()).step_by(stride) {
        let p = model.color_at(s.x[i], s.y[i]);
        let d = [p[0] - mean[0], p[1] - mean[1], p[2] - mean[2]];
        n += 1;
        if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > thr2 {
            k += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        k as f64 / n as f64
    }
}

fn visible_contrast(model: &FillModel, s: &Samples) -> f64 {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let stride = (s.len() / fit_cap()).max(1);
    for i in (0..s.len()).step_by(stride) {
        let p = model.color_at(s.x[i], s.y[i]);
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (0..3).map(|k| (hi[k] - lo[k]) as f64).fold(0.0, f64::max)
}

/// Every admissible candidate for the samples, flat first.
fn fit_samples(s: &Samples, w: usize, strict: bool, sigma: f64, lambda: f64) -> Vec<FillFit> {
    let sigma = if sigma > 0.0 { sigma } else { 0.5 / 255.0 };
    let score = |model: FillModel| {
        let c2 = chi2(&model, s, sigma);
        let params = model.params();
        FillFit {
            model,
            chi2: c2,
            params,
            cost: 0.5 * c2 + lambda * params,
        }
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
    // the MDL below would accept them by a wide margin. `INKVEC_MIN_CONTRAST` scales
    // the floor so the full set can price it.
    let scale = std::env::var("INKVEC_MIN_CONTRAST")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(1.0);
    let min_contrast = (3.0 * sigma).max(MIN_VISIBLE_CONTRAST) * scale;
    for space in INTERPS {
        let cols = s.colors(space);
        let t_r = inkvec_core::clock::Instant::now();
        let radial = fit_radial(s, &cols, space, w);
        tick(&FIT_NS_RADIAL, t_r);
        let t_e = inkvec_core::clock::Instant::now();
        let elliptic = radial
            .as_ref()
            .and_then(|r| fit_radial_elliptic(s, &cols, space, r));
        tick(&FIT_NS_ELLIPTIC, t_e);
        let t_l = inkvec_core::clock::Instant::now();
        let linear = fit_linear(s, &cols, space);
        tick(&FIT_NS_LINEAR, t_l);
        for cand in [linear, radial, elliptic].into_iter().flatten() {
            let contrast = visible_contrast(&cand, s);
            let support = ramp_support(&cand, s, flat_c, contrast);
            if contrast < min_contrast || support < MIN_RAMP_SUPPORT {
                // Which gate refused a candidate is otherwise invisible: a region that
                // ends up "cands 1" looks identical whether no ramp was ever tried or
                // every ramp was thrown away here. `INKVEC_EVDBG=1`.
                if std::env::var_os("INKVEC_EVDBG").is_some() || debug::verbose() {
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
                continue;
            }
            let multi = fit_mid_stops(&cand, s, &cols, space);
            out.push(score(cand));
            for m in multi {
                let c = visible_contrast(&m, s);
                if c >= min_contrast && ramp_support(&m, s, flat_c, c) >= MIN_RAMP_SUPPORT {
                    out.push(score(m));
                }
            }
        }
    }
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
        let margin = std::env::var("INKVEC_BIMODAL")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(BIMODAL_MARGIN);
        debug::candidates(s.len(), &out, two, best_grad);
        if two.is_finite() && best_grad.is_finite() && two < margin * best_grad {
            out.truncate(n_flat_only);
        }
    }
    out
}

/// The cheapest candidate; on a tie the earlier (simpler) one.
pub(crate) fn select(mut candidates: Vec<FillFit>) -> FillFit {
    let mut best = 0;
    for (i, c) in candidates.iter().enumerate() {
        if c.cost < candidates[best].cost {
            best = i;
        }
    }
    candidates.swap_remove(best)
}

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
    if std::env::var_os("INKVEC_EVDBG").is_some() {
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
mod evidence;
pub(crate) mod regions;
pub(crate) mod stops;
pub mod svg;

pub use bands::{merge_gradient_bands, merge_gradient_bands_with_ink};
pub(crate) use budget::*;
pub use carve::{carve_residual_features, carve_residual_features_with_detail_noise};
pub(crate) use evidence::*;
pub(crate) use stops::fit_mid_stops;
pub use svg::{fade_to_svg, fill_to_svg};
