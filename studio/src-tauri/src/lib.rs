//! Inkvec Studio.
//!
//! A desktop app that turns a raster logo into an SVG, exactly, and minimises SVGs you
//! already have. This module is the seam between the interface and the engine: every
//! command here is a small, explicit piece of work with a name, and nothing in the
//! interface can reach the tracer except through one of them.
//!
//! What a command does, as opposed to how the desktop runs it, lives in
//! `inkvec_studio_core` and is shared with the browser build, Inkvec Studio Lite
//! (`studio/wasm`). This crate adds what only a desktop has: a thread per trace, events,
//! files, preferences on disk, the native denoiser, batch runs, the update check and the
//! desktop integrations.
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
pub mod integration;
pub mod settings;

// The shared core's modules, under the names this crate has always used for them.
use inkvec_studio_core::api::{Capabilities, SampleInfo, SourceInfo, TraceRequest};
pub use inkvec_studio_core::{api, export, lost, minify, options, quality, trace, wizard};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Listener, Manager, State};

/// Everything the app holds between commands.
#[derive(Default)]
pub struct AppState {
    /// The image currently open, if any.
    source: Mutex<Option<Arc<trace::Source>>>,
    /// Which trace the interface is waiting for.
    generation: trace::Generation,
    /// Which wizard preview the interface is waiting for. A counter of its own, so a
    /// preview never retires the trace the viewer is waiting for. See [`wizard`].
    preview_generation: trace::Generation,
    /// The image the app was launched to open, until the interface asks for it.
    launch: Mutex<Option<PathBuf>>,
    /// Preferences, as loaded and as edited.
    prefs: Mutex<settings::Prefs>,
    /// The controls of a batch run, while one is going.
    batch: Mutex<Option<Arc<batch::Controls>>>,
    /// The last batch run's rows, so a finished run can still be read and exported.
    batch_rows: Mutex<Vec<batch::Row>>,
    /// Set to stop a denoiser download.
    denoiser_stop: Arc<AtomicBool>,
    /// One trace at a time, the interactive one first. See [`trace::Scheduler`].
    pipeline: Arc<trace::Scheduler>,
    /// The last drawing traced, so an output option does not cost a trace. See
    /// [`trace::Cache`].
    traced: Arc<trace::Cache>,
    /// The confidence bands of the last trace sent to the interface, by generation. They
    /// are often a hundred times the size of the drawing, so they are not part of the
    /// `trace:done` event; the interface asks for them with [`trace_bands`].
    bands: Mutex<Option<(u64, Arc<String>)>>,
    /// The newest Fabricate request, so a burst of edits prepares only the last one.
    fab_latest: AtomicU64,
    /// One Fabricate preparation at a time.
    fab_running: Mutex<()>,
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
        // First, so a second launch hands its file to this window before anything else
        // starts: the context menu's "Vectorize with Inkvec" on a running app opens the
        // file here rather than starting a second copy.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let cwd = PathBuf::from(cwd);
            if let Some(path) = wizard::path_from_args(&argv, Some(&cwd), std::path::Path::is_file)
            {
                let _ = app.emit_to("main", "open:path", path);
            }
            if let Some(main) = app.get_webview_window("main") {
                let _ = main.unminimize();
                let _ = main.show();
                let _ = main.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::default())
        .setup(|app| {
            let prefs = settings::load();
            configure_threads(&prefs);
            let state = app.state::<AppState>();
            *state.prefs.lock().expect("preferences lock") = prefs;
            // The file the context menu (or anything else) launched the app with. Held
            // until the interface is ready to open it; see [`launch_path`].
            *state.launch.lock().expect("launch lock") = wizard::path_from_args(
                std::env::args(),
                std::env::current_dir().ok().as_deref(),
                std::path::Path::is_file,
            );

            // If the interface never reports that it is ready — a script error before it
            // gets that far — the splash must not stay on screen forever with the app
            // hidden behind it. Past this, the window swap happens anyway, and whatever
            // the interface managed to draw (its own error, at best) is what is shown.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(SPLASH_FAILSAFE);
                finish_splash(&handle, true);
            });

            // The splash says when its animation has played to the end. The app does not
            // take the screen until that has happened, however early it is ready.
            let handle = app.handle().clone();
            app.listen("splash-animation-done", move |_| {
                ANIMATION_DONE.store(true, Ordering::SeqCst);
                finish_splash(&handle, false);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            capabilities,
            startup_progress,
            app_ready,
            open_path,
            open_bytes,
            open_sample,
            list_samples,
            start_trace,
            trace_bands,
            cancel_trace,
            start_preview,
            cancel_previews,
            source_facts,
            launch_path,
            snap_inks,
            match_palette,
            plan_export,
            write_export,
            minify_svg,
            fab_analyze,
            fab_prepare,
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
            cli_status,
            install_cli,
            remove_cli,
            context_menu_status,
            install_context_menu,
            remove_context_menu,
        ])
        .run(tauri::generate_context!())
        .expect("Inkvec Studio could not start its window");
}

// ------------------------------------------------------------------------- splash ---

/// Set once the interface has drawn itself and loaded its data.
static APP_READY: AtomicBool = AtomicBool::new(false);
/// Set once the splash has played its animation to the end. The splash is the one that
/// knows: its animation starts when its own page and fonts are ready, which is not when
/// the process started, so no timer counted from launch could promise the whole thing.
static ANIMATION_DONE: AtomicBool = AtomicBool::new(false);
/// Set once the splash has been handed over, so the swap happens exactly once.
static SPLASH_DONE: AtomicBool = AtomicBool::new(false);

/// How long the app waits for the splash to say its animation is over, once the app itself
/// is ready. It never comes to this unless the splash's page failed to run.
const ANIMATION_GRACE: Duration = Duration::from_secs(6);
/// The most time the splash may stay up when the interface never says it is ready.
const SPLASH_FAILSAFE: Duration = Duration::from_secs(20);
/// How long the splash's fade-out gets before it is closed.
const SPLASH_FADE: Duration = Duration::from_millis(280);

/// What the splash window's status line and bar should say.
#[derive(Clone, Serialize)]
struct SplashProgress {
    text: String,
    progress: f32,
}

/// The interface reporting one real step of its own start-up.
#[tauri::command]
fn startup_progress(app: AppHandle, text: String, progress: f32) {
    // No splash (it has already gone, or this is a build without one) is not an error.
    let _ = app.emit_to("splash", "splash-status", SplashProgress { text, progress });
}

/// The interface is drawn and its data is loaded. The screen is handed over as soon as the
/// splash has also finished its animation, which may already be true or may be a moment away.
#[tauri::command]
fn app_ready(app: AppHandle) {
    APP_READY.store(true, Ordering::SeqCst);
    finish_splash(&app, false);
    // If the splash never reports, do not wait on it for ever.
    std::thread::spawn(move || {
        std::thread::sleep(ANIMATION_GRACE);
        finish_splash(&app, true);
    });
}

/// Fade the splash, show the app and close the splash. Runs once, when both the app is
/// ready and the animation is over — or, with `force`, regardless of either.
fn finish_splash(app: &AppHandle, force: bool) {
    let both = APP_READY.load(Ordering::SeqCst) && ANIMATION_DONE.load(Ordering::SeqCst);
    if !(both || force) || SPLASH_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        // The app is shown first, under the splash: the splash is always on top, so it
        // fades out over a window that is already there instead of over the desktop,
        // and the app is on screen a fade sooner.
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.show();
            let _ = main.set_focus();
        }
        let _ = app.emit_to("splash", "splash-done", ());
        std::thread::sleep(SPLASH_FADE);
        if let Some(splash) = app.get_webview_window("splash") {
            let _ = splash.destroy();
        }
    });
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
//
// Every command that does real work — reads a file, decodes, renders, hashes, reaches the
// network — is `async`, which in Tauri means it runs on a worker thread rather than on
// the thread that drives the window. A plain command blocks the interface for as long as
// it takes: seconds, for a Fabricate plan of a detailed drawing or an update check on a
// network that does not answer.

#[tauri::command(async)]
fn capabilities() -> Capabilities {
    describe(denoiser::status())
}

/// What this build can do, with the denoiser's status as given.
fn describe(denoiser: denoiser::Status) -> Capabilities {
    api::describe(denoiser, std::env::consts::OS)
}

#[tauri::command(async)]
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

#[tauri::command(async)]
fn open_bytes(
    bytes: Vec<u8>,
    name: Option<String>,
    state: State<'_, AppState>,
) -> Result<SourceInfo, String> {
    adopt(trace::Source::open(bytes, name.map(PathBuf::from))?, &state)
}

#[tauri::command(async)]
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
#[tauri::command(async)]
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
                preview: api::data_url(&bytes, "image/png"),
            })
        })
        .collect()
}

fn sample_path(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    // Only a bare file name: a sample is one of the four the app ships, never a path.
    if !api::is_bare_name(name) {
        return Err(format!("{name} is not one of the bundled samples"));
    }
    app.path()
        .resolve(
            format!("samples/{name}"),
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|e| format!("cannot find the bundled samples: {e}"))
}

fn adopt(source: trace::Source, state: &State<'_, AppState>) -> Result<SourceInfo, String> {
    let info = api::source_info(&source)?;
    *state.source.lock().map_err(lock)? = Some(Arc::new(source));
    // The drawing in hand belongs to the image that has just been replaced. Keying on the
    // image would miss it anyway; this is so its bytes are not held for an image nobody
    // has open any more.
    state.traced.clear();
    Ok(info)
}

// --------------------------------------------------------------------------- trace ---

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
    // A draft of an image that already fits the draft size runs as the final: see
    // [`trace::plan`].
    let (settings, tier) = trace::plan(
        &request.settings,
        request.tier,
        draft_px,
        draft_seconds,
        (source.width, source.height),
    );

    // Remember what was asked for, not the draft's reduction of it.
    // Colour groups belong to the image that is open, so they are not remembered.
    state.prefs.lock().map_err(lock)?.trace = request.settings.clone().for_keeping();

    let generation = state.generation.next();
    let pipeline = Arc::clone(&state.pipeline);
    let traced = Arc::clone(&state.traced);
    std::thread::Builder::new()
        .name(format!("inkvec-trace-{generation}"))
        .spawn(move || {
            // One trace at a time. Waiting here rather than starting straight away is what
            // stops a handful of quick control changes becoming a handful of full traces
            // sharing the cores, each slower for the others.
            let slot = pipeline.interactive();
            // Whoever we were waiting for has finished, and the user may well have moved on
            // while we waited. A trace the interface is no longer waiting for is not worth
            // the cores: its result would be dropped on arrival anyway.
            if !app.state::<AppState>().generation.is_current(generation) {
                return;
            }
            let stage_app = app.clone();
            let drawn = trace::draw(
                &source,
                &settings,
                tier,
                Some(&traced),
                trace::MeasureLevel::Full,
                move |name, ms| {
                    let _ = stage_app.emit(
                        "trace:stage",
                        StageEvent {
                            generation,
                            name,
                            ms,
                        },
                    );
                },
            );
            // The drawing is finished and kept; measuring it needs no slot, so the next
            // trace can start while this one is measured.
            drop(slot);
            let state = app.state::<AppState>();
            let mut outcome = match drawn {
                // Superseded while it was drawn: nobody is waiting for its measurements.
                // The drawing stays in the cache, where the next request may still find it.
                Ok(_) if !state.generation.is_current(generation) => return,
                Ok(drawn) => {
                    trace::measure(&source, drawn, trace::MeasureLevel::Full, Some(&traced))
                }
                Err(outcome) => outcome,
            };
            // The bands stay here until the interface asks for them.
            let bands = match &mut outcome {
                trace::Outcome::Traced(t) => t.bands.take(),
                _ => None,
            };
            if !state.generation.is_current(generation) {
                return;
            }
            if let Ok(mut held) = state.bands.lock() {
                *held = bands.map(|b| (generation, Arc::new(b)));
            }
            let _ = app.emit(
                "trace:done",
                TraceDone {
                    generation,
                    outcome: &outcome,
                },
            );
        })
        .map_err(|e| format!("cannot start the trace: {e}"))?;

    Ok(generation)
}

/// One stage the engine passed, for the progress rail.
#[derive(Clone, Serialize)]
struct StageEvent {
    generation: u64,
    name: &'static str,
    ms: f64,
}

/// A finished trace, for the interface. The confidence bands are not in it: see
/// [`trace_bands`].
#[derive(Clone, Serialize)]
struct TraceDone<'a> {
    generation: u64,
    outcome: &'a trace::Outcome,
}

/// The confidence bands of the trace `generation`, if it is the one last sent.
///
/// They are an SVG of their own and often a hundred times the size of the drawing. An
/// event's payload reaches the webview as a script to evaluate, the slow way for that
/// much text, so the bands come back as a command's reply instead.
#[tauri::command(async)]
fn trace_bands(generation: u64, state: State<'_, AppState>) -> Result<Option<String>, String> {
    let held = state.bands.lock().map_err(lock)?;
    Ok(held
        .as_ref()
        .filter(|(g, _)| *g == generation)
        .map(|(_, bands)| bands.as_str().to_owned()))
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

// -------------------------------------------------------------------------- wizard ---

/// A finished wizard preview, for the interface.
#[derive(Clone, Serialize)]
struct PreviewDone<'a> {
    generation: u64,
    outcome: &'a trace::Outcome,
}

/// Start a wizard preview: a draft of `request.settings` that does not become the drawing.
///
/// Returns its generation at once; the result arrives as a `preview:done` event. Unlike
/// [`start_trace`] it leaves everything the viewer depends on alone: the trace generation
/// (so the automatic trace keeps running), the drawing cache, the confidence bands and the
/// remembered settings. It waits for the trace slot the way a batch row does — behind any
/// interactive trace — and gives up while waiting if a newer preview or
/// [`cancel_previews`] has retired it. The tier in the request is ignored: a preview is
/// always a draft.
#[tauri::command]
fn start_preview(
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
    let generation = state.preview_generation.next();
    let pipeline = Arc::clone(&state.pipeline);
    std::thread::Builder::new()
        .name(format!("inkvec-preview-{generation}"))
        .spawn(move || {
            let wanted = || {
                app.state::<AppState>()
                    .preview_generation
                    .is_current(generation)
            };
            let Some(slot) = pipeline.batch_row(|| !wanted()) else {
                return;
            };
            let mut outcome =
                wizard::run_preview(&source, &request.settings, draft_px, draft_seconds);
            drop(slot);
            if !wanted() {
                return;
            }
            if let trace::Outcome::Traced(t) = &mut outcome {
                t.bands = None;
            }
            let _ = app.emit(
                "preview:done",
                PreviewDone {
                    generation,
                    outcome: &outcome,
                },
            );
        })
        .map_err(|e| format!("cannot start the preview: {e}"))?;
    Ok(generation)
}

/// Retire every wizard preview in flight or waiting. The trace the viewer waits for is not
/// touched.
#[tauri::command]
fn cancel_previews(state: State<'_, AppState>) {
    state.preview_generation.cancel();
}

/// What the wizard wants to know about the open image: its noise, and whether it has any
/// transparency. Read from the raster a trace at `max_dim` decodes, which after the first
/// trace is already in memory.
#[tauri::command(async)]
fn source_facts(max_dim: usize, state: State<'_, AppState>) -> Result<wizard::Facts, String> {
    let source = state
        .source
        .lock()
        .map_err(lock)?
        .clone()
        .ok_or_else(|| "no image is open".to_string())?;
    let raster = source.raster(max_dim)?;
    Ok(wizard::facts_of(&raster))
}

/// The image the app was launched to open, once: asking again returns nothing, so a
/// reload of the interface does not open it a second time.
#[tauri::command]
fn launch_path(state: State<'_, AppState>) -> Result<Option<PathBuf>, String> {
    Ok(state.launch.lock().map_err(lock)?.take())
}

// ------------------------------------------------------------------------- palette ---

/// Rewrite the SVG's fills. See [`api::snap_inks`].
#[tauri::command(async)]
fn snap_inks(
    svg: String,
    snaps: Vec<api::Snap>,
    width: u32,
    height: u32,
) -> Result<api::SnapResult, String> {
    api::snap_inks(svg, snaps, width, height)
}

/// Match a pasted brand palette to the traced inks. See [`api::match_palette`].
#[tauri::command]
fn match_palette(traced: Vec<String>, pasted: String) -> Vec<api::Match> {
    api::match_palette(traced, pasted)
}

// -------------------------------------------------------------------------- export ---

/// Build the export in memory and report what it would write, with real byte counts.
#[tauri::command(async)]
fn plan_export(
    request: api::ExportRequest,
    state: State<'_, AppState>,
) -> Result<Vec<api::PlannedFile>, String> {
    Ok(api::planned(&build_export(&request, &state)?))
}

/// Write the export into `destination`.
#[tauri::command(async)]
fn write_export(
    request: api::ExportRequest,
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

/// Every file of the export, for the image that is open.
fn build_export(
    request: &api::ExportRequest,
    state: &State<'_, AppState>,
) -> Result<Vec<export::Artifact>, String> {
    let source = state
        .source
        .lock()
        .map_err(lock)?
        .clone()
        .ok_or_else(|| "no image is open".to_string())?;
    api::build_export(request, &source)
}

// -------------------------------------------------------------------------- minify ---

/// Rewrite an SVG's paths as the fewest segments that draw the same picture.
#[tauri::command(async)]
fn minify_svg(
    svg: String,
    settings: minify::MinifySettings,
) -> Result<minify::MinifyResult, String> {
    minify::run(&svg, settings)
}

/// What an SVG holds, for the Fabricate tab: its colours, which is the page, and what
/// cannot be cut.
#[tauri::command(async)]
fn fab_analyze(svg: String) -> Result<inkvec_fab::Analysis, String> {
    inkvec_fab::analyze(&svg).map_err(|e| e.to_string())
}

/// The sheets to cut for one set of Fabricate choices, with the preflight.
///
/// Latest wins. Dragging a slider sends a request per step and a detailed drawing takes
/// a while to prepare, so requests wait their turn here and every one that a newer
/// request has overtaken by then stands down without preparing anything. The tab already
/// ignores a reply to anything but its newest request, so the error it gets back is
/// never shown.
#[tauri::command(async)]
fn fab_prepare(
    svg: String,
    options: inkvec_fab::Options,
    state: State<'_, AppState>,
) -> Result<inkvec_fab::Plan, String> {
    let ticket = state.fab_latest.fetch_add(1, Ordering::SeqCst) + 1;
    let _turn = state
        .fab_running
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.fab_latest.load(Ordering::SeqCst) != ticket {
        return Err("superseded by a newer request".into());
    }
    inkvec_fab::prepare(&svg, &options).map_err(|e| e.to_string())
}

/// Read a text file the user chose. Used by the Minify tab to open an existing SVG.
#[tauri::command(async)]
fn read_text_file(path: PathBuf) -> Result<String, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("{} is not text", path.display()))
}

/// Write bytes the interface produced — the share card's PNG, a stats CSV, a saved SVG.
#[tauri::command(async)]
fn save_bytes(path: PathBuf, bytes: Vec<u8>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

// --------------------------------------------------------------------------- batch ---

#[tauri::command(async)]
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
    // A batch is many images; colour groups name the colours of one.
    let base = state
        .prefs
        .lock()
        .map_err(lock)?
        .trace
        .clone()
        .for_keeping();
    let pipeline = Arc::clone(&state.pipeline);

    std::thread::Builder::new()
        .name("inkvec-batch".into())
        .spawn(move || {
            let row_app = app.clone();
            let totals_app = app.clone();
            let rows = batch::run(
                &plan,
                &base,
                &controls,
                &pipeline,
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

#[tauri::command(async)]
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
///
/// Both documents, in one string: the app's own dependencies and the engine's. They are
/// generated separately because `studio/src-tauri` is not a member of the root cargo
/// workspace, but the notices a user reads have to be the notices for the binary they are
/// actually running, so the screen shows the pair.
#[tauri::command(async)]
fn third_party_notices(app: AppHandle) -> Result<String, String> {
    let read = |name: &str| -> Option<String> {
        let path = app
            .path()
            .resolve(name, tauri::path::BaseDirectory::Resource)
            .ok()?;
        std::fs::read_to_string(path).ok()
    };
    let parts: Vec<String> = ["STUDIO_THIRD_PARTY.md", "THIRD_PARTY.md"]
        .iter()
        .filter_map(|n| read(n))
        .collect();
    if parts.is_empty() {
        return Err("the bundled notices could not be read".into());
    }
    Ok(parts.join("\n\n\n"))
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
///
/// A network that swallows the request rather than refusing it gets five seconds, not
/// the operating system's minutes: the answer only ever reaches the status strip.
#[tauri::command(async)]
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
    let mut response = match update_agent()
        .get(RELEASES)
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

// ------------------------------------------------------- desktop integration ---

#[tauri::command]
fn cli_status() -> integration::Status {
    integration::cli_status()
}

#[tauri::command]
fn install_cli() -> Result<integration::Status, String> {
    integration::install_cli()
}

#[tauri::command]
fn remove_cli() -> Result<integration::Status, String> {
    integration::remove_cli()
}

#[tauri::command]
fn context_menu_status() -> integration::Status {
    integration::context_menu_status()
}

#[tauri::command]
fn install_context_menu() -> Result<integration::Status, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this app: {e}"))?;
    integration::install_context_menu(&exe)
}

#[tauri::command]
fn remove_context_menu() -> Result<integration::Status, String> {
    integration::remove_context_menu()
}

/// The whole update check may take this long, however the network behaves.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(5);
/// And this long to connect at all.
const UPDATE_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// The HTTP client the update check uses: ureq's defaults, with a deadline.
fn update_agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(UPDATE_TIMEOUT))
            .timeout_connect(Some(UPDATE_CONNECT_TIMEOUT))
            .build(),
    )
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
    fn versions_compare_numerically_not_alphabetically() {
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.4", "0.1.4"));
        assert!(!is_newer("0.1.3", "0.1.4"));
        assert!(is_newer("0.2", "0.1.9"));
    }

    /// An update check against a network that never answers gives up on time.
    #[test]
    fn the_update_check_gives_up_on_a_network_that_does_not_answer() {
        let started = std::time::Instant::now();
        // TEST-NET-1 (RFC 5737): reserved for documentation, never routed.
        let result = update_agent().get("http://192.0.2.1/").call();
        assert!(result.is_err());
        assert!(
            started.elapsed() < UPDATE_TIMEOUT + Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_capabilities_describe_the_whole_tune_tab() {
        // A status for a file that is not there: the real one would read, and possibly
        // mark, the model installed for whoever runs the tests.
        let missing = std::env::temp_dir().join(format!("inkvec-no-model-{}", std::process::id()));
        let c = describe(denoiser::status_at(Some(missing)));
        assert_eq!(c.controls.len(), 22);
        assert_eq!(c.presets.len(), 8);
        assert_eq!(c.stages.len(), 9);
        assert_eq!(c.version, env!("CARGO_PKG_VERSION"));
        assert!(c.build_target.starts_with(std::env::consts::ARCH));
        assert!(c.presets.iter().any(|p| p.wants_denoiser));
    }
}
