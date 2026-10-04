//! Tests of the intake functions in [`super`]: each against the serial version it
//! replaced, bit for bit.

use super::*;
use inkvec_trace::Rgba;

/// Deterministic pseudo-random numbers (a 64-bit LCG).
fn lcg(s: &mut u64) -> u64 {
    *s = s
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *s >> 33
}

/// The integer-only `pixel_grid` that the lattice inverse replaced: every divisor of both
/// sides from the largest down (at most 32, and leaving at least 64 px on each side), each
/// with the block test -- every channel of every pixel of every `k x k` block within `1/512`
/// of the block's first pixel. Kept as an oracle for the factors both find.
fn old_pixel_grid(img: &Rgba) -> Option<usize> {
    const MAX_FACTOR: usize = 32;
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return None;
    }
    let blocks_constant = |k: usize| {
        let px = |x: usize, y: usize| &img.data[(y * w + x) * 4..(y * w + x) * 4 + 4];
        (0..h / k).all(|by| {
            (0..w / k).all(|bx| {
                let first = px(bx * k, by * k);
                (0..k).all(|dy| {
                    (0..k).all(|dx| {
                        let p = px(bx * k + dx, by * k + dy);
                        (0..4).all(|c| (p[c] - first[c]).abs() < 1.0 / 512.0)
                    })
                })
            })
        })
    };
    let smallest = 64;
    let mut k = MAX_FACTOR.min(w / smallest.min(w)).min(h / smallest.min(h));
    while k >= 2 {
        if w % k == 0 && h % k == 0 && blocks_constant(k) {
            return Some(k);
        }
        k -= 1;
    }
    None
}

/// A `w × h` 8-bit image of flat rectangles on a ground (edges at arbitrary positions),
/// optionally with a few noisy pixels.
fn art(w: usize, h: usize, seed: u64, noise: usize) -> Rgba {
    let mut s = seed;
    let mut data = vec![1.0f32; w * h * 4];
    for _ in 0..6 {
        let (x0, y0) = ((lcg(&mut s) as usize) % w, (lcg(&mut s) as usize) % h);
        let (x1, y1) = (
            (x0 + 1 + (lcg(&mut s) as usize) % w).min(w),
            (y0 + 1 + (lcg(&mut s) as usize) % h).min(h),
        );
        let c = [0, 1, 2, 3].map(|_| (lcg(&mut s) % 256) as f32 / 255.0);
        for y in y0..y1 {
            for x in x0..x1 {
                data[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&c);
            }
        }
    }
    for _ in 0..noise {
        let p = (lcg(&mut s) as usize) % (w * h);
        data[p * 4] = (lcg(&mut s) % 256) as f32 / 255.0;
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

/// Nearest-neighbour upscale by `k`.
fn upscale(img: &Rgba, k: usize) -> Rgba {
    let (w, h) = (img.width * k, img.height * k);
    let mut data = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let p = (y / k) * img.width + x / k;
            data.extend_from_slice(&img.data[p * 4..p * 4 + 4]);
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

/// `flatten_over` as it was: a serial push loop.
fn old_flatten_over(img: &Rgba, matte: [f32; 3]) -> (Rgba, Vec<f32>) {
    let n = img.width * img.height;
    let mut data = Vec::with_capacity(n * 4);
    let mut alpha = Vec::with_capacity(n);
    for i in 0..n {
        let p = &img.data[i * 4..i * 4 + 4];
        let a = p[3].clamp(0.0, 1.0);
        for c in 0..3 {
            data.push(p[c] * a + matte[c] * (1.0 - a));
        }
        data.push(1.0);
        alpha.push(a);
    }
    (
        Rgba {
            width: img.width,
            height: img.height,
            data,
        },
        alpha,
    )
}

/// Random straight RGBA with odd values mixed in: NaN, −0, alpha outside [0, 1].
fn random_rgba(w: usize, h: usize, seed: u64, extra: usize) -> Rgba {
    let mut s = seed;
    let mut data: Vec<f32> = (0..w * h * 4 + extra)
        .map(|i| match lcg(&mut s) % 6 {
            0 if i % 4 == 3 => 1.0,
            1 => 0.0,
            2 => (lcg(&mut s) % 256) as f32 / 255.0,
            _ => (lcg(&mut s) % 100_000) as f32 * 1.2e-5 - 0.05,
        })
        .collect();
    for (i, v) in [f32::NAN, -0.0, 1.5, -0.25].into_iter().enumerate() {
        if let Some(d) = data.get_mut(i * 13 + 3) {
            *d = v;
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|f| f.to_bits()).collect()
}

/// Serial and parallel sizes, every matte the ladder uses: the same image and alphas,
/// bit for bit, as the serial push loop.
#[test]
fn the_parallel_flatten_is_the_serial_one() {
    for (w, h) in [(0, 0), (1, 1), (7, 1), (1, 7), (50, 40), (300, 260)] {
        let img = random_rgba(w, h, (w * 31 + h) as u64, 0);
        for matte in [[1.0, 1.0, 1.0], [0.0, 0.0, 0.0], [1.0, 0.5, 0.0]] {
            let (a, aa) = flatten_over(&img, matte);
            let (b, ba) = old_flatten_over(&img, matte);
            assert_eq!(bits(&a.data), bits(&b.data), "{w}x{h}");
            assert_eq!(bits(&aa), bits(&ba), "{w}x{h}");
        }
    }
}

/// The parallel scan against the old strided scan, on opaque images with one translucent
/// pixel anywhere (first, last, middle, in a trailing partial pixel), on NaN alpha, and
/// on buffers whose length is not a multiple of four.
#[test]
fn the_transparency_scan_is_the_old_one() {
    let old = |img: &Rgba| img.data.iter().skip(3).step_by(4).any(|&a| a < 0.999);
    for (w, h) in [(0usize, 0usize), (1, 1), (9, 3), (300, 260)] {
        for extra in [0usize, 1, 3] {
            let n = w * h;
            let mut img = Rgba {
                width: w,
                height: h,
                data: vec![1.0; n * 4 + extra],
            };
            assert_eq!(has_transparency(&img), old(&img));
            for at in [0, n / 2, n.saturating_sub(1), n] {
                for v in [0.5, 0.9989, f32::NAN] {
                    let mut t = img.clone();
                    if let Some(d) = t.data.get_mut(at * 4 + 3) {
                        *d = v;
                    }
                    assert_eq!(has_transparency(&t), old(&t), "{w}x{h}+{extra} at {at}");
                }
            }
            img.data.iter_mut().for_each(|v| *v = 0.2);
            assert_eq!(has_transparency(&img), old(&img));
        }
    }
}

/// Flattening over the input's own buffer gives what flattening into a copy gives: the
/// same matted image, alphas, matte and cutout decision, in every mode; and an opaque
/// image comes back untouched.
#[test]
fn flattening_in_place_is_flattening_a_copy() {
    for (w, h) in [(1usize, 1usize), (40, 30), (300, 260)] {
        let mut white_mark = random_rgba(w, h, 5, 0);
        for p in white_mark.data.as_chunks_mut::<4>().0 {
            p[..3].copy_from_slice(&[1.0, 1.0, 1.0]);
        }
        for mut img in [random_rgba(w, h, 3, 0), white_mark] {
            // At least one translucent pixel, whatever the random draw.
            img.data[w * h * 4 - 1] = 0.4;
            for (cutout, native) in [(false, false), (true, false), (false, true)] {
                let a = alpha_source(&img, true, cutout, native).expect("translucent");
                let Ok(b) = alpha_source_owned(img.clone(), true, cutout, native) else {
                    panic!("translucent");
                };
                let what = format!("{w}x{h} cutout {cutout} native {native}");
                assert_eq!(bits(&a.flat.data), bits(&b.flat.data), "{what}");
                assert_eq!((a.flat.width, a.flat.height), (b.flat.width, b.flat.height));
                assert_eq!(bits(&a.alpha), bits(&b.alpha), "{what}");
                assert_eq!(bits(&a.matte), bits(&b.matte), "{what}");
                assert_eq!(a.cutout, b.cutout, "{what}");
            }
        }
        let opaque = Rgba {
            width: w,
            height: h,
            data: vec![0.5; w * h * 4]
                .chunks(4)
                .flat_map(|_| [0.3, 0.6, 0.9, 1.0])
                .collect(),
        };
        assert!(alpha_source(&opaque, true, false, true).is_none());
        let Err(back) = alpha_source_owned(opaque.clone(), true, false, true) else {
            panic!("opaque");
        };
        assert_eq!(bits(&back.data), bits(&opaque.data));
    }
}

/// A `w × h` 8-bit image whose every pixel is drawn at random (all four channels, alpha
/// between 1 and 255 levels), so neighbouring pixels differ almost surely: every cell of an
/// upscale of it is pinned by a change, and its lattice is unique.
fn noise(w: usize, h: usize, seed: u64) -> Rgba {
    let mut s = seed;
    Rgba {
        width: w,
        height: h,
        data: (0..w * h * 4)
            .map(|i| {
                let v = lcg(&mut s) % 256;
                let v = if i % 4 == 3 { v.max(1) } else { v };
                v as f32 / 255.0
            })
            .collect(),
    }
}

/// How a nearest-neighbour resize picks the source pixel of output pixel `x` along an axis
/// of `n` outputs from `m` sources.
#[derive(Clone, Copy, Debug)]
enum Sampling {
    /// At the pixel centre, in exact rationals: `⌊(x + ½)·m/n⌋` (Pillow, scikit-image,
    /// OpenCV `INTER_NEAREST_EXACT`).
    Centre,
    /// At the pixel's left edge: `⌊x·m/n⌋` (OpenCV `INTER_NEAREST`).
    Corner,
    /// At the centre through an f32 scale, so exact half-pixel ties fall either way.
    CentreF32,
}

fn source_index(x: usize, m: usize, n: usize, how: Sampling) -> usize {
    let i = match how {
        Sampling::Centre => (2 * x + 1) * m / (2 * n),
        Sampling::Corner => x * m / n,
        Sampling::CentreF32 => ((x as f32 + 0.5) * (m as f32 / n as f32)).floor() as usize,
    };
    i.min(m - 1)
}

/// A nearest-neighbour resize of `src` to `nw × nh`.
fn resize_nearest(src: &Rgba, nw: usize, nh: usize, how: Sampling) -> Rgba {
    let mut data = Vec::with_capacity(nw * nh * 4);
    for y in 0..nh {
        let sy = source_index(y, src.height, nh, how);
        for x in 0..nw {
            let sx = source_index(x, src.width, nw, how);
            let p = (sy * src.width + sx) * 4;
            data.extend_from_slice(&src.data[p..p + 4]);
        }
    }
    Rgba {
        width: nw,
        height: nh,
        data,
    }
}

/// Every whole factor the old test found, the lattice finds too, and its reduction is the
/// area average the old intake made of it, bit for bit (alpha-0 pixels included, whose
/// colour the average writes as 0 and the noise source does not). Noise sources, so the
/// lattice is pinned; a sparse drawing may come back coarser (see the module docs).
#[test]
fn whole_factors_come_back_as_the_old_test_found_them() {
    for (seed, (w, h)) in [(1u64, (160usize, 128usize)), (2, (96, 80)), (3, (64, 64))] {
        // One pixel in 17 fully transparent, over a colour the average does not keep.
        let mut clear = noise(w, h, seed + 10);
        for p in clear.data.chunks_mut(4).step_by(17) {
            p[3] = 0.0;
        }
        for src in [noise(w, h, seed), clear] {
            for k in [2, 3, 4, 5, 8] {
                let up = upscale(&src, k);
                let old = old_pixel_grid(&up);
                let grid = pixel_grid(&up);
                assert_eq!(
                    grid.as_ref().map(|g| up.width / g.source_size().0),
                    old,
                    "{w}x{h} x{k}"
                );
                if let Some(g) = grid {
                    let reduced = g.reduce(&up);
                    let averaged =
                        inkvec_trace::coverage::downsample_to(&up, up.width / k, up.height / k);
                    assert_eq!(
                        (reduced.width, reduced.height),
                        (averaged.width, averaged.height)
                    );
                    assert_eq!(bits(&reduced.data), bits(&averaged.data), "{w}x{h} x{k}");
                }
            }
        }
    }
}

/// Any factor of 2 or more, whole or fractional, in each sampling convention and with each
/// side rounded down or up, comes back as the source bit for bit (the noise source pins
/// every cell), with a pitch per axis.
#[test]
fn fractional_factors_come_back_bit_for_bit() {
    let src = noise(128, 50, 7);
    for how in [Sampling::Centre, Sampling::Corner, Sampling::CentreF32] {
        for s in [2.0f64, 2.25, 2.5, 2.9, 3.0, 3.5, 4.0, 5.75] {
            let (fw, fh) = (128.0 * s, 50.0 * s);
            for (nw, nh) in [
                (fw.floor(), fh.floor()),
                (fw.ceil(), fh.ceil()),
                (fw.floor(), fh.ceil()),
                (fw.ceil(), fh.floor()),
            ] {
                let (nw, nh) = (nw as usize, nh as usize);
                let up = resize_nearest(&src, nw, nh, how);
                let grid =
                    pixel_grid(&up).unwrap_or_else(|| panic!("{how:?} x{s} {nw}x{nh}: not found"));
                assert_eq!(grid.source_size(), (128, 50), "{how:?} x{s} {nw}x{nh}");
                assert_eq!(
                    bits(&grid.reduce(&up).data),
                    bits(&src.data),
                    "{how:?} x{s}"
                );
                let (px, py) = grid.pitch(nw, nh);
                assert!(
                    (px - nw as f64 / 128.0).abs() < 1e-12 && (py - nh as f64 / 50.0).abs() < 1e-12
                );
            }
        }
    }
}

/// The wordmark the old 64 px floor stopped half way: 128 x 50 blown up 4x came back as
/// 256 x 100 (still 2x blocky). The short side may now drop to 16 px.
#[test]
fn a_wordmark_is_undone_all_the_way() {
    let up = upscale(&noise(128, 50, 11), 4);
    assert_eq!(old_pixel_grid(&up), Some(2));
    assert_eq!(pixel_grid(&up).map(|g| g.source_size()), Some((128, 50)));
}

/// A drawing whose edges sit on pixel boundaries (flat rectangles, no anti-aliasing, so no
/// adjacent changes) fits a pitch-2 lattice of closed windows by accident; with whole
/// pitches half-open and the evidence floor it is left alone, while its true upscale (with
/// enough edges) is found.
#[test]
fn a_pixel_aligned_drawing_is_not_taken_for_an_upscale() {
    for seed in 0..20u64 {
        let native = art(300, 200, seed, 0);
        assert_eq!(pixel_grid(&native), None, "seed {seed}");
    }
    let busy = art(160, 128, 3, 60);
    let up = upscale(&busy, 3);
    let g = pixel_grid(&up).expect("60 noisy pixels give enough edges");
    assert_eq!(g.source_size(), (160, 128));
    assert_eq!(bits(&g.reduce(&up).data), bits(&busy.data));
}

/// The floors: the long side keeps at least 64 px (a 32 px sprite at 8x comes back at 64, a
/// finer lattice of the same image), the short side at least 16 (a 100 x 12 strip at 4x
/// comes back at 2x, 200 x 24), and an image too small to keep them at pitch 2 is not
/// looked at.
#[test]
fn the_floors_hold() {
    let sprite = upscale(&noise(32, 32, 5), 8);
    let g = pixel_grid(&sprite).expect("a finer lattice keeps the long side at 64");
    assert_eq!(g.source_size(), (64, 64));
    assert_eq!(
        bits(&upscale(&g.reduce(&sprite), 4).data),
        bits(&sprite.data)
    );
    assert!(pixel_grid(&upscale(&noise(30, 30, 5), 4)).is_none());
    assert_eq!(
        pixel_grid(&upscale(&noise(100, 12, 5), 4)).map(|g| g.source_size()),
        Some((200, 24))
    );
    assert_eq!(
        pixel_grid(&upscale(&noise(100, 16, 5), 4)).map(|g| g.source_size()),
        Some((100, 16))
    );
    assert!(MIN_SOURCE_LONG >= 2 * MIN_SOURCE);
}

/// An upscale by 2 across and 4 down is not one scale, but it is a 2x upscale of the source
/// stretched 2x down, which is one: that is what comes back, still exact.
#[test]
fn two_scales_come_back_as_the_finer_common_one() {
    let src = noise(100, 100, 4);
    let up = resize_nearest(&src, 200, 400, Sampling::Centre);
    let g = pixel_grid(&up).expect("a 2x lattice on both axes");
    assert_eq!(g.source_size(), (100, 200));
    assert_eq!(
        bits(&g.reduce(&up).data),
        bits(&resize_nearest(&src, 100, 200, Sampling::Centre).data)
    );
}

/// Not upscales: native-looking art, noise, a flat image, an upscale with one pixel broken
/// (early, late, by one level), a 1.5x upscale (pitch under 2), one with a NaN in one pixel
/// of a cell, and zero or tiny sides.
#[test]
fn what_is_not_an_upscale_is_left_alone() {
    let mut cases: Vec<(String, Rgba)> = vec![
        ("art".into(), art(300, 200, 1, 0)),
        ("noise".into(), noise(200, 200, 2)),
        (
            "flat".into(),
            Rgba {
                width: 256,
                height: 192,
                data: vec![0.25; 256 * 192 * 4],
            },
        ),
        (
            "1.5x".into(),
            resize_nearest(&noise(128, 128, 3), 192, 192, Sampling::Centre),
        ),
    ];
    let up = upscale(&noise(100, 80, 5), 3);
    for at in [
        7usize,
        up.width * up.height / 2 + 1,
        up.width * up.height - 3,
    ] {
        for delta in [1.0f32 / 255.0, 0.5] {
            let mut b = up.clone();
            let v = &mut b.data[at * 4 + 1];
            *v = if *v > 0.5 { *v - delta } else { *v + delta };
            cases.push((format!("broken at {at} by {delta}"), b));
        }
    }
    let mut nan = up.clone();
    nan.data[(up.width + 1) * 4] = f32::NAN;
    cases.push(("nan".into(), nan));
    for (w, h) in [
        (0usize, 30_000usize),
        (30_000, 0),
        (0, 0),
        (1, 1),
        (127, 300),
    ] {
        cases.push((
            format!("{w}x{h}"),
            Rgba {
                width: w,
                height: h,
                data: vec![0.5; w * h * 4],
            },
        ));
    }
    for (name, img) in &cases {
        assert_eq!(pixel_grid(img), None::<PixelGrid>, "{name}");
    }
}

/// A buffer shorter than its stated size is not read past its end.
#[test]
fn a_short_buffer_is_refused() {
    let mut up = upscale(&noise(100, 80, 6), 2);
    up.data.truncate(up.data.len() - 4);
    assert!(pixel_grid(&up).is_none());
}
