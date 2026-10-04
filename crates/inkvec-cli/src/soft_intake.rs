//! Soft intake: reduce a resampled or blurred raster to the detail it carries.
//!
//! # The problem
//!
//! A raster upscaled from a smaller one, or blurred, spreads every edge over several pixels.
//! The tracer was built for one-pixel edges and meets the wide ramp three ways, all measured
//! on the degraded benchmark of the `engine/soft-input` branch: the ramp's middle colours
//! become an ink of their own (a grey halo round every shape), the exact-coverage boundary
//! solve ripples points across a ramp no sharp edge can reproduce (lumpy outlines, lucide at
//! 30x the artist's parameters under a blur), and the fitter prices every surplus pixel as
//! evidence (4x bicubic input at 3.2x the artist's parameters). On the r2-inputs stress set
//! a 4x bicubic upscale traced at dE00 0.839 and 8.1x the artist's parameters, where its
//! 128 px source traces at 0.375 and 1.4x.
//!
//! # What this does, in order ([`verdict`], then [`reduce`])
//!
//! 1. Measure the raster composited over white ([`softness::ramp_evidence`]).
//! 2. **Gate**: at least [`softness::MIN_EDGES`] measured edges, and at least
//!    [`SOFT_GATE`] of them wider than any native render can make one. A native render reads
//!    at most 0.18 over the corpus; every resampled or blurred raster measured reads 1.00.
//! 3. **Sharp-edge veto** ([`SHARP_VETO`]): not when the raster's strong edges are mostly
//!    native-sharp. That is a sharp drawing with a soft element -- an outer glow or a drop
//!    shadow -- not a soft raster, and it must be traced at the size it arrived.
//! 4. **Factor**: the upscale's own period when [`softness::grid_contrast`] shows one near
//!    what the edge width implies, else the width's own integer factor
//!    ([`factor_for_width`]); an integer, because the reduction has to land back on the grid
//!    the upscale came from (4x bicubic input reduced by 3 scored 0.358 against 0.256 at 4).
//! 5. **Upscales of about 2x are kept** ([`MIN_UPSCALE`]): reducing them did not pay.
//! 6. **Guards**: the thinnest strokes and gaps (their 10th percentile) keep at least
//!    [`MIN_FEATURE_PX`] pixels and the short side at least [`MIN_SIDE`]; within that, the
//!    factor is the largest divisor of the measured upscale ([`capped_factor`]), so the
//!    reduction still lands on the source's grid; and shaded artwork
//!    ([`softness::shading_share`] at least [`SHADED_SHARE`]) is only reduced by
//!    [`SHADED_MIN_FACTOR`] or more.
//! 7. **Reduce** by the factor with the exact area average (`coverage::downsample_to`), back
//!    onto the 8-bit lattice. The intake in `lib.rs` writes the SVG at the arrival size,
//!    axis by axis (`post::stretch_axes`), since each side is rounded on its own.
//!
//! # Literature
//!
//! The degradation model this answers is the one practical blind super-resolution trains
//! for -- blur, bilinear/bicubic/nearest down- and up-sampling, noise, JPEG (Zhang et al.,
//! BSRGAN, ICCV 2021, arXiv 2103.14006) -- but where super-resolution restores the detail an
//! upscale never had, this only takes away the surplus pixels it added, which needs no
//! model. Kernel estimation (Bell-Kligler, Shocher, Irani, "Blind Super-Resolution Kernel
//! Estimation using an Internal-GAN", NeurIPS 2019, arXiv 1909.06581) answers the same
//! "which kernel, which factor" question for photographs and was judged heavier than vector
//! art needs (r2-inputs report, 4.1). Not from the literature: the decision rules above,
//! each set from the degraded benchmarks named at its constant, because the published
//! resampling detectors estimate a factor but say nothing about whether tracing the reduced
//! raster is better. Ported from `engine/soft-input` (2026-09-25) via the r2-inputs research
//! port; steps 3 and 5 are new here.

use inkvec_trace::softness::{self, RampEvidence};

/// Least share of measured edges wider than native for the raster to count as soft.
pub(crate) const SOFT_GATE: f64 = softness::SOFT_FRACTION_GATE;

/// Most share of native-sharp strong edges a soft raster may show
/// ([`RampEvidence::sharp_fraction`]). Measured on the r2-inputs stress set (28 sources per
/// group): every glow and shadow image reads 0.80 or more, every bicubic 4x, bicubic 2x and
/// Lanczos 3x upscale 0.20 or less, every clean render 1.00. Without it, 9 of the 28 glow
/// images passed the soft gate (the glow's own ramps are what the gate measures) and were
/// reduced 7-8x, dE00 0.690 -> 1.013 on the group.
pub(crate) const SHARP_VETO: f64 = 0.5;

/// Least box-equivalent edge width that is worth a reduction at all.
const MIN_WIDTH: f64 = 1.6;

/// `factor = round(SLOPE · width − OFFSET)`: the line through the measured upscales
/// (bilinear 2x at 2.1, bicubic 3x at 2.5, bicubic 4x at 3.3, bicubic 8x at 6.1), shifted
/// down by 0.2 so a bilinear 2x, whose widths spread to 2.4, is never read as 3x -- reducing
/// it by 3 scored worse than not reducing it at all (dE00 0.195 against 0.180).
const FACTOR_SLOPE: f64 = 1.4;
const FACTOR_OFFSET: f64 = 0.756;
/// A repeat period whose [`softness::grid_contrast`] reaches this is taken as the upscale's
/// own grid, when it lies within [`GRID_REACH`] of what the edge width implies.
const GRID_CONTRAST: f64 = 0.5;
const GRID_REACH: f64 = 1.2;
/// Periods tested for a grid. 2 is absent: a symmetric kernel shows no phase at 2x.
const GRID_PERIODS: [usize; 5] = [3, 4, 5, 6, 8];
/// Never reduce by more than this, however wide the edges read.
const MAX_FACTOR: usize = 8;

/// Least upscale, as measured (the factor before the guards cap it), that is reduced.
///
/// A raster about twice the size of its detail traces better as it is. On the r2-inputs
/// `down2up` group (a 256 px render, bicubic 2x to 512) every one of the 28 measured as a 2x
/// upscale, and reducing them by 2 made the group worse, dE00 0.248 -> 0.295 (10 better, 14
/// worse): the five wordmarks lost most (betterimpact 0.448 -> 0.930), but so did icons with
/// nothing thin in them (lucide omega 0.029 -> 0.124, its strokes 42 px apart), so a feature
/// guard alone does not separate them. The perfect inverse -- tracing the 256 px source
/// itself -- gains only 12 %, and the box-filtered reduction of a bicubic upscale is a blurred
/// copy of that source, not the source. Lanczos 3x inputs measured as 2x told the same story
/// (openmoji 1F195 0.093 -> 0.222, simple-icons fluxer 0.125 -> 0.208). A larger upscale
/// still capped to 2 by its thin features does pay (4x bicubic wordmarks: rent 1.654 ->
/// 0.915, shinhancard 0.549 -> 0.269), so the test is on the measured upscale, not on the
/// factor applied.
const MIN_UPSCALE: usize = 3;

/// After the reduction the thinnest strokes and gaps (their 10th percentile) keep at least
/// this many pixels.
///
/// Set from the degraded benchmark's reductions ranked by this quantity: below 2.5 every
/// wordmark lost; between 2.5 and 3.0 sat a 2x wordmark whose reduction cost 1.51 dE00
/// (letters 20 px tall trace worse at their own size than upscaled), a noto emoji (−0.06)
/// and one icon that gained 0.25 at /3 (it still reduces, by 2). Above 3.0 the icons gain.
pub(crate) const MIN_FEATURE_PX: f64 = 3.0;
/// Never trace a reduction smaller than this on its short side.
const MIN_SIDE: usize = 16;
/// Shaded artwork -- [`softness::shading_share`] at or above this -- is reduced only by
/// [`SHADED_MIN_FACTOR`] or more. Measured on the degraded benchmark: noto emoji, the one
/// shaded family, came out worse on 5 of 6 at 3x Lanczos and on all 6 by DISTS at 2x
/// bilinear when reduced, while at 4x bicubic the reduction still paid on average and on
/// the shaded twemoji and openmoji. Flat families read 0.000 here.
const SHADED_SHARE: f64 = 0.008;
const SHADED_MIN_FACTOR: usize = 4;

/// The integer reduction an edge width implies, `round(1.4·width − 0.756)` in `[1, 8]`.
pub(crate) fn factor_for_width(width: f64) -> usize {
    (FACTOR_SLOPE * width - FACTOR_OFFSET)
        .round()
        .clamp(1.0, MAX_FACTOR as f64) as usize
}

/// The reduction: the upscale's own grid -- the period in [`GRID_PERIODS`] within
/// [`GRID_REACH`] of the implied factor whose [`softness::grid_contrast`] is highest, if it
/// reaches [`GRID_CONTRAST`] -- else [`factor_for_width`]. Landing on the source grid matters
/// more than anything else here.
fn factor_for(rgb: &[[f32; 3]], w: usize, h: usize, width: f64) -> usize {
    let implied = FACTOR_SLOPE * width - FACTOR_OFFSET;
    GRID_PERIODS
        .iter()
        .filter(|&&k| (k as f64 - implied).abs() <= GRID_REACH)
        .map(|&k| (k, softness::grid_contrast(rgb, w, h, k)))
        .filter(|&(_, c)| c >= GRID_CONTRAST)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(k, _)| k)
        .unwrap_or_else(|| factor_for_width(width))
}

/// The largest divisor of the measured upscale `wanted` that is at most `cap`, the most the
/// guards allow: the reduction must land on a grid commensurate with the source's, so each
/// source pixel becomes a whole number of traced pixels.
///
/// This is the module's integer-factor rule carried over to a capped factor. A 3x upscale
/// capped to 2 by its thin features lands half-way between source pixels: on the r2-inputs
/// `up3lanw` group (Lanczos 3x), the three wordmarks reduced that way all traced worse
/// (jurlique dE00 0.999 -> 1.399, rent 0.646 -> 0.950, shinhancard 0.288 -> 0.359), and
/// keeping them as they arrived took the group from -6.8 % to -13.4 % with none worse. A 4x
/// capped to 3 is held at 2 for the same reason; the one input where 3 had done better
/// (dribbble, 2.356 -> 1.530 at /3) still gains at /2 (2.216). O(wanted).
fn capped_factor(wanted: usize, cap: usize) -> usize {
    let mut f = wanted.min(cap);
    while f > 1 && (wanted / f) * f != wanted {
        f -= 1;
    }
    f
}

/// The decision, with the evidence it was made on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Verdict {
    pub(crate) evidence: RampEvidence,
    /// The reduction to apply; 1 means none.
    pub(crate) factor: usize,
    /// Why nothing was done, when nothing was.
    pub(crate) reason: &'static str,
}

/// Decide whether, and by how much, to reduce this raster before tracing: the steps 1-6 of
/// the module documentation, in that order, each ending the decision with its reason when
/// it says no. O(pixels) for the evidence, plus one O(pixels) pass per candidate grid
/// period and one for the shading share when they are reached.
pub(crate) fn verdict(img: &inkvec_trace::Rgba) -> Verdict {
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let (w, h) = (img.width, img.height);
    let evidence = softness::ramp_evidence(&rgb, w, h);
    let none = |reason| Verdict {
        evidence,
        factor: 1,
        reason,
    };
    if evidence.edges < softness::MIN_EDGES {
        return none("too few strong edges");
    }
    if evidence.soft_fraction < SOFT_GATE {
        return none("native edges");
    }
    if evidence.sharp_fraction >= SHARP_VETO {
        return none("sharp edges beside the soft ones (a glow or a shadow)");
    }
    if evidence.width < MIN_WIDTH {
        return none("edges barely soft");
    }
    let wanted = factor_for(&rgb, w, h, evidence.width);
    if wanted < MIN_UPSCALE {
        return none("about a 2x upscale");
    }
    let room = (evidence.feature_p10 / MIN_FEATURE_PX).floor().max(1.0) as usize;
    let by_side = (w.min(h) / MIN_SIDE).max(1);
    let factor = capped_factor(wanted, room.min(by_side));
    if factor < 2 {
        return none("thin features");
    }
    if factor < SHADED_MIN_FACTOR && softness::shading_share(&rgb, w, h, 3 * factor) >= SHADED_SHARE
    {
        return none("shaded artwork");
    }
    Verdict {
        evidence,
        factor,
        reason: "",
    }
}

/// Reduce `img` when [`verdict`] says so: the raster to trace and whether it was reduced.
///
/// The reduction is `round(side / factor)` on each side (at least 8 px) by the exact area
/// average, then every channel is put back on the 8-bit lattice every decoded raster arrives
/// on, `round(255·v + 10⁻⁴) / 255`. The average leaves a blurred raster's flat regions a few
/// thousandths of a level off flat, and every stage that asks whether a fill is flat was
/// tuned on decoded input, where flat is exact: on a blurred lucide icon the unquantised
/// reduction traced at dE00 0.250, the same reduction rounded to 8 bits at 0.139. The
/// `10⁻⁴` makes an exact half level (a 2x2 average of 8-bit values often is one) round up,
/// as an image library writing the reduction out would, where f32 would land either side.
pub(crate) fn reduce(img: inkvec_trace::Rgba, quiet: bool) -> (inkvec_trace::Rgba, bool) {
    let v = verdict(&img);
    let e = v.evidence;
    if v.factor < 2 {
        crate::diag::stage(
            quiet || e.soft_fraction < SOFT_GATE || e.edges < softness::MIN_EDGES,
            || {
                format!(
                "  soft intake   edges {:.2} px wide on {:.0}% of {}, {:.0}% of strong edges sharp, thinnest features {:.1} px, kept as is: {}",
                e.width,
                e.soft_fraction * 100.0,
                e.edges,
                e.sharp_fraction * 100.0,
                e.feature_p10,
                v.reason
            )
            },
        );
        return (img, false);
    }
    let k = v.factor as f64;
    let (nw, nh) = (
        ((img.width as f64) / k).round().max(8.0) as usize,
        ((img.height as f64) / k).round().max(8.0) as usize,
    );
    crate::diag::stage(quiet, || {
        format!(
            "  soft intake   edges {:.2} px wide on {:.0}% of {}, thinnest features {:.1} px: tracing at {}x{} (/{}), emitting at {}x{}",
            e.width,
            e.soft_fraction * 100.0,
            e.edges,
            e.feature_p10,
            nw,
            nh,
            v.factor,
            img.width,
            img.height
        )
    });
    let mut out = inkvec_trace::coverage::downsample_to(&img, nw, nh);
    for v in out.data.iter_mut() {
        *v = ((*v as f64 * 255.0 + 1e-4).round() / 255.0) as f32;
    }
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_factor_lands_on_the_measured_upscales() {
        // Widths read off the degraded corpus (medians): bilinear 2x, bicubic 2x,
        // bicubic 3x, Lanczos 4x, bicubic 4x, bicubic 8x.
        assert_eq!(factor_for_width(2.14), 2);
        assert_eq!(factor_for_width(2.3), 2);
        assert_eq!(factor_for_width(1.85), 2);
        assert_eq!(factor_for_width(2.54), 3);
        assert_eq!(factor_for_width(3.11), 4);
        assert_eq!(factor_for_width(3.27), 4);
        assert_eq!(factor_for_width(6.12), 8);
        assert_eq!(factor_for_width(0.0), 1);
        assert_eq!(factor_for_width(40.0), MAX_FACTOR);
    }

    #[test]
    fn a_capped_factor_divides_the_upscale() {
        assert_eq!(capped_factor(4, 8), 4);
        assert_eq!(capped_factor(4, 3), 2);
        assert_eq!(capped_factor(3, 2), 1);
        assert_eq!(capped_factor(8, 7), 4);
        assert_eq!(capped_factor(6, 5), 3);
        assert_eq!(capped_factor(5, 4), 1);
        assert_eq!(capped_factor(3, 0), 0);
    }

    #[test]
    fn a_flat_raster_is_left_alone() {
        let img = inkvec_trace::Rgba {
            width: 64,
            height: 64,
            data: vec![1.0; 64 * 64 * 4],
        };
        let (out, did) = reduce(img, true);
        assert!(!did);
        assert_eq!((out.width, out.height), (64, 64));
    }

    /// A white disc on black, `size²`, `ss`x box-supersampled, as straight RGBA.
    fn disc(size: usize, r: f64, ss: usize) -> inkvec_trace::Rgba {
        let c = size as f64 / 2.0;
        let mut data = Vec::with_capacity(size * size * 4);
        for y in 0..size {
            for x in 0..size {
                let mut cov = 0.0;
                for sy in 0..ss {
                    for sx in 0..ss {
                        let px = x as f64 + (sx as f64 + 0.5) / ss as f64 - c;
                        let py = y as f64 + (sy as f64 + 0.5) / ss as f64 - c;
                        if px * px + py * py < r * r {
                            cov += 1.0;
                        }
                    }
                }
                let v = ((cov / (ss * ss) as f64) * 255.0).round() as f32 / 255.0;
                data.extend_from_slice(&[v, v, v, 1.0]);
            }
        }
        inkvec_trace::Rgba {
            width: size,
            height: size,
            data,
        }
    }

    /// Bilinear upscale by an integer factor, rounded to 8 bits.
    fn bilinear_up(src: &inkvec_trace::Rgba, k: usize) -> inkvec_trace::Rgba {
        let n = src.width;
        let m = n * k;
        let mut data = Vec::with_capacity(m * m * 4);
        for y in 0..m {
            for x in 0..m {
                let fx = ((x as f64 + 0.5) / k as f64 - 0.5).clamp(0.0, n as f64 - 1.0);
                let fy = ((y as f64 + 0.5) / k as f64 - 0.5).clamp(0.0, n as f64 - 1.0);
                let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(n - 1), (y0 + 1).min(n - 1));
                let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
                let p = |xx: usize, yy: usize| src.data[(yy * n + xx) * 4] as f64;
                let a = p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx;
                let b = p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx;
                let v = ((a * (1.0 - ty) + b * ty) * 255.0).round() as f32 / 255.0;
                data.extend_from_slice(&[v, v, v, 1.0]);
            }
        }
        inkvec_trace::Rgba {
            width: m,
            height: m,
            data,
        }
    }

    /// A native render is kept; its bilinear 4x upscale is reduced by about 4 and lands on
    /// the 8-bit lattice; its bilinear 2x upscale is kept as about a 2x upscale.
    #[test]
    fn an_upscale_is_reduced_and_a_native_render_is_not() {
        let native = disc(128, 40.0, 8);
        assert_eq!(verdict(&native).reason, "native edges");
        let up4 = bilinear_up(&native, 4);
        let v = verdict(&up4);
        assert!((3..=5).contains(&v.factor), "{v:?}");
        let (out, did) = reduce(up4, true);
        assert!(did);
        assert!(out.width < 200 && out.width == out.height);
        assert!(out
            .data
            .iter()
            .all(|&c| ((c as f64 * 255.0).round() / 255.0) as f32 == c));
        let up2 = bilinear_up(&native, 2);
        assert_eq!(verdict(&up2).factor, 1, "{:?}", verdict(&up2));
    }
}
