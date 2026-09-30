//! The desktop app's executable: a thin shim. Everything lives in the library half
//! (`lib.rs`, `inkvec_studio_lib::run`), which is also what Tauri's mobile entry point would
//! take. A release build is a Windows GUI program, with no console window behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Start the app.
fn main() {
    inkvec_studio_lib::run();
}
