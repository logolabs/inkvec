//! Content units: the fit's pixel-denominated tolerances restated per unit of detail.
//!
//! The fitter's knobs -- boundary sigma, `--precision`, lambda -- are in pixels, and a
//! raster that carries the same drawing on more pixels per unit of detail (an upscale, a
//! blur, a simple mark rendered large) would otherwise be read as a more detailed drawing
//! and bought segments to match. `--content-units` (off by default) rescales them by a
//! measured pixels-per-detail factor `s`; with the flag off `s` is exactly 1 and every
//! function here is the identity.
//!
//! Part of the fit stage: [`fit_config`] builds the `FitConfig` every pipeline in
//! [`crate::pipeline`] fits with, and [`in_content_units`] rescales each boundary's
//! sigmas before the fit. [`REF_EXTENT`] is also read by the intake in the crate root.

use crate::args::Args;
use inkvec_fit::FitConfig;

/// The intake size every pixel-denominated constant in the tracer was tuned at.
///
/// sigma is ~0.05 px, precision is 0.1 px and min_area is 2 px^2, and all three
/// were set against a 128 px corpus. Handed a 512 px raster of the *same* logo the
/// tracer therefore saw four times the boundary points, each carrying four times
/// the pixel residual for the same relative fit, against a lambda that had grown
/// by ln 4, about 1.4 nats -- and bought segments accordingly. Measured on real brand logos:
/// 3.54x the artist's parameters at 128, 6.02x at 256, 11.62x at 512, for content
/// that had not changed. A brand mark has one complexity, and a vectoriser should
/// return it whatever resolution the export happened to be.
///
/// So the three are stated per unit of content. `REF_EXTENT` records the intake size they
/// were tuned at; [`content_scale`] measures how many pixels the raster in hand spends per
/// unit of that detail, rather than assuming it from the extent.
pub(crate) const REF_EXTENT: f64 = 128.0;

/// Pixels per unit of content for this image, never below one.
///
/// **Measured, not inferred from the pixel count.** `intake_scale` reports the width of an
/// edge: a natively rendered raster puts one pixel on a boundary and reads 1.00 whatever
/// its size, while an upscaled, blurred or photographed one spreads that boundary over
/// several pixels and reads how many. That ratio is exactly "pixels per unit of detail",
/// which is the quantity this function is supposed to return.
///
/// It used to return `extent / REF_EXTENT`, which asks a different question -- how big is
/// this? -- and gets the right answer only when size and detail happen to move together.
/// Measured on the corpus tiers, the two disagree completely: a natively rendered icon
/// reads an edge width of 1.00 at 128, 256, 512 and 1024 alike, where `extent / REF_EXTENT`
/// claims 1x, 2x, 4x and 8x. The old scale therefore loosened every tolerance eightfold on
/// a 1024 px render that had lost no detail at all, and the failure that kept this flag
/// opt-in -- "5-px rotated squares were accepted as circles and a 2-px ring came out
/// broken" at 512, recorded in this comment before it was rewritten -- is that
/// over-correction, described but never traced to its cause. Its own words were that the
/// scale "claims the edges are four times less certain than they are". They were.
///
/// The measured scale cannot make that mistake: content that is genuinely resolved reads
/// 1.0 and is left exactly as it was found.
pub(crate) fn content_scale(img: &inkvec_trace::Rgba, args: &Args) -> f64 {
    if !args.content_units {
        return 1.0;
    }
    let rgb = img.composited([1.0, 1.0, 1.0]);
    // Two measurements, asking different questions, and the budget wants both.
    //
    // `intake_scale` reports the width of an edge: it catches blur, resampling and upscaling,
    // and reads 1.00 on anything with honest one-pixel boundaries.
    //
    // `oversample_factor` asks whether the pixels can be thrown away and put back. That is a
    // statement about the *content*, not the optics, and it is the one that sees a simple
    // drawing rendered larger than it needs: measured on the corpus tiers, a natively
    // rendered icon reads 1, 1, 2 and 4 at 128, 256, 512 and 1024 while its edges stay
    // exactly 1.00 px wide throughout. Nothing was blurred; there is simply no detail at
    // that scale to spend coordinates on.
    //
    // An upscaled logo trips the first, a simple drawing at a large size trips the second,
    // and a genuinely detailed raster trips neither and is left alone -- which is the whole
    // point, and what `extent / REF_EXTENT` could never do, because it cannot tell a
    // detailed 1024 px illustration from a blown-up 128 px mark.
    let edge = inkvec_trace::coverage::intake_scale(&rgb, img.width, img.height);
    let redundancy = inkvec_trace::coverage::oversample_factor(&rgb, img.width, img.height) as f64;
    edge.max(redundancy).max(1.0)
}

/// The fit configuration in content units.
///
/// `FitConfig::from_precision` prices a coordinate at `ln(extent / precision)` nats, the
/// information in a number confined to the canvas and written to `precision` px. Asking
/// for `precision * s` instead makes that `ln(extent / (s * precision))`: the nats of a
/// coordinate at the content's own resolution. The further factor of `s` on lambda is the
/// point count: the data term is a sum over boundary samples, there are `s` times as many
/// of them per unit of content, and pricing a parameter in the same currency means scaling
/// its cost by the same `s`. Together with sigma scaled by `s` at the fit
/// ([`in_content_units`]), the optimum is the one a raster at one pixel per unit of detail
/// would reach -- from better points. With `--content-units` off, `s = 1` and this is
/// `FitConfig::from_precision(extent, precision, tau)` unchanged.
pub(crate) fn fit_config(img: &inkvec_trace::Rgba, args: &Args) -> FitConfig {
    fit_config_sized(img, args, img.width.max(img.height))
}

/// [`fit_config`] with the extent given instead of read off `img`: what a raster traced on a
/// canvas larger than the one it arrived on uses, so the price of a coordinate is the one
/// the original raster's size sets, not the larger canvas's.
pub(crate) fn fit_config_sized(img: &inkvec_trace::Rgba, args: &Args, extent: usize) -> FitConfig {
    let extent = extent as f64;
    let s = content_scale(img, args);
    let mut cfg = FitConfig::from_precision(extent, args.precision * s, args.tau);
    cfg.lambda *= s;
    cfg
}

/// A polyline's sigma restated in content units: every per-point sigma (px) multiplied by
/// `s`, the points themselves untouched. The identity (a clone) at `s == 1`.
pub(crate) fn in_content_units(poly: &inkvec_core::Polyline, s: f64) -> inkvec_core::Polyline {
    if s == 1.0 {
        return poly.clone();
    }
    let mut p = poly.clone();
    for sg in p.sigma.iter_mut() {
        *sg *= s;
    }
    p
}
