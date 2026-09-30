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

/// `pixel_grid` as it was: every divisor of both sides from the largest down, each with
/// the block test.
fn old_pixel_grid(img: &Rgba) -> Option<usize> {
    const MAX_FACTOR: usize = 32;
    let (w, h) = (img.width, img.height);
    let smallest = 64;
    let mut k = MAX_FACTOR.min(w / smallest.min(w)).min(h / smallest.min(h));
    while k >= 2 {
        if w % k == 0 && h % k == 0 && blocks_constant(img, k) {
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
        for p in white_mark.data.chunks_exact_mut(4) {
            p[..3].copy_from_slice(&[1.0, 1.0, 1.0]);
        }
        for mut img in [random_rgba(w, h, 3, 0), white_mark] {
            // At least one translucent pixel, whatever the random draw.
            img.data[w * h * 4 - 1] = 0.4;
            for (cutout, native) in [(false, false), (true, false), (false, true)] {
                let a = alpha_source(&img, true, cutout, native).expect("translucent");
                let b = alpha_source_owned(img.clone(), true, cutout, native)
                    .ok()
                    .expect("translucent");
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
        let back = alpha_source_owned(opaque.clone(), true, false, true)
            .err()
            .expect("opaque");
        assert_eq!(bits(&back.data), bits(&opaque.data));
    }
}

#[test]
fn gcd_is_euclid() {
    let euclid = |mut a: usize, mut b: usize| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    for a in 0..200 {
        for b in 0..200 {
            assert_eq!(gcd(a, b), euclid(a, b), "{a} {b}");
        }
    }
    assert!(divides(4, 2048) && !divides(3, 2048) && divides(7, 0));
}

/// The gcd filter against the old full search: plain art, upscales by 2 to 8 (the
/// factor must come back the same), upscales with one pixel broken early or late,
/// odd sizes, a flat image, blocks that vary inside the 1/512 tolerance (not 8-bit, so
/// the block test has to decide), and a NaN.
#[test]
fn the_gcd_filter_finds_what_the_full_search_found() {
    let mut cases: Vec<(String, Rgba)> = Vec::new();
    for (seed, (w, h)) in [(1u64, (160usize, 128usize)), (2, (96, 80)), (3, (64, 64))] {
        let a = art(w, h, seed, 0);
        cases.push((format!("art {w}x{h}"), a.clone()));
        for k in [2, 3, 4, 5, 8] {
            let up = upscale(&a, k);
            cases.push((format!("art {w}x{h} x{k}"), up.clone()));
            for at in [7usize, up.width * up.height - 3] {
                let mut b = up.clone();
                b.data[at * 4 + 1] = if b.data[at * 4 + 1] > 0.5 { 0.0 } else { 1.0 };
                cases.push((format!("art {w}x{h} x{k} broken at {at}"), b));
            }
        }
        cases.push((format!("noisy {w}x{h}"), upscale(&art(w, h, seed, 40), 2)));
    }
    let flat = Rgba {
        width: 256,
        height: 192,
        data: vec![0.25; 256 * 192 * 4],
    };
    cases.push(("flat".into(), flat));
    let mut odd = upscale(&art(97, 64, 9, 0), 2);
    odd.width -= 1;
    odd.data.truncate(odd.width * odd.height * 4);
    cases.push(("odd width".into(), odd));
    // Within-block wobble below the tolerance: the old test passes, and the new one must
    // not be fooled by neighbours that differ by up to 2/512.
    let mut wobble = upscale(&art(80, 64, 4, 0), 4);
    let mut s = 99u64;
    for v in wobble.data.iter_mut() {
        *v += ((lcg(&mut s) % 7) as f32 - 3.0) * (0.45 / 512.0) / 3.0;
    }
    assert_eq!(
        old_pixel_grid(&wobble),
        Some(4),
        "the old test forgives the wobble"
    );
    cases.push(("wobble".into(), wobble.clone()));
    let mut drift = wobble;
    for (i, v) in drift.data.iter_mut().enumerate() {
        if (i / 4) % drift.width % 4 == 3 {
            *v += 1.5 / 512.0;
        }
    }
    cases.push(("drift".into(), drift));
    let mut nan = upscale(&art(64, 64, 5, 0), 2);
    nan.data[1000] = f32::NAN;
    cases.push(("nan".into(), nan));
    for (name, img) in &cases {
        assert_eq!(pixel_grid(img), old_pixel_grid(img), "{name}");
    }
    // And the factors really are recovered.
    assert_eq!(pixel_grid(&upscale(&art(160, 128, 1, 0), 4)), Some(4));
}
