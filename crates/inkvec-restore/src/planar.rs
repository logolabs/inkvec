//! Layout conversion shared by every backend: interleaved RGB in, planar CHW padded to a
//! multiple of 16 for the network, and back.
//!
//! Padding replicates the last row and column, the same `F.pad(..., mode="replicate")` on the
//! bottom and right that `restore_cli.py` applies, so every backend agrees with the Python
//! reference pixel for pixel.
//!
//! The in-process backends call these directly. A backend that runs the network out of this
//! crate's reach -- the browser's ONNX Runtime Web session, which holds the same `.onnx` file
//! -- reaches them through [`crate::network_input`] and [`crate::network_output`], so that it
//! pads and crops with this code and not with a reimplementation of it in another language.

/// Both sides of the network input must be multiples of this (four 2x downsamplings).
pub const MULTIPLE: usize = 16;

/// The padded size for a `width x height` input: each side rounded up to the next multiple
/// of [`MULTIPLE`] (a side that already is one is unchanged; 0 stays 0).
pub fn padded(width: usize, height: usize) -> (usize, usize) {
    (
        width.div_ceil(MULTIPLE) * MULTIPLE,
        height.div_ceil(MULTIPLE) * MULTIPLE,
    )
}

/// Interleaved RGB `width x height` to planar CHW `pw x ph`, replicating the last column and
/// row into the padding.
///
/// Output index `c·pw·ph + y·pw + x` holds input channel `c` at
/// `(min(x, width − 1), min(y, height − 1))`, so the padding is edge replication (clamp to
/// edge), never zeros: a zero border would be a black frame the network tries to restore.
/// Requires `width, height ≥ 1`, `pw ≥ width`, `ph ≥ height` and `rgb.len() ≥ 3·width·height`
/// ([`check_input`] guarantees the first and last).
pub fn to_planar_padded(
    rgb: &[f32],
    width: usize,
    height: usize,
    pw: usize,
    ph: usize,
) -> Vec<f32> {
    let mut out = vec![0f32; 3 * pw * ph];
    for c in 0..3 {
        let plane = &mut out[c * pw * ph..(c + 1) * pw * ph];
        for y in 0..ph {
            let sy = y.min(height - 1);
            let row = &mut plane[y * pw..(y + 1) * pw];
            for (x, v) in row.iter_mut().enumerate() {
                *v = rgb[(sy * width + x.min(width - 1)) * 3 + c];
            }
        }
    }
    out
}

/// Planar CHW `pw x ph` back to interleaved RGB, keeping the top-left `width x height`.
///
/// The exact inverse of [`to_planar_padded`] on the kept region: the padding was added only on
/// the right and bottom, so the original pixels sit at the same `(x, y)`. `chw` must hold
/// `3·pw·ph` floats.
pub fn from_planar_cropped(
    chw: &[f32],
    width: usize,
    height: usize,
    pw: usize,
    ph: usize,
) -> Vec<f32> {
    let mut out = vec![0f32; width * height * 3];
    for c in 0..3 {
        let plane = &chw[c * pw * ph..(c + 1) * pw * ph];
        for y in 0..height {
            for x in 0..width {
                out[(y * width + x) * 3 + c] = plane[y * pw + x];
            }
        }
    }
    out
}

/// Check an input buffer before any backend sees it: both sides non-zero and exactly
/// `3·width·height` floats. The error states what was given and what was wanted.
pub fn check_input(rgb: &[f32], width: usize, height: usize) -> Result<(), String> {
    if width == 0 || height == 0 || rgb.len() != width * height * 3 {
        return Err(format!(
            "restorer input is {} floats, want {width}x{height}x3",
            rgb.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_replicates_edges_and_crop_inverts_it() {
        // 2x1 image: red, green. Padded to 3x2.
        let rgb = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let planar = to_planar_padded(&rgb, 2, 1, 3, 2);
        // R plane: row 0 = 1 0 0 (last column replicates green's R = 0), row 1 repeats row 0.
        assert_eq!(&planar[0..6], &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        // G plane.
        assert_eq!(&planar[6..12], &[0.0, 1.0, 1.0, 0.0, 1.0, 1.0]);
        assert_eq!(from_planar_cropped(&planar, 2, 1, 3, 2), rgb);
    }

    #[test]
    fn padded_rounds_up_to_sixteen() {
        assert_eq!(padded(512, 500), (512, 512));
        assert_eq!(padded(1, 17), (16, 32));
        assert_eq!(padded(16, 32), (16, 32), "a multiple is left alone");
    }

    #[test]
    fn the_corner_of_the_padding_repeats_the_corner_pixel() {
        // 2x2 image, every value distinct, padded to 16x16.
        let rgb: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let (pw, ph) = padded(2, 2);
        let planar = to_planar_padded(&rgb, 2, 2, pw, ph);
        assert_eq!(planar.len(), 3 * 16 * 16);
        for c in 0..3 {
            let plane = &planar[c * 256..(c + 1) * 256];
            // Bottom-right pixel (1, 1) is rgb[9 + c]; the far corner copies it.
            assert_eq!(plane[15 * 16 + 15], rgb[9 + c]);
            // Row 0 past the image repeats pixel (1, 0); column 0 below it repeats (0, 1).
            assert_eq!(plane[7], rgb[3 + c]);
            assert_eq!(plane[7 * 16], rgb[6 + c]);
        }
        assert_eq!(from_planar_cropped(&planar, 2, 2, pw, ph), rgb);
    }

    #[test]
    fn check_input_refuses_empty_and_mismatched_buffers() {
        assert!(check_input(&[0.0; 12], 2, 2).is_ok());
        assert!(check_input(&[], 0, 4).is_err());
        assert!(check_input(&[], 4, 0).is_err());
        let err = check_input(&[0.0; 11], 2, 2).unwrap_err();
        assert_eq!(err, "restorer input is 11 floats, want 2x2x3");
    }
}
