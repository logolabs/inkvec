//! The pre-pass arithmetic: box downsample, and putting the colours back.
//!
//! The upscaler removes about two thirds of a JPEG's error energy -- not because it was trained to, it was
//! not, but because it was fitted on flat-colour logo art and ringing is not on
//! that manifold, so it cannot represent the damage.
//!
//! Three steps, each here for a measured reason.
//!
//! **Upscale x4** is what cleans. Artefact gain -- what survives the network
//! relative to what went in -- is 0.36 across seven families.
//!
//! **Box-downsample x2** is not a denoiser, whatever it looks like. A 2x2 box
//! has its only transfer zero at Nyquist, and the DCT grid puts JPEG damage at
//! f = 1/8 cycles/pixel and below, which it passes at 0.974 of amplitude: half
//! the error survives, and the surviving half is the *more* coherent half. It is
//! here because x2 is the useful output scale, not because it cleans.
//!
//! **Recolour** exists because the network costs ~0.6 dE00 on clean input too.
//! Split by region against bicubic as a control, that is 0.60 on flat interiors
//! where bicubic scores 0.03, and 1.57 on edges where bicubic scores 8.63. Each
//! resampler is right about the half the other is wrong about, so take geometry
//! from the network and colour from the source.

use inkvec_trace::Rgba;

/// Exact box average by an integer factor. Alpha rides along with the colour.
///
/// Output pixel `(x, y)` is the plain mean of the `k x k` input block whose top-left
/// corner is `(k·x, k·y)`, for `k = factor`:
///
/// ```text
/// out(x, y) = (1 / k²) Σ_{dx, dy ∈ 0..k} in(k·x + dx, k·y + dy)
/// ```
///
/// All four channels are averaged as stored (straight alpha, not premultiplied), which is
/// what the reference Python pre-pass does. The output is `floor(w / k) x floor(h / k)`:
/// rows and columns that do not fill a whole block are dropped, which never happens for an
/// upscaler's output since its size is a multiple of `k`. Panics when `factor` is 0.
pub fn box_downsample(img: &Rgba, factor: usize) -> Rgba {
    assert!(factor >= 1, "factor must be positive");
    if factor == 1 {
        return img.clone();
    }
    let (w, h) = (img.width / factor, img.height / factor);
    let mut data = vec![0.0f32; w * h * 4];
    let inv = 1.0 / (factor * factor) as f32;
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for dy in 0..factor {
                for dx in 0..factor {
                    let p = img.pixel(x * factor + dx, y * factor + dy);
                    for c in 0..4 {
                        acc[c] += p[c];
                    }
                }
            }
            let o = (y * w + x) * 4;
            for c in 0..4 {
                data[o + c] = acc[c] * inv;
            }
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

/// Bicubic resample to an explicit size. Catmull-Rom (a = -0.5), matching what
/// PIL calls BICUBIC, because the recolour map is fitted against it and a
/// different kernel would move the flat mask.
///
/// This is Keys' cubic convolution. The images are aligned by their outer edges: output
/// pixel `x`, whose centre is at `x + 0.5` in continuous coordinates, maps to the input
/// position `fx = (x + 0.5)·W/w − 0.5`, measured in input pixel indices (input pixel `i`
/// sits at `fx = i`). Each output is the weighted sum of the 4 x 4 input pixels around
/// `(fx, fy)` with the separable kernel
///
/// ```text
/// W(t) = (a+2)|t|³ − (a+3)|t|² + 1          for |t| ≤ 1
///      = a(|t|³ − 5|t|² + 8|t| − 4)         for 1 < |t| < 2
///      = 0                                  otherwise,   a = −0.5
/// ```
///
/// Samples outside the image are clamped to the nearest edge pixel. The sum is divided by
/// the total weight (which is 1 up to rounding for this kernel) and clamped to `0..1`,
/// since the kernel's negative lobes can overshoot at edges. The kernel is applied without
/// widening when downscaling, so it is a proper resampler only for upsampling, which is
/// the one use here.
pub fn bicubic(img: &Rgba, w: usize, h: usize) -> Rgba {
    fn weight(t: f32) -> f32 {
        const A: f32 = -0.5;
        let t = t.abs();
        if t <= 1.0 {
            (A + 2.0) * t * t * t - (A + 3.0) * t * t + 1.0
        } else if t < 2.0 {
            A * (t * t * t - 5.0 * t * t + 8.0 * t - 4.0)
        } else {
            0.0
        }
    }
    let mut data = vec![0.0f32; w * h * 4];
    let sx = img.width as f32 / w as f32;
    let sy = img.height as f32 / h as f32;
    for y in 0..h {
        let fy = (y as f32 + 0.5) * sy - 0.5;
        let iy = fy.floor() as isize;
        for x in 0..w {
            let fx = (x as f32 + 0.5) * sx - 0.5;
            let ix = fx.floor() as isize;
            let mut acc = [0.0f32; 4];
            let mut wsum = 0.0f32;
            for m in -1..=2isize {
                let wy = weight(fy - (iy + m) as f32);
                if wy == 0.0 {
                    continue;
                }
                let yy = (iy + m).clamp(0, img.height as isize - 1) as usize;
                for n in -1..=2isize {
                    let wx = weight(fx - (ix + n) as f32);
                    if wx == 0.0 {
                        continue;
                    }
                    let xx = (ix + n).clamp(0, img.width as isize - 1) as usize;
                    let p = img.pixel(xx, yy);
                    let ww = wx * wy;
                    for c in 0..4 {
                        acc[c] += p[c] * ww;
                    }
                    wsum += ww;
                }
            }
            let o = (y * w + x) * 4;
            let inv = if wsum.abs() > 1e-8 { 1.0 / wsum } else { 0.0 };
            for c in 0..4 {
                data[o + c] = (acc[c] * inv).clamp(0.0, 1.0);
            }
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

/// True where the 3x3 neighbourhood of the luma spans less than `tol`. Border
/// pixels are never flat: they have no full neighbourhood to be judged on.
///
/// "Luma" here is the plain mean `(R + G + B) / 3` of the stored channels, not a
/// perceptual weighting, and alpha is ignored; `tol` is in the same `0..1` units. The
/// mask is row-major, one entry per pixel, and all `false` for images narrower or shorter
/// than 3 pixels.
pub fn flat_mask(img: &Rgba, tol: f32) -> Vec<bool> {
    let (w, h) = (img.width, img.height);
    let luma: Vec<f32> = (0..w * h)
        .map(|i| {
            let p = &img.data[i * 4..i * 4 + 3];
            (p[0] + p[1] + p[2]) / 3.0
        })
        .collect();
    let mut mask = vec![false; w * h];
    if w < 3 || h < 3 {
        return mask;
    }
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for dy in 0..3 {
                for dx in 0..3 {
                    let v = luma[(y + dy - 1) * w + (x + dx - 1)];
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            mask[y * w + x] = hi - lo < tol;
        }
    }
    mask
}

/// Put the upscaler's flat regions back on the source's colours.
///
/// One affine map per channel, fitted from the upscaled image to a bicubic
/// upsample of the same input, over the pixels the bicubic image says are flat.
/// Uses nothing but the input, so it is available at inference; the artist's
/// file is never consulted.
///
/// For each colour channel this is ordinary least squares for `y ≈ a·x + b`, where `x` is
/// the upscaled value and `y` the bicubic value at the same pixel, over the flat pixels
/// `M`. The closed form from the normal equations, with `n = |M|`, is
///
/// ```text
/// a = (n Σxy − Σx Σy) / (n Σx² − (Σx)²),    b = (Σy − a Σx) / n
/// ```
///
/// and every pixel of `hi` is then replaced by `clamp(a·x + b, 0, 1)`. Alpha is left
/// alone. The flat mask is the bicubic image's with a 3-level tolerance; when under 1% of
/// the pixels qualify, every pixel is used instead. A channel is left untouched when fewer
/// than 16 pixels are fitted or the denominator is near zero (the upscaled channel is
/// constant, so it carries no slope).
///
/// `hi` must be the size the bicubic is taken to; `src` is the source it was upscaled from.
pub fn match_flats(hi: &mut Rgba, src: &Rgba) {
    const TOL: f32 = 3.0 / 255.0;
    let bi = bicubic(src, hi.width, hi.height);
    let mut mask = flat_mask(&bi, TOL);
    let n_flat = mask.iter().filter(|&&m| m).count();
    if (n_flat as f64) < 0.01 * mask.len() as f64 {
        // Nothing flat enough to fit on; use every pixel rather than nothing.
        mask.iter_mut().for_each(|m| *m = true);
    }

    for c in 0..3 {
        // Least squares for y = a*x + b over the masked pixels, in f64: the
        // normal equations of a two-parameter fit over ~10^5 samples lose more
        // than f32 has to give.
        let (mut sx, mut sy, mut sxx, mut sxy, mut n) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0u64);
        for (i, &m) in mask.iter().enumerate() {
            if !m {
                continue;
            }
            let x = hi.data[i * 4 + c] as f64;
            let y = bi.data[i * 4 + c] as f64;
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
            n += 1;
        }
        if n < 16 {
            continue;
        }
        let nf = n as f64;
        let det = nf * sxx - sx * sx;
        if det.abs() < 1e-9 {
            continue; // a constant channel carries no slope to fit
        }
        let a = (nf * sxy - sx * sy) / det;
        let b = (sy - a * sx) / nf;
        for i in 0..hi.width * hi.height {
            let v = hi.data[i * 4 + c] as f64;
            hi.data[i * 4 + c] = ((a * v + b) as f32).clamp(0.0, 1.0);
        }
    }
}

/// Composite onto white, dropping alpha: `c' = c·α + (1 − α)` per channel, on the stored
/// sRGB values (the blend is done in gamma-encoded space, as a browser composites).
pub fn on_white(img: &Rgba) -> Vec<[f32; 3]> {
    (0..img.width * img.height)
        .map(|i| {
            let p = &img.data[i * 4..i * 4 + 4];
            [
                p[0] * p[3] + (1.0 - p[3]),
                p[1] * p[3] + (1.0 - p[3]),
                p[2] * p[3] + (1.0 - p[3]),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, c: [f32; 4]) -> Rgba {
        Rgba {
            width: w,
            height: h,
            data: (0..w * h).flat_map(|_| c).collect(),
        }
    }

    #[test]
    fn box_averages_exactly() {
        let mut img = solid(4, 4, [0.0, 0.0, 0.0, 1.0]);
        // Red channel of the pixel at (x, y) on a 4-wide RGBA image.
        let red = |x: usize, y: usize| (y * 4 + x) * 4;
        // One 2x2 block set to 1.0 in red, so its average must be exactly 1.0,
        // and a block with a single 1.0 must average to exactly 0.25.
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            img.data[red(x, y)] = 1.0;
        }
        img.data[red(2, 0)] = 1.0;
        let d = box_downsample(&img, 2);
        assert_eq!(d.width, 2);
        assert!((d.pixel(0, 0)[0] - 1.0).abs() < 1e-6);
        assert!((d.pixel(1, 0)[0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn box_by_one_is_a_copy_and_a_ragged_edge_is_dropped() {
        let img = solid(5, 3, [0.2, 0.4, 0.6, 0.8]);
        let same = box_downsample(&img, 1);
        assert_eq!((same.width, same.height, &same.data), (5, 3, &img.data));
        let d = box_downsample(&img, 2);
        assert_eq!(
            (d.width, d.height),
            (2, 1),
            "the fifth column and third row are dropped"
        );
        assert!(d
            .data
            .iter()
            .zip([0.2, 0.4, 0.6, 0.8])
            .all(|(a, b)| (a - b).abs() < 1e-6));
    }

    #[test]
    fn bicubic_interpolates_a_ramp_and_clamps_overshoot() {
        // A horizontal ramp 0, 1/3, 2/3, 1 upsampled x2: interior samples fall between
        // their neighbours, and the step at the edge cannot overshoot 0..1.
        let mut img = solid(4, 1, [0.0, 0.0, 0.0, 1.0]);
        for x in 0..4 {
            img.data[x * 4] = x as f32 / 3.0;
        }
        let up = bicubic(&img, 8, 2);
        assert_eq!((up.width, up.height), (8, 2));
        let row: Vec<f32> = (0..8).map(|x| up.pixel(x, 0)[0]).collect();
        assert!(
            row.windows(2).all(|w| w[1] >= w[0] - 1e-6),
            "monotone: {row:?}"
        );
        assert!(row.iter().all(|v| (0.0..=1.0).contains(v)));
        let mut step = solid(4, 1, [0.0, 0.0, 0.0, 1.0]);
        step.data[8] = 1.0;
        step.data[12] = 1.0;
        let up = bicubic(&step, 16, 1);
        assert!(up.data.iter().all(|v| (0.0..=1.0).contains(v)), "clamped");
    }

    #[test]
    fn a_tiny_image_has_no_flat_pixels() {
        let img = solid(2, 5, [0.5, 0.5, 0.5, 1.0]);
        assert!(flat_mask(&img, 1.0).iter().all(|&m| !m));
    }

    #[test]
    fn recolour_leaves_a_constant_channel_alone() {
        // An upscale whose channels are constant has no slope to fit; it must survive
        // unchanged rather than divide by zero.
        let src = solid(8, 8, [0.25, 0.25, 0.25, 1.0]);
        let mut hi = solid(16, 16, [0.5, 0.5, 0.5, 1.0]);
        match_flats(&mut hi, &src);
        assert!(hi.data.iter().all(|v| v.is_finite()));
        assert_eq!(hi.pixel(5, 5), [0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn on_white_blends_by_alpha() {
        let img = Rgba {
            width: 2,
            height: 1,
            data: vec![0.0, 0.5, 1.0, 1.0, 0.0, 0.0, 0.0, 0.25],
        };
        let c = on_white(&img);
        assert_eq!(c[0], [0.0, 0.5, 1.0]);
        assert_eq!(c[1], [0.75, 0.75, 0.75]);
    }

    #[test]
    fn bicubic_preserves_a_constant() {
        let img = solid(8, 8, [0.25, 0.5, 0.75, 1.0]);
        let up = bicubic(&img, 32, 32);
        for i in 0..up.width * up.height {
            assert!((up.data[i * 4] - 0.25).abs() < 1e-5, "at {i}");
            assert!((up.data[i * 4 + 2] - 0.75).abs() < 1e-5, "at {i}");
        }
    }

    #[test]
    fn recolour_inverts_a_known_bias() {
        // A flat source, and an "upscale" of it carrying a linear distortion.
        // The fit must undo the distortion exactly.
        let mut src = solid(16, 16, [0.4, 0.4, 0.4, 1.0]);
        for i in 0..16 * 16 {
            let v = 0.2 + 0.6 * ((i % 16) as f32 / 15.0);
            src.data[i * 4] = v;
            src.data[i * 4 + 1] = v;
            src.data[i * 4 + 2] = v;
        }
        let mut hi = bicubic(&src, 32, 32);
        for i in 0..32 * 32 {
            for c in 0..3 {
                hi.data[i * 4 + c] = (hi.data[i * 4 + c] * 0.9 + 0.05).clamp(0.0, 1.0);
            }
        }
        let want = bicubic(&src, 32, 32);
        match_flats(&mut hi, &src);
        let err = (0..32 * 32)
            .map(|i| (hi.data[i * 4] - want.data[i * 4]).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 2e-2, "residual bias after recolour: {err}");
    }

    #[test]
    fn flat_mask_finds_the_flat_part() {
        let mut img = solid(16, 16, [0.5, 0.5, 0.5, 1.0]);
        for y in 0..16 {
            for x in 8..16 {
                for c in 0..3 {
                    img.data[(y * 16 + x) * 4 + c] = if x % 2 == 0 { 0.0 } else { 1.0 };
                }
            }
        }
        let m = flat_mask(&img, 3.0 / 255.0);
        assert!(m[8 * 16 + 3], "the constant half should be flat");
        assert!(!m[8 * 16 + 11], "the striped half should not be");
    }
}
