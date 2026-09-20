//! Inkvec: raster logos, icons and illustrations to compact, accurate SVG.
//!
//! This crate is the stable library API. It is the whole pipeline behind the `inkvec`
//! command line, behind a small surface that the C ABI (`inkvec-ffi`), the Python package
//! and the other language bindings all call:
//!
//! * [`trace`] -- encoded image bytes (PNG, JPEG, WebP, GIF, BMP, TIFF) in, SVG out;
//! * [`trace_rgba`] -- raw straight RGBA8 pixels in, SVG out;
//! * [`Options`] -- every knob, with the command line's defaults, as one serde struct whose
//!   JSON Schema ([`options_schema_json`]) the bindings are generated from;
//! * [`Traced`] and [`Error`] -- the result.
//!
//! ```
//! // A black square on a white 32 x 32 canvas, as raw RGBA8 pixels.
//! let (w, h) = (32u32, 32u32);
//! let mut pixels = vec![255u8; (w * h * 4) as usize];
//! for y in 8..24 {
//!     for x in 8..24 {
//!         let i = ((y * w + x) * 4) as usize;
//!         pixels[i..i + 3].copy_from_slice(&[0, 0, 0]);
//!     }
//! }
//!
//! let mut opts = inkvec::Options::default();
//! opts.colors = 8;
//! let traced = inkvec::trace_rgba(&pixels, w, h, &opts)?;
//! assert!(traced.svg.starts_with("<svg"));
//! assert_eq!((traced.width, traced.height), (32, 32));
//!
//! // The same call on the encoded file is `inkvec::trace(&std::fs::read("logo.png")?, &opts)`.
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Guarantees
//!
//! * **Deterministic.** The same input and options give byte-identical SVG, whatever the
//!   thread count, as long as `time_budget` is 0 (the default) and no `INKVEC_*` research
//!   environment variable is set -- on one build target. Another target can write the same
//!   drawing slightly differently (subpaths in another order, a last digit); see
//!   [`build_target`].
//! * **Thread-safe.** Every function may be called from any number of threads at once. The
//!   pipeline runs its own parallel work on rayon's global pool.
//! * **No panics escape.** A panic inside the pipeline is caught and returned as
//!   [`Error::Internal`].
//! * **No I/O.** Nothing is read from or written to disk, no process is started and nothing
//!   is printed, with the default options and no `INKVEC_*` variables set.
//!
//! # Not included
//!
//! The optional neural pre-passes of the command line -- the trained restorer (`--restore`)
//! and the super-resolution pre-pass (`--sr`) -- are not part of this API: they need model
//! weights, an ML runtime or an external process. Clean the image first if it needs it.
//!
//! # Transparency
//!
//! Traced natively by default, as on the command line: every ink is a colour and an
//! opacity and the transparent ground is an ink of its own, so holes stay holes and a fade
//! is one gradient of colour and opacity. An opaque input traces the same either way. Set
//! [`Options::native_alpha`] to `false` to composite onto a matte first, as releases up to
//! 0.1.3 did ([`Options::cutout`] then decides whether the transparency is put back).
//!
//! # Shape harmonization
//!
//! On by default, as on the command line. Marks that repeat across the drawing are redrawn
//! from one consensus geometry per cluster, which saves parameters. A mark takes the
//! consensus only where that stays within 0.1 px of the boundary traced for it and costs
//! fewer parameters. Set [`Options::harmonize`] to `false` to skip the pass.

mod options;

pub use options::{options_schema_json, Options};

/// A finished trace.
///
/// `#[non_exhaustive]`: fields may be added.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Traced {
    /// The SVG document, UTF-8.
    pub svg: String,
    /// Width of the input image, in pixels. The SVG's `width` attribute carries the same
    /// number unless [`Options::margin`] grew it. Its `viewBox` can be a smaller coordinate
    /// space when the input was reduced for tracing ([`Options::max_dim`], or an exact
    /// pixel-block upscale that was undone).
    pub width: u32,
    /// Height of the input image, in pixels; see [`Traced::width`].
    pub height: u32,
}

/// Why a trace failed.
///
/// `#[non_exhaustive]`: variants may be added.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The input is not an image this crate can decode, or raw pixels do not match the
    /// dimensions given.
    InvalidImage(String),
    /// An option is unknown, of the wrong type or out of range.
    InvalidOptions(String),
    /// The pipeline failed or panicked. Not the caller's fault; worth a bug report.
    Internal(String),
}

impl Error {
    /// A stable identifier for the kind of error: `"invalid_image"`, `"invalid_options"`
    /// or `"internal"`. The same strings name the error cases in the cross-language
    /// contract fixtures.
    pub fn code(&self) -> &'static str {
        match self {
            Error::InvalidImage(_) => "invalid_image",
            Error::InvalidOptions(_) => "invalid_options",
            Error::Internal(_) => "internal",
        }
    }

    /// The human-readable message, without the kind.
    pub fn message(&self) -> &str {
        match self {
            Error::InvalidImage(m) | Error::InvalidOptions(m) | Error::Internal(m) => m,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Error::InvalidImage(_) => "invalid image",
            Error::InvalidOptions(_) => "invalid options",
            Error::Internal(_) => "internal error",
        };
        write!(f, "{kind}: {}", self.message())
    }
}

impl std::error::Error for Error {}

/// Trace an encoded image: PNG, JPEG, WebP, GIF, BMP or TIFF.
///
/// The container is read as well as the pixels: JPEG and lossy WebP are traced with the
/// noise-aware intake the command line uses for them. Transparency is honoured as the
/// command line does (see [`Options::native_alpha`]).
pub fn trace(image: &[u8], opts: &Options) -> Result<Traced, Error> {
    opts.validate()?;
    guarded(|| {
        let args = opts.to_args();
        let (img, (w, h)) = inkvec_trace::decode_image_capped(image, args.max_dim)
            .map_err(|e| Error::InvalidImage(e.to_string()))?;
        // `lossy` is a question about the container; its first bytes answer it.
        let head = &image[..image.len().min(32)];
        let args = inkvec_cli::resolve_lossy(&args, || Some(head.to_vec()));
        run(img, &args, w, h)
    })
}

/// Trace raw pixels: straight (not premultiplied) RGBA, 8 bits per channel, row-major,
/// tightly packed -- exactly `width * height * 4` bytes.
///
/// The result is byte-identical to [`trace`] on a PNG holding the same pixels. There is no
/// container to read, so the intake is the one for lossless input.
pub fn trace_rgba(pixels: &[u8], width: u32, height: u32, opts: &Options) -> Result<Traced, Error> {
    // No container: resolve `lossy` the way an unrecognised file resolves it.
    trace_rgba_lossy(pixels, width, height, opts, None)
}

/// Trace pixels that came out of the restorer pre-pass (`inkvec-restore`), which are not the
/// same thing as ordinary lossless pixels.
///
/// Restoration and the tracer's soft intake -- measured noise, a relaxed same-ink merge --
/// travel together: restored output can look clean enough that the automatic
/// edge-width/ringing detector no longer opens soft intake on its own, so a caller that
/// restores must force it. That is what `inkvec-cli` does after `--restore`, and this is
/// [`trace_rgba`] with the same forcing, for a caller whose restorer ran somewhere the
/// command line cannot reach -- a browser's ONNX Runtime Web session, say. Handing restored
/// pixels to [`trace_rgba`] instead traces them in a configuration the restorer was never
/// validated in.
pub fn trace_rgba_restored(
    pixels: &[u8],
    width: u32,
    height: u32,
    opts: &Options,
) -> Result<Traced, Error> {
    trace_rgba_lossy(pixels, width, height, opts, Some(inkvec_sr::Mode::On))
}

/// [`trace_rgba`] with the intake decided by the caller: `None` resolves it the way an
/// unrecognised file resolves it, `Some(mode)` sets it outright.
fn trace_rgba_lossy(
    pixels: &[u8],
    width: u32,
    height: u32,
    opts: &Options,
    lossy: Option<inkvec_sr::Mode>,
) -> Result<Traced, Error> {
    opts.validate()?;
    if width == 0 || height == 0 {
        return Err(Error::InvalidImage(format!(
            "an image needs a nonzero size, got {width}x{height}"
        )));
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| Error::InvalidImage(format!("{width}x{height} is too large")))?;
    if pixels.len() != expected {
        return Err(Error::InvalidImage(format!(
            "{width}x{height} RGBA8 is {expected} bytes, got {}",
            pixels.len()
        )));
    }
    guarded(|| {
        let args = opts.to_args();
        let img = inkvec_trace::rgba8_capped(pixels, width, height, args.max_dim);
        let args = match lossy {
            Some(mode) => inkvec_cli::Args {
                lossy: mode,
                ..args
            },
            None => inkvec_cli::resolve_lossy(&args, || None),
        };
        run(img, &args, width, height)
    })
}

/// The crate version, which is also the version of every binding built from it.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The target this library was compiled for, as `arch-os-env` (`x86_64-windows-msvc`,
/// `x86_64-linux-gnu`, `aarch64-macos`, ...; `wasm32-unknown` for the browser build and
/// `wasm32-wasip1` for the WASI one the Go package runs).
///
/// Output is byte-identical between two builds only when this string is the same. The
/// pipeline takes a few transcendental functions (cube roots, trigonometry, logarithms) from
/// the platform's maths library, whose last-bit rounding differs between, say, glibc and the
/// MSVC runtime, and a last-bit difference can tip a near-tie downstream: measured on the
/// 96-px contract sample, Linux and Windows write the same shapes with the subpaths of one
/// compound path in a different order. Within one build target the output is deterministic;
/// the contract fixtures record one SVG hash per target.
pub fn build_target() -> &'static str {
    static TARGET: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TARGET.get_or_init(|| {
        let os = match std::env::consts::OS {
            // Empty on every WebAssembly target. WASI is named: its maths library is
            // wasi-libc's, not the browser build's, and the output differs.
            "" if cfg!(target_os = "wasi") => "wasi",
            "" => "unknown",
            os => os,
        };
        let env = if cfg!(target_env = "msvc") {
            "-msvc"
        } else if cfg!(target_env = "gnu") {
            "-gnu"
        } else if cfg!(target_env = "musl") {
            "-musl"
        } else if cfg!(target_env = "p1") {
            "p1" // wasm32-wasip1, as the Rust target is spelled
        } else if cfg!(target_env = "p2") {
            "p2"
        } else {
            ""
        };
        format!("{}-{os}{env}", std::env::consts::ARCH)
    })
}

/// The pipeline and the output options, on a raster already decoded and capped.
fn run(img: inkvec_trace::Rgba, args: &inkvec_cli::Args, w: u32, h: u32) -> Result<Traced, Error> {
    let t = inkvec_cli::trace_image_sized(img, args, Some((w as usize, h as usize)))
        .map_err(|e| Error::Internal(e.to_string()))?;
    let svg = inkvec_cli::post_process(args, t.svg, t.width, t.height);
    Ok(Traced {
        svg,
        width: w,
        height: h,
    })
}

/// Run `f`, turning a panic into [`Error::Internal`].
fn guarded<T>(f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            Err(Error::Internal(format!("panic inside the tracer: {msg}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_name_their_kind_in_code_and_text() {
        let cases = [
            (
                Error::InvalidImage("x".into()),
                "invalid_image",
                "invalid image: x",
            ),
            (
                Error::InvalidOptions("y".into()),
                "invalid_options",
                "invalid options: y",
            ),
            (Error::Internal("z".into()), "internal", "internal error: z"),
        ];
        for (e, code, text) in cases {
            assert_eq!(e.code(), code);
            assert_eq!(e.to_string(), text);
            assert_eq!(e.message(), &text[text.len() - 1..]);
        }
    }

    #[test]
    fn a_panic_becomes_an_internal_error() {
        let r: Result<(), Error> = guarded(|| panic!("boom"));
        assert!(matches!(r, Err(Error::Internal(m)) if m.contains("boom")));
        assert_eq!(guarded(|| Ok(7)), Ok(7));
    }

    #[test]
    fn the_build_target_names_arch_and_os() {
        let t = build_target();
        assert!(t.starts_with(std::env::consts::ARCH), "{t}");
        assert!(t.split('-').count() >= 2, "{t}");
        assert!(std::ptr::eq(t, build_target()), "computed once");
    }
}
