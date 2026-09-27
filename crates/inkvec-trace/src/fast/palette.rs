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
//! whose bin is not itself an ink colour (a blend) takes the nearest of the inks its
//! ink-coloured neighbours carry, or its own nearest ink when it is a blend of that ink and
//! one of theirs (a thin stroke over its ground) -- never an ink its pixels are not made of.

use crate::color::{rgb_to_oklab, Palette};
use crate::native::{over_black, snap_alpha, Ink2, OPAQUE};

/// Bins: 16-bit keys.
const BINS: usize = 1 << 16;
/// Flat pixels a bin needs before it can found an ink.
const MIN_FLAT: u32 = 3;
/// Inks this close (OKLab) are one ink whether or not their bins touch.
const SAME_INK: f32 = 0.012;
/// Most candidates [`thin_inks`] weighs (the blend test is cubic in them).
const MAX_THIN_CANDIDATES: usize = 48;
/// Paired pixels a bin needs to be a thin ink: a stroke 8 px long.
const MIN_PAIRED: u32 = 8;
/// And the share of the image they must be: 8 px on a 128 px icon, 2100 on 2048 px.
const THIN_SHARE: f32 = 0.0005;

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
    /// Pixels with at least one of their four neighbours in the same bin: the bin's
    /// pixels that belong to a run of it, not to scattered noise.
    paired: Vec<u32>,
    flat_sum: Vec<[f64; 4]>,
    all_sum: Vec<[f64; 4]>,
}

fn histogram(px: &[[f32; 4]], keys: &[u16], w: usize, h: usize) -> Bins {
    let mut b = Bins {
        count: vec![0; BINS],
        flat: vec![0; BINS],
        paired: vec![0; BINS],
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
            let (l, r) = (x > 0 && same(p - 1), x + 1 < w && same(p + 1));
            let (u, d) = (y > 0 && same(p - w), y + 1 < h && same(p + w));
            if l || r || u || d {
                b.paired[ku] += 1;
            }
            let flat = (x == 0 || l) && (x + 1 == w || r) && (y == 0 || u) && (y + 1 == h || d);
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

/// Inks with no flat pixel: small text and hairlines, whose every pixel touches the ground.
///
/// Flatness is the evidence [`found_inks`] reads, and a stroke two or three pixels wide has
/// none, so black text beside a red mark would have no black ink at all: its pixels then
/// take the nearest ink there is, and the text comes out red. Quality mode admits a colour
/// by its share of the image and drops it when it is a blend of two inks; this is the same
/// test on the bins, with the share counted in *paired* pixels (see [`Bins::paired`]) so
/// that a pair of eyes on a 128 px emoji counts and scattered noise does not. A bin with
/// at least [`MIN_PAIRED`] and [`THIN_SHARE`] of the image in paired pixels, not within
/// `merge_distance` of an ink, is a candidate; candidates are then dropped, least
/// populous first, while they lie on the line between two other inks or candidates -- the
/// grey rim of the text is a blend of the text and the paper, the text itself is not.
fn thin_inks(
    bins: &Bins,
    used: &[bool],
    inks: &[[f32; 4]],
    n: usize,
    merge_distance: f32,
    max_colors: usize,
) -> Vec<[f32; 4]> {
    if inks.len() >= max_colors || inks.is_empty() {
        return Vec::new();
    }
    let min_paired = ((THIN_SHARE as f64 * n as f64).ceil() as u32).max(MIN_PAIRED);
    let mut cands: Vec<usize> = (0..BINS)
        .filter(|&k| !used[k] && bins.paired[k] >= min_paired)
        .collect();
    cands.sort_unstable_by_key(|&k| (std::cmp::Reverse(bins.paired[k]), k));
    let mut points: Vec<Ink2> = inks.iter().map(|&c| ink2(c)).collect();
    // Candidate colours, most paired pixels first.
    let mut picked: Vec<[f32; 4]> = Vec::new();
    for &k in &cands {
        let c = mean(bins.all_sum[k], bins.count[k] as f64);
        let p = ink2(c);
        if nearest(&points, p).1 < merge_distance {
            continue;
        }
        points.push(p);
        picked.push(c);
        if picked.len() >= MAX_THIN_CANDIDATES {
            break;
        }
    }
    let mut keep = vec![true; picked.len()];
    for i in (0..picked.len()).rev() {
        let others: Vec<[f32; 4]> = inks
            .iter()
            .chain(
                picked
                    .iter()
                    .enumerate()
                    .filter(|&(j, _)| j != i && keep[j])
                    .map(|(_, c)| c),
            )
            .copied()
            .collect();
        let blend = others.iter().enumerate().any(|(a, &ia)| {
            others[a + 1..]
                .iter()
                .any(|&ib| super::faces::is_blend(picked[i], ia, ib))
        });
        keep[i] = !blend;
    }
    picked
        .into_iter()
        .zip(keep)
        .filter(|&(_, k)| k)
        .map(|(c, _)| c)
        .take(max_colors - inks.len())
        .collect()
}

/// What labelling a blend pixel reads: the pixels, their bins, and the inks.
struct Blends<'a> {
    px: &'a [[f32; 4]],
    keys: &'a [u16],
    /// Per bin: its nearest ink, and whether the bin is that ink.
    lut: &'a [(u16, bool)],
    /// Per bin: its mean colour.
    bin_point: &'a [Ink2],
    inks: &'a [[f32; 4]],
    points: &'a [Ink2],
    /// How near (OKLab) its nearest ink must be for a pixel nothing around explains to
    /// keep it.
    near: f32,
    w: usize,
    h: usize,
}

impl Blends<'_> {
    /// The distinct inks of the sure pixels within `r` of (x, y).
    fn around(&self, x: usize, y: usize, r: usize, out: &mut Vec<u16>) {
        out.clear();
        for yy in y.saturating_sub(r)..(y + r + 1).min(self.h) {
            for xx in x.saturating_sub(r)..(x + r + 1).min(self.w) {
                let (l, ok) = self.lut[self.keys[yy * self.w + xx] as usize];
                if ok && !out.contains(&l) {
                    out.push(l);
                }
            }
        }
    }

    /// The label of pixel (x, y). An ink-coloured pixel keeps its ink. Any other pixel
    /// takes the nearer of its own nearest ink and its ink-coloured neighbours' nearest,
    /// when it is made of that ink: is it, or a blend of it and an ink around. Its own
    /// nearest is how a thin stroke keeps its ink: it has no flat pixel anywhere near, and
    /// its partly covered pixels must not all go to the ground beside it.
    ///
    /// Nearest in colour alone is not enough: the grey rim between black text and white
    /// paper lies nearer a red used elsewhere than either -- or a red touching it -- and is
    /// no blend of red with anything around it. A pixel not made of its nearest ink takes
    /// the ink it is mostly made of ([`Blends::explain`]). A pixel nothing around explains
    /// -- a thin band of an ink too small to be in the palette -- keeps its own nearest ink
    /// when that is near it ([`KEEP_OWN`]), and its neighbours' nearest otherwise.
    ///
    /// Neighbours are the ink-coloured pixels within one pixel, or, when there are none
    /// (small text downscaled is all blends), within up to [`REACH`].
    fn label(&self, x: usize, y: usize) -> u16 {
        let p = y * self.w + x;
        let (own, is_ink) = self.lut[self.keys[p] as usize];
        if is_ink {
            return own;
        }
        let mut around = Vec::with_capacity(9);
        for r in 1..=REACH {
            self.around(x, y, r, &mut around);
            if !around.is_empty() {
                break;
            }
        }
        if around.is_empty() {
            return own;
        }
        let c = self.bin_point[self.keys[p] as usize];
        let mut best = (own, f32::INFINITY);
        for &l in &around {
            let d = self.points[l as usize].dist(c);
            if d < best.1 {
                best = (l, d);
            }
        }
        let col = self.px[p];
        // Whether the pixel is ink `i`, or a blend of it and an ink around.
        let made_of = |i: u16| {
            let ci = self.inks[i as usize];
            let tol2 = super::faces::BLEND_TOL * super::faces::BLEND_TOL;
            (0..4).map(|k| (col[k] - ci[k]).powi(2)).sum::<f32>() <= tol2
                || around
                    .iter()
                    .any(|&l| l != i && super::faces::is_blend(col, ci, self.inks[l as usize]))
        };
        let d_own = self.points[own as usize].dist(c);
        let nearest = if d_own < best.1 { own } else { best.0 };
        if made_of(nearest) {
            return nearest;
        }
        if let Some(l) = self.explain(col, &around) {
            return l;
        }
        if d_own < self.near {
            own
        } else {
            best.0
        }
    }

    /// The ink a pixel of colour `col` is mostly made of, when it is a blend of an ink in
    /// `around` and any ink: of all such pairs the one it lies nearest the line of, and
    /// the side of it that covers more of the pixel -- `absorb_slivers`' rule. A pixel
    /// within the blend tolerance of an ink in `around` is that ink.
    fn explain(&self, col: [f32; 4], around: &[u16]) -> Option<u16> {
        let mut best: Option<(u16, f32)> = None;
        let mut consider = |l: u16, d: f32| {
            if d <= super::faces::BLEND_TOL * super::faces::BLEND_TOL
                && best.is_none_or(|(_, e)| d < e)
            {
                best = Some((l, d));
            }
        };
        for &a in around {
            let ia = self.inks[a as usize];
            consider(a, (0..4).map(|k| (col[k] - ia[k]).powi(2)).sum());
            for (b, &ib) in self.inks.iter().enumerate() {
                if b == a as usize {
                    continue;
                }
                if let Some((t, d)) = super::faces::blend_of(col, ia, ib) {
                    consider(if t < 0.5 { a } else { b as u16 }, d);
                }
            }
        }
        best.map(|(l, _)| l)
    }
}

/// Farthest (in pixels) a blend looks for ink-coloured neighbours.
const REACH: usize = 4;
/// A pixel nothing around explains keeps its nearest ink only within this many
/// `merge_distance`s of it: a thin band of a near-black that missed the palette keeps the
/// black beside it in colour, a grey rim does not become red.
const KEEP_OWN: f32 = 3.0;

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

    let found = found_inks(&bins, grid, merge_distance, max_colors);
    let mut used = vec![false; BINS];
    for i in &found {
        for &b in &i.bins {
            used[b] = true;
        }
    }
    let mut inks: Vec<[f32; 4]> = found
        .iter()
        .map(|i| {
            let _ = i.point;
            mean(i.sum, i.flat)
        })
        .collect();
    let thin = thin_inks(&bins, &used, &inks, n, merge_distance, max_colors);
    inks.extend(thin);
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

    let blends = Blends {
        px: &px,
        keys: &keys,
        lut: &lut,
        bin_point: &bin_point,
        inks: &inks,
        points: &points,
        near: KEEP_OWN * merge_distance,
        w,
        h,
    };
    let mut labels = vec![0u16; n];
    // Row by row in parallel: each label reads only `keys` and the tables, so the result
    // does not depend on the thread count.
    labels
        .par_chunks_mut(w.max(1))
        .enumerate()
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                *out = blends.label(x, y);
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
}
