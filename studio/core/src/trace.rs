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

use inkvec_core::clock::Instant;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

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

/// What a request from the interface actually runs: the settings and the tier.
///
/// A final is what it says. A draft is the same settings at the draft size with the
/// draft's time limit — except when the image already fits inside the draft size. Then
/// the draft would trace the very raster the final traces, with the same settings, and
/// only the time limit tells them apart; the limit exists to keep a large image live, and
/// a small one does not need it. So such a draft runs as the final itself: the drawing
/// that comes back is the one export would write, it is kept as the final, and the final
/// the interface asks for once the controls settle is then served from the cache instead
/// of tracing the same image a second time. The restorer is the one exception worth
/// naming: a draft never runs it, so with "Clean up damage" on the two differ and the
/// draft stays a draft.
pub fn plan(
    settings: &Settings,
    tier: Tier,
    draft_px: u32,
    draft_seconds: f64,
    source_px: (u32, u32),
) -> (Settings, Tier) {
    match tier {
        Tier::Final => (settings.clone(), Tier::Final),
        Tier::Draft => {
            let draft = settings.draft(draft_px, draft_seconds);
            if draft_is_the_final(settings, &draft, source_px) {
                (settings.clone(), Tier::Final)
            } else {
                (draft, Tier::Draft)
            }
        }
    }
}

/// Whether `draft` would trace exactly what `settings` traces: the image fits inside both
/// sizes, so neither reduces it, and nothing but the time limit differs.
fn draft_is_the_final(settings: &Settings, draft: &Settings, (w, h): (u32, u32)) -> bool {
    let (full, draft) = (settings.clone().sanitised(), draft.clone().sanitised());
    w.max(h) <= draft.trace_size
        && Settings {
            trace_size: full.trace_size,
            time_limit: full.time_limit,
            ..draft
        } == full
}

/// How much of a finished drawing is measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureLevel {
    /// Everything the Vectorize tab shows: the colour differences, the palette, what could
    /// not be recovered, the editability counts and the confidence bands.
    Full,
    /// What a batch row shows: the mean colour difference and the counts. A batch writes
    /// hundreds of files and nobody looks at the palette of each one.
    Summary,
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
///
/// `None` for the one mark that is not a stage: `trace_total` is the whole trace again,
/// and adding it to the last stage would count every millisecond twice.
pub fn stage_label(internal: &str) -> Option<&'static str> {
    Some(match internal {
        "trace_total" => return None,
        "decode" => "intake",
        "palette" | "labels" | "labels_in" | "despeckle" | "blend_absorb" | "merge_bands"
        | "carve" | "split" | "fades" => "palette",
        "saddles" | "build_map" => "planar map",
        "refine_subpix" | "refine_junc" | "boundary_opt" => "boundary solve",
        "symmetry" | "symmetry_detect" => "symmetry",
        "repair" => "repair",
        "fit_dp" => "segments",
        "fills" => "lambda",
        "emit" => "wrote",
        // Unknown: it happened after the palette and before the emit, which is where
        // nearly all of the pipeline lives.
        _ => "boundary solve",
    })
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
    /// The rasters already decoded from `bytes`, by the cap they were decoded at. A trace
    /// and its measurement read the same raster, and a draft and a final of a small image
    /// read the same one too, so each is decoded once rather than once per use.
    rasters: Mutex<Vec<(usize, Arc<inkvec_trace::Rgba>)>>,
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

/// How many decoded rasters a source keeps: a draft's and a final's.
const RASTERS_KEPT: usize = 2;

impl Source {
    /// Read an image from bytes, refusing early and in words if it is not an image.
    ///
    /// Only the header is read here: the size is what opening needs, and a 6000 px photo
    /// takes a third of a second to decode for no other reason. The pixels are decoded
    /// when a trace first wants them (see [`Source::raster`]), and a file whose header
    /// reads but whose pixels do not is reported then, as an image that will not decode.
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
        // The same header read `inkvec_trace::decode_image_capped` reports its size from,
        // so the size here is the one every trace of this file will see. Neither applies
        // an EXIF orientation, so nothing is swapped.
        let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|e| describe_decode_failure(container, &e.to_string()))?
            .into_dimensions()
            .map_err(|e| describe_decode_failure(container, &e.to_string()))?;
        Ok(Self::from_parts(bytes, path, container, width, height))
    }

    /// A source from what is already known about it, with nothing decoded yet.
    pub fn from_parts(
        bytes: Vec<u8>,
        path: Option<std::path::PathBuf>,
        container: Container,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            bytes,
            path,
            container,
            width,
            height,
            rasters: Mutex::new(Vec::new()),
        }
    }

    /// The pixels, with the longer side capped at `max_dim` (0 for no cap), exactly as
    /// `inkvec_trace::decode_image_capped` produces them. Decoded on first use and kept.
    pub fn raster(&self, max_dim: usize) -> Result<Arc<inkvec_trace::Rgba>, String> {
        // A cap the image already fits inside changes nothing, so it shares the uncapped
        // raster: a draft and a final of a small image are one decode.
        let key = if max_dim == 0 || (self.width.max(self.height) as usize) <= max_dim {
            0
        } else {
            max_dim
        };
        let lock = || {
            self.rasters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        if let Some((_, r)) = lock().iter().find(|(k, _)| *k == key) {
            return Ok(Arc::clone(r));
        }
        // Decoded outside the lock: a measurement reading the raster it already has must
        // not wait for the next trace's decode at another size.
        let (img, _) =
            inkvec_trace::decode_image_capped(&self.bytes, key).map_err(|e| e.to_string())?;
        let img = Arc::new(img);
        let mut held = lock();
        if let Some((_, r)) = held.iter().find(|(k, _)| *k == key) {
            return Ok(Arc::clone(r));
        }
        if held.len() >= RASTERS_KEPT {
            held.remove(0);
        }
        held.push((key, Arc::clone(&img)));
        Ok(img)
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
#[cfg(not(target_arch = "wasm32"))]
const MEMORY_CEILING: u64 = 8 * 1024 * 1024 * 1024;
/// In a browser tab the whole address space is 4 GiB (wasm32), shared with the module, the
/// source bytes and every drawing held, so the ceiling is lower.
#[cfg(target_arch = "wasm32")]
const MEMORY_CEILING: u64 = 3 * 1024 * 1024 * 1024;

/// Run one trace to completion on the calling thread, and measure everything about it.
///
/// `on_stage` is called from inside the pipeline as each stage is passed; it should hand
/// the value off and return.
pub fn run(
    source: &Arc<Source>,
    settings: &Settings,
    tier: Tier,
    cache: Option<&Cache>,
    on_stage: impl Fn(&'static str, f64) + 'static,
) -> Outcome {
    run_at(source, settings, tier, cache, MeasureLevel::Full, on_stage)
}

/// [`run`], measuring only as much as `level` asks for.
pub fn run_at(
    source: &Arc<Source>,
    settings: &Settings,
    tier: Tier,
    cache: Option<&Cache>,
    level: MeasureLevel,
    on_stage: impl Fn(&'static str, f64) + 'static,
) -> Outcome {
    match draw(source, settings, tier, cache, level, on_stage) {
        Ok(drawn) => measure(source, drawn, level, cache),
        Err(outcome) => outcome,
    }
}

/// A finished drawing that has not been measured yet.
///
/// Drawing and measuring are separate steps because they want different things. Drawing
/// is the pipeline, and only one of those may run at a time (see [`Scheduler`]).
/// Measuring renders the drawing back and compares it with the source, which is a
/// fraction of the cost and needs no slot: the next trace can start while this one is
/// being measured, and a drawing nobody is waiting for any more need not be measured at
/// all.
pub struct Drawn {
    settings: Settings,
    args: inkvec_cli::Args,
    tier: Tier,
    /// The SVG as it will be written: the output options applied.
    svg: String,
    /// The same drawing without the two output-only options, Minify and Margin, when
    /// either is on. The report is measured on this one, so that what it says about the
    /// drawing does not move when only the file's spelling or its canvas does.
    unstyled: Option<String>,
    traced_w: u32,
    traced_h: u32,
    engine_log: Vec<String>,
    stages: Vec<Stage>,
    bands: Option<String>,
    seconds: f64,
    /// What an earlier trace of this very drawing measured, when the cache had it.
    measured: Option<Arc<Measured>>,
}

/// Trace, or take the drawing from the cache, and apply the output options. What runs
/// inside the trace slot.
pub fn draw(
    source: &Arc<Source>,
    settings: &Settings,
    tier: Tier,
    cache: Option<&Cache>,
    level: MeasureLevel,
    on_stage: impl Fn(&'static str, f64) + 'static,
) -> Result<Drawn, Outcome> {
    let settings = settings.clone().sanitised();
    let started = Instant::now();

    let needed = memory_estimate(source.width, source.height, settings.trace_size);
    if needed > MEMORY_CEILING {
        // The largest size that fits, rounded down to something a slider can land on.
        let ratio = (MEMORY_CEILING as f64 / needed as f64).sqrt();
        let suggest = (((settings.trace_size as f64 * ratio) as u32) / 256 * 256).max(256);
        return Err(Outcome::OutOfMemory {
            needed_gb: needed as f64 / (1024.0 * 1024.0 * 1024.0),
            suggest_px: suggest,
        });
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
    let (raw_svg, raw_w, raw_h, engine_log, stages, bands, measured) = match reused {
        Some(hit) => (
            hit.svg,
            hit.width,
            hit.height,
            hit.stats,
            Vec::new(),
            hit.bands,
            hit.measured,
        ),
        None => {
            let bands = level == MeasureLevel::Full;
            let t = trace_pipeline(source, &args, bands, on_stage)?;
            if let Some(c) = cache {
                c.keep(source, &settings, tier, &t.svg, t.width, t.height, &t.stats);
                c.keep_bands(t.bands.clone());
            }
            (t.svg, t.width, t.height, t.stats, t.stages, t.bands, None)
        }
    };

    // Measured on the drawing with neither output-only option; written with both.
    let styled = args.minify || args.margin != 0.0;
    let unstyled = (styled && measured.is_none()).then(|| {
        let plain = inkvec_cli::Args {
            minify: false,
            margin: 0.0,
            ..args.clone()
        };
        inkvec_cli::post_process(&plain, raw_svg.clone(), raw_w, raw_h)
    });
    let svg = inkvec_cli::post_process(&args, raw_svg, raw_w, raw_h);
    Ok(Drawn {
        settings,
        args,
        tier,
        svg,
        unstyled,
        traced_w: raw_w as u32,
        traced_h: raw_h as u32,
        engine_log,
        stages,
        bands,
        seconds: started.elapsed().as_secs_f64(),
        measured,
    })
}

/// What was produced and how it compares: the quality report, the palette and what could
/// not be recovered. Every one of these can fail on a document resvg will not take, and
/// none of that should cost the user the trace, so each degrades to "not measured".
///
/// What does not depend on the output options is measured once per drawing and kept in
/// the cache beside it, so toggling Minify or Margin re-measures only the file itself.
pub fn measure(
    source: &Arc<Source>,
    drawn: Drawn,
    level: MeasureLevel,
    cache: Option<&Cache>,
) -> Outcome {
    let Drawn {
        settings,
        args,
        tier,
        svg,
        unstyled,
        traced_w,
        traced_h,
        engine_log,
        stages,
        bands,
        seconds,
        measured,
    } = drawn;

    let full = level == MeasureLevel::Full;
    // Neither output-only option on: the file is the drawing the report measures, and its
    // minified size may already be known from an earlier measurement of it.
    let plain = !args.minify && args.margin == 0.0;
    let known = measured
        .as_ref()
        .filter(|_| plain)
        .and_then(|m| m.plain_minified.get().copied());
    // The comparison with the source and the counts of the file are independent of each
    // other, so they run side by side.
    let (measured, (counts, minified_bytes, structure)) = rayon::join(
        || match measured {
            Some(m) => m,
            None => {
                let target = unstyled.as_deref().unwrap_or(&svg);
                let m = Arc::new(Measured::of(
                    source, &settings, &args, target, traced_w, traced_h, level,
                ));
                if full {
                    if let Some(c) = cache {
                        c.keep_measured(source, &settings, tier, Arc::clone(&m));
                    }
                }
                m
            }
        },
        || {
            if full {
                // The minifier is the slow one on a large drawing; it gets a task of its
                // own.
                let (minified, (counts, structure)) = rayon::join(
                    || known.unwrap_or_else(|| minified_size(&svg, args.minify)),
                    || (quality::count(&svg), inkvec_svgmin::structure(&svg).into()),
                );
                (counts, minified, structure)
            } else {
                (quality::count(&svg), None, Default::default())
            }
        },
    );

    if full && plain {
        let _ = measured.plain_minified.set(minified_bytes);
    }

    let (coordinates, segments, paths, colours) = counts;
    let report = Report {
        mean_de00: measured.mean,
        median_de00: measured.median,
        worst_de00: measured.worst,
        coordinates,
        paths,
        segments,
        colours,
        bytes: svg.len(),
        minified_bytes,
        structure,
        seconds,
        traced_px: traced_w.max(traced_h),
    };

    Outcome::Traced(Box::new(Traced {
        tier: tier.label(),
        svg,
        report,
        palette: measured.palette.clone(),
        losses: measured.losses.clone(),
        worst_corner: measured.corner,
        stages,
        engine_log,
        bands,
        traced_px: traced_w.max(traced_h),
        oversized: scaled_to(source.width, source.height, args.max_dim as u32).is_some(),
        source_px: (source.width, source.height),
    }))
}

/// What measuring a drawing found, apart from the counts of the file itself.
#[derive(Clone, Debug)]
pub struct Measured {
    /// Mean dE00, if the drawing could be rendered back.
    pub mean: Option<f64>,
    /// Median dE00.
    pub median: Option<f64>,
    /// 99th-percentile dE00.
    pub worst: Option<f64>,
    /// Where the two disagree most.
    pub corner: Option<WorstCorner>,
    /// The inks, largest share first. Empty at [`MeasureLevel::Summary`].
    pub palette: Vec<Ink>,
    /// What could not be recovered. Empty at [`MeasureLevel::Summary`].
    pub losses: Vec<Loss>,
    /// The minified size of the drawing written with neither output-only option, once
    /// it has been worked out: the figure beside the file size when Minify is turned off
    /// again. The minifier is the slowest of the counts on a large drawing.
    pub plain_minified: std::sync::OnceLock<Option<usize>>,
}

impl Measured {
    fn of(
        source: &Source,
        settings: &Settings,
        args: &inkvec_cli::Args,
        svg: &str,
        traced_w: u32,
        traced_h: u32,
        level: MeasureLevel,
    ) -> Self {
        let full = level == MeasureLevel::Full;
        let raster = source.raster(args.max_dim).ok();
        // The palette is a small render of its own and needs nothing from the comparison,
        // so it is worked out beside it; what could not be recovered reads the comparison
        // and follows it.
        let ((analysis, losses), palette) = rayon::join(
            || {
                let analysis = raster
                    .as_deref()
                    .and_then(|r| quality::analyse(r, svg).ok());
                let losses = match (full, raster.as_deref(), analysis.as_ref()) {
                    (true, Some(r), Some(a)) => lost::detect(r, a, svg, settings, source.container),
                    _ => Vec::new(),
                };
                (analysis, losses)
            },
            || {
                if full {
                    quality::palette(svg, traced_w, traced_h).unwrap_or_default()
                } else {
                    Vec::new()
                }
            },
        );
        Self {
            mean: analysis.as_ref().map(|a| a.mean),
            median: analysis.as_ref().map(|a| a.median),
            worst: analysis.as_ref().map(|a| a.worst),
            corner: analysis.as_ref().and_then(|a| a.corner),
            palette,
            losses,
            plain_minified: std::sync::OnceLock::new(),
        }
    }
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
///
/// `bands` asks the engine for its confidence bands as well. They do not change the
/// drawing; a batch, which never shows them, does not ask.
fn trace_pipeline(
    source: &Source,
    args: &inkvec_cli::Args,
    bands: bool,
    on_stage: impl Fn(&'static str, f64) + 'static,
) -> Result<Pipeline, Outcome> {
    // Stages are reported from inside the pipeline, on this thread, as each one is
    // passed. They are folded onto the nine names the interface shows — accumulating
    // time where several pipeline steps map to one name — and handed straight on, so the
    // rail fills while the trace runs rather than after it.
    // The bands come back through a file, as the command line writes them.
    // A browser has no file system to receive them through; see `bands_path`.
    let bands_file = (bands && cfg!(not(target_arch = "wasm32"))).then(bands_path);
    let mut args = args.clone();
    args.uncertainty = bands_file.clone();
    let args = &args;
    let collected: std::rc::Rc<std::cell::RefCell<Vec<Stage>>> = Default::default();
    let store = std::rc::Rc::clone(&collected);
    let traced = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        inkvec_trace::with_stage_sink(
            move |internal, ms| {
                let Some(name) = stage_label(internal) else {
                    return;
                };
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
                // The pipeline takes its raster by value, so it gets a copy of the one the
                // source keeps; the measurement afterwards reads the kept one.
                let kept: Arc<inkvec_trace::Rgba> = source.raster(args.max_dim)?;
                let img = inkvec_trace::Rgba::clone(&kept);
                drop(kept);
                let size = Some((source.width as usize, source.height as usize));
                let out = match external_denoiser() {
                    Some(denoise) if args.restore != inkvec_restore::Mode::Off => {
                        trace_denoised(img, args, size, denoise)?
                    }
                    _ => inkvec_cli::trace_image_sized(img, args, size)?,
                };
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

    let bands = bands_file.and_then(|file| {
        let text = std::fs::read_to_string(&file).ok();
        let _ = std::fs::remove_file(&file);
        text
    });
    Ok(Pipeline {
        svg: traced.svg,
        stats: traced.stats,
        width: traced.width,
        height: traced.height,
        stages,
        bands,
    })
}

/// A denoiser the shell runs itself, outside the engine: the network's input tensor in
/// (`1 x 3 x height x width`, planar, as `inkvec_restore::network_input` lays it out, with
/// the padded width and height), its output tensor back, the same layout.
///
/// The desktop app never sets one: its denoiser is the engine's own (`inkvec-cli`'s
/// `restore-model` feature), which `--restore` reaches inside the pipeline. A browser
/// cannot link ONNX Runtime into the tracer; it runs the same `restorer.onnx` in ONNX
/// Runtime Web instead, and hands the tensor across through this.
pub type ExternalDenoiser = fn(&[f32], usize, usize) -> Result<Vec<f32>, String>;

static EXTERNAL_DENOISER: std::sync::OnceLock<ExternalDenoiser> = std::sync::OnceLock::new();

/// Install the shell's denoiser, once, for every trace from here on that asks for one.
pub fn set_external_denoiser(denoise: ExternalDenoiser) {
    let _ = EXTERNAL_DENOISER.set(denoise);
}

fn external_denoiser() -> Option<ExternalDenoiser> {
    EXTERNAL_DENOISER.get().copied()
}

/// `--restore` with the network run by [`ExternalDenoiser`]: the engine's own seam, split
/// where the engine documents it for exactly this caller (`inkvec_cli::intake`).
///
/// The same decisions as the engine's pre-pass: `auto` traces once, measures how far the
/// raster disagrees with that trace where it claims a flat interior, and keeps the trace
/// when the residual is under the threshold; otherwise, and always for `on`, the raster is
/// denoised at the size the tracer will see it and traced with soft intake forced on.
fn trace_denoised(
    img: inkvec_trace::Rgba,
    args: &inkvec_cli::Args,
    size: Option<(usize, usize)>,
    denoise: ExternalDenoiser,
) -> Result<inkvec_cli::Traced, Box<dyn std::error::Error>> {
    let mut intake = inkvec_cli::intake(img, args, size);
    let mode = intake.args.restore;
    intake.args.restore = inkvec_restore::Mode::Off;
    let mut note = String::new();
    if mode == inkvec_restore::Mode::Auto {
        let mut probe = inkvec_cli::trace_prepared(intake.clone())?;
        let decision = inkvec_restore::decide(
            &intake.img,
            &probe.svg,
            inkvec_restore::Options {
                residual_threshold: args.restore_threshold,
            },
        );
        match decision {
            inkvec_restore::Decision::Keep { residual } => {
                probe.stats.insert(
                    0,
                    match residual {
                        Some(r) => format!(
                            "restore       residual {r:.3} <= {:.3}, traced directly",
                            args.restore_threshold
                        ),
                        None => "restore       could not measure the fit; traced directly".into(),
                    },
                );
                return Ok(probe);
            }
            inkvec_restore::Decision::Restore { residual } => {
                if let Some(r) = residual {
                    note = format!("residual {r:.3} > {:.3}; ", args.restore_threshold);
                }
            }
        }
    }
    let started = Instant::now();
    let tensor = inkvec_restore::network_input(&intake.img);
    let out = denoise(&tensor.data, tensor.width, tensor.height)?;
    intake.img = inkvec_restore::network_output(&out, &intake.img)?;
    // Restored input is traced with soft intake on, as the engine's pre-pass forces it.
    intake.args.lossy = inkvec_sr::Mode::On;
    let (w, h) = (intake.img.width, intake.img.height);
    let mut traced = inkvec_cli::trace_prepared(intake)?;
    traced.stats.insert(
        0,
        format!(
            "restore       {note}restored {w}x{h} in {:.2}s, in the browser",
            started.elapsed().as_secs_f64()
        ),
    );
    Ok(traced)
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
            "something_new_in_2027",
        ];
        for name in internal {
            let label = stage_label(name).expect("a stage");
            assert!(
                STAGES.contains(&label),
                "{name} -> {label}, not a named stage"
            );
        }
        // The whole trace's own total is not a stage: counted as one, the rail showed
        // every trace taking twice as long as it did.
        assert_eq!(stage_label("trace_total"), None);
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

    /// The palette panel's round trip: inks read off one trace, sent back as a colour group,
    /// come back from the next trace as one ink, with the engine's line saying so.
    #[test]
    fn grouping_the_inks_of_a_trace_merges_them_in_the_next() {
        // Two reds a little apart side by side on a light ground, and a blue apart from both.
        let mut img = image::RgbaImage::from_pixel(96, 96, image::Rgba([245, 242, 234, 255]));
        for y in 16..80u32 {
            for x in 8..88u32 {
                let c = match x {
                    8..=35 => [200, 40, 40],
                    36..=63 => [170, 30, 60],
                    _ => [30, 60, 170],
                };
                img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let source = std::sync::Arc::new(Source::open(png.into_inner(), None).unwrap());
        let traced =
            |settings: &Settings| match run(&source, settings, Tier::Final, None, |_, _| {}) {
                Outcome::Traced(t) => t,
                other => panic!("expected a drawing, got {other:?}"),
            };

        let before = traced(&Settings::default());
        assert!(
            before
                .engine_log
                .iter()
                .all(|l| !l.starts_with("merge colors")),
            "no groups, no merge: {:?}",
            before.engine_log
        );
        let red = |t: &Traced, want: [f32; 3]| {
            t.palette
                .iter()
                .min_by(|a, b| {
                    let d = |i: &Ink| {
                        quality::hex_distance(&i.traced, &quality::to_hex(want)).unwrap_or(f64::MAX)
                    };
                    d(a).total_cmp(&d(b))
                })
                .unwrap()
                .traced
                .clone()
        };
        let a = red(&before, [200.0 / 255.0, 40.0 / 255.0, 40.0 / 255.0]);
        let b = red(&before, [170.0 / 255.0, 30.0 / 255.0, 60.0 / 255.0]);
        assert_ne!(a, b, "{:?}", before.palette);

        let grouped = Settings {
            colour_groups: vec![crate::options::ColourGroup {
                members: vec![a.clone(), b.clone()],
                target: Some("@1".into()),
            }],
            ..Settings::default()
        };
        let after = traced(&grouped);
        let line = after
            .engine_log
            .iter()
            .find(|l| l.starts_with("merge colors"))
            .unwrap_or_else(|| panic!("the engine says what it merged: {:?}", after.engine_log));
        assert!(line.contains(&a) && line.contains(&b), "{line}");
        assert_eq!(
            after.palette.len() + 1,
            before.palette.len(),
            "two inks became one: {:?} then {:?}",
            before.palette,
            after.palette
        );
        assert!(
            after.palette.iter().all(|i| i.traced != b),
            "{:?}",
            after.palette
        );
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

    /// The report's colour differences and the palette, bit for bit, as one string.
    fn numbers(t: &Traced) -> String {
        format!(
            "{:?} {:?} {:?} {:?}",
            t.report.mean_de00.map(f64::to_bits),
            t.report.median_de00.map(f64::to_bits),
            t.report.worst_de00.map(f64::to_bits),
            t.palette
                .iter()
                .map(|i| (i.traced.as_str(), i.share.to_bits()))
                .collect::<Vec<_>>(),
        )
    }

    /// The report describes the drawing, not its spelling or its canvas: Minify and Margin
    /// leave every colour-difference number and the palette exactly where they were, and a
    /// measurement served from the cache is the measurement made from cold.
    #[test]
    fn output_options_do_not_move_the_report_and_the_cache_measures_the_same() {
        let source = Arc::new(Source::open(sample_png(), None).unwrap());
        let plain = Settings {
            trace_size: 96,
            ..Settings::default()
        };
        let cache = Cache::default();
        let Outcome::Traced(first) = run(&source, &plain, Tier::Final, Some(&cache), |_, _| {})
        else {
            panic!("expected a drawing")
        };
        for changed in [
            Settings {
                minify: true,
                ..plain.clone()
            },
            Settings {
                margin: 0.1,
                ..plain.clone()
            },
        ] {
            let Outcome::Traced(hit) = run(&source, &changed, Tier::Final, Some(&cache), |_, _| {})
            else {
                panic!("expected a drawing")
            };
            let Outcome::Traced(fresh) = run(&source, &changed, Tier::Final, None, |_, _| {})
            else {
                panic!("expected a drawing")
            };
            assert_eq!(numbers(&hit), numbers(&first), "{changed:?}");
            assert_eq!(numbers(&fresh), numbers(&first), "{changed:?}");
            assert_eq!(hit.report.bytes, fresh.report.bytes);
            assert_eq!(hit.report.coordinates, fresh.report.coordinates);
        }
        cache.forget_measurements();
        let Outcome::Traced(cold) = run(&source, &plain, Tier::Final, Some(&cache), |_, _| {})
        else {
            panic!("expected a drawing")
        };
        assert_eq!(numbers(&cold), numbers(&first));
        assert_eq!(cold.losses.len(), first.losses.len());
    }

    /// A batch measures less, and draws exactly the same thing.
    #[test]
    fn a_summary_measures_the_mean_and_draws_the_same_drawing() {
        let source = Arc::new(Source::open(sample_png(), None).unwrap());
        let Outcome::Traced(full) =
            run(&source, &Settings::default(), Tier::Final, None, |_, _| {})
        else {
            panic!("expected a drawing")
        };
        let Outcome::Traced(summary) = run_at(
            &source,
            &Settings::default(),
            Tier::Final,
            None,
            MeasureLevel::Summary,
            |_, _| {},
        ) else {
            panic!("expected a drawing")
        };
        assert_eq!(summary.svg, full.svg);
        assert_eq!(summary.report.mean_de00, full.report.mean_de00);
        assert_eq!(summary.report.coordinates, full.report.coordinates);
        assert!(summary.palette.is_empty() && summary.bands.is_none());
        assert!(full.bands.is_some(), "a colour trace has its bands");
    }

    /// The raster is decoded once per size and shared by everything that reads it.
    #[test]
    fn a_source_decodes_each_size_once() {
        let source = Source::open(sample_png(), None).unwrap();
        let a = source.raster(2048).unwrap();
        let b = source.raster(0).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "a cap the image fits inside is no cap");
        let small = source.raster(64).unwrap();
        assert_eq!((small.width, small.height), (64, 64));
        assert!(Arc::ptr_eq(&small, &source.raster(64).unwrap()));
        let (direct, _) = inkvec_trace::decode_image_capped(&source.bytes, 64).unwrap();
        assert_eq!(
            small.data, direct.data,
            "the kept raster is the engine's own"
        );
    }

    /// Opening reads only the header, so a file whose pixels are broken opens, and says
    /// it will not decode when a trace first needs them.
    #[test]
    fn a_file_that_will_not_decode_is_reported_when_traced() {
        let mut png = sample_png();
        png.truncate(png.len() / 2);
        let source = Arc::new(Source::open(png, None).expect("the header is intact"));
        assert_eq!((source.width, source.height), (96, 96));
        let outcome = run(&source, &Settings::default(), Tier::Final, None, |_, _| {});
        let Outcome::Undecodable { message } = outcome else {
            panic!("expected undecodable, got {outcome:?}")
        };
        assert!(message.contains("PNG, JPEG, WebP"), "{message}");
    }

    #[test]
    fn a_draft_of_an_image_that_fits_the_draft_size_is_the_final() {
        let s = Settings::default();
        // 96 px fits inside the 512 px draft: the draft is the final.
        let (settings, tier) = plan(&s, Tier::Draft, 512, 0.4, (96, 96));
        assert_eq!(tier, Tier::Final);
        assert_eq!(settings, s, "and it runs with the final's own settings");
        // Larger than the draft size: a real draft.
        let (settings, tier) = plan(&s, Tier::Draft, 512, 0.4, (600, 300));
        assert_eq!(tier, Tier::Draft);
        assert_eq!(settings, s.draft(512, 0.4));
        // The restorer only runs for a final, so with it on the two differ.
        let restoring = Settings {
            clean_up_damage: crate::options::Cleanup::On,
            ..Settings::default()
        };
        assert_eq!(
            plan(&restoring, Tier::Draft, 512, 0.4, (96, 96)).1,
            Tier::Draft
        );
        // A final is always a final.
        assert_eq!(
            plan(&s, Tier::Final, 512, 0.4, (4000, 4000)),
            (s.clone(), Tier::Final)
        );
    }

    /// The whole point of the rule above: a control change on a small image traces it once.
    #[test]
    fn a_small_image_is_traced_once_for_a_draft_and_the_final_after_it() {
        let source = Arc::new(Source::open(sample_png(), None).unwrap());
        let s = Settings::default();
        let cache = Cache::default();
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut drawings = Vec::new();
        for asked in [Tier::Draft, Tier::Final] {
            let (settings, tier) = plan(&s, asked, 512, 0.4, (source.width, source.height));
            let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = Arc::clone(&seen);
            let Outcome::Traced(t) = run(&source, &settings, tier, Some(&cache), move |_, _| {
                flag.store(true, Ordering::SeqCst);
            }) else {
                panic!("expected a drawing")
            };
            runs.fetch_add(seen.load(Ordering::SeqCst) as usize, Ordering::SeqCst);
            drawings.push(t.svg);
        }
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the final was served from the cache"
        );
        assert_eq!(drawings[0], drawings[1]);
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
    /// What the drawing measured, once it has been. The same for every output option,
    /// because it is measured on the drawing without them (see [`Drawn`]).
    measured: Option<Arc<Measured>>,
}

/// What a hit gives back: the pipeline's own output, before the output options.
pub struct Reused {
    pub svg: String,
    pub width: usize,
    pub height: usize,
    pub stats: Vec<String>,
    pub bands: Option<String>,
    pub measured: Option<Arc<Measured>>,
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
            measured: hit.measured.clone(),
        })
    }

    /// Keep this drawing for the next trace of the same image. One at a time: the only
    /// drawing worth keeping is the one on screen.
    #[allow(clippy::too_many_arguments)]
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
            measured: None,
        });
    }

    /// The confidence bands of the drawing just kept.
    pub fn keep_bands(&self, bands: Option<String>) {
        if let Some(held) = self.lock().as_mut() {
            held.bands = bands;
        }
    }

    /// What this drawing measured, kept beside it — if the drawing in hand is still this
    /// one. The measurement runs outside the trace slot, so another trace may have
    /// replaced the drawing meanwhile, and its measurement must not be taken for this.
    pub fn keep_measured(
        &self,
        source: &Arc<Source>,
        settings: &Settings,
        tier: Tier,
        measured: Arc<Measured>,
    ) {
        if let Some(held) = self.lock().as_mut() {
            if Arc::ptr_eq(&held.source, source) && held.key == drawing_key(settings, tier) {
                held.measured = Some(measured);
            }
        }
    }

    /// Drop the measurement but keep the drawing, so the next use measures from cold.
    /// For pricing and testing the measurement itself.
    pub fn forget_measurements(&self) {
        if let Some(held) = self.lock().as_mut() {
            held.measured = None;
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
        Arc::new(Source::from_parts(vec![byte], None, Container::Png, 4, 4))
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
