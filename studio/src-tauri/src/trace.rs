//! Tracing: the two tiers, the stage log, and the states a trace can end in.
//!
//! A trace takes about a second, sometimes several, and that one fact shapes everything
//! here. The app traces twice. While a control is moving it runs a **draft** — the same
//! settings at 512 px with a short time limit, so it feels live. When the controls have
//! been still for ~800 ms it queues the **full-resolution** trace and swaps it in. Draft
//! and final are therefore two visible states of the same result, never a silent
//! substitution, and export always forces a fresh full trace.
//!
//! The wait is filled with what the engine is actually doing. `inkvec_trace::with_stage_sink`
//! reports each pipeline stage as it is passed, and [`stage_label`] maps the pipeline's
//! internal names onto the nine the interface names. Nothing is invented: a stage appears
//! when the engine has finished it, with the time it took.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::lost::{self, Container, Loss};
use crate::options::Settings;
use crate::quality::{self, Ink, Report, WorstCorner};

/// Which of the two tiers a trace is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// Small and time-limited; shown while a control is moving.
    Draft,
    /// Full resolution, no draft time limit. What gets exported.
    Final,
}

impl Tier {
    fn label(self) -> &'static str {
        match self {
            Tier::Draft => "draft",
            Tier::Final => "final",
        }
    }
}

/// One stage the engine finished, on its way through a trace.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stage {
    /// The name the interface shows: one of the nine in [`STAGES`].
    pub name: &'static str,
    /// Milliseconds spent in it so far. Stages that map to several pipeline steps
    /// accumulate.
    pub ms: f64,
}

/// The nine stages the interface names, in the order the pipeline reaches them.
///
/// The pipeline has about thirty internal marks; these are the boundaries that mean
/// something to somebody watching, and [`stage_label`] folds the rest into them.
pub const STAGES: [&str; 9] = [
    "intake",
    "palette",
    "planar map",
    "boundary solve",
    "symmetry",
    "repair",
    "segments",
    "lambda",
    "wrote",
];

/// Map one of the pipeline's internal stage names onto the stage the interface shows.
///
/// The internal names are explicitly not a stable interface, so an unrecognised one is
/// folded into the stage it most likely belongs to rather than dropped or shown raw: a
/// new pipeline step must not put a word nobody can read in front of the user.
pub fn stage_label(internal: &str) -> &'static str {
    match internal {
        "decode" => "intake",
        "palette" | "labels" | "labels_in" | "despeckle" | "blend_absorb" | "merge_bands"
        | "carve" | "split" | "fades" => "palette",
        "saddles" | "build_map" => "planar map",
        "refine_subpix" | "refine_junc" | "boundary_opt" => "boundary solve",
        "symmetry" | "symmetry_detect" => "symmetry",
        "repair" => "repair",
        "fit_dp" => "segments",
        "fills" => "lambda",
        "emit" | "trace_total" => "wrote",
        // Unknown: it happened after the palette and before the emit, which is where
        // nearly all of the pipeline lives.
        _ => "boundary solve",
    }
}

/// How a trace ended.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Outcome {
    /// A drawing, with everything measured about it.
    Traced(Box<Traced>),
    /// The image is a single flat colour. Not an error: there is simply nothing to trace.
    Flat,
    /// The file did not decode.
    Undecodable { message: String },
    /// The trace would need more memory than this machine is likely to have.
    OutOfMemory { needed_gb: f64, suggest_px: u32 },
    /// The pipeline failed. Worth a bug report.
    Failed { message: String },
}

/// A finished trace and every number the interface shows about it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Traced {
    /// Which tier this is.
    pub tier: &'static str,
    /// The SVG, as it would be written.
    pub svg: String,
    /// The measurements.
    pub report: Report,
    /// The inks, largest share first.
    pub palette: Vec<Ink>,
    /// What could not be recovered. Empty is the positive state.
    pub losses: Vec<Loss>,
    /// Where source and trace disagree most, in traced-raster pixels.
    pub worst_corner: Option<WorstCorner>,
    /// The stage log, with the time each stage took.
    pub stages: Vec<Stage>,
    /// The engine's own one-line-per-stage summary, as the command line prints it.
    pub engine_log: Vec<String>,
    /// The raster the trace actually ran on, longer side in pixels.
    pub traced_px: u32,
    /// Whether the input was larger than the trace size and so was measured smaller.
    pub oversized: bool,
    /// Size of the input as it arrived.
    pub source_px: (u32, u32),
}

/// An image that has been opened but not necessarily traced.
pub struct Source {
    /// The file's own bytes, kept so a re-trace at another size re-decodes from the
    /// original rather than from an already-resampled copy.
    pub bytes: Vec<u8>,
    /// Where it came from, for the title bar and for Export's default name.
    pub path: Option<std::path::PathBuf>,
    /// What the file is.
    pub container: Container,
    /// Its size as it arrived.
    pub width: u32,
    /// Its size as it arrived.
    pub height: u32,
}

/// Written by hand rather than derived: the file's own bytes are in here, and a
/// megabyte of PNG in a panic message or a log line helps nobody.
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("path", &self.path)
            .field("container", &self.container)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &format_args!("{} bytes", self.bytes.len()))
            .finish()
    }
}

impl Source {
    /// Read an image from bytes, refusing early and in words if it will not decode.
    pub fn open(bytes: Vec<u8>, path: Option<std::path::PathBuf>) -> Result<Self, String> {
        let container = Container::sniff(&bytes);
        // `max_dim` 0: dimensions only, no resampling, so the reported size is the file's.
        let (_img, (width, height)) = inkvec_trace::decode_image_capped(&bytes, 0)
            .map_err(|e| describe_decode_failure(container, &e.to_string()))?;
        Ok(Self {
            bytes,
            path,
            container,
            width,
            height,
        })
    }

    /// The file's name, or a stand-in for an image that arrived from the clipboard.
    pub fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "pasted image".to_string())
    }

    /// The name without its extension, which is what an export is named after.
    pub fn stem(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".to_string())
    }
}

/// A trace's identity, so a result that arrives after the user has moved on can be
/// recognised and dropped.
///
/// Cancelling cannot interrupt the pipeline — it has no cancellation point, and inventing
/// one would mean unwinding a numerical solve halfway. What cancelling does instead is
/// retire the generation: the interface returns to the last result immediately, and when
/// the abandoned trace finishes, its result is dropped because its generation is stale.
/// The thread finishes its work; nothing waits for it.
#[derive(Debug, Default)]
pub struct Generation(AtomicU64);

impl Generation {
    /// Start a new trace, retiring any in flight, and return the new generation's id.
    pub fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    /// Retire whatever is in flight without starting anything.
    pub fn cancel(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    /// Whether `id` is still the trace the interface is waiting for.
    pub fn is_current(&self, id: u64) -> bool {
        self.0.load(Ordering::SeqCst) == id
    }
}

/// Roughly how much memory a trace at this size will want, in bytes.
///
/// The pipeline keeps the raster as f32 RGBA and a handful of working copies and label
/// maps alongside it. The multiplier is deliberately generous: the number exists to catch
/// "you asked for 16384 px" before the allocator does, and an allocation failure in Rust
/// aborts the process rather than returning an error we could show.
fn memory_estimate(w: u32, h: u32, max_dim: u32) -> u64 {
    let (w, h) = match scaled_to(w, h, max_dim) {
        Some(d) => d,
        None => (w, h),
    };
    let px = w as u64 * h as u64;
    px * 4 * 4 * 14
}

/// The size a raster is reduced to by a `max_dim` cap, or `None` if it already fits.
fn scaled_to(w: u32, h: u32, max_dim: u32) -> Option<(u32, u32)> {
    if max_dim == 0 || w.max(h) <= max_dim {
        return None;
    }
    let s = max_dim as f64 / w.max(h) as f64;
    Some((
        ((w as f64 * s).round() as u32).max(1),
        ((h as f64 * s).round() as u32).max(1),
    ))
}

/// The ceiling a trace is refused at, in bytes.
///
/// Not a measurement of this machine — reading free memory portably would need another
/// dependency, and the answer moves while the trace runs anyway. 8 GiB is well above what
/// any sane logo needs and well below what a desktop will survive losing.
const MEMORY_CEILING: u64 = 8 * 1024 * 1024 * 1024;

/// Run one trace to completion on the calling thread.
///
/// `on_stage` is called from inside the pipeline as each stage is passed; it should hand
/// the value off and return.
pub fn run(
    source: &Source,
    settings: &Settings,
    tier: Tier,
    on_stage: impl Fn(&'static str, f64) + Send + 'static,
) -> Outcome {
    let settings = settings.clone().sanitised();
    let started = Instant::now();

    let needed = memory_estimate(source.width, source.height, settings.trace_size);
    if needed > MEMORY_CEILING {
        // The largest size that fits, rounded down to something a slider can land on.
        let ratio = (MEMORY_CEILING as f64 / needed as f64).sqrt();
        let suggest = (((settings.trace_size as f64 * ratio) as u32) / 256 * 256).max(256);
        return Outcome::OutOfMemory {
            needed_gb: needed as f64 / (1024.0 * 1024.0 * 1024.0),
            suggest_px: suggest,
        };
    }

    let mut args = settings.to_args();
    // A flat image is not an error, but the engine only says so when asked strictly;
    // without this it silently writes a single rectangle and the app would present that
    // as a trace.
    args.strict = true;
    // `lossy` is a question about the container, and its first bytes answer it — the same
    // resolution `inkvec::trace` does for a caller who handed over a file.
    let head: Vec<u8> = source.bytes[..source.bytes.len().min(32)].to_vec();
    let args = inkvec_cli::resolve_lossy(&args, move || Some(head));

    // Stages are reported from inside the pipeline, on this thread, as each one is
    // passed. They are folded onto the nine names the interface shows — accumulating
    // time where several pipeline steps map to one name — and handed straight on, so the
    // rail fills while the trace runs rather than after it.
    let collected: std::rc::Rc<std::cell::RefCell<Vec<Stage>>> = Default::default();
    let store = std::rc::Rc::clone(&collected);
    let traced = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        inkvec_trace::with_stage_sink(
            move |internal, ms| {
                let name = stage_label(internal);
                {
                    let mut stages = store.borrow_mut();
                    match stages.iter_mut().find(|s| s.name == name) {
                        Some(existing) => existing.ms += ms,
                        None => stages.push(Stage { name, ms }),
                    }
                }
                on_stage(name, ms);
            },
            || {
                let (img, (w, h)) = inkvec_trace::decode_image_capped(&source.bytes, args.max_dim)?;
                let out =
                    inkvec_cli::trace_image_sized(img, &args, Some((w as usize, h as usize)))?;
                Ok::<_, Box<dyn std::error::Error>>(out)
            },
        )
    }));
    let stages = collected.borrow().clone();

    let traced = match traced {
        Err(payload) => {
            return Outcome::Failed {
                message: panic_message(&payload),
            }
        }
        Ok(Err(e)) => {
            let msg = e.to_string();
            return if msg.contains("single flat colour") {
                Outcome::Flat
            } else if msg.contains("decode") || msg.contains("format") {
                Outcome::Undecodable {
                    message: describe_decode_failure(source.container, &msg),
                }
            } else {
                Outcome::Failed { message: msg }
            };
        }
        Ok(Ok(t)) => t,
    };

    let engine_log = traced.stats.clone();
    let (traced_w, traced_h) = (traced.width as u32, traced.height as u32);
    let svg = inkvec_cli::post_process(&args, traced.svg, traced.width, traced.height);
    let seconds = started.elapsed().as_secs_f64();

    // Everything below is measurement of what was just produced, and every one of them
    // can fail on a document resvg will not take. None of that should cost the user the
    // trace itself, so each degrades to "not measured" and the panel says so.
    let raster = inkvec_trace::decode_image_capped(&source.bytes, args.max_dim)
        .ok()
        .map(|(img, _)| img);
    let analysis = raster.as_ref().and_then(|r| quality::analyse(r, &svg).ok());

    let (coordinates, segments, paths, colours) = quality::count(&svg);
    let minified_bytes = minified_size(&svg, args.minify);
    let report = Report {
        mean_de00: analysis.as_ref().map(|a| a.mean),
        median_de00: analysis.as_ref().map(|a| a.median),
        worst_de00: analysis.as_ref().map(|a| a.worst),
        coordinates,
        paths,
        segments,
        colours,
        bytes: svg.len(),
        minified_bytes,
        seconds,
        traced_px: traced_w.max(traced_h),
    };

    let palette = quality::palette(&svg, traced_w, traced_h).unwrap_or_default();
    let losses = match (raster.as_ref(), analysis.as_ref()) {
        (Some(r), Some(a)) => lost::detect(r, a, &svg, &settings, source.container),
        _ => Vec::new(),
    };

    Outcome::Traced(Box::new(Traced {
        tier: tier.label(),
        svg,
        report,
        palette,
        losses,
        worst_corner: analysis.as_ref().and_then(|a| a.corner),
        stages,
        engine_log,
        traced_px: traced_w.max(traced_h),
        oversized: scaled_to(source.width, source.height, args.max_dim as u32).is_some(),
        source_px: (source.width, source.height),
    }))
}

/// The size the same drawing takes after the minifier, for the "minified" figure beside
/// the file size.
///
/// `None` when the output is already minified — the report then shows one number, because
/// showing the same value twice would suggest the minifier had nothing to offer when in
/// fact it has already been applied.
fn minified_size(svg: &str, already: bool) -> Option<usize> {
    if already {
        return None;
    }
    let opts = inkvec_svgmin::Options {
        // The geometry is the tracer's own and is already rounded to 2 decimals; this is
        // the spelling pass, not a refit, so nothing moves.
        decimals: Some(2),
        ..Default::default()
    };
    inkvec_svgmin::compact(svg, &opts)
        .ok()
        .map(|(s, _)| s.len())
}

/// Turn a decoder's own message into one that says what to do about it.
fn describe_decode_failure(container: Container, raw: &str) -> String {
    let known = !matches!(container, Container::Unknown);
    if known {
        format!(
            "This file says it is a {} but will not decode: {raw}. \
             PNG, JPEG, WebP, BMP, GIF and TIFF are supported.",
            container.name()
        )
    } else {
        format!(
            "This file is not an image format the tracer reads: {raw}. \
             PNG, JPEG, WebP, BMP, GIF and TIFF are supported."
        )
    }
}

/// The text out of a panic payload, for the "trace failed" state.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "the tracer stopped without saying why".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 96 x 96 PNG: a dark disc on a light ground, with an off-centre notch so the
    /// trace has more than one boundary to find.
    fn sample_png() -> Vec<u8> {
        let mut img = image::RgbaImage::from_pixel(96, 96, image::Rgba([245, 242, 234, 255]));
        for y in 0..96i32 {
            for x in 0..96i32 {
                let (dx, dy) = ((x - 48) as f64, (y - 48) as f64);
                if (dx * dx + dy * dy).sqrt() < 34.0 {
                    img.put_pixel(x as u32, y as u32, image::Rgba([20, 69, 63, 255]));
                }
                if (18..34).contains(&x) && (18..30).contains(&y) {
                    img.put_pixel(x as u32, y as u32, image::Rgba([231, 176, 74, 255]));
                }
            }
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn flat_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(48, 48, image::Rgba([12, 34, 56, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn every_internal_stage_name_lands_on_a_named_stage() {
        // The names the pipeline marks today. An unknown one must still map to something
        // the interface can show, which is what the wildcard arm is for.
        let internal = [
            "decode",
            "palette",
            "labels",
            "labels_in",
            "despeckle",
            "blend_absorb",
            "merge_bands",
            "carve",
            "split",
            "fades",
            "saddles",
            "build_map",
            "symmetry_detect",
            "symmetry",
            "refine_subpix",
            "refine_junc",
            "boundary_opt",
            "repair",
            "fit_dp",
            "fills",
            "emit",
            "trace_total",
            "something_new_in_2027",
        ];
        for name in internal {
            let label = stage_label(name);
            assert!(
                STAGES.contains(&label),
                "{name} -> {label}, not a named stage"
            );
        }
    }

    #[test]
    fn opening_a_png_reads_its_size_without_resampling_it() {
        let s = Source::open(sample_png(), None).unwrap();
        assert_eq!((s.width, s.height), (96, 96));
        assert_eq!(s.container, Container::Png);
        assert_eq!(s.name(), "pasted image");
    }

    #[test]
    fn opening_something_that_is_not_an_image_says_what_is_supported() {
        let e = Source::open(b"this is a text file".to_vec(), None).unwrap_err();
        assert!(e.contains("PNG, JPEG, WebP"), "{e}");
    }

    #[test]
    fn a_trace_produces_an_svg_and_measures_it() {
        let source = Source::open(sample_png(), None).unwrap();
        let outcome = run(&source, &Settings::default(), Tier::Final, |_, _| {});
        let Outcome::Traced(t) = outcome else {
            panic!("expected a drawing, got {outcome:?}");
        };
        assert!(
            t.svg.starts_with("<svg"),
            "{}",
            &t.svg[..40.min(t.svg.len())]
        );
        assert!(t.report.paths >= 2, "{:?}", t.report);
        assert!(t.report.coordinates > 0);
        assert!(!t.palette.is_empty());
        // The whole point of the panel: the number is measured, not asserted.
        let mean = t.report.mean_de00.expect("dE00 was measured");
        assert!((0.0..12.0).contains(&mean), "{mean}");
        assert_eq!(t.tier, "final");
        assert!(!t.oversized);
    }

    #[test]
    fn the_stage_log_arrives_while_the_trace_runs() {
        let source = Source::open(sample_png(), None).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let outcome = run(
            &source,
            &Settings::default(),
            Tier::Final,
            move |name, _| {
                let _ = tx.send(name);
            },
        );
        assert!(matches!(outcome, Outcome::Traced(_)));
        let seen: Vec<&str> = rx.into_iter().collect();
        assert!(!seen.is_empty(), "no stages were reported");
        for name in &seen {
            assert!(STAGES.contains(name), "{name} is not a named stage");
        }
        assert!(seen.contains(&"palette"), "{seen:?}");
    }

    /// The honesty panel must not invent losses.
    ///
    /// This is a regression test for a real one: the dropped-detail heuristic used to
    /// count every thread of anti-aliased pixels along a boundary as a lost feature, and
    /// reported 469 of them on a trace whose mean colour difference was 0.07 dE00. A
    /// panel that says that is worse than no panel, because it teaches people to ignore
    /// it. A clean trace of clean artwork reports nothing.
    #[test]
    fn a_clean_trace_reports_no_losses_at_all() {
        let source = Source::open(sample_png(), None).unwrap();
        let outcome = run(&source, &Settings::default(), Tier::Final, |_, _| {});
        let Outcome::Traced(t) = outcome else {
            panic!("expected a drawing, got {outcome:?}")
        };
        assert!(
            t.report.mean_de00.unwrap() < 1.0,
            "this fixture should trace cleanly: {:?}",
            t.report.mean_de00
        );
        assert!(
            t.losses.is_empty(),
            "a clean trace reported losses: {:?}",
            t.losses.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_flat_image_is_a_state_of_its_own_not_an_error() {
        let source = Source::open(flat_png(), None).unwrap();
        let outcome = run(&source, &Settings::default(), Tier::Final, |_, _| {});
        assert!(matches!(outcome, Outcome::Flat), "{outcome:?}");
    }

    #[test]
    fn a_draft_is_smaller_and_quicker_than_the_final() {
        let source = Source::open(sample_png(), None).unwrap();
        let draft = Settings::default().draft(64, 0.4);
        let outcome = run(&source, &draft, Tier::Draft, |_, _| {});
        let Outcome::Traced(t) = outcome else {
            panic!("expected a drawing, got {outcome:?}")
        };
        assert_eq!(t.tier, "draft");
        assert_eq!(t.traced_px, 64, "the draft ran at its own trace size");
        assert_eq!(
            t.source_px,
            (96, 96),
            "and still reports the size that arrived"
        );
        assert!(t.oversized, "96 px measured at 64 px is a reduced trace");
    }

    #[test]
    fn an_impossible_trace_size_is_refused_before_the_allocator_sees_it() {
        let source = Source::open(sample_png(), None).unwrap();
        let huge = Settings {
            trace_size: 16384,
            ..Settings::default()
        };
        // A 96 px input is capped *down* to 96, so it never reaches the ceiling: the
        // estimate has to be driven by a genuinely large input.
        assert!(matches!(
            run(&source, &huge, Tier::Final, |_, _| {}),
            Outcome::Traced(_)
        ));
        assert!(memory_estimate(20000, 20000, 16384) > MEMORY_CEILING);
        assert!(memory_estimate(20000, 20000, 2048) < MEMORY_CEILING);
    }

    #[test]
    fn a_generation_retires_what_came_before_it() {
        let g = Generation::default();
        let first = g.next();
        assert!(g.is_current(first));
        let second = g.next();
        assert!(!g.is_current(first));
        assert!(g.is_current(second));
        g.cancel();
        assert!(!g.is_current(second));
    }

    #[test]
    fn scaling_only_ever_reduces() {
        assert_eq!(scaled_to(1000, 500, 2048), None);
        assert_eq!(scaled_to(1000, 500, 0), None);
        assert_eq!(scaled_to(1000, 500, 500), Some((500, 250)));
    }
}
