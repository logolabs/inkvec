// A thin shim. Everything lives in the library half, which is also what Tauri's mobile
// entry point would take.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    inkvec_studio_lib::run();
}
