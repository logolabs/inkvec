//! Inkvec's C ABI.
//!
//! A thin layer over the `inkvec` crate for every language that can call C: C and C++
//! directly, and Java (JNA / Panama), C# (P/Invoke), Go (cgo), Swift, Ruby, PHP and others
//! through their foreign-function interfaces. The header is `include/inkvec.h`, generated
//! from this file by cbindgen; the library is `inkvec_ffi` (`inkvec_ffi.dll`,
//! `libinkvec_ffi.so`, `libinkvec_ffi.dylib`, and the static `inkvec_ffi.lib` /
//! `libinkvec_ffi.a`).
//!
//! # Design
//!
//! * **Options are JSON.** Every trace takes its options as a UTF-8 JSON object (NULL or
//!   `""` for the defaults), validated in Rust against the same schema the other bindings
//!   are generated from (`inkvec_options_schema`). Adding an option to Inkvec therefore
//!   never changes this header or any binding built on it.
//! * **Plain C types, explicit sizes.** No Rust types cross the boundary: `uint8_t`,
//!   `size_t`, `uint32_t`, `int32_t` and NUL-terminated UTF-8 strings.
//! * **One allocation owner.** Strings in an [`InkvecResult`] are allocated by the library
//!   and released only by [`inkvec_result_free`]. Strings returned by `inkvec_version`,
//!   `inkvec_build_target`, `inkvec_options_schema` and `inkvec_default_options` are static and
//!   must not be freed.
//! * **Forward compatible.** The caller records `sizeof(InkvecResult)` in `struct_size`
//!   before each call (`INKVEC_RESULT_INIT` does it), so a later library can append fields
//!   without writing past an older caller's struct.
//! * **Never unwinds.** Every entry point catches panics; a panic becomes
//!   `INKVEC_ERR_INTERNAL`.
//! * **Thread-safe.** Every function may be called from any number of threads at once, with
//!   a separate `InkvecResult` per call.

use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::OnceLock;

use inkvec as facade;

/// Success.
pub const INKVEC_OK: i32 = 0;
/// A required pointer was NULL, or `out->struct_size` is smaller than this library's
/// `InkvecResult`. Nothing is written to `out` in the second case.
pub const INKVEC_ERR_INVALID_ARGUMENT: i32 = 1;
/// The bytes are not an image Inkvec can decode, or the RGBA buffer does not match the
/// dimensions given.
pub const INKVEC_ERR_INVALID_IMAGE: i32 = 2;
/// The options JSON is malformed, names an unknown option, or holds a value of the wrong
/// type or out of range.
pub const INKVEC_ERR_INVALID_OPTIONS: i32 = 3;
/// The tracer failed or panicked. Not the caller's fault.
pub const INKVEC_ERR_INTERNAL: i32 = 4;

/// Version of this C ABI: incremented only by a change that breaks existing callers.
/// Appending a field to `InkvecResult` or adding a function does not change it.
pub const INKVEC_ABI_VERSION: u32 = 1;

/// The outcome of one trace.
///
/// Before each call, zero-initialise it and set `struct_size` to `sizeof(InkvecResult)`
/// (`InkvecResult r = INKVEC_RESULT_INIT;`). After the call, release it with
/// `inkvec_result_free`, whether the call succeeded or not.
#[repr(C)]
#[derive(Debug)]
pub struct InkvecResult {
    /// Set by the caller: `sizeof(InkvecResult)` as the caller compiled it.
    pub struct_size: u32,
    /// `INKVEC_OK` or one of the `INKVEC_ERR_*` codes; the same value the call returned.
    pub status: i32,
    /// The SVG document, NUL-terminated UTF-8, owned by the library. NULL on error.
    pub svg: *mut c_char,
    /// Length of `svg` in bytes, not counting the terminating NUL. 0 on error.
    pub svg_len: usize,
    /// Width of the input image in pixels (the SVG's `width` unless a margin grew it).
    pub width: u32,
    /// Height of the input image in pixels.
    pub height: u32,
    /// Why the call failed, NUL-terminated UTF-8, owned by the library. NULL on success.
    pub error: *mut c_char,
}

/// Trace an encoded image (PNG, JPEG, WebP, GIF, BMP or TIFF) to SVG.
///
/// `options_json` is a JSON object of options, NULL, or `""`; missing options take their
/// defaults (`inkvec_default_options`). Returns the status also stored in `out->status`.
///
/// # Safety
///
/// `bytes` must point to `len` readable bytes. `options_json` must be NULL or a
/// NUL-terminated string. `out` must point to a writable `InkvecResult` whose `struct_size`
/// is set.
#[no_mangle]
pub unsafe extern "C" fn inkvec_trace(
    bytes: *const u8,
    len: usize,
    options_json: *const c_char,
    out: *mut InkvecResult,
) -> i32 {
    entry(out, || {
        if bytes.is_null() {
            return Err(Failure::argument("bytes is NULL"));
        }
        // SAFETY: the caller promises `len` readable bytes at a non-NULL `bytes`.
        let image = unsafe { std::slice::from_raw_parts(bytes, len) };
        // SAFETY: forwarded from the caller's promise about `options_json`.
        let opts = unsafe { parse_options(options_json) }?;
        facade::trace(image, &opts).map_err(Failure::from)
    })
}

/// Trace raw pixels to SVG: straight (not premultiplied) RGBA, 8 bits per channel,
/// row-major, tightly packed. `len` must be exactly `width * height * 4`.
///
/// The SVG is byte-identical to `inkvec_trace` on a PNG holding the same pixels.
///
/// # Safety
///
/// `rgba` must point to `len` readable bytes. `options_json` must be NULL or a
/// NUL-terminated string. `out` must point to a writable `InkvecResult` whose `struct_size`
/// is set.
#[no_mangle]
pub unsafe extern "C" fn inkvec_trace_rgba(
    rgba: *const u8,
    len: usize,
    width: u32,
    height: u32,
    options_json: *const c_char,
    out: *mut InkvecResult,
) -> i32 {
    entry(out, || {
        if rgba.is_null() {
            return Err(Failure::argument("rgba is NULL"));
        }
        // SAFETY: the caller promises `len` readable bytes at a non-NULL `rgba`.
        let pixels = unsafe { std::slice::from_raw_parts(rgba, len) };
        // SAFETY: forwarded from the caller's promise about `options_json`.
        let opts = unsafe { parse_options(options_json) }?;
        facade::trace_rgba(pixels, width, height, &opts).map_err(Failure::from)
    })
}

/// Release the strings a trace stored in `result` and reset them to NULL. Safe to call on
/// NULL, on a result that holds no strings, and more than once.
///
/// # Safety
///
/// `result` must be NULL or point to an `InkvecResult` last filled by `inkvec_trace` or
/// `inkvec_trace_rgba` (or zero-initialised), whose strings have not been freed any other way.
#[no_mangle]
pub unsafe extern "C" fn inkvec_result_free(result: *mut InkvecResult) {
    if result.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: non-NULL and, per the contract, a result this library filled.
        let r = unsafe { &mut *result };
        for p in [&mut r.svg, &mut r.error] {
            if !p.is_null() {
                // SAFETY: produced by `CString::into_raw` in `fill` and not freed since.
                drop(unsafe { CString::from_raw(*p) });
                *p = std::ptr::null_mut();
            }
        }
        r.svg_len = 0;
    }));
}

/// The library version, e.g. `"0.1.3"`. Static; do not free.
#[no_mangle]
pub extern "C" fn inkvec_version() -> *const c_char {
    static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION.as_ptr().cast()
}

/// The target this library was compiled for, e.g. `"x86_64-linux-gnu"`. Output is
/// byte-identical between builds with the same target; another target can write the same
/// drawing slightly differently. Static; do not free.
#[no_mangle]
pub extern "C" fn inkvec_build_target() -> *const c_char {
    static TARGET: OnceLock<CString> = OnceLock::new();
    TARGET
        .get_or_init(|| c_string(facade::build_target()))
        .as_ptr()
}

/// `INKVEC_ABI_VERSION` as this library was built, for a binding to check at load time.
#[no_mangle]
pub extern "C" fn inkvec_abi_version() -> u32 {
    INKVEC_ABI_VERSION
}

/// The JSON Schema (draft 2020-12) of the options object: every option's name, type,
/// default, description and range. Static; do not free.
#[no_mangle]
pub extern "C" fn inkvec_options_schema() -> *const c_char {
    static SCHEMA: OnceLock<CString> = OnceLock::new();
    SCHEMA
        .get_or_init(|| c_string(facade::options_schema_json()))
        .as_ptr()
}

/// Every option at its default, as a compact JSON object. Static; do not free.
#[no_mangle]
pub extern "C" fn inkvec_default_options() -> *const c_char {
    static DEFAULTS: OnceLock<CString> = OnceLock::new();
    DEFAULTS
        .get_or_init(|| c_string(&facade::Options::default().to_json()))
        .as_ptr()
}

/// A failed call: its status code and message.
struct Failure {
    status: i32,
    message: String,
}

impl Failure {
    fn argument(message: &str) -> Self {
        Self {
            status: INKVEC_ERR_INVALID_ARGUMENT,
            message: message.to_string(),
        }
    }
}

impl From<facade::Error> for Failure {
    fn from(e: facade::Error) -> Self {
        let status = match e {
            facade::Error::InvalidImage(_) => INKVEC_ERR_INVALID_IMAGE,
            facade::Error::InvalidOptions(_) => INKVEC_ERR_INVALID_OPTIONS,
            _ => INKVEC_ERR_INTERNAL,
        };
        Self {
            status,
            message: e.to_string(),
        }
    }
}

/// The options behind a C string: defaults for NULL or blank, otherwise parsed and validated.
///
/// # Safety
///
/// `json` is NULL or a NUL-terminated string.
unsafe fn parse_options(json: *const c_char) -> Result<facade::Options, Failure> {
    if json.is_null() {
        return Ok(facade::Options::default());
    }
    // SAFETY: non-NULL and NUL-terminated, per the caller.
    let text = unsafe { CStr::from_ptr(json) }.to_str().map_err(|_| {
        Failure::from(facade::Error::InvalidOptions(
            "options are not UTF-8".into(),
        ))
    })?;
    facade::Options::from_json(text).map_err(Failure::from)
}

/// Every trace entry point: check `out`, run `work` with panics caught, and fill `out`.
fn entry(out: *mut InkvecResult, work: impl FnOnce() -> Result<facade::Traced, Failure>) -> i32 {
    if out.is_null() {
        return INKVEC_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: `out` is non-NULL and, per the contract, points to at least a `u32`.
    let caller_size = unsafe { std::ptr::addr_of!((*out).struct_size).read_unaligned() };
    if (caller_size as usize) < size_of::<InkvecResult>() {
        return INKVEC_ERR_INVALID_ARGUMENT;
    }
    let outcome = catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| {
        Err(Failure {
            status: INKVEC_ERR_INTERNAL,
            message: "internal error: panic in the C binding".into(),
        })
    });
    // SAFETY: `out` is writable and at least as large as this library's struct.
    let out = unsafe { &mut *out };
    fill(out, outcome)
}

/// Store an outcome in the caller's result and return its status.
fn fill(out: &mut InkvecResult, outcome: Result<facade::Traced, Failure>) -> i32 {
    out.svg = std::ptr::null_mut();
    out.svg_len = 0;
    out.width = 0;
    out.height = 0;
    out.error = std::ptr::null_mut();
    out.status = match outcome {
        Ok(t) => {
            out.svg_len = t.svg.len();
            out.svg = c_string(&t.svg).into_raw();
            out.width = t.width;
            out.height = t.height;
            INKVEC_OK
        }
        Err(f) => {
            out.error = c_string(&f.message).into_raw();
            f.status
        }
    };
    out.status
}

/// A C string from Rust text. An interior NUL -- which SVG and these messages never
/// contain -- is dropped rather than truncating the string or failing.
fn c_string(s: &str) -> CString {
    CString::new(s).unwrap_or_else(|_| CString::new(s.replace('\0', "")).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    //! The C ABI exercised the way a foreign caller uses it: raw pointers, C strings, a
    //! caller-owned result, and the shared contract cases.

    use super::*;
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;
    use std::ptr::{null, null_mut};

    fn fresh() -> InkvecResult {
        InkvecResult {
            struct_size: size_of::<InkvecResult>() as u32,
            status: -1,
            svg: null_mut(),
            svg_len: 0,
            width: 0,
            height: 0,
            error: null_mut(),
        }
    }

    fn text(p: *const c_char) -> String {
        assert!(!p.is_null());
        // SAFETY: every string the library returns is NUL-terminated.
        unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string()
    }

    fn contract_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bindings/contract")
    }

    /// One contract case through `inkvec_trace` / `inkvec_trace_rgba`: its `expect` block
    /// and, on success, the SVG.
    fn run_case(case: &Value) -> (Value, Option<String>) {
        let input = std::fs::read(contract_dir().join(case["input"].as_str().unwrap())).unwrap();
        let options = CString::new(case["options"].to_string()).unwrap();
        let mut r = fresh();
        // SAFETY: valid buffers and a fresh result, as a C caller would pass them.
        let status = unsafe {
            match case["form"].as_str().unwrap() {
                "encoded" => inkvec_trace(input.as_ptr(), input.len(), options.as_ptr(), &mut r),
                _ => inkvec_trace_rgba(
                    input.as_ptr(),
                    input.len(),
                    case["width"].as_u64().unwrap() as u32,
                    case["height"].as_u64().unwrap() as u32,
                    options.as_ptr(),
                    &mut r,
                ),
            }
        };
        assert_eq!(status, r.status);
        let got = if status == INKVEC_OK {
            assert!(r.error.is_null());
            let svg = text(r.svg);
            assert_eq!(svg.len(), r.svg_len);
            let expect = serde_json::json!({ "width": r.width, "height": r.height });
            (expect, Some(svg))
        } else {
            assert!(r.svg.is_null() && r.svg_len == 0);
            assert!(!text(r.error).is_empty());
            let kind = match status {
                INKVEC_ERR_INVALID_IMAGE => "invalid_image",
                INKVEC_ERR_INVALID_OPTIONS => "invalid_options",
                INKVEC_ERR_INTERNAL => "internal",
                other => panic!("unexpected status {other}"),
            };
            (serde_json::json!({ "error": kind }), None)
        };
        // SAFETY: filled by the call above.
        unsafe { inkvec_result_free(&mut r) };
        got
    }

    #[test]
    fn the_c_abi_reproduces_the_contract() {
        let text = std::fs::read_to_string(contract_dir().join("cases.json")).unwrap();
        let doc: Value = serde_json::from_str(&text).unwrap();
        let target = facade::build_target();
        let mut svgs = std::collections::HashMap::new();
        for case in doc["cases"].as_array().unwrap() {
            let (expect, svg) = run_case(case);
            assert_eq!(expect, case["expect"], "case {}", case["name"]);
            if let (Some(svg), Some(want)) = (&svg, case["svg"].get(target)) {
                let hash: String = Sha256::digest(svg.as_bytes())
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                assert_eq!(
                    want["sha256"],
                    hash.as_str(),
                    "case {} on {target}",
                    case["name"]
                );
                assert_eq!(
                    want["bytes"],
                    svg.len(),
                    "case {} on {target}",
                    case["name"]
                );
            }
            svgs.insert(case["name"].as_str().unwrap().to_string(), svg);
        }
        for case in doc["cases"].as_array().unwrap() {
            if let Some(other) = case["same_svg_as"].as_str() {
                let name = case["name"].as_str().unwrap();
                assert!(
                    svgs[name].is_some() && svgs[name] == svgs[other],
                    "{name} vs {other}"
                );
            }
        }
    }

    #[test]
    fn null_and_blank_options_mean_defaults() {
        let png = std::fs::read(contract_dir().join("tiny.png")).unwrap();
        let mut a = fresh();
        let mut b = fresh();
        let blank = CString::new("  ").unwrap();
        // SAFETY: valid buffers and fresh results.
        unsafe {
            assert_eq!(
                inkvec_trace(png.as_ptr(), png.len(), null(), &mut a),
                INKVEC_OK
            );
            assert_eq!(
                inkvec_trace(png.as_ptr(), png.len(), blank.as_ptr(), &mut b),
                INKVEC_OK
            );
            assert_eq!(text(a.svg), text(b.svg));
            inkvec_result_free(&mut a);
            inkvec_result_free(&mut b);
        }
    }

    #[test]
    fn bad_arguments_are_refused() {
        let mut r = fresh();
        // SAFETY: exercising the NULL checks; nothing is dereferenced.
        unsafe {
            assert_eq!(
                inkvec_trace(null(), 0, null(), &mut r),
                INKVEC_ERR_INVALID_ARGUMENT
            );
            assert!(text(r.error).contains("bytes"));
            inkvec_result_free(&mut r);
            assert_eq!(
                inkvec_trace(b"x".as_ptr(), 1, null(), null_mut()),
                INKVEC_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                inkvec_trace_rgba(null(), 0, 1, 1, null(), &mut r),
                INKVEC_ERR_INVALID_ARGUMENT
            );
            inkvec_result_free(&mut r);
        }
        // A result smaller than this library's is left untouched.
        let mut small = fresh();
        small.struct_size = 8;
        small.status = 77;
        // SAFETY: `small` is a full struct; only its recorded size is too small.
        let s = unsafe { inkvec_trace(b"x".as_ptr(), 1, null(), &mut small) };
        assert_eq!(s, INKVEC_ERR_INVALID_ARGUMENT);
        assert_eq!(small.status, 77);
        assert!(small.error.is_null());
    }

    #[test]
    fn option_errors_carry_the_field_name() {
        let png = std::fs::read(contract_dir().join("tiny.png")).unwrap();
        for (json, needle) in [
            (r#"{"colours": 8}"#, "colours"),
            (r#"{"colors": 0}"#, "colors"),
            ("{not json", "key"),
        ] {
            let opts = CString::new(json).unwrap();
            let mut r = fresh();
            // SAFETY: valid buffers and a fresh result.
            let s = unsafe { inkvec_trace(png.as_ptr(), png.len(), opts.as_ptr(), &mut r) };
            assert_eq!(s, INKVEC_ERR_INVALID_OPTIONS, "{json}");
            let msg = text(r.error);
            assert!(
                msg.starts_with("invalid options: ") && msg.contains(needle),
                "{json}: {msg}"
            );
            // SAFETY: filled above.
            unsafe { inkvec_result_free(&mut r) };
        }
        let invalid_utf8 = [0xffu8, 0xfe, 0];
        let mut r = fresh();
        // SAFETY: a NUL-terminated byte string, as C would pass it.
        let s = unsafe {
            inkvec_trace(
                png.as_ptr(),
                png.len(),
                invalid_utf8.as_ptr().cast(),
                &mut r,
            )
        };
        assert_eq!(s, INKVEC_ERR_INVALID_OPTIONS);
        // SAFETY: filled above.
        unsafe { inkvec_result_free(&mut r) };
    }

    #[test]
    fn freeing_is_safe_on_null_and_twice() {
        let png = std::fs::read(contract_dir().join("tiny.png")).unwrap();
        let mut r = fresh();
        // SAFETY: valid buffers; freeing NULL, a filled result, and the same result again.
        unsafe {
            inkvec_result_free(null_mut());
            assert_eq!(
                inkvec_trace(png.as_ptr(), png.len(), null(), &mut r),
                INKVEC_OK
            );
            inkvec_result_free(&mut r);
            assert!(r.svg.is_null() && r.svg_len == 0);
            inkvec_result_free(&mut r);
        }
    }

    #[test]
    fn static_strings_describe_the_library() {
        assert_eq!(text(inkvec_version()), env!("CARGO_PKG_VERSION"));
        assert_eq!(text(inkvec_build_target()), facade::build_target());
        assert_eq!(inkvec_abi_version(), INKVEC_ABI_VERSION);
        let schema: Value = serde_json::from_str(&text(inkvec_options_schema())).unwrap();
        assert_eq!(schema["title"], "InkvecOptions");
        let defaults: Value = serde_json::from_str(&text(inkvec_default_options())).unwrap();
        for (name, prop) in schema["properties"].as_object().unwrap() {
            assert_eq!(defaults[name], prop["default"], "{name}");
        }
        // Static: the same pointer every time.
        assert_eq!(inkvec_options_schema(), inkvec_options_schema());
    }

    #[test]
    fn the_committed_header_declares_this_abi() {
        // A cheap check that needs no cbindgen: every exported function, constant and field
        // appears in include/inkvec.h. CI runs `cbindgen --verify` for the exact text.
        let header = include_str!("../include/inkvec.h");
        let src = include_str!("lib.rs");
        let hint = "include/inkvec.h is stale; regenerate it with \
                    `cbindgen --config crates/inkvec-ffi/cbindgen.toml --crate inkvec-ffi \
                    --output crates/inkvec-ffi/include/inkvec.h`";
        let names = src
            .split("pub unsafe extern \"C\" fn ")
            .skip(1)
            .chain(src.split("pub extern \"C\" fn ").skip(1))
            .map(|s| s.split('(').next().unwrap().to_string())
            .chain(
                src.split("pub const ")
                    .skip(1)
                    .map(|s| s.split(':').next().unwrap().to_string()),
            )
            .chain(
                [
                    "struct_size",
                    "status",
                    "svg",
                    "svg_len",
                    "width",
                    "height",
                    "error",
                ]
                .iter()
                .map(|s| s.to_string()),
            )
            // This test's own string literals split too; keep only real identifiers.
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        for name in names {
            assert!(header.contains(&name), "{name} missing: {hint}");
        }
    }
}
