//! Tests of the Fast palette: its behaviour on hand-made images, and bit-for-bit
//! equivalence with the implementation it replaced ([`super::reference`]).

use super::*;

/// A white canvas with a black square whose edges are anti-aliased to grey.
fn square(w: usize) -> Vec<[f32; 3]> {
    let mut img = vec![[1.0f32; 3]; w * w];
    for y in 0..w {
        for x in 0..w {
            let inside = (w / 4..3 * w / 4).contains(&x) && (w / 4..3 * w / 4).contains(&y);
            let edge = x == w / 4 - 1 || y == w / 4 - 1;
            if inside {
                img[y * w + x] = [0.0; 3];
            } else if edge {
                img[y * w + x] = [0.5; 3];
            }
        }
    }
    img
}

/// A deterministic pseudo-random stream (a 64-bit LCG, Knuth's MMIX constants), so the
/// equivalence tests need no dependency and fail reproducibly.
fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// One test image: the colour over white, the opacity when traced natively, and its size.
struct Case {
    name: String,
    rgb: Vec<[f32; 3]>,
    alpha: Option<Vec<f32>>,
    w: usize,
    h: usize,
}

/// A `w × h` 8-bit image as the intake hands it over: runs of `inks` source colours
/// copied down from the row above most of the time (so bins have flat pixels), `noise`
/// percent of pixels a fresh random colour, and, with `alpha`, 8-bit opacities (mostly
/// opaque, some clear, some half) composited over white exactly as
/// `Rgba::composited` does.
fn img8(w: usize, h: usize, seed: u64, inks: usize, noise: u64, alpha: bool) -> Case {
    let mut s = seed;
    let byte = |s: &mut u64| (lcg(s) % 256) as f32 / 255.0;
    let pal: Vec<[f32; 4]> = (0..inks.max(1))
        .map(|_| {
            let a = match lcg(&mut s) % 4 {
                0 if alpha => 0.0,
                1 if alpha => 128.0 / 255.0,
                _ => 1.0,
            };
            [byte(&mut s), byte(&mut s), byte(&mut s), a]
        })
        .collect();
    let mut src: Vec<[f32; 4]> = Vec::with_capacity(w * h);
    let mut cur = pal[0];
    for p in 0..w * h {
        let r = lcg(&mut s) % 100;
        if r < noise {
            let a = if alpha { byte(&mut s) } else { 1.0 };
            cur = [byte(&mut s), byte(&mut s), byte(&mut s), a];
        } else if r < noise + 6 {
            cur = pal[(lcg(&mut s) % pal.len() as u64) as usize];
        } else if p >= w && r < 70 {
            cur = src[p - w];
        }
        src.push(cur);
    }
    let rgb = src
        .iter()
        .map(|p| {
            let a = p[3];
            [
                p[0] * a + 1.0 * (1.0 - a),
                p[1] * a + 1.0 * (1.0 - a),
                p[2] * a + 1.0 * (1.0 - a),
            ]
        })
        .collect();
    Case {
        name: format!("img8 {w}x{h} seed {seed} inks {inks} noise {noise} alpha {alpha}"),
        rgb,
        alpha: alpha.then(|| src.iter().map(|p| p[3]).collect()),
        w,
        h,
    }
}

/// Values no 8-bit intake produces: arbitrary floats, as a box-averaged (resampled)
/// raster carries, including values below 2⁻⁸ and exact zeros.
fn resampled(w: usize, h: usize, seed: u64, alpha: bool) -> Case {
    let mut c = img8(w, h, seed, 4, 10, alpha);
    let mut s = seed ^ 0x9e37_79b9;
    for (p, px) in c.rgb.iter_mut().enumerate() {
        if lcg(&mut s) % 3 == 0 {
            for v in px.iter_mut() {
                *v = (*v * 0.999_7 + (lcg(&mut s) % 1000) as f32 * 1e-6).min(1.0);
            }
        }
        if p % 17 == 0 {
            px[0] = 1e-4;
        }
    }
    if let Some(a) = c.alpha.as_mut() {
        for v in a.iter_mut().step_by(5) {
            *v = (*v * 0.9 + 0.003).min(1.0);
        }
    }
    c.name = format!("resampled {w}x{h} seed {seed} alpha {alpha}");
    c
}

/// Hand-made degenerate images: one pixel, one row, one column, one ink, a checkerboard
/// (no pixel has a 4-neighbour in its bin), and stripes touching every border.
fn degenerate() -> Vec<Case> {
    let mk = |name: &str, w: usize, h: usize, f: &dyn Fn(usize, usize) -> [f32; 3]| Case {
        name: name.into(),
        rgb: (0..w * h).map(|p| f(p % w, p / w)).collect(),
        alpha: None,
        w,
        h,
    };
    let mut out = vec![
        mk("1x1", 1, 1, &|_, _| [0.2, 0.4, 0.6]),
        mk("one ink", 16, 9, &|_, _| [0.9, 0.1, 0.1]),
        mk("checkerboard", 12, 12, &|x, y| {
            if (x + y) % 2 == 0 {
                [0.0; 3]
            } else {
                [1.0; 3]
            }
        }),
        mk("border stripes", 20, 14, &|x, y| {
            if x == 0 || y == 13 {
                [0.1, 0.2, 0.9]
            } else if x == 19 || y == 0 {
                [0.9, 0.8, 0.1]
            } else {
                [1.0; 3]
            }
        }),
        mk("empty", 0, 0, &|_, _| [0.0; 3]),
    ];
    for (w, h) in [(1, 40), (40, 1)] {
        let mut c = img8(w, h, 7, 3, 5, false);
        c.name = format!("line {w}x{h}");
        out.push(c);
        let mut c = img8(w, h, 8, 3, 5, true);
        c.name = format!("line {w}x{h} alpha");
        out.push(c);
    }
    let mut clear = mk("all clear", 9, 9, &|_, _| [1.0; 3]);
    clear.alpha = Some(vec![0.0; 81]);
    out.push(clear);
    out
}

/// Every equivalence case: degenerate images, 8-bit images large and small, opaque and
/// traced with their transparency, and resampled (non-8-bit) values.
fn cases() -> Vec<Case> {
    let mut out = degenerate();
    for (seed, (w, h)) in [(1u64, (37usize, 23usize)), (2, (64, 64)), (3, (130, 97))] {
        for alpha in [false, true] {
            out.push(img8(w, h, seed, 5, 4, alpha));
            out.push(img8(w, h, seed + 10, 12, 30, alpha));
            out.push(resampled(w, h, seed + 20, alpha));
        }
    }
    // Past the size where the palette works in parallel row bands.
    for alpha in [false, true] {
        out.push(img8(300, 290, 40, 6, 3, alpha));
        out.push(resampled(300, 290, 41, alpha));
    }
    out
}

/// Run the shipped palette and the frozen reference on `c` and demand the same inks to
/// the bit (`rgb`, `colors`, `alpha`, `weight`) and the same label for every pixel.
fn assert_same(c: &Case, merge_distance: f32, max_colors: usize) {
    let a = c.alpha.as_deref();
    let (pn, ln) = palette_and_labels(&c.rgb, a, c.w, c.h, merge_distance, max_colors);
    let (po, lo) = reference::palette_and_labels(&c.rgb, a, c.w, c.h, merge_distance, max_colors);
    let what = format!("{} (merge {merge_distance}, max {max_colors})", c.name);
    let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
    let flat3 = |v: &[[f32; 3]]| v.iter().flatten().copied().collect::<Vec<_>>();
    let lab = |p: &Palette| {
        p.colors
            .iter()
            .flat_map(|c| [c.l, c.a, c.b])
            .collect::<Vec<_>>()
    };
    assert_eq!(bits(&flat3(&pn.rgb)), bits(&flat3(&po.rgb)), "{what}: rgb");
    assert_eq!(bits(&lab(&pn)), bits(&lab(&po)), "{what}: oklab");
    assert_eq!(bits(&pn.alpha), bits(&po.alpha), "{what}: alpha");
    assert_eq!(bits(&pn.weight), bits(&po.weight), "{what}: weight");
    assert!(ln == lo, "{what}: labels differ");
}

/// The banded parallel histogram against the serial run walk, bin by bin and bit for
/// bit, on images that pass the exactness check (8-bit values, where the bands' partial
/// sums are merged) and on one that does not (where the serial order is kept).
#[test]
fn banded_histogram_equals_the_serial_one() {
    for c in [
        img8(97, 70, 3, 6, 5, false),
        img8(64, 200, 4, 9, 20, true),
        img8(33, 33, 5, 3, 2, true),
        img8(300, 290, 6, 12, 40, false),
        resampled(80, 90, 7, true),
    ] {
        let a = c.alpha.as_deref();
        let grid = if a.is_some() {
            Grid {
                bits: 4,
                alpha_bits: 4,
            }
        } else {
            Grid {
                bits: 5,
                alpha_bits: 0,
            }
        };
        let (ks, bs) = keys_and_histogram(&c.rgb, a, c.w, c.h, grid, false);
        let (kp, bp) = keys_and_histogram(&c.rgb, a, c.w, c.h, grid, true);
        assert!(ks == kp, "{}: keys", c.name);
        assert_eq!(bs.len(), bp.len(), "{}: occupied bins", c.name);
        let bits = |s: [f64; 4]| s.map(f64::to_bits);
        for id in 0..bs.len() {
            let j = bp.id(bs.key[id]);
            let what = format!("{}: bin {}", c.name, bs.key[id]);
            assert_eq!(bs.count[id], bp.count[j], "{what}");
            assert_eq!(bs.flat[id], bp.flat[j], "{what}");
            assert_eq!(bs.paired[id], bp.paired[j], "{what}");
            assert_eq!(bits(bs.all_sum[id]), bits(bp.all_sum[j]), "{what}");
            assert_eq!(bits(bs.flat_sum[id]), bits(bp.flat_sum[j]), "{what}");
        }
    }
}

#[test]
fn exact_set_is_zero_or_two_to_minus_eight_through_one() {
    for v in [0.0f32, -0.0, 1.0 / 256.0, 1.0 / 255.0, 0.5, 1.0] {
        assert!(in_exact_set(v), "{v}");
    }
    let below = f32::from_bits((1.0f32 / 256.0).to_bits() - 1);
    for v in [
        below,
        1e-4,
        f32::from_bits(1.0f32.to_bits() + 1),
        -0.5,
        f32::NAN,
    ] {
        assert!(!in_exact_set(v), "{v}");
    }
    // Every 8-bit level, and every translucent 8-bit colour over white, is in the set.
    for k in 0..=255u32 {
        let s = k as f32 / 255.0;
        assert!(in_exact_set(s));
        for j in 0..=255u32 {
            let a = j as f32 / 255.0;
            assert!(in_exact_set(s * a + 1.0 * (1.0 - a)), "{k} at {j}");
        }
    }
}

#[test]
fn the_palette_equals_the_shipped_one_bit_for_bit() {
    for c in cases() {
        for (md, mc) in [(0.035, 64), (0.01, 64), (0.035, 3), (0.2, 2)] {
            assert_same(&c, md, mc);
        }
    }
}

/// Every float in `[0, 1]` on both grids: 1 065 353 217 values, a few seconds in release
/// on all cores. Skipped in debug builds, where it would take minutes;
/// `cargo test --release` runs it.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "exhaustive over every float in [0, 1]; run with --release"
)]
fn rounding_is_exact_on_every_float() {
    use rayon::prelude::*;
    assert_eq!(BELOW_HALF.to_bits(), 0.5f32.to_bits() - 1);
    let bad = (0..=1.0f32.to_bits())
        .into_par_iter()
        .filter(|&b| {
            let v = f32::from_bits(b);
            [4, 5]
                .iter()
                .any(|&bits| level(v, bits) != reference::level(v, bits))
        })
        .count();
    assert_eq!(bad, 0);
}

/// The cheap half of the exhaustive test, run in every build: every float within 2¹⁶
/// steps of each half-level boundary `(j + 1/2) / T`, and the values outside `[0, 1]`.
#[test]
fn rounding_is_exact_near_every_half_level_and_off_range() {
    for bits in [4u32, 5] {
        let t = ((1u32 << bits) - 1) as f32;
        for j in 0..(1u32 << bits) {
            let centre = ((j as f32 + 0.5) / t).to_bits();
            for b in centre.saturating_sub(1 << 16)..=centre + (1 << 16) {
                let v = f32::from_bits(b);
                assert_eq!(level(v, bits), reference::level(v, bits), "{v} on {bits}");
            }
        }
        for v in [
            f32::NAN,
            -0.0,
            -1.0,
            2.0,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
        ] {
            assert_eq!(level(v, bits), reference::level(v, bits), "{v} on {bits}");
        }
    }
}

#[test]
fn integer_shares_equal_the_running_float_sum() {
    // The old sum of ones stalls at 2^24; the integer count converted once agrees.
    for (count, n) in [
        (0usize, 0usize),
        (3, 7),
        (1 << 24, 1 << 25),
        ((1 << 24) + 5, 1 << 26),
    ] {
        let mut f = 0f32;
        for _ in 0..count {
            f += 1.0;
        }
        let old = f / n.max(1) as f32;
        assert_eq!(
            ink_shares(&[count], n)[0].to_bits(),
            old.to_bits(),
            "{count}/{n}"
        );
    }
    let mut count = vec![0; 3];
    add_label_runs(&[], &mut count);
    assert_eq!(count, vec![0, 0, 0]);
    add_label_runs(&[2, 2, 0, 2, 1, 1], &mut count);
    add_label_runs(&[1], &mut count);
    assert_eq!(count, vec![1, 3, 3]);
}

#[test]
fn blends_are_not_inks() {
    let img = square(32);
    let (pal, labels) = palette_and_labels(&img, None, 32, 32, 0.035, 64);
    assert_eq!(pal.len(), 2, "{:?}", pal.rgb);
    assert!(labels.iter().all(|&l| (l as usize) < pal.len()));
    let black = pal.rgb.iter().position(|c| c[0] < 0.1).unwrap() as u16;
    assert_eq!(labels[16 * 32 + 16], black);
}

#[test]
fn a_grey_rim_does_not_take_a_red_used_elsewhere() {
    // The black square's grey rim is nearer red than black or white in OKLab, and is a
    // blend of black and white.
    let w = 32;
    let mut img = square(w);
    for y in 0..w {
        for x in 26..w {
            img[y * w + x] = [0.94, 0.14, 0.12];
        }
    }
    let (pal, labels) = palette_and_labels(&img, None, w, w, 0.035, 64);
    assert_eq!(pal.len(), 3, "{:?}", pal.rgb);
    let red = pal
        .rgb
        .iter()
        .position(|c| c[1] < 0.5 && c[0] > 0.5)
        .unwrap() as u16;
    for p in 0..w * w {
        if img[p] == [0.5; 3] {
            assert_ne!(labels[p], red, "rim pixel {p} went red");
        }
    }
}

#[test]
fn neighbouring_bins_are_one_ink_and_distant_ones_two() {
    let mut img = vec![[0.80f32, 0.20, 0.20]; 64];
    img.extend(vec![[0.81f32, 0.21, 0.20]; 64]);
    img.extend(vec![[0.20f32, 0.20, 0.80]; 64]);
    let (pal, _) = palette_and_labels(&img, None, 8, 24, 0.035, 64);
    assert_eq!(pal.len(), 2, "{:?}", pal.rgb);
}

#[test]
fn close_flat_colours_in_separate_bins_stay_two_inks() {
    // Two flat greys 8 levels apart: close in OKLab, but bins that do not touch.
    let mut img = vec![[0.50f32; 3]; 64];
    img.extend(vec![[0.50f32 + 8.0 / 255.0 * 2.2; 3]; 64]);
    let (pal, _) = palette_and_labels(&img, None, 8, 16, 0.035, 64);
    assert_eq!(pal.len(), 2, "{:?}", pal.rgb);
}

#[test]
fn the_clear_ground_is_an_ink_of_its_own() {
    // White paint on a transparent ground: over white they are the same colour.
    let rgb = vec![[1.0f32; 3]; 16 * 16];
    let alpha: Vec<f32> = (0..16 * 16)
        .map(|p| {
            if (4..12).contains(&(p % 16)) {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let (pal, labels) = palette_and_labels(&rgb, Some(&alpha), 16, 16, 0.035, 64);
    assert_eq!(pal.len(), 2, "{:?} {:?}", pal.rgb, pal.alpha);
    assert!(pal.alpha.contains(&0.0) && pal.alpha.contains(&1.0));
    assert_ne!(labels[0], labels[8]);
}

#[test]
fn the_palette_is_capped() {
    let img: Vec<[f32; 3]> = (0..40 * 40)
        .map(|p| {
            let band = (p / 40) / 4;
            [band as f32 / 10.0, 0.5, 1.0 - band as f32 / 10.0]
        })
        .collect();
    let (pal, labels) = palette_and_labels(&img, None, 40, 40, 0.01, 4);
    assert_eq!(pal.len(), 4);
    assert!(labels.iter().all(|&l| l < 4));
}
