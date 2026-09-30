//! The Fast palette exactly as it shipped at 55ee4e0, frozen as the test oracle.
//!
//! Every speed-up in [`super`] is an *exact* rewrite: same inks to the last bit, same label
//! for every pixel. This module keeps the original code -- dense 65 536-entry per-bin tables,
//! a four-channel copy of the image, a serial pixel-by-pixel histogram, `f32::round` in the
//! bin key, one `label()` call per pixel and a running `f32` share per ink -- so that the tests
//! in [`super`] can compare the two on random and degenerate images. It is compiled only for
//! tests and must not be "improved": its value is that it is the old behaviour, verbatim
//! except for paths and names.

use crate::color::{rgb_to_oklab, Palette};
use crate::fast::faces::{blend_of, is_blend, BLEND_TOL};
use crate::native::{over_black, snap_alpha, Ink2, OPAQUE};

const BINS: usize = 1 << 16;
const MIN_FLAT: u32 = 3;
const SAME_INK: f32 = 0.012;
const MAX_THIN_CANDIDATES: usize = 48;
const MIN_PAIRED: u32 = 8;
const THIN_SHARE: f32 = 0.0005;
const REACH: usize = 4;
const KEEP_OWN: f32 = 3.0;

/// The original bin grid, with `f32::round` (half away from zero).
#[derive(Clone, Copy)]
struct Grid {
    bits: u32,
    alpha_bits: u32,
}

impl Grid {
    /// The original key: each channel clamped to 0..1, scaled to `2^bits − 1` levels and
    /// rounded half away from zero, packed most significant first.
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

    /// The quantised levels of key `k`.
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

    /// Whether no level of `a` and `b` differs by more than one.
    fn touch(self, a: usize, b: usize) -> bool {
        let (p, q) = (self.parts(a), self.parts(b));
        p.iter().zip(q).all(|(x, y)| (x - y).abs() <= 1)
    }
}

/// The original dense per-bin statistics.
struct Bins {
    count: Vec<u32>,
    flat: Vec<u32>,
    paired: Vec<u32>,
    flat_sum: Vec<[f64; 4]>,
    all_sum: Vec<[f64; 4]>,
}

/// The original serial histogram, pixel by pixel in raster order.
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

/// `s / f` per channel, rounded to f32.
fn mean(s: [f64; 4], f: f64) -> [f32; 4] {
    [
        (s[0] / f) as f32,
        (s[1] / f) as f32,
        (s[2] / f) as f32,
        (s[3] / f) as f32,
    ]
}

/// The two-ground point of a colour with its opacity.
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

/// The first nearest ink and its distance; `(0, ∞)` with no inks.
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

/// One accepted ink while the palette is built.
struct Ink {
    bins: Vec<usize>,
    sum: [f64; 4],
    flat: f64,
}

/// The original leader clustering of flat bins.
fn found_inks(bins: &Bins, grid: Grid, merge_distance: f32, max_colors: usize) -> Vec<Ink> {
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

/// The original thin-ink pass.
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
                .any(|&ib| is_blend(picked[i], ia, ib))
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

/// The original blend labeller's tables.
struct Blends<'a> {
    px: &'a [[f32; 4]],
    keys: &'a [u16],
    lut: &'a [(u16, bool)],
    bin_point: &'a [Ink2],
    inks: &'a [[f32; 4]],
    points: &'a [Ink2],
    near: f32,
    w: usize,
    h: usize,
}

impl Blends<'_> {
    /// The distinct sure-pixel inks within `r` of (x, y), in raster order.
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

    /// The original per-pixel label.
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
        let made_of = |i: u16| {
            let ci = self.inks[i as usize];
            let tol2 = BLEND_TOL * BLEND_TOL;
            (0..4).map(|k| (col[k] - ci[k]).powi(2)).sum::<f32>() <= tol2
                || around
                    .iter()
                    .any(|&l| l != i && is_blend(col, ci, self.inks[l as usize]))
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

    /// The original `explain`.
    fn explain(&self, col: [f32; 4], around: &[u16]) -> Option<u16> {
        let mut best: Option<(u16, f32)> = None;
        let mut consider = |l: u16, d: f32| {
            if d <= BLEND_TOL * BLEND_TOL && best.is_none_or(|(_, e)| d < e) {
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
                if let Some((t, d)) = blend_of(col, ia, ib) {
                    consider(if t < 0.5 { a } else { b as u16 }, d);
                }
            }
        }
        best.map(|(l, _)| l)
    }
}

/// `palette_and_labels` as it shipped at 55ee4e0.
pub(super) fn palette_and_labels(
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

/// The original channel level, rounded by `f32::round`, for the rounding tests in
/// [`super`].
pub(super) fn level(v: f32, bits: u32) -> usize {
    let top = ((1usize << bits) - 1) as f32;
    (v.clamp(0.0, 1.0) * top).round() as usize
}
