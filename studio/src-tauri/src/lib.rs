//! Inkvec Studio Lite.
//!
//! A desktop app that turns a raster logo into an SVG, exactly, and minimises SVGs you
//! already have. This module is the seam between the interface and the engine: every
//! command here is a small, explicit piece of work with a name, and nothing in the
//! interface can reach the tracer except through one of them.
//!
//! Two rules hold throughout.
//!
//! **Nothing is uploaded, ever.** The only outbound requests the whole binary can make are
//! the update check in [`check_update`] and the optional denoiser download, and both say
//! exactly what they send. There is no account, no telemetry and no analytics, and an
//! image is never a payload.
//!
//! **Long work does not block the window.** Tracing and batch runs happen on their own
//! threads and report back as events, so the app stays answerable — the cancel button in
//! particular has to be reachable while a trace is running, which it cannot be if the
//! trace is holding the command loop.

pub mod batch;
pub mod denoiser;
pub mod export;
pub mod lost;
pub mod minify;
pub mod options;
pub mod quality;
pub mod settings;
pub mod trace;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// Everything the app holds between commands.
#[derive(Default)]
pub struct AppState {
    /// The image currently open, if any.
    source: Mutex<Option<Arc<trace::Source>>>,
    /// Which trace the interface is waiting for.
    generation: trace::Generation,
    /// Preferences, as loaded and as edited.
    prefs: Mutex<settings::Prefs>,
    /// The controls of a batch run, while one is going.
    batch: Mutex<Option<Arc<batch::Controls>>>,
    /// The last batch run's rows, so a finished run can still be read and exported.
    batch_rows: Mutex<Vec<batch::Row>>,
    /// Set to stop a denoiser download.
    denoiser_stop: Arc<AtomicBool>,
}

/// What this build can do, sent once at startup so the interface never has to guess.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// The app's version.
    pub version: &'static str,
    /// The engine's version: the same tree, but worth naming separately on About.
    pub engine_version: &'static str,
    /// The target the engine was compiled for. Two builds write byte-identical SVG only
    /// when this string matches, which is worth showing where anyone compares outputs.
    pub build_target: String,
    /// The operating system, for the few places the interface says ⌘ rather than Ctrl.
    pub platform: &'static str,
    /// The advanced drawer's rows.
    pub controls: &'static [options::Control],
    /// The presets, with their names and subtitles.
    pub presets: Vec<PresetInfo>,
    /// The stage names the progress list shows, in order.
    pub stages: [&'static str; 9],
    /// Where the denoiser stands.
    pub denoiser: denoiser::Status,
}

/// One preset, as the tray shows it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetInfo {
    /// Its identifier, as the frontend sends it back.
    pub id: options::Preset,
    /// Its plain-language name.
    pub name: &'static str,
    /// The one line under the name.
    pub subtitle: &'static str,
    /// The settings it means, so the drawer can show them without a round trip.
    pub settings: options::Settings,
    /// Whether it wants the denoiser to do what it says.
    pub wants_denoiser: bool,
}

/// An image that has been opened.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    /// The file's name.
    pub name: String,
    /// Where it came from, if it came from disk.
    pub path: Option<PathBuf>,
    /// Its width as it arrived.
    pub width: u32,
    /// Its height as it arrived.
    pub height: u32,
    /// What the file is: "PNG", "JPEG", ...
    pub container: &'static str,
    /// Whether the container compressed it lossily.
    pub lossy: bool,
    /// The pixels, as a `data:` URL, for the viewer's left pane. Capped on the longer
    /// side: the pane is a few hundred pixels and an 8000 px source would cost tens of
    /// megabytes to hand across for no visible gain.
    pub preview: String,
}

/// One bundled sample.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleInfo {
    /// Its file name, which is what `open_sample` takes.
    pub file: String,
    /// What the first-run screen calls it.
    pub label: String,
    /// The image itself, for the thumbnail.
    pub preview: String,
}

// ------------------------------------------------------------------------- startup ---

/// Build the app and run it.
///
/// # Panics
///
/// If the webview cannot be created, which is not a condition the app can continue past.
pub fn run() {
    denoiser::announce_to_engine();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::default())
        .setup(|app| {
            let prefs = settings::load();
            configure_threads(&prefs);
            let state = app.state::<AppState>();
            *state.prefs.lock().expect("preferences lock") = prefs;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            capabilities,
            open_path,
            open_bytes,
            open_sample,
            list_samples,
            start_trace,
            cancel_trace,
            snap_inks,
            match_palette,
            plan_export,
            write_export,
            minify_svg,
            read_text_file,
            save_bytes,
            batch_scan,
            batch_start,
            batch_pause,
            batch_cancel,
            batch_stats_csv,
            denoiser_status,
            denoiser_download,
            denoiser_cancel,
            denoiser_remove,
            load_prefs,
            save_prefs,
            reset_prefs,
            third_party_notices,
            check_update,
        ])
        .run(tauri::generate_context!())
        .expect("Inkvec Studio Lite could not start its window");
}

/// Hold the tracer's thread pool to the number of threads the user asked for.
///
/// Rayon's global pool is built once, on first use, so this only works before any trace
/// has run — which is why it happens in setup. A failure here is not worth stopping for:
/// the pool then has its default size, which is every core.
fn configure_threads(prefs: &settings::Prefs) {
    if let Some(n) = prefs.threads {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global();
    }
}

// ------------------------------------------------------------------------ commands ---

#[tauri::command]
fn capabilities() -> Capabilities {
    Capabilities {
        version: env!("CARGO_PKG_VERSION"),
        engine_version: env!("CARGO_PKG_VERSION"),
        build_target: build_target(),
        platform: std::env::consts::OS,
        controls: options::CONTROLS,
        presets: options::Preset::ALL
            .iter()
            .map(|p| {
                let (name, subtitle) = p.labels();
                PresetInfo {
                    id: *p,
                    name,
                    subtitle,
                    settings: p.settings(),
                    wants_denoiser: p.wants_denoiser(),
                }
            })
            .collect(),
        stages: trace::STAGES,
        denoiser: denoiser::status(),
    }
}

/// `arch-os-env`, the string two builds must share to write byte-identical SVG.
fn build_target() -> String {
    let env = if cfg!(target_env = "msvc") {
        "-msvc"
    } else if cfg!(target_env = "gnu") {
        "-gnu"
    } else if cfg!(target_env = "musl") {
        "-musl"
    } else {
        ""
    };
    format!("{}-{}{env}", std::env::consts::ARCH, std::env::consts::OS)
}

#[tauri::command]
fn open_path(path: PathBuf, state: State<'_, AppState>) -> Result<SourceInfo, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let info = adopt(trace::Source::open(bytes, Some(path.clone()))?, &state)?;
    {
        let mut prefs = state.prefs.lock().map_err(lock)?;
        prefs.remember(&path);
        let _ = settings::save(&prefs);
    }
    Ok(info)
}

#[tauri::command]
fn open_bytes(
    bytes: Vec<u8>,
    name: Option<String>,
    state: State<'_, AppState>,
) -> Result<SourceInfo, String> {
    adopt(trace::Source::open(bytes, name.map(PathBuf::from))?, &state)
}

#[tauri::command]
fn open_sample(
    name: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SourceInfo, String> {
    let path = sample_path(&app, &name)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read the sample {name}: {e}"))?;
    adopt(trace::Source::open(bytes, Some(path))?, &state)
}

/// The bundled samples, for the first-run screen.
#[tauri::command]
fn list_samples(app: AppHandle) -> Vec<SampleInfo> {
    // Names and labels are fixed rather than read off the directory: the order on the
    // first-run screen is deliberate, and a stray file in the resource folder should not
    // appear as something we chose to offer.
    const SAMPLES: [(&str, &str); 4] = [
        ("flat-logo.png", "Flat logo"),
        ("icon-64.png", "Icon, 64 px"),
        ("crest-filigree.png", "Crest, filigree"),
        ("signature-bw.png", "Signature, B&W"),
    ];
    SAMPLES
        .iter()
        .filter_map(|(file, label)| {
            let path = sample_path(&app, file).ok()?;
            let bytes = std::fs::read(&path).ok()?;
            Some(SampleInfo {
                file: (*file).to_string(),
                label: (*label).to_string(),
                preview: data_url(&bytes, "image/png"),
            })
        })
        .collect()
}

fn sample_path(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    // Only a bare file name: a sample is one of the four the app ships, never a path.
    if !is_bare_name(name) {
        return Err(format!("{name} is not one of the bundled samples"));
    }
    app.path()
        .resolve(
            format!("samples/{name}"),
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|e| format!("cannot find the bundled samples: {e}"))
}

/// Whether `name` is a plain file name that cannot climb out of its directory.
fn is_bare_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && !name.contains("..")
}

fn adopt(source: trace::Source, state: &State<'_, AppState>) -> Result<SourceInfo, String> {
    let info = SourceInfo {
        name: source.name(),
        path: source.path.clone(),
        width: source.width,
        height: source.height,
        container: source.container.name(),
        lossy: source.container.is_lossy(),
        preview: preview_of(&source)?,
    };
    *state.source.lock().map_err(lock)? = Some(Arc::new(source));
    Ok(info)
}

/// The source as a `data:` URL the viewer can show, capped on the longer side.
fn preview_of(source: &trace::Source) -> Result<String, String> {
    const CAP: u32 = 2048;
    if source.width.max(source.height) <= CAP && source.container == lost::Container::Png {
        return Ok(data_url(&source.bytes, "image/png"));
    }
    let raster = inkvec_trace::decode_image_capped(&source.bytes, CAP as usize)
        .map_err(|e| format!("cannot read the image: {e}"))?
        .0;
    let (w, h) = (raster.width as u32, raster.height as u32);
    let bytes: Vec<u8> = raster
        .data
        .iter()
        .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let img = image::RgbaImage::from_raw(w, h, bytes)
        .ok_or_else(|| "the decoded image did not fill its buffer".to_string())?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("cannot encode the preview: {e}"))?;
    Ok(data_url(&out.into_inner(), "image/png"))
}

fn data_url(bytes: &[u8], mime: &str) -> String {
    format!("data:{mime};base64,{}", base64(bytes))
}

/// Standard base64, written out rather than pulled in: this is the only place the app
/// needs it, and it is twenty lines.
fn base64(bytes: &[u8]) -> String {
    const SET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(SET[(n >> 18) as usize & 63] as char);
        out.push(SET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            SET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            SET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// --------------------------------------------------------------------------- trace ---

/// What the interface asks for when it wants a trace.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRequest {
    /// The eighteen controls as they stand.
    pub settings: options::Settings,
    /// Draft or final.
    pub tier: trace::Tier,
}

/// Start a trace, retiring any that was already running, and return its generation.
///
/// Returns immediately. The result arrives as a `trace:done` event carrying the same
/// generation, and stages arrive as `trace:stage` events before it. A result whose
/// generation is no longer current is dropped here rather than sent, so the interface
/// never has to reason about a stale trace.
#[tauri::command]
fn start_trace(
    request: TraceRequest,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<u64, String> {
    let source = state
        .source
        .lock()
        .map_err(lock)?
        .clone()
        .ok_or_else(|| "no image is open".to_string())?;

    let (draft_px, draft_seconds) = {
        let prefs = state.prefs.lock().map_err(lock)?;
        (prefs.draft_px, prefs.draft_seconds)
    };
    let settings = match request.tier {
        trace::Tier::Draft => request.settings.draft(draft_px, draft_seconds),
        trace::Tier::Final => request.settings.clone(),
    };

    // Remember what was asked for, not the draft's reduction of it.
    state.prefs.lock().map_err(lock)?.trace = request.settings.clone();

    let generation = state.generation.next();
    let tier = request.tier;
    std::thread::Builder::new()
        .name(format!("inkvec-trace-{generation}"))
        .spawn(move || {
            let stage_app = app.clone();
            let outcome = trace::run(&source, &settings, tier, move |name, ms| {
                let _ = stage_app.emit(
                    "trace:stage",
                    serde_json::json!({ "generation": generation, "name": name, "ms": ms }),
                );
            });
            let state = app.state::<AppState>();
            if state.generation.is_current(generation) {
                let _ = app.emit(
                    "trace:done",
                    serde_json::json!({ "generation": generation, "outcome": outcome }),
                );
            }
        })
        .map_err(|e| format!("cannot start the trace: {e}"))?;

    Ok(generation)
}

/// Retire whatever is in flight.
///
/// The pipeline has no cancellation point, so this does not interrupt it: it retires the
/// generation, the interface goes straight back to the last result, and the abandoned
/// thread's output is dropped when it arrives. Nothing waits for it, and the last full
/// trace is still on screen and still exportable.
#[tauri::command]
fn cancel_trace(state: State<'_, AppState>) {
    state.generation.cancel();
}

// ------------------------------------------------------------------------- palette ---

/// A traced ink and what to paint it as instead.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snap {
    /// The colour the tracer measured.
    pub from: String,
    /// The colour to paint it.
    pub to: String,
}

/// The rewritten drawing and its palette.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapResult {
    /// The SVG with the new fills.
    pub svg: String,
    /// The palette as it now stands.
    pub inks: Vec<quality::Ink>,
}

/// Rewrite the SVG's fills, and say how far each ink was moved from its measurement.
///
/// This is the one thing in the app that is genuinely instant: it rewrites fill strings
/// and nothing else, so there is no re-trace and no new measurement. The distance moved is
/// shown beside every snapped swatch, because the user is overriding something that was
/// measured and should be able to see by how much.
#[tauri::command]
fn snap_inks(svg: String, snaps: Vec<Snap>, width: u32, height: u32) -> Result<SnapResult, String> {
    let mut out = svg;
    for s in &snaps {
        let (from, to) = (s.from.to_ascii_lowercase(), s.to.to_ascii_lowercase());
        if quality::parse_hex(&from).is_none() || quality::parse_hex(&to).is_none() {
            return Err(format!("{} or {} is not a colour", s.from, s.to));
        }
        for attr in ["fill", "stroke"] {
            out = out
                .replace(&format!("{attr}=\"{from}\""), &format!("{attr}=\"{to}\""))
                .replace(
                    &format!("{attr}=\"{}\"", from.to_ascii_uppercase()),
                    &format!("{attr}=\"{to}\""),
                );
        }
    }
    let mut inks = quality::palette(&out, width.max(1), height.max(1))?;
    for ink in inks.iter_mut() {
        if let Some(s) = snaps.iter().find(|s| s.to.eq_ignore_ascii_case(&ink.hex)) {
            ink.traced = s.from.to_ascii_lowercase();
            ink.snapped_de00 = quality::hex_distance(&ink.traced, &ink.hex);
        }
    }
    Ok(SnapResult { svg: out, inks })
}

/// One traced ink and the pasted colour nearest to it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Match {
    /// The traced colour.
    pub from: String,
    /// The nearest pasted colour.
    pub to: String,
    /// How far apart they are.
    pub de00: f64,
}

/// Match a pasted brand palette to the traced inks, without changing anything yet.
///
/// Each traced ink takes the nearest pasted colour, and the distance is reported so the
/// modal can show what each match would cost before the user commits to it.
#[tauri::command]
fn match_palette(traced: Vec<String>, pasted: String) -> Vec<Match> {
    let wanted: Vec<String> = pasted
        .split(|c: char| c == '\n' || c == ',' || c == ';')
        .filter_map(parse_colour)
        .collect();
    traced
        .iter()
        .filter_map(|t| {
            let from = t.to_ascii_lowercase();
            let lab_from = quality::lab(quality::parse_hex(&from)?);
            let best = wanted
                .iter()
                .map(|w| {
                    let d = quality::parse_hex(w)
                        .map(|c| quality::ciede2000(lab_from, quality::lab(c)))
                        .unwrap_or(f64::MAX);
                    (d, w.clone())
                })
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))?;
            Some(Match {
                from,
                to: best.1,
                de00: best.0,
            })
        })
        .collect()
}

/// Read one colour out of a line of pasted text.
///
/// Hex with or without a `#`, `rgb(...)`, or a CSS custom property whose value is either.
/// Anything else is skipped rather than guessed at.
fn parse_colour(line: &str) -> Option<String> {
    let line = line.trim().trim_end_matches(';');
    let value = line.rsplit(':').next().unwrap_or(line).trim();
    if let Some(rest) = value.strip_prefix("rgb") {
        let inside = rest.trim_start_matches('(').trim_end_matches(')');
        let parts: Vec<u8> = inside
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.trim().parse::<f32>().ok())
            .map(|v| v.clamp(0.0, 255.0) as u8)
            .collect();
        if parts.len() >= 3 {
            return Some(format!("#{:02x}{:02x}{:02x}", parts[0], parts[1], parts[2]));
        }
        return None;
    }
    let hex = value.trim_start_matches('#');
    if !hex.is_empty() && hex.len() <= 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return quality::parse_hex(&format!("#{hex}")).map(quality::to_hex);
    }
    None
}

// -------------------------------------------------------------------------- export ---

/// The subset of the report an export needs. Sent back rather than re-derived, so the
/// README quotes the numbers the user was actually looking at.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    /// Mean colour difference, if one was measured.
    pub mean_de00: Option<f64>,
    /// Coordinates in the path data.
    pub coordinates: usize,
    /// Drawn elements.
    pub paths: usize,
    /// The longer side the trace ran at.
    pub traced_px: u32,
}

/// One ink, as the interface has it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportInk {
    /// What it is painted as now.
    pub hex: String,
    /// What the tracer measured.
    pub traced: String,
    /// Share of the canvas.
    pub share: f64,
}

/// One honesty-panel row, as the interface has it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportLoss {
    /// The row's sentence.
    pub text: String,
}

/// What the interface sends when it wants to export.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    /// The drawing to write, which may carry snapped fills.
    pub svg: String,
    /// The measurements, for the README.
    pub report: ExportReport,
    /// The palette, for `palette.json` and the README.
    pub palette: Vec<ExportInk>,
    /// The honesty panel's rows, for the README's one honest line.
    #[serde(default)]
    pub losses: Vec<ExportLoss>,
    /// Which formats to write.
    pub formats: export::Formats,
}

/// One file an export would write.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFile {
    /// Path relative to the destination.
    pub name: String,
    /// Its size, measured rather than estimated: the file already exists in memory.
    pub bytes: usize,
    /// Which checklist row it belongs to.
    pub group: &'static str,
}

/// Build the export in memory and report what it would write, with real byte counts.
#[tauri::command]
fn plan_export(
    request: ExportRequest,
    state: State<'_, AppState>,
) -> Result<Vec<PlannedFile>, String> {
    Ok(build_export(&request, &state)?
        .iter()
        .map(|a| PlannedFile {
            name: a.name.clone(),
            bytes: a.bytes,
            group: a.group,
        })
        .collect())
}

/// Write the export into `destination`.
#[tauri::command]
fn write_export(
    request: ExportRequest,
    destination: PathBuf,
    state: State<'_, AppState>,
) -> Result<Vec<PathBuf>, String> {
    let built = build_export(&request, &state)?;
    let written = export::write_all(&built, &destination)?;
    {
        let mut prefs = state.prefs.lock().map_err(lock)?;
        prefs.output_folder = Some(destination);
        let _ = settings::save(&prefs);
    }
    Ok(written)
}

fn build_export(
    request: &ExportRequest,
    state: &State<'_, AppState>,
) -> Result<Vec<export::Artifact>, String> {
    let source = state
        .source
        .lock()
        .map_err(lock)?
        .clone()
        .ok_or_else(|| "no image is open".to_string())?;

    let report = quality::Report {
        mean_de00: request.report.mean_de00,
        coordinates: request.report.coordinates,
        paths: request.report.paths,
        traced_px: request.report.traced_px,
        bytes: request.svg.len(),
        ..Default::default()
    };
    let inks: Vec<quality::Ink> = request
        .palette
        .iter()
        .map(|i| quality::Ink {
            traced: i.traced.clone(),
            hex: i.hex.clone(),
            share: i.share,
            snapped_de00: None,
        })
        .collect();
    let losses: Vec<lost::Loss> = request
        .losses
        .iter()
        .map(|l| lost::Loss {
            kind: "note",
            text: l.text.clone(),
            why: String::new(),
            link: None,
        })
        .collect();

    let (stem, name) = (source.stem(), source.name());
    export::build(
        &export::Subject {
            stem: &stem,
            source_name: &name,
            svg: &request.svg,
            report: &report,
            palette: &inks,
            losses: &losses,
            source_px: (source.width, source.height),
        },
        &request.formats,
    )
}

// -------------------------------------------------------------------------- minify ---

/// Rewrite an SVG's paths as the fewest segments that draw the same picture.
#[tauri::command]
fn minify_svg(
    svg: String,
    settings: minify::MinifySettings,
) -> Result<minify::MinifyResult, String> {
    minify::run(&svg, settings)
}

/// Read a text file the user chose. Used by the Minify tab to open an existing SVG.
#[tauri::command]
fn read_text_file(path: PathBuf) -> Result<String, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("{} is not text", path.display()))
}

/// Write bytes the interface produced — the share card's PNG, a stats CSV, a saved SVG.
#[tauri::command]
fn save_bytes(path: PathBuf, bytes: Vec<u8>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

// --------------------------------------------------------------------------- batch ---

#[tauri::command]
fn batch_scan(folder: PathBuf) -> Result<Vec<PathBuf>, String> {
    batch::scan(&folder)
}

/// Start a batch run. Rows and totals arrive as events; the run holds one worker thread.
#[tauri::command]
fn batch_start(
    plan: batch::Plan,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let controls = {
        let mut slot = state.batch.lock().map_err(lock)?;
        if slot.as_ref().is_some_and(|c| !c.is_cancelled()) {
            return Err("a batch run is already going".into());
        }
        let fresh = Arc::new(batch::Controls::default());
        *slot = Some(Arc::clone(&fresh));
        fresh
    };
    let base = state.prefs.lock().map_err(lock)?.trace.clone();

    std::thread::Builder::new()
        .name("inkvec-batch".into())
        .spawn(move || {
            let row_app = app.clone();
            let totals_app = app.clone();
            let rows = batch::run(
                &plan,
                &base,
                &controls,
                move |row| {
                    let _ = row_app.emit("batch:row", row);
                },
                move |totals| {
                    let _ = totals_app.emit("batch:totals", totals);
                },
            );
            let state = app.state::<AppState>();
            if let Ok(mut slot) = state.batch_rows.lock() {
                slot.clone_from(&rows);
            }
            if let Ok(mut slot) = state.batch.lock() {
                *slot = None;
            }
            let _ = app.emit("batch:finished", &rows);
        })
        .map_err(|e| format!("cannot start the batch run: {e}"))?;
    Ok(())
}

#[tauri::command]
fn batch_pause(paused: bool, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(c) = state.batch.lock().map_err(lock)?.as_ref() {
        c.pause(paused);
    }
    Ok(())
}

#[tauri::command]
fn batch_cancel(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(c) = state.batch.lock().map_err(lock)?.as_ref() {
        c.cancel();
    }
    Ok(())
}

#[tauri::command]
fn batch_stats_csv(state: State<'_, AppState>) -> Result<String, String> {
    Ok(batch::stats_csv(&state.batch_rows.lock().map_err(lock)?))
}

// ------------------------------------------------------------------------ denoiser ---

#[tauri::command]
fn denoiser_status() -> denoiser::Status {
    denoiser::status()
}

/// Download the denoiser, reporting progress as `denoiser:progress` events.
#[tauri::command]
fn denoiser_download(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let stop = Arc::clone(&state.denoiser_stop);
    stop.store(false, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("inkvec-denoiser".into())
        .spawn(move || {
            let progress_app = app.clone();
            let result = denoiser::download(
                move |got, total| {
                    let _ = progress_app.emit(
                        "denoiser:progress",
                        serde_json::json!({ "got": got, "total": total }),
                    );
                },
                || stop.load(Ordering::SeqCst),
            );
            denoiser::announce_to_engine();
            let payload = match result {
                Ok(path) => serde_json::json!({
                    "ok": true, "path": path, "status": denoiser::status()
                }),
                Err(message) => serde_json::json!({
                    "ok": false, "message": message, "status": denoiser::status()
                }),
            };
            let _ = app.emit("denoiser:done", payload);
        })
        .map_err(|e| format!("cannot start the download: {e}"))?;
    Ok(())
}

#[tauri::command]
fn denoiser_cancel(state: State<'_, AppState>) {
    state.denoiser_stop.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn denoiser_remove() -> Result<denoiser::Status, String> {
    denoiser::remove()?;
    Ok(denoiser::status())
}

// ----------------------------------------------------------------------- prefs etc ---

#[tauri::command]
fn load_prefs(state: State<'_, AppState>) -> Result<settings::Prefs, String> {
    let mut prefs = state.prefs.lock().map_err(lock)?;
    prefs.prune_recent();
    Ok(prefs.clone())
}

#[tauri::command]
fn save_prefs(
    prefs: settings::Prefs,
    state: State<'_, AppState>,
) -> Result<settings::Prefs, String> {
    let cleaned = prefs.sanitised();
    settings::save(&cleaned)?;
    *state.prefs.lock().map_err(lock)? = cleaned.clone();
    Ok(cleaned)
}

#[tauri::command]
fn reset_prefs(state: State<'_, AppState>) -> Result<settings::Prefs, String> {
    let fresh = settings::reset()?;
    *state.prefs.lock().map_err(lock)? = fresh.clone();
    Ok(fresh)
}

/// The licence and third-party notices, as shipped beside the app.
#[tauri::command]
fn third_party_notices(app: AppHandle) -> Result<String, String> {
    let path = app
        .path()
        .resolve("THIRD_PARTY.md", tauri::path::BaseDirectory::Resource)
        .map_err(|e| format!("cannot find the notices: {e}"))?;
    std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// What the update check found.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// The newest version published, if the check reached the server.
    pub latest: Option<String>,
    /// Whether it is newer than this build.
    pub newer: bool,
    /// Where its notes are.
    pub url: String,
    /// Why the check did not complete, when it did not. A missing network is not an
    /// error worth a dialog: tracing never needed one.
    pub offline: Option<String>,
}

/// Ask GitHub what the newest release is.
///
/// This is the only request the app makes that the user did not initiate, and what it
/// sends is exactly what the Privacy paragraph says it sends: the app version and the
/// operating system, in the user agent, and nothing else. No identifier, no image, no
/// cookie. Turning the check off in Settings means the app never contacts the network.
#[tauri::command]
fn check_update(state: State<'_, AppState>) -> Result<UpdateInfo, String> {
    const RELEASES: &str = "https://api.github.com/repos/logolabs/inkvec/releases/latest";
    let notes = "https://github.com/logolabs/inkvec/releases".to_string();

    if !state.prefs.lock().map_err(lock)?.check_updates {
        return Ok(UpdateInfo {
            latest: None,
            newer: false,
            url: notes,
            offline: Some("the update check is turned off".into()),
        });
    }

    let agent = format!(
        "inkvec-studio/{} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    );
    let mut response = match ureq::get(RELEASES)
        .header("User-Agent", &agent)
        .header("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            return Ok(UpdateInfo {
                latest: None,
                newer: false,
                url: notes,
                offline: Some(e.to_string()),
            })
        }
    };
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("cannot read the reply: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("cannot read the reply: {e}"))?;
    let latest = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .map(|t| t.trim_start_matches('v').to_string());
    let newer = latest
        .as_deref()
        .is_some_and(|l| is_newer(l, env!("CARGO_PKG_VERSION")));

    Ok(UpdateInfo {
        latest,
        newer,
        url: json
            .get("html_url")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or(notes),
        offline: None,
    })
}

/// Whether `candidate` is a later version than `current`, compared numerically.
fn is_newer(candidate: &str, current: &str) -> bool {
    let parts = |s: &str| -> Vec<u64> {
        s.split(['.', '-', '+'])
            .map_while(|p| p.parse::<u64>().ok())
            .collect()
    };
    let (a, b) = (parts(candidate), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// A poisoned lock, in words. It means a previous command panicked while holding it,
/// which is a bug rather than a condition to recover from.
fn lock<T>(_: std::sync::PoisonError<T>) -> String {
    "the app's state was left locked by a failed command; please restart".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_data_url_names_its_type() {
        assert!(data_url(b"foo", "image/png").starts_with("data:image/png;base64,Zm9v"));
    }

    #[test]
    fn versions_compare_numerically_not_alphabetically() {
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.4", "0.1.4"));
        assert!(!is_newer("0.1.3", "0.1.4"));
        assert!(is_newer("0.2", "0.1.9"));
    }

    #[test]
    fn a_sample_name_cannot_be_a_path() {
        for bad in ["../secrets", "a/b.png", "..\\windows", ""] {
            assert!(!is_bare_name(bad), "{bad} was accepted");
        }
        assert!(is_bare_name("flat-logo.png"));
    }

    #[test]
    fn a_pasted_palette_is_read_in_the_forms_people_paste() {
        assert_eq!(parse_colour("#12443E").as_deref(), Some("#12443e"));
        assert_eq!(parse_colour("12443E").as_deref(), Some("#12443e"));
        assert_eq!(parse_colour("  #abc  ").as_deref(), Some("#aabbcc"));
        assert_eq!(
            parse_colour("rgb(207, 198, 180)").as_deref(),
            Some("#cfc6b4")
        );
        assert_eq!(
            parse_colour("--brand-ink: #12443E;").as_deref(),
            Some("#12443e")
        );
        assert_eq!(parse_colour("not a colour"), None);
        assert_eq!(parse_colour(""), None);
    }

    #[test]
    fn matching_a_palette_picks_the_nearest_and_says_how_far() {
        let traced = vec!["#14453f".to_string(), "#e7b04a".to_string()];
        let matches = match_palette(traced, "#12443E\n#E9B24C\n#ffffff".into());
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].to, "#12443e");
        assert_eq!(matches[1].to, "#e9b24c");
        for m in &matches {
            assert!(m.de00 > 0.0 && m.de00 < 5.0, "{m:?}");
        }
    }

    #[test]
    fn snapping_rewrites_fills_and_reports_the_distance_moved() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\" height=\"32\" \
                   viewBox=\"0 0 32 32\"><path d=\"M0 0L32 0L32 32L0 32Z\" fill=\"#14453f\"/></svg>";
        let result = snap_inks(
            svg.into(),
            vec![Snap {
                from: "#14453f".into(),
                to: "#12443e".into(),
            }],
            32,
            32,
        )
        .unwrap();
        assert!(result.svg.contains("fill=\"#12443e\""));
        assert!(!result.svg.contains("#14453f"));
        let ink = result.inks.iter().find(|i| i.hex == "#12443e").unwrap();
        assert_eq!(ink.traced, "#14453f");
        let moved = ink
            .snapped_de00
            .expect("a snapped ink reports what it cost");
        assert!(moved > 0.0 && moved < 3.0, "{moved}");
    }

    #[test]
    fn snapping_to_something_that_is_not_a_colour_is_refused() {
        let e = snap_inks(
            "<svg/>".into(),
            vec![Snap {
                from: "#14453f".into(),
                to: "chartreuse".into(),
            }],
            8,
            8,
        )
        .unwrap_err();
        assert!(e.contains("chartreuse"), "{e}");
    }

    #[test]
    fn the_capabilities_describe_the_whole_advanced_drawer() {
        let c = capabilities();
        assert_eq!(c.controls.len(), 18);
        assert_eq!(c.presets.len(), 7);
        assert_eq!(c.stages.len(), 9);
        assert_eq!(c.version, env!("CARGO_PKG_VERSION"));
        assert!(c.build_target.starts_with(std::env::consts::ARCH));
        assert!(c.presets.iter().any(|p| p.wants_denoiser));
    }
}
