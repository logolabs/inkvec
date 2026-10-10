//! Codec halos: thin bands of colour that a lossy codec leaves along an edge, put back into
//! the two inks of the edge. Fast mode's clean-up pass for a lossy intake ([`super`],
//! [`RunLabels::absorb_halos`]).
//!
//! # The problem
//!
//! A JPEG stores colour at half resolution (4:2:0) and quantises every 8 × 8 block, so an
//! edge between two inks comes back with a band two or three pixels wide on each side whose
//! luma overshoots or rings and whose chroma has bled a pixel or two across, independently of
//! the luma. Such a band has flat pixels, so Fast's histogram palette founds it as an ink of
//! its own, and its pixels are no blend of the two inks in sRGB: the chroma moved where the
//! luma did not, which puts them 0.08-0.09 sRGB off the line between the inks
//! (`twemoji/1f1fa` on the `web` tier, twice [`super::BLEND_TOL`]), so neither
//! [`RunLabels::absorb_slivers`] nor [`RunLabels::absorb_rims`] recognises them. A
//! two-colour flag came back with four inks and 215 faces.
//!
//! # The test
//!
//! In JPEG's own colour space (BT.601 full-range YCbCr, [`ycc`]) the halo is easy to state:
//! each channel of its colour lies between the two inks' values in that channel, or a little
//! beyond them (overshoot), but the three channels need not agree on *where* between them.
//! A component of the label image is a halo of the inks `a` and `b` when
//!
//! 1. it is **thin**: no pixel of it has a window of radius [`HALO_REACH`] holding only its
//!    own label (a JPEG halo reaches about three pixels from the edge; the "no interior
//!    pixel" rule of the other passes, radius one, misses the wider ones);
//! 2. `a` and `b` are inks of **thick** components near it: bordering it, or bordering a
//!    thin component that borders it (the band between blue and white is a dark halo next
//!    to the blue and a light one next to the white, each touching one of the two inks);
//! 3. the **mean colour** of its pixels lies, channel by channel, in the hull
//!    `[min − τ·|Δ|, max + τ·|Δ|]` of the two inks' values, `τ` = [`HALO_OVERSHOOT`], widened
//!    by [`HALO_SLACK`] for a channel in which the inks barely differ ([`in_hull`]).
//!
//! That is the whole test for a component whose ink has no thick component anywhere in the
//! image: a colour the codec made, which owns no area of its own. A thin component of an ink
//! that does own an area somewhere is held to more: it must be a strip (no pixel whose four
//! neighbours share its label, [`STRIP_REACH`]) and its mean must lie within
//! [`HALO_LINE_TOL`] of the segment between the two inks ([`coverage`]'s distance). The
//! hull alone is a box, wide wherever the two inks differ in a channel, and a real ink can
//! sit in it: `twemoji/1faa3`'s #55ACEE rim, a band four pixels wide between a navy and a
//! blue, lies in the box of either pair around it and 0.054 from the nearer segment, where
//! the codec's halos lie 0.009-0.035 from theirs (`twemoji/1f1fa`, `openmoji/1F1FF-1F1F2`).
//!
//! Of the pairs that pass, the one whose segment the mean lies nearest is taken
//! ([`halo_pair`]). Each pixel then goes to `b` when its coverage `t` of `b` is at least
//! one half and to `a` otherwise, `t` read mostly from luma ([`coverage`]): the chroma is at
//! half resolution and bleeds, the luma carries the edge. Assigning by `t ≥ ½` keeps each
//! ink's area, as a pixel's coverage is the share of it each ink covers.
//!
//! A halo ink usually also leaves specks inside one of the two inks, a pixel or two from
//! the edge, with a single side and no pair. After the first pass, an ink some of whose
//! components were halos and that has no thick component left is a halo ink, and each of
//! its remaining components that borders a single other ink takes that ink. One bordering
//! two or more is left as it is: a ring of it around an eye goes neither to the eye nor to
//! the face around it whole.
//!
//! # When
//!
//! Only on a lossy intake (`ColorOptions::lossy_intake`, which reads the container) and only
//! for an opaque image: a clean render's edges carry no halos, its thin bands are artwork,
//! and its output stays byte-identical. Even then a component is touched only on the
//! evidence of its own pixels: thin, between two inks, and coloured as a halo of them; a
//! raster with no halos (one the trained restorer has cleaned) keeps its components.
//! Cost: one pass over the runs for the thick components at each radius, one over the
//! contacts for the neighbours, and a colour read per pixel of a thin component; O(pixels)
//! at worst, O(runs + thin pixels) in practice.
//!
//! # Literature
//!
//! - Method from: ITU-R BT.601 (and JFIF, which stores JPEG's colour in its full-range
//!   form) for the luma-chroma transform, and the JPEG standard's 4:2:0 chroma subsampling
//!   (ITU-T T.81), which is why chroma is read at half the weight of luma.
//! - See also: Zhang, Liang, Van Gool & Timofte (2021), "Designing a practical degradation
//!   model for deep blind image super-resolution", ICCV, <https://arxiv.org/abs/2103.14006>,
//!   for resizing then JPEG as the degradation real inputs carry; and Bioucas-Dias et al.
//!   (2012), <https://arxiv.org/abs/1202.6294>, for the two-endmember unmixing the
//!   reassignment is (as in [`super::blend_of`]).
//! - Not from the literature: the per-channel hull with overshoot as the test of a halo, the
//!   thinness by window radius, the two-hop sides, and the stricter test for an ink that
//!   owns an area elsewhere.

use super::runs::Run;
use super::{Pixels, RunLabels};

/// How far (in pixels, a window of radius this) a pixel must be from every pixel of another
/// label for its component to count as thick. A JPEG halo reaches about three pixels from
/// the edge (`docs/theory/noise.md`: luma RMS 4.5, 3.1, 2.3 levels at under 1, 1-2 and 2-3 px
/// from an edge, against 0.2 in the interiors), so a band up to four pixels wide, whose
/// middle is two pixels from either side, is thin.
pub(crate) const HALO_REACH: u32 = 2;

/// How far beyond either ink a halo's channel may lie, as a share of the inks' difference in
/// that channel: ringing overshoots the step it rings around.
pub(crate) const HALO_OVERSHOOT: f32 = 0.2;

/// An absolute slack on every channel of the hull (YCbCr, 0..1 units), so a channel in which
/// the two inks barely differ still lets the codec's noise through: about five 8-bit levels.
pub(crate) const HALO_SLACK: f32 = 0.02;

/// The radius at which a thin component of an ink that owns an area elsewhere must have no
/// deep pixel: one, the strip rule of [`RunLabels::absorb_rims`] (no pixel whose four
/// neighbours share its label, here with the eight diagonal ones too). A band of a real ink
/// three or four pixels wide keeps its interior at this radius.
pub(crate) const STRIP_REACH: u32 = 1;

/// How far (YCbCr, [`coverage`]'s weighted distance) from the segment between the two inks
/// the mean of a thin component of an ink that owns an area elsewhere may lie and still be
/// their halo. The codec's halos measured 0.009-0.035 from theirs (`twemoji/1f1fa`,
/// `openmoji/1F1FF-1F1F2`, `web` tier); the real #55ACEE rim of `twemoji/1faa3`, 0.054.
pub(crate) const HALO_LINE_TOL: f32 = 0.04;

/// The weight of each chroma channel against luma's in [`coverage`], on squared differences
/// (half the amplitude): 4:2:0 stores chroma at half resolution, so where luma and chroma
/// disagree about an edge the luma is the better reading of how much of each ink a pixel
/// holds. Two inks that differ only in chroma are still told apart by it.
const CHROMA_WEIGHT: f32 = 0.25;

/// BT.601 full-range YCbCr (JPEG's, as JFIF stores it) of an sRGB 0..1 colour, chroma
/// centred on zero: `Y = 0.299 R + 0.587 G + 0.114 B`,
/// `Cb = −0.168736 R − 0.331264 G + 0.5 B`, `Cr = 0.5 R − 0.418688 G − 0.081312 B`.
pub(crate) fn ycc(c: [f32; 4]) -> [f32; 3] {
    let [r, g, b, _] = c;
    [
        0.299 * r + 0.587 * g + 0.114 * b,
        -0.168_736 * r - 0.331_264 * g + 0.5 * b,
        0.5 * r - 0.418_688 * g - 0.081_312 * b,
    ]
}

/// Whether `c` lies in the hull of `a` and `b` channel by channel: each channel within
/// `[min − τ·|Δ| − ε, max + τ·|Δ| + ε]` of the two inks' values, `τ` = [`HALO_OVERSHOOT`],
/// `ε` = [`HALO_SLACK`]. The channels are tested independently: a halo's luma and chroma
/// need not sit at the same place between the inks.
pub(crate) fn in_hull(c: [f32; 3], a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|k| {
        let span = (b[k] - a[k]).abs();
        let lo = a[k].min(b[k]) - HALO_OVERSHOOT * span - HALO_SLACK;
        let hi = a[k].max(b[k]) + HALO_OVERSHOOT * span + HALO_SLACK;
        (lo..=hi).contains(&c[k])
    })
}

/// `c`'s coverage `t` of `b` on the segment from `a` to `b` in YCbCr, chroma weighted by
/// [`CHROMA_WEIGHT`], and its weighted squared distance from the segment (`t` clamped to
/// 0..1 for the distance only). `None` when the two inks are one colour.
pub(crate) fn coverage(c: [f32; 3], a: [f32; 3], b: [f32; 3]) -> Option<(f32, f32)> {
    let wt = [1.0, CHROMA_WEIGHT, CHROMA_WEIGHT];
    let ab: [f32; 3] = std::array::from_fn(|k| b[k] - a[k]);
    let l2: f32 = (0..3).map(|k| wt[k] * ab[k] * ab[k]).sum();
    if l2 < 1e-9 {
        return None;
    }
    let t = (0..3).map(|k| wt[k] * (c[k] - a[k]) * ab[k]).sum::<f32>() / l2;
    let tc = t.clamp(0.0, 1.0);
    let d = (0..3)
        .map(|k| wt[k] * (c[k] - (a[k] + tc * ab[k])).powi(2))
        .sum::<f32>();
    Some((t, d))
}

/// The pair of inks `(lo, hi)`, `lo < hi`, from the sorted distinct labels `sides`, whose
/// hull holds `mean` ([`in_hull`]) and whose segment it lies nearest ([`coverage`]'s
/// distance), the smaller pair on a tie; `None` when no pair holds it. `inks` are YCbCr.
pub(crate) fn halo_pair(mean: [f32; 3], sides: &[u16], inks: &[[f32; 3]]) -> Option<(u16, u16)> {
    let mut best: Option<(f32, u16, u16)> = None;
    for (i, &lo) in sides.iter().enumerate() {
        for &hi in &sides[i + 1..] {
            let (a, b) = (inks[lo as usize], inks[hi as usize]);
            if !in_hull(mean, a, b) {
                continue;
            }
            let Some((_, d)) = coverage(mean, a, b) else {
                continue;
            };
            if best.is_none_or(|(e, l0, h0)| d < e || (d == e && (lo, hi) < (l0, h0))) {
                best = Some((d, lo, hi));
            }
        }
    }
    best.map(|(_, a, b)| (a, b))
}

/// A list of lists in one buffer: `items[start[i]..start[i + 1]]` is list `i`.
struct Lists<T> {
    start: Vec<usize>,
    items: Vec<T>,
}

impl<T: Copy + Ord> Lists<T> {
    /// Group `pairs` (list index, item) into sorted, deduplicated lists for `n` indices.
    fn of(n: usize, mut pairs: Vec<(u32, T)>) -> Self {
        pairs.sort_unstable();
        pairs.dedup();
        let mut start = vec![0usize; n + 1];
        for &(i, _) in &pairs {
            start[i as usize + 1] += 1;
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        Lists {
            start,
            items: pairs.into_iter().map(|(_, t)| t).collect(),
        }
    }

    fn get(&self, i: usize) -> &[T] {
        &self.items[self.start[i]..self.start[i + 1]]
    }
}

impl RunLabels {
    /// Which components have a *deep* pixel: one whose window of radius `reach` (a square of
    /// side `2·reach + 1`) holds only its own label, the outside of the image counting as
    /// its own label. Needs fresh components.
    ///
    /// **On runs.** Pixel `x` of a run `x0..x1` has its own label across its window's row
    /// when `x − reach .. x + reach` lies in the run or off the image: the stretch
    /// [`deep_span`] of the run. The pixel is deep when that holds in every row of the
    /// window, each time in a run of its label. So a run's deep pixels are its span cut, row
    /// by row, with the spans of the overlapping runs of its label in the `2·reach` rows
    /// around it; the run holds a deep pixel when anything is left. The window is one block
    /// of the label containing the pixel, so every run met is in the pixel's component. A
    /// component already known to be deep is skipped. Cost: per run, the overlapping runs of
    /// `2·reach` rows; O(R · reach) on images whose runs overlap a few others.
    pub(super) fn deep_components(&self, reach: u32) -> Vec<bool> {
        debug_assert!(reach >= 1);
        let (runs, rs, w, h) = (&self.runs, &self.row_start, self.w as u32, self.h);
        let span = |r: &Run| deep_span(r, w, reach);
        let mut deep = vec![false; self.size.len()];
        let (mut cur, mut next): (Vec<(u32, u32)>, Vec<(u32, u32)>) = (Vec::new(), Vec::new());
        for y in 0..h {
            for r in rs[y]..rs[y + 1] {
                let run = runs[r];
                let c = self.run_comp[r] as usize;
                let (s, e) = span(&run);
                if deep[c] || s >= e {
                    continue;
                }
                cur.clear();
                cur.push((s, e));
                let (y0, y1) = (
                    y.saturating_sub(reach as usize),
                    (y + reach as usize).min(h - 1),
                );
                for yy in (y0..=y1).filter(|&yy| yy != y) {
                    next.clear();
                    let row = &runs[rs[yy]..rs[yy + 1]];
                    for &(a, b) in &cur {
                        let mut k = row.partition_point(|q| q.x1 <= a);
                        while k < row.len() && row[k].x0 < b {
                            if row[k].label == run.label {
                                let (s2, e2) = span(&row[k]);
                                let (s3, e3) = (a.max(s2), b.min(e2));
                                if s3 < e3 {
                                    next.push((s3, e3));
                                }
                            }
                            k += 1;
                        }
                    }
                    std::mem::swap(&mut cur, &mut next);
                    if cur.is_empty() {
                        break;
                    }
                }
                deep[c] = !cur.is_empty();
            }
        }
        deep
    }

    /// Each component's neighbouring components (sharing a pixel edge), sorted. Needs fresh
    /// components. O(E log E) for the E contacts.
    fn neighbours(&self) -> Lists<u32> {
        let mut pairs: Vec<(u32, u32)> = Vec::new();
        self.for_each_contact(|i, j, _| {
            let (a, b) = (self.run_comp[i], self.run_comp[j]);
            if a != b {
                pairs.push((a, b));
                pairs.push((b, a));
            }
        });
        Lists::of(self.size.len(), pairs)
    }

    /// Give the codec's halos back to the inks of the edge they lie along: see the module
    /// documentation for the test, the reassignment and the second pass. `px` must be
    /// opaque (no opacity channel) and `inks` sRGB 0..1; every label must index into them.
    /// Returns the number of components given back in each of the two passes.
    pub(crate) fn absorb_halos(&mut self, px: Pixels<'_>, inks: &[[f32; 4]]) -> (usize, usize) {
        self.components();
        let inks_ycc: Vec<[f32; 3]> = inks.iter().map(|&c| ycc(c)).collect();
        let pair = self.halo_pairs(px, &inks_ycc);
        let first = pair.iter().filter(|p| p.is_some()).count();
        if first == 0 {
            return (0, 0);
        }
        let mut was_halo = vec![false; inks.len()];
        for (c, p) in pair.iter().enumerate() {
            if p.is_some() {
                was_halo[self.label[c] as usize] = true;
            }
        }
        // Every pixel of a halo goes to the ink it covers more of. Decided for every pixel
        // before any is written, in scan order, as `apply_pixel_edits` needs.
        let mut edits = Vec::new();
        for y in 0..self.h {
            for r in self.row_start[y]..self.row_start[y + 1] {
                let Some((a, b)) = pair[self.run_comp[r] as usize] else {
                    continue;
                };
                let (ya, yb) = (inks_ycc[a as usize], inks_ycc[b as usize]);
                let Run { x0, x1, .. } = self.runs[r];
                for x in x0..x1 {
                    let v = ycc(px.get(y * self.w + x as usize));
                    let to = match coverage(v, ya, yb) {
                        Some((t, _)) if t >= 0.5 => b,
                        _ => a,
                    };
                    edits.push((r as u32, x, to));
                }
            }
        }
        self.apply_pixel_edits(&edits);
        let second = self.absorb_halo_specks(&was_halo);
        (first, second)
    }

    /// The pair of inks each component is a halo of ([`RunLabels::absorb_halos`]'s first
    /// pass), `None` for one that is not a halo; one entry per component. Needs fresh
    /// components. `inks` are YCbCr ([`ycc`]); labels outside them are never halos.
    fn halo_pairs(&self, px: Pixels<'_>, inks: &[[f32; 3]]) -> Vec<Option<(u16, u16)>> {
        let n_inks = inks.len();
        let n = self.size.len();
        let deep = self.deep_components(HALO_REACH);
        let strip_deep = self.deep_components(STRIP_REACH);
        let nbrs = self.neighbours();
        let in_palette = |c: usize| (self.label[c] as usize) < n_inks;
        // Inks with an area of their own somewhere: a thin component of one is held to the
        // stricter test.
        let mut has_area = vec![false; n_inks];
        for c in (0..n).filter(|&c| deep[c] && in_palette(c)) {
            has_area[self.label[c] as usize] = true;
        }
        let candidate = |c: usize| {
            !deep[c] && in_palette(c) && !(has_area[self.label[c] as usize] && strip_deep[c])
        };
        // The inks of each component's thick neighbours.
        let thick_sides = Lists::of(
            n,
            (0..n)
                .flat_map(|c| nbrs.get(c).iter().map(move |&o| (c as u32, o)))
                .filter(|&(_, o)| deep[o as usize])
                .map(|(c, o)| (c, self.label[o as usize]))
                .filter(|&(_, l)| (l as usize) < n_inks)
                .collect(),
        );
        // The mean YCbCr of every candidate, in one pass over the runs.
        let mut sum = vec![[0.0f64; 3]; n];
        for y in 0..self.h {
            for r in self.row_start[y]..self.row_start[y + 1] {
                let c = self.run_comp[r] as usize;
                if !candidate(c) {
                    continue;
                }
                let Run { x0, x1, .. } = self.runs[r];
                for x in x0..x1 {
                    let v = ycc(px.get(y * self.w + x as usize));
                    for k in 0..3 {
                        sum[c][k] += v[k] as f64;
                    }
                }
            }
        }
        let mut pair: Vec<Option<(u16, u16)>> = vec![None; n];
        let mut sides: Vec<u16> = Vec::new();
        for c in (0..n).filter(|&c| candidate(c)) {
            sides.clear();
            sides.extend_from_slice(thick_sides.get(c));
            for &o in nbrs.get(c).iter().filter(|&&o| !deep[o as usize]) {
                sides.extend_from_slice(thick_sides.get(o as usize));
            }
            sides.sort_unstable();
            sides.dedup();
            sides.retain(|&l| l != self.label[c]);
            if sides.len() < 2 {
                continue;
            }
            let mean = sum[c].map(|v| (v / self.size[c] as f64) as f32);
            pair[c] = halo_pair(mean, &sides, inks).filter(|&(a, b)| {
                !has_area[self.label[c] as usize]
                    || coverage(mean, inks[a as usize], inks[b as usize])
                        .is_some_and(|(_, d)| d <= HALO_LINE_TOL * HALO_LINE_TOL)
            });
        }
        pair
    }

    /// The second pass of [`RunLabels::absorb_halos`]: an ink some of whose components were
    /// halos (`was_halo`, per ink) and that has no thick component left is a halo ink, and
    /// each of its components that borders exactly one ink that is not a halo ink takes that
    /// ink; one bordering two or more, or none, stays. Labels are read as they were before
    /// the pass. Returns how many components were relabelled.
    fn absorb_halo_specks(&mut self, was_halo: &[bool]) -> usize {
        self.components();
        let deep = self.deep_components(HALO_REACH);
        let mut halo_ink = was_halo.to_vec();
        for (c, &d) in deep.iter().enumerate() {
            let l = self.label[c] as usize;
            if d && l < halo_ink.len() {
                halo_ink[l] = false;
            }
        }
        if !halo_ink.contains(&true) {
            return 0;
        }
        let is_halo = |l: u16| halo_ink.get(l as usize).copied().unwrap_or(false);
        // (halo component, the label across the border).
        let mut sides: Vec<(u32, u16)> = Vec::new();
        self.for_each_contact(|i, j, _| {
            let (ci, cj) = (self.run_comp[i], self.run_comp[j]);
            let (li, lj) = (self.runs[i].label, self.runs[j].label);
            if ci != cj && is_halo(li) && !is_halo(lj) {
                sides.push((ci, lj));
            }
            if ci != cj && is_halo(lj) && !is_halo(li) {
                sides.push((cj, li));
            }
        });
        sides.sort_unstable();
        sides.dedup();
        let mut to = self.label.clone();
        let mut moved = 0;
        for group in sides.chunk_by(|a, b| a.0 == b.0) {
            if let [(c, l)] = group {
                to[*c as usize] = *l;
                moved += 1;
            }
        }
        if moved > 0 {
            self.relabel_components(&to);
        }
        moved
    }
}

/// The stretch `s..e` of run `r` whose pixels have their own label for `reach` pixels on
/// either side within the row (the image's edge counting as their own); empty (`s ≥ e`)
/// when the run is too short. `w` is the image width.
fn deep_span(r: &Run, w: u32, reach: u32) -> (u32, u32) {
    let s = if r.x0 == 0 { 0 } else { r.x0 + reach };
    let e = if r.x1 == w {
        w
    } else {
        r.x1.saturating_sub(reach)
    };
    (s, e)
}

#[cfg(test)]
mod tests;
