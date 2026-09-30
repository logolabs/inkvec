//! Fast mode's gradients: posterised ramps put back together, one fit per ramp.
//!
//! A smooth gradient has no flat colour, so the palette quantises it into bands, and each
//! band is a face with a boundary of its own: on a gradient-filled icon that is most of the
//! document. Quality mode merges bands pairwise under its MDL objective, refitting after
//! every merge; this is the one-shot version. Adjacent faces whose inks are a small step
//! apart are joined into clusters, each cluster is fitted once with the quality fitter's
//! own gradient models (linear, radial, elliptical), and the fit is kept only when it
//! explains the cluster's pixels at least about as well as the flat bands did. A step
//! between two genuinely different inks cannot be fitted by a ramp, fails that test and
//! stays two faces.
//!
//! Stage 3 of the Fast pipeline, after [`super::faces::faces`] and only for opaque images
//! with gradients on. In: the image over white (sRGB 0..1), the palette, and a face id per
//! pixel with each face's fill and ink. Out: the same three rewritten, with each accepted
//! cluster renumbered as one face carrying a gradient fill. Called from [`super::front`].
//!
//! # Passes
//!
//! 1. **Palette precheck** ([`inks_may_join`]): two faces can only be joined when two
//!    different inks lie within [`RAMP_STEP`]; when none do, the stage returns before
//!    reading a pixel. That settles most images: 5 of the 7 opaque 2048 px test images,
//!    18 of 26 at 512 px.
//! 2. **Row runs** (`planar::runs`): the face map coded once as maximal runs per row.
//! 3. **Contacts** ([`contacts`]): the border length of every touching face pair, from the
//!    runs, and union-find over the pairs close enough in colour. No join: return.
//! 4. **Samples** ([`gather_samples`]): pixel counts and a grid sample per cluster, from
//!    the runs.
//! 5. **Fit and test**, in parallel per cluster, then **renumber** ([`merge_ramps`]).
//!
//! Passes 1 to 4 were rewritten on 2026-09-30 from per-pixel scans to the palette and the
//! runs; each function states why its result is the same as the scan's, bit for bit.

use crate::color::{rgb_to_oklab, Palette};
use crate::gradient::{self, FillFit, FillModel};
use crate::planar::runs::{overlaps, RowRuns};
use std::collections::HashMap;

/// Largest OKLab distance between two adjacent faces' inks for them to be two bands of
/// one ramp. The palette's own merge distance is 0.035, so bands of a ramp sit one or two
/// of those apart.
const RAMP_STEP: f32 = 0.09;
/// Pixels a boundary must share before it counts as adjacency rather than a corner touch.
const MIN_CONTACT: u32 = 3;
/// Smallest cluster, in pixels, worth a gradient.
const MIN_PIXELS: usize = 64;
/// Most pixels gathered per cluster for its fit, as an even grid over the cluster.
const SAMPLE_PIXELS: usize = 65536;
/// Most pixels the acceptance test samples per cluster.
const CHECK_SAMPLES: usize = 4096;
/// A gradient is kept when its RMS residual is at most this multiple of the flat bands'
/// residual, plus [`SLACK`]: it replaces every band boundary in the cluster with one fill,
/// so it may explain the colour slightly worse and still be the better document.
const RATIO: f64 = 1.25;
/// Two flat bands whose RMS residual is at most this, in sRGB units, are left as they are.
const FLAT_ENOUGH: f64 = 1.5 / 255.0;
/// A gradient whose RMS residual is at most this, in sRGB units, is kept however well the
/// bands did: two display levels are not visible, and the bands' boundaries are.
const INVISIBLE: f64 = 2.0 / 255.0;
/// Absolute slack on the acceptance test, in sRGB units (one display level).
const SLACK: f64 = 1.0 / 255.0;

/// Union-find root of face `x`, with path halving.
fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

/// Contact length between every pair of adjacent faces, in pixel edges: the number of
/// 4-neighbour pixel pairs with one pixel in each face, as `((min, max), length)` sorted by
/// face pair, one entry per pair that touches at all.
///
/// Read off the row runs instead of the pixels. A horizontal pixel pair with two different
/// labels is exactly a boundary between two consecutive runs of a row (runs are maximal),
/// and counts 1. The vertical pairs between rows `y − 1` and `y` are covered by the
/// [`overlaps`] of the two rows' runs, each an interval `lo..hi` over which both rows are
/// constant, and count `hi − lo` when the two labels differ; two equal rows have none and
/// are skipped. Each event is keyed `(min << 16) | max`, which orders exactly as the pair
/// `(min, max)` does; the events are sorted by key and equal keys summed.
///
/// # Why this equals the pixel count
///
/// Every counted pixel pair is counted once, by the partition argument above, and the sums
/// are integers, so the order of addition cannot matter. The pixel version returned a hash
/// map that [`merge_ramps`] then sorted by `((min, max), length)`; keys are unique, so that
/// sort is by key alone and gives exactly this vector. `contacts_scan`, kept under
/// `#[cfg(test)]`, is the pixel version, and the tests compare the two.
///
/// # Cost
///
/// `O(runs)` after the run coding, against `O(w · h)` map updates before: 4.6 ms of the
/// 14.8 ms ramp stage at 2048 px (mean over the seven opaque `big` images).
///
/// Method from: He, Chao & Suzuki 2008, "A Run-Based Two-Scan Labeling Algorithm", IEEE TIP
/// 17(5) 749–756, <https://doi.org/10.1109/TIP.2008.919369> (row runs, merged row against
/// row); adapted to measure the length of each run pair's contact instead of connecting
/// them. Inspired by: Ji, Piper & Tang 1989, "Erosion and dilation of binary images by
/// arbitrary structuring elements using interval coding", Pattern Recognition Letters 9(3)
/// 201–209, <https://doi.org/10.1016/0167-8655(89)90055-X>: computing on the interval code
/// rather than the pixels.
fn contacts(runs: &RowRuns, h: usize) -> Vec<((u16, u16), u32)> {
    let key = |a: u16, b: u16| (u32::from(a.min(b)) << 16) | u32::from(a.max(b));
    let mut events: Vec<(u32, u32)> = Vec::new();
    for y in 0..h {
        let row = runs.row(y);
        for pair in row.windows(2) {
            events.push((key(pair[0].label, pair[1].label), 1));
        }
        if y > 0 && !runs.same_as_above(y) {
            overlaps(runs.row(y - 1), row, |lo, hi, a, b| {
                if a != b {
                    events.push((key(a, b), hi - lo));
                }
            });
        }
    }
    events.sort_unstable_by_key(|e| e.0);
    let mut out: Vec<((u16, u16), u32)> = Vec::new();
    for (k, len) in events {
        let pair = ((k >> 16) as u16, (k & 0xFFFF) as u16);
        match out.last_mut() {
            Some(last) if last.0 == pair => last.1 += len,
            _ => out.push((pair, len)),
        }
    }
    out
}

/// Contact length between every pair of adjacent faces, pixel by pixel: the form
/// [`contacts`] replaced, kept as its test reference. Keys are `(min, max)` face ids.
#[cfg(test)]
fn contacts_scan(labels: &[u16], w: usize, h: usize) -> HashMap<(u16, u16), u32> {
    let mut out: HashMap<(u16, u16), u32> = HashMap::new();
    let mut touch = |a: u16, b: u16| {
        if a != b {
            *out.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    };
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            if x + 1 < w {
                touch(labels[p], labels[p + 1]);
            }
            if y + 1 < h {
                touch(labels[p], labels[p + w]);
            }
        }
    }
    out
}

/// Whether two faces of the map could be joined into one ramp at all: some two *different*
/// inks used by the faces lie within [`RAMP_STEP`] of each other in OKLab. When none do,
/// [`merge_ramps`] is provably a no-op and returns before touching the pixels.
///
/// `face_color[f]` is face `f`'s palette index; a face's colour is the one [`merge_ramps`]
/// compares, `rgb_to_oklab(pal.rgb[c])`, with black for an index past the palette (both
/// take the same fallback, so they compute the same `Oklab` for the same index).
///
/// # Why a `false` here means nothing would change
///
/// [`merge_ramps`] joins two faces only when they touch (share a 4-neighbour pixel pair)
/// and their colours are within `RAMP_STEP`. The faces are the 4-connected components of
/// the ink map (`faces::faces`), so two pixels that are 4-neighbours and carry the same
/// ink are in the same face: two faces that touch always have **different** inks. Their
/// distance is then the distance between two different inks' colours, computed by the same
/// `Oklab::dist` on the same values (the distance is exactly symmetric, since
/// `(u − v)² = (v − u)²` in floating point). So if every pair of different inks is farther
/// than `RAMP_STEP`, no join happens, every face is its own cluster, no cluster has two
/// members, and `merge_ramps` returns 0 with the labels and fills untouched — the value
/// this early return gives.
///
/// One exception is guarded: past `u16::MAX − 1` components, `faces::faces` folds the rest
/// into face 0, which then holds pixels of several inks under one colour, and the argument
/// fails. With that many faces this returns `true` and the full pass decides.
///
/// # Cost
///
/// `O(n log n)` to list the distinct inks and `O(k²)` colour distances for `k` inks, against
/// a full pass over the label map (contacts 4.6 ms at 2048 px). The pass was provably empty
/// on 5 of the 7 opaque 2048 px images, 18 of 26 at 512 px and 3 of 5 at 128 px (research
/// 2026-09-30).
///
/// Not from the literature: a necessary condition for a join, checked on the palette before
/// the image, because the join rule is ours. See also: He & Chao 2015, "A Very Fast
/// Algorithm for Simultaneously Performing Connected-Component Labeling and Euler Number
/// Computing", IEEE TIP 24(9) 2725–2735, <https://doi.org/10.1109/TIP.2015.2425540>, on
/// deciding region facts without a second pass over the pixels.
fn inks_may_join(face_color: &[usize], pal: &Palette) -> bool {
    /// Faces past this count are folded into face 0 by `faces::faces`; see above.
    const FOLDED: usize = (u16::MAX - 1) as usize;
    if face_color.len() >= FOLDED {
        return true;
    }
    let mut inks: Vec<usize> = face_color.to_vec();
    inks.sort_unstable();
    inks.dedup();
    let lab: Vec<_> = inks
        .iter()
        .map(|&c| rgb_to_oklab(pal.rgb.get(c).copied().unwrap_or([0.0; 3])))
        .collect();
    // `<=` or NaN: the negation of the join test's `dist > RAMP_STEP`, which a NaN
    // distance passes.
    (0..lab.len()).any(|i| {
        (i + 1..lab.len()).any(|j| {
            let d = lab[i].dist(lab[j]);
            d <= RAMP_STEP || d.is_nan()
        })
    })
}

/// RMS residual over sampled pixels of the cluster's interior: of `model` (zero without
/// one), and of each pixel's own flat band colour.
///
/// Over every `step`-th pixel `p` of `pixels`, `step = max(1, |pixels| / CHECK_SAMPLES)`:
///
/// `g = sqrt(Σ_p Σ_k (I_k(p) − M_k(p))² / 3N)`, `f = sqrt(Σ_p Σ_k (I_k(p) − B_k(p))² / 3N)`
///
/// with `I` the image, `M` the model evaluated at the pixel's integer coordinates, `B` the
/// band's flat colour, `k` over the three sRGB channels (0..1) and `N` the pixels read.
/// Both are in sRGB units. An empty `pixels` gives `(0, 0)`.
fn residuals(
    rgb: &[[f32; 3]],
    w: usize,
    pixels: &[usize],
    model: Option<&FillModel>,
    band: impl Fn(usize) -> [f32; 3],
) -> (f64, f64) {
    let step = (pixels.len() / CHECK_SAMPLES).max(1);
    let (mut g, mut f, mut n) = (0.0f64, 0.0f64, 0usize);
    for &p in pixels.iter().step_by(step) {
        let c = rgb[p];
        let m = model.map_or([0.0; 3], |m| m.color_at((p % w) as f64, (p / w) as f64));
        let b = band(p);
        for k in 0..3 {
            g += ((c[k] - m[k]) as f64).powi(2);
            f += ((c[k] - b[k]) as f64).powi(2);
        }
        n += 3;
    }
    let n = n.max(1) as f64;
    ((g / n).sqrt(), (f / n).sqrt())
}

/// The grid sample of every cluster worth a fit, as `(root, pixel indices)` sorted by root,
/// each list in increasing pixel index `p = y · w + x`.
///
/// A cluster (union-find root `r`) is sampled when it has more than one face
/// (`members[r] > 1`) and at least [`MIN_PIXELS`] pixels (`count[r]`); its sample is every
/// pixel of the cluster with `x` and `y` both multiples of its stride `s = stride[r]`.
///
/// Read off the runs: in a row `y` that is a multiple of `s`, a run of the cluster
/// contributes `x = ⌈x0 / s⌉ · s, … < x1` in steps of `s`. Rows are visited in increasing `y`
/// and a row's runs in increasing `x`, so each list comes out in increasing `p`, the order
/// the per-pixel pass produced; the fit sums over the list in that order, so the order is
/// part of the result. The per-pixel pass also returned the clusters in hash order and
/// sorted them by root, and roots are unique, so the order here (sorted by root) is the
/// same. Divisibility is tested as `y == (y / s) · s`, without `%`: wazero's arm64 compiler
/// once miscompiled `i32.rem_u` in a hot loop (commit 55ee4e0, in `boundary_opt`).
///
/// `O(runs + samples)` against `O(w · h)` hash-map probes before (5.9 ms of the ramp stage
/// at 2048 px, mean over the seven opaque `big` images).
///
/// Method from: He, Chao & Suzuki 2008, "A Run-Based Two-Scan Labeling Algorithm", IEEE TIP
/// 17(5) 749–756, <https://doi.org/10.1109/TIP.2008.919369>: per-region data gathered from
/// the runs rather than the pixels.
fn gather_samples(
    runs: &RowRuns,
    w: usize,
    h: usize,
    root: &[usize],
    members: &[usize],
    count: &[usize],
    stride: &[usize],
) -> Vec<(usize, Vec<usize>)> {
    let mut slot: HashMap<usize, usize> = HashMap::new();
    let mut clusters: Vec<(usize, Vec<usize>)> = Vec::new();
    for y in 0..h {
        for run in runs.row(y) {
            let r = root[run.label as usize];
            if members[r] <= 1 || count[r] < MIN_PIXELS {
                continue;
            }
            let s = stride[r];
            if y != (y / s) * s {
                continue;
            }
            let (x0, x1) = (run.x0 as usize, run.x1 as usize);
            // The first multiple of `s` at or after `x0`, again without `%` (so not
            // `usize::div_ceil`, which computes a remainder).
            let below = x0 / s * s;
            let first = if below < x0 { below + s } else { below };
            if first >= x1 {
                continue;
            }
            let k = *slot.entry(r).or_insert_with(|| {
                clusters.push((r, Vec::new()));
                clusters.len() - 1
            });
            clusters[k]
                .1
                .extend((first..x1).step_by(s).map(|x| y * w + x));
        }
    }
    clusters.sort_unstable_by_key(|c| c.0);
    clusters
}

/// Merge ramps of bands into gradient faces. `labels` are face ids; the three per-face
/// vectors are rewritten with the merged faces renumbered. Returns the number of gradient
/// faces made.
///
/// 1. **Clusters.** Faces are joined by union-find when they share at least
///    [`MIN_CONTACT`] pixel edges and their inks lie within [`RAMP_STEP`] in OKLab. Pairs
///    are visited in sorted order and the smaller root wins, so the clustering does not
///    depend on hash order.
/// 2. **Samples.** Each cluster of two or more faces and at least [`MIN_PIXELS`] pixels is
///    sampled on an even grid of stride `s = ceil(sqrt(count / SAMPLE_PIXELS))`.
/// 3. **Fit and test**, in parallel per cluster. The flat bands' residual `f` and, after
///    the fit, the gradient's `g` are measured on the sampled pixels whose four neighbours
///    are all in the cluster ([`residuals`]), so the anti-aliased rim does not count
///    against either. A pair of faces with `f <= FLAT_ENOUGH` is two flat inks and is not
///    fitted. Otherwise the quality fitter's models are tried (`gradient::fit_pixels`
///    at the noise floor and the BIC penalty `gradient::bic_lambda`, then
///    `gradient::select`), and a gradient is kept when
///    `g <= max(RATIO · f + SLACK, INVISIBLE)`.
/// 4. **Renumber.** Faces of an accepted cluster become one face with the gradient fill
///    and the root face's ink; every other face keeps its fill. New ids follow the order
///    of the old ones.
///
/// Before step 1 the palette is checked ([`inks_may_join`]): when no two different inks lie
/// within [`RAMP_STEP`], no pair can pass the join test and the function returns 0 at once;
/// after step 1, when no pair joined, it returns 0 the same way. Steps 1 and 2 read the row
/// runs of `labels` ([`contacts`], [`gather_samples`]) rather than its pixels.
///
/// Preconditions: `labels` are the faces of `faces::faces`, the 4-connected components of
/// the ink map, with `face_color` their inks (the palette precheck relies on it), and every
/// label is below `face_color.len()`.
///
/// Edge cases: fewer than two faces, or no accepted cluster, leaves everything untouched
/// and returns 0. A cluster with no interior sample reads both residuals as 0, so a pair
/// is left flat and a ramp of three or more bands is accepted on the fit alone.
pub(crate) fn merge_ramps(
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pal: &Palette,
    labels: &mut [u16],
    face_fill: &mut Vec<FillFit>,
    face_color: &mut Vec<usize>,
) -> usize {
    use rayon::prelude::*;
    let n = face_color.len();
    if n < 2 || !inks_may_join(face_color, pal) {
        return 0;
    }
    let lab: Vec<_> = face_color
        .iter()
        .map(|&c| rgb_to_oklab(pal.rgb.get(c).copied().unwrap_or([0.0; 3])))
        .collect();
    let runs = RowRuns::new(labels, w, h);
    let mut parent: Vec<usize> = (0..n).collect();
    // `contacts` returns the pairs sorted, as the sort that used to follow it did.
    let mut joined = false;
    for ((a, b), len) in contacts(&runs, h) {
        let (a, b) = (a as usize, b as usize);
        if a >= n || b >= n || len < MIN_CONTACT || lab[a].dist(lab[b]) > RAMP_STEP {
            continue;
        }
        joined = true;
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent[ra.max(rb)] = ra.min(rb);
        }
    }
    // No pair joined: every face is its own cluster, no cluster has two members, and the
    // passes below would gather nothing, fit nothing and return 0 with nothing changed.
    if !joined {
        return 0;
    }
    let root: Vec<usize> = (0..n).map(|f| find(&mut parent, f)).collect();
    let mut members = vec![0usize; n];
    for &r in &root {
        members[r] += 1;
    }
    // Pixels per cluster that has more than one face, then a grid sample of each: a fit
    // reads a few thousand samples, so gathering millions of pixels would buy nothing.
    // Both passes read the runs, not the pixels (`gather_samples`).
    let mut count = vec![0usize; n];
    for y in 0..h {
        for run in runs.row(y) {
            let r = root[run.label as usize];
            if members[r] > 1 {
                count[r] += (run.x1 - run.x0) as usize;
            }
        }
    }
    let stride: Vec<usize> = count
        .iter()
        .map(|&c| ((c as f64 / SAMPLE_PIXELS as f64).sqrt().ceil() as usize).max(1))
        .collect();
    let clusters = gather_samples(&runs, w, h, &root, &members, &count, &stride);
    let sigma = crate::coverage::NOISE_FLOOR;
    let lambda = gradient::bic_lambda(w * h);
    let labels_ro: &[u16] = labels;
    let fits: Vec<Option<(usize, FillFit)>> = clusters
        .par_iter()
        .map(|(r, px)| {
            let member = |p: usize| root[labels_ro[p] as usize] == *r;
            let interior: Vec<usize> = px
                .iter()
                .copied()
                .filter(|&p| {
                    let (x, y) = (p % w, p / w);
                    x > 0 && y > 0 && x + 1 < w && y + 1 < h && {
                        [p - 1, p + 1, p - w, p + w].iter().all(|&q| member(q))
                    }
                })
                .collect();
            let band = |p: usize| {
                let c = face_color[labels_ro[p] as usize];
                pal.rgb.get(c).copied().unwrap_or([0.0; 3])
            };
            // Bands that already explain their pixels are flat inks side by side, not a
            // quantised ramp: nothing for a gradient to win, and the fit is the costly part.
            let (_, f) = residuals(rgb, w, &interior, None, band);
            // Two faces that already explain their pixels are two flat inks side by side. A
            // ramp of three or more bands is fitted whatever: fine bands explain the
            // colour well and still cost a boundary each.
            if f <= FLAT_ENOUGH && members[*r] < 3 {
                return None;
            }
            let best = gradient::select(gradient::fit_pixels(
                rgb,
                w,
                h,
                px,
                member,
                |_| true,
                sigma,
                lambda,
            ));
            if !best.model.is_gradient() {
                return None;
            }
            let (g, _) = residuals(rgb, w, &interior, Some(&best.model), band);
            (g <= (RATIO * f + SLACK).max(INVISIBLE)).then_some((*r, best))
        })
        .collect();
    let mut made = 0;
    let mut fill_of_root: HashMap<usize, FillFit> = HashMap::new();
    for (r, fit) in fits.into_iter().flatten() {
        fill_of_root.insert(r, fit);
        made += 1;
    }
    if made == 0 {
        return 0;
    }
    // Renumber: a merged cluster becomes its root's face; everything else keeps its own.
    let target = |f: usize| {
        if fill_of_root.contains_key(&root[f]) {
            root[f]
        } else {
            f
        }
    };
    let mut new_id = vec![u16::MAX; n];
    let mut next = 0u16;
    let (mut fills, mut colors) = (Vec::new(), Vec::new());
    for f in 0..n {
        let t = target(f);
        if new_id[t] == u16::MAX {
            new_id[t] = next;
            next += 1;
            fills.push(
                fill_of_root
                    .get(&t)
                    .cloned()
                    .unwrap_or_else(|| face_fill[t].clone()),
            );
            colors.push(face_color[t]);
        }
    }
    let final_id: Vec<u16> = (0..n).map(|f| new_id[target(f)]).collect();
    for l in labels.iter_mut() {
        *l = final_id[*l as usize];
    }
    *face_fill = fills;
    *face_color = colors;
    made
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pal(colors: &[[f32; 3]]) -> Palette {
        Palette {
            colors: colors.iter().map(|&c| rgb_to_oklab(c)).collect(),
            rgb: colors.to_vec(),
            weight: vec![1.0 / colors.len() as f32; colors.len()],
            alpha: vec![1.0; colors.len()],
        }
    }

    fn flat(c: [f32; 3]) -> FillFit {
        FillFit {
            model: FillModel::Flat(c),
            chi2: 0.0,
            params: gradient::PARAMS_FLAT,
            cost: 0.0,
        }
    }

    #[test]
    fn a_banded_ramp_becomes_one_gradient_face() {
        let (w, h) = (64, 32);
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|p| {
                let t = (p % w) as f32 / (w - 1) as f32;
                [0.2 + 0.6 * t, 0.3, 0.8 - 0.5 * t]
            })
            .collect();
        // Four bands, each labelled with its mean colour.
        let bands: Vec<[f32; 3]> = (0..4)
            .map(|b| {
                let t = (b as f32 + 0.5) / 4.0;
                [0.2 + 0.6 * t, 0.3, 0.8 - 0.5 * t]
            })
            .collect();
        let p = pal(&bands);
        let mut labels: Vec<u16> = (0..w * h).map(|q| ((q % w) / 16) as u16).collect();
        let mut fills: Vec<FillFit> = bands.iter().map(|&c| flat(c)).collect();
        let mut colors: Vec<usize> = (0..4).collect();
        let made = merge_ramps(&rgb, w, h, &p, &mut labels, &mut fills, &mut colors);
        assert_eq!(made, 1);
        assert_eq!(fills.len(), 1);
        assert!(fills[0].model.is_gradient());
        assert!(labels.iter().all(|&l| l == 0));
    }

    #[test]
    fn a_step_between_two_close_inks_stays_two_faces() {
        let (w, h) = (64, 32);
        let a = [0.80f32, 0.40, 0.30];
        let b = [0.74f32, 0.36, 0.28];
        let rgb: Vec<[f32; 3]> = (0..w * h).map(|p| if p % w < 32 { a } else { b }).collect();
        let p = pal(&[a, b]);
        let mut labels: Vec<u16> = (0..w * h).map(|q| u16::from(q % w >= 32)).collect();
        let mut fills = vec![flat(a), flat(b)];
        let mut colors = vec![0, 1];
        assert_eq!(
            merge_ramps(&rgb, w, h, &p, &mut labels, &mut fills, &mut colors),
            0
        );
        assert_eq!(fills.len(), 2);
    }

    /// The per-pixel sample gather `merge_ramps` did before [`gather_samples`], kept as its
    /// reference: every pixel of a sampled cluster on the stride grid, hashed by root, then
    /// sorted by root.
    fn gather_samples_scan(
        labels: &[u16],
        w: usize,
        root: &[usize],
        members: &[usize],
        count: &[usize],
        stride: &[usize],
    ) -> Vec<(usize, Vec<usize>)> {
        let mut cluster_px: HashMap<usize, Vec<usize>> = HashMap::new();
        for (p, &l) in labels.iter().enumerate() {
            let r = root[l as usize];
            let s = stride[r];
            if members[r] > 1
                && count[r] >= MIN_PIXELS
                && (p % w).is_multiple_of(s)
                && (p / w).is_multiple_of(s)
            {
                cluster_px.entry(r).or_default().push(p);
            }
        }
        let mut clusters: Vec<(usize, Vec<usize>)> = cluster_px.into_iter().collect();
        clusters.sort_unstable_by_key(|c| c.0);
        clusters
    }

    /// xorshift64, so the tests need no dependency.
    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n.max(1)
        }
    }

    /// Ink maps: random noise, blocks, stripes, one ink, a checkerboard, on shapes from one
    /// pixel, one row and one column up. Returned as faces (the precondition of
    /// `merge_ramps`) with each face's ink.
    fn face_maps(rng: &mut Rng) -> Vec<(Vec<u16>, Vec<usize>, usize, usize)> {
        let mut out = Vec::new();
        for (w, h) in [
            (1usize, 1usize),
            (23, 1),
            (1, 23),
            (9, 7),
            (31, 29),
            (64, 40),
        ] {
            let inks = 2 + rng.below(5) as u16;
            let noise: Vec<u16> = (0..w * h).map(|_| rng.below(inks as u64) as u16).collect();
            let bw = w.div_ceil(4);
            let blocks_of: Vec<u16> = (0..bw * h.div_ceil(4))
                .map(|_| rng.below(inks as u64) as u16)
                .collect();
            let blocks: Vec<u16> = (0..w * h)
                .map(|p| blocks_of[(p / w / 4) * bw + (p % w) / 4])
                .collect();
            let stripes: Vec<u16> = (0..w * h)
                .map(|p| ((p % w) / 3 % inks as usize) as u16)
                .collect();
            let checker: Vec<u16> = (0..w * h).map(|p| ((p % w + p / w) % 2) as u16).collect();
            for ink_map in [noise, blocks, stripes, vec![0u16; w * h], checker] {
                let (labels, face_color) = super::super::faces::faces(&ink_map, w, h);
                out.push((labels, face_color, w, h));
            }
        }
        out
    }

    #[test]
    fn contacts_from_runs_equal_the_pixel_count() {
        let mut rng = Rng(0x243F_6A88_85A3_08D3);
        for (labels, _, w, h) in face_maps(&mut rng) {
            let mut old: Vec<((u16, u16), u32)> =
                contacts_scan(&labels, w, h).into_iter().collect();
            old.sort_unstable();
            let runs = RowRuns::new(&labels, w, h);
            assert_eq!(contacts(&runs, h), old, "{w}x{h}");
        }
        // Labels at the top of the range, including the virtual outside's `u16::MAX`.
        let (w, h) = (5, 4);
        let labels: Vec<u16> = (0..w * h)
            .map(|p| {
                if p % 3 == 0 {
                    u16::MAX
                } else {
                    u16::MAX - 1 - (p % 2) as u16
                }
            })
            .collect();
        let mut old: Vec<((u16, u16), u32)> = contacts_scan(&labels, w, h).into_iter().collect();
        old.sort_unstable();
        assert_eq!(contacts(&RowRuns::new(&labels, w, h), h), old);
    }

    #[test]
    fn samples_from_runs_equal_the_pixel_gather() {
        let mut rng = Rng(0x1319_8A2E_0370_7344);
        for (labels, face_color, w, h) in face_maps(&mut rng) {
            let n = face_color.len();
            // Random clusters over the faces, and small strides so several grids are hit.
            let root: Vec<usize> = (0..n)
                .map(|f| {
                    if rng.below(3) == 0 {
                        f
                    } else {
                        rng.below(f as u64 + 1) as usize
                    }
                })
                .map(|r| r.min(n - 1))
                .collect();
            let root: Vec<usize> = (0..n).map(|f| root[root[f]].min(root[f])).collect();
            let mut members = vec![0usize; n];
            for &r in &root {
                members[r] += 1;
            }
            let mut count = vec![0usize; n];
            for &l in &labels {
                if members[root[l as usize]] > 1 {
                    count[root[l as usize]] += 1;
                }
            }
            for s in 1..4 {
                let stride = vec![s; n];
                let runs = RowRuns::new(&labels, w, h);
                assert_eq!(
                    gather_samples(&runs, w, h, &root, &members, &count, &stride),
                    gather_samples_scan(&labels, w, &root, &members, &count, &stride)
                );
            }
        }
    }

    /// The palette precheck's claim, checked by brute force: when it says no two inks can
    /// join, no touching pair of faces is within `RAMP_STEP`, and `merge_ramps` changes
    /// nothing.
    #[test]
    fn a_palette_with_no_close_inks_admits_no_join() {
        let mut rng = Rng(0xA409_3822_299F_31D0);
        let mut skipped = 0;
        for (labels, face_color, w, h) in face_maps(&mut rng) {
            for _ in 0..4 {
                let n_inks = face_color.iter().max().map_or(0, |&m| m + 1);
                let colors: Vec<[f32; 3]> = (0..n_inks)
                    .map(|_| {
                        [
                            rng.below(256) as f32 / 255.0,
                            rng.below(256) as f32 / 255.0,
                            rng.below(256) as f32 / 255.0,
                        ]
                    })
                    .collect();
                let p = pal(&colors);
                if inks_may_join(&face_color, &p) {
                    continue;
                }
                skipped += 1;
                let lab = |f: u16| rgb_to_oklab(colors[face_color[f as usize]]);
                for ((a, b), _) in contacts_scan(&labels, w, h) {
                    assert!(
                        lab(a).dist(lab(b)) > RAMP_STEP,
                        "faces {a} and {b} could join"
                    );
                }
                let rgb: Vec<[f32; 3]> = labels
                    .iter()
                    .map(|&l| colors[face_color[l as usize]])
                    .collect();
                let (mut l2, mut fc) = (labels.clone(), face_color.clone());
                let mut fills: Vec<FillFit> = fc.iter().map(|&c| flat(colors[c])).collect();
                assert_eq!(merge_ramps(&rgb, w, h, &p, &mut l2, &mut fills, &mut fc), 0);
                assert_eq!((l2, fc), (labels.clone(), face_color.clone()));
            }
        }
        assert!(skipped > 0, "no case exercised the precheck");
    }
}
