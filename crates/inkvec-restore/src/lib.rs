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
pub mod planar;

pub use planar::MULTIPLE;

#[cfg(feature = "model")]
pub mod model;

#[cfg(feature = "onnxruntime")]
pub mod onnx;

use inkvec_trace::Rgba;
use sha2::{Digest, Sha256};

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

/// An RGBA image composited onto white as interleaved RGB in `[0, 1]`, what every backend
/// receives.
///
/// The network is RGB-only and was trained exclusively on opaque renders (every damage
/// condition in this project's corpus -- JPEG, WebP, a VAE round trip -- produces an opaque
/// raster; there is no alpha-aware restoration to do). Compositing on white with straight
/// alpha matches how every restorer-eval script this project has produced its
/// training/validation input from a transparent source.
pub fn composite_on_white(img: &Rgba) -> Vec<f32> {
    let n = img.width * img.height;
    let mut rgb = vec![0f32; n * 3];
    for i in 0..n {
        let a = img.data[i * 4 + 3];
        for c in 0..3 {
            rgb[i * 3 + c] = img.data[i * 4 + c] * a + (1.0 - a);
        }
    }
    rgb
}

/// The restorer's output as an image: quantised to 256 levels, extremes snapped, and the
/// input's alpha carried through unchanged.
fn finish(mut rgb: Vec<f32>, img: &Rgba) -> Rgba {
    quantize_levels(&mut rgb);
    snap_extremes(&mut rgb);
    let n = img.width * img.height;
    let mut data = vec![0f32; n * 4];
    for i in 0..n {
        for c in 0..3 {
            data[i * 4 + c] = rgb[i * 3 + c];
        }
        data[i * 4 + 3] = img.data[i * 4 + 3];
    }
    Rgba {
        width: img.width,
        height: img.height,
        data,
    }
}

/// Restore an RGBA image (straight alpha). Composite, run the network, quantise, snap, and
/// put the alpha back.
pub fn restore_rgba(r: &dyn Restore, img: &Rgba) -> Result<Rgba, Box<dyn std::error::Error>> {
    let rgb = composite_on_white(img);
    let out = r.restore(&rgb, img.width, img.height)?;
    Ok(finish(out, img))
}

/// The tensor the network takes for an RGBA image: composited onto white, planar CHW, padded
/// to a multiple of [`MULTIPLE`] by replicating the last row and column. `1 x 3 x height x
/// width` of the [`PaddedTensor`]'s own size.
///
/// The in-process backends build this inside [`Restore::restore`] and no caller sees it. It is
/// public for a backend that runs the network somewhere this crate cannot follow it: the
/// browser's ONNX Runtime Web session (`web/denoise.js`), which reads the same `.onnx` export
/// the `onnx` backend reads. Such a backend pairs it with [`network_output`], so that the
/// padding, the crop, the quantisation and the snap are this code rather than a
/// reimplementation of it in another language -- the same reason [`external`] hands its
/// command a PNG instead of asking it to composite.
pub fn network_input(img: &Rgba) -> PaddedTensor {
    let rgb = composite_on_white(img);
    let (width, height) = planar::padded(img.width, img.height);
    PaddedTensor {
        data: planar::to_planar_padded(&rgb, img.width, img.height, width, height),
        width,
        height,
    }
}

/// A planar CHW tensor and the padded size it is laid out at.
#[derive(Debug, Clone)]
pub struct PaddedTensor {
    /// `3 * width * height` floats, channel-major.
    pub data: Vec<f32>,
    /// Padded width, a multiple of [`MULTIPLE`].
    pub width: usize,
    /// Padded height, a multiple of [`MULTIPLE`].
    pub height: usize,
}

/// The image the network's output becomes: cropped back to `img`'s size, quantised to 256
/// levels, extremes snapped, `img`'s alpha carried through. The other half of
/// [`network_input`].
pub fn network_output(chw: &[f32], img: &Rgba) -> Result<Rgba, String> {
    let (pw, ph) = planar::padded(img.width, img.height);
    if chw.len() != 3 * pw * ph {
        return Err(format!(
            "restorer output is {} floats, want {}x{}x3 = {}",
            chw.len(),
            pw,
            ph,
            3 * pw * ph
        ));
    }
    let rgb = planar::from_planar_cropped(chw, img.width, img.height, pw, ph);
    Ok(finish(rgb, img))
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

/// SHA-256 of the published `restorer.onnx`, mirroring `tools/pull_model.py`. A file that
/// does not hash to this is not the model the restorer was validated against.
///
/// Public because it is not only this crate that downloads the file: the browser build
/// fetches the same weights for its own runtime, and checks them against this.
pub const WEIGHTS_SHA256: &str = "bdc2762157632f6f74dd91474f0598e591d49ded47e0a87642c89416b0809d6d";

/// SHA-256 of a file, hex-encoded.
fn file_sha256(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Refuse a weights file whose contents do not match the published model. This is what makes
/// a half-downloaded or tampered `restorer.onnx` an explicit error instead of a corrupt model
/// that fails later, deep inside the runtime, with an unrelated message.
fn verify_weights(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let actual = file_sha256(path)
        .map_err(|e| format!("could not hash restorer weights at {}: {e}", path.display()))?;
    if actual != WEIGHTS_SHA256 {
        return Err(format!(
            "restorer weights at {} failed verification: SHA256 {actual} != {WEIGHTS_SHA256}; \
             re-download from {HF_DENOISER_REPO} with `python tools/pull_model.py`",
            path.display()
        )
        .into());
    }
    Ok(())
}

/// Auto-pull the restorer ONNX weights from Hugging Face if not already present, verifying the
/// result against this crate's expected SHA256 in both cases (already present, or just
/// downloaded).
pub fn pull_onnx_weights(dest: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if dest.is_file() {
        return verify_weights(dest);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    eprintln!(
        "Restorer ONNX weights not found locally. Auto-pulling from Hugging Face ({HF_DENOISER_REPO})..."
    );

    let temp_dest = dest.with_extension("tmp");
    let _ = std::fs::remove_file(&temp_dest);

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
            verify_weights(&temp_dest)?;
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
            verify_weights(&temp_dest)?;
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
                verify_weights(&temp_dest)?;
                std::fs::rename(&temp_dest, dest)?;
                eprintln!(
                    "Successfully downloaded restorer weights to {}",
                    dest.display()
                );
                return Ok(());
            }
        }
    }

    let _ = std::fs::remove_file(&temp_dest);
    Err(format!(
        "Failed to auto-pull the restorer ONNX model from {HF_DENOISER_REPO}. \
         Download it manually with `python tools/pull_model.py` into {}",
        dest.display()
    )
    .into())
}

/// Where the ONNX export is read from when the caller does not say.
///
/// Resolution order: `INKVEC_RESTORE_ONNX`; then `restorer.onnx` beside the executable, or in a
/// `models/` folder beside it, which is how a release archive ships it; then this crate's
/// `models/` directory in the source checkout; then user cache, auto-pulling (and verifying)
/// from Hugging Face (`Logolabs/inkvec-denoiser-001`). A failed pull is an error, never a
/// build-machine path baked into the binary.
#[cfg(feature = "onnxruntime")]
pub fn default_onnx() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    if let Some(p) = std::env::var_os("INKVEC_RESTORE_ONNX") {
        return Ok(std::path::PathBuf::from(p));
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
                return Ok(candidate);
            }
        }
    }
    let manifest_path =
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/models/restorer.onnx"));
    if manifest_path.is_file() {
        return Ok(manifest_path);
    }
    if let Some(cache_path) = user_cache_model_path() {
        // Verifies an already-present file and downloads+verifies a missing one; either way a
        // failure is reported here rather than silently falling through to a non-existent path.
        pull_onnx_weights(&cache_path)?;
        return Ok(cache_path);
    }
    Err(format!(
        "no restorer weights found locally and no user cache directory to pull them into; \
         download the model from {HF_DENOISER_REPO} with `python tools/pull_model.py`"
    )
    .into())
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
            let path = match weights {
                Some(w) => w.to_path_buf(),
                None => default_onnx()?,
            };
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

    /// A network that returns its input is enough to pin the layout: whatever
    /// [`network_input`] lays out, [`network_output`] must read back as the image
    /// [`restore_rgba`] produces from the same backend. The browser backend is exactly this
    /// pair with a real network in the middle, so a drift between the two paths -- a padding
    /// rule applied on one side only, a missing quantisation -- fails here.
    #[test]
    fn the_out_of_process_pair_matches_the_in_process_path() {
        struct Identity;
        impl Restore for Identity {
            fn restore(
                &self,
                rgb: &[f32],
                _w: usize,
                _h: usize,
            ) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
                Ok(rgb.to_vec())
            }
            fn describe(&self) -> String {
                "identity".into()
            }
        }

        // A size that is not a multiple of 16 on either side, so the padding is exercised.
        let (w, h) = (19usize, 7usize);
        let data: Vec<f32> = (0..w * h * 4)
            .map(|i| ((i * 37) % 256) as f32 / 255.0)
            .collect();
        let img = Rgba {
            width: w,
            height: h,
            data,
        };

        let in_process = restore_rgba(&Identity, &img).unwrap();

        let t = network_input(&img);
        assert_eq!((t.width, t.height), (32, 16));
        assert_eq!(t.data.len(), 3 * t.width * t.height);
        let out_of_process = network_output(&t.data, &img).unwrap();

        assert_eq!(out_of_process.width, in_process.width);
        assert_eq!(out_of_process.height, in_process.height);
        assert_eq!(out_of_process.data, in_process.data);
    }

    #[test]
    fn network_output_refuses_a_tensor_of_the_wrong_size() {
        let img = Rgba {
            width: 4,
            height: 4,
            data: vec![1.0; 4 * 4 * 4],
        };
        let err = network_output(&[0.0; 3 * 16 * 16 - 1], &img).unwrap_err();
        assert!(err.contains("want 16x16x3"), "got: {err}");
    }

    #[test]
    fn verify_weights_rejects_a_corrupt_file() {
        let dir =
            std::env::temp_dir().join(format!("inkvec-restore-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("restorer.onnx");
        std::fs::write(&path, b"not the model").unwrap();
        let err = verify_weights(&path).unwrap_err();
        assert!(
            err.to_string().contains(WEIGHTS_SHA256),
            "error must name the expected hash, got: {err}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pull_onnx_weights_verifies_an_existing_file_before_downloading() {
        // The "already present" short-circuit must verify, not just return Ok, so a corrupt
        // or truncated cache file is refused without any network access.
        let dir = std::env::temp_dir().join(format!("inkvec-restore-pull-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("restorer.onnx");
        std::fs::write(&path, b"garbage").unwrap();
        let err = pull_onnx_weights(&path).unwrap_err();
        assert!(
            err.to_string().contains("verification"),
            "error must say verification failed, got: {err}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
