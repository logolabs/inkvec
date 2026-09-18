//! Layout conversion shared by every in-process backend: interleaved RGB in, planar CHW padded
//! to a multiple of 16 for the network, and back.
//!
//! Padding replicates the last row and column, the same `F.pad(..., mode="replicate")` on the
//! bottom and right that `restore_cli.py` applies, so every backend agrees with the Python
//! reference pixel for pixel.

/// Both sides of the network input must be multiples of this (four 2x downsamplings).
pub const MULTIPLE: usize = 16;

/// The padded size for a `width x height` input.
#[cfg_attr(not(any(feature = "model", feature = "onnxruntime")), allow(dead_code))]
pub(crate) fn padded(width: usize, height: usize) -> (usize, usize) {
    (
        width.div_ceil(MULTIPLE) * MULTIPLE,
        height.div_ceil(MULTIPLE) * MULTIPLE,
    )
}

/// Interleaved RGB `width x height` to planar CHW `pw x ph`, replicating the last column and
/// row into the padding.
#[cfg_attr(not(any(feature = "model", feature = "onnxruntime")), allow(dead_code))]
pub(crate) fn to_planar_padded(
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
#[cfg_attr(not(any(feature = "model", feature = "onnxruntime")), allow(dead_code))]
pub(crate) fn from_planar_cropped(
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

/// Check an input buffer before any backend sees it.
#[cfg_attr(not(any(feature = "model", feature = "onnxruntime")), allow(dead_code))]
pub(crate) fn check_input(rgb: &[f32], width: usize, height: usize) -> Result<(), String> {
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
    }
}
