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

// As in `inkvec_trace`: the pixel loops here address several parallel arrays by one index,
// and an enumerate over one of them reads worse than the index does.
#![allow(clippy::needless_range_loop)]

mod alpha;
mod args;
mod editable;
mod emit;
mod fast;
mod harmonize;
mod pathdata;
mod post;
mod rings;
mod seams;
mod uncertainty;

use alpha::{alpha_source, pixel_grid, AlphaSource, FaceAlpha};
use args::{parse_args, usage};
pub use args::{parse_color_groups, Args, TraceMode};
use emit::{emit_bilevel, emit_color, emit_decimals};
pub use inkvec_trace::regroup;
use pathdata::fmt_fitted;
pub use post::post_process;
use post::retarget;
use rings::repair_ring_crossings;

use std::path::Path;

/// One ring of a face: the edges it walks, each with whether it is walked backwards.
///
/// Edges are shared — the same edge appears in the two faces either side of it, once
/// forwards and once reversed — which is what keeps the two sides of a boundary on
/// exactly the same curve.
pub(crate) type Ring = Vec<(usize, bool)>;

/// The rings of one face: its outer boundary, then any holes.
pub(crate) type FaceRings = Vec<Ring>;

/// A recovered translucent layer and the face rings it covers.
pub(crate) type Layers<'a> = (&'a inkvec_trace::alpha::AlphaAnalysis, &'a [FaceRings]);
pub use std::process::ExitCode;

use inkvec_core::Point;
use inkvec_fit::{
    adjust_vertices,
    curves::Segment,
    multimodel, optimal_polygon,
    primitives::{fit_primitive_or_arcs, PrimitiveFit},
    FitConfig, FittedPath, Segmentation,
};
use inkvec_trace::{
    gradient, load_image_capped, planar, trace_bilevel, ColorOptions, TraceOptions,
};

/// The command-line entry point.
pub fn cli_main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
    };

    match run(&args) {
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

/// The intake size every pixel-denominated constant in the tracer was tuned at.
///
/// sigma is ~0.05 px, precision is 0.1 px and min_area is 2 px^2, and all three
/// were set against a 128 px corpus. Handed a 512 px raster of the *same* logo the
/// tracer therefore saw four times the boundary points, each carrying four times
/// the pixel residual for the same relative fit, against a lambda that had grown
/// by 0.6 nats -- and bought segments accordingly. Measured on real brand logos:
/// 3.54x the artist's parameters at 128, 6.02x at 256, 11.62x at 512, for content
/// that had not changed. A brand mark has one complexity, and a vectoriser should
/// return it whatever resolution the export happened to be.
///
/// So the three are stated per unit of content. `REF_EXTENT` records the intake size they
/// were tuned at; [`content_scale`] measures how many pixels the raster in hand spends per
/// unit of that detail, rather than assuming it from the extent.
const REF_EXTENT: f64 = 128.0;

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
    let on = args.content_units || std::env::var("INKVEC_CONTENT_SCALE").is_ok_and(|v| v == "1");
    if !on {
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
/// `precision * s` keeps lambda at ln(REF_EXTENT / precision) whatever the extent,
/// and the further factor of `s` on lambda is the point count: the data term is a
/// sum over boundary samples, there are `s` times as many of them per unit of
/// content, and pricing a parameter in the same currency means scaling its cost
/// by the same `s`. Together with sigma scaled by `s` at the fit, the optimum is
/// the one a 128 px raster of the same shape would reach -- from better points.
pub(crate) fn fit_config(img: &inkvec_trace::Rgba, args: &Args) -> FitConfig {
    let extent = img.width.max(img.height) as f64;
    let s = content_scale(img, args);
    let mut cfg = FitConfig::from_precision(extent, args.precision * s, args.tau);
    cfg.lambda *= s;
    cfg
}

/// A polyline's sigma restated in content units.
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

/// Below this the intake is already at one pixel per unit of detail and is left
/// alone. Every image in the corpus reads exactly 1.00.
const INTAKE_SCALE_FLOOR: f64 = 1.5;
/// Never throw away more than this much, however oversampled the estimate says
/// the input is. A wrong estimate should cost detail slowly, not all at once.
const INTAKE_SCALE_CAP: f64 = 8.0;

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
    if !quiet {
        eprintln!(
            "  intake        {:.2} px per detail unit; tracing at {}x{} and              emitting at {}x{}",
            scale, nw, nh, img.width, img.height
        );
    }
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
    /// The size the SVG is presented at.
    pub display: (usize, usize),
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
    let mut replicated = false;
    if !args.no_unblock {
        if let Some(k) = pixel_grid(&img) {
            if !args.quiet {
                eprintln!(
                    "  unblock       {}x{} is a {k}x pixel upscale of {}x{}; tracing the original",
                    img.width,
                    img.height,
                    img.width / k,
                    img.height / k
                );
            }
            let (nw, nh) = (img.width / k, img.height / k);
            img = inkvec_trace::coverage::downsample_to(&img, nw, nh);
            replicated = true;
        }
    }

    // The cap, intake normalisation and oversample correction run *before* the restore and
    // SR pre-passes, so that a probe trace either `auto` mode makes of an input it keeps is
    // bounded exactly like the real trace. They used to run after the pre-passes' early
    // returns, so `--restore auto` / `--sr auto` kept a probe traced at full resolution and
    // `--max-dim` (and `--intake-scale`) were silently ignored.
    let mut normalised = if replicated {
        true
    } else if !args.intake_scale || args.sr != inkvec_sr::Mode::Off {
        false
    } else {
        let (out, did) = normalise_intake(img, args.quiet);
        img = out;
        did
    };

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
        if !args.quiet {
            eprintln!(
                "  max-dim       {}x{} -> {}x{} for tracing; output keeps {}x{}",
                img.width, img.height, nw, nh, display_w, display_h
            );
        }
        img = inkvec_trace::coverage::downsample_to(&img, nw, nh);
        normalised = true;
    }

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
    // resolution. It reads 1 for every native render, so this does nothing at all to a
    // native intake and the benchmark is untouched by construction.
    let (oversample, redundancy) = {
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
    // measurably carrying surplus pixels cannot make that trade: on a natively rendered
    // intake the round trip reads 1 and nothing moves.
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
    let args = if oversample > 1.0 || floor_scale > 1.0 || redundancy_lambda > 1.0 {
        if !args.quiet {
            if oversample > 1.0 {
                eprintln!(
                    "  intake        oversampled x{oversample:.0}: scaling precision x{oversample:.0}, min-area x{floor_scale:.0}, lambda x{redundancy_lambda:.0}"
                );
            } else {
                eprintln!(
                    "  intake        {redundancy:.0}x more pixels than detail: scaling min-area x{floor_scale:.0} and lambda x{redundancy_lambda:.0}"
                );
            }
        }
        let mut a = args.clone();
        a.precision *= oversample;
        a.min_area *= floor_scale;
        a.lambda_scale *= redundancy_lambda;
        a
    } else {
        args.clone()
    };

    Intake {
        img,
        args,
        replicated,
        normalised,
        display: (display_w, display_h),
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

fn trace_prepared_priced(prepared: Intake) -> Result<Traced, Box<dyn std::error::Error>> {
    let Intake {
        mut img,
        args,
        replicated,
        normalised,
        display: (display_w, display_h),
    } = prepared;
    let args = &args;
    let mut sr_note: Option<String> = None;

    // The restorer comes before SR and before anything that resamples: it was trained on
    // damage at the size the damage happened, and it returns an image of the same size.
    let pass = restore_prepass(img, args)?;
    img = pass.img;
    let restore_note = pass.note;
    let restored = pass.restored;
    let mut probe = pass.probe;
    if args.sr == inkvec_sr::Mode::Off {
        // `auto` kept the input, and nothing else is going to look at it: the probe is the trace.
        if let Some(svg) = probe.take() {
            let svg = if normalised || (img.width, img.height) != (display_w, display_h) {
                retarget(&svg, display_w, display_h)
            } else {
                svg
            };
            return Ok(Traced {
                svg,
                stats: restore_note.into_iter().collect(),
                width: img.width,
                height: img.height,
                lambda: None,
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
    if args.sr == inkvec_sr::Mode::Auto {
        let probe = match probe.take() {
            Some(p) => p,
            None => trace_once(&img, args)?,
        };
        match inkvec_sr::decide(&img, &probe, args.sr_threshold) {
            inkvec_sr::Decision::Keep { residual } => {
                let note = match residual {
                    Some(r) => format!(
                        "sr            residual {r:.3} <= {:.3}, traced directly",
                        args.sr_threshold
                    ),
                    None => "sr            could not measure the fit; traced directly".into(),
                };
                let svg = if normalised || (img.width, img.height) != (display_w, display_h) {
                    retarget(&probe, display_w, display_h)
                } else {
                    probe
                };
                return Ok(Traced {
                    svg,
                    stats: restore_note
                        .into_iter()
                        .chain(std::iter::once(note))
                        .collect(),
                    width: img.width,
                    height: img.height,
                    lambda: None,
                });
            }
            inkvec_sr::Decision::Clean { residual } => {
                sr_note = residual.map(|r| format!("residual {r:.3} > {:.3}", args.sr_threshold));
            }
        }
    }

    if args.sr != inkvec_sr::Mode::Off {
        let up = build_upscaler(args)?;
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
    } else if args.sr != inkvec_sr::Mode::Off {
        (img.width, img.height)
    } else {
        (display_w, display_h)
    };

    // Transparency, once, after every resampling step: put the image against a matte the
    // artwork is not made of and keep the alphas for the emitter. Everything from here
    // traces the matted copy.
    let alpha_src = alpha_source(&img, args.quiet, args.cutout, args.native_alpha);
    let img = alpha_src.as_ref().map(|s| &s.flat).unwrap_or(&img);
    let cut_args = alpha::cutout_args(args, alpha_src.as_ref());
    let args = &*cut_args;

    let (w, h) = (img.width, img.height);

    // Through `fit_config`, not `FitConfig::from_precision` directly, so that
    // `--content-units` applies the whole of its mechanism here and not half of it.
    //
    // It used to build its own configuration and skip the scaling, which left the flag
    // scaling sigma (via `in_content_units`, below) while lambda stayed tied to raw pixel
    // extent -- exactly the half that `fit_config`'s doc comment says must not be applied
    // alone. `content_scale` returns 1.0 unless the flag is set, so this is the identity
    // on the default path; `content_units_changes_the_fit_cost` pins that.
    let cfg = fit_config(img, args);

    // Line art, emitted the way it was drawn. Tried before the ordinary paths and
    // declines by returning None, so anything that is not a stroked drawing is
    // untouched.
    let stroked = if args.strokes {
        run_strokes(img, args, &cfg)
    } else {
        None
    };
    let (svg, mut stats) = if let Some(r) = stroked {
        r
    } else if args.bilevel {
        run_bilevel(img, args, &cfg)
    } else {
        run_color(img, args, &cfg, alpha_src.as_ref())?
    };
    if let Some(n) = sr_note {
        stats.insert(0, n);
    }
    if let Some(n) = restore_note {
        stats.insert(0, n);
    }
    let svg = if normalised || (w, h) != (display_w, display_h) {
        retarget(&svg, display_w, display_h)
    } else {
        svg
    };
    Ok(Traced {
        svg,
        stats,
        width: w,
        height: h,
        lambda: Some(cfg.lambda),
    })
}

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
    if let Some(dir) = std::env::var_os("INKVEC_TOOLS_DIR").map(std::path::PathBuf::from) {
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
    /// The probe trace `--restore auto` made of an input it kept, for SR's `auto` to reuse or,
    /// with SR off, to return as the trace.
    probe: Option<String>,
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
        let probe = trace_once(&pass.img, args)?;
        let decision = inkvec_restore::decide(
            &pass.img,
            &probe,
            inkvec_restore::Options {
                residual_threshold: args.restore_threshold,
            },
        );
        match decision {
            inkvec_restore::Decision::Restore { residual } => {
                pass.note =
                    residual.map(|r| format!("residual {r:.3} > {:.3}", args.restore_threshold));
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
    pass.img = inkvec_restore::restore_rgba(restorer.as_ref(), &pass.img)?;
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
fn trace_once(img: &inkvec_trace::Rgba, args: &Args) -> Result<String, Stop> {
    let cfg = fit_config(img, args);
    if args.bilevel {
        Ok(run_bilevel(img, args, &cfg).0)
    } else {
        // The probe sees what the real trace will see, matte and all.
        match alpha_source(img, true, args.cutout, args.native_alpha) {
            Some(src) => {
                let args = alpha::cutout_args(args, Some(&src));
                Ok(run_color(&src.flat, &args, &cfg, Some(&src))?.0)
            }
            None => Ok(run_color(img, args, &cfg, None)?.0),
        }
    }
}

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

pub mod pipeline;
pub(crate) use pipeline::colour_name;
pub use pipeline::Stop;
pub(crate) use pipeline::{run_bilevel, run_color, run_strokes};
#[cfg(feature = "research-guidance")]
pub use pipeline::{trace_color_from_labels_guided, trace_color_guided};

#[allow(dead_code)]
fn ensure_parent(p: &Path) -> std::io::Result<()> {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
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
}
