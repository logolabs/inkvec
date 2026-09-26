//! Fast mode's palette and labels, in two passes over the pixels.
//!
//! Quality mode recovers inks by minimum description length, with spatial evidence tests for
//! every candidate; it is the single most expensive stage of its front end. This is the
//! cheap version of the same idea. Every pixel is binned to 15-bit colour, and a bin counts
//! as evidence for an ink only through its *flat* pixels -- those whose four neighbours fall
//! in the same bin. An anti-aliased rim is one pixel wide and has almost none, so blends
//! between inks never become inks; a filled region of any size has plenty. Bins are taken in
//! order of flat pixels and merged into the nearest accepted ink within `merge_distance`
//! (OKLab), up to `max_colors` inks.
//!
//! Labelling is then per bin, not per pixel: each bin maps to its nearest ink once. A pixel
//! whose bin is not itself an ink colour (a blend) is given the nearest of the inks its
//! flat-coloured neighbours carry, so a rim between black and white goes to black or white,
//! never to a grey ink that happens to sit between them in colour.

use crate::color::{rgb_to_oklab, Oklab, Palette};

/// Bits per channel of the colour bins.
const BITS: u32 = 5;
const LEVELS: usize = 1 << BITS;
const BINS: usize = LEVELS * LEVELS * LEVELS;
/// Flat pixels a bin needs before it can found an ink.
const MIN_FLAT: u32 = 3;

fn key(c: [f32; 3]) -> usize {
    let q = |v: f32| ((v.clamp(0.0, 1.0) * (LEVELS - 1) as f32).round() as usize).min(LEVELS - 1);
    (q(c[0]) << (2 * BITS)) | (q(c[1]) << BITS) | q(c[2])
}

/// Per-bin statistics: all pixels, and flat pixels with their colour sum.
struct Bins {
    count: Vec<u32>,
    flat: Vec<u32>,
    flat_sum: Vec<[f64; 3]>,
    all_sum: Vec<[f64; 3]>,
}

fn histogram(rgb: &[[f32; 3]], keys: &[u16], w: usize, h: usize) -> Bins {
    let mut b = Bins {
        count: vec![0; BINS],
        flat: vec![0; BINS],
        flat_sum: vec![[0.0; 3]; BINS],
        all_sum: vec![[0.0; 3]; BINS],
    };
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            let k = keys[p];
            let ku = k as usize;
            let c = rgb[p];
            b.count[ku] += 1;
            for (s, v) in b.all_sum[ku].iter_mut().zip(c) {
                *s += v as f64;
            }
            let same = |q: usize| keys[q] == k;
            let flat = (x == 0 || same(p - 1))
                && (x + 1 == w || same(p + 1))
                && (y == 0 || same(p - w))
                && (y + 1 == h || same(p + w));
            if flat {
                b.flat[ku] += 1;
                for (s, v) in b.flat_sum[ku].iter_mut().zip(c) {
                    *s += v as f64;
                }
            }
        }
    }
    b
}

fn nearest(inks: &[Oklab], c: Oklab) -> (usize, f32) {
    let mut best = (0, f32::INFINITY);
    for (i, &k) in inks.iter().enumerate() {
        let d = k.dist(c);
        if d < best.1 {
            best = (i, d);
        }
    }
    best
}

/// The inks, and one label (an ink index) per pixel.
pub(crate) fn palette_and_labels(
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    merge_distance: f32,
    max_colors: usize,
) -> (Palette, Vec<u16>) {
    let n = w * h;
    let keys: Vec<u16> = rgb.iter().map(|&c| key(c) as u16).collect();
    let bins = histogram(rgb, &keys, w, h);

    // Candidate bins, most flat pixels first; the key breaks ties so the order is total.
    let mut cands: Vec<usize> = (0..BINS).filter(|&k| bins.flat[k] >= MIN_FLAT).collect();
    cands.sort_unstable_by_key(|&k| (std::cmp::Reverse(bins.flat[k]), k));
    let max_colors = max_colors.clamp(1, u16::MAX as usize);
    let mut lab: Vec<Oklab> = Vec::new();
    let mut acc: Vec<([f64; 3], f64)> = Vec::new();
    for &k in &cands {
        let f = bins.flat[k] as f64;
        let s = bins.flat_sum[k];
        let c = [(s[0] / f) as f32, (s[1] / f) as f32, (s[2] / f) as f32];
        let ck = rgb_to_oklab(c);
        let (i, d) = nearest(&lab, ck);
        if lab.len() < max_colors && d >= merge_distance {
            lab.push(ck);
            acc.push((s, f));
        } else if !lab.is_empty() {
            // One ink measured twice: its colour is the flat pixels' mean over both bins.
            let a = &mut acc[i];
            for (t, v) in a.0.iter_mut().zip(s) {
                *t += v;
            }
            a.1 += f;
        }
    }
    let mut rgb_inks: Vec<[f32; 3]> = acc
        .iter()
        .map(|(s, f)| [(s[0] / f) as f32, (s[1] / f) as f32, (s[2] / f) as f32])
        .collect();
    if rgb_inks.is_empty() {
        // Nothing flat anywhere (noise, or a tiny image): one ink, the mean colour.
        let mut s = [0.0f64; 3];
        for c in rgb {
            for (t, v) in s.iter_mut().zip(c) {
                *t += *v as f64;
            }
        }
        let m = n.max(1) as f64;
        rgb_inks.push([(s[0] / m) as f32, (s[1] / m) as f32, (s[2] / m) as f32]);
    }
    let lab: Vec<Oklab> = rgb_inks.iter().map(|&c| rgb_to_oklab(c)).collect();

    // Each occupied bin, once: its nearest ink, and whether the bin *is* that ink.
    let mut lut = vec![(0u16, false); BINS];
    let mut bin_lab = vec![
        Oklab {
            l: 0.0,
            a: 0.0,
            b: 0.0
        };
        BINS
    ];
    for k in 0..BINS {
        let m = bins.count[k];
        if m == 0 {
            continue;
        }
        let s = bins.all_sum[k];
        let f = m as f64;
        let c = rgb_to_oklab([(s[0] / f) as f32, (s[1] / f) as f32, (s[2] / f) as f32]);
        let (i, d) = nearest(&lab, c);
        bin_lab[k] = c;
        lut[k] = (i as u16, d < merge_distance);
    }

    // A blend takes the nearest ink among its sure neighbours'.
    let sure = |p: usize| lut[keys[p] as usize];
    let mut labels = vec![0u16; n];
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            let (own, is_ink) = sure(p);
            if is_ink {
                labels[p] = own;
                continue;
            }
            let c = bin_lab[keys[p] as usize];
            let mut best = (own, f32::INFINITY);
            for yy in y.saturating_sub(1)..(y + 2).min(h) {
                for xx in x.saturating_sub(1)..(x + 2).min(w) {
                    let (l, ok) = sure(yy * w + xx);
                    if ok {
                        let d = lab[l as usize].dist(c);
                        if d < best.1 {
                            best = (l, d);
                        }
                    }
                }
            }
            labels[p] = best.0;
        }
    }

    let mut weight = vec![0f32; lab.len()];
    for &l in &labels {
        weight[l as usize] += 1.0;
    }
    for v in weight.iter_mut() {
        *v /= n.max(1) as f32;
    }
    let k = lab.len();
    (
        Palette {
            colors: lab,
            rgb: rgb_inks,
            weight,
            alpha: vec![1.0; k],
        },
        labels,
    )
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn blends_are_not_inks() {
        let img = square(32);
        let (pal, labels) = palette_and_labels(&img, 32, 32, 0.035, 64);
        assert_eq!(pal.len(), 2, "{:?}", pal.rgb);
        assert!(labels.iter().all(|&l| (l as usize) < pal.len()));
        let black = pal.rgb.iter().position(|c| c[0] < 0.1).unwrap() as u16;
        assert_eq!(labels[16 * 32 + 16], black);
    }

    #[test]
    fn nearby_bins_are_one_ink_and_distant_ones_two() {
        let mut img = vec![[0.80f32, 0.20, 0.20]; 64];
        img.extend(vec![[0.81f32, 0.21, 0.20]; 64]);
        img.extend(vec![[0.20f32, 0.20, 0.80]; 64]);
        let (pal, _) = palette_and_labels(&img, 8, 24, 0.035, 64);
        assert_eq!(pal.len(), 2, "{:?}", pal.rgb);
    }

    #[test]
    fn the_palette_is_capped() {
        let img: Vec<[f32; 3]> = (0..40 * 40)
            .map(|p| {
                let band = (p / 40) / 4;
                [band as f32 / 10.0, 0.5, 1.0 - band as f32 / 10.0]
            })
            .collect();
        let (pal, labels) = palette_and_labels(&img, 40, 40, 0.01, 4);
        assert_eq!(pal.len(), 4);
        assert!(labels.iter().all(|&l| l < 4));
    }
}
