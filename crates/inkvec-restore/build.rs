//! Generates the restorer's Burn model code from its ONNX export, only when the `model` feature
//! is enabled -- a default build of this crate needs neither the ONNX file nor burn-onnx.
//!
//! The ONNX file is not in the repository (tens of MB of weights). Point `INKVEC_RESTORE_ONNX`
//! at it, or place it at `crates/inkvec-restore/models/restorer.onnx`. It is produced by
//! `export_restorer_onnx.py` in the training repository.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=INKVEC_RESTORE_ONNX");
    if std::env::var_os("CARGO_FEATURE_MODEL").is_none() {
        return;
    }
    generate();
}

#[cfg(feature = "model")]
fn generate() {
    let onnx_str =
        std::env::var("INKVEC_RESTORE_ONNX").unwrap_or_else(|_| "models/restorer.onnx".into());
    let onnx = std::path::PathBuf::from(&onnx_str);
    if !onnx.exists() {
        eprintln!(
            "Restorer ONNX model not found at {}. Auto-pulling from Hugging Face (Logolabs/inkvec-denoiser-001)...",
            onnx.display()
        );
        if let Some(parent) = onnx.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let url =
            "https://huggingface.co/Logolabs/inkvec-denoiser-001/resolve/main/restorer.onnx";
        let temp = onnx.with_extension("tmp");
        let curl_ok = std::process::Command::new("curl")
            .args(["-fSL", "-o", temp.to_str().unwrap_or(""), url])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if curl_ok && temp.is_file() {
            let _ = std::fs::rename(&temp, &onnx);
        }
    }
    if !onnx.exists() {
        panic!(
            "feature `model` needs the restorer ONNX export: set INKVEC_RESTORE_ONNX or place it at \
             crates/inkvec-restore/models/restorer.onnx (not found: {onnx_str})"
        );
    }
    println!("cargo:rerun-if-changed={onnx}");
    burn_onnx::ModelGen::new()
        .input(&onnx)
        .out_dir("model/")
        .run_from_script();
}

#[cfg(not(feature = "model"))]
fn generate() {}
