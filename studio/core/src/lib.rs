//! Inkvec Studio's shared core.
//!
//! Everything the Studio does that does not depend on where it runs: opening an image,
//! tracing it in two tiers, measuring the drawing against its source, the palette and its
//! colour groups, what could not be recovered, the wizard's facts and previews, export,
//! the SVG minifier and fabrication. There is no window, no file system and no thread of
//! its own in here -- the long work fans out over rayon, which the desktop runs on its
//! cores and the browser on a pool of Web Workers (or on the one thread it has).
//!
//! Two shells wrap it:
//!
//! * **Inkvec Studio** (`studio/src-tauri`), the desktop app: Tauri commands, a thread per
//!   trace, events, files and preferences on disk, the native denoiser, batch runs.
//! * **Inkvec Studio Lite** (`studio/wasm`), the same interface in a browser tab: this core
//!   compiled to WebAssembly and driven from a Web Worker with the same commands and
//!   events, downloads for files and the browser's storage for preferences.
//!
//! A feature added here is a feature of both.

/// The app's name as this build is called: the desktop app is Inkvec Studio, the browser
/// build Inkvec Studio Lite. It signs what an export writes (`palette.json`'s `generator`,
/// the asset pack's README).
pub const APP_NAME: &str = if cfg!(target_arch = "wasm32") {
    "Inkvec Studio Lite"
} else {
    "Inkvec Studio"
};

pub mod api;
pub mod export;
pub mod interface;
pub mod lost;
pub mod minify;
pub mod options;
pub mod prefs;
pub mod quality;
pub mod trace;
pub mod wizard;
