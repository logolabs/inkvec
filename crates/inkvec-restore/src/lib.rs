//! An optional trained-restorer pre-pass for inkvec.
//!
//! For JPEG/WebP/AI-decoder-damaged input, running the trained restorer before tracing takes
//! colour error down ~28-45% and parameter count down ~29-33% in benchmarks. On **clean**
//! input the same pass still costs a few percent of colour accuracy, which is why it is a mode and not a default, and why
//! [`Mode::Auto`] exists: it borrows [`inkvec_sr::detect`]'s interior-residual damage test
//! (the same "trace once, measure disagreement where the trace claims flat" signal SR already
//! uses) rather than inventing a second detector for a second cleaner. That threshold was
//! calibrated for SR's cost asymmetry, not independently re-measured for this restorer's
//! (smaller) clean-input cost, so it is a borrowed, conservative choice, not a proven-optimal
//! one -- see `Options::residual_threshold`.
//!
//! Restoration and the tracer's "soft intake" (measured-noise, relaxed same-ink merge) travel
//! together: a restored image can look clean enough that the automatic edge-width/ringing
//! detector no longer opens soft intake on its own, so a caller that restores must force it,
//! the same way `--lossy on` does (`inkvec-cli` does this whenever restoration runs).
//!
//! Everything here except the network itself is a few hundred lines of arithmetic and ships in
//! the binary unconditionally. The network is a [`Restore`] implementation: ONNX Runtime
//! (feature `onnxruntime`), Burn (feature `model` plus a backend), or an external command. The
//! mode logic does not know the difference, the same split `inkvec-sr` uses and for the same
//! reason: the network pulls in a full ML runtime, and nothing else here should have to.

pub mod external;
mod planar;

pub use planar::MULTIPLE;

#[cfg(feature = "model")]
pub mod model;

#[cfg(feature = "onnxruntime")]
pub mod onnx;

use inkvec_trace::Rgba;

/// When to restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Trace, measure the fit, restore and retrace only if the fit is bad.
    #[default]
    Auto,
    /// Always restore first.
    On,
    /// Never restore. Identical to not having the mode.
    Off,
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Mode::Auto),
            "on" => Ok(Mode::On),
            "off" => Ok(Mode::Off),
            other => Err(format!(
                "unknown restore mode {other:?}; want auto, on or off"
            )),
        }
    }
}

/// Something that can restore a damaged raster in place (same size, same channel count).
pub trait Restore {
    /// Restore straight RGB in `[0, 1]`, row-major, 3 floats per pixel.
    fn restore(
        &self,
        rgb: &[f32],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, Box<dyn std::error::Error>>;

    /// A one-line description for logging.
    fn describe(&self) -> String;
}

/// Pixels within this many /255 levels of an extreme, on every channel, snap to it exactly.
///
/// The restorer's direct-output head has no gradient once its internal clamp is active, so it
/// settles a few levels short of true black/white (`#fdffff`/`#040101` rather than white/black)
/// — measured and explained in experiments 10p B.1. Snapping is the fix validated there; it is
/// applied uniformly regardless of which [`Restore`] backend ran, the same way `inkvec_sr`
/// applies `clean::match_flats` after either of its upscaler backends.
pub const SNAP_LEVELS: u8 = 6;

/// Round to the nearest of 256 levels, clamped to `[0, 1]`.
///
/// Every measurement behind this pre-pass traced a restored image read back from an 8-bit PNG
/// (the research evals, `restore_cli.py`, the [`external`] backend), and the tracer's noise
/// estimate reads those levels. An in-process backend hands back unquantised floats; rounding
/// them here keeps every backend on the recipe that was validated instead of a near neighbour
/// of it.
fn quantize_levels(rgb: &mut [f32]) {
    for v in rgb.iter_mut() {
        *v = (v.clamp(0.0, 1.0) * 255.0).round() / 255.0;
    }
}

fn snap_extremes(rgb: &mut [f32]) {
    for px in rgb.as_chunks_mut::<3>().0 {
        let hi = px.iter().all(|&v| v >= 1.0 - SNAP_LEVELS as f32 / 255.0);
        let lo = px.iter().all(|&v| v <= SNAP_LEVELS as f32 / 255.0);
        if hi {
            px.iter_mut().for_each(|v| *v = 1.0);
        } else if lo {
            px.iter_mut().for_each(|v| *v = 0.0);
        }
    }
}

/// Restore an RGBA image (straight alpha), compositing onto white first -- the network is
/// RGB-only and was trained exclusively on opaque renders (every damage condition in this
/// project's corpus -- JPEG, WebP, a VAE round trip -- produces an opaque raster; there is no
/// alpha-aware restoration to do). Alpha is carried through unchanged.
pub fn restore_rgba(r: &dyn Restore, img: &Rgba) -> Result<Rgba, Box<dyn std::error::Error>> {
    let n = img.width * img.height;
    let mut rgb = vec![0f32; n * 3];
    for i in 0..n {
        let a = img.data[i * 4 + 3];
        for c in 0..3 {
            // On white, straight alpha: matches how every restorer-eval script this project
            // has produced its training/validation input composited a transparent source.
            rgb[i * 3 + c] = img.data[i * 4 + c] * a + (1.0 - a);
        }
    }
    let mut out = r.restore(&rgb, img.width, img.height)?;
    quantize_levels(&mut out);
    snap_extremes(&mut out);
    let mut data = vec![0f32; n * 4];
    for i in 0..n {
        for c in 0..3 {
            data[i * 4 + c] = out[i * 3 + c];
        }
        data[i * 4 + 3] = img.data[i * 4 + 3];
    }
    Ok(Rgba {
        width: img.width,
        height: img.height,
        data,
    })
}

/// Where the built-in restorer's weights come from when the caller does not say: the
/// `INKVEC_RESTORE_WEIGHTS` environment variable, else the file the build converted.
#[cfg(feature = "model")]
pub fn default_weights() -> std::path::PathBuf {
    std::env::var_os("INKVEC_RESTORE_WEIGHTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(model::BUILD_WEIGHTS))
}

/// Hugging Face repository where denoiser weights are hosted.
pub const HF_DENOISER_REPO: &str = "Logolabs/inkvec-denoiser-001";
/// Direct download URL for the ONNX restorer weights.
pub const HF_DENOISER_URL: &str =
    "https://huggingface.co/Logolabs/inkvec-denoiser-001/resolve/main/restorer.onnx";

/// Path to user-level cache directory for inkvec models.
pub fn user_cache_model_path() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|p| std::path::PathBuf::from(p).join("AppData").join("Local"))
        });
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| std::path::PathBuf::from(p).join(".cache")));

    base.map(|b| b.join("inkvec").join("models").join("restorer.onnx"))
}

/// Auto-pull the restorer ONNX weights from Hugging Face if not already present.
pub fn pull_onnx_weights(dest: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if dest.is_file() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    eprintln!(
        "Restorer ONNX weights not found locally. Auto-pulling from Hugging Face ({HF_DENOISER_REPO})..."
    );

    let temp_dest = dest.with_extension("tmp");

    // 1. Try curl if available
    let curl_status = std::process::Command::new("curl")
        .args([
            "-fSL",
            "-o",
            temp_dest.to_str().unwrap_or(""),
            HF_DENOISER_URL,
        ])
        .status();

    if let Ok(status) = curl_status {
        if status.success() && temp_dest.is_file() {
            std::fs::rename(&temp_dest, dest)?;
            eprintln!(
                "Successfully downloaded restorer weights to {}",
                dest.display()
            );
            return Ok(());
        }
    }

    // 2. Try python via urllib
    let py_script = format!(
        "import urllib.request; urllib.request.urlretrieve(r'{}', r'{}')",
        HF_DENOISER_URL,
        temp_dest.display()
    );
    let py_status = std::process::Command::new("python")
        .args(["-c", &py_script])
        .status();

    if let Ok(status) = py_status {
        if status.success() && temp_dest.is_file() {
            std::fs::rename(&temp_dest, dest)?;
            eprintln!(
                "Successfully downloaded restorer weights to {}",
                dest.display()
            );
            return Ok(());
        }
    }

    // 3. Try powershell on Windows
    #[cfg(windows)]
    {
        let ps_cmd = format!(
            "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; (New-Object Net.WebClient).DownloadFile('{}', '{}')",
            HF_DENOISER_URL,
            temp_dest.display()
        );
        let ps_status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps_cmd])
            .status();

        if let Ok(status) = ps_status {
            if status.success() && temp_dest.is_file() {
                std::fs::rename(&temp_dest, dest)?;
                eprintln!(
                    "Successfully downloaded restorer weights to {}",
                    dest.display()
                );
                return Ok(());
            }
        }
    }

    Err(format!(
        "Failed to auto-pull restorer ONNX model from {HF_DENOISER_URL}. Please download manually to {}",
        dest.display()
    )
    .into())
}

/// Where the ONNX export is read from when the caller does not say.
///
/// Resolution order: `INKVEC_RESTORE_ONNX`; then `restorer.onnx` beside the executable, or in a
/// `models/` folder beside it, which is how a release archive ships it; then this crate's
/// `models/` directory in the source checkout; then user cache; if not present, auto-pulls
/// from Hugging Face (`Logolabs/inkvec-denoiser-001`).
#[cfg(feature = "onnxruntime")]
pub fn default_onnx() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("INKVEC_RESTORE_ONNX") {
        return std::path::PathBuf::from(p);
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
    {
        for candidate in [
            dir.join("restorer.onnx"),
            dir.join("models").join("restorer.onnx"),
        ] {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    let manifest_path =
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/models/restorer.onnx"));
    if manifest_path.is_file() {
        return manifest_path;
    }
    if let Some(cache_path) = user_cache_model_path() {
        if cache_path.is_file() {
            return cache_path;
        }
        if pull_onnx_weights(&cache_path).is_ok() {
            return cache_path;
        }
    }
    manifest_path
}

/// Load the in-process restorer on the fastest runtime compiled in. ONNX Runtime first (the
/// fast CPU path) unless `weights` is a Burn `.bpk` file; then Burn on CUDA, wgpu, flex, ndarray.
#[cfg(any(feature = "model", feature = "onnxruntime"))]
pub fn load_builtin(
    weights: Option<&std::path::Path>,
) -> Result<Box<dyn Restore>, Box<dyn std::error::Error>> {
    #[cfg(feature = "onnxruntime")]
    {
        let burn_weights = weights
            .and_then(std::path::Path::extension)
            .is_some_and(|e| e.eq_ignore_ascii_case("bpk"));
        if !burn_weights {
            let path = weights
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(default_onnx);
            return Ok(Box::new(onnx::OnnxRestorer::load(&path)?));
        }
    }
    #[cfg(feature = "model")]
    {
        load_burn(
            &weights
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(default_weights),
        )
    }
    #[cfg(not(feature = "model"))]
    {
        let _ = weights;
        Err("Burn .bpk weights need inkvec-restore's `model` feature".into())
    }
}

#[cfg(feature = "model")]
fn load_burn(path: &std::path::Path) -> Result<Box<dyn Restore>, Box<dyn std::error::Error>> {
    #[cfg(feature = "cuda")]
    {
        Ok(Box::new(model::Restorer::<burn::backend::Cuda>::load(
            path,
            Default::default(),
        )?))
    }
    #[cfg(all(feature = "wgpu", not(feature = "cuda")))]
    {
        Ok(Box::new(model::Restorer::<burn::backend::Wgpu>::load(
            path,
            Default::default(),
        )?))
    }
    #[cfg(all(feature = "flex", not(any(feature = "cuda", feature = "wgpu"))))]
    {
        Ok(Box::new(model::Restorer::<burn::backend::Flex>::load(
            path,
            Default::default(),
        )?))
    }
    #[cfg(all(
        feature = "ndarray",
        not(any(feature = "cuda", feature = "wgpu", feature = "flex"))
    ))]
    {
        Ok(Box::new(model::Restorer::<burn::backend::NdArray>::load(
            path,
            Default::default(),
        )?))
    }
    #[cfg(not(any(
        feature = "cuda",
        feature = "wgpu",
        feature = "flex",
        feature = "ndarray"
    )))]
    {
        let _ = path;
        Err(
            "inkvec-restore was built with `model` but no backend (flex, ndarray, wgpu or cuda)"
                .into(),
        )
    }
}

/// Options for [`decide`].
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Interior-residual threshold above which an input is treated as damaged. Borrowed from
    /// `inkvec_sr::detect::DEGRADED_RESIDUAL` by default; see the module doc for why that is a
    /// conservative choice rather than a validated one for this specific network.
    pub residual_threshold: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            residual_threshold: inkvec_sr::detect::DEGRADED_RESIDUAL,
        }
    }
}

/// What [`decide`] concluded, and why.
#[derive(Debug, Clone, Copy)]
pub enum Decision {
    /// Restore it.
    Restore {
        /// The interior residual that triggered this.
        residual: Option<f64>,
    },
    /// Leave it alone, and keep the trace already produced.
    Keep {
        /// The interior residual measured, or `None` when the trace could not be rendered or
        /// had no flat interior to measure.
        residual: Option<f64>,
    },
}

/// Decide whether a traced result is good enough to keep, or whether the input looks damaged
/// enough to be worth restoring first. Same signal as `inkvec_sr::decide`, deliberately: both
/// questions are "does this input disagree with its own trace where the trace claims flat",
/// and a second, differently-calibrated detector for the same question would be a second thing
/// to keep in sync, not a more accurate one.
pub fn decide(img: &Rgba, svg: &str, opt: Options) -> Decision {
    let Ok(model) = inkvec_sr::detect::render_svg(svg, img.width, img.height) else {
        return Decision::Keep { residual: None };
    };
    match inkvec_sr::detect::interior_residual(img, &model) {
        Some(r) if r > opt.residual_threshold => Decision::Restore { residual: Some(r) },
        r => Decision::Keep { residual: r },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_extremes_only_touches_pixels_near_an_extreme() {
        let mut rgb = vec![
            1.0, 1.0, 1.0, // already white
            0.98, 0.99, 1.0, // within 6/255 of white on every channel -> snaps
            0.5, 0.5, 0.5, // mid-grey -> untouched
            0.02, 0.0, 0.01, // within 6/255 of black -> snaps
            0.5, 0.0, 0.0, // only one channel near black -> untouched
        ];
        snap_extremes(&mut rgb);
        assert_eq!(&rgb[3..6], &[1.0, 1.0, 1.0]);
        assert_eq!(&rgb[6..9], &[0.5, 0.5, 0.5]);
        assert_eq!(&rgb[9..12], &[0.0, 0.0, 0.0]);
        assert_eq!(&rgb[12..15], &[0.5, 0.0, 0.0]);
    }
}
