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
use std::sync::{Condvar, Mutex};
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
    /// Where each boundary could be: the engine's confidence bands, an SVG in the
    /// drawing's own coordinates, when the trace produced them (colour traces do).
    pub bands: Option<String>,
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
    ///
    /// An SVG is accepted too, rendered to a PNG first: re-tracing a drawing is how a
    /// messy one comes back clean (the paths a generator or a converter left behind,
    /// stacked translucent strokes, thousands of nodes), and from then on it is an image
    /// like any other, with a quality report measured against its own render.
    pub fn open(bytes: Vec<u8>, path: Option<std::path::PathBuf>) -> Result<Self, String> {
        let bytes = if looks_like_svg(&bytes) {
            svg_to_png(&bytes)?
        } else {
            bytes
        };
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

/// Long side of the render an SVG is traced from, in pixels.
const SVG_RENDER_PX: u32 = 1024;
/// Samples per pixel along each axis when rendering an SVG. resvg's own antialiasing gives
/// an edge a handful of alpha levels; the tracer reads sub-pixel boundary positions from
/// edge coverage, so the render is supersampled and box-filtered to exact coverage.
const SVG_SUPERSAMPLE: u32 = 4;

/// Whether `bytes` are an SVG document rather than an image file.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let t = head.trim_start_matches('\u{feff}').trim_start();
    t.starts_with('<') && t.contains("<svg")
}

/// Render an SVG document to a PNG, [`SVG_RENDER_PX`] on its long side.
fn svg_to_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use resvg::{tiny_skia, usvg};
    let text = std::str::from_utf8(bytes).map_err(|_| "this SVG is not UTF-8 text".to_string())?;
    let tree = usvg::Tree::from_str(text, &usvg::Options::default())
        .map_err(|e| format!("this SVG could not be read: {e}"))?;
    let size = tree.size();
    let (sw, sh) = (size.width() as f64, size.height() as f64);
    if !(sw > 0.0 && sh > 0.0) {
        return Err("this SVG declares an empty canvas".into());
    }
    let fit = SVG_RENDER_PX as f64 / sw.max(sh);
    let w = ((sw * fit).round() as u32).max(1);
    let h = ((sh * fit).round() as u32).max(1);
    let k = SVG_SUPERSAMPLE;
    let mut pixmap = tiny_skia::Pixmap::new(w * k, h * k)
        .ok_or_else(|| "this SVG is too large to render".to_string())?;
    let ts = tiny_skia::Transform::from_scale(
        (w * k) as f32 / size.width(),
        (h * k) as f32 / size.height(),
    );
    resvg::render(&tree, ts, &mut pixmap.as_mut());
    // Box filter in tiny-skia's premultiplied space, then unpremultiply once.
    let src = pixmap.data();
    let stride = (w * k * 4) as usize;
    let n = k * k;
    let mut out = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0u32; 4];
            for j in 0..k {
                let row = ((y * k + j) as usize) * stride;
                for i in 0..k {
                    let at = row + ((x * k + i) * 4) as usize;
                    for (c, v) in acc.iter_mut().enumerate() {
                        *v += src[at + c] as u32;
                    }
                }
            }
            let a = (acc[3] + n / 2) / n;
            let px = if a == 0 {
                [0, 0, 0, 0]
            } else {
                // Straight colour is the premultiplied sum over the alpha sum.
                let un = |c: u32| ((c * 255 + acc[3] / 2) / acc[3]).min(255) as u8;
                [un(acc[0]), un(acc[1]), un(acc[2]), a as u8]
            };
            out.put_pixel(x, y, image::Rgba(px));
        }
    }
    let mut png = Vec::new();
    out.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("could not encode the render: {e}"))?;
    Ok(png)
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
    source: &std::sync::Arc<Source>,
    settings: &Settings,
    tier: Tier,
    cache: Option<&Cache>,
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

    // The drawing, either from the pipeline or from the one already in hand. Only the
    // output options can be different for a reused drawing, and they are applied below.
    let reused = cache.and_then(|c| c.reuse(source, &settings, tier));
    let (raw_svg, raw_w, raw_h, engine_log, stages, bands) = match reused {
        Some(hit) => (
            hit.svg,
            hit.width,
            hit.height,
            hit.stats,
            Vec::new(),
            hit.bands,
        ),
        None => match trace_pipeline(source, &args, on_stage) {
            Ok(t) => {
                if let Some(c) = cache {
                    c.keep(source, &settings, tier, &t.svg, t.width, t.height, &t.stats);
                    c.keep_bands(t.bands.clone());
                }
                (t.svg, t.width, t.height, t.stats, t.stages, t.bands)
            }
            Err(outcome) => return outcome,
        },
    };

    let (traced_w, traced_h) = (raw_w as u32, raw_h as u32);
    let svg = inkvec_cli::post_process(&args, raw_svg, raw_w, raw_h);
    let seconds = started.elapsed().as_secs_f64();
    measure(
        source, &settings, &args, tier, svg, traced_w, traced_h, engine_log, stages, bands, seconds,
    )
}

/// What the pipeline produced, with the stages it reported on the way.
struct Pipeline {
    svg: String,
    stats: Vec<String>,
    width: usize,
    height: usize,
    stages: Vec<Stage>,
    bands: Option<String>,
}

/// A file for the engine's confidence bands, unique to this trace.
fn bands_path() -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "inkvec-bands-{}-{}.svg",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Run the pipeline itself, turning a failure into the outcome the interface shows.
fn trace_pipeline(
    source: &Source,
    args: &inkvec_cli::Args,
    on_stage: impl Fn(&'static str, f64) + Send + 'static,
) -> Result<Pipeline, Outcome> {
    // Stages are reported from inside the pipeline, on this thread, as each one is
    // passed. They are folded onto the nine names the interface shows — accumulating
    // time where several pipeline steps map to one name — and handed straight on, so the
    // rail fills while the trace runs rather than after it.
    // The bands come back through a file, as the command line writes them.
    let bands_file = bands_path();
    let mut args = args.clone();
    args.uncertainty = Some(bands_file.clone());
    let args = &args;
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
            return Err(Outcome::Failed {
                message: panic_message(&payload),
            })
        }
        Ok(Err(e)) => {
            let msg = e.to_string();
            return Err(if msg.contains("single flat colour") {
                Outcome::Flat
            } else if msg.contains("decode") || msg.contains("format") {
                Outcome::Undecodable {
                    message: describe_decode_failure(source.container, &msg),
                }
            } else {
                Outcome::Failed { message: msg }
            });
        }
        Ok(Ok(t)) => t,
    };

    let bands = std::fs::read_to_string(&bands_file).ok();
    let _ = std::fs::remove_file(&bands_file);
    Ok(Pipeline {
        svg: traced.svg,
        stats: traced.stats,
        width: traced.width,
        height: traced.height,
        stages,
        bands,
    })
}

/// Measure what was produced and describe it: the quality report, the palette and what
/// could not be recovered. Every one of these can fail on a document resvg will not take,
/// and none of that should cost the user the trace, so each degrades to "not measured".
#[allow(clippy::too_many_arguments)]
fn measure(
    source: &Source,
    settings: &Settings,
    args: &inkvec_cli::Args,
    tier: Tier,
    svg: String,
    traced_w: u32,
    traced_h: u32,
    engine_log: Vec<String>,
    stages: Vec<Stage>,
    bands: Option<String>,
    seconds: f64,
) -> Outcome {
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
        structure: inkvec_svgmin::structure(&svg).into(),
        seconds,
        traced_px: traced_w.max(traced_h),
    };

    let palette = quality::palette(&svg, traced_w, traced_h).unwrap_or_default();
    let losses = match (raster.as_ref(), analysis.as_ref()) {
        (Some(r), Some(a)) => lost::detect(r, a, &svg, settings, source.container),
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
        bands,
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
    fn an_svg_is_opened_as_its_render_with_exact_edge_coverage() {
        // A 10 x 5 canvas whose left 5.00488 units are black: the render is 1024 x 512
        // and the boundary sits at x = 512.5, half way across pixel 512.
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="5"><rect width="5.00488" height="5"/></svg>"#;
        let s = Source::open(svg.to_vec(), Some("drawing.svg".into())).unwrap();
        assert_eq!((s.width, s.height), (1024, 512));
        assert_eq!(s.container, Container::Png);
        assert_eq!(s.stem(), "drawing");
        let img = image::load_from_memory(&s.bytes).unwrap().to_rgba8();
        let a = |x: u32| img.get_pixel(x, 100)[3];
        assert_eq!((a(100), a(900)), (255, 0));
        assert!((a(512) as i32 - 128).abs() <= 16, "{}", a(512));
    }

    #[test]
    fn opening_something_that_is_not_an_image_says_what_is_supported() {
        let e = Source::open(b"this is a text file".to_vec(), None).unwrap_err();
        assert!(e.contains("PNG, JPEG, WebP"), "{e}");
    }

    #[test]
    fn a_trace_produces_an_svg_and_measures_it() {
        let source = std::sync::Arc::new(Source::open(sample_png(), None).unwrap());
        let outcome = run(&source, &Settings::default(), Tier::Final, None, |_, _| {});
        let Outcome::Traced(t) = outcome else {
            panic!("expected a drawing, got {outcome:?}");
        };
        // The engine writes an XML declaration and a generator comment before the root.
        assert!(
            t.svg.starts_with("<svg") || t.svg.starts_with("<?xml"),
            "{}",
            &t.svg[..40.min(t.svg.len())]
        );
        assert!(t.svg.contains("<svg"));
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
        let source = std::sync::Arc::new(Source::open(sample_png(), None).unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let outcome = run(
            &source,
            &Settings::default(),
            Tier::Final,
            None,
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
        let source = std::sync::Arc::new(Source::open(sample_png(), None).unwrap());
        let outcome = run(&source, &Settings::default(), Tier::Final, None, |_, _| {});
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
        let source = std::sync::Arc::new(Source::open(flat_png(), None).unwrap());
        let outcome = run(&source, &Settings::default(), Tier::Final, None, |_, _| {});
        assert!(matches!(outcome, Outcome::Flat), "{outcome:?}");
    }

    #[test]
    fn a_draft_is_smaller_and_quicker_than_the_final() {
        let source = std::sync::Arc::new(Source::open(sample_png(), None).unwrap());
        let draft = Settings::default().draft(64, 0.4);
        let outcome = run(&source, &draft, Tier::Draft, None, |_, _| {});
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
        let source = std::sync::Arc::new(Source::open(sample_png(), None).unwrap());
        let huge = Settings {
            trace_size: 16384,
            ..Settings::default()
        };
        // A 96 px input is capped *down* to 96, so it never reaches the ceiling: the
        // estimate has to be driven by a genuinely large input.
        assert!(matches!(
            run(&source, &huge, Tier::Final, None, |_, _| {}),
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

    /// The whole promise of the cache in one test: a drawing that was reused has to be the
    /// drawing a fresh trace would have produced, byte for byte. If this ever fails, the
    /// cache is handing back something stale and the key is wrong.
    #[test]
    fn a_reused_drawing_is_what_a_fresh_trace_would_have_written() {
        let source = std::sync::Arc::new(Source::open(sample_png(), None).unwrap());
        let plain = Settings {
            trace_size: 96,
            ..Settings::default()
        };

        // Trace once, filling the cache, then ask for each output option in turn. Each of
        // those is a cache hit, and each must match the same settings traced from scratch.
        let cache = Cache::default();
        assert!(matches!(
            run(&source, &plain, Tier::Final, Some(&cache), |_, _| {}),
            Outcome::Traced(_)
        ));

        for changed in [
            Settings {
                minify: true,
                ..plain.clone()
            },
            Settings {
                margin: 0.1,
                ..plain.clone()
            },
            Settings {
                minify: true,
                margin: 0.05,
                ..plain.clone()
            },
            // Not a hit, but it must still come out right: the emitter reads this one, so
            // the cache has to trace again rather than hand back the drawing it has.
            Settings {
                transparent_background: true,
                ..plain.clone()
            },
        ] {
            let Outcome::Traced(hit) = run(&source, &changed, Tier::Final, Some(&cache), |_, _| {})
            else {
                panic!("the cached path did not produce a drawing");
            };
            let Outcome::Traced(fresh) = run(&source, &changed, Tier::Final, None, |_, _| {})
            else {
                panic!("the fresh path did not produce a drawing");
            };
            assert_eq!(
                hit.svg, fresh.svg,
                "a reused drawing differs from a fresh one: {changed:?}"
            );
            assert_eq!(hit.report.coordinates, fresh.report.coordinates);
            assert_eq!(hit.report.bytes, fresh.report.bytes);
            assert_eq!(hit.palette.len(), fresh.palette.len());
        }
    }

    /// And the other half: a setting that can move a line must not be served from the cache.
    #[test]
    fn a_setting_that_changes_the_drawing_is_traced_again() {
        let source = std::sync::Arc::new(Source::open(sample_png(), None).unwrap());
        let plain = Settings {
            trace_size: 96,
            ..Settings::default()
        };
        let cache = Cache::default();
        let Outcome::Traced(first) = run(&source, &plain, Tier::Final, Some(&cache), |_, _| {})
        else {
            panic!("expected a drawing")
        };

        let coarse = Settings {
            precision: 0.5,
            ..plain.clone()
        };
        let Outcome::Traced(second) = run(&source, &coarse, Tier::Final, Some(&cache), |_, _| {})
        else {
            panic!("expected a drawing")
        };
        assert_ne!(
            first.svg, second.svg,
            "a coarser precision came back as the drawing traced at the finer one"
        );
        // And the pipeline really ran: a cache hit reports no stages.
        assert!(
            !second.stages.is_empty(),
            "the pipeline did not run for a changed setting"
        );
    }
}

/// One trace at a time, and the one somebody is looking at goes first.
///
/// Two things can ask the engine to trace: the Vectorize tab, and a batch run going in the
/// background. Nothing used to stop them running at once, or stop a second interactive
/// trace starting while the first was still going — every request got its own thread, and
/// every one of those threads fed the same global rayon pool. The pipeline has no
/// cancellation point (see [`Generation`]), so a trace nobody wants any more keeps its
/// cores until it finishes: change three controls on a large image and three full traces
/// grind away together, each of them slower for the other two, and the one the user is
/// actually waiting for arrives minutes later. The cost is not one slow trace, it is
/// several slow traces sharing one machine.
///
/// So the pipeline is entered through here. [`Scheduler::interactive`] and
/// [`Scheduler::batch_row`] both wait for the same slot, and while any interactive trace
/// is waiting the batch does not take it — what somebody is watching comes before what is
/// running behind them. A batch row already in flight cannot be interrupted, so the batch
/// yields at its next row boundary and resumes there afterwards, which loses no work: the
/// row that was running finishes and keeps its result.
///
/// Superseded interactive traces need no queue of their own. Each one waits for the slot
/// and then asks [`Generation::is_current`] whether it is still the trace the interface is
/// waiting for; the ones the user has already moved past find that they are not, and stop
/// without tracing anything.
#[derive(Default)]
pub struct Scheduler {
    slots: Mutex<Slots>,
    freed: Condvar,
}

#[derive(Default)]
struct Slots {
    /// A trace is running right now, interactive or batch.
    busy: bool,
    /// Interactive traces waiting for the slot.
    waiting: usize,
}

/// The right to run one trace, given up when it is dropped — including when the pipeline
/// panics, which [`run`] catches.
pub struct Slot(std::sync::Arc<Scheduler>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.lock().busy = false;
        self.0.freed.notify_all();
    }
}

impl Scheduler {
    fn lock(&self) -> std::sync::MutexGuard<'_, Slots> {
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Wait for the slot for an interactive trace. Batch rows wait for these.
    pub fn interactive(self: &std::sync::Arc<Self>) -> Slot {
        let mut slots = self.lock();
        slots.waiting += 1;
        while slots.busy {
            slots = self
                .freed
                .wait(slots)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        slots.waiting -= 1;
        slots.busy = true;
        drop(slots);
        Slot(std::sync::Arc::clone(self))
    }

    /// Wait for the slot for one batch row, yielding to any interactive trace that wants
    /// it. `give_up` is polled while waiting — a run that is cancelled or paused meanwhile
    /// gets `None` and goes back to its own loop rather than holding the queue open.
    pub fn batch_row(self: &std::sync::Arc<Self>, give_up: impl Fn() -> bool) -> Option<Slot> {
        let mut slots = self.lock();
        while slots.busy || slots.waiting > 0 {
            if give_up() {
                return None;
            }
            let (next, _) = self
                .freed
                .wait_timeout(slots, std::time::Duration::from_millis(50))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            slots = next;
        }
        if give_up() {
            return None;
        }
        slots.busy = true;
        drop(slots);
        Some(Slot(std::sync::Arc::clone(self)))
    }

    /// Whether an interactive trace is running or waiting to. What a batch run shows in
    /// its own status while it stands aside.
    pub fn interactive_wants_it(&self) -> bool {
        self.lock().waiting > 0
    }
}

#[cfg(test)]
mod scheduler_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn one_trace_at_a_time() {
        let s = Arc::new(Scheduler::default());
        let first = s.interactive();
        assert!(s.lock().busy);
        // A batch row must not take the slot while the interactive trace holds it.
        assert!(
            s.batch_row(|| true).is_none(),
            "gave up rather than running alongside"
        );
        drop(first);
        assert!(!s.lock().busy);
        assert!(
            s.batch_row(|| false).is_some(),
            "free again once the trace finished"
        );
    }

    #[test]
    fn a_batch_row_waits_while_an_interactive_trace_wants_the_slot() {
        let s = Arc::new(Scheduler::default());
        let held = s.interactive();
        let waiter = {
            let s = Arc::clone(&s);
            std::thread::spawn(move || s.interactive())
        };
        // Wait until the second interactive trace is registered as waiting.
        while !s.interactive_wants_it() {
            std::thread::yield_now();
        }
        // The batch sees interactive demand even though the slot is about to be free.
        assert!(s.batch_row(|| true).is_none());
        drop(held);
        let second = waiter.join().expect("the waiting trace got the slot");
        drop(second);
        assert!(
            s.batch_row(|| false).is_some(),
            "the batch gets it once no trace wants it"
        );
    }

    #[test]
    fn a_cancelled_batch_stops_waiting() {
        let s = Arc::new(Scheduler::default());
        let _held = s.interactive();
        assert!(
            s.batch_row(|| true).is_none(),
            "a cancelled run does not block on the slot"
        );
    }

    #[test]
    fn the_slot_comes_back_when_a_trace_panics() {
        let s = Arc::new(Scheduler::default());
        let s2 = Arc::clone(&s);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _slot = s2.interactive();
            panic!("a trace that failed");
        }));
        assert!(
            !s.lock().busy,
            "the slot is given up even when the pipeline panics"
        );
        assert!(s.batch_row(|| false).is_some());
    }
}

/// The drawing the pipeline last produced, kept so that changing something which only
/// rewrites the finished document does not trace it all over again.
///
/// Two of the controls do not change the drawing at all. Minify rewrites the same geometry
/// in fewer bytes and Margin grows the viewBox: `inkvec_cli::post_process` applies both to
/// a finished document, which is why [`inkvec_cli::Traced::svg`] is documented as the SVG
/// *before* output post-processing. Tracing again to apply them is a minute of arithmetic
/// to produce a drawing identical to the one already in hand.
///
/// Everything else can change the drawing, so everything else is part of the key —
/// including the two that look like output options and are not. Transparent background is
/// one of them: `post_process` knocks out the backing rectangle, but the emitter reads it
/// too (`emit.rs`, the canvas fill and whether holes are punched), so a cached drawing is
/// the wrong drawing for it. Holes as cutouts reads the alpha differently again. The key is
/// therefore the whole `Settings` with exactly the two safe ones blanked out, compared as a
/// whole rather than field by field: a control added later is part of the key by default,
/// and the worst a mistake here can do is trace again when it did not have to.
#[derive(Default)]
pub struct Cache(Mutex<Option<Cached>>);

struct Cached {
    /// Which image. The same file opened again is a different `Arc`, so this is identity
    /// rather than equality and never has to hash a megabyte of PNG.
    source: std::sync::Arc<Source>,
    key: (Settings, Tier),
    svg: String,
    width: usize,
    height: usize,
    stats: Vec<String>,
    bands: Option<String>,
}

/// What a hit gives back: the pipeline's own output, before the output options.
pub struct Reused {
    pub svg: String,
    pub width: usize,
    pub height: usize,
    pub stats: Vec<String>,
    pub bands: Option<String>,
}

/// The settings with the two post-processing controls blanked out: what decides whether two
/// traces would draw the same thing.
fn drawing_key(settings: &Settings, tier: Tier) -> (Settings, Tier) {
    (
        Settings {
            minify: false,
            margin: 0.0,
            ..settings.clone()
        },
        tier,
    )
}

impl Cache {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Cached>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The drawing already in hand for this image and these settings, if there is one.
    pub fn reuse(
        &self,
        source: &std::sync::Arc<Source>,
        settings: &Settings,
        tier: Tier,
    ) -> Option<Reused> {
        let held = self.lock();
        let hit = held.as_ref()?;
        if !std::sync::Arc::ptr_eq(&hit.source, source) || hit.key != drawing_key(settings, tier) {
            return None;
        }
        Some(Reused {
            svg: hit.svg.clone(),
            width: hit.width,
            height: hit.height,
            stats: hit.stats.clone(),
            bands: hit.bands.clone(),
        })
    }

    /// Keep this drawing for the next trace of the same image. One at a time: the only
    /// drawing worth keeping is the one on screen.
    pub fn keep(
        &self,
        source: &std::sync::Arc<Source>,
        settings: &Settings,
        tier: Tier,
        svg: &str,
        width: usize,
        height: usize,
        stats: &[String],
    ) {
        *self.lock() = Some(Cached {
            source: std::sync::Arc::clone(source),
            key: drawing_key(settings, tier),
            svg: svg.to_string(),
            width,
            height,
            stats: stats.to_vec(),
            bands: None,
        });
    }

    /// The confidence bands of the drawing just kept.
    pub fn keep_bands(&self, bands: Option<String>) {
        if let Some(held) = self.lock().as_mut() {
            held.bands = bands;
        }
    }

    /// Forget it. What opening another image does, so a drawing is never held for an image
    /// nobody has open.
    pub fn clear(&self) {
        *self.lock() = None;
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::sync::Arc;

    fn source(byte: u8) -> Arc<Source> {
        Arc::new(Source {
            bytes: vec![byte],
            path: None,
            container: Container::Png,
            width: 4,
            height: 4,
        })
    }

    fn keep(cache: &Cache, src: &Arc<Source>, settings: &Settings) {
        cache.keep(
            src,
            settings,
            Tier::Final,
            "<svg/>",
            8,
            8,
            &["palette 1ms".to_string()],
        );
    }

    #[test]
    fn the_same_settings_come_back() {
        let (cache, src, s) = (Cache::default(), source(1), Settings::default());
        keep(&cache, &src, &s);
        let hit = cache.reuse(&src, &s, Tier::Final).expect("a hit");
        assert_eq!(hit.svg, "<svg/>");
        assert_eq!((hit.width, hit.height), (8, 8));
    }

    #[test]
    fn only_rewriting_the_document_is_still_the_same_drawing() {
        let (cache, src, s) = (Cache::default(), source(1), Settings::default());
        keep(&cache, &src, &s);
        for changed in [
            Settings {
                minify: !s.minify,
                ..s.clone()
            },
            Settings {
                margin: 0.25,
                ..s.clone()
            },
        ] {
            assert!(
                cache.reuse(&src, &changed, Tier::Final).is_some(),
                "an output option must not cost a trace"
            );
        }
    }

    #[test]
    fn anything_that_could_move_a_line_traces_again() {
        let (cache, src, s) = (Cache::default(), source(1), Settings::default());
        keep(&cache, &src, &s);
        for changed in [
            Settings {
                precision: s.precision * 2.0,
                ..s.clone()
            },
            Settings {
                trace_size: s.trace_size / 2,
                ..s.clone()
            },
            Settings {
                max_colours: 3,
                ..s.clone()
            },
            Settings {
                bezier_cost: 3.0,
                ..s.clone()
            },
            Settings {
                corner_angle: 30.0,
                ..s.clone()
            },
            Settings {
                editability: !s.editability,
                ..s.clone()
            },
            // These two look like output options and are not: the emitter reads both.
            // `a_reused_drawing_is_what_a_fresh_trace_would_have_written` caught
            // Transparent background being treated as post-processing.
            Settings {
                holes_as_cutouts: !s.holes_as_cutouts,
                ..s.clone()
            },
            Settings {
                transparent_background: !s.transparent_background,
                ..s.clone()
            },
            Settings {
                clean_up_damage: crate::options::Cleanup::On,
                ..s.clone()
            },
        ] {
            assert!(
                cache.reuse(&src, &changed, Tier::Final).is_none(),
                "a setting that can change the drawing must trace again"
            );
        }
    }

    #[test]
    fn a_draft_is_not_a_final_and_another_image_is_not_this_one() {
        let (cache, src, s) = (Cache::default(), source(1), Settings::default());
        keep(&cache, &src, &s);
        assert!(
            cache.reuse(&src, &s, Tier::Draft).is_none(),
            "a draft is its own drawing"
        );
        assert!(
            cache.reuse(&source(2), &s, Tier::Final).is_none(),
            "another image entirely"
        );
    }

    #[test]
    fn clearing_forgets_it() {
        let (cache, src, s) = (Cache::default(), source(1), Settings::default());
        keep(&cache, &src, &s);
        cache.clear();
        assert!(cache.reuse(&src, &s, Tier::Final).is_none());
    }
}
