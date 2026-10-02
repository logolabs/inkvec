//! Colour management at intake: the colours of an image that carries an ICC profile,
//! converted to sRGB before anything reads them.
//!
//! # The problem
//!
//! A pixel's numbers mean a colour only together with the colour space they were written
//! in. The tracer reads every pixel as sRGB (IEC 61966-2-1), which is right for the great
//! majority of files: a file without a profile is sRGB by convention, and most profiles
//! embedded by design tools *are* sRGB. But a screenshot from a Mac is tagged Display P3, a
//! photo export is often Adobe RGB, and an old prepress file may be in a gamma-1.8 space;
//! every viewer converts such a file to the screen's space through its profile, so the
//! colours a person sees are the converted ones. Reading the stored numbers as sRGB instead
//! traced six such images (gamma 1.8, profile embedded) at dE00 0.84 on average (worst 3.03)
//! against 0.077 for the same art saved as sRGB (r2-inputs `formats_test.py`, 2026-10-02).
//!
//! # The method
//!
//! 1. Parse the profile (`moxcms::ColorProfile::new_from_slice`). An unreadable profile, or
//!    one whose colour space does not match the decoded pixels, leaves the image as it was:
//!    the tracer is no worse off than before, and the diagnostics say why.
//! 2. Build the 8-bit transform from the profile to sRGB (`create_transform_8bit`), with the
//!    relative colorimetric rendering intent: in-gamut colours keep their measured value and
//!    out-of-gamut ones are clipped to the sRGB gamut, which is what a colour-managed browser
//!    shows for an image on an sRGB display.
//!    * an RGB profile converts RGBA to RGBA, alpha passed through;
//!    * a grey profile on a grey image converts grey + alpha to RGBA (sRGB grey);
//!    * any other pairing (a CMYK profile on a JPEG the decoder has already turned into
//!      RGB, a Lab profile, ...) is left alone, because the pixels are no longer in the
//!      profile's space.
//! 3. Probe the transform on every level of each primary ramp, the grey ramp and a 9 x 9 x 9
//!    lattice ([`moves_colours`]). When no probe moves by more than one level the profile is
//!    sRGB in all but name (an embedded "sRGB IEC61966-2.1" is the common case) and the image
//!    is returned untouched: converting it through lookup tables would only add a level of
//!    rounding noise to pixels that were already right, and a clean image must not change.
//! 4. Otherwise convert the whole 8-bit image, in parallel bands of rows (each pixel is
//!    converted on its own, so the split cannot change a value).
//!
//! The conversion happens on the decoded 8-bit image, before the `--max-dim` cap and before
//! the floats exist, so the box filter averages sRGB values exactly as it would for a file
//! saved in sRGB.
//!
//! # Where it sits
//!
//! Called by `load::decode_upright` for every decode, with the profile the decoder reports
//! (`ImageDecoder::icc_profile`: PNG `iCCP`, JPEG `APP2`, WebP `ICCP`, TIFF tag 34675). No
//! image in the benchmark sets carries a profile, so the traces judged there are unchanged.
//!
//! Method from: the ICC colour-management architecture, Specification ICC.1:2010 (profile
//! version 4.3.0.0), *Image technology colour management -- Architecture, profile format,
//! and data structure*, International Color Consortium,
//! <https://www.color.org/specification/ICC1v43_2010-12.pdf> (profile connection space,
//! rendering intents), as implemented by the `moxcms` crate (pure Rust, already the
//! `image` crate's colour engine). Not from the literature: the identity probe of step 3,
//! because a profile can describe sRGB without being byte-identical to any reference
//! profile, and the probe asks the only question that matters here -- would the conversion
//! change a pixel -- directly.
//!
//! Determinism: `moxcms` picks SIMD paths at run time (SSE, AVX, NEON); its 8-bit
//! transforms use fixed-point arithmetic by default, so a given machine always converts the
//! same way, and only images that carry a non-sRGB profile take this path at all.

use image::DynamicImage;
use moxcms::{ColorProfile, DataColorSpace, Layout, RenderingIntent, TransformOptions};
use rayon::prelude::*;

/// How many levels a probe colour may move for the profile still to count as sRGB.
const SRGB_TOLERANCE: i16 = 1;

/// Rows per parallel job of the conversion.
const ROWS_PER_JOB: usize = 64;

/// `img` with its colours converted from the ICC profile `icc` to sRGB, or `img` itself
/// when the profile cannot be read, does not match the pixels, or describes sRGB already
/// (see the module comment for each case). The result is 8-bit RGBA when converted.
pub(super) fn to_srgb(img: DynamicImage, icc: &[u8]) -> DynamicImage {
    let profile = match ColorProfile::new_from_slice(icc) {
        Ok(p) => p,
        Err(e) => {
            crate::diag!("intake", "icc unreadable ({e:?}); colours read as sRGB");
            return img;
        }
    };
    let srgb = ColorProfile::new_srgb();
    let options = TransformOptions {
        rendering_intent: RenderingIntent::RelativeColorimetric,
        ..TransformOptions::default()
    };
    use image::ColorType as C;
    let grey = matches!(img.color(), C::L8 | C::La8 | C::L16 | C::La16);
    let src_layout = match profile.color_space {
        DataColorSpace::Rgb => Layout::Rgba,
        DataColorSpace::Gray if grey => Layout::GrayAlpha,
        space => {
            crate::diag!(
                "intake",
                "icc profile for {space:?} on {:?} pixels; colours read as sRGB",
                img.color()
            );
            return img;
        }
    };
    let transform = match profile.create_transform_8bit(src_layout, &srgb, Layout::Rgba, options) {
        Ok(t) => t,
        Err(e) => {
            crate::diag!(
                "intake",
                "icc transform unavailable ({e:?}); colours read as sRGB"
            );
            return img;
        }
    };
    let channels = if src_layout == Layout::Rgba { 4 } else { 2 };
    if !moves_colours(&*transform, channels) {
        crate::diag!(
            "intake",
            "icc profile is sRGB to within a level; image unchanged"
        );
        return img;
    }
    let (w, h) = (img.width(), img.height());
    let src = if channels == 4 {
        img.into_rgba8().into_raw()
    } else {
        img.into_luma_alpha8().into_raw()
    };
    let mut out = vec![0u8; w as usize * h as usize * 4];
    let (row_in, row_out) = (w as usize * channels, w as usize * 4);
    let ok = out
        .par_chunks_mut(row_out * ROWS_PER_JOB)
        .zip(src.par_chunks(row_in * ROWS_PER_JOB))
        .all(|(o, s)| transform.transform(s, o).is_ok());
    if !ok {
        crate::diag!("intake", "icc transform failed; colours read as sRGB");
        return match channels {
            4 => {
                DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, src).expect("w x h x 4"))
            }
            _ => DynamicImage::ImageLumaA8(
                image::GrayAlphaImage::from_raw(w, h, src).expect("w x h x 2"),
            ),
        };
    }
    crate::diag!(
        "intake",
        "icc profile converted to sRGB ({:?})",
        profile.color_space
    );
    DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, out).expect("w x h x 4"))
}

/// Whether the transform moves any probe colour by more than [`SRGB_TOLERANCE`] levels in
/// any channel, or changes its alpha.
///
/// The probes, for an RGBA transform (`channels == 4`): every level of the red, green and
/// blue ramps and of the grey ramp (4 x 256 colours: a tone curve or a primary that differs
/// from sRGB's moves some of them), and the 9 x 9 x 9 lattice of levels `0, 32, ..., 224, 255`
/// (a 3-D table that differs anywhere moves some of those). For a grey-to-RGBA transform
/// (`channels == 2`): every grey level, expected back as the same sRGB grey. All opaque.
/// A transform that fails on the probes counts as moving them.
fn moves_colours(t: &(dyn moxcms::TransformExecutor<u8> + Send + Sync), channels: usize) -> bool {
    let mut probe: Vec<u8> = Vec::new();
    if channels == 4 {
        for v in 0..=255u8 {
            for c in [[v, 0, 0], [0, v, 0], [0, 0, v], [v, v, v]] {
                probe.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        let lattice = [0u8, 32, 64, 96, 128, 160, 192, 224, 255];
        for &r in &lattice {
            for &g in &lattice {
                for &b in &lattice {
                    probe.extend_from_slice(&[r, g, b, 255]);
                }
            }
        }
    } else {
        for v in 0..=255u8 {
            probe.extend_from_slice(&[v, 255]);
        }
    }
    let n = probe.len() / channels;
    let mut out = vec![0u8; n * 4];
    if t.transform(&probe, &mut out).is_err() {
        return true;
    }
    (0..n).any(|i| {
        let src = &probe[i * channels..(i + 1) * channels];
        let want = if channels == 4 {
            [src[0], src[1], src[2], src[3]]
        } else {
            [src[0], src[0], src[0], src[1]]
        };
        let got = &out[i * 4..i * 4 + 4];
        got[3] != want[3]
            || (0..3).any(|c| (i16::from(got[c]) - i16::from(want[c])).abs() > SRGB_TOLERANCE)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gamma-1.8 RGB profile with sRGB's primaries, and the image the r2 test made with
    /// one: the colours must move (mid grey 128 under gamma 1.8 is about 116 in sRGB) and
    /// alpha must pass through.
    fn gamma_profile(gamma: f32) -> Vec<u8> {
        let mut p = ColorProfile::new_srgb();
        // The built-in sRGB profile also carries a CICP tag naming the sRGB transfer, which a
        // transform prefers to the curves; a real gamma-1.8 profile has none.
        p.cicp = None;
        let curve = moxcms::ToneReprCurve::Parametric(vec![gamma]);
        p.red_trc = Some(curve.clone());
        p.green_trc = Some(curve.clone());
        p.blue_trc = Some(curve);
        p.encode().expect("a matrix-shaper profile encodes")
    }

    fn rgba_image(px: &[[u8; 4]]) -> DynamicImage {
        DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(px.len() as u32, 1, px.concat()).unwrap(),
        )
    }

    #[test]
    fn an_srgb_profile_leaves_the_image_untouched() {
        let srgb = ColorProfile::new_srgb().encode().unwrap();
        let img = rgba_image(&[[10, 200, 30, 255], [128, 128, 128, 90], [255, 0, 7, 0]]);
        let out = to_srgb(img.clone(), &srgb);
        assert_eq!(out.as_bytes(), img.as_bytes());
        assert_eq!(out.color(), img.color());
    }

    #[test]
    fn a_gamma_18_profile_moves_the_mid_tones_and_keeps_alpha() {
        let img = rgba_image(&[[0, 0, 0, 255], [128, 128, 128, 77], [255, 255, 255, 255]]);
        let out = to_srgb(img, &gamma_profile(1.8)).into_rgba8();
        let px: Vec<[u8; 4]> = out.pixels().map(|p| p.0).collect();
        assert_eq!(px[0], [0, 0, 0, 255], "black stays black");
        assert_eq!(px[2], [255, 255, 255, 255], "white stays white");
        assert_eq!(px[1][3], 77, "alpha passes through");
        // (128/255)^1.8 in linear light is 0.289, which sRGB encodes as about 145; the
        // exact level depends on the transform's rounding, so allow a few.
        assert!((140..=150).contains(&px[1][0]), "{:?}", px[1]);
        assert!(
            px[1][0] == px[1][1] && px[1][1] == px[1][2],
            "grey stays grey"
        );
    }

    #[test]
    fn an_unreadable_or_mismatched_profile_is_ignored() {
        let img = rgba_image(&[[1, 2, 3, 4]]);
        assert_eq!(
            to_srgb(img.clone(), b"not a profile").as_bytes(),
            img.as_bytes()
        );
        let grey = ColorProfile::new_gray_with_gamma(1.8).encode().unwrap();
        // A grey profile on RGB pixels: not this profile's pixels.
        assert_eq!(to_srgb(img.clone(), &grey).as_bytes(), img.as_bytes());
    }

    #[test]
    fn a_grey_profile_on_a_grey_image_becomes_srgb_grey() {
        let grey = ColorProfile::new_gray_with_gamma(1.8).encode().unwrap();
        let img = DynamicImage::ImageLumaA8(
            image::GrayAlphaImage::from_raw(2, 1, vec![128, 200, 255, 255]).unwrap(),
        );
        let out = to_srgb(img, &grey).into_rgba8();
        let p = out.get_pixel(0, 0).0;
        assert_eq!(p[3], 200);
        assert!(
            p[0] == p[1] && p[1] == p[2] && (140..=150).contains(&p[0]),
            "{p:?}"
        );
        assert_eq!(out.get_pixel(1, 0).0, [255, 255, 255, 255]);
    }

    /// The parallel split by rows converts each pixel on its own: a tall image converts to
    /// the same bytes as one pass over all of it.
    #[test]
    fn the_row_split_does_not_change_a_value() {
        let (w, h) = (37u32, 3 * ROWS_PER_JOB as u32 + 5);
        let px: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, px.clone()).unwrap());
        let icc = gamma_profile(2.6);
        let out = to_srgb(img, &icc).into_rgba8().into_raw();
        let profile = ColorProfile::new_from_slice(&icc).unwrap();
        let t = profile
            .create_transform_8bit(
                Layout::Rgba,
                &ColorProfile::new_srgb(),
                Layout::Rgba,
                TransformOptions {
                    rendering_intent: RenderingIntent::RelativeColorimetric,
                    ..TransformOptions::default()
                },
            )
            .unwrap();
        let mut whole = vec![0u8; px.len()];
        t.transform(&px, &mut whole).unwrap();
        assert_eq!(out, whole);
    }
}
