//! Exact-area (box filter) resampling of RGBA rasters.
//!
//! Intake uses this to cap an oversized input (`--max-dim`) and to bring an oversampled
//! one back to the resolution its drawing actually needs (`inkvec-cli`'s intake, and the
//! crate root's decode-time cap, which reads the decoder's 8-bit buffer directly). It
//! lives beside [`super::Rgba`] because it produces one; it was moved out of `coverage.rs`
//! unchanged apart from sharing one integration loop between the f32 and 8-bit readers.
//!
//! Coordinates here are *pixel-edge* coordinates: source pixel `(x, y)` covers the unit
//! square `[x, x+1) x [y, y+1)`, and target pixel `(ox, oy)` covers
//! `[ox*sx, (ox+1)*sx) x [oy*sy, (oy+1)*sy)` in source units, with `sx = w/nw` and
//! `sy = h/nh`.

use super::Rgba;

/// Area-average an image down to `nw` x `nh` via exact continuous 2D area integration.
///
/// For non-integer downsampling ratios `(sx, sy)`, target pixel cells partition continuous
/// source space with exact area weights summing to `sx * sy`. Source pixels spanning multiple
/// target cells are partitioned proportionally according to continuous box cell overlap,
/// eliminating pixel double-counting and periodic spatial aliasing ripples along edges.
///
/// Colour is averaged premultiplied and then un-premultiplied, so a transparent
/// pixel's stored colour cannot bleed into its neighbours as a dark halo — which
/// downstream would become a traced contour that is not in the artwork.
///
/// Returns a clone of the input when there is nothing to do (same size) or nothing that
/// can be done safely (a zero dimension, or a buffer shorter than `w * h * 4`). Also works
/// as an upsample (`nw > w`), where each target cell averages the one or two source pixels
/// it straddles.
pub fn downsample_to(img: &Rgba, nw: usize, nh: usize) -> Rgba {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || nw == 0 || nh == 0 || (nw == w && nh == h) || img.data.len() < w * h * 4
    {
        return img.clone();
    }
    box_resample(w, h, nw, nh, |x, y| {
        let p = img.pixel(x, y);
        [p[0] as f64, p[1] as f64, p[2] as f64, p[3] as f64]
    })
}

/// Exact-area (box) downsample of an 8-bit RGBA buffer to `nw x nh` — the same operator
/// [`downsample_to`] applies to an f32 [`Rgba`], read straight from the decoder's 8-bit
/// buffer so the decode-time `--max-dim` cap never has to materialise the full-resolution
/// f32 image.
///
/// The arithmetic is identical to [`downsample_to`]: each target pixel is the exact
/// area-weighted average of the source pixels under its footprint, with fractional weights
/// on the boundary pixels, colour averaged premultiplied and then un-premultiplied. The only
/// difference is that source channels are read at 8 bits per channel, which differs from the
/// f32 path by at most the 8-bit quantisation of the input.
///
/// With a zero dimension or no size change it only converts to f32 (`v / 255`). The caller
/// guarantees `src.len() >= w * h * 4`.
pub fn box_downsample_rgba8(src: &[u8], w: usize, h: usize, nw: usize, nh: usize) -> Rgba {
    if w == 0 || h == 0 || nw == 0 || nh == 0 || (nw == w && nh == h) {
        let mut data = vec![0.0f32; w * h * 4];
        for (i, &b) in src.iter().take(w * h * 4).enumerate() {
            data[i] = b as f32 / 255.0;
        }
        return Rgba {
            width: w,
            height: h,
            data,
        };
    }
    box_resample(w, h, nw, nh, |x, y| {
        let p = (y * w + x) * 4;
        [
            src[p] as f64 / 255.0,
            src[p + 1] as f64 / 255.0,
            src[p + 2] as f64 / 255.0,
            src[p + 3] as f64 / 255.0,
        ]
    })
}

/// The shared integration loop behind [`downsample_to`] and [`box_downsample_rgba8`].
///
/// For each target cell `T = [x_start, x_end) x [y_start, y_end)` (in source pixel units)
/// it sums over every source pixel `S` that overlaps it:
///
/// ```text
///     wgt(S)  = |S ∩ T|                         (overlap area, px²)
///     alpha_T = Σ wgt(S)·a_S / Σ wgt(S)
///     c_T     = Σ wgt(S)·a_S·c_S / Σ wgt(S)·a_S (premultiplied, then un-premultiplied)
/// ```
///
/// so it is the box filter of width `sx x sy` evaluated exactly, not by point sampling.
/// The last row and column end at exactly `h` and `w` rather than at `n*s`, so rounding in
/// `s` cannot leave a sliver of the source outside every cell.
///
/// `fetch(x, y)` returns straight (not premultiplied) RGBA in `0..1`. Results are clamped
/// to `0..1`; a non-finite value (a NaN in the source) becomes 0, and a cell with no alpha
/// at all gets colour 0.
fn box_resample(
    w: usize,
    h: usize,
    nw: usize,
    nh: usize,
    fetch: impl Fn(usize, usize) -> [f64; 4],
) -> Rgba {
    let mut data = vec![0.0f32; nw * nh * 4];
    let sx = w as f64 / nw as f64;
    let sy = h as f64 / nh as f64;

    for oy in 0..nh {
        let y_start = oy as f64 * sy;
        let y_end = if oy + 1 == nh {
            h as f64
        } else {
            (oy + 1) as f64 * sy
        };
        let y0 = (y_start.floor() as usize).min(h);
        let y1 = (y_end.ceil() as usize).min(h);

        for ox in 0..nw {
            let x_start = ox as f64 * sx;
            let x_end = if ox + 1 == nw {
                w as f64
            } else {
                (ox + 1) as f64 * sx
            };
            let x0 = (x_start.floor() as usize).min(w);
            let x1 = (x_end.ceil() as usize).min(w);

            let (mut acc, mut a_sum, mut total_weight) = ([0.0f64; 3], 0.0f64, 0.0f64);

            for y in y0..y1 {
                let wy = ((y + 1) as f64).min(y_end) - (y as f64).max(y_start);
                if wy <= 0.0 {
                    continue;
                }
                for x in x0..x1 {
                    let wx = ((x + 1) as f64).min(x_end) - (x as f64).max(x_start);
                    if wx <= 0.0 {
                        continue;
                    }
                    let weight = wx * wy;
                    total_weight += weight;
                    let p = fetch(x, y);
                    let wa = p[3] * weight;
                    for c in 0..3 {
                        acc[c] += p[c] * wa;
                    }
                    a_sum += wa;
                }
            }

            let o = (oy * nw + ox) * 4;
            let alpha = if total_weight > 0.0 {
                let a = (a_sum / total_weight) as f32;
                if a.is_finite() {
                    a.clamp(0.0, 1.0)
                } else {
                    0.0
                }
            } else {
                0.0
            };
            for c in 0..3 {
                data[o + c] = if a_sum > 1e-9 {
                    let v = (acc[c] / a_sum) as f32;
                    if v.is_finite() {
                        v.clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
            }
            data[o + 3] = alpha;
        }
    }

    Rgba {
        width: nw,
        height: nh,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_to_is_identity_at_the_same_size() {
        let img = Rgba {
            width: 8,
            height: 8,
            data: (0..8 * 8 * 4).map(|i| (i % 255) as f32 / 255.0).collect(),
        };
        let same = downsample_to(&img, 8, 8);
        assert_eq!(same.data, img.data);
    }

    #[test]
    fn downsample_does_not_bleed_colour_from_transparent_pixels() {
        // The halo bug this function's doc comment exists to prevent: a transparent pixel
        // storing black must not darken an opaque white neighbour.
        let (w, h) = (4, 4);
        let mut data = vec![0.0f32; w * h * 4];
        for i in 0..w * h {
            let opaque = i % 2 == 0;
            let px = if opaque {
                [1.0, 1.0, 1.0, 1.0]
            } else {
                [0.0, 0.0, 0.0, 0.0]
            };
            data[i * 4..i * 4 + 4].copy_from_slice(&px);
        }
        let small = downsample_to(
            &Rgba {
                width: w,
                height: h,
                data,
            },
            2,
            2,
        );
        for i in 0..4 {
            let c = &small.data[i * 4..i * 4 + 3];
            assert!(
                c.iter().all(|&v| v > 0.99),
                "transparent black bled into the average: {c:?}"
            );
        }
    }

    #[test]
    fn downsample_non_integer_ratio_partitions_pixels_proportionally() {
        // 5x5 down to 2x2: sx = 2.5, sy = 2.5
        // A single delta pixel at (2, 2) spans continuous coordinates [2, 3] x [2, 3].
        // Target cells:
        // (0, 0): [0, 2.5] x [0, 2.5] -> overlaps [2, 2.5] x [2, 2.5] -> area 0.25
        // (1, 0): [2.5, 5] x [0, 2.5] -> overlaps [2.5, 3] x [2, 2.5] -> area 0.25
        // (0, 1): [0, 2.5] x [2.5, 5] -> overlaps [2, 2.5] x [2.5, 3] -> area 0.25
        // (1, 1): [2.5, 5] x [2.5, 5] -> overlaps [2.5, 3] x [2.5, 3] -> area 0.25
        // Total area = 1.0 (exact partition of unity without double-counting).
        let (w, h) = (5, 5);
        let mut data = vec![0.0f32; w * h * 4];
        let p_idx = (2 * w + 2) * 4;
        data[p_idx] = 1.0;
        data[p_idx + 1] = 1.0;
        data[p_idx + 2] = 1.0;
        data[p_idx + 3] = 1.0;

        let img = Rgba {
            width: w,
            height: h,
            data,
        };
        let small = downsample_to(&img, 2, 2);

        // Each target cell area is sx * sy = 2.5 * 2.5 = 6.25.
        // The expected alpha in each target pixel is 0.25 / 6.25 = 0.04.
        let expected_alpha = 0.25 / 6.25;
        let mut total_alpha = 0.0f64;
        for i in 0..4 {
            let a = small.data[i * 4 + 3] as f64;
            total_alpha += a;
            assert!(
                (a - expected_alpha).abs() < 1e-6,
                "target pixel {i} alpha {a} != expected {expected_alpha}"
            );
            // Color should remain [1.0, 1.0, 1.0] without darkening
            for c in 0..3 {
                assert!(
                    (small.data[i * 4 + c] - 1.0).abs() < 1e-5,
                    "target pixel {i} channel {c} corrupted"
                );
            }
        }
        // Total alpha integrated over all target cells: sum(a * 6.25) = 4 * (0.04 * 6.25) = 1.0
        assert!((total_alpha * 6.25 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn downsample_ramp_matches_piecewise_constant_integral() {
        // Pixel values are constant on each source cell, not a continuous ramp.
        let (w, h, nw, nh) = (100, 4, 41, 3);
        let mut data = vec![0.0; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                data[i..i + 3].fill(x as f32 / w as f32);
                data[i + 3] = 1.0;
            }
        }
        let small = downsample_to(
            &Rgba {
                width: w,
                height: h,
                data,
            },
            nw,
            nh,
        );
        // Closed-form antiderivative of floor(x)/w, independent of overlap code.
        let integral = |x: f64| {
            let n = x.floor();
            (n * (n - 1.0) / 2.0 + n * (x - n)) / w as f64
        };
        let sx = w as f64 / nw as f64;
        for y in 0..nh {
            for x in 0..nw {
                let expected = (integral((x + 1) as f64 * sx) - integral(x as f64 * sx)) / sx;
                assert!((small.pixel(x, y)[0] as f64 - expected).abs() < 1e-7);
            }
        }
    }

    #[test]
    fn downsample_partitions_every_source_impulse() {
        // Exercise the real downsampler for every source pixel, including borders,
        // with asymmetric fractional ratios and mixed up/downsampling.
        for (w, h, nw, nh) in [(7, 5, 3, 2), (11, 7, 4, 3), (5, 3, 2, 7)] {
            let area = (w * h) as f64 / (nw * nh) as f64;
            let mut target_sums = vec![0.0; nw * nh];
            for source in 0..w * h {
                let mut data = vec![0.0; w * h * 4];
                data[source * 4..source * 4 + 4].fill(1.0);
                let small = downsample_to(
                    &Rgba {
                        width: w,
                        height: h,
                        data,
                    },
                    nw,
                    nh,
                );
                let mut mass = 0.0;
                for (i, sum) in target_sums.iter_mut().enumerate() {
                    let alpha = small.data[i * 4 + 3] as f64;
                    mass += alpha * area;
                    *sum += alpha;
                }
                assert!((mass - 1.0).abs() < 1e-6, "source {source}: {mass}");
            }
            for sum in target_sums {
                assert!((sum - 1.0).abs() < 1e-6, "target coverage {sum}");
            }
        }
    }

    #[test]
    fn downsample_edge_dimensions_preserve_safety() {
        // 1x1 image downsampled / upsampled
        let one = Rgba {
            width: 1,
            height: 1,
            data: vec![0.5, 0.4, 0.3, 0.8],
        };
        let down1 = downsample_to(&one, 1, 1);
        assert_eq!(down1.data, one.data);

        let up2 = downsample_to(&one, 2, 2);
        assert_eq!(up2.width, 2);
        assert_eq!(up2.height, 2);
        for i in 0..4 {
            assert!((up2.data[i * 4] - 0.5).abs() < 1e-5);
            assert!((up2.data[i * 4 + 1] - 0.4).abs() < 1e-5);
            assert!((up2.data[i * 4 + 2] - 0.3).abs() < 1e-5);
            assert!((up2.data[i * 4 + 3] - 0.8).abs() < 1e-5);
        }

        // Large to 1x1
        let img = Rgba {
            width: 7,
            height: 5,
            data: vec![1.0; 7 * 5 * 4],
        };
        let down_to_one = downsample_to(&img, 1, 1);
        assert_eq!(down_to_one.width, 1);
        assert_eq!(down_to_one.height, 1);
        assert!((down_to_one.data[0] - 1.0).abs() < 1e-5);
        assert!((down_to_one.data[3] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn downsample_conserves_total_energy_for_arbitrary_non_integer_ratios() {
        let (w, h) = (37, 23);
        let mut data = vec![0.0f32; w * h * 4];
        let mut src_sum = 0.0f64;
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 7 + y * 13) % 256) as f32 / 255.0;
                let a = ((x * 11 + y * 5) % 256) as f32 / 255.0;
                let i = (y * w + x) * 4;
                data[i] = v;
                data[i + 1] = v;
                data[i + 2] = v;
                data[i + 3] = a;
                src_sum += (v * a) as f64;
            }
        }
        let img = Rgba {
            width: w,
            height: h,
            data,
        };
        let (nw, nh) = (17, 11);
        let small = downsample_to(&img, nw, nh);

        let sx = w as f64 / nw as f64;
        let sy = h as f64 / nh as f64;
        let cell_area = sx * sy;
        let mut dst_sum = 0.0f64;
        for oy in 0..nh {
            for ox in 0..nw {
                let i = (oy * nw + ox) * 4;
                let v = small.data[i] as f64;
                let a = small.data[i + 3] as f64;
                dst_sum += v * a * cell_area;
            }
        }
        let rel_err = (dst_sum - src_sum).abs() / src_sum;
        assert!(
            rel_err < 1e-5,
            "energy not conserved: src={src_sum}, dst={dst_sum}, rel_err={rel_err}"
        );
    }

    #[test]
    fn downsample_zero_dimensions_or_truncated_buffer_is_safe() {
        // Zero dimensions
        let empty_w = Rgba {
            width: 0,
            height: 5,
            data: vec![],
        };
        let out = downsample_to(&empty_w, 10, 10);
        assert_eq!(out.width, 0);

        let empty_h = Rgba {
            width: 5,
            height: 0,
            data: vec![],
        };
        let out = downsample_to(&empty_h, 10, 10);
        assert_eq!(out.height, 0);

        let valid = Rgba {
            width: 4,
            height: 4,
            data: vec![0.5; 64],
        };
        let out_zero_nw = downsample_to(&valid, 0, 4);
        assert_eq!(out_zero_nw.width, 4);

        let out_zero_nh = downsample_to(&valid, 4, 0);
        assert_eq!(out_zero_nh.height, 4);

        // Truncated data buffer
        let truncated = Rgba {
            width: 4,
            height: 4,
            data: vec![0.5; 10],
        };
        let out_trunc = downsample_to(&truncated, 2, 2);
        assert_eq!(out_trunc.data.len(), 10);
    }

    #[test]
    fn downsample_anisotropic_scaling_conserves_weights() {
        // Downsampling along X (sx = 3.333), upsampling along Y (sy = 0.25)
        let (w, h) = (10, 2);
        let (nw, nh) = (3, 8);
        let mut data = vec![0.0f32; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                data[i] = 0.8;
                data[i + 1] = 0.4;
                data[i + 2] = 0.2;
                data[i + 3] = 1.0;
            }
        }
        let img = Rgba {
            width: w,
            height: h,
            data,
        };
        let out = downsample_to(&img, nw, nh);
        assert_eq!(out.width, nw);
        assert_eq!(out.height, nh);
        for i in 0..nw * nh {
            assert!((out.data[i * 4] - 0.8).abs() < 1e-5);
            assert!((out.data[i * 4 + 1] - 0.4).abs() < 1e-5);
            assert!((out.data[i * 4 + 2] - 0.2).abs() < 1e-5);
            assert!((out.data[i * 4 + 3] - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn downsample_solid_color_and_transparent_pixels_preserve_exact_invariants() {
        // Solid opaque color
        let (w, h) = (33, 19);
        let (nw, nh) = (11, 7);
        let data = [0.7f32, 0.2, 0.9, 1.0].repeat(w * h);
        let img = Rgba {
            width: w,
            height: h,
            data,
        };
        let out = downsample_to(&img, nw, nh);
        for i in 0..nw * nh {
            assert!((out.data[i * 4] - 0.7).abs() < 1e-5);
            assert!((out.data[i * 4 + 1] - 0.2).abs() < 1e-5);
            assert!((out.data[i * 4 + 2] - 0.9).abs() < 1e-5);
            assert!((out.data[i * 4 + 3] - 1.0).abs() < 1e-5);
        }

        // Fully transparent pixels
        let data_trans = [1.0f32, 0.5, 0.2, 0.0].repeat(w * h);
        let img_trans = Rgba {
            width: w,
            height: h,
            data: data_trans,
        };
        let out_trans = downsample_to(&img_trans, nw, nh);
        for i in 0..nw * nh {
            assert_eq!(out_trans.data[i * 4 + 3], 0.0);
            assert_eq!(out_trans.data[i * 4], 0.0);
            assert_eq!(out_trans.data[i * 4 + 1], 0.0);
            assert_eq!(out_trans.data[i * 4 + 2], 0.0);
        }
    }

    /// The decode-time box cap is the *same operator* as the f32 exact-area downsample, read
    /// from an 8-bit source. The only difference allowed is the 8-bit quantisation of the
    /// input, so the two must agree to within a couple of levels.
    #[test]
    fn box_downsample_rgba8_matches_downsample_to_within_8_bit_rounding() {
        for &(w, h, nw, nh) in &[(37, 23, 17, 11), (64, 48, 8, 8), (40, 40, 15, 15)] {
            let mut seed = 7u32;
            let mut next = move || {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 8) as f64 + 0.5) / 16777216.0
            };
            let src = {
                let mut data = vec![0.0f32; w * h * 4];
                for p in 0..w * h {
                    for c in 0..3 {
                        data[p * 4 + c] = next() as f32;
                    }
                    data[p * 4 + 3] = (0.5 + 0.5 * next()) as f32;
                }
                Rgba {
                    width: w,
                    height: h,
                    data,
                }
            };
            let u8buf: Vec<u8> = src
                .data
                .iter()
                .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
                .collect();

            let reference = downsample_to(&src, nw, nh);
            let from_u8 = box_downsample_rgba8(&u8buf, w, h, nw, nh);
            assert_eq!((from_u8.width, from_u8.height), (nw, nh));
            for i in 0..nw * nh * 4 {
                let (a, b) = (reference.data[i] as f64, from_u8.data[i] as f64);
                assert!(
                    (a - b).abs() < 2.0 / 255.0,
                    "{w}x{h} -> {nw}x{nh}: channel {i}: f32 {a} vs u8 {b}"
                );
            }
        }
    }
}
