//! `inkvec` — raster to vector.
//!
//! This crate is the driver: it reads an image, chooses which of the three tracers to run,
//! and writes the document. The decisions all live elsewhere.
//!
//! ```text
//!   image ─▶ intake ─▶ trace ─▶ fit ─▶ repair ─▶ emit ─▶ post ─▶ SVG
//! ```
//!
//! * **intake** — decode, optionally upscale or unblock, and matte away transparency
//!   (`alpha`). What the tracer sees is always opaque RGB.
//! * **trace** — `inkvec_trace`: palette, labels, the planar map, and a fill model per
//!   face. One of three modes: colour (`run_color`, the default), bilevel
//!   (`run_bilevel`) or centreline strokes (`run_strokes`).
//! * **fit** — `inkvec_fit`: one dynamic program per boundary, choosing lines, cubics and
//!   arcs against a description length. `rings` then settles how the results nest.
//! * **emit** — `emit`: fitted geometry to SVG text.
//! * **post** — `post`: viewBox, margin, background knock-out, minification.
//!
//! The objective is the same at every stage — squared residual against the image plus
//! `lambda` per parameter written — so a stage only keeps what it can pay for.
//!
//! Where things live, driver first:
//!
//! * this file: [`cli_main`], [`intake`] (unblock, intake normalisation, `--max-dim`, knob
//!   pricing) and [`trace_prepared`] (the restorer and SR pre-passes, the alpha matte, then
//!   one of the three pipelines);
//! * `args` (the command line), `units` (content units), `alpha` (transparency),
//!   `pipeline` (the three pipelines), `fast` (fast mode's fit);
//! * writing: `emit` (colour and bilevel documents), `mono` (`--monochrome`), `harmonize`
//!   (repeated shapes), `seams` (underlap), `editable` (`--editability`), `uncertainty`
//!   (`--uncertainty` bands), `post` (output options);
//! * leaves every stage may use: `faces` (ring types), `rings` (ring geometry and
//!   repair), `pathdata` (path text), `primitive` (circle, ellipse, rectangle), `naming`
//!   (element ids), `diag` (standard error).
//!
//! Coordinates everywhere below the driver are pixels of the traced raster with pixel
//! centres at integers, so the canvas spans `-0.5..w-0.5`; colours are sRGB in 0..1 unless
//! a comment says OKLab.

// As in `inkvec_trace`: the pixel loops here address several parallel arrays by one index,
// and an enumerate over one of them reads worse than the index does.
#![allow(clippy::needless_range_loop)]
// As in `inkvec_trace`: doc comments point at the private helper doing each step.
#![allow(rustdoc::private_intra_doc_links)]

mod alpha;
mod args;
mod border;
mod damage;
mod diag;
mod editable;
mod emit;
mod faces;
mod fast;
mod harmonize;
mod mirror_fit;
mod mono;
mod naming;
mod pathdata;
pub mod pipeline;
mod post;
mod primitive;
mod ribbons;
mod rings;
mod seams;
mod select;
mod soft_intake;
mod strokes;
mod uncertainty;
mod units;

#[cfg(test)]
mod lib_tests;

use alpha::pixel_grid;
use args::{parse_args, usage};
pub use args::{parse_color_groups, Args, TraceMode};
pub use fast::fast_ignored;
pub use inkvec_trace::regroup;
pub use pipeline::Stop;
use pipeline::{run_bilevel, run_color};
#[cfg(feature = "research-guidance")]
pub use pipeline::{trace_color_from_labels_guided, trace_color_guided};
pub use post::post_process;
use post::retarget;
pub use std::process::ExitCode;
use strokes::run_strokes;
use units::{fit_config, fit_config_sized, REF_EXTENT};

use inkvec_trace::load_image_capped;
use std::path::Path;

/// The command-line entry point.
pub fn cli_main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
    };
    run_cli(&args)
}

/// The command-line entry point with explicit argument iterator.
pub fn cli_main_from(it: impl Iterator<Item = String>) -> ExitCode {
    let args = match args::parse_args_from(it) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
    };
    run_cli(&args)
}

fn run_cli(args: &Args) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        // The one place a stop becomes an exit code; the library below only ever returns it.
        Err(e) => match e.downcast_ref::<Stop>() {
            Some(Stop::MapDumped(_)) => ExitCode::SUCCESS,
            Some(Stop::FlatInput) => {
                eprintln!("error: {e}");
                ExitCode::from(2)
            }
            Some(Stop::MapDumpFailed(_)) => {
                eprintln!("error: {e}");
                ExitCode::from(3)
            }
            None => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

/// Below this the intake is already at one pixel per unit of detail and is left
/// alone. Every image in the corpus reads exactly 1.00.
const INTAKE_SCALE_FLOOR: f64 = 1.5;
/// Never throw away more than this much, however oversampled the estimate says
/// the input is. A wrong estimate should cost detail slowly, not all at once.
const INTAKE_SCALE_CAP: f64 = 8.0;

/// `--intake-scale`: resample an oversampled intake down to one pixel per unit of real
/// detail.
///
/// This is the whole answer to "make the thresholds work at any resolution", and
/// it is one change rather than a scale factor threaded through every constant.
/// The thresholds are in pixels and were tuned where one pixel was one unit of
/// detail; rather than restate each of them in some other unit, put the input back
/// into the units they were written in.
///
/// It has to be the *point spread* that decides, not the image size. A native
/// render at 1024 resolves genuine detail — it reads a scale of exactly 1.00 and
/// is not touched, and it already traces in 1.7 s. An upsample, a blur or a
/// photograph of a screen carries fewer units of detail than it has pixels, and
/// those extra pixels are not information: they are what shatters the palette into
/// a thousand regions and what the tracer then spends a minute describing.
///
/// Nothing is lost in the output. The SVG keeps its `width` and `height` in the
/// original units and only its `viewBox` shrinks, so it renders at exactly the
/// size it always did — and being a vector, at any other size too.
///
/// The edge width `s` (`intake_scale`, px per unit of detail) is measured on the image
/// composited over white. At or above [`INTAKE_SCALE_FLOOR`] the raster is box-filtered
/// down by `min(s, INTAKE_SCALE_CAP)` (each side rounded, at least 8 px); otherwise, or if
/// that would not shrink both sides, it is returned untouched. The flag says whether it was
/// resampled, which later decides that the SVG is retargeted to the arrival size.
fn normalise_intake(img: inkvec_trace::Rgba, quiet: bool) -> (inkvec_trace::Rgba, bool) {
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let scale = inkvec_trace::coverage::intake_scale(&rgb, img.width, img.height);
    if scale < INTAKE_SCALE_FLOOR {
        return (img, false);
    }
    let s = scale.min(INTAKE_SCALE_CAP);
    let (nw, nh) = (
        ((img.width as f64) / s).round().max(8.0) as usize,
        ((img.height as f64) / s).round().max(8.0) as usize,
    );
    if nw >= img.width || nh >= img.height {
        return (img, false);
    }
    diag::stage(quiet, || {
        format!(
            "  intake        {:.2} px per detail unit; tracing at {}x{} and              emitting at {}x{}",
            scale, nw, nh, img.width, img.height
        )
    });
    let (w, h) = (img.width, img.height);
    let out = inkvec_trace::coverage::downsample_to(&img, nw, nh);
    debug_assert!(out.width < w && out.height < h);
    (out, true)
}

/// What a trace produced: the SVG (before the output post-processing), the stage log,
/// the traced raster's size and the fit's lambda.
#[derive(Debug, Clone)]
pub struct Traced {
    /// The traced SVG, before output post-processing.
    pub svg: String,
    /// Log lines, one per pipeline stage.
    pub stats: Vec<String>,
    /// Width, in pixels, of the raster that was actually traced.
    pub width: usize,
    /// Height, in pixels, of the raster that was actually traced.
    pub height: usize,
    /// The fit's lambda, when the pipeline reached fitting.
    pub lambda: Option<f64>,
}

/// Trace a loaded raster. This is the whole pipeline behind the command line and the
/// browser build alike; `run` wraps it with file I/O and `post_process` applies the
/// output options.
pub fn trace_image(
    img: inkvec_trace::Rgba,
    args: &Args,
) -> Result<Traced, Box<dyn std::error::Error>> {
    trace_image_sized(img, args, None)
}

/// [`trace_image`], with the size the SVG is presented at carried in from the caller.
///
/// When the caller has already capped the raster before this point (a decode-time
/// `--max-dim`), the raster's own dimensions are the capped ones and would be written as
/// the SVG's `width`/`height`. `display_size` restores the arrival size so the document
/// keeps the `--max-dim` contract: geometry in capped space, presented at the size that
/// arrived.
pub fn trace_image_sized(
    img: inkvec_trace::Rgba,
    args: &Args,
    display_size: Option<(usize, usize)>,
) -> Result<Traced, Box<dyn std::error::Error>> {
    trace_prepared(intake(img, args, display_size))
}

/// The image and the settings the tracer proper works from: everything
/// [`trace_image_sized`] does to a freshly decoded raster before the restorer sees it.
///
/// The fields are what the rest of the pipeline reads, and [`trace_prepared`] is what reads
/// them.
#[derive(Debug, Clone)]
pub struct Intake {
    /// The raster the tracer sees: unblocked, intake-normalised and capped.
    pub img: inkvec_trace::Rgba,
    /// The settings, with the pixel-denominated knobs priced in this raster's own units.
    pub args: Args,
    /// Whether an exact pixel-grid upscale was undone.
    pub replicated: bool,
    /// Whether anything resampled the raster, which is what decides if the SVG must be
    /// retargeted to the presentation size.
    pub normalised: bool,
    /// Whether the raster was reduced to whole source pixels, whose two axes need not keep
    /// the aspect exactly: the SVG is then presented axis by axis (`post::stretch_axes`).
    pub stretch: bool,
    /// The size the SVG is presented at.
    pub display: (usize, usize),
}

/// `svg`, traced at the raster's own size, presented at `w` x `h` (`post::retarget`), and
/// axis by axis when `stretch` says the raster's two axes were reduced by different factors.
fn present(svg: &str, w: usize, h: usize, stretch: bool) -> String {
    let svg = retarget(svg, w, h);
    if stretch {
        post::stretch_axes(&svg)
    } else {
        svg
    }
}

/// The reductions that run after an unblock and ahead of the pre-passes: a resampled or
/// blurred raster is reduced to the detail it carries (`soft_intake`), unless asked not to
/// (`--no-soft-intake`); `--intake-scale` keeps its own older measurement in its place.
/// Neither runs after an unblock (the raster is already the source) nor ahead of SR, which
/// wants the soft raster to put detail back into. Returns the raster, whether anything
/// resampled it (an unblock counts), and whether `soft_intake` reduced it.
///
/// Quality mode only. Fast mode traces a soft raster as it arrived: reduced, its parameters
/// fell 92 % on 4x bicubic input but its colour error did not follow -- dE00 −2.4 % with 12
/// of 28 worse, and +6.6 % on Lanczos 3x (9 of 28 worse, r2-inputs stress set, 2026-10-04)
/// -- because its palette has none of the soft-input noise guards the reduced raster's
/// remaining ramps need (the r2-inputs report's 5.9).
fn reduce_intake(
    img: inkvec_trace::Rgba,
    args: &Args,
    replicated: bool,
) -> (inkvec_trace::Rgba, bool, bool) {
    if replicated || args.sr != inkvec_sr::Mode::Off {
        (img, replicated, false)
    } else if args.intake_scale {
        let (out, did) = normalise_intake(img, args.quiet);
        (out, did, false)
    } else if args.no_soft_intake || args.mode == TraceMode::Fast {
        (img, false, false)
    } else {
        let (out, did) = soft_intake::reduce(img, args.quiet);
        (out, did, did)
    }
}

/// Everything [`trace_image_sized`] does before the restorer pre-pass: undo an exact
/// pixel-grid upscale, normalise the intake, apply `--max-dim`, and price `--precision`,
/// `--min-area` and lambda in the raster's own units.
///
/// It is public, and separate from [`trace_prepared`], for one caller: a browser that runs
/// the restorer network itself, in a runtime this crate cannot call into -- WebGPU through
/// ONNX Runtime Web, whose session is asynchronous where this pipeline is not. Such a caller
/// cannot hand a [`crate::Args::restore_command`]-style backend to `restore_prepass`, so it
/// splits the pipeline at the same seam instead: `intake`, then its own network, then
/// [`trace_prepared`] with `restore` off and `lossy` forced on. Splitting it here rather
/// than before the decode is the point -- the restorer must see the raster the tracer will
/// see, at the size `--max-dim` settled on, which is the order `trace_image_sized` has.
pub fn intake(
    img: inkvec_trace::Rgba,
    args: &Args,
    display_size: Option<(usize, usize)>,
) -> Intake {
    let mut img = img;
    let (display_w, display_h) = display_size.unwrap_or((img.width, img.height));

    // A nearest-neighbour upscale, undone, before anything else looks at the image. Exact,
    // so it is not a trade: the pixels that come back are the ones the file was made from,
    // and the SVG is still written at the size that arrived.
    //
    // Before the pre-pass, not after, and that is the whole point of the order: run the
    // upscaler on the blocky version and it treats the block edges as the artwork —
    // measured on a 96-px logo blown up to 768, `--sr on` came back with 14 inks and 1846
    // segments, worse than doing nothing. On the recovered original it has something real
    // to put detail back into.
    //
    // Any factor of 2 or more, whole or not (a 3.5x browser zoom is cells of 3 and 4 pixels):
    // `pixel_grid` finds the lattice and has checked that one pixel per cell rebuilds the
    // input bit for bit, so `reduce` hands back the source pixels themselves. Its two axes
    // can have slightly different pitches (a rounded output size), so the SVG is presented
    // axis by axis (`stretch`).
    let mut replicated = false;
    if !args.no_unblock {
        if let Some(grid) = pixel_grid(&img) {
            let (sw, sh) = grid.source_size();
            let (px, py) = grid.pitch(img.width, img.height);
            diag::stage(args.quiet, || {
                let factor = if (px - py).abs() < 5e-4 {
                    format!("{px:.3}")
                } else {
                    format!("{px:.3}x{py:.3}")
                };
                format!(
                    "  unblock       {}x{} is a {}x pixel upscale of {sw}x{sh}; tracing the original",
                    img.width,
                    img.height,
                    factor.trim_end_matches('0').trim_end_matches('.')
                )
            });
            img = grid.reduce(&img);
            replicated = true;
        }
    }

    // The cap, intake normalisation and oversample correction run *before* the restore and
    // SR pre-passes, so that a probe trace either `auto` mode makes of an input it keeps is
    // bounded exactly like the real trace. They used to run after the pre-passes' early
    // returns, so `--restore auto` / `--sr auto` kept a probe traced at full resolution and
    // `--max-dim` (and `--intake-scale`) were silently ignored.
    let (reduced, mut normalised, soft_reduced) = reduce_intake(img, args, replicated);
    img = reduced;

    // Larger than the product wants to spend time on: trace a box-filtered
    // reduction and write the SVG at the original size. The reduction is the same
    // exact area average the intake normaliser uses, so edges stay edges.
    let longest = img.width.max(img.height);
    if args.max_dim > 0 && longest > args.max_dim {
        let s = longest as f64 / args.max_dim as f64;
        let (nw, nh) = (
            ((img.width as f64) / s).round().max(8.0) as usize,
            ((img.height as f64) / s).round().max(8.0) as usize,
        );
        diag::stage(args.quiet, || {
            format!(
                "  max-dim       {}x{} -> {}x{} for tracing; output keeps {}x{}",
                img.width, img.height, nw, nh, display_w, display_h
            )
        });
        img = inkvec_trace::coverage::downsample_to(&img, nw, nh);
        normalised = true;
    }

    let args = price_in_raster_units(&img, args);

    Intake {
        img,
        args,
        replicated,
        normalised,
        stretch: replicated || soft_reduced,
        display: (display_w, display_h),
    }
}

/// `args` with `--precision`, `--min-area` and lambda priced in this raster's own units:
/// scaled up when the raster carries the drawing on more pixels than the drawing needs.
///
/// Two measurements of `img` decide it (neither in fast mode, which reads none of the
/// three): the edge width `e` (`intake_scale`, 1.00 for an honest one-pixel boundary) and
/// the round-trip factor `r` (`oversample_factor`, how far the raster can be downsampled
/// and put back without loss). Then, with `R` the reference extent ([`REF_EXTENT`]):
///
/// * precision `× r` when `e` exceeds the soft-intake threshold, else unchanged;
/// * min-area `× r²` when the longest side exceeds `R`, else unchanged;
/// * lambda `× r` when the longest side exceeds `R` and `r > 1`, else unchanged.
///
/// A native render's `e` stays under the threshold; at 512 px its `r` is mostly 2 to 8. The
/// reasons for each factor, and the measurements behind them, are in the comments below.
fn price_in_raster_units(img: &inkvec_trace::Rgba, args: &Args) -> Args {
    // Two of the knobs below are denominated in pixels, and a raster that carries the
    // same drawing at more pixels per unit therefore gets read as if it were a more
    // detailed drawing. `--precision` asks for accuracy in pixels, so at 4x it silently
    // demands four times the accuracy the artwork was ever measured to; `--min-area` is a
    // speckle floor in px^2, so at 4x a speckle is sixteen times over it and survives.
    // Both push the fitter toward spending segments on the same content. Measured on the
    // a real brand mark upscaled 4x: 301 paths and 11,960 coordinates against 16 and 456 for
    // the same drawing at 1x -- and hand-scaling the two knobs brought it back to 21 and
    // 441. That is not a better trace of a better raster, it is the same trace priced in
    // the wrong units.
    //
    // So price them in the raster's own units. `oversample_factor` asks how far the
    // raster can be downsampled without losing anything, which is the factor by which it
    // carries the drawing on more pixels than the drawing needs -- and unlike edge width
    // it is not fooled by a super-resolution model that returns sharp edges at high
    // resolution. A native intake at or below `REF_EXTENT` is left as it is (see below), so
    // the 128 px benchmark is untouched; at 512 px most native renders read 2 to 8.
    // Fast mode reads neither: its fit has no lambda or precision, and its front end sets its
    // own speckle floor from the image's size (`inkvec_trace::fast::front`). The two
    // measurements are three full-image round trips, the largest cost in a fast trace at
    // 2048 px. Balanced runs the same fitter and front end, so it reads neither either.
    let (oversample, redundancy) = if args.mode.fast_engine() {
        (1.0, 1.0)
    } else {
        let (w, h) = (img.width, img.height);
        let rgb = img.composited([1.0, 1.0, 1.0]);
        // Two signals, and each does the half of the job the other cannot. Edge width
        // decides *whether* this is a native render, which it can: every corpus raster
        // reads at most 1.50 against a 1.75 threshold. It cannot say by how much a raster
        // is oversampled, because a super-resolution model returns sharp edges and reads
        // 2.00 where the answer is 4. The downsampling round trip measures that factor
        // exactly, but cannot be trusted to make the first decision -- smooth native
        // artwork survives halving too, and would be rewritten for no reason.
        let edge = inkvec_trace::coverage::intake_scale(&rgb, w, h);
        // The round trip must keep the drawing in absolute terms (a mean error under 3
        // levels) and in relative ones (at most half of the image's detail lost): a
        // near-empty 144 px raster with one 38 px² disc passed the first test at every
        // factor, because the empty canvas dilutes the mean, read as 8x, and its disc
        // fell under the 64-fold speckle floor. See `coverage::oversample_factor`.
        let round_trip = inkvec_trace::coverage::oversample_factor(&rgb, w, h) as f64;
        let up = if edge > inkvec_trace::color::SOFT_INTAKE_EDGE {
            round_trip
        } else {
            1.0
        };
        (up, round_trip)
    };

    // The speckle floor scales with the round trip alone; `--precision` stays behind the
    // edge-width gate above.
    //
    // These two knobs look alike and are not. Measured on ten corpus icons at 128, 256,
    // 512 and 1024, the segment count grows only 1.33x across the whole range and a search
    // for the precision that reproduces the 128 px drawing picks the shipped 0.1 at every
    // tier -- the fitter is already near enough resolution-invariant, because a shape the
    // alphabet can represent exactly leaves no residual for another segment to chase.
    // `prim_ellipse` comes out as nine segments at 128, 256, 512 and 1024 alike, with no
    // correction of any kind.
    //
    // The speckle floor is a different story, because it is an *area*. An anti-aliasing
    // sliver between two regions is eight times longer and eight times wider at eight
    // times the resolution, so it carries sixty-four times the area and sails over a floor
    // that removed it at 128. It then becomes a face, and a face needs a boundary.
    // Measured on `mosaic_grid6`, same palette either way: 44 faces at 128 against 772 at
    // 1024, which is where the 131 segments become 2367. Scaling the floor by the round
    // trip squared brings that back to 94 faces and 293 segments.
    //
    // Crucially this is *relative*, and the absolute version is on record as a failure: a
    // flat 9-px colour-mode floor was tried on 2026-09-03 and measured worse on the full
    // 980-icon set (objective 0.7885 against 0.7673, 508 icons worse), because it erased
    // real dots as readily as confetti. A floor that rises only when the raster is
    // measurably carrying surplus pixels cannot make that trade: where the round trip reads 1
    // nothing moves, and at or below `REF_EXTENT` nothing moves at all (next paragraph).
    // Only above `REF_EXTENT`, and this guard is not tidiness -- without it the change
    // regresses the tier it was tuned on. `min_area` was fitted against 128 px rasters as
    // they are, so whatever redundancy a typical icon carries at that size is already
    // inside the constant; scaling by it again counts it twice, raises the floor on
    // genuinely small art, and erases real dots. Measured: 128ss objective 0.4005 -> 0.4112,
    // the same shape of failure as the flat 9-px floor of 2026-09-03. Above the reference
    // there is no such double count, because the constant was never fitted there.
    let over_reference = (img.width.max(img.height) as f64) > REF_EXTENT;
    let floor_scale = if over_reference {
        redundancy * redundancy
    } else {
        1.0
    };
    let redundancy_lambda = if over_reference && redundancy > 1.0 {
        redundancy
    } else {
        1.0
    };
    if oversample > 1.0 || floor_scale > 1.0 || redundancy_lambda > 1.0 {
        diag::stage(args.quiet, || {
            if oversample > 1.0 {
                format!(
                    "  intake        oversampled x{oversample:.0}: scaling precision x{oversample:.0}, min-area x{floor_scale:.0}, lambda x{redundancy_lambda:.0}"
                )
            } else {
                format!(
                    "  intake        {redundancy:.0}x more pixels than detail: scaling min-area x{floor_scale:.0} and lambda x{redundancy_lambda:.0}"
                )
            }
        });
        let mut a = args.clone();
        a.precision *= oversample;
        a.min_area *= floor_scale;
        a.lambda_scale *= redundancy_lambda;
        a
    } else {
        args.clone()
    }
}

/// The tracer, from the restorer pre-pass onward, on a raster [`intake`] has already
/// prepared. The second half of [`trace_image_sized`].
pub fn trace_prepared(prepared: Intake) -> Result<Traced, Box<dyn std::error::Error>> {
    // The curve prices are asked for in the settings and held for the whole trace, restorer
    // retrace and all, so every stage that compares a line with a curve compares at the
    // same price. A trace that asks for nothing is not affected.
    let model = inkvec_fit::cost::CostModel::with_overrides(
        prepared.args.bezier_cost,
        prepared.args.corner_angle,
    );
    inkvec_fit::cost::with_cost_model(model, || trace_prepared_priced(prepared))
}

/// [`trace_prepared`] under the cost model it installed.
///
/// In order: the restorer pre-pass ([`restore_prepass`]), soft intake forced on for a
/// restored image, the SR pre-pass (with `auto` deciding from a probe trace, which is kept
/// as the result when the input measures clean or no upscaler is available), the alpha
/// matte ([`alpha::alpha_source_owned`]), the
/// fit configuration, and then one pipeline -- strokes when asked for and the drawing is
/// line art, else bilevel or colour. The SVG is retargeted to the presentation size
/// whenever anything resampled the raster.
fn trace_prepared_priced(prepared: Intake) -> Result<Traced, Box<dyn std::error::Error>> {
    let Intake {
        mut img,
        args,
        replicated,
        normalised,
        stretch,
        display: (display_w, display_h),
    } = prepared;
    let args = &args;
    let mut sr_note: Option<String> = None;

    // The restorer comes after the intake (see `intake`) and before SR, which resamples: it
    // was trained on damage at the size the damage happened, and returns an image that size.
    let pass = restore_prepass(img, args)?;
    img = pass.img;
    let restore_note = pass.note;
    let restored = pass.restored;
    let mut probe = pass.probe;
    // A probe is traced in colour, because it is measured against the colour input (see
    // `trace_once`); under monochrome it decides and is then traced again as asked.
    if args.monochrome {
        probe = None;
    }
    if args.sr == inkvec_sr::Mode::Off {
        // `auto` kept the input, and nothing else is going to look at it: the probe is the
        // trace, stage log and lambda included, as the plain path reports them.
        if let Some((svg, stats, lambda)) = probe.take() {
            let svg = if normalised || (img.width, img.height) != (display_w, display_h) {
                present(&svg, display_w, display_h, stretch)
            } else {
                svg
            };
            return Ok(Traced {
                svg,
                stats: restore_note.into_iter().chain(stats).collect(),
                width: img.width,
                height: img.height,
                lambda: Some(lambda),
            });
        }
    }

    // Restored input is traced with soft intake on, the configuration the restorer was
    // validated in. It has to be forced: a restored image can look clean enough that the
    // edge-width and ringing detectors no longer open soft intake by themselves.
    let lossy_args;
    let args = if restored && args.lossy != inkvec_sr::Mode::On {
        lossy_args = Args {
            lossy: inkvec_sr::Mode::On,
            ..args.clone()
        };
        &lossy_args
    } else {
        args
    };

    // `auto` needs a trace before it can decide, so it produces one and keeps it
    // when the input turns out to be undamaged -- the common case pays one trace
    // and never touches the upscaler.
    let mut sr_on = args.sr != inkvec_sr::Mode::Off;
    // The upscaler `auto` built when it decided to clean, so it is not built twice.
    let mut upscaler: Option<Box<dyn inkvec_sr::Upscaler>> = None;
    if args.sr == inkvec_sr::Mode::Auto {
        let probe = match probe.take() {
            Some(p) => p,
            None => trace_once(&img, args)?,
        };
        // `Some(stats line)` when `auto` traces the input as it is instead of cleaning it. As
        // for `--restore auto`: the residual alone reads clean shading as damage, so cleaning
        // also needs a sign of resampling or compression (see `damage`), and that cheaper
        // half is asked first.
        let decision = damage::soft(&img, args.lossy)
            .map(|_| inkvec_sr::decide(&img, &probe.0, args.sr_threshold));
        let direct = match decision {
            None => {
                Some("sr            no sign of resampling or compression; traced directly".into())
            }
            Some(inkvec_sr::Decision::Keep { residual }) => Some(match residual {
                Some(r) => format!(
                    "sr            residual {r:.3} <= {:.3}, traced directly",
                    args.sr_threshold
                ),
                None => "sr            could not measure the fit; traced directly".into(),
            }),
            Some(inkvec_sr::Decision::Clean { residual }) => {
                let measured =
                    residual.map(|r| format!("residual {r:.3} > {:.3}", args.sr_threshold));
                match build_upscaler(args) {
                    Ok(up) => {
                        upscaler = Some(up);
                        sr_note = measured;
                        None
                    }
                    // `auto` is a request to clean *if it helps*, so no upscaler to be had --
                    // no `tools/inkvec_sr` beside the binary, an unusable `--sr-command` --
                    // is not a reason to fail the trace: keep the probe, as a clean input
                    // would, and say why in the stats. `on` asked for the clean-up outright
                    // and still fails without one. This is `--restore auto`'s rule (see
                    // `restore_prepass`), word for word in the stats line.
                    Err(e) => Some(format!(
                        "sr            {}but no upscaler is available ({e}); traced directly",
                        measured.map(|n| format!("{n}, ")).unwrap_or_default()
                    )),
                }
            }
        };
        if let Some(note) = direct {
            if args.monochrome {
                // The probe is a colour trace; a monochrome one is traced below, uncleaned.
                sr_note = Some(note);
                sr_on = false;
            } else {
                let (svg, stats, lambda) = probe;
                let svg = if normalised || (img.width, img.height) != (display_w, display_h) {
                    present(&svg, display_w, display_h, stretch)
                } else {
                    svg
                };
                return Ok(Traced {
                    svg,
                    stats: restore_note
                        .into_iter()
                        .chain(std::iter::once(note))
                        .chain(stats)
                        .collect(),
                    width: img.width,
                    height: img.height,
                    lambda: Some(lambda),
                });
            }
        }
    }

    if sr_on {
        let up = match upscaler {
            Some(up) => up,
            None => build_upscaler(args)?,
        };
        let opt = inkvec_sr::Options {
            out_scale: args.sr_scale,
            recolour: !args.sr_no_recolour,
        };
        let t = inkvec_core::clock::Instant::now();
        let cleaned = inkvec_sr::prepass(up.as_ref(), &img, opt)?;
        sr_note = Some(format!(
            "sr            {}cleaned to {}x{}{} in {:.2}s, {}",
            sr_note.map(|n| format!("{n}; ")).unwrap_or_default(),
            cleaned.width,
            cleaned.height,
            if args.sr_no_recolour {
                ", no recolour"
            } else {
                ""
            },
            t.elapsed().as_secs_f64(),
            up.describe()
        ));
        img = cleaned;
    }

    // The SVG is presented at the size it arrived at -- or, when SR ran, at the size SR
    // produced -- while the viewBox stays at the (capped) size the tracer actually saw.
    let (display_w, display_h) = if replicated {
        (display_w, display_h)
    } else if sr_on {
        (img.width, img.height)
    } else {
        (display_w, display_h)
    };

    // With `--hypotheses`, the structural alternatives are traced as well and the trace with
    // the shortest description length against this raster is kept (see `select`). The
    // raster is cloned only then; the default path moves it into the one trace.
    let (w, h) = (img.width, img.height);
    let (svg, mut stats, lambda) = if args.hypotheses && hypotheses_apply(args) {
        let base = trace_bordered(img.clone(), args)?;
        select::choose(&img, args, base, |a| trace_bordered(img.clone(), a))?
    } else {
        trace_bordered(img, args)?
    };
    if let Some(n) = sr_note {
        stats.insert(0, n);
    }
    if let Some(n) = restore_note {
        stats.insert(0, n);
    }
    let svg = if normalised || (w, h) != (display_w, display_h) {
        present(&svg, display_w, display_h, stretch)
    } else {
        svg
    };
    Ok(Traced {
        svg,
        stats,
        width: w,
        height: h,
        lambda: Some(lambda),
    })
}

/// One trace of `img` under `args`, through the border pad when it applies.
///
/// Art that reaches the border of a transparent raster is traced on a canvas `PAD` px
/// larger on every side and moved back afterwards (see `border`): the colour tracer fits a
/// boundary that ends on the image frame worse than the closed outline it becomes with
/// room round it. Quality colour mode only; when the traced document holds anything
/// `border::crop` cannot translate, the raster is traced again as it is.
fn trace_bordered(
    img: inkvec_trace::Rgba,
    args: &Args,
) -> Result<select::Trace, Box<dyn std::error::Error>> {
    let (w, h) = (img.width, img.height);
    if border_pad_applies(args) && border::touches_border(&img) {
        let padded = border::pad(&img);
        diag::stage(args.quiet, || {
            format!(
                "  border        art touches the canvas edge; traced with a {} px transparent margin",
                border::PAD
            )
        });
        let (svg, stats, lambda) = trace_matted(padded, args, Some(w.max(h)))?;
        match border::crop(&svg, w, h) {
            Some(svg) => Ok((svg, stats, lambda)),
            None => trace_matted(img, args, None),
        }
    } else {
        trace_matted(img, args, None)
    }
}

/// Whether `--hypotheses` can do anything under these settings: the Quality colour
/// pipeline, whose document is compared with the colour input (not the monochrome,
/// bilevel or stroke writers, nor Fast mode), and no `--uncertainty` bands, which every
/// traced hypothesis would write over the last one's.
fn hypotheses_apply(args: &Args) -> bool {
    args.mode == TraceMode::Quality
        && !args.bilevel
        && !args.strokes
        && !args.monochrome
        && args.uncertainty.is_none()
}

/// Whether the border pad (`border`) may be used under these settings: the Quality colour
/// pipeline with none of the outputs that carry coordinates outside the document
/// (`--uncertainty`'s bands) or in a symbol's own frame (`--use-symbols`). Fast mode, the
/// bilevel, stroke and monochrome writers trace as they always did.
fn border_pad_applies(args: &Args) -> bool {
    args.mode == TraceMode::Quality
        && !args.bilevel
        && !args.strokes
        && !args.monochrome
        && !args.use_symbols
        && args.uncertainty.is_none()
}

/// The tail of [`trace_prepared_priced`] on one raster: the alpha matte, the fit
/// configuration and one pipeline. Returns the document, its report lines and the fit's
/// lambda.
///
/// `extent`, when given, is the longest side the fit configuration is priced for instead of
/// the raster's own: the original raster's, when `img` is that raster on a padded canvas
/// (`border`), so the price of a coordinate (`ln(extent / precision)`) does not move.
fn trace_matted(
    img: inkvec_trace::Rgba,
    args: &Args,
    extent: Option<usize>,
) -> Result<(String, Vec<String>, f64), Box<dyn std::error::Error>> {
    // Transparency, once, after every resampling step: put the image against a matte the
    // artwork is not made of and keep the alphas for the emitter. Everything from here
    // traces the matted image, which is written over the input's own buffer: nothing reads
    // the unmatted one again (see `alpha::alpha_source_owned`).
    let (alpha_src, opaque) =
        match alpha::alpha_source_owned(img, args.quiet, args.cutout, args.native_alpha) {
            Ok(src) => (Some(src), None),
            Err(img) => (None, Some(img)),
        };
    let img = match (&alpha_src, &opaque) {
        (Some(src), _) => &src.flat,
        (None, Some(img)) => img,
        (None, None) => unreachable!("alpha_source_owned returns the image or its source"),
    };
    let cut_args = alpha::cutout_args(args, alpha_src.as_ref());
    let args = &*cut_args;

    // Through `fit_config`, not `FitConfig::from_precision` directly, so that
    // `--content-units` applies the whole of its mechanism here and not half of it.
    //
    // It used to build its own configuration and skip the scaling, which left the flag
    // scaling sigma (via `in_content_units`, below) while lambda stayed tied to raw pixel
    // extent -- exactly the half that `fit_config`'s doc comment says must not be applied
    // alone. `content_scale` returns 1.0 unless the flag is set, so this is the identity
    // on the default path: `FitConfig::from_precision` with nothing scaled.
    let cfg = match extent {
        Some(e) => fit_config_sized(img, args, e),
        None => fit_config(img, args),
    };

    // Line art, emitted the way it was drawn. Tried before the ordinary paths and
    // declines by returning None, so anything that is not a stroked drawing is
    // untouched.
    let stroked = if args.strokes {
        run_strokes(img, args, &cfg)
    } else {
        None
    };
    let (svg, stats) = if let Some(r) = stroked {
        r
    } else if args.bilevel {
        run_bilevel(img, args, &cfg)
    } else {
        run_color(img, args, &cfg, alpha_src.as_ref())?
    };
    Ok((svg, stats, cfg.lambda))
}

/// The command line's whole job for one file: check the input and output paths, decode
/// (capped at `--max-dim` during the decode), resolve `--lossy auto` from the file's first
/// bytes, trace, and [`finish`].
fn run(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    // Checked here rather than left to the decoder, which reports a missing file as a decode
    // error carrying the operating system's own wording.
    if !args.input.is_file() {
        return Err(format!("input file not found: {}", args.input.display()).into());
    }
    // And before the trace, not after it: a missing output folder should not cost a whole run.
    let out = args
        .output
        .clone()
        .unwrap_or_else(|| args.input.with_extension("svg"));
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !parent.is_dir() {
            return Err(format!("output folder does not exist: {}", parent.display()).into());
        }
    }
    let (img, (arr_w, arr_h)) = load_image_capped(&args.input, args.max_dim)?;
    // `--lossy auto` is a question about the file, so it is answered here, where the file
    // is, and not inside the tracer, which only ever sees decoded pixels. Reading the
    // first few bytes is enough for every container the tracer accepts.
    let args = &resolve_lossy(args, || {
        let mut head = [0u8; 32];
        use std::io::Read;
        std::fs::File::open(&args.input)
            .and_then(|mut f| f.read(&mut head).map(|n| head[..n].to_vec()))
            .ok()
    });
    let t = trace_image_sized(img, args, Some((arr_w as usize, arr_h as usize)))?;
    finish(args, t.svg, t.stats, t.width, t.height, t.lambda)
}

/// Turn `--lossy auto` into a definite yes or no by looking at the container.
///
/// `head` supplies the first bytes of the source file, or `None` when they cannot be read;
/// an unreadable or unrecognised container resolves to `Off`, because the palette's noise
/// guard costs 10.9 % on the 246-icon screen set when it runs on a clean intake
/// (0.4005 -> 0.4442, measured 2026-09-08) and must not fire on a guess.
pub fn resolve_lossy(args: &Args, head: impl FnOnce() -> Option<Vec<u8>>) -> Args {
    let mut out = args.clone();
    if out.lossy == inkvec_sr::Mode::Auto {
        let lossy = head()
            .and_then(|b| inkvec_trace::lossy_container(&b))
            .unwrap_or(false);
        out.lossy = if lossy {
            inkvec_sr::Mode::On
        } else {
            inkvec_sr::Mode::Off
        };
    }
    out
}

/// The upscaler the flags ask for. An explicit `--sr-command` always wins; otherwise the
/// packaged Python pre-pass under `tools/`.
fn build_upscaler(args: &Args) -> Result<Box<dyn inkvec_sr::Upscaler>, Box<dyn std::error::Error>> {
    // The tests run the SR pre-pass with an upscaler in this process: an external command
    // would make them depend on what the test machine has installed.
    #[cfg(test)]
    if args.sr_command.as_deref() == Some(lib_tests::NEAREST_UPSCALER) {
        return Ok(Box::new(lib_tests::Nearest));
    }
    if let Some(cmd) = &args.sr_command {
        let mut parts = split_command(cmd).into_iter();
        let program = parts.next().ok_or("--sr-command is empty")?;
        return Ok(Box::new(inkvec_sr::external::External {
            program,
            args: parts.collect(),
            scale: 4,
            work_dir: None,
        }));
    }
    let tools = sr_tools_dir().ok_or(
        "the packaged SR pre-pass (tools/inkvec_sr) was not found next to this binary; set \
         INKVEC_TOOLS_DIR to the folder that contains inkvec_sr, or pass --sr-command",
    )?;
    Ok(Box::new(inkvec_sr::external::External::python(&tools, 4)))
}

/// The `tools` folder that holds the packaged SR pre-pass (`tools/inkvec_sr`).
///
/// Found at run time: `INKVEC_TOOLS_DIR`, then a `tools/` folder beside the executable or in
/// one of its parents (a release archive, or `target/release` inside a checkout), then the
/// source checkout the binary was built from. The last is only a convenience for development
/// builds; a binary copied to another machine never depends on it.
fn sr_tools_dir() -> Option<std::path::PathBuf> {
    let has_sr = |dir: &Path| dir.join("inkvec_sr").is_dir();
    if let Some(dir) = inkvec_core::env::path("INKVEC_TOOLS_DIR") {
        return has_sr(&dir).then_some(dir);
    }
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().skip(1).take(4) {
            let dir = ancestor.join("tools");
            if has_sr(&dir) {
                return Some(dir);
            }
        }
    }
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools");
    has_sr(&dev).then(|| dev.canonicalize().unwrap_or(dev))
}

/// Split a `--sr-command` / `--restore-command` string into a program and its arguments.
///
/// Whitespace separates words, and a word can be wrapped in double or single quotes to keep
/// spaces inside it, so `"C:\Program Files\tool\up.exe" {in} {out}` names one program.
/// Backslashes are not escapes: they are path separators on Windows, where this matters most.
fn split_command(cmd: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    for c in cmd.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            None => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// What the restorer pre-pass did before tracing.
struct RestorePass {
    /// The image to trace from here on: restored, or the input unchanged.
    img: inkvec_trace::Rgba,
    /// One line for the stats, when the pass ran or measured anything.
    note: Option<String>,
    /// The probe trace `--restore auto` made of an input it kept (SVG, stage log, lambda), for
    /// SR's `auto` to reuse or, with SR off, to return as the trace.
    probe: Option<select::Trace>,
    /// Whether the image was restored, which forces soft intake for the trace that follows.
    restored: bool,
}

/// Run `--restore`. `auto` works exactly like SR's: one probe trace, kept when the input
/// measures clean, and handed on so the two pre-passes never trace the same image twice.
fn restore_prepass(
    img: inkvec_trace::Rgba,
    args: &Args,
) -> Result<RestorePass, Box<dyn std::error::Error>> {
    let mut pass = RestorePass {
        img,
        note: None,
        probe: None,
        restored: false,
    };
    if args.restore == inkvec_restore::Mode::Off {
        return Ok(pass);
    }
    let mut auto_probe = None;
    if args.restore == inkvec_restore::Mode::Auto {
        // The residual alone reads smooth shading in clean art as damage, so `auto` also
        // needs the file or the pixels to say the input was compressed (see `damage`). That
        // evidence is asked first: it is the cheap half (a flag, or the ringing score), while
        // the residual needs a probe trace rendered at full size. Without it there is nothing
        // to decide, so no probe is made and the input takes the plain path: a clean input
        // pays the evidence alone, and its trace is the plain one by construction. Fast mode
        // reads the container only (`damage::compressed`).
        let pixels = args.mode != TraceMode::Fast;
        let Some(why) = damage::compressed(&pass.img, args.lossy, pixels) else {
            pass.note = Some("restore       no sign of compression; traced directly".into());
            return Ok(pass);
        };
        inkvec_core::progress::note(|| {
            "a first trace, to see whether the denoiser is needed".into()
        });
        let probe = trace_once(&pass.img, args)?;
        let decision = inkvec_restore::decide(
            &pass.img,
            &probe.0,
            inkvec_restore::Options {
                residual_threshold: args.restore_threshold,
            },
        );
        match decision {
            inkvec_restore::Decision::Restore { residual } => {
                pass.note = residual
                    .map(|r| format!("residual {r:.3} > {:.3}, {why}", args.restore_threshold));
                auto_probe = Some(probe);
            }
            inkvec_restore::Decision::Keep { residual } => {
                pass.note = Some(match residual {
                    Some(r) => format!(
                        "restore       residual {r:.3} <= {:.3}, traced directly",
                        args.restore_threshold
                    ),
                    None => "restore       could not measure the fit; traced directly".into(),
                });
                pass.probe = Some(probe);
                return Ok(pass);
            }
        }
    }
    let restorer = match (build_restorer(args), auto_probe) {
        (Ok(r), _) => r,
        // `auto` is a request to restore *if it helps*, so no restorer to be had -- a binary
        // without the network, weights that are not on disk -- is not a reason to fail the
        // trace: keep the probe, as a clean input would, and say why in the stats. `on`
        // asked for the restorer outright and still fails without one.
        (Err(e), Some(probe)) => {
            let measured = pass
                .note
                .take()
                .map(|n| format!("{n}, "))
                .unwrap_or_default();
            pass.note = Some(format!(
                "restore       {measured}but no restorer is available ({e}); traced directly"
            ));
            pass.probe = Some(probe);
            return Ok(pass);
        }
        (Err(e), None) => return Err(e),
    };
    let t = inkvec_core::clock::Instant::now();
    inkvec_core::progress::begin("restore");
    pass.img = inkvec_restore::restore_rgba(restorer.as_ref(), &pass.img)?;
    inkvec_core::progress::end("restore", t.elapsed().as_secs_f64() * 1e3);
    let earlier = pass
        .note
        .take()
        .map(|n| format!("{n}; "))
        .unwrap_or_default();
    pass.note = Some(format!(
        "restore       {earlier}restored {}x{} in {:.2}s, {}",
        pass.img.width,
        pass.img.height,
        t.elapsed().as_secs_f64(),
        restorer.describe()
    ));
    pass.restored = true;
    Ok(pass)
}

/// The restorer the flags ask for. An explicit `--restore-command` always wins; otherwise
/// the network compiled into this binary.
fn build_restorer(
    args: &Args,
) -> Result<Box<dyn inkvec_restore::Restore>, Box<dyn std::error::Error>> {
    // As in `build_upscaler`: the tests restore in this process, not through a command.
    #[cfg(test)]
    if args.restore_command.as_deref() == Some(lib_tests::IDENTITY_RESTORER) {
        return Ok(Box::new(lib_tests::Identity));
    }
    if let Some(cmd) = &args.restore_command {
        let mut parts = split_command(cmd).into_iter();
        let program = parts.next().ok_or("--restore-command is empty")?;
        return Ok(Box::new(inkvec_restore::external::External {
            program,
            args: parts.collect(),
            work_dir: None,
        }));
    }
    #[cfg(any(
        feature = "restore-model",
        feature = "restore-burn",
        feature = "restore-wgpu"
    ))]
    {
        inkvec_restore::load_builtin(args.restore_weights.as_deref())
    }
    #[cfg(not(any(
        feature = "restore-model",
        feature = "restore-burn",
        feature = "restore-wgpu"
    )))]
    {
        Err(
            "this binary was built without the `restore-model` feature; rebuild with \
             `--features restore-model` or pass --restore-command"
                .into(),
        )
    }
}

/// Trace without any of the pre-pass logic, for `auto` to measure.
///
/// Always in colour: `auto` compares the probe with the input, and a monochrome drawing
/// disagrees with a colour input everywhere it is not black or white, which would read as
/// damage on every image. A monochrome trace discards the probe and traces again.
///
/// Otherwise the probe is the trace the plain path makes (`trace_bordered`, through the
/// border pad, and through `--hypotheses` when it applies), so a probe `auto` keeps is byte
/// for byte what `--restore off` / `--sr off` returns. It used to skip the border pad, and
/// on the gate's transparent 128 px tier a third of the probes `--restore auto` kept differed
/// from the plain trace (81 of 246). The stage log and lambda come with it, so a kept probe
/// reports what the plain trace reports; they used to be dropped, leaving only the `restore`
/// or `sr` line.
fn trace_once(
    img: &inkvec_trace::Rgba,
    args: &Args,
) -> Result<select::Trace, Box<dyn std::error::Error>> {
    let colour_args;
    let args = if args.monochrome {
        colour_args = Args {
            monochrome: false,
            ..args.clone()
        };
        &colour_args
    } else {
        args
    };
    let traced = if args.hypotheses && hypotheses_apply(args) {
        let base = trace_bordered(img.clone(), args)?;
        select::choose(img, args, base, |a| trace_bordered(img.clone(), a))?
    } else {
        trace_bordered(img.clone(), args)?
    };
    Ok(traced)
}

/// Apply the output options ([`post_process`]), write the SVG to `--output` (default: the
/// input with an `.svg` extension), and print the stage log unless `--quiet`. This is the
/// command line's own output, so it prints directly rather than through `diag`.
fn finish(
    args: &Args,
    svg: String,
    stats: Vec<String>,
    w: usize,
    h: usize,
    lambda: Option<f64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let out = args
        .output
        .clone()
        .unwrap_or_else(|| args.input.with_extension("svg"));
    let svg = post_process(args, svg, w, h);
    std::fs::write(&out, &svg)?;

    if !args.quiet {
        eprintln!("{} ({}x{})", args.input.display(), w, h);
        for line in &stats {
            eprintln!("  {line}");
        }
        if let Some(l) = lambda {
            eprintln!("  lambda        {l:.2}");
        }
        eprintln!("  wrote         {} ({} bytes)", out.display(), svg.len());
    }
    Ok(())
}

#[cfg(all(test, feature = "research-guidance"))]
mod guidance_tests {
    use super::*;

    #[test]
    fn no_op_guidance_preserves_normal_colour_output_and_uses_args() {
        let mut img = inkvec_trace::Rgba {
            width: 24,
            height: 24,
            data: vec![1.0; 24 * 24 * 4],
        };
        for y in 6..18 {
            for x in 6..18 {
                for c in 0..3 {
                    img.data[(y * 24 + x) * 4 + c] = 0.0;
                }
            }
        }
        let args = Args {
            precision: 0.05,
            tau: 1.5,
            ..Args::default()
        };
        let expected = run_color(&img, &args, &fit_config(&img, &args), None)
            .unwrap()
            .0;
        let mut called = false;
        let actual = trace_color_guided(&img, &args, |t| {
            called = true;
            assert!(!t.map.edges.is_empty());
        })
        .unwrap()
        .0;
        assert!(called);
        assert_eq!(expected, actual);
    }
}

#[cfg(test)]
mod command_tests {
    use super::split_command;

    #[test]
    fn plain_words_split_on_whitespace() {
        assert_eq!(
            split_command("python -m up  {in} {out}"),
            ["python", "-m", "up", "{in}", "{out}"]
        );
    }

    #[test]
    fn quotes_keep_a_path_with_spaces_together() {
        assert_eq!(
            split_command(r#""C:\Program Files\tool\up.exe" {in} '{out} x'"#),
            [r"C:\Program Files\tool\up.exe", "{in}", "{out} x"]
        );
    }

    #[test]
    fn empty_quotes_are_an_empty_argument_and_blank_is_nothing() {
        assert_eq!(split_command(r#"tool "" end"#), ["tool", "", "end"]);
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn test_resolve_lossy_variants() {
        use super::resolve_lossy;
        use crate::args::Args;
        let args = Args {
            lossy: inkvec_sr::Mode::Auto,
            ..Args::default()
        };
        let jpeg_bytes =
            b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00".to_vec();
        let jpeg_res = resolve_lossy(&args, || Some(jpeg_bytes));
        assert_eq!(jpeg_res.lossy, inkvec_sr::Mode::On);

        let png_res = resolve_lossy(&args, || Some(vec![0x89, 0x50, 0x4E, 0x47]));
        assert_eq!(png_res.lossy, inkvec_sr::Mode::Off);

        let none_res = resolve_lossy(&args, || None);
        assert_eq!(none_res.lossy, inkvec_sr::Mode::Off);
    }

    #[test]
    fn test_border_and_hypotheses_gates() {
        use super::{border_pad_applies, hypotheses_apply};
        use crate::args::Args;
        let mut args = Args::default();
        assert!(border_pad_applies(&args));
        args.bilevel = true;
        assert!(!border_pad_applies(&args));
        assert!(!hypotheses_apply(&args));

        args.bilevel = false;
        args.hypotheses = true;
        assert!(hypotheses_apply(&args));
        args.strokes = true;
        assert!(!hypotheses_apply(&args));
    }

    #[test]
    fn test_run_errors_and_exit_codes() {
        use super::{run, run_cli};
        use crate::args::Args;
        use std::path::PathBuf;
        use std::process::ExitCode;
        let bad_args = Args {
            input: PathBuf::from("nonexistent_file_12345.png"),
            ..Args::default()
        };
        assert!(run(&bad_args).is_err());
        assert_eq!(run_cli(&bad_args), ExitCode::FAILURE);

        let bad_dir_args = Args {
            input: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../bindings/contract/tiny.png"),
            output: Some(PathBuf::from("nonexistent_dir_99999/out.svg")),
            ..Args::default()
        };
        assert!(run(&bad_dir_args).is_err());
    }

    #[test]
    fn test_run_success_with_output_and_stats() {
        use super::run;
        use crate::args::Args;
        let tmp = std::env::temp_dir().join("test_run_success.svg");
        let args = Args {
            input: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../bindings/contract/tiny.png"),
            output: Some(tmp.clone()),
            quiet: false,
            ..Args::default()
        };
        assert!(run(&args).is_ok());
        assert!(tmp.exists());
        let _ = std::fs::remove_file(tmp);
    }
}
