//! Fast mode's palette and labels, in two passes over the pixels.
//!
//! Quality mode recovers inks by minimum description length, with spatial evidence tests for
//! every candidate; it is the single most expensive stage of its front end. This is the
//! cheap version of the same idea. Every pixel is binned (15-bit colour, or 12-bit colour
//! and 4-bit opacity for an image traced with its transparency), and a bin counts as
//! evidence for an ink only through its *flat* pixels -- those whose four neighbours fall in
//! the same bin. An anti-aliased rim is one pixel wide and has almost none, so blends
//! between inks never become inks; a filled region of any size has plenty. Bins are taken in
//! order of flat pixels. One joins the nearest accepted ink when it lies within
//! `merge_distance` of it *and* next to one of that ink's bins (one ink spread over
//! neighbouring bins by noise), or within [`SAME_INK`] outright; otherwise it founds an
//! ink of its own, up to `max_colors`. Two flat colours a few levels apart, in bins that do
//! not touch, stay two inks.
//!
//! Colours are compared as the transparency-aware module compares them
//! ([`crate::native::Ink2`]): over white and over a second ground, so white paint and the
//! clear ground are as far apart as white and black. For an opaque image that is plain OKLab.
//!
//! Labelling is then per bin, not per pixel: each bin maps to its nearest ink once. A pixel
//! whose bin is not itself an ink colour (a blend) takes the nearest of its own nearest ink
//! and the inks its ink-coloured neighbours carry.

use crate::color::{rgb_to_oklab, Palette};
use crate::native::{over_black, snap_alpha, Ink2, OPAQUE};

/// Bins: 16-bit keys.
const BINS: usize = 1 << 16;
/// Flat pixels a bin needs before it can found an ink.
const MIN_FLAT: u32 = 3;
/// Inks this close (OKLab) are one ink whether or not their bins touch.
const SAME_INK: f32 = 0.012;

/// How pixels are binned: bits per colour channel and for opacity.
#[derive(Clone, Copy)]
struct Grid {
    bits: u32,
    alpha_bits: u32,
}

impl Grid {
    fn key(self, c: [f32; 4]) -> usize {
        let q = |v: f32, bits: u32| {
            let top = ((1usize << bits) - 1) as f32;
            (v.clamp(0.0, 1.0) * top).round() as usize
        };
        let (b, ab) = (self.bits, self.alpha_bits);
        let mut k = (q(c[0], b) << (2 * b)) | (q(c[1], b) << b) | q(c[2], b);
        if ab > 0 {
            k = (k << ab) | q(c[3], ab);
        }
        k
    }

    fn parts(self, k: usize) -> [isize; 4] {
        let (b, ab) = (self.bits, self.alpha_bits);
        let m = (1usize << b) - 1;
        let a = k & ((1usize << ab) - 1);
        let c = k >> ab;
        [
            ((c >> (2 * b)) & m) as isize,
            ((c >> b) & m) as isize,
            (c & m) as isize,
            a as isize,
        ]
    }

    /// Two bins are neighbours when no channel differs by more than one step.
    fn touch(self, a: usize, b: usize) -> bool {
        let (p, q) = (self.parts(a), self.parts(b));
        p.iter().zip(q).all(|(x, y)| (x - y).abs() <= 1)
    }
}

/// Per-bin statistics: all pixels, and flat pixels, with their colour-and-opacity sums.
struct Bins {
    count: Vec<u32>,
    flat: Vec<u32>,
    flat_sum: Vec<[f64; 4]>,
    all_sum: Vec<[f64; 4]>,
}

fn histogram(px: &[[f32; 4]], keys: &[u16], w: usize, h: usize) -> Bins {
    let mut b = Bins {
        count: vec![0; BINS],
        flat: vec![0; BINS],
        flat_sum: vec![[0.0; 4]; BINS],
        all_sum: vec![[0.0; 4]; BINS],
    };
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            let k = keys[p];
            let ku = k as usize;
            let c = px[p];
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

fn mean(s: [f64; 4], f: f64) -> [f32; 4] {
    [
        (s[0] / f) as f32,
        (s[1] / f) as f32,
        (s[2] / f) as f32,
        (s[3] / f) as f32,
    ]
}

/// A colour over white with its opacity, as the two-ground point inks are compared by.
fn ink2(c: [f32; 4]) -> Ink2 {
    let w = rgb_to_oklab([c[0], c[1], c[2]]);
    if c[3] >= OPAQUE {
        Ink2::opaque(w)
    } else {
        Ink2 {
            w,
            k: rgb_to_oklab(over_black([c[0], c[1], c[2]], c[3])),
        }
    }
}

fn nearest(inks: &[Ink2], c: Ink2) -> (usize, f32) {
    let mut best = (0, f32::INFINITY);
    for (i, &k) in inks.iter().enumerate() {
        let d = k.dist(c);
        if d < best.1 {
            best = (i, d);
        }
    }
    best
}

/// One accepted ink while the palette is built: its bins and its flat pixels' sums.
struct Ink {
    point: Ink2,
    bins: Vec<usize>,
    sum: [f64; 4],
    flat: f64,
}

/// Found inks from the candidate bins, most flat pixels first.
fn found_inks(bins: &Bins, grid: Grid, merge_distance: f32, max_colors: usize) -> Vec<Ink> {
    // The key breaks ties so the order is total.
    let mut cands: Vec<usize> = (0..BINS).filter(|&k| bins.flat[k] >= MIN_FLAT).collect();
    cands.sort_unstable_by_key(|&k| (std::cmp::Reverse(bins.flat[k]), k));
    let max_colors = max_colors.clamp(1, u16::MAX as usize);
    let mut inks: Vec<Ink> = Vec::new();
    let mut points: Vec<Ink2> = Vec::new();
    for &k in &cands {
        let f = bins.flat[k] as f64;
        let s = bins.flat_sum[k];
        let point = ink2(mean(s, f));
        let (i, d) = nearest(&points, point);
        let joins = !inks.is_empty()
            && (d < SAME_INK
                || (d < merge_distance && inks[i].bins.iter().any(|&b| grid.touch(b, k))));
        if joins || (inks.len() >= max_colors && !inks.is_empty()) {
            // One ink measured twice: its colour is the flat pixels' mean over its bins.
            let ink = &mut inks[i];
            for (t, v) in ink.sum.iter_mut().zip(s) {
                *t += v;
            }
            ink.flat += f;
            ink.bins.push(k);
        } else {
            points.push(point);
            inks.push(Ink {
                point,
                bins: vec![k],
                sum: s,
                flat: f,
            });
        }
    }
    inks
}

/// The inks, and one label (an ink index) per pixel. `alpha`, when given, is the source's
/// opacity per pixel and `rgb` the image over white: inks then carry an opacity, and the
/// clear ground is an ink of its own.
pub(crate) fn palette_and_labels(
    rgb: &[[f32; 3]],
    alpha: Option<&[f32]>,
    w: usize,
    h: usize,
    merge_distance: f32,
    max_colors: usize,
) -> (Palette, Vec<u16>) {
    use rayon::prelude::*;
    let n = w * h;
    let grid = match alpha {
        Some(_) => Grid {
            bits: 4,
            alpha_bits: 4,
        },
        None => Grid {
            bits: 5,
            alpha_bits: 0,
        },
    };
    let px: Vec<[f32; 4]> = (0..n)
        .into_par_iter()
        .map(|p| {
            let c = rgb[p];
            [c[0], c[1], c[2], alpha.map_or(1.0, |a| a[p])]
        })
        .collect();
    let keys: Vec<u16> = px.par_iter().map(|&c| grid.key(c) as u16).collect();
    let bins = histogram(&px, &keys, w, h);

    let mut inks: Vec<[f32; 4]> = found_inks(&bins, grid, merge_distance, max_colors)
        .iter()
        .map(|i| {
            let _ = i.point;
            mean(i.sum, i.flat)
        })
        .collect();
    if inks.is_empty() {
        // Nothing flat anywhere (noise, or a tiny image): one ink, the mean colour.
        let mut s = [0.0f64; 4];
        for c in &px {
            for (t, v) in s.iter_mut().zip(c) {
                *t += *v as f64;
            }
        }
        inks.push(mean(s, n.max(1) as f64));
    }
    for c in inks.iter_mut() {
        c[3] = snap_alpha(c[3]);
    }
    let points: Vec<Ink2> = inks.iter().map(|&c| ink2(c)).collect();

    // Each occupied bin, once: its nearest ink, and whether the bin *is* that ink.
    let mut lut = vec![(0u16, false); BINS];
    let mut bin_point = vec![Ink2::opaque(rgb_to_oklab([0.0; 3])); BINS];
    for k in 0..BINS {
        let m = bins.count[k];
        if m == 0 {
            continue;
        }
        let c = ink2(mean(bins.all_sum[k], m as f64));
        let (i, d) = nearest(&points, c);
        bin_point[k] = c;
        lut[k] = (i as u16, d < merge_distance);
    }

    // A blend takes the nearest ink among its sure neighbours'.
    let sure = |p: usize| lut[keys[p] as usize];
    let mut labels = vec![0u16; n];
    // Row by row in parallel: each label reads only `keys` and the tables, so the result
    // does not depend on the thread count.
    labels
        .par_chunks_mut(w.max(1))
        .enumerate()
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                let p = y * w + x;
                let (own, is_ink) = sure(p);
                if is_ink {
                    *out = own;
                    continue;
                }
                let c = bin_point[keys[p] as usize];
                // Its own nearest ink stays a candidate: a thin stroke has no flat pixel
                // anywhere near, and its partly covered pixels must not all go to the
                // ground beside it. A rim that lands on a third ink is `absorb_slivers`' job.
                let mut best = (own, points[own as usize].dist(c));
                for yy in y.saturating_sub(1)..(y + 2).min(h) {
                    for xx in x.saturating_sub(1)..(x + 2).min(w) {
                        let (l, ok) = sure(yy * w + xx);
                        if ok {
                            let d = points[l as usize].dist(c);
                            if d < best.1 {
                                best = (l, d);
                            }
                        }
                    }
                }
                *out = best.0;
            }
        });

    let mut weight = vec![0f32; inks.len()];
    for &l in &labels {
        weight[l as usize] += 1.0;
    }
    for v in weight.iter_mut() {
        *v /= n.max(1) as f32;
    }
    let rgb_inks: Vec<[f32; 3]> = inks.iter().map(|c| [c[0], c[1], c[2]]).collect();
    (
        Palette {
            colors: rgb_inks.iter().map(|&c| rgb_to_oklab(c)).collect(),
            rgb: rgb_inks,
            weight,
            alpha: inks.iter().map(|c| c[3]).collect(),
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
        let (pal, labels) = palette_and_labels(&img, None, 32, 32, 0.035, 64);
        assert_eq!(pal.len(), 2, "{:?}", pal.rgb);
        assert!(labels.iter().all(|&l| (l as usize) < pal.len()));
        let black = pal.rgb.iter().position(|c| c[0] < 0.1).unwrap() as u16;
        assert_eq!(labels[16 * 32 + 16], black);
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
}
