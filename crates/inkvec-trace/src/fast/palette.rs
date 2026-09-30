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
//!
//! Stage 1 of the Fast pipeline. In: the image over white as sRGB 0..1 per pixel, and the
//! source opacity when transparency is traced natively. Out: a [`Palette`] (sRGB, OKLab,
//! opacity and pixel share per ink) and one ink index per pixel. Called once per trace from
//! [`super::front`].

use crate::color::{rgb_to_oklab, Palette};
use crate::native::{over_black, snap_alpha, Ink2, OPAQUE};

/// The palette as it shipped before the exact speed-ups, kept verbatim as the tests' oracle.
#[cfg(test)]
mod reference;

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
///
/// An opaque image uses 5 bits per sRGB channel and none for opacity (15-bit keys); an
/// image traced with its transparency uses 4 + 4 + 4 colour bits and 4 opacity bits. Both
/// fit the 16-bit key space of [`BINS`].
#[derive(Clone, Copy)]
struct Grid {
    bits: u32,
    alpha_bits: u32,
}

impl Grid {
    /// The bin of a colour `c = [r, g, b, a]` (sRGB 0..1 and opacity 0..1).
    ///
    /// Each channel is clamped to 0..1 and rounded to the nearest of `2^bits` levels,
    /// `q(v) = round(v · (2^bits − 1))`; the key packs `q(r) q(g) q(b)` from the most
    /// significant end, followed by `q(a)` on `alpha_bits` when opacity is binned. Rounding
    /// (not truncation) puts a pure colour at the centre of its bin, so noise around it
    /// spreads into both neighbours evenly.
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

    /// The inverse of [`Grid::key`] on the quantised levels: `[q(r), q(g), q(b), q(a)]`,
    /// with `q(a) = 0` when opacity is not binned. Signed, so that [`Grid::touch`] can
    /// subtract levels.
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

/// One pass over the `w × h` image: per bin, how many pixels fall in it (`count`, with
/// their colour sums `all_sum`), how many are *paired* (at least one 4-neighbour in the
/// same bin) and how many are *flat* (every 4-neighbour inside the image in the same bin,
/// with their sums `flat_sum`).
///
/// A neighbour outside the image counts as agreeing, so a filled region touching the
/// border keeps its flat pixels there. Sums are in f64 so a 2048 px image's totals do not
/// lose the low bits of each 0..1 channel. `px` is sRGB 0..1 plus opacity; `keys` is
/// [`Grid::key`] of each pixel.
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

/// The mean colour-and-opacity `s / f` of `f` summed pixels. The caller guarantees
/// `f > 0` (a bin or ink with at least one pixel, or `n.max(1)`).
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

/// The index of the ink nearest `c` by [`Ink2::dist`] (OKLab distance, the larger over
/// the two grounds), and that distance. The first ink wins a tie. With no inks it returns
/// `(0, ∞)`, which every caller treats as "no ink near enough".
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
    bins: Vec<usize>,
    sum: [f64; 4],
    flat: f64,
}

/// Found inks from the candidate bins, most flat pixels first.
///
/// A greedy clustering in a single pass. Candidates are the bins with at least
/// [`MIN_FLAT`] flat pixels, sorted by flat count (bin key breaking ties, so the order is
/// total and the result deterministic). Each candidate's flat mean is compared with the
/// inks founded so far, whose points stay where their founding bin put them:
///
/// * it joins the nearest ink `i` when `d < SAME_INK`, or when `d < merge_distance` and
///   one of `i`'s bins touches it ([`Grid::touch`]): one ink spread over adjacent bins by
///   noise or anti-aliasing;
/// * once `max_colors` inks exist, every further candidate joins its nearest;
/// * otherwise it founds a new ink.
///
/// `d` is [`Ink2::dist`] in OKLab. A joined bin adds its flat sums to the ink, so the
/// ink's final colour is the flat-pixel mean over all its bins, not its founder's colour.
/// Most populous first means the dominant colour of a cluster founds it, which keeps a
/// faint neighbour bin from pulling the ink off its true colour.
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

    /// The label of a *blend* pixel (x, y): one whose bin is not itself an ink colour, so
    /// the lookup table only gave it its nearest ink `own`. (An ink-coloured, "sure", pixel
    /// keeps its ink; [`label_rows`] settles those inline and never calls this.) The pixel
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
    ///
    /// A pure function of the keys in the 9 × 9 window around (x, y), the tables and the
    /// pixel's own colour: it reads no other label, so pixels may be labelled in any order
    /// and on any thread. Cost: a window of 9 to 81 table reads, then O(|A| + K) distances
    /// and, in `explain`, O(|A| · K) blend tests for K inks and |A| inks around. Blends are
    /// 0.29 % of pixels at 2048 px and 4.4 % on the 128 px screen set (medians).
    #[inline(never)]
    fn blend_label(&self, x: usize, y: usize, own: u16) -> u16 {
        let p = y * self.w + x;
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
///
/// Steps: bin every pixel ([`Grid::key`], [`histogram`]); found inks from flat bins
/// ([`found_inks`]) and add stroke inks with no flat pixel ([`thin_inks`]); snap each
/// ink's opacity ([`snap_alpha`]); give every occupied bin its nearest ink once, marking it
/// "is that ink" when within `merge_distance` (OKLab); then label each pixel from that table
/// ([`label_rows`], [`Blends::blend_label`]). With no flat bin anywhere (pure noise, or a tiny image) the palette
/// is one ink, the mean colour of the image.
///
/// Outputs: the [`Palette`] with `rgb` (sRGB 0..1), `colors` (OKLab of `rgb`), `alpha`
/// (0..1) and `weight` (each ink's share of the labels, summing to 1), and `w · h` labels,
/// every one a valid ink index. `max_colors` caps the palette (flat inks and thin inks
/// together); [`found_inks`] clamps it to `1..=65535` so a label fits a `u16`. Labelling
/// runs row by row in parallel but reads only immutable tables, so the result does not
/// depend on the thread count.
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
    let mut inks: Vec<[f32; 4]> = found.iter().map(|i| mean(i.sum, i.flat)).collect();
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
    // does not depend on the thread count. Each worker also counts its labels; integer
    // counts add up to the same totals in any order.
    let n_inks = inks.len();
    let count = labels
        .par_chunks_mut(w.max(1))
        .enumerate()
        .fold(
            || vec![0usize; n_inks],
            |mut count, (y, row)| {
                label_rows(&blends, y, row, &mut count);
                count
            },
        )
        .reduce(
            || vec![0usize; n_inks],
            |mut a, b| {
                for (s, v) in a.iter_mut().zip(b) {
                    *s += v;
                }
                a
            },
        );

    let weight = ink_shares(&count, n);
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

/// Label the pixels of `rows` -- whole image rows, the first of them row `y0` -- and add each
/// ink's pixel count to `count` (indexed by ink).
///
/// Per pixel `p` with bin key `k_p`, the lookup table gives `(own, sure) = lut[k_p]`. A sure
/// pixel's label is `own` and is written here, inline; only the others go to
/// [`Blends::blend_label`]. That split is the whole change from the per-pixel call it
/// replaces: 99.7 % of pixels are sure at 2048 px and 95.6 % on the 128 px screen set
/// (medians), and for them the call was pure overhead -- 3.3 ms of the stage at 2048 px,
/// 1.0 ms inline (labels identical on 254 images).
///
/// *Why identical:* the old `label(x, y)` began `if sure { return own }`, and
/// `blend_label` is the rest of that function verbatim, so each pixel gets the same value
/// from the same expression.
///
/// Method from: M. J. Swain, D. H. Ballard, "Color Indexing", IJCV 7(1):11–32, 1991,
/// DOI 10.1007/BF00130487 -- histogram backprojection: a pixel is labelled by looking its
/// colour bin up in a table built from the histogram. Adapted: the table holds the bin's
/// nearest ink and whether the bin *is* that ink, and the pixels it cannot decide go to the
/// neighbourhood rule.
///
/// The counts are taken per run of equal labels, in a register, and added once when the
/// run ends. 99.6 % of neighbouring labels are equal on the 7-image 2048 px set, so the
/// per-pixel `count[l] += 1` made every iteration wait on the store of the one before it
/// (store-to-load forwarding on one address). Θ(pixels) reads, Θ(runs) count updates;
/// an empty `rows` changes nothing.
///
/// Inspired by: Y. Collet, FiniteStateEntropy `lib/hist.c`, `HIST_count_parallel_wksp`,
/// <https://github.com/Cyan4973/FiniteStateEntropy/blob/dev/lib/hist.c>, which breaks the
/// same chain with four sub-tables ("noticeably faster when some values are heavily
/// repeated"). Ours counts runs instead: labels are an image, so repeats come in runs, and
/// a run needs no second table.
fn label_rows(blends: &Blends, y0: usize, rows: &mut [u16], count: &mut [usize]) {
    let w = blends.w.max(1);
    let keys = &blends.keys[y0 * w..y0 * w + rows.len()];
    for (dy, (row, krow)) in rows.chunks_mut(w).zip(keys.chunks(w)).enumerate() {
        for (x, (out, &k)) in row.iter_mut().zip(krow).enumerate() {
            let (own, sure) = blends.lut[k as usize];
            *out = if sure {
                own
            } else {
                blends.blend_label(x, y0 + dy, own)
            };
        }
    }
    add_label_runs(rows, count);
}

/// Add to `count[l]` the number of entries of `labels` equal to `l`, one update per run of
/// equal entries (see [`label_rows`] for why). Every entry must be a valid index into
/// `count`. An empty slice adds nothing.
fn add_label_runs(labels: &[u16], count: &mut [usize]) {
    let mut it = labels.iter();
    let Some(&first) = it.next() else {
        return;
    };
    let (mut cur, mut run) = (first, 1usize);
    for &l in it {
        if l == cur {
            run += 1;
        } else {
            count[cur as usize] += run;
            (cur, run) = (l, 1);
        }
    }
    count[cur as usize] += run;
}

/// Each ink's share of the `n` pixels, `Palette::weight`: `min(count_i, 2²⁴) / max(n, 1)`,
/// computed in f32.
///
/// # Why this is the number the old code gave
///
/// The old code summed `1.0` into an f32 per pixel, then divided by `n`. Adding 1.0 to an
/// f32 holding an integer below 2²⁴ is exact (the result is an integer ≤ 2²⁴, which has a
/// 24-bit significand), so up to 2²⁴ pixels the running sum equals the integer count. At
/// 2²⁴ it stops: 2²⁴ + 1 lies halfway between 2²⁴ and 2²⁴ + 2, and round-half-to-even keeps
/// 2²⁴. So the old sum was exactly `min(count, 2²⁴)`, and `min(count, 2²⁴) as f32` is that
/// value converted exactly; the division is the same f32 operation on the same operands.
///
/// Nothing in the workspace reads `weight` (checked with `git grep` at 7a4e054), but
/// [`Palette`] is a public type, so the field keeps its meaning rather than being dropped.
/// It used to cost 23 % of the palette stage at 2048 px (9.8 ms), a serial chain of
/// dependent f32 additions; counted per run it is under a millisecond.
///
/// Not from the literature: an exactness argument about f32 integer sums, because nothing
/// published covers replacing a running float count with an integer one bit for bit.
/// See also: D. Goldberg, "What Every Computer Scientist Should Know About Floating-Point
/// Arithmetic", ACM Computing Surveys, March 1991,
/// <https://docs.oracle.com/cd/E19957-01/806-3568/ncg_goldberg.html> (IEEE 754 operations are
/// "computed exactly and then rounded").
fn ink_shares(counts: &[usize], n: usize) -> Vec<f32> {
    /// Where an f32 running count of ones stops growing.
    const F32_COUNT_LIMIT: usize = 1 << 24;
    counts
        .iter()
        .map(|&c| c.min(F32_COUNT_LIMIT) as f32 / n.max(1) as f32)
        .collect()
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
        let (po, lo) =
            reference::palette_and_labels(&c.rgb, a, c.w, c.h, merge_distance, max_colors);
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

    #[test]
    fn the_palette_equals_the_shipped_one_bit_for_bit() {
        for c in cases() {
            for (md, mc) in [(0.035, 64), (0.01, 64), (0.035, 3), (0.2, 2)] {
                assert_same(&c, md, mc);
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
}
