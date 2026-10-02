//! Intake: reading a raster into straight RGBA floats, optionally capped in size, and
//! asking the container whether a lossy codec wrote it.
//!
//! Every entry point returns an [`Rgba`] with channels in `[0, 1]` (8-bit samples divided
//! by 255, not premultiplied), which is what the tracer takes. Re-exported at the crate
//! root.
//!
//! # The passes, for a file
//!
//! 1. **Read** the file into memory once ([`load_image_capped`]); the header and the pixels
//!    are both decoded from those bytes, in the format the bytes themselves announce (their
//!    signature), and only when the bytes announce none, the one the file's extension names
//!    ([`sniff`]).
//! 2. **Decode** with the `image` crate into whatever layout the file holds (8-bit RGB for
//!    most opaque PNGs and every JPEG, 8-bit RGBA for most transparent ones), with a larger
//!    allocation allowance when the cap will apply ([`capped_decode_limits`]).
//! 3. **Turn** the image upright by its EXIF orientation ([`decode_upright`]), so a phone
//!    photo is traced as it is shown; the dimensions reported from here on are upright.
//! 4. **Cap**: above `max_dim` on the longer side, box-average the 8-bit buffer down
//!    (`coverage::box_downsample_rgba8`) before any float exists.
//! 5. **Widen** to floats ([`from_dynamic`]): one table lookup per byte ([`UNIT`]), straight
//!    from the decoder's own buffer for 8-bit RGB and RGBA, in parallel chunks on a large
//!    image. The result is the same float the old per-byte division gave.
//!
//! At 2048 px the intake was 20 ms (decode 4.9, the RGBA copy 4.0, the float conversion
//! 10.1), all serial; the copy is gone for RGB and RGBA files and the conversion is split
//! over the cores.

use std::path::Path;

use crate::coverage::{self, Rgba};

/// Error loading or decoding a raster image.
#[derive(Debug)]
pub enum TraceError {
    /// The file could not be read.
    Io(String),
    /// The bytes could not be decoded as a supported image format.
    Decode(String),
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceError::Io(m) => write!(f, "io error: {m}"),
            TraceError::Decode(m) => write!(f, "decode error: {m}"),
        }
    }
}

impl std::error::Error for TraceError {}

/// Was this file written by a lossy codec?
///
/// This is a fact about the container, not a statistic about the pixels, and that is the
/// whole reason it exists. A lossy codec reconstructs flat regions with ringing and
/// blocking, so an image that came out of one carries colours near every edge that no
/// designer ever chose. The palette has a guard for exactly that (`color::SOFT_NOISE_SIGMAS`),
/// but it was switched only by `coverage::intake_scale` -- the *edge width* -- which sees
/// resampling and blur and is blind to compression: JPEG rings flat areas without widening
/// an edge, so a q50 logo measured 1.15 px, under the 1.75 px threshold, and the guard
/// stayed off while the palette took 200-odd ringing colours for inks.
///
/// Two pixel-level detectors were tried first and both failed, for the same reason the
/// 8x8 block signature failed before them: the Laplacian of a lossily-coded flat region and
/// the Laplacian of a cleanly-rendered 8-bit colour ramp are the same size. A clean radial
/// gradient measured a *higher* "damage" score than a q50 flat icon. There is no separating
/// the two from the pixels alone, so this asks the file instead, where the answer is exact.
///
/// Returns `None` when the format cannot be determined; the caller treats that as "not
/// known to be lossy", because switching the guard on costs quality on a clean intake.
pub fn lossy_container(bytes: &[u8]) -> Option<bool> {
    match image::guess_format(bytes).ok()? {
        image::ImageFormat::Jpeg => Some(true),
        // WebP is both codecs in one container. The RIFF chunk after the 12-byte header
        // says which: `VP8 ` (with the trailing space) is lossy, `VP8L` is lossless, and
        // `VP8X` is the extended form whose sub-chunks have to be walked to find out --
        // treat that last one as unknown rather than guessing.
        image::ImageFormat::WebP => match bytes.get(12..16)? {
            b"VP8 " => Some(true),
            b"VP8L" => Some(false),
            _ => None,
        },
        // Everything else the tracer accepts stores exact samples.
        image::ImageFormat::Png
        | image::ImageFormat::Gif
        | image::ImageFormat::Bmp
        | image::ImageFormat::Tiff => Some(false),
        _ => None,
    }
}

/// Load a raster image from a file path into straight RGBA floats: [`load_image_capped`]
/// without a cap.
pub fn load_image(path: &Path) -> Result<Rgba, TraceError> {
    load_image_capped(path, 0).map(|(img, _)| img)
}

/// Decode a raster image from in-memory bytes into straight RGBA floats:
/// [`decode_image_capped`] without a cap.
pub fn decode_image(bytes: &[u8]) -> Result<Rgba, TraceError> {
    decode_image_capped(bytes, 0).map(|(img, _)| img)
}

/// The format to decode `bytes` as: the one their signature announces, when it is a
/// format the tracer reads; otherwise the one `path`'s extension names; `None` when
/// neither says.
///
/// The extension used to decide alone (as `image::open` decides), so a JPEG saved as
/// `.png` failed with "Invalid PNG signature" and a PNG saved as `.jpg` with "Illegal start
/// bytes" (intake fuzz, 2026-10-02) -- a mislabelled file is common (a browser's "save
/// image as", a rename) and the bytes say plainly what it is. A signature names a format
/// unambiguously (`\x89PNG`, `\xFF\xD8\xFF`, `GIF8`, `RIFF....WEBP`, `BM`, `II*\0` /
/// `MM\0*`), so where it is present it wins; where it is absent (a truncated or damaged
/// file) the extension still chooses the decoder, whose error then describes the damage.
/// A file whose extension is right decodes with the same decoder as before, so its pixels
/// are unchanged.
///
/// Method from: magic-number content sniffing, the rule of the WHATWG MIME Sniffing
/// Standard (§ 6.1, "Matching an image type pattern",
/// <https://mimesniff.spec.whatwg.org/#matching-an-image-type-pattern>), by which
/// browsers decide an image's type from its first bytes, not its name; the patterns are
/// the `image` crate's `guess_format`.
fn sniff(bytes: &[u8], path: Option<&Path>) -> Option<image::ImageFormat> {
    use image::ImageFormat as F;
    let read = |f: &F| matches!(f, F::Png | F::Jpeg | F::Gif | F::WebP | F::Bmp | F::Tiff);
    image::guess_format(bytes).ok().filter(read).or_else(|| {
        path.and_then(Path::extension)
            .and_then(image::ImageFormat::from_extension)
    })
}

/// Decode `bytes` as `format`, capped at `max_dim` (see [`load_image_capped`]), with the
/// arrival dimensions.
fn decode_as(
    bytes: &[u8],
    format: image::ImageFormat,
    max_dim: usize,
) -> Result<(Rgba, (u32, u32)), TraceError> {
    let err = |e: image::ImageError| TraceError::Decode(e.to_string());
    let reader = || {
        let mut r = image::ImageReader::new(std::io::Cursor::new(bytes));
        r.set_format(format);
        r
    };
    let (w, h) = reader().into_dimensions().map_err(err)?;
    has_pixels(w, h)?;
    let limits = if target_dims(w, h, max_dim).is_some() {
        capped_decode_limits()
    } else {
        image::Limits::default()
    };
    let img = decode_upright(reader(), limits).map_err(err)?;
    // Upright: a quarter turn swaps the sides, and the arrival size is the upright one.
    let (w, h) = (img.width(), img.height());
    Ok((cap_decoded(img, w, h, max_dim), (w, h)))
}

/// `ImageReader::decode` under `limits`, then turned the way the file says it is to be
/// shown: its EXIF orientation applied.
///
/// A camera stores the sensor's rows as they came and records how the picture is to be
/// turned in the EXIF Orientation tag (274, values 1-8: identity, the two mirrors, the
/// three rotations and the two transposes); every viewer applies it, so the image a person
/// sees and drops on the tracer is the turned one. `decode` does not apply it, and six
/// JPEGs stored with Orientation = 6 traced sideways (dE00 2.9 to 17.6 against 0.03 to 0.42
/// for the same images without the tag, r2-inputs `formats_test.py`, 2026-10-02).
///
/// The decode is `ImageReader::decode` taken apart only to read the orientation in between:
/// the same decoder, built with the same limits, whose output buffer is reserved from them
/// before the pixels are read, exactly as `decode` does. The orientation comes from the
/// decoder (`ImageDecoder::orientation`: the EXIF of a JPEG, WebP or PNG `eXIf` chunk, the
/// TIFF tag), and an unreadable one counts as no orientation rather than failing a decode
/// whose pixels are fine. An image without the tag, or with value 1, is returned as decoded.
///
/// Method from: the Orientation tag (274) of the Exif standard (CIPA DC-008), as the
/// `image` crate implements it: `image::metadata::Orientation` (docs.rs, image 0.25.10:
/// "Rotate90: rotate by 90 degrees clockwise", and so on for the eight values) and
/// `DynamicImage::apply_orientation`. Applied to the decoded 8-bit image, before the cap, so
/// the cap and everything after it see the image as it is shown.
fn decode_upright(
    reader: image::ImageReader<std::io::Cursor<&[u8]>>,
    limits: image::Limits,
) -> image::ImageResult<image::DynamicImage> {
    use image::ImageDecoder;
    let mut reader = reader;
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut limits = limits;
    limits.reserve(decoder.total_bytes())?;
    decoder.set_limits(limits)?;
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// The `image` crate's default allocation limit, which every uncapped decode keeps.
const DEFAULT_MAX_ALLOC: u64 = 512 << 20;

/// What a decode that is going to be capped may allocate beyond the default allowance
/// (see [`capped_decode_limits`]): 768 MiB on a 64-bit target, for 1.25 GiB in all, which
/// holds a 16384 x 16384 RGBA image at 8 bits (1 GiB) or 12000 x 12000 at 16 (1.15 GB) and
/// still refuses a 20000 x 20000 RGBA claim (1.6 GB, the decompression bomb of the intake
/// fuzz); nothing on a 32-bit target (WebAssembly), whose whole address space is 4 GiB.
#[cfg(target_pointer_width = "64")]
const CAPPED_EXTRA_ALLOC: u64 = 768 << 20;
/// See the 64-bit definition.
#[cfg(not(target_pointer_width = "64"))]
const CAPPED_EXTRA_ALLOC: u64 = 0;

/// Decoder limits for an image whose longer side is over the `--max-dim` cap.
///
/// The `image` crate refuses any decode whose output buffer would pass 512 MiB, which is
/// its guard against decompression bombs (a few kilobytes of deflate claiming 20000 x 20000
/// pixels), and that refused legitimate files too: a 12000 x 12000 RGBA PNG is 576 MB
/// decoded, so it failed with "Memory limit exceeded" (intake fuzz, 2026-10-02), although
/// the trace would be capped at 2048 px and the full-size buffer lives only until the box
/// filter has read it. A 16-bit 8192 x 8192 RGBA PNG (exactly 512 MiB) failed the same way.
///
/// So when the cap applies, the decode may allocate [`CAPPED_EXTRA_ALLOC`] more than the
/// default: `max_alloc = 512 MiB + 768 MiB`, from which `ImageReader::decode` reserves the
/// full-size output buffer first and hands the rest to the decoder's working buffers. A
/// claim beyond that (the 20000 x 20000 bomb is 1.6 GB, a 100000 x 100000 header 40 GB) is
/// still refused before anything is allocated, and an image within the cap, or any decode
/// with `max_dim = 0`, keeps the default limits exactly. Raising a limit changes no pixel of
/// an image that decoded before.
///
/// Not from the literature: a resource limit. See also: the `image` crate's `Limits`
/// documentation (docs.rs, `image::Limits::max_alloc`), whose default this keeps for every
/// decode that is not capped; and, for the alternative of never holding the full-size
/// buffer, the demand-driven, strip-at-a-time evaluation of J. Cupitt, K. Martinez (1996),
/// *VIPS: an image processing system for large images*, Proc. SPIE 2663,
/// <https://doi.org/10.1117/12.233043> -- not taken, because the `image` crate decodes
/// whole frames and a streaming PNG path would duplicate its colour conversions.
fn capped_decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(DEFAULT_MAX_ALLOC + CAPPED_EXTRA_ALLOC);
    limits
}

/// Refuse a raster with no pixels: `Ok` when both sides are at least one pixel.
///
/// A decoder can hand back an image with a zero side without an error: a GIF whose
/// logical-screen width is 0 decodes to a 0 x 30000 image (found by the 2026-10-02 intake
/// fuzz, 3 of 1,200 mutated files, all GIFs), while PNG refuses the same header itself. Every
/// stage after the intake divides by a side or indexes row 0, and the first to do so was the
/// unblock test (`w / min(64, w)`, a division by zero, in Quality and Fast alike). An image
/// with no pixels has nothing to trace, so it is refused here, once, as a decode error.
///
/// Not from the literature: an input check.
fn has_pixels(w: u32, h: u32) -> Result<(), TraceError> {
    if w == 0 || h == 0 {
        return Err(TraceError::Decode(format!(
            "the image has no pixels ({w} x {h})"
        )));
    }
    Ok(())
}

/// Below this many pixels (256 × 256) the byte-to-float conversion runs on the calling
/// thread: at 128 px it is a few microseconds, less than handing it to rayon.
const PARALLEL_MIN_PIXELS: usize = 1 << 16;

/// Pixels per parallel job of the byte-to-float conversion: 64 KiB of floats, enough work
/// to amortise the job and few enough jobs (64 at 2048 × 2048) for rayon to balance.
const CONVERT_CHUNK_PIXELS: usize = 1 << 16;

/// `UNIT[k] = k as f32 / 255.0`: the value every 8-bit sample becomes.
///
/// A table rather than the division, because the division per byte was the largest part of
/// the intake's float conversion (10 ms of 20 at 2048 px, serial): a table lookup gives the
/// same float -- it holds exactly the quotient the old code computed, byte by byte -- for a
/// load. Built at compile time.
static UNIT: [f32; 256] = {
    let mut t = [0.0f32; 256];
    let mut k = 0;
    while k < 256 {
        t[k] = k as f32 / 255.0;
        k += 1;
    }
    t
};

/// Any decoded image to straight RGBA floats: 8-bit samples divided by 255, alpha 1 where
/// the image has none.
///
/// The old conversion always went through `to_rgba8()` -- a full copy even when the image
/// already was 8-bit RGBA -- and then divided every byte serially: 4.0 + 10.1 ms of the
/// 20 ms intake at 2048 px. Now:
///
/// * 8-bit RGBA is read straight from the decoder's buffer (no copy);
/// * 8-bit RGB, the common opaque container, is widened straight to four floats with alpha
///   `UNIT[255] = 1.0`, skipping the intermediate RGBA8 buffer;
/// * every other layout (grey, grey + alpha, 16-bit, float) still goes through the
///   library's `into_rgba8()`, whose colour conversion this does not reimplement;
///
/// and the bytes become floats through [`UNIT`], in parallel chunks on a large image.
///
/// *Why identical:* the library's RGB8 → RGBA8 conversion writes `[r, g, b, 255]` per pixel
/// (`subpixel_cast_rgb_to_rgba` in `image` 0.25), and `to_rgba8()` of an RGBA8 image is a
/// clone of its buffer; both are reproduced here byte for byte, and each byte becomes the
/// same float the division gave. Checked against the old path for every layout in
/// `from_dynamic_is_the_old_conversion`.
///
/// Not from the literature: an engineering change (skip a copy, table the division, split
/// the loop), because the conversion is a memory-bound map with no algorithm to choose.
/// See also: J. Ragan-Kelley et al., "Halide: A Language and Compiler for Optimizing
/// Parallelism, Locality, and Recomputation in Image Processing Pipelines", PLDI 2013,
/// DOI 10.1145/2491956.2462176 -- fusing the stages of an image pipeline so each pixel is
/// touched once; the conversion is fused here only with the layout change, since the
/// stages after it belong to other parts of the pipeline.
fn from_dynamic(img: image::DynamicImage) -> Rgba {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let data = match img {
        image::DynamicImage::ImageRgba8(buf) => widen::<4>(&buf.into_raw(), |p| *p),
        image::DynamicImage::ImageRgb8(buf) => {
            // Exactly the pixels, as the library's conversion reads them.
            let raw = buf.into_raw();
            widen::<3>(&raw[..w * h * 3], |p| [p[0], p[1], p[2], 255])
        }
        other => widen::<4>(&other.into_rgba8().into_raw(), |p| *p),
    };
    Rgba {
        width: w,
        height: h,
        data,
    }
}

/// Straight RGBA floats from packed `C`-byte pixels: every whole pixel of `raw` is widened
/// to four bytes by `rgba` and each byte mapped through [`UNIT`]. A trailing partial pixel
/// is dropped (a decoder never produces one). Parallel above [`PARALLEL_MIN_PIXELS`]; each
/// output float depends on one input byte, so the split cannot change a value.
fn widen<const C: usize>(raw: &[u8], rgba: impl Fn(&[u8; C]) -> [u8; 4] + Sync) -> Vec<f32> {
    use rayon::prelude::*;
    let n = raw.len() / C;
    let mut data = vec![0.0f32; n * 4];
    let convert = |(out, src): (&mut [f32], &[u8])| {
        for (o, s) in out
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(src.as_chunks::<C>().0)
        {
            let q = rgba(s);
            for c in 0..4 {
                o[c] = UNIT[q[c] as usize];
            }
        }
    };
    if n >= PARALLEL_MIN_PIXELS {
        data.par_chunks_mut(4 * CONVERT_CHUNK_PIXELS)
            .zip(raw.par_chunks(C * CONVERT_CHUNK_PIXELS))
            .for_each(convert);
    } else {
        convert((&mut data, raw));
    }
    data
}

/// The decode-time target for a `w x h` raster capped at `max_dim` on its longer side,
/// or `None` when no cap applies. `max_dim == 0` means no cap.
fn target_dims(w: u32, h: u32, max_dim: usize) -> Option<(u32, u32)> {
    if max_dim == 0 {
        return None;
    }
    let longest = w.max(h);
    if longest <= max_dim as u32 {
        return None;
    }
    let s = longest as f64 / max_dim as f64;
    Some((
        ((w as f64 / s).round() as u32).max(1),
        ((h as f64 / s).round() as u32).max(1),
    ))
}

/// Load a raster from a file path into straight RGBA floats, capping the longer side at
/// `max_dim` pixels (0 = no cap) before the pixels are read into floats, and returning the
/// file's original dimensions alongside so a caller can present the result at the size that
/// arrived. Both are upright: an image whose EXIF orientation turns it a quarter is
/// returned turned, with its sides swapped ([`decode_upright`]).
///
/// The size is decided from the file's header first, so the cap is known before the decode
/// allocates. The full-resolution 8-bit buffer may still be decoded once, but the cap is an
/// exact-area box average over that buffer, and the f32 conversion -- four bytes per channel,
/// the dominant allocation -- then runs at the capped size rather than at the file's size,
/// which is what used to blow past the decoder's 512 MiB guard on very large rasters.
pub fn load_image_capped(path: &Path, max_dim: usize) -> Result<(Rgba, (u32, u32)), TraceError> {
    let bytes = std::fs::read(path).map_err(|e| TraceError::Decode(e.to_string()))?;
    load_file_bytes_capped(path, &bytes, max_dim)
}

/// [`load_image_capped`] for a file the caller has already read into `bytes`: the same
/// raster, the same dimensions, the same errors, from one read of the file.
///
/// `load_image_capped` used to open the file twice -- once for the header, once to decode
/// through `image::open` -- and the command line opened it a third time for the first bytes
/// `--lossy auto` inspects. Reading it once and decoding from memory gives the same pixels:
/// the decoders are deterministic functions of the bytes, the reader here is set up with
/// default limits, and no decoding hooks are registered anywhere in this workspace. The
/// format is the one the bytes announce, else the extension's ([`sniff`]); a file that
/// names neither is handed to `image::open` itself, for its own error message.
///
/// Not from the literature: plumbing.
fn load_file_bytes_capped(
    path: &Path,
    bytes: &[u8],
    max_dim: usize,
) -> Result<(Rgba, (u32, u32)), TraceError> {
    match sniff(bytes, Some(path)) {
        Some(format) => decode_as(bytes, format, max_dim),
        // Neither the content nor the extension names a format: `image::open`'s error.
        None => Err(TraceError::Decode(image::open(path).err().map_or_else(
            || "unrecognised image format".into(),
            |e| e.to_string(),
        ))),
    }
}

/// A decoded `w × h` image as straight RGBA floats, box-averaged to the `max_dim` cap when
/// it is larger ([`target_dims`]), converted as it is ([`from_dynamic`]) otherwise. The
/// capped branch hands the library's `into_rgba8()` buffer straight to the box filter; the
/// `to_rgba8()` it replaces copied an RGBA8 image first, which changed no byte.
fn cap_decoded(img: image::DynamicImage, w: u32, h: u32, max_dim: usize) -> Rgba {
    match target_dims(w, h, max_dim) {
        Some((nw, nh)) => {
            let raw = img.into_rgba8().into_raw();
            coverage::box_downsample_rgba8(&raw, w as usize, h as usize, nw as usize, nh as usize)
        }
        None => from_dynamic(img),
    }
}

/// Decode in-memory bytes into straight RGBA floats, capping the longer side at `max_dim`
/// pixels (0 = no cap) before the pixels are read into floats, and returning the original
/// dimensions alongside. See [`load_image_capped`].
pub fn decode_image_capped(bytes: &[u8], max_dim: usize) -> Result<(Rgba, (u32, u32)), TraceError> {
    match sniff(bytes, None) {
        Some(format) => decode_as(bytes, format, max_dim),
        // No signature the tracer reads: the library's own error for such bytes.
        None => Err(TraceError::Decode(
            image::load_from_memory(bytes)
                .err()
                .map_or_else(|| "unrecognised image format".into(), |e| e.to_string()),
        )),
    }
}

/// Raw straight RGBA8 pixels (row-major, tightly packed, `w * h * 4` bytes) into straight
/// RGBA floats, capping the longer side at `max_dim` (0 = no cap) exactly as
/// [`decode_image_capped`] caps a decoded file.
///
/// The two share the cap and the 8-bit conversion so that a caller holding pixels and a
/// caller holding the encoded file trace the same raster, byte for byte. The caller checks
/// the length; a short buffer reads as transparent black past its end.
pub fn rgba8_capped(raw: &[u8], w: u32, h: u32, max_dim: usize) -> Rgba {
    match target_dims(w, h, max_dim) {
        Some((nw, nh)) => {
            coverage::box_downsample_rgba8(raw, w as usize, h as usize, nw as usize, nh as usize)
        }
        None => {
            // The first `n` bytes (or all of a short buffer) as floats, then zeros: whole
            // pixels through the parallel table conversion, a trailing partial pixel byte by
            // byte, exactly as the old serial `take(n).map(b / 255)` produced them.
            let n = w as usize * h as usize * 4;
            let take = raw.len().min(n);
            let whole = take / 4 * 4;
            let mut data = widen::<4>(&raw[..whole], |p| *p);
            data.extend(raw[whole..take].iter().map(|&b| UNIT[b as usize]));
            data.resize(n, 0.0);
            Rgba {
                width: w as usize,
                height: h as usize,
                data,
            }
        }
    }
}

/// The intake as it was before the table conversion and the single file read, kept as the
/// oracle for the tests below.
#[cfg(test)]
mod reference {
    use super::*;

    /// The old `from_dynamic`: always `to_rgba8()`, then a serial division per byte.
    pub(super) fn from_dynamic(img: &image::DynamicImage) -> Rgba {
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        let data = rgba.into_raw().iter().map(|&b| b as f32 / 255.0).collect();
        Rgba {
            width: w,
            height: h,
            data,
        }
    }

    /// The old `load_image_capped`: the header from one open, the pixels from `image::open`.
    pub(super) fn load_image_capped(
        path: &Path,
        max_dim: usize,
    ) -> Result<(Rgba, (u32, u32)), TraceError> {
        let (w, h) = image::ImageReader::open(path)
            .map_err(|e| TraceError::Decode(e.to_string()))?
            .into_dimensions()
            .map_err(|e| TraceError::Decode(e.to_string()))?;
        let img = image::open(path).map_err(|e| TraceError::Decode(e.to_string()))?;
        let out = match target_dims(w, h, max_dim) {
            Some((nw, nh)) => {
                let raw = img.to_rgba8().into_raw();
                coverage::box_downsample_rgba8(
                    &raw,
                    w as usize,
                    h as usize,
                    nw as usize,
                    nh as usize,
                )
            }
            None => from_dynamic(&img),
        };
        Ok((out, (w, h)))
    }

    /// The old uncapped branch of `rgba8_capped`.
    pub(super) fn rgba8(raw: &[u8], w: u32, h: u32) -> Vec<f32> {
        let n = w as usize * h as usize * 4;
        let mut data: Vec<f32> = raw.iter().take(n).map(|&b| b as f32 / 255.0).collect();
        data.resize(n, 0.0);
        data
    }
}

#[cfg(test)]
mod intake_equivalence_tests {
    use super::*;

    /// Deterministic bytes (a 64-bit LCG).
    fn bytes(n: usize, seed: u64) -> Vec<u8> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (s >> 56) as u8
            })
            .collect()
    }

    /// One image of every layout the decoder can hand over, at size `w × h`.
    fn every_layout(w: u32, h: u32) -> Vec<image::DynamicImage> {
        use image::DynamicImage as D;
        let n = (w * h) as usize;
        let b = |c: usize, seed| bytes(n * c, seed);
        let wide = |c: usize, seed| -> Vec<u16> {
            b(2 * c, seed)
                .chunks(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect()
        };
        let float = |c: usize, seed| -> Vec<f32> {
            b(c, seed).iter().map(|&v| v as f32 / 200.0 - 0.1).collect()
        };
        vec![
            D::ImageRgba8(image::RgbaImage::from_raw(w, h, b(4, 1)).unwrap()),
            D::ImageRgb8(image::RgbImage::from_raw(w, h, b(3, 2)).unwrap()),
            D::ImageLuma8(image::GrayImage::from_raw(w, h, b(1, 3)).unwrap()),
            D::ImageLumaA8(image::GrayAlphaImage::from_raw(w, h, b(2, 4)).unwrap()),
            D::ImageRgb16(image::ImageBuffer::from_raw(w, h, wide(3, 5)).unwrap()),
            D::ImageRgba16(image::ImageBuffer::from_raw(w, h, wide(4, 6)).unwrap()),
            D::ImageLuma16(image::ImageBuffer::from_raw(w, h, wide(1, 7)).unwrap()),
            D::ImageLumaA16(image::ImageBuffer::from_raw(w, h, wide(2, 8)).unwrap()),
            D::ImageRgb32F(image::ImageBuffer::from_raw(w, h, float(3, 9)).unwrap()),
            D::ImageRgba32F(image::ImageBuffer::from_raw(w, h, float(4, 10)).unwrap()),
        ]
    }

    fn bits(v: &[f32]) -> Vec<u32> {
        v.iter().map(|f| f.to_bits()).collect()
    }

    #[test]
    fn the_table_holds_the_quotients() {
        for k in 0..=255u8 {
            assert_eq!(UNIT[k as usize].to_bits(), (k as f32 / 255.0).to_bits());
        }
    }

    /// Every layout, at sizes from one pixel to past the parallel threshold, against the old
    /// `to_rgba8()`-and-divide conversion, bit for bit.
    #[test]
    fn from_dynamic_is_the_old_conversion() {
        for (w, h) in [(1, 1), (3, 1), (1, 5), (17, 9), (300, 250)] {
            for img in every_layout(w, h) {
                let what = format!("{:?} {w}x{h}", img.color());
                let old = reference::from_dynamic(&img);
                let new = from_dynamic(img);
                assert_eq!((new.width, new.height), (old.width, old.height), "{what}");
                assert_eq!(bits(&new.data), bits(&old.data), "{what}");
            }
        }
    }

    /// Short, exact and long raw buffers, including a partial trailing pixel, against the
    /// old serial conversion.
    #[test]
    fn raw_pixels_are_the_old_conversion() {
        for (w, h) in [(1u32, 1u32), (5, 3), (300, 250)] {
            let n = (w * h * 4) as usize;
            for len in [0, 1, 6, n.saturating_sub(3), n, n + 5] {
                let raw = bytes(len, len as u64 + 11);
                let new = rgba8_capped(&raw, w, h, 0);
                assert_eq!(
                    bits(&new.data),
                    bits(&reference::rgba8(&raw, w, h)),
                    "{len}"
                );
            }
        }
    }

    /// The one-read loader against the old two-open loader, on real files of several
    /// containers, capped and uncapped (same pixels, or the same error text); and on files
    /// whose extension lies about or omits the format, which the old loader refused and the
    /// content-sniffing one decodes as what they are, pixel for pixel the correctly named
    /// file.
    #[test]
    fn one_read_equals_image_open() {
        let dir = std::env::temp_dir().join(format!("inkvec-load-once-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rgba = image::RgbaImage::from_raw(90, 70, bytes(90 * 70 * 4, 42)).unwrap();
        let rgb = image::DynamicImage::ImageRgba8(rgba.clone()).to_rgb8();
        let mut files = Vec::new();
        for (name, img) in [
            ("a.png", image::DynamicImage::ImageRgba8(rgba.clone())),
            ("b.png", image::DynamicImage::ImageRgb8(rgb.clone())),
            ("c.bmp", image::DynamicImage::ImageRgb8(rgb.clone())),
            ("d.jpg", image::DynamicImage::ImageRgb8(rgb.clone())),
            ("e.tiff", image::DynamicImage::ImageRgba8(rgba.clone())),
        ] {
            let p = dir.join(name);
            img.save(&p).unwrap();
            files.push(p);
        }
        // A PNG named as a JPEG, one with no extension at all, and a JPEG named as a PNG:
        // each must read exactly as the file it is a copy of.
        let mut lying = Vec::new();
        for (name, real) in [("f.jpg", "a.png"), ("g", "a.png"), ("h.png", "d.jpg")] {
            let p = dir.join(name);
            std::fs::copy(dir.join(real), &p).unwrap();
            lying.push((p, dir.join(real)));
        }
        for (p, real) in &lying {
            for max_dim in [0usize, 64, 2048] {
                let (a, da) = load_image_capped(p, max_dim).unwrap();
                let (b, db) = load_image_capped(real, max_dim).unwrap();
                assert_eq!((da, a.width, a.height), (db, b.width, b.height), "{p:?}");
                assert_eq!(bits(&a.data), bits(&b.data), "{p:?} at {max_dim}");
                let (c, _) = decode_image_capped(&std::fs::read(p).unwrap(), max_dim).unwrap();
                assert_eq!(bits(&a.data), bits(&c.data), "{p:?}: file and bytes agree");
            }
        }
        // A file that is no image, under a name that promises one, keeps its decoder's error.
        let text = dir.join("text.png");
        std::fs::write(&text, b"this is not an image\n").unwrap();
        files.push(text);
        files.push(dir.join("missing.png"));
        for p in &files {
            for max_dim in [0usize, 64, 2048] {
                let new = load_image_capped(p, max_dim);
                let old = reference::load_image_capped(p, max_dim);
                match (new, old) {
                    (Ok((a, da)), Ok((b, db))) => {
                        assert_eq!(da, db, "{p:?}");
                        assert_eq!((a.width, a.height), (b.width, b.height), "{p:?}");
                        assert_eq!(bits(&a.data), bits(&b.data), "{p:?} at {max_dim}");
                    }
                    (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string(), "{p:?}"),
                    (a, b) => panic!("{p:?}: {:?} against {:?}", a.is_ok(), b.is_ok()),
                }
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod lossy_container_tests {
    use super::lossy_container;

    /// A real encode of each, so the test fails if the `image` crate ever disagrees with
    /// the byte patterns this reads.
    fn encode(fmt: image::ImageFormat) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(24, 24, |x, y| {
            image::Rgb([(x * 10) as u8, (y * 10) as u8, 90])
        }));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, fmt).unwrap();
        buf.into_inner()
    }

    #[test]
    fn jpeg_is_lossy_and_png_is_not() {
        assert_eq!(
            lossy_container(&encode(image::ImageFormat::Jpeg)),
            Some(true)
        );
        assert_eq!(
            lossy_container(&encode(image::ImageFormat::Png)),
            Some(false)
        );
    }

    #[test]
    fn a_header_is_enough() {
        // The caller only reads the first bytes of the file, so the answer must not need
        // the rest of it.
        let jpeg = encode(image::ImageFormat::Jpeg);
        assert_eq!(lossy_container(&jpeg[..32.min(jpeg.len())]), Some(true));
    }

    #[test]
    fn webp_is_read_from_the_riff_chunk() {
        let mut b = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
        assert_eq!(lossy_container(&b), Some(true));
        b[12..16].copy_from_slice(b"VP8L");
        assert_eq!(lossy_container(&b), Some(false));
        // The extended container needs its sub-chunks walked; do not guess at it.
        b[12..16].copy_from_slice(b"VP8X");
        assert_eq!(lossy_container(&b), None);
    }

    #[test]
    fn nonsense_is_unknown_not_clean() {
        assert_eq!(lossy_container(b"not an image at all"), None);
    }
}

#[cfg(test)]
mod rgba8_capped_tests {
    use super::{decode_image_capped, rgba8_capped};

    /// Raw pixels and the same pixels encoded as a PNG must reach the tracer as the same
    /// raster, capped or not: the language bindings promise that the two inputs agree.
    #[test]
    fn raw_pixels_match_the_decoded_file_with_and_without_a_cap() {
        let (w, h) = (40u32, 24u32);
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([
                (x * 6) as u8,
                (y * 9) as u8,
                200,
                if x > 20 { 255 } else { 90 },
            ])
        });
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        for max_dim in [0usize, 2048, 16] {
            let (decoded, dims) = decode_image_capped(&png, max_dim).unwrap();
            let raw = rgba8_capped(img.as_raw(), w, h, max_dim);
            assert_eq!(dims, (w, h));
            assert_eq!((raw.width, raw.height), (decoded.width, decoded.height));
            assert_eq!(raw.data, decoded.data, "max_dim {max_dim}");
        }
    }
}

#[cfg(test)]
mod decode_cap_tests {
    use super::*;

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            w,
            h,
            image::Rgb([200, 30, 30]),
        ));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn decode_cap_bounds_a_large_raster() {
        let bytes = png_bytes(512, 256);

        // No cap decodes at full size.
        let (full, _) = decode_image_capped(&bytes, 0).unwrap();
        assert_eq!((full.width, full.height), (512, 256));

        // A cap reduces the raster before the f32 conversion, so the buffer that comes
        // back is bounded by the cap rather than by the file's dimensions -- and the
        // original dimensions still come back alongside.
        let (capped, (aw, ah)) = decode_image_capped(&bytes, 64).unwrap();
        assert_eq!((aw, ah), (512, 256), "arrival dimensions must be preserved");
        assert!(
            capped.width <= 64 && capped.height <= 64,
            "capped raster is {}x{}, want at most 64 on the longer side",
            capped.width,
            capped.height
        );
        assert_eq!(capped.data.len(), capped.width * capped.height * 4);
        assert!(capped.width < full.width);
    }

    /// CRC-32 (IEEE), for writing PNG chunks by hand.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
            }
        }
        !crc
    }

    /// `png` (an encoded PNG) with an `eXIf` chunk holding the Orientation tag set to
    /// `orientation`, inserted after `IHDR`. The chunk is a bare big-endian TIFF structure:
    /// header, one IFD with one SHORT entry (tag 274), no next IFD.
    fn with_orientation(png: &[u8], orientation: u16) -> Vec<u8> {
        let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
        tiff.extend_from_slice(&1u16.to_be_bytes());
        tiff.extend_from_slice(&274u16.to_be_bytes());
        tiff.extend_from_slice(&3u16.to_be_bytes());
        tiff.extend_from_slice(&1u32.to_be_bytes());
        tiff.extend_from_slice(&orientation.to_be_bytes());
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&0u32.to_be_bytes());
        let mut chunk = (tiff.len() as u32).to_be_bytes().to_vec();
        let mut body = b"eXIf".to_vec();
        body.extend_from_slice(&tiff);
        chunk.extend_from_slice(&body);
        chunk.extend_from_slice(&crc32(&body).to_be_bytes());
        // Signature (8) + IHDR (4 + 4 + 13 + 4).
        let at = 8 + 25;
        [&png[..at], &chunk[..], &png[at..]].concat()
    }

    /// A 3 x 2 image of six distinct opaque colours stored with each EXIF orientation comes
    /// back turned as a viewer shows it -- a quarter turn swaps the sides, also in the
    /// arrival dimensions -- and without the tag (or with value 1) exactly as before.
    #[test]
    fn the_exif_orientation_is_applied() {
        let (w, h) = (3u32, 2u32);
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([
                (40 * x + 100 * y) as u8,
                (10 + 70 * y) as u8,
                (200 - 50 * x) as u8,
                255,
            ])
        });
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        let px = |r: &Rgba, x: u32, y: u32| -> [u32; 4] {
            let i = ((y as usize) * r.width + x as usize) * 4;
            std::array::from_fn(|c| (r.data[i + c] * 255.0).round() as u32)
        };
        let src = |x: u32, y: u32| -> [u32; 4] { img.get_pixel(x, y).0.map(u32::from) };
        let plain = decode_image_capped(&png, 0).unwrap();
        for o in 1..=8u16 {
            let (got, dims) = decode_image_capped(&with_orientation(&png, o), 0).unwrap();
            let quarter = o >= 5;
            let (gw, gh) = if quarter { (h, w) } else { (w, h) };
            assert_eq!(dims, (gw, gh), "orientation {o}");
            assert_eq!(
                (got.width, got.height),
                (gw as usize, gh as usize),
                "orientation {o}"
            );
            for y in 0..gh {
                for x in 0..gw {
                    // Where each displayed pixel comes from in the stored image (Exif values:
                    // 2 mirror, 3 half turn, 4 flip, 5 transpose, 6 quarter clockwise,
                    // 7 transverse, 8 quarter anticlockwise).
                    let (sx, sy) = match o {
                        1 => (x, y),
                        2 => (w - 1 - x, y),
                        3 => (w - 1 - x, h - 1 - y),
                        4 => (x, h - 1 - y),
                        5 => (y, x),
                        6 => (y, h - 1 - x),
                        7 => (w - 1 - y, h - 1 - x),
                        _ => (w - 1 - y, x),
                    };
                    assert_eq!(px(&got, x, y), src(sx, sy), "orientation {o} at ({x}, {y})");
                }
            }
            if o == 1 {
                assert_eq!(bits(&got.data), bits(&plain.0.data), "value 1 is no change");
            }
        }
    }

    fn bits(v: &[f32]) -> Vec<u32> {
        v.iter().map(|f| f.to_bits()).collect()
    }

    /// The capped allowance is the library's default plus the extra, so the default this
    /// file assumes must be the library's; and an uncapped decode keeps the default.
    #[test]
    fn the_capped_limit_extends_the_library_default() {
        assert_eq!(image::Limits::default().max_alloc, Some(DEFAULT_MAX_ALLOC));
        assert_eq!(
            capped_decode_limits().max_alloc,
            Some(DEFAULT_MAX_ALLOC + CAPPED_EXTRA_ALLOC)
        );
        // 12000 x 12000 RGBA fits the capped allowance (on a 64-bit target); 20000 x 20000
        // does not.
        if cfg!(target_pointer_width = "64") {
            assert!(12_000u64 * 12_000 * 4 <= DEFAULT_MAX_ALLOC + CAPPED_EXTRA_ALLOC);
        }
        assert!(20_000u64 * 20_000 * 4 > DEFAULT_MAX_ALLOC + CAPPED_EXTRA_ALLOC);
    }

    /// The fuzz case `m1_00557_s0.gif`, made small: a GIF whose logical screen is 0 pixels
    /// wide decodes without an error in the `image` crate, and must be refused at intake by
    /// every entry point rather than reach the tracer with no pixels.
    #[test]
    fn a_gif_with_a_zero_wide_screen_is_refused() {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            5,
            7,
            image::Rgba([10, 200, 30, 255]),
        ));
        let mut gif = std::io::Cursor::new(Vec::new());
        img.write_to(&mut gif, image::ImageFormat::Gif).unwrap();
        let mut gif = gif.into_inner();
        // The logical screen descriptor follows the 6-byte signature: width, then height,
        // as little-endian u16.
        gif[6..8].copy_from_slice(&0u16.to_le_bytes());
        for result in [
            decode_image_capped(&gif, 0).map(|_| ()),
            decode_image_capped(&gif, 2048).map(|_| ()),
            decode_image(&gif).map(|_| ()),
        ] {
            let e = result.expect_err("an image with no pixels is refused");
            assert!(e.to_string().contains("no pixels"), "{e}");
        }
        let path =
            std::env::temp_dir().join(format!("inkvec-zero-wide-{}.gif", std::process::id()));
        std::fs::write(&path, &gif).unwrap();
        let capped = load_image_capped(&path, 2048).map(|_| ());
        let plain = load_image(&path).map(|_| ());
        std::fs::remove_file(&path).ok();
        for result in [capped, plain] {
            let e = result.expect_err("an image with no pixels is refused");
            assert!(e.to_string().contains("no pixels"), "{e}");
        }
    }

    #[test]
    fn load_cap_bounds_a_large_file() {
        let path = std::env::temp_dir().join(format!("inkvec-load-cap-{}.png", std::process::id()));
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            300,
            100,
            image::Rgb([0, 0, 0]),
        ))
        .save(&path)
        .unwrap();
        let (capped, (aw, ah)) = load_image_capped(&path, 40).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!((aw, ah), (300, 100), "arrival dimensions must be preserved");
        assert!(
            capped.width <= 40 && capped.height <= 40,
            "capped file is {}x{}, want at most 40 on the longer side",
            capped.width,
            capped.height
        );
        assert_eq!(capped.data.len(), capped.width * capped.height * 4);
    }
}
