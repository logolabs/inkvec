//! The bilevel and full-colour tracing pipelines (the third, centreline strokes, is in
//! [`crate::strokes`]), from an opaque raster to an SVG document and its report lines.
//!
//! The crate root prepares the image (intake, the restorer and SR pre-passes, the alpha
//! matte) and calls [`crate::strokes::run_strokes`], [`run_bilevel`] or [`run_color`];
//! the research entries call the colour pipeline with a hook on the trace. Each pipeline
//! runs the same three steps with its own tracer:
//!
//! * **trace** -- `inkvec_trace` finds the boundaries: bilevel contours, stroke
//!   centrelines, or the colour tracer's palette, labels and planar map of shared edges;
//! * **fit** -- `inkvec_fit` replaces each boundary's measured points by the curve that
//!   minimises the description length `χ²/2 + λ·k` (see [`path_cost`]);
//! * **emit** -- [`crate::emit`] (or [`crate::mono`]) writes the document.
//!
//! Everything the colour pipeline does after its trace is [`finish_color`], split into
//! stages that are documented where they are defined. Output coordinates are the traced
//! raster's pixels, with pixel centres at integers; [`crate::post`] applies the output
//! options afterwards.

use crate::alpha::{self, AlphaRamp, AlphaSource, FaceAlpha};
use crate::args::Args;
use crate::diag;
use crate::editable;
use crate::emit::{emit_bilevel, emit_color, ColorDoc, EmitOptions};
use crate::faces::FaceRings;
use crate::fast;
use crate::mirror_fit;
use crate::mono;
use crate::rings::repair_ring_crossings;
use crate::uncertainty;
use crate::units::{content_scale, in_content_units};
use inkvec_core::Point;
use inkvec_fit::{
    adjust_vertices, choice, curves::Segment, multimodel, optimal_polygon,
    primitives::PrimitiveFit, FitConfig, FittedPath, Segmentation,
};
use inkvec_trace::{gradient, planar, regroup, trace_bilevel, ColorOptions, TraceOptions};

mod demote;
use demote::demote_imperceptible_gradient;

/// The bilevel pipeline (`--bilevel`): the image thresholded to ink and paper, each
/// contour fitted with straight lines only, and written as one even-odd black path.
///
/// The Potrace-comparable mode. `inkvec_trace::trace_bilevel` extracts sub-pixel contours
/// from the coverage field; `optimal_polygon` chooses each contour's vertices by the same
/// MDL objective the colour fit uses, restricted to lines (a dynamic program, globally
/// optimal); `adjust_vertices` then moves each vertex towards the intersection of the lines
/// fitted either side of it, by at most four times the contour's largest sigma (at least a
/// pixel). Returns the SVG and three report lines.
pub(crate) fn run_bilevel(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
) -> (String, Vec<String>) {
    let (w, h) = (img.width, img.height);
    inkvec_core::progress::begin("contours");
    let (contours, field) = trace_bilevel(
        img,
        &TraceOptions {
            // Deliberately NOT in content units. `min_area` is a speckle floor: it removes
            // what noise makes, and noise does not scale with the raster. Scaled by s² it
            // was 32 px² at 512 and swallowed the 1-2 px gaps between a scribble's strokes
            // (abra_agency: tile error 602 -> 371 with the floor back in pixels; 60 real
            // brands at 512: dE00 worst 1.33 -> 0.96, p99 0.73 -> 0.58, params 0.69x -> 0.92x
            // of the artist's, 2026-09-05). The fit tolerances stay in content units.
            min_area: args.min_area,
        },
    );
    let measured: usize = contours.iter().map(|c| c.len()).sum();
    let s_content = content_scale(img, args);
    let contours: Vec<inkvec_core::Polyline> = contours
        .iter()
        .map(|c| in_content_units(c, s_content))
        .collect();
    inkvec_core::progress::begin("fit_dp");
    let segs: Vec<Segmentation> = contours.iter().map(|c| optimal_polygon(c, cfg)).collect();
    let anchors: usize = segs.iter().map(|s| s.segment_count()).sum();

    let paths: Vec<Vec<Point>> = contours
        .iter()
        .zip(&segs)
        .map(|(c, s)| {
            let max_shift = 4.0 * c.sigma.iter().copied().fold(0.0, f64::max).max(0.25);
            adjust_vertices(c, s, max_shift)
        })
        .collect();

    // Under monochrome the knock-out in `post_process` stands aside, so the paper is left
    // out here instead.
    let paper = !(args.monochrome && args.no_background);
    inkvec_core::progress::begin("emit");
    let svg = emit_bilevel(&paths, w, h, args.precision, paper);
    (
        svg,
        vec![
            format!(
                "coverage      sigma_alpha {:.5}   resolvedness {:.2}",
                field.sigma_alpha, field.saturation
            ),
            format!("contours      {}", contours.len()),
            format!(
                "anchors       {anchors}  (from {measured} measured points, {:.1}x reduction)",
                measured as f64 / anchors.max(1) as f64
            ),
        ],
    )
}

/// The colour pipeline, the default: [`run_color_impl`] with no research hook.
///
/// `img` is opaque (matted by the caller when the source had alpha, the true alphas in
/// `alpha_src`) and `cfg` comes from [`crate::units::fit_config`].
pub(crate) fn run_color(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
    alpha_src: Option<&AlphaSource>,
) -> Result<(String, Vec<String>), Stop> {
    run_color_impl(img, args, cfg, alpha_src, |_| {})
}

/// Research-only access to shared geometry between raster tracing and fitting.
/// The caller must preserve edge incidence, junction endpoints, and uncertainty.
#[cfg(feature = "research-guidance")]
pub fn trace_color_guided(
    img: &inkvec_trace::Rgba,
    args: &Args,
    guide: impl FnOnce(&mut inkvec_trace::ColorTrace),
) -> Result<(String, Vec<String>), Stop> {
    run_color_impl(img, args, &crate::units::fit_config(img, args), None, guide)
}

/// The tracer options the colour path runs with, as the command line asks for them, for a
/// `width` x `height` raster (the size decides whether `balanced` adds its boundary solve,
/// [`fast::solve_iters`]).
///
/// Extracted so that every entry into the colour tracer -- the shipped one and the
/// research ones below -- hands it the same options, rather than a hand-copied
/// approximation that drifts the first time a flag is added. The solve's wall-clock budget
/// is Quality's alone: balanced caps its solve by iterations, so its output does not depend
/// on the machine.
pub(crate) fn color_options(args: &Args, width: usize, height: usize) -> ColorOptions {
    let (deadline, boundary_ms) = if args.time_budget > 0.0 {
        (
            Some(
                inkvec_core::clock::Instant::now()
                    + std::time::Duration::from_secs_f64(args.time_budget * 0.6),
            ),
            // Only Quality's solve reads a clock; Fast runs none and balanced caps its own by
            // iterations. (Fast reads neither value, so this changes nothing there.)
            (!fast::on(args)).then(|| (args.time_budget * 0.25 * 1000.0).max(50.0) as u64),
        )
    } else {
        (None, None)
    };
    ColorOptions {
        merge_distance: args.merge_distance,
        max_colors: args.max_colors,
        // `Auto` should already have been resolved by `resolve_lossy`; if a caller skipped
        // that, treating it as "not known to be lossy" keeps a clean intake untouched.
        lossy_intake: args.lossy == inkvec_sr::Mode::On,
        alpha_inks: args.cutout,
        native_alpha: args.native_alpha,
        simplify_faint: args.simplify_faint,
        deadline,
        boundary_ms,
        // A 2px floor is appropriate for the continuous bilevel path, but in colour
        // mode it lets anti-aliased edge samples become their own tiny faces. Those faces
        // split a smooth shared boundary into a literal pixel staircase before fitting.
        // Nine pixels removes that confetti on the grid canaries while long thin artwork
        // remains intact; an explicit --min-area always wins for deliberately tiny art.
        // A 9-pixel colour-mode floor was tried (2026-09-03) to absorb anti-aliased
        // stair-step faces and measured worse on the full 980-icon set: objective
        // 0.7885 vs 0.7673 with the 2 px floor, 508 icons worse / 171 better in dE00 -
        // it erased real dots and thin details more often than it removed confetti.
        // Blend absorption and residual carving handle the stair-steps at the label
        // level instead. The default therefore stays at the --min-area value (2 px).
        // An area: `price_in_raster_units` scales it by the round trip squared, not here.
        min_region: args.min_area.max(1.0) as usize,
        gradients: !args.no_gradients,
        fast: fast::on(args),
        absorb_blends: args.absorb_blends,
        boundary_iters: fast::solve_iters(args, width, height),
    }
}

/// Research-only: trace from a label map the caller produced, e.g. a network's.
///
/// The palette and labelling stages are replaced wholesale by `labels`; everything from
/// the planar map onwards, and the whole of the fit/repair/emit path below, is the
/// shipped one. See [`inkvec_trace::trace_color_from_labels`].
#[cfg(feature = "research-guidance")]
pub fn trace_color_from_labels_guided(
    img: &inkvec_trace::Rgba,
    args: &Args,
    labels: &[u16],
    n_labels: usize,
    guide: impl FnOnce(&mut inkvec_trace::ColorTrace),
) -> Result<(String, Vec<String>), Stop> {
    let opts = color_options(args, img.width, img.height);
    let mut sw = inkvec_trace::Stopwatch::start();
    let mut traced = inkvec_trace::trace_color_from_labels(img, &opts, labels, n_labels);
    guide(&mut traced);
    sw.mark("trace_total");
    finish_color(
        img,
        args,
        &crate::units::fit_config(img, args),
        None,
        traced,
    )
}

/// The colour pipeline: colour groups (`--merge-colors`) applied, the colour trace, the
/// research hook `guide` on the finished trace, then [`finish_color`]. The colour-group
/// report lines come first in the report.
///
/// Colour groups name fills a person saw in a trace, so a first trace finds them, the
/// image is recoloured by `regroup::apply`, and the real trace runs on the recoloured
/// image.
pub(crate) fn run_color_impl(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
    alpha_src: Option<&AlphaSource>,
    guide: impl FnOnce(&mut inkvec_trace::ColorTrace),
) -> Result<(String, Vec<String>), Stop> {
    let opts = color_options(args, img.width, img.height);
    let mut sw = inkvec_trace::Stopwatch::start();
    // Colour groups recolour the image, and everything after -- the trace and the fit that
    // checks itself against the pixels -- sees the recoloured one. See `regroup`.
    let regrouped;
    let (img, merge_report) = if args.merge_colors.is_empty() {
        (img, Vec::new())
    } else {
        // The groups name fills the caller saw in a trace, so they are found in one.
        let first = inkvec_trace::trace_color_full_with_alpha(
            img,
            &opts,
            alpha_src.map(|a| a.alpha.as_slice()),
        );
        inkvec_core::progress::begin("merge_colors");
        let (out, outcomes) = regroup::apply(img, &first, &args.merge_colors);
        sw.mark("merge_colors");
        regrouped = out;
        (&regrouped, merge_lines(&outcomes))
    };
    let mut traced = inkvec_trace::trace_color_full_with_alpha(
        img,
        &opts,
        alpha_src.map(|a| a.alpha.as_slice()),
    );
    guide(&mut traced);
    sw.mark("trace_total");
    let (svg, mut report) = finish_color(img, args, cfg, alpha_src, traced)?;
    report.splice(0..0, merge_report);
    Ok((svg, report))
}

/// One report line per colour group: what merged into what, and what matched nothing.
fn merge_lines(outcomes: &[regroup::GroupOutcome]) -> Vec<String> {
    let hex = |c: [f32; 3]| {
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
    };
    outcomes
        .iter()
        .map(|o| {
            let mut line = match (o.target, o.gradient) {
                (Some(t), _) => {
                    let fills: Vec<String> = o.merged.iter().map(|&c| hex(c)).collect();
                    format!("merge colors  {} -> {}", fills.join(" + "), hex(t))
                }
                (None, true) => {
                    let fills: Vec<String> = o.merged.iter().map(|&c| hex(c)).collect();
                    format!("merge colors  {} -> one gradient", fills.join(" + "))
                }
                (None, false) => {
                    "merge colors  group left alone (fewer than two fills found)".to_string()
                }
            };
            if !o.unmatched.is_empty() {
                let missing: Vec<String> = o
                    .unmatched
                    .iter()
                    .map(|m| match m {
                        regroup::Member::Flat(c) => hex(*c),
                        regroup::Member::Gradient(s) => {
                            s.iter().map(|&c| hex(c)).collect::<Vec<_>>().join(">")
                        }
                    })
                    .collect();
                line.push_str(&format!("; not found in the trace: {}", missing.join(", ")));
            }
            line
        })
        .collect()
}

/// Why the colour pipeline stopped before producing an SVG.
///
/// Returned instead of exiting the process, so a library caller (the WASM build, the desk
/// app, a research harness) keeps control. Only the command line turns it into an exit code.
#[derive(Debug)]
pub enum Stop {
    /// The image is a single flat colour and `Args::strict` asked for that to be an error.
    FlatInput,
    /// `INKVEC_DUMP_MAP` asked for the planar map to be written instead of a trace, and it
    /// was written to this path.
    MapDumped(std::path::PathBuf),
    /// `INKVEC_DUMP_MAP` asked for the planar map, and writing it failed.
    MapDumpFailed(String),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stop::FlatInput => {
                write!(
                    f,
                    "nothing to trace: the image is a single flat colour (--strict)"
                )
            }
            Stop::MapDumped(p) => write!(f, "planar map written to {}", p.display()),
            Stop::MapDumpFailed(e) => write!(f, "could not write the planar map dump: {e}"),
        }
    }
}

impl std::error::Error for Stop {}
/// Everything after the trace: fit each boundary, repair crossings, choose fills, and
/// emit. It reads nothing but the `ColorTrace` and the raster, which is what lets a
/// research entry swap the tracer out and keep the rest of the pipeline intact.
///
/// The stages, each a function below:
///
/// 1. [`dump_map`] -- `INKVEC_DUMP_MAP` stops here with the planar map written out.
/// 2. [`fit_boundaries`] -- one MDL fit per boundary, or a primitive where that is cheaper.
/// 3. [`repair_fits`] -- refit boundaries whose assembled rings cross themselves.
/// 4. [`apply_mirrors`] -- mirrored boundary pairs made to agree exactly.
/// 5. [`final_fills`] -- each face's fill model, imperceptible gradients demoted to flat.
/// 6. [`face_transparency`] -- what the emitter needs to know about alpha, per face.
/// 7. `--editability` passes and `--monochrome`, when asked for.
/// 8. [`write_colour`] -- the document, flat or with translucent layers.
///
/// `img` is the raster that was traced (already matted when it had transparency, with the
/// true alphas in `alpha_src`); `cfg` the fit configuration from [`crate::units::fit_config`].
/// Returns the SVG and the report lines, or a [`Stop`].
fn finish_color(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
    alpha_src: Option<&AlphaSource>,
    traced: inkvec_trace::ColorTrace,
) -> Result<(String, Vec<String>), Stop> {
    let (w, h) = (img.width, img.height);
    if let Some(stop) = dump_map(&traced, w, h) {
        return Err(stop);
    }
    if traced.palette.len() <= 1 {
        diag::warn(|| "warning: nothing to trace -- the image is a single flat colour".into());
        if args.strict {
            return Err(Stop::FlatInput);
        }
    }
    let mut sw = inkvec_trace::Stopwatch::start();
    inkvec_core::progress::begin("fit_dp");
    // Moved out, not cloned: nothing reads `traced.labels` again (the trace is taken apart
    // field by field below), and a 2048 px image's labels are 8 MB (1.28 ms to copy).
    let traced_labels = traced.labels;
    let boundary_report = traced.boundary_opt;
    let symmetry = traced.symmetry;
    let symmetrised = traced.symmetrised;
    let (map, pal, face_color, face_fill) = (
        traced.map,
        traced.palette,
        traced.face_color,
        traced.face_fill,
    );

    let measured: usize = map.edges.iter().map(|e| e.points.len()).sum();
    let fast = fast::on(args);
    let (order, fits, repaired) =
        fit_and_repair(img, args, cfg, &map, &face_fill, &symmetry, fast, &mut sw);
    let Fits {
        polys,
        mut fitted,
        mut prims,
        ..
    } = fits;

    let (n_line, n_cubic) = segment_counts(&fitted);
    let mirrored_fits = apply_mirrors(&symmetry, &mut fitted, &mut prims);
    let fills = final_fills(face_fill, &face_color, &pal, args.no_gradients);
    sw.mark("fills");
    inkvec_core::progress::begin("emit");

    let layers = alpha::recover_layers(args, &map, &face_color, &fills, &pal, &traced_labels);
    let alpha = face_transparency(
        img,
        args,
        alpha_src,
        &face_color,
        &pal,
        &traced_labels,
        &traced.face_fade,
    );

    // Editability mode: post-fit structure passes, every one guarded to the ring's own
    // tolerance. Runs after repair and harmonization so nothing re-breaks what was locked.
    if args.editability {
        let stats = editable::edit_all(&polys, &mut fitted, &prims);
        diag::stage(args.quiet, || stats.summary());
    }
    let doc = ColorDoc {
        order: &order,
        fitted: &fitted,
        prims: &prims,
        fill_fits: &fills,
        pal: &pal,
        face_color: &face_color,
        clear: &alpha.clear,
        opacity: &alpha.opacity,
        alpha_ramps: &alpha.alpha_ramps,
        fades: &alpha.fades,
        layers: None,
        matte: alpha.matte,
        ribbons: &Default::default(),
        w,
        h,
    };
    // Stroke detection (on by default, `--no-detect-strokes` off): stroke-drawn faces written as strokes.
    let ribbons = crate::ribbons::stage(args, cfg, fast, &doc, &map, &traced_labels);
    let doc = doc.with_ribbons(&ribbons);
    // Monochrome: the ink faces as one black shape, from the same fitted edges. It replaces
    // the colour document, and with it the layer form, which only ever repaints colours.
    let (svg, mono_line) = if args.monochrome {
        let (svg, line) = monochrome(&doc, &map, &traced_labels, args);
        (svg, Some(line))
    } else {
        let opts = emit_options(args, fast);
        (write_colour(&doc, &opts, &map, layers, args.quiet), None)
    };
    sw.mark("emit");
    write_uncertainty(args, &svg, &map);

    let report = report_lines(
        args,
        fast.then_some((w, h)),
        mono_line,
        ColourReport {
            palette: pal.len(),
            faces: face_color.len(),
            gradients: gradient_count(&fills),
            edges: map.edges.len(),
            primitives: prims.iter().filter(|p| p.is_some()).count(),
            boundary: boundary_report.as_ref(),
            symmetry: &symmetry,
            symmetrised,
            mirrored_fits,
            repaired,
            n_line,
            n_cubic,
            measured,
        },
    );
    Ok((svg, report))
}

/// How many faces are filled with a gradient rather than a flat colour.
fn gradient_count(fills: &[gradient::FillFit]) -> usize {
    fills
        .iter()
        .filter(|f| !matches!(f.model, gradient::FillModel::Flat(_)))
        .count()
}

/// Stages 2 and 3 with their progress and timing marks: fit every boundary, then repair
/// crossing rings. Returns the rings of each face, the fits, and how many boundaries the
/// repair refitted.
#[allow(clippy::too_many_arguments)]
fn fit_and_repair(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
    map: &planar::PlanarMap,
    face_fill: &[gradient::FillFit],
    symmetry: &inkvec_trace::symmetry::Symmetry,
    fast: bool,
    sw: &mut inkvec_trace::Stopwatch,
) -> (Vec<FaceRings>, Fits, usize) {
    let mut fits = fit_boundaries(img, args, cfg, map, face_fill, symmetry, fast);
    sw.mark("fit_dp");
    inkvec_core::progress::begin("repair");
    report_ring_times(fits.ring_times.as_deref());

    // The rings depend only on the map, so they are walked once, before the repair that
    // needs them to find crossings. (They used to be taken before the since-removed polish
    // stage too, so that a boundary the repair refitted was polished afterwards.)
    let order = planar::face_edge_order(map);
    let repaired = repair_fits(&order, &mut fits, args, cfg, fast);
    sw.mark("repair");
    (order, fits, repaired)
}

/// The colour pipeline's report: fast or balanced mode's line first when the Fast engine
/// ran (`fast` is then the traced raster's size), then the monochrome line when there is
/// one, then the stage lines of `r`.
fn report_lines(
    args: &Args,
    fast: Option<(usize, usize)>,
    mono_line: Option<String>,
    r: ColourReport,
) -> Vec<String> {
    let mut report = match fast {
        Some((w, h)) => vec![fast::report(args, w, h)],
        None => Vec::new(),
    };
    report.extend(mono_line);
    report.extend(r.lines());
    report
}

/// The emitter's options, from the command line. Harmonizing is skipped in fast mode.
fn emit_options(args: &Args, fast: bool) -> EmitOptions {
    EmitOptions {
        cutout: args.cutout,
        native: args.native_alpha,
        no_background: args.no_background,
        precision: args.precision,
        harmonize: args.harmonize && !fast,
        harmonize_threshold: args.harmonize_threshold,
        use_symbols: args.use_symbols,
    }
}

/// `--monochrome`: every face classified as ink or ground ([`crate::mono`]) and the ink
/// written as one black even-odd path, from the same fitted edges `doc` holds. Returns the
/// document and its report line.
fn monochrome(
    doc: &ColorDoc,
    map: &planar::PlanarMap,
    labels: &[u16],
    args: &Args,
) -> (String, String) {
    let ground = mono::classify(&mono::Trace {
        map,
        order: doc.order,
        fitted: doc.fitted,
        labels,
        fills: doc.fill_fits,
        face_color: doc.face_color,
        pal: doc.pal,
        clear: doc.clear,
        opacity: doc.opacity,
    });
    let svg = mono::emit(
        map,
        &ground.ink,
        doc.fitted,
        doc.prims,
        args.no_background,
        doc.w,
        doc.h,
        args.precision,
    );
    (svg, mono::report(&ground))
}

/// `INKVEC_DUMP_MAP=<path>`: write the planar map and stop, for label generation, which
/// needs the map and nothing after it.
///
/// It writes the map in a compact binary form and stops before the curve fit, which is
/// half the run. Format (little endian): u32 w, h, n_labels, n_edges; then per edge u32
/// left, right, u8 closed, u32 n, then n x (f32 x, f32 y, f32 sigma), coordinates in px of
/// the traced raster. Same intake as a trace. Returns the [`Stop`] to hand back -- written,
/// or the write failed -- or `None` when the variable is not set.
fn dump_map(traced: &inkvec_trace::ColorTrace, w: usize, h: usize) -> Option<Stop> {
    let path = inkvec_core::env::path("INKVEC_DUMP_MAP")?;
    let mut buf: Vec<u8> = Vec::new();
    let m = &traced.map;
    for v in [w as u32, h as u32, m.n_labels as u32, m.edges.len() as u32] {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    for e in &m.edges {
        buf.extend_from_slice(&(e.left as u32).to_le_bytes());
        buf.extend_from_slice(&(e.right as u32).to_le_bytes());
        buf.push(u8::from(e.closed));
        buf.extend_from_slice(&(e.points.len() as u32).to_le_bytes());
        for (k, p) in e.points.iter().enumerate() {
            let s = e.sigma.get(k).copied().unwrap_or(0.1);
            buf.extend_from_slice(&(p.x as f32).to_le_bytes());
            buf.extend_from_slice(&(p.y as f32).to_le_bytes());
            buf.extend_from_slice(&(s as f32).to_le_bytes());
        }
    }
    Some(match std::fs::write(&path, buf) {
        Ok(()) => Stop::MapDumped(path),
        Err(e) => Stop::MapDumpFailed(e.to_string()),
    })
}

/// Every boundary of the planar map fitted: what [`fit_boundaries`] produces and
/// [`repair_fits`] refines. All vectors are indexed by edge.
struct Fits {
    /// Each edge's measured points, sigmas in content units: the fit's input. Empty in fast
    /// mode unless `--editability` or the research structural baseline reads it (see
    /// [`fit_boundaries`]).
    polys: Vec<inkvec_core::Polyline>,
    /// Each edge's lambda multiplier: its own scale times `--lambda-scale`. Empty whenever
    /// `polys` is.
    lambda_scales: Vec<f64>,
    /// Each edge's fitted curve.
    fitted: Vec<FittedPath>,
    /// Each edge's whole-boundary primitive, where one won.
    prims: Vec<Option<PrimitiveFit>>,
    /// Research builds under `INKVEC_STRUCTURAL` only: the same fit without the structural
    /// simplifier, for the transactional comparison in [`repair_fits`].
    structural_baseline: Option<(Vec<FittedPath>, Vec<Option<PrimitiveFit>>)>,
    /// Under `INKVEC_TIMING`, (milliseconds, points) of each boundary's dynamic program.
    ring_times: Option<Vec<(f64, usize)>>,
}

/// `cfg` with its lambda multiplied by `scale`: one boundary's own exchange rate.
fn scaled(cfg: &FitConfig, scale: f64) -> FitConfig {
    FitConfig {
        lambda: cfg.lambda * scale,
        ..*cfg
    }
}

/// Stage 2: fit every boundary once, on every core.
///
/// Each boundary is fitted exactly once, with a mixed line/cubic alphabet chosen by
/// MDL. Both faces that touch it then reference the same fitted curve — that is what
/// makes seams unrepresentable rather than merely rare. One global dynamic program per
/// boundary, with lines and cubics in the same alphabet, replaced an earlier two-pass
/// fitter (line DP for corners, then a swept kurbo fit per run), which was both slower — a
/// 96-point smoothing/tolerance sweep per run, enough to time out on complex inputs — and,
/// on smooth lobed shapes, catastrophically wrong: it reached 20px of deviation on a star
/// where this reaches 0.09px. Each fit then competes with a whole-boundary primitive
/// ([`prefer_primitive`]).
///
/// A boundary that is its own mirror image (`symmetry.self_mirrors`) is fitted by
/// [`mirror_fit::choose`]: its ordinary fit when that is already symmetric, else the fit of
/// one side of the axis and its reflection, when that is no dearer. Mirror-paired
/// boundaries are made to agree afterwards, by [`apply_mirrors`].
///
/// Fast mode fits nothing here: `fast::fit` turns the map's edges into paths directly.
/// It reads neither the content-unit polylines nor the per-edge λ multipliers, so it does
/// not build them unless `--editability` (which reads the polylines) or the research
/// structural baseline asks for them; `Fits::polys` and `Fits::lambda_scales` are then
/// empty. Building them was 0.47 ms of the fit stage at 2048 px, plus the content scale's
/// two raster passes under `--content-units`, which Fast ignores. The fitted paths do not
/// depend on either, so the output is unchanged. Not from the literature: this only skips
/// work nothing in Fast reads.
fn fit_boundaries(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
    map: &planar::PlanarMap,
    face_fill: &[gradient::FillFit],
    symmetry: &inkvec_trace::symmetry::Symmetry,
    fast: bool,
) -> Fits {
    let structural = cfg!(feature = "research") && inkvec_core::env::flag("INKVEC_STRUCTURAL");
    let need_polys = !fast || args.editability || structural;
    let polys: Vec<inkvec_core::Polyline> = if need_polys {
        let s_content = content_scale(img, args);
        map.edges
            .iter()
            .map(|e| in_content_units(&e.as_polyline(), s_content))
            .collect()
    } else {
        Vec::new()
    };
    // What a parameter costs on *this* boundary. `lambda` is one exchange rate for the
    // whole drawing, which prices a boundary the artist lavished detail on exactly like a
    // plain straight run; the per-edge scale is where a predictor of local parameter
    // density is allowed to disagree with that average, and the global one is the leeway
    // over the drawing as a whole. Both are 1.0 unless something set them, and at 1.0
    // `cfg_k` is `cfg`, so the fit is unchanged.
    let lambda_scales: Vec<f64> = if need_polys {
        map.edges
            .iter()
            .map(|e| e.lambda_scale * args.lambda_scale)
            .collect()
    } else {
        Vec::new()
    };
    // Every boundary is fitted independently, so fit them on every core. The work per
    // edge varies by orders of magnitude (a two-point sliver against a thousand-point
    // outline), which is exactly the shape of problem rayon's work stealing handles.
    use rayon::prelude::*;
    let ring_timing = inkvec_core::env::flag("INKVEC_TIMING");
    let ring_times: std::sync::Mutex<Vec<(f64, usize)>> = std::sync::Mutex::new(Vec::new());
    inkvec_core::progress::step("boundaries fitted", 0, map.edges.len() as u64);
    let live = inkvec_core::progress::handle();
    let results: Vec<(FittedPath, Option<PrimitiveFit>)> = if fast {
        fast::fit(args, map, face_fill)
    } else {
        polys
            .par_iter()
            .zip(lambda_scales.par_iter())
            .enumerate()
            .map(|(k, (poly, &scale))| {
                // A cancelled trace stops at the next boundary; the count moves as each
                // one is finished.
                live.check();
                let poly = poly.clone();
                // This boundary's own exchange rate. `cfg_k == *cfg` when the scale is 1.0,
                // which is every path but the guided one.
                let cfg_k = scaled(cfg, scale);
                // The image frame's rectangle can be proved cheapest without its dynamic
                // program, which is then not run (`inkvec_fit::choice`).
                let frame =
                    poly.closed && choice::lies_on_frame(&poly.points, map.width, map.height);
                let fitted = choice::describe(&poly, &cfg_k, frame, || {
                    let t_ring = inkvec_core::clock::Instant::now();
                    // Scoped, so the dynamic program itself can stop a trace nobody wants in
                    // the middle of a boundary of thousands of points.
                    // A boundary that is its own mirror image keeps its ordinary fit when
                    // that is symmetric, else is fitted on one side of the axis and
                    // reflected when that is no dearer (`mirror_fit::choose`); the rest as
                    // before.
                    let plain = |p: &inkvec_core::Polyline| {
                        live.scoped(|| multimodel::optimal_multimodel(p, &cfg_k))
                    };
                    let mirrors = symmetry.self_mirrors(k);
                    let curve = if mirrors.is_empty() {
                        plain(&poly)
                    } else {
                        mirror_fit::choose(&poly, &mirrors, &cfg_k, &plain)
                    };
                    if ring_timing {
                        ring_times
                            .lock()
                            .expect("nothing panics while holding the ring timer")
                            .push((t_ring.elapsed().as_secs_f64() * 1e3, poly.points.len()));
                    }
                    curve
                });
                live.tick();
                fitted
            })
            .collect()
    };
    let mut prims: Vec<Option<PrimitiveFit>> = Vec::with_capacity(results.len());
    let mut fitted: Vec<FittedPath> = Vec::with_capacity(results.len());
    for (f, pr) in results {
        fitted.push(f);
        prims.push(pr);
    }
    // A local merge can trigger a more expensive global crossing repair. Keep
    // an independent baseline through primitive selection and repair so the
    // trial is judged on the geometry that those stages actually return.
    // The structural simplifier's transactional baseline: research builds only.
    let structural_baseline = if structural {
        let results: Vec<_> = polys
            .par_iter()
            .zip(lambda_scales.par_iter())
            .map(|(poly, &scale)| {
                let cfg_k = scaled(cfg, scale);
                let curve = multimodel::optimal_multimodel_without_structural(poly, &cfg_k);
                prefer_primitive(poly, curve, &cfg_k)
            })
            .collect();
        let (paths, primitives): (Vec<_>, Vec<_>) = results.into_iter().unzip();
        Some((paths, primitives))
    } else {
        None
    };
    Fits {
        polys,
        lambda_scales,
        fitted,
        prims,
        structural_baseline,
        ring_times: ring_timing.then(|| {
            ring_times
                .into_inner()
                .expect("nothing panics while holding the ring timer")
        }),
    }
}

/// A boundary's fitted `curve`, or the whole-boundary primitive (circle, ellipse, rounded
/// rectangle, or a run of arcs) when that describes `poly` more cheaply.
///
/// A boundary that *is* a circle should be described as one. Both candidates are scored by
/// the same MDL cost ([`path_cost`]), so the three numbers of a circle beat the
/// twenty-four of four cubics whenever the evidence actually supports a circle, and lose
/// when it does not. It is worth a great deal: measured by taking this whole-boundary
/// primitive path out (the `INKVEC_NO_PRIMITIVE` ablation, since removed) over the 246-icon
/// gate set, removing it costs 30.52% of the parameter ratio (1.4818 -> 1.9341) and 9.01%
/// of dE00, far more than any other lever measured on this tree.
///
/// [`fit_boundaries`] reaches the same choice through `inkvec_fit::choice::describe`,
/// which also avoids work the choice discards; this plain form serves the research
/// structural baseline.
fn prefer_primitive(
    poly: &inkvec_core::Polyline,
    curve: FittedPath,
    cfg: &FitConfig,
) -> (FittedPath, Option<PrimitiveFit>) {
    choice::choose(poly, curve, choice::primitive_offer(poly, cfg), cfg)
}

/// `INKVEC_TIMING`: the total time of the boundary fits and the five slowest boundaries.
fn report_ring_times(ring_times: Option<&[(f64, usize)]>) {
    let Some(rt) = ring_times else {
        return;
    };
    let mut rt = rt.to_vec();
    rt.sort_by(|a, b| b.0.total_cmp(&a.0));
    let total: f64 = rt.iter().map(|r| r.0).sum();
    let top: Vec<String> = rt
        .iter()
        .take(5)
        .map(|(ms, n)| format!("{ms:.0} ms/{n} pts"))
        .collect();
    diag::debug(true, || {
        format!(
            "  [t] fit_dp rings: {} rings, {:.0} ms of ring work, top {}",
            rt.len(),
            total,
            top.join(", ")
        )
    });
}

/// Stage 3: refit whichever boundaries make a ring cross itself, and return how many were
/// refitted. Skipped under `--no-repair` and in fast mode.
///
/// A self-crossing boundary is invisible to the objective — both curves pass through
/// their measured points and the render barely changes — but the ring it produces is
/// invalid and unpleasant to edit. Measured over 180 real emoji, 53 (29%) emitted at
/// least one, against VTracer's 10 (5.6%), and classifying 2126 rings showed *none*
/// were a single cubic looping: every one is two different edges of a face crossing
/// after each was fitted within its own tolerance. See [`crate::rings::repair_ring_crossings`].
///
/// This runs *after* polish, not before. Polish moves control points by up to a pixel,
/// and a thin neck is exactly where that reopens a crossing the repair had closed —
/// measured over 40 emoji, repairing first left 12 invalid rings where repairing last
/// leaves 9. The repair has to see the geometry that is actually emitted. (Polish itself
/// has since been removed; see the note below.)
///
/// In research builds with a structural baseline, the baseline is repaired too and kept
/// instead unless [`structural_trial_improves`] says the simplified fit is strictly better.
fn repair_fits(
    order: &[FaceRings],
    fits: &mut Fits,
    args: &Args,
    cfg: &FitConfig,
    fast: bool,
) -> usize {
    // Removed: two stages that refined curves against the image after the fit, and a
    // third switch that turned one of them off.
    //
    // `polish` nudged each cubic's control points against the coverage field; the
    // adjudication stage re-scored chord runs against the raster and upgraded the ones
    // the image said were bending. Both were measured wins when they were written, and
    // both are now harmful, because `boundary_opt::optimise` solves the whole boundary
    // against that same image before the fit ever runs. Refining locally afterwards
    // partly undoes a better global answer.
    //
    // Ablated on 246 icons, then confirmed on all 980 (paired, 2026-09-03): removing them
    // takes the objective from 0.5477 to 0.4960 and dE00 from 0.2503 to 0.2146, with 799
    // icons better and 86 worse, and every family improving. Polish alone made 197 of 246
    // better by not running. They cost 580 lines and eleven per cent of the objective.
    //
    // What went with them: `inkvec_fit::polish`, `inkvec_fit::smooth`, `--no-polish`, and
    // the INKVEC_IMGADJ / ADJ_SIGMA / ADJ_MARGIN / ADJ_DEVCAP knobs. The lesson worth
    // keeping is the one that justified the adjudication in the first place: the dynamic
    // program cannot tell a gently curving boundary from a chain of chords using the
    // extracted points alone, because the extraction error is *correlated along* the
    // boundary. The answer to that is to put the question to the image - which is now
    // what the boundary solve does, for every point at once rather than per run.

    // Removed: a joint image-evidence solve for shared junctions, off by default and
    // deleted with its two modules (`inkvec_fit::junctions`, `inkvec_trace::render`).
    // A junction is one topological point shared by all its incident boundaries, and
    // solving it from all of them at once instead of letting one edge's local fit decide
    // is the right idea — but `boundary_opt::optimise` above now does that job for the
    // whole boundary, not just its nodes, and does it by default. The node-only solve
    // measured 0.0002 on the objective over 980 icons (0.7255 against 0.7253), which is
    // not a difference. Two lessons from it are worth keeping. Arcs had to be frozen
    // during the solve because their radius is a coupled variable that a two-coordinate
    // node solve cannot see. And a move proposed from local normal evidence had to be
    // judged by a full-scene render before it was accepted: at dense junctions a move
    // can lower every incident one-dimensional residual and still make a neighbouring
    // face's raster coverage worse.
    let cfg_repair = scaled(cfg, args.lambda_scale);
    let mut repaired = if args.no_repair || fast {
        0
    } else {
        repair_ring_crossings(order, &mut fits.fitted, &fits.polys, &cfg_repair)
    };
    if let Some((mut baseline, baseline_prims)) = fits.structural_baseline.take() {
        let baseline_repaired = if args.no_repair {
            0
        } else {
            repair_ring_crossings(order, &mut baseline, &fits.polys, &cfg_repair)
        };
        if !structural_trial_improves(
            &fits.polys,
            &fits.fitted,
            &baseline,
            &fits.lambda_scales,
            cfg,
        ) {
            fits.fitted = baseline;
            fits.prims = baseline_prims;
            repaired = baseline_repaired;
        }
    }
    repaired
}

/// The number of line and of cubic segments across all fitted boundaries (arcs are
/// counted in neither).
fn segment_counts(fitted: &[FittedPath]) -> (usize, usize) {
    let count = |want: fn(&Segment) -> bool| {
        fitted
            .iter()
            .flat_map(|f| f.segments.iter())
            .filter(|s| want(s))
            .count()
    };
    (
        count(|s| matches!(s, Segment::Line(..))),
        count(|s| matches!(s, Segment::Cubic(..))),
    )
}

/// Stage 4: make mirrored boundaries agree exactly. Returns how many fits were reflected
/// or centred.
///
/// Every stage above is free to break a mirror by breaking a tie, and the fitter breaks
/// them most of all: a dynamic program walks one boundary forwards and its reflection
/// backwards, and can segment them differently. So the fits are made to agree here,
/// after everyone else has finished: one boundary of each mirrored pair keeps its own
/// fit, and its partner takes that fit reflected (and reversed where the pairing walks it
/// the other way). Exact by construction, and no search.
fn apply_mirrors(
    symmetry: &inkvec_trace::symmetry::Symmetry,
    fitted: &mut [FittedPath],
    prims: &mut [Option<PrimitiveFit>],
) -> usize {
    let mut mirrored_fits = 0usize;
    if symmetry.is_empty() {
        return mirrored_fits;
    }
    for k in 0..fitted.len() {
        // A shape that is its own reflection keeps its fit; where that fit is a
        // primitive, centring it on the axis makes it symmetric exactly.
        for m in symmetry.self_mirrors(k) {
            if let Some(pf) = prims.get(k).and_then(|p| p.as_ref()) {
                let centred = inkvec_trace::symmetry::centre_primitive(pf, m);
                prims[k] = Some(centred);
                mirrored_fits += 1;
            }
        }
        let Some((m, pr)) = symmetry.mirror_of(k) else {
            continue;
        };
        if pr.edge >= fitted.len() {
            continue;
        }
        let mut path = inkvec_trace::symmetry::reflect_path(&fitted[pr.edge], m);
        if pr.rev {
            path = path.reversed();
        }
        fitted[k] = path;
        prims[k] = prims[pr.edge]
            .as_ref()
            .map(|pf| inkvec_trace::symmetry::reflect_primitive(pf, m));
        mirrored_fits += 1;
    }
    mirrored_fits
}

/// Stage 5: each face's fill model as it will be written.
///
/// A region shaded by a gradient is one region, not a stack of flat bands: fitting it as a
/// gradient replaces a pile of near-duplicate faces with four numbers and two stops, and
/// the same MDL objective decides whether the evidence supports that. What the tracer
/// chose is kept, except that `--no-gradients` paints every face its palette ink (black
/// when it has none) and a gradient nobody could see is demoted to its mean
/// ([`demote_imperceptible_gradient`]).
fn final_fills(
    face_fill: Vec<gradient::FillFit>,
    face_color: &[usize],
    pal: &inkvec_trace::Palette,
    no_gradients: bool,
) -> Vec<gradient::FillFit> {
    face_fill
        .into_iter()
        .enumerate()
        .map(|(fi, f)| {
            if no_gradients {
                let c = face_color
                    .get(fi)
                    .and_then(|&ci| pal.rgb.get(ci))
                    .copied()
                    .unwrap_or([0.0, 0.0, 0.0]);
                gradient::FillFit {
                    model: gradient::FillModel::Flat(c),
                    ..f
                }
            } else {
                demote_imperceptible_gradient(f)
            }
        })
        .collect()
}

/// Per face, what the emitter needs to know about transparency: [`face_transparency`].
struct Transparency {
    /// Transparent in the source: a hole, not a colour.
    clear: Vec<bool>,
    /// The opacity to write, 1.0 for opaque.
    opacity: Vec<f32>,
    /// The colour the image was composited onto before tracing, sRGB 0..1.
    matte: [f32; 3],
    /// A linear fade of one colour, as a two-stop gradient of `stop-opacity`.
    alpha_ramps: Vec<Option<AlphaRamp>>,
    /// A fade traced natively: (opacity profile, colour profile).
    fades: Vec<Option<(gradient::FillModel, gradient::FillModel)>>,
}

/// Stage 6: per-face transparency, from the source's alpha ([`alpha::face_alpha`]) and,
/// when transparency was traced natively, from the palette's own inks.
///
/// Traced natively, a face's opacity is its ink's: the palette found the ink *as* a
/// colour at an opacity, so a band of a fade is one opacity by construction, where the
/// flatness test in `face_alpha` would call it varying and bake it opaque. An ink under
/// 0.05 opacity is clear and one over 0.98 opaque. The ramp fit stays for the faces the
/// palette did find opaque.
fn face_transparency(
    img: &inkvec_trace::Rgba,
    args: &Args,
    alpha_src: Option<&AlphaSource>,
    face_color: &[usize],
    pal: &inkvec_trace::Palette,
    traced_labels: &[u16],
    face_fade: &[Option<inkvec_trace::native::Fade>],
) -> Transparency {
    let (w, h) = (img.width, img.height);
    let FaceAlpha {
        mut clear,
        mut opacity,
        matte,
        alpha_ramps,
    } = alpha::face_alpha(img, args, alpha_src, face_color, traced_labels, w, h);
    if args.native_alpha && alpha_src.is_some() {
        for (f, &ink) in face_color.iter().enumerate() {
            let a = pal.alpha.get(ink).copied().unwrap_or(1.0);
            if let Some(c) = clear.get_mut(f) {
                *c = a < 0.05;
            }
            if let Some(o) = opacity.get_mut(f) {
                *o = if a < 0.05 || a > 0.98 { 1.0 } else { a };
            }
        }
        // A fade is translucent throughout, so it is punched out of whatever paints under
        // it exactly as a wash is; its opacity is written by its gradient's stops, and the
        // number here only has to say "below one".
        for (f, fade) in face_fade.iter().enumerate() {
            if fade.is_some() {
                if let Some(c) = clear.get_mut(f) {
                    *c = false;
                }
                if let Some(o) = opacity.get_mut(f) {
                    *o = 0.5;
                }
            }
        }
    }
    let fades: Vec<Option<(gradient::FillModel, gradient::FillModel)>> = (0..face_color.len())
        .map(|f| {
            face_fade
                .get(f)
                .cloned()
                .flatten()
                .map(|fd| (fd.alpha, fd.color))
        })
        .collect();
    Transparency {
        clear,
        opacity,
        matte,
        alpha_ramps,
        fades,
    }
}

/// The faces a layer covers, merged into the ground beneath them.
///
/// A layer cuts every shape it crosses into pieces, and painting those pieces back is
/// what makes the layer form *bigger* than the flat one: the cut is drawn twice, once
/// on each side, and neither drawing means anything once the layer is composited over
/// them. Relabelling the pieces to a single face makes the cut interior, and an
/// interior edge belongs to no ring — so it stops being written at all, and the
/// geometry is untouched otherwise. The boundary points, their fits and their sigmas
/// are per *edge* and are not disturbed by any of this.
///
/// `remap[f]` is the label face `f` becomes; a label past its end is left as it is.
fn merge_map(map: &planar::PlanarMap, remap: &[u16]) -> planar::PlanarMap {
    let mut out = map.clone();
    for e in out.edges.iter_mut() {
        let l = remap.get(e.left as usize).copied().unwrap_or(e.left);
        let r = remap.get(e.right as usize).copied().unwrap_or(e.right);
        if l == r {
            // Interior now: `face_edge_order` skips a label out of range, which is how
            // an edge leaves every ring without disturbing the indices the fits use.
            e.left = u16::MAX;
            e.right = u16::MAX;
        } else {
            e.left = l;
            e.right = r;
        }
    }
    out
}

/// Stage 8: the colour document, as `doc` describes it (`doc.layers` is ignored), with or
/// without the recovered translucent `layers`.
///
/// Both forms of the document, costed against each other.
///
/// A layer is only worth having if it says the same thing in fewer marks. It repaints
/// the faces it covers with the ground and composites itself over them, so the image is
/// identical either way and the whole decision is a parameter count — the same test
/// every fill model and every arc has to pass, with the residual term equal on both
/// sides. Where the layer does not pay, the flat form is what is written.
fn write_colour(
    doc: &ColorDoc,
    opts: &EmitOptions,
    map: &planar::PlanarMap,
    layers: Option<inkvec_trace::alpha::AlphaAnalysis>,
    quiet: bool,
) -> String {
    let flat_doc = ColorDoc {
        layers: None,
        ..*doc
    };
    let Some(an) = layers.as_ref().filter(|an| !an.layers.is_empty()) else {
        return emit_color(&flat_doc, opts);
    };
    let flat = emit_color(&flat_doc, opts);
    // Every face a layer covers is relabelled onto the ground it belongs with, so
    // the cuts the layer made stop being drawn at all.
    let mut remap: Vec<u16> = (0..map.n_labels as u16).collect();
    for l in &an.layers {
        for &f in &l.faces {
            if f >= map.n_labels {
                continue;
            }
            let base = an.base_rgb.get(f).copied().unwrap_or([0.0, 0.0, 0.0]);
            // The ground this piece belongs with: a face not under the layer whose
            // own colour is the colour underneath this one.
            let host = (0..map.n_labels).find(|&g| {
                g != f
                    && !l.faces.contains(&g)
                    && an
                        .base_rgb
                        .get(g)
                        .is_some_and(|c| inkvec_trace::color::de00(*c, base) < 1.0)
            });
            if let Some(g) = host {
                remap[f] = g as u16;
            }
        }
    }
    let merged = merge_map(map, &remap);
    let order_m = planar::face_edge_order(&merged);
    // The layer's own outline, by the same trick: its pieces become one face, so
    // the cuts *inside* the layer stop being drawn as well. Without this the layer
    // path repeats every edge the ground merge just removed, and the document comes
    // out with fewer shapes and more coordinates, which is not simpler.
    let layer_shapes: Vec<FaceRings> = an
        .layers
        .iter()
        .map(|l| {
            let Some(&first) = l.faces.first() else {
                return Vec::new();
            };
            let mut remap_l: Vec<u16> = (0..map.n_labels as u16).collect();
            for &f in &l.faces {
                if f < map.n_labels {
                    remap_l[f] = first as u16;
                }
            }
            planar::face_edge_order(&merge_map(map, &remap_l))
                .get(first)
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    let layered = emit_color(
        &ColorDoc {
            order: &order_m,
            layers: Some((an, &layer_shapes)),
            ..*doc
        },
        opts,
    );
    let cost = |doc: &str| -> (usize, usize) {
        let shapes = ["<path", "<rect", "<circle", "<ellipse"]
            .iter()
            .map(|t| doc.matches(t).count())
            .sum::<usize>();
        (shapes, doc.len())
    };
    let (pf, bf) = cost(&flat);
    let (pl, bl) = cost(&layered);
    // Fewer shapes *and* not more coordinates. A document with fewer paths and
    // more numbers in them is not the simpler one, whatever the path count says.
    let pays = pl < pf && bl <= bf;
    diag::stage(quiet, || {
        format!(
            "                as layers {pl} shapes / {bl} bytes, flat {pf} / {bf}: {}",
            if pays { "kept" } else { "dropped" }
        )
    });
    if pays {
        layered
    } else {
        flat
    }
}

/// `--uncertainty <path>`: write the confidence bands of the traced boundaries beside the
/// document (see [`crate::uncertainty`]). A write failure is reported and does not fail
/// the trace.
fn write_uncertainty(args: &Args, svg: &str, map: &planar::PlanarMap) {
    let Some(path) = &args.uncertainty else {
        return;
    };
    let bands = uncertainty::bands_svg(svg, &map.edges, map.width, map.height, args.uncertainty_k);
    if let Err(e) = std::fs::write(path, bands) {
        diag::warn(|| {
            format!(
                "could not write the confidence bands to {}: {e}",
                path.display()
            )
        });
    }
}

/// The numbers the colour pipeline reports: one line per stage, in [`ColourReport::lines`].
struct ColourReport<'a> {
    /// Palette entries.
    palette: usize,
    /// Faces of the planar map.
    faces: usize,
    /// Faces written with a gradient fill.
    gradients: usize,
    /// Shared edges of the planar map.
    edges: usize,
    /// Edges written as one primitive.
    primitives: usize,
    /// What the global boundary solve did, if it gained anything.
    boundary: Option<&'a inkvec_trace::boundary_opt::Report>,
    /// The label map's mirrors.
    symmetry: &'a inkvec_trace::symmetry::Symmetry,
    /// Boundary points the symmetry pass averaged.
    symmetrised: usize,
    /// Fits reflected or centred by [`apply_mirrors`].
    mirrored_fits: usize,
    /// Boundaries refitted to stop rings crossing.
    repaired: usize,
    /// Line segments fitted, before mirroring.
    n_line: usize,
    /// Cubic segments fitted, before mirroring.
    n_cubic: usize,
    /// Boundary points measured by the tracer.
    measured: usize,
}

impl ColourReport<'_> {
    /// The report lines, in pipeline order.
    fn lines(&self) -> [String; 6] {
        let (n_line, n_cubic, measured) = (self.n_line, self.n_cubic, self.measured);
        let (symmetrised, mirrored_fits, repaired) =
            (self.symmetrised, self.mirrored_fits, self.repaired);
        let n_grad = self.gradients;
        let anchors = n_cubic + n_line;
        [
            format!(
                "palette       {} colours, {} faces ({n_grad} gradient)",
                self.palette, self.faces
            ),
            format!(
                "planar map    {} shared edges ({} primitive)",
                self.edges, self.primitives
            ),
            self.boundary.map_or_else(
                || "boundary solve  no gain".to_string(),
                |r| {
                    format!(
                        "boundary solve  E {:.1} -> {:.1} in {} iteration(s), {} point(s) moved{}",
                        r.before,
                        r.after,
                        r.iters,
                        r.moved,
                        if r.scale < 1.0 {
                            format!(", scaled to {:.0}% to stay simple", r.scale * 100.0)
                        } else {
                            String::new()
                        }
                    )
                },
            ),
            if self.symmetry.is_empty() {
                "symmetry      none in the label map".to_string()
            } else {
                format!(
                    "symmetry      {} mirror(s), {symmetrised} point(s) averaged, {mirrored_fits} fit(s) reflected",
                    self.symmetry.mirrors.len()
                )
            },
            format!("repair        {repaired} boundary refit(s) to stop rings crossing"),
            format!(
                "segments      {anchors}  ({n_line} line, {n_cubic} cubic) from {measured} measured points, {:.1}x reduction",
                measured as f64 / anchors.max(1) as f64
            ),
        ]
    }
}

/// Research builds: whether the structurally simplified fit `trial` should replace the
/// `baseline` fit, boundary by boundary priced at each one's own lambda (`scales`).
///
/// A transaction: the trial is accepted only if, on *every* boundary, it spends no more
/// parameters (by the fitter's count and by the numbers SVG would write) and costs no more
/// by [`path_cost`], and on at least one boundary it spends strictly fewer. Mismatched
/// lengths or a non-finite cost reject it.
fn structural_trial_improves(
    polys: &[inkvec_core::Polyline],
    trial: &[FittedPath],
    baseline: &[FittedPath],
    scales: &[f64],
    cfg: &FitConfig,
) -> bool {
    if polys.len() != trial.len() || polys.len() != baseline.len() || polys.len() != scales.len() {
        return false;
    }
    let mut strict = false;
    for (((poly, candidate), original), &scale) in polys.iter().zip(trial).zip(baseline).zip(scales)
    {
        let rate = |p: &FittedPath| p.segments.iter().map(|s| s.params()).sum::<f64>();
        // An SVG arc writes seven numbers even when its geometric model has
        // only five degrees of freedom. Protect both notions of complexity.
        let written = |p: &FittedPath| {
            p.segments
                .iter()
                .map(|s| match s {
                    Segment::Line(..) => 2usize,
                    Segment::Cubic(..) => 6,
                    Segment::Arc { .. } => 7,
                })
                .sum::<usize>()
        };
        let (new_rate, old_rate) = (rate(candidate), rate(original));
        if new_rate > old_rate || written(candidate) > written(original) {
            return false;
        }
        let local = FitConfig {
            lambda: cfg.lambda * scale,
            ..*cfg
        };
        let (new_cost, old_cost) = (
            path_cost(poly, candidate, &local),
            path_cost(poly, original, &local),
        );
        if !new_cost.is_finite() || !old_cost.is_finite() || new_cost > old_cost + 1e-9 {
            return false;
        }
        strict |= new_rate < old_rate;
    }
    strict
}

/// MDL cost of a fitted path against the measurements it came from, in the same units
/// the fitters minimise, so a primitive and a curve description are directly comparable:
///
/// `cost = χ²/2 + λ·k`, where `χ² = Σ_i (d_i / σ_i)²` sums each measured point's squared
/// distance `d_i` (px) to the path (sampled every quarter pixel) over its sigma `σ_i` (px),
/// `λ` is `cfg.lambda` (nats per parameter) and `k` the path's parameter count. Half of χ²
/// is the negative log-likelihood of the points under independent Gaussian errors, so both
/// terms are in nats. A path with no segments costs infinity.
///
/// Defined once, as `inkvec_fit::choice::boundary_cost`, beside the lower bound that lets
/// the image frame skip its dynamic program.
fn path_cost(poly: &inkvec_core::Polyline, path: &FittedPath, cfg: &FitConfig) -> f64 {
    choice::boundary_cost(poly, path, cfg)
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
