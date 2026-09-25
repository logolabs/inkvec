//! Inkvec Studio Lite: the Studio's shared core, compiled to WebAssembly.
//!
//! This is the browser's counterpart to `studio/src-tauri`. Both are shells round
//! `inkvec_studio_core`; this one is driven from a Web Worker (`studio/web/studio-worker.js`)
//! that answers the same commands, with the same argument and result shapes, that the
//! desktop app's Tauri commands answer -- `studio/src/lib/ipc.ts` types them once for both.
//!
//! What differs is only what a browser tab is:
//!
//! * **No threads of its own.** The desktop runs every trace on a thread and keeps the window
//!   answerable; here the whole worker is that thread, so a trace is one synchronous call and
//!   the page's transport (`studio/src/lib/web/transport.ts`) queues and retires requests the
//!   way the desktop's scheduler and generations do. The pipeline's own parallelism is
//!   rayon's, on a pool of Web Workers where the page is cross-origin isolated (the
//!   `threads` feature, `initThreadPool`) and on this one thread where it is not.
//! * **No file system.** An image arrives as bytes, an export leaves as bytes for the page to
//!   download, preferences live in the page's storage (sanitised here, so the clamps are the
//!   desktop's), and the engine's confidence bands, which it writes to a file, are not
//!   available.
//! * **The denoiser is ONNX Runtime Web**, in a worker of its own. The pipeline is
//!   synchronous and the network is not, so the tensor crosses through a shared buffer the
//!   trace blocks on (`inkvecDenoiseSync`, installed by the worker); see
//!   [`enable_denoiser`].

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use wasm_bindgen::prelude::*;

use inkvec_studio_core::{api, minify, options, prefs, trace, wizard};

#[cfg(feature = "threads")]
pub use wasm_bindgen_rayon::init_thread_pool;

/// The longest side a trace may ask for in a browser tab. The desktop allows 16384; a tab's
/// 4 GiB address space and one pipeline's working copies make 2048 the honest ceiling.
pub const MAX_TRACE_PX: u32 = 2048;

/// The trace size a first visit starts at: half the desktop's, for a trace that feels like
/// the desktop's on a laptop's cores. 2048 is one choice away in the Tune tab.
pub const DEFAULT_TRACE_PX: u32 = 1024;

/// Whether this is the threaded build, whose pool `initThreadPool` starts.
#[wasm_bindgen]
pub fn threads_available() -> bool {
    cfg!(feature = "threads")
}

/// The app's version, which is the engine's: they are built from one tree.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[wasm_bindgen]
extern "C" {
    /// Run the denoiser network on one tensor and wait for the answer. Installed on the
    /// worker's global scope by `studio-worker.js`: it posts the tensor to the denoiser's own
    /// worker and blocks on a `SharedArrayBuffer` until that worker has written the result.
    #[wasm_bindgen(catch, js_namespace = globalThis, js_name = inkvecDenoiseSync)]
    fn denoise_sync(
        input: &[f32],
        width: u32,
        height: u32,
    ) -> Result<js_sys::Float32Array, JsValue>;
}

fn denoise_bridge(input: &[f32], width: usize, height: usize) -> Result<Vec<f32>, String> {
    denoise_sync(input, width as u32, height as u32)
        .map(|out| out.to_vec())
        .map_err(|e| message_of(&e))
}

/// Route every trace that asks for "Clean up damage" through the page's denoiser. Called
/// once by the worker when the page can block on a shared buffer (it is cross-origin
/// isolated); without it such a trace runs as the desktop app does without its model.
#[wasm_bindgen]
pub fn enable_denoiser() {
    trace::set_external_denoiser(denoise_bridge);
}

/// The Hugging Face URL of the denoiser weights, the same file the command line pulls.
#[wasm_bindgen]
pub fn denoiser_model_url() -> String {
    inkvec_restore::HF_DENOISER_URL.to_string()
}

/// The Hugging Face repository the weights come from.
#[wasm_bindgen]
pub fn denoiser_model_repo() -> String {
    inkvec_restore::HF_DENOISER_REPO.to_string()
}

/// The SHA-256 the weights must hash to.
#[wasm_bindgen]
pub fn denoiser_model_sha256() -> String {
    inkvec_restore::WEIGHTS_SHA256.to_string()
}

/// One Studio session: the image that is open and what has been traced of it. The
/// browser's `AppState`.
#[wasm_bindgen]
pub struct Studio {
    source: Option<Arc<trace::Source>>,
    /// The last drawing traced, so an output option does not cost a trace.
    traced: trace::Cache,
    /// The confidence bands of the last trace answered, by generation.
    bands: Option<(u64, String)>,
}

impl Default for Studio {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl Studio {
    /// A session with nothing open.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Studio {
        console_error_panic_hook::set_once();
        Studio {
            source: None,
            traced: trace::Cache::default(),
            bands: None,
        }
    }

    /// What this build can do (`capabilities`), with the denoiser's standing as the page
    /// measured it: whether it can run here, and whether the weights are in its cache.
    pub fn capabilities(
        &self,
        denoiser_supported: bool,
        denoiser_installed: bool,
        denoiser_bytes: Option<f64>,
    ) -> Result<String, JsValue> {
        let status = api::DenoiserStatus {
            supported: denoiser_supported,
            installed: denoiser_installed,
            path: None,
            bytes: denoiser_bytes.map(|b| b as u64),
            repo: inkvec_restore::HF_DENOISER_REPO,
            sha256: inkvec_restore::WEIGHTS_SHA256,
        };
        to_json(&web_capabilities(status))
    }

    /// Open an image (`open_bytes`): PNG, JPEG, WebP, GIF, BMP, TIFF or SVG bytes.
    pub fn open_bytes(&mut self, bytes: Vec<u8>, name: Option<String>) -> Result<String, JsValue> {
        let source = trace::Source::open(bytes, name.map(std::path::PathBuf::from))
            .map_err(|e| JsValue::from_str(&e))?;
        let info = api::source_info(&source).map_err(|e| JsValue::from_str(&e))?;
        self.source = Some(Arc::new(source));
        self.traced.clear();
        self.bands = None;
        to_json(&info)
    }

    /// Trace the open image (`start_trace`'s work), synchronously.
    ///
    /// `on_stage(name, ms)` is called as the pipeline passes each stage; `is_current()` is
    /// asked, once the drawing is done, whether the page still wants this generation --
    /// a drawing it has moved past is kept in the cache but not measured. Returns the
    /// outcome as JSON (`trace:done`'s `outcome`), or `undefined` when it was retired.
    pub fn trace(
        &mut self,
        request: &str,
        draft_px: u32,
        draft_seconds: f64,
        generation: f64,
        on_stage: &js_sys::Function,
        is_current: &js_sys::Function,
    ) -> Result<Option<String>, JsValue> {
        let source = self.source()?;
        let request: api::TraceRequest = from_json(request)?;
        let asked = web_settings(request.settings);
        let (settings, tier) = trace::plan(
            &asked,
            request.tier,
            draft_px,
            draft_seconds,
            (source.width, source.height),
        );
        let current = || {
            is_current
                .call1(&JsValue::NULL, &JsValue::from_f64(generation))
                .map(|v| v.is_truthy())
                .unwrap_or(true)
        };
        let stage = on_stage.clone();
        let drawn = trace::draw(
            &source,
            &settings,
            tier,
            Some(&self.traced),
            trace::MeasureLevel::Full,
            move |name, ms| {
                let _ = stage.call2(&JsValue::NULL, &JsValue::from_str(name), &ms.into());
            },
        );
        let mut outcome = match drawn {
            Ok(_) if !current() => return Ok(None),
            Ok(drawn) => trace::measure(
                &source,
                drawn,
                trace::MeasureLevel::Full,
                Some(&self.traced),
            ),
            Err(outcome) => outcome,
        };
        let bands = match &mut outcome {
            trace::Outcome::Traced(t) => t.bands.take(),
            _ => None,
        };
        if !current() {
            return Ok(None);
        }
        self.bands = bands.map(|b| (generation as u64, b));
        to_json(&outcome).map(Some)
    }

    /// A wizard preview (`start_preview`'s work): a draft of `request.settings` that does
    /// not become the drawing.
    pub fn preview(
        &self,
        request: &str,
        draft_px: u32,
        draft_seconds: f64,
    ) -> Result<String, JsValue> {
        let source = self.source()?;
        let request: api::TraceRequest = from_json(request)?;
        let mut outcome = wizard::run_preview(
            &source,
            &web_settings(request.settings),
            draft_px,
            draft_seconds,
        );
        if let trace::Outcome::Traced(t) = &mut outcome {
            t.bands = None;
        }
        to_json(&outcome)
    }

    /// Every other command, by its Tauri name, with its arguments as the interface sends
    /// them (a JSON object, camelCase keys). Returns the reply as JSON.
    pub fn call(&mut self, cmd: &str, args: &str) -> Result<String, JsValue> {
        match cmd {
            "source_facts" => {
                let a: SourceFactsArgs = from_json(args)?;
                let raster = self
                    .source()?
                    .raster(a.max_dim)
                    .map_err(|e| JsValue::from_str(&e))?;
                to_json(&wizard::facts_of(&raster))
            }
            "trace_bands" => {
                let a: BandsArgs = from_json(args)?;
                let held = self
                    .bands
                    .as_ref()
                    .filter(|(g, _)| *g == a.generation as u64)
                    .map(|(_, b)| b.as_str());
                to_json(&held)
            }
            "snap_inks" => {
                let a: SnapArgs = from_json(args)?;
                to_json(&err(api::snap_inks(a.svg, a.snaps, a.width, a.height))?)
            }
            "match_palette" => {
                let a: MatchArgs = from_json(args)?;
                to_json(&api::match_palette(a.traced, a.pasted))
            }
            "plan_export" => {
                let a: ExportArgs = from_json(args)?;
                let built = err(api::build_export(&a.request, &*self.source()?))?;
                to_json(&api::planned(&built))
            }
            "minify_svg" => {
                let a: MinifyArgs = from_json(args)?;
                to_json(&err(minify::run(&a.svg, a.settings))?)
            }
            "fab_analyze" => {
                let a: SvgArgs = from_json(args)?;
                to_json(&err(inkvec_fab::analyze(&a.svg).map_err(|e| e.to_string()))?)
            }
            "fab_prepare" => {
                let a: FabArgs = from_json(args)?;
                to_json(&err(
                    inkvec_fab::prepare(&a.svg, &a.options).map_err(|e| e.to_string())
                )?)
            }
            "default_prefs" => to_json(&web_prefs(prefs::Prefs::default())),
            "sanitise_prefs" => {
                let a: PrefsArgs = from_json(args)?;
                let parsed = serde_json::from_value::<prefs::Prefs>(a.prefs)
                    .map(web_prefs)
                    .unwrap_or_else(|_| web_prefs(prefs::Prefs::default()));
                to_json(&parsed)
            }
            other => Err(JsValue::from_str(&format!(
                "{other} is not something the browser build does"
            ))),
        }
    }

    /// Build an export (`write_export`'s work) and hand every file back as
    /// `[{ name, data }]`, for the page to download. With more than one file, `zip` packs
    /// them into one archive instead, returned as a single `{ name: "", data }`.
    pub fn export_files(&self, request: &str, zip: bool) -> Result<js_sys::Array, JsValue> {
        let a: ExportArgs = from_json(request)?;
        let built = err(api::build_export(&a.request, &*self.source()?))?;
        let out = js_sys::Array::new();
        let push = |name: &str, data: &[u8]| {
            let o = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&o, &"name".into(), &JsValue::from_str(name));
            let _ =
                js_sys::Reflect::set(&o, &"data".into(), &js_sys::Uint8Array::from(data).into());
            out.push(&o);
        };
        if zip && built.len() > 1 {
            push("", &err(inkvec_studio_core::export::zip(&built))?);
        } else {
            for a in &built {
                push(&a.name, &a.data);
            }
        }
        Ok(out)
    }

    /// Forget the open image and everything traced from it.
    pub fn close(&mut self) {
        self.source = None;
        self.traced.clear();
        self.bands = None;
    }
}

impl Studio {
    fn source(&self) -> Result<Arc<trace::Source>, JsValue> {
        self.source
            .clone()
            .ok_or_else(|| JsValue::from_str("no image is open"))
    }
}

/// The desktop's capabilities, narrowed to what a tab runs: `platform` is `web`, the trace
/// size tops out at [`MAX_TRACE_PX`], and every preset starts at [`DEFAULT_TRACE_PX`].
fn web_capabilities(denoiser: api::DenoiserStatus) -> api::Capabilities {
    let mut caps = api::describe(denoiser, "web");
    for c in &mut caps.controls {
        if c.key == "traceSize" {
            c.max = c.max.min(MAX_TRACE_PX as f64);
        }
    }
    for p in &mut caps.presets {
        p.settings.trace_size = p.settings.trace_size.min(DEFAULT_TRACE_PX);
    }
    caps
}

/// Settings as a tab can run them: no larger than [`MAX_TRACE_PX`].
fn web_settings(mut s: options::Settings) -> options::Settings {
    s.trace_size = s.trace_size.clamp(1, MAX_TRACE_PX);
    s
}

/// Preferences as the web build keeps them: the desktop's clamps, the tab's trace-size
/// ceiling, and no update check (a page is always the version it is served as).
fn web_prefs(p: prefs::Prefs) -> prefs::Prefs {
    let fresh = p == prefs::Prefs::default();
    let mut p = p.sanitised();
    if fresh {
        p.trace.trace_size = DEFAULT_TRACE_PX;
    }
    p.trace = web_settings(p.trace);
    for s in &mut p.saved {
        s.settings = web_settings(s.settings.clone());
    }
    p.check_updates = false;
    p.threads = None;
    p.output_folder = None;
    p.recent.clear();
    p
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceFactsArgs {
    max_dim: usize,
}

#[derive(Deserialize)]
struct BandsArgs {
    generation: f64,
}

#[derive(Deserialize)]
struct SnapArgs {
    svg: String,
    snaps: Vec<api::Snap>,
    width: u32,
    height: u32,
}

#[derive(Deserialize)]
struct MatchArgs {
    traced: Vec<String>,
    pasted: String,
}

#[derive(Deserialize)]
struct ExportArgs {
    request: api::ExportRequest,
}

#[derive(Deserialize)]
struct MinifyArgs {
    svg: String,
    settings: minify::MinifySettings,
}

#[derive(Deserialize)]
struct SvgArgs {
    svg: String,
}

#[derive(Deserialize)]
struct FabArgs {
    svg: String,
    options: inkvec_fab::Options,
}

#[derive(Deserialize)]
struct PrefsArgs {
    prefs: serde_json::Value,
}

fn from_json<T: DeserializeOwned>(text: &str) -> Result<T, JsValue> {
    serde_json::from_str(if text.trim().is_empty() { "{}" } else { text })
        .map_err(|e| JsValue::from_str(&format!("the request could not be read: {e}")))
}

fn to_json<T: serde::Serialize + ?Sized>(value: &T) -> Result<String, JsValue> {
    serde_json::to_string(value)
        .map_err(|e| JsValue::from_str(&format!("the reply could not be written: {e}")))
}

fn err<T>(r: Result<T, String>) -> Result<T, JsValue> {
    r.map_err(|e| JsValue::from_str(&e))
}

fn message_of(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| {
            js_sys::Reflect::get(e, &"message".into())
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| "the denoiser failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_denoiser() -> api::DenoiserStatus {
        api::DenoiserStatus {
            supported: false,
            installed: false,
            path: None,
            bytes: None,
            repo: inkvec_restore::HF_DENOISER_REPO,
            sha256: inkvec_restore::WEIGHTS_SHA256,
        }
    }

    #[test]
    fn the_web_build_says_it_is_the_web_and_caps_the_trace_size() {
        let caps = web_capabilities(no_denoiser());
        assert_eq!(caps.platform, "web");
        let size = caps
            .controls
            .iter()
            .find(|c| c.key == "traceSize")
            .expect("a trace size control");
        assert_eq!(size.max, MAX_TRACE_PX as f64);
        assert!(caps
            .presets
            .iter()
            .all(|p| p.settings.trace_size <= DEFAULT_TRACE_PX));
        // Everything else is the desktop's.
        let desk = api::describe(no_denoiser(), "windows");
        assert_eq!(caps.controls.len(), desk.controls.len());
        assert_eq!(caps.presets.len(), desk.presets.len());
    }

    #[test]
    fn a_first_visit_traces_at_1024_and_a_saved_2048_is_kept() {
        let fresh = web_prefs(prefs::Prefs::default());
        assert_eq!(fresh.trace.trace_size, DEFAULT_TRACE_PX);
        assert!(!fresh.check_updates);
        let mut chosen = prefs::Prefs::default();
        chosen.trace.trace_size = 2048;
        chosen.theme = prefs::Theme::Light;
        assert_eq!(web_prefs(chosen).trace.trace_size, 2048);
        let mut huge = prefs::Prefs::default();
        huge.trace.trace_size = 8000;
        huge.theme = prefs::Theme::Light;
        assert_eq!(web_prefs(huge).trace.trace_size, MAX_TRACE_PX);
    }
}
