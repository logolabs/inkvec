//! Intake: reading a raster into straight RGBA floats, optionally capped in size, and
//! asking the container whether a lossy codec wrote it.
//!
//! Every entry point returns an [`Rgba`] with channels in `[0, 1]` (8-bit samples divided
//! by 255, not premultiplied), which is what the tracer takes. Re-exported at the crate
//! root; moved out of `lib.rs` unchanged.

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

/// Load a raster image from a file path into straight RGBA floats.
pub fn load_image(path: &Path) -> Result<Rgba, TraceError> {
    let img = image::open(path).map_err(|e| TraceError::Decode(e.to_string()))?;
    Ok(from_dynamic(&img))
}

/// Decode a raster image from in-memory bytes into straight RGBA floats.
pub fn decode_image(bytes: &[u8]) -> Result<Rgba, TraceError> {
    let img = image::load_from_memory(bytes).map_err(|e| TraceError::Decode(e.to_string()))?;
    Ok(from_dynamic(&img))
}

/// Any decoded image to straight RGBA floats: 8-bit samples divided by 255.
fn from_dynamic(img: &image::DynamicImage) -> Rgba {
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let data = rgba.into_raw().iter().map(|&b| b as f32 / 255.0).collect();
    Rgba {
        width: w,
        height: h,
        data,
    }
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
/// arrived.
///
/// The size is decided from the file's header first, so the cap is known before the decode
/// allocates. The full-resolution 8-bit buffer may still be decoded once, but the cap is an
/// exact-area box average over that buffer, and the f32 conversion -- four bytes per channel,
/// the dominant allocation -- then runs at the capped size rather than at the file's size,
/// which is what used to blow past the decoder's 512 MiB guard on very large rasters.
pub fn load_image_capped(path: &Path, max_dim: usize) -> Result<(Rgba, (u32, u32)), TraceError> {
    let (w, h) = image::ImageReader::open(path)
        .map_err(|e| TraceError::Decode(e.to_string()))?
        .into_dimensions()
        .map_err(|e| TraceError::Decode(e.to_string()))?;
    let img = image::open(path).map_err(|e| TraceError::Decode(e.to_string()))?;
    let out = match target_dims(w, h, max_dim) {
        Some((nw, nh)) => {
            let rgba = img.to_rgba8();
            let raw = rgba.into_raw();
            coverage::box_downsample_rgba8(&raw, w as usize, h as usize, nw as usize, nh as usize)
        }
        None => from_dynamic(&img),
    };
    Ok((out, (w, h)))
}

/// Decode in-memory bytes into straight RGBA floats, capping the longer side at `max_dim`
/// pixels (0 = no cap) before the pixels are read into floats, and returning the original
/// dimensions alongside. See [`load_image_capped`].
pub fn decode_image_capped(bytes: &[u8], max_dim: usize) -> Result<(Rgba, (u32, u32)), TraceError> {
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| TraceError::Decode(e.to_string()))?
        .into_dimensions()
        .map_err(|e| TraceError::Decode(e.to_string()))?;
    let img = image::load_from_memory(bytes).map_err(|e| TraceError::Decode(e.to_string()))?;
    let out = match target_dims(w, h, max_dim) {
        Some((nw, nh)) => {
            let rgba = img.to_rgba8();
            let raw = rgba.into_raw();
            coverage::box_downsample_rgba8(&raw, w as usize, h as usize, nw as usize, nh as usize)
        }
        None => from_dynamic(&img),
    };
    Ok((out, (w, h)))
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
            let n = w as usize * h as usize * 4;
            let mut data: Vec<f32> = raw.iter().take(n).map(|&b| b as f32 / 255.0).collect();
            data.resize(n, 0.0);
            Rgba {
                width: w as usize,
                height: h as usize,
                data,
            }
        }
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
