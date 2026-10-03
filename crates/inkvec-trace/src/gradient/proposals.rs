//! Gradient region proposals inside smooth segments, accepted by MDL. Research prototype
//! A10, parts `segments` and `mdl` ([`super::gregions`]).
//!
//! # Problem
//!
//! The band merger ([`super::bands`]) grows gradient regions two components at a time,
//! and only from a pair whose union is already a gradient on the evidence the merger
//! trusts: pixels that are not blends towards a neighbour's ink. In a quantised ramp every
//! pixel lies between two band inks, so a band has no evidence of its own, every band fits
//! flat, and the ramp stays cut. On the 168 gradient icons, 301 of the 524 artist gradients
//! we lose are lost here. [`super::segments`] finds the regions a gradient *could* fill
//! without the palette; this module asks, segment by segment, which groups of the band
//! merger's components one fill explains, and hands the accepted groups to the merger as
//! single components before its pairwise agglomeration starts.
//!
//! # Steps, per smooth segment
//!
//! 1. **Members.** A component belongs to the segment holding at least half of its pixels
//!    ([`assign_to_segments`]); components that are mostly discontinuity (anti-aliased
//!    rims) belong to none and are left to the merger.
//! 2. **The whole segment.** Its members, when they are connected through the merger's
//!    adjacency, are fitted as one region ([`fit_set`]) with the segment's *smooth*
//!    pixels as evidence -- not the merger's evidence mask, which calls every pixel of a
//!    banded ramp a blend. If one fill explains them, that is the group.
//! 3. **Growth** otherwise: from the largest member not yet taken, the adjacent member
//!    whose union explains best is added, while adding it still pays; each grown group is
//!    kept when its fill is a gradient. Inspired by: G. Lecot, B. Lévy (2006), Ardeco:
//!    Automatic Region DEtection and COnversion, EGSR (region growing under a fit
//!    criterion). The work is capped per segment ([`MAX_GROW_FITS`]).
//!
//! A segment of one member is refitted on its smooth pixels too: the palette did not band
//! it, but its own fit took its evidence from the merger's mask, which may have left it
//! none.
//!
//! # Acceptance: "one fill explains them"
//!
//! * **Threshold** (part `segments` alone; the port of acda7ac, after Chakraborty et al.
//!   2025, doi:10.1111/cgf.70055, §3.4, which accepts the minimum-error fill "provided the
//!   error is below a specified threshold"): the union's mean largest-channel residual on
//!   the group's smooth pixels is at most `max(2/255, 3σ)` ([`GROW_TOL_LEVELS`]). The
//!   round-2 research measured it: as-one 52–56 %, but dE00 +2.3 to +3.8 % from groups
//!   merged across weak edges, because an absolute tolerance accepts a union that explains
//!   its pixels far worse than the fills it replaces, as long as "far worse" is under two
//!   levels.
//! * **MDL** (parts `segments,mdl`): the union must lower the description length of the
//!   pixels it covers against the fills it replaces, both priced on the same pixels
//!   ([`group_gain`]): `gain = ½·Σ (e_split² − e_union²)/σ² + λ·(Σ params_split −
//!   params_union) > 0`. This is the band merger's own objective (`0.5·chi² + λ·params`,
//!   the MDL cost of the whole module) and its common-pixel pricing (`common_pixel_gain`
//!   in `bands.rs`) generalised to k members. Inspired by: S. C. Zhu, A. Yuille (1996),
//!   Region Competition, IEEE TPAMI 18(9), doi:10.1109/34.537343, whose step 6 merges
//!   adjacent regions only when the merge lowers the Bayes/MDL energy. Across a weak edge
//!   the union misfits both sides of the step on many pixels; the split side does not, and
//!   its few extra parameters cost less than the misfit. See also: J. Rissanen (1978),
//!   Modeling by shortest data description, Automatica 14(5),
//!   doi:10.1016/0005-1098(78)90005-5.
//!
//! # Cost
//!
//! One fit per segment for the whole-segment proposal, plus up to [`MAX_GROW_FITS`] per
//! segment that grows. Each fit gathers at most `FIT_PIXELS_CAP` pixels and evaluates at
//! most `fit_cap()` of them, like every band-merger fit. Segments are independent and run
//! in parallel; the output order is the segments' order, whatever the thread count.

use std::collections::{BTreeMap, HashMap};

use super::segments::DISCONTINUITY;
use super::{
    collect_samples, fit_cap, fit_pixels, select, FillFit, FIT_PIXELS_CAP, QUANT_HALF_STEP,
};

/// Mean residual, in 8-bit levels of the largest sRGB channel, under which the threshold
/// acceptance calls a group explained by one fill (floored at `3σ`). acda7ac's value.
pub(crate) const GROW_TOL_LEVELS: f64 = 2.0;

/// Most union fits the growth step of one segment may make. Growth is quadratic in the
/// segment's members (each step refits the union with every frontier member); this bounds
/// a segment of hundreds of flecks to a fixed amount of work. Counted per segment, so the
/// bound is the same whatever the thread schedule. Not tuned: a 128 px noto emoji's largest
/// segment has a few dozen members.
pub(crate) const MAX_GROW_FITS: usize = 256;

/// What the proposals read: the image, the segmentation, and the band merger's state
/// before its first round.
pub(crate) struct Inputs<'a> {
    /// The composited sRGB image, 0..1.
    pub(crate) rgb: &'a [[f32; 3]],
    /// Image width, px.
    pub(crate) w: usize,
    /// Image height, px.
    pub(crate) h: usize,
    /// Smooth region per pixel, or [`DISCONTINUITY`] ([`super::segments::smooth_segments`]).
    pub(crate) seg: &'a [u32],
    /// Pixels of each component.
    pub(crate) members: &'a [Vec<usize>],
    /// Whether each component is live.
    pub(crate) alive: &'a [bool],
    /// The merger's component adjacency (seam lengths); pairs it vetoes are absent.
    pub(crate) adj: &'a [HashMap<u32, u32>],
    /// Component of each pixel.
    pub(crate) group: &'a [u32],
    /// Each component's current fit.
    pub(crate) fits: &'a [FillFit],
    /// Per-channel noise, sRGB units (floored at half an LSB where it prices anything).
    pub(crate) sigma: f64,
    /// Price of one editable number.
    pub(crate) lambda: f64,
    /// Accept by MDL gain ([`group_gain`]) rather than by the residual threshold.
    pub(crate) mdl: bool,
    /// `INKVEC_MERGEDBG`: print each segment's decision.
    pub(crate) debug: bool,
}

/// The accepted groups, in segment order: each a sorted list of component ids (two or
/// more, or one whose fill is replaced) and the fill that explains them. Groups are
/// disjoint. Only groups whose fill is a gradient are returned.
///
/// Also returns the work done, in the band merger's units, for its cap: each fit as a
/// union fit of its pixels (`bands::union_work`), each MDL price as a common-pixel gain
/// (`bands::gain_work`), each threshold residual as one unit per pixel it reads. A sum over
/// segments, each counted on its own, so it does not depend on the thread schedule.
pub(crate) fn propose(inp: &Inputs) -> (Vec<(Vec<u32>, FillFit)>, u64) {
    use rayon::prelude::*;
    let by_seg: Vec<(u32, Vec<u32>)> = assign_to_segments(inp.seg, inp.members, inp.alive)
        .into_iter()
        .collect();
    let found: Vec<(Vec<(Vec<u32>, FillFit)>, u64)> = by_seg
        .into_par_iter()
        .map(|(sid, ids)| {
            let mut work = 0u64;
            let found = propose_in_segment(inp, sid, ids, &mut work);
            (found, work)
        })
        .collect();
    let work = found.iter().map(|f| f.1).fold(0u64, u64::saturating_add);
    (found.into_iter().flat_map(|f| f.0).collect(), work)
}

/// Pixels of the components `ids`.
fn pixels_of(inp: &Inputs, ids: &[u32]) -> usize {
    ids.iter().map(|&c| inp.members[c as usize].len()).sum()
}

/// [`fit_set`], with its work added to `work` (`bands::union_work` of its pixels).
fn fit_set_charged(inp: &Inputs, ids: &[u32], work: &mut u64) -> FillFit {
    *work = work.saturating_add(super::bands::union_work(pixels_of(inp, ids)));
    fit_set(inp, ids)
}

/// [`group_gain`], with its work added to `work` (`bands::gain_work` of its pixels).
fn group_gain_charged(
    inp: &Inputs,
    ids: &[u32],
    parts: &[(Vec<u32>, &FillFit)],
    union: &FillFit,
    work: &mut u64,
) -> Option<f64> {
    *work = work.saturating_add(super::bands::gain_work(pixels_of(inp, ids)));
    group_gain(inp, ids, parts, union)
}

/// Components by the smooth segment holding at least half of all their pixels (ties of
/// the largest share by the lower segment id), ascending by component within a segment.
///
/// A component split evenly between two segments, or mostly in the discontinuity map,
/// belongs to none. Dead and empty components are skipped. O(total pixels).
fn assign_to_segments(
    seg: &[u32],
    members: &[Vec<usize>],
    alive: &[bool],
) -> BTreeMap<u32, Vec<u32>> {
    let mut by_seg: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (c, px) in members.iter().enumerate() {
        if !alive[c] || px.is_empty() {
            continue;
        }
        let mut count: BTreeMap<u32, usize> = BTreeMap::new();
        for &p in px {
            if seg[p] != DISCONTINUITY {
                *count.entry(seg[p]).or_insert(0) += 1;
            }
        }
        // Largest share; on equal shares the lower segment id (BTreeMap order, and
        // `max_by` keeps the last maximum, hence the reversed comparison of ids).
        if let Some((&s, &k)) = count.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))) {
            if 2 * k >= px.len() {
                by_seg.entry(s).or_default().push(c as u32);
            }
        }
    }
    by_seg
}

/// The accepted groups of one segment `sid` with members `ids` (ascending): steps 2 and 3
/// of the module docs, and the one-member refit.
fn propose_in_segment(
    inp: &Inputs,
    sid: u32,
    ids: Vec<u32>,
    work: &mut u64,
) -> Vec<(Vec<u32>, FillFit)> {
    let mut found = Vec::new();
    let whole = fit_set_charged(inp, &ids, work);
    let accepted = if ids.len() >= 2 && !connected(inp.adj, &ids) {
        false // not one region in the merger's adjacency: only growth may join them
    } else {
        explains(inp, &ids, &whole, &own_parts(inp, &ids), work)
    };
    if inp.debug {
        eprintln!(
            "gregions segment {sid}: {} members, whole {} -> {}",
            ids.len(),
            whole.model.kind(),
            if accepted { "accepted" } else { "refused" }
        );
    }
    if accepted || ids.len() == 1 {
        if accepted && whole.model.is_gradient() {
            found.push((ids, whole));
        }
        return found;
    }
    grow_in_segment(inp, sid, &ids, &mut found, work);
    found
}

/// Whether the components `ids` form one connected set in the merger's adjacency `adj`
/// (any seam length counts). A single component is connected. O(k²) on `k` members.
fn connected(adj: &[HashMap<u32, u32>], ids: &[u32]) -> bool {
    let mut reached = vec![false; ids.len()];
    reached[0] = true;
    let mut stack = vec![0usize];
    while let Some(i) = stack.pop() {
        for (j, &c) in ids.iter().enumerate() {
            if !reached[j] && adj[ids[i] as usize].contains_key(&c) {
                reached[j] = true;
                stack.push(j);
            }
        }
    }
    reached.iter().all(|&r| r)
}

/// Each component of `ids` described by its own current fit: the "split" alternative
/// a whole-segment proposal is priced against.
fn own_parts<'a>(inp: &'a Inputs, ids: &[u32]) -> Vec<(Vec<u32>, &'a FillFit)> {
    ids.iter()
        .map(|&c| (vec![c], &inp.fits[c as usize]))
        .collect()
}

/// Whether `union`, the fit of the components `ids`, explains them under the acceptance in
/// force: the residual threshold, or a positive [`group_gain`] against `parts` (the
/// components of `ids` grouped by the fill each part currently has). Adds its work to
/// `work`.
fn explains(
    inp: &Inputs,
    ids: &[u32],
    union: &FillFit,
    parts: &[(Vec<u32>, &FillFit)],
    work: &mut u64,
) -> bool {
    if inp.mdl {
        group_gain_charged(inp, ids, parts, union, work).is_some_and(|g| g > 0.0)
    } else {
        *work = work.saturating_add(pixels_of(inp, ids).min(FIT_PIXELS_CAP) as u64);
        mean_residual(inp, ids, union) <= threshold_tol(inp.sigma)
    }
}

/// The threshold acceptance's tolerance: `max(GROW_TOL_LEVELS/255, 3σ)`, sRGB units.
fn threshold_tol(sigma: f64) -> f64 {
    (GROW_TOL_LEVELS / 255.0).max(3.0 * sigma)
}

/// Ardeco-style growth inside one segment whose members one fill does not explain
/// together (step 3 of the module docs). Appends each grown group of two or more whose
/// fill is a gradient (and, under MDL, that still pays against its members' own fills)
/// to `found`.
///
/// From the largest member left (ties by id), the frontier -- members left that touch the
/// group in the merger's adjacency -- is fitted member by member with the group; the
/// member whose union is best (lowest residual, or highest gain under MDL; ties by id)
/// joins when it is accepted, and the group grows until none is. The group's members
/// leave the pool, accepted or not, and the next group starts. Stops early, keeping what it
/// found, after [`MAX_GROW_FITS`] fits. Adds its work to `work`.
fn grow_in_segment(
    inp: &Inputs,
    sid: u32,
    ids: &[u32],
    found: &mut Vec<(Vec<u32>, FillFit)>,
    work: &mut u64,
) {
    let mut left: Vec<u32> = ids.to_vec();
    let mut fits_made = 0usize;
    while left.len() >= 2 && fits_made < MAX_GROW_FITS {
        left.sort_by_key(|&c| (std::cmp::Reverse(inp.members[c as usize].len()), c));
        let seed = left[0];
        let mut cur = vec![seed];
        let mut cur_fit: FillFit = inp.fits[seed as usize].clone();
        loop {
            let frontier: Vec<u32> = left
                .iter()
                .copied()
                .filter(|c| {
                    !cur.contains(c) && cur.iter().any(|m| inp.adj[*m as usize].contains_key(c))
                })
                .collect();
            if frontier.is_empty() || fits_made + frontier.len() > MAX_GROW_FITS {
                break;
            }
            fits_made += frontier.len();
            // (score, member, union fit): higher score is better.
            let mut best: Option<(f64, u32, FillFit)> = None;
            for &c in &frontier {
                let mut set = cur.clone();
                set.push(c);
                set.sort_unstable();
                let f = fit_set_charged(inp, &set, work);
                let score = if inp.mdl {
                    let parts = [(cur.clone(), &cur_fit), (vec![c], &inp.fits[c as usize])];
                    match group_gain_charged(inp, &set, &parts, &f, work) {
                        Some(g) if g > 0.0 => g,
                        _ => continue,
                    }
                } else {
                    *work = work.saturating_add(pixels_of(inp, &set).min(FIT_PIXELS_CAP) as u64);
                    let r = mean_residual(inp, &set, &f);
                    if r > threshold_tol(inp.sigma) {
                        continue;
                    }
                    -r
                };
                if best.as_ref().is_none_or(|(s, _, _)| score > *s) {
                    best = Some((score, c, f));
                }
            }
            match best {
                Some((_, c, f)) => {
                    cur.push(c);
                    cur.sort_unstable();
                    cur_fit = f;
                }
                None => break,
            }
        }
        left.retain(|c| !cur.contains(c));
        if cur.len() < 2 || !cur_fit.model.is_gradient() {
            continue;
        }
        // Every step paid against the group as it then was; the group as a whole must
        // still pay against the fills its members had (MDL only: the threshold has no
        // notion of the alternative).
        if inp.mdl
            && !group_gain_charged(inp, &cur, &own_parts(inp, &cur), &cur_fit, work)
                .is_some_and(|g| g > 0.0)
        {
            continue;
        }
        if inp.debug {
            let members: Vec<String> = cur
                .iter()
                .map(|&c| {
                    let f = &inp.fits[c as usize];
                    format!(
                        "{c}:{}px {} cost {:.0}",
                        inp.members[c as usize].len(),
                        f.model.kind(),
                        f.cost
                    )
                })
                .collect();
            eprintln!(
                "gregions segment {sid}: grew {cur:?} -> {} (gain {:?}) from [{}]; {:?}",
                cur_fit.model.kind(),
                group_gain(inp, &cur, &own_parts(inp, &cur), &cur_fit),
                members.join(", "),
                cur_fit.model
            );
        }
        found.push((cur, cur_fit));
    }
}

/// Model selection over the pixels of the components `ids` (sorted), with the segment's
/// smooth pixels as evidence: [`fit_pixels`] with membership "in one of `ids`" and
/// evidence "not in the discontinuity map".
///
/// Like the band merger's `UnionFitter::fit`, the gather sees at most [`FIT_PIXELS_CAP`]
/// pixels, every `⌊total / cap⌋`-th of the members' pixels in member order, and the chi²
/// and cost are scaled back up by `total / seen` so the cost reads on the same scale as
/// every other fit's.
fn fit_set(inp: &Inputs, ids: &[u32]) -> FillFit {
    let total: usize = ids.iter().map(|&c| inp.members[c as usize].len()).sum();
    let stride = if total > FIT_PIXELS_CAP {
        total / FIT_PIXELS_CAP
    } else {
        1
    };
    let seen: Vec<usize> = ids
        .iter()
        .flat_map(|&c| inp.members[c as usize].iter().copied())
        .step_by(stride)
        .collect();
    let (group, seg) = (inp.group, inp.seg);
    let mut fit = select(fit_pixels(
        inp.rgb,
        inp.w,
        inp.h,
        &seen,
        |p| ids.binary_search(&group[p]).is_ok(),
        |p| seg[p] != DISCONTINUITY,
        inp.sigma,
        inp.lambda,
    ));
    if !seen.is_empty() && seen.len() < total {
        let k = total as f64 / seen.len() as f64;
        let params_term = fit.cost - 0.5 * fit.chi2;
        fit.chi2 *= k;
        fit.cost = params_term + 0.5 * fit.chi2;
    }
    fit
}

/// The threshold acceptance's residual: the mean, over the smooth pixels of the
/// components `ids`, of the largest sRGB channel difference between pixel and `fit`.
/// Every smooth pixel counts, boundary or not (acda7ac's measure), over at most
/// [`FIT_PIXELS_CAP`] of them taken by stride. Infinity when there is none.
fn mean_residual(inp: &Inputs, ids: &[u32], fit: &FillFit) -> f64 {
    let total: usize = ids.iter().map(|&c| inp.members[c as usize].len()).sum();
    let stride = if total > FIT_PIXELS_CAP {
        total / FIT_PIXELS_CAP
    } else {
        1
    };
    let e = fit.model.eval();
    let (mut sum, mut n) = (0.0f64, 0usize);
    let pixels = ids
        .iter()
        .flat_map(|&c| inp.members[c as usize].iter().copied())
        .step_by(stride);
    for p in pixels {
        if inp.seg[p] == DISCONTINUITY {
            continue;
        }
        // Pixel coordinates by division, once per pixel of a gather that is itself
        // capped; not a wrap-around index.
        let (x, y) = ((p % inp.w) as f64, (p / inp.w) as f64);
        let c = e.color_at(x, y);
        let r = inp.rgb[p];
        let d = (0..3).map(|k| (r[k] - c[k]).abs()).fold(0.0f32, f32::max);
        sum += d as f64;
        n += 1;
    }
    if n == 0 {
        f64::INFINITY
    } else {
        sum / n as f64
    }
}

/// What replacing the fills `parts` by `union` saves, in description length, on the common
/// pixels of the components `ids` (sorted; the union of the parts' components).
///
/// The pixels are the strictly interior (all four in-image neighbours in `ids`, not on the
/// picture edge) pixels of the union, smooth or in the discontinuity map, gathered by
/// `collect_samples` and strided to at most `fit_cap()` (`m` of `n`). With `e(q) = max(|o − q| − ½LSB, 0)` per sRGB channel
/// (`o` observed, `½LSB` the quantisation dead zone of every chi² in this module):
///
/// `gain = ½ · (n/m) · Σ_i Σ_ch (e(split_i)² − e(union_i)²) / σ²
///         + λ · (Σ_parts params − params_union)`
///
/// where `split_i` is the prediction of the part whose components hold pixel `i` -- or, for
/// a pixel with a 4-neighbour in another part, the best coverage blend of the two parts'
/// fills there ([`seam_blend`]), when that is closer: the emitted SVG draws such a pixel as
/// two shapes meeting inside it, and its anti-aliasing is what the boundary solve fits.
/// Priced as one part's flat colour, a seam pixel favours any union that ramps across the
/// seam: two flat tiles of a mosaic (synthetic mosaic_grid6, +0.031 dE00) were joined by a
/// narrow ramp that "explained" their anti-aliasing. Positive means the union is the
/// shorter description. Both alternatives are priced on the same
/// pixels, so a cost measured on another population (each part's fit saw its own samples)
/// cannot leak in -- the reason `common_pixel_gain` exists in `bands.rs`, of which this is
/// the k-part form. `σ` is floored at half an LSB as in `fit_samples`.
///
/// One more condition, per part: the union must not describe any part's own pixels worse
/// than the part's fill does by more than the part's parameters are worth (the part's share
/// of the gain, `½·Σ_{i∈k}(e(split_i)² − e(union_i)²)/σ² + λ·params_k`, must not be
/// negative); otherwise the gain is negative infinity. The total gain alone lets a part
/// with a poor fill buy a union that degrades a well-fitted neighbour: in synthetic
/// mosaic_grid6 a component holding two tiles under one palette ink (its flat costs 367k)
/// bought an ellipse across three tiles, +0.031 dE00. Not from the literature: Zhu & Yuille
/// accept a merge on the total energy alone; this asks the merge to be no worse for any
/// member, a stricter rule, because here a member's fill can be wrong for reasons (the
/// palette) the merge cannot repair.
///
/// `None` when the union has no such pixel, a pixel belongs to no part, or the gain is not
/// finite.
pub(crate) fn group_gain(
    inp: &Inputs,
    ids: &[u32],
    parts: &[(Vec<u32>, &FillFit)],
    union: &FillFit,
) -> Option<f64> {
    let pixels: Vec<usize> = ids
        .iter()
        .flat_map(|&c| inp.members[c as usize].iter().copied())
        .collect();
    let group = inp.group;
    // Every strictly interior pixel, smooth or not. The fit took only smooth pixels as
    // evidence (a banded ramp has no other), but the price must also see the
    // discontinuity pixels *inside* the union: they are where a union would erase an edge
    // between two of its members, which the split description keeps. The union's outer
    // rim is not interior and stays out, as in every other price here.
    let s = collect_samples(
        inp.rgb,
        inp.w,
        inp.h,
        &pixels,
        |p| ids.binary_search(&group[p]).is_ok(),
        |_| true,
        true,
    );
    if s.len() == 0 {
        return None;
    }
    // Component -> part index, for the split prediction.
    let mut part_of: Vec<(u32, usize)> = parts
        .iter()
        .enumerate()
        .flat_map(|(k, (comps, _))| comps.iter().map(move |&c| (c, k)))
        .collect();
    part_of.sort_unstable();
    let evals: Vec<_> = parts.iter().map(|(_, f)| f.model.eval()).collect();
    let union_eval = union.model.eval();
    let sigma = if inp.sigma > 0.0 {
        inp.sigma
    } else {
        0.5 / 255.0
    };
    let part_at = |q: usize| {
        part_of
            .binary_search_by_key(&group[q], |&(c, _)| c)
            .ok()
            .map(|j| part_of[j].1)
    };
    // Squared residual beyond the dead zone, summed over the channels.
    let err = |o: [f32; 3], q: [f32; 3]| -> f64 {
        (0..3)
            .map(|c| {
                let d = ((o[c] - q[c]).abs() as f64 - QUANT_HALF_STEP).max(0.0);
                d * d
            })
            .sum()
    };
    let w = inp.w;
    let stride = (s.len() / fit_cap()).max(1);
    let (mut difference, mut used) = (0.0f64, 0usize);
    // The same difference per part, for the per-part condition below.
    let mut by_part = vec![0.0f64; parts.len()];
    for i in (0..s.len()).step_by(stride) {
        let (p, o) = (s.px[i], s.srgb[i]);
        let k = part_at(p)?;
        let own = evals[k].color_at(s.x[i], s.y[i]);
        // On a seam between two parts the split description does not paint one part's
        // fill: the emitted boundary covers the pixel partly by each, so it is priced as
        // the best coverage blend of the two fills there (`seam_blend`). A pixel is
        // strictly interior, so its four neighbours exist and lie in the union.
        let mut split_err = err(o, own);
        for q in [p - 1, p + 1, p - w, p + w] {
            match part_at(q) {
                Some(k2) if k2 != k => {
                    let other = evals[k2].color_at(s.x[i], s.y[i]);
                    split_err = split_err.min(err(o, seam_blend(o, own, other)));
                }
                _ => {}
            }
        }
        let merged = union_eval.color_at(s.x[i], s.y[i]);
        let d = split_err - err(o, merged);
        difference += d;
        by_part[k] += d;
        used += 1;
    }
    let scale = 0.5 / (sigma * sigma) * s.len() as f64 / used as f64;
    // No part may be described worse by the union, on its own pixels, than its own
    // parameters are worth: `½·Σ_{i∈k} (e(split_i)² − e(union_i)²)/σ² + λ·params_k ≥ 0` for
    // every part `k`. The total gain is the sum of these minus `λ·params_union`, so a part
    // whose fill is poor (a component holding two flat colours the palette gave one ink)
    // could otherwise pay for a union that degrades a well-described neighbour -- the
    // mosaic tile next to it, the hair next to the crown's band.
    if parts
        .iter()
        .zip(&by_part)
        .any(|((_, f), &d)| scale * d + inp.lambda * f.params < 0.0)
    {
        return Some(f64::NEG_INFINITY);
    }
    let params_split: f64 = parts.iter().map(|(_, f)| f.params).sum();
    let gain = scale * difference + inp.lambda * (params_split - union.params);
    gain.is_finite().then_some(gain)
}

/// The point on the segment between the colours `a` and `b` (sRGB) nearest the observed
/// `o`: `a + t·(b − a)` with `t = clamp(((o − a)·(b − a)) / |b − a|², 0, 1)`, the linear
/// coverage blend of two fills that best explains a pixel they share (the unmixing the
/// planar map does at every boundary pixel). `a` itself when the two agree.
fn seam_blend(o: [f32; 3], a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if dd <= 0.0 {
        return a;
    }
    let t =
        (((o[0] - a[0]) * d[0] + (o[1] - a[1]) * d[1] + (o[2] - a[2]) * d[2]) / dd).clamp(0.0, 1.0);
    [a[0] + t * d[0], a[1] + t * d[1], a[2] + t * d[2]]
}

#[cfg(test)]
mod tests {
    use super::super::{flat_only, FillModel, Interp};
    use super::*;

    /// A `w`×`h` horizontal ramp quantised into `bands` flat bands: the image is the ramp
    /// itself, the components are the bands (as the palette would cut it).
    struct Banded {
        rgb: Vec<[f32; 3]>,
        members: Vec<Vec<usize>>,
        group: Vec<u32>,
        adj: Vec<HashMap<u32, u32>>,
        seg: Vec<u32>,
        w: usize,
        h: usize,
    }

    /// The ramp's colour at column `x` of a `w`-wide image, with `jump` added from column
    /// `step_at` on, rounded to 8 bits.
    fn ramp_at(x: f64, w: usize, step: Option<(usize, f32)>) -> [f32; 3] {
        let jump = step.map_or(0.0, |(s, j)| if x >= s as f64 { j } else { 0.0 });
        let v = ((0.2 + 0.3 * x as f32 / w as f32 + jump) * 255.0).round() / 255.0;
        [v, (v * 0.8 * 255.0).round() / 255.0, 0.3]
    }

    fn banded(w: usize, h: usize, bands: usize, step: Option<(usize, f32)>) -> Banded {
        let mut rgb = Vec::new();
        let mut group = Vec::new();
        for _y in 0..h {
            for x in 0..w {
                // A gentle ramp, with an optional jump at a column.
                rgb.push(ramp_at(x as f64, w, step));
                group.push((x * bands / w) as u32);
            }
        }
        let mut members = vec![Vec::new(); bands];
        for (p, &g) in group.iter().enumerate() {
            members[g as usize].push(p);
        }
        let mut adj = vec![HashMap::new(); bands];
        for b in 1..bands {
            adj[b - 1].insert(b as u32, h as u32);
            adj[b].insert((b - 1) as u32, h as u32);
        }
        // One segment for the whole picture: these tests are about acceptance, and a weak
        // step is smooth by construction (the leak `segments` documents).
        let seg = vec![0u32; w * h];
        Banded {
            rgb,
            members,
            group,
            adj,
            seg,
            w,
            h,
        }
    }

    fn inputs<'a>(b: &'a Banded, fits: &'a [FillFit], alive: &'a [bool], mdl: bool) -> Inputs<'a> {
        Inputs {
            rgb: &b.rgb,
            w: b.w,
            h: b.h,
            seg: &b.seg,
            members: &b.members,
            alive,
            adj: &b.adj,
            group: &b.group,
            fits,
            sigma: 1.0 / 255.0,
            lambda: 0.5 * ((b.w * b.h) as f64).ln(),
            mdl,
            debug: false,
        }
    }

    /// Each band's own fit: flat at its median colour, priced like a perfect flat.
    fn band_fits(b: &Banded, lambda: f64) -> Vec<FillFit> {
        b.members
            .iter()
            .map(|px| flat_only(b.rgb[px[px.len() / 2]], lambda))
            .collect()
    }

    #[test]
    fn a_banded_ramp_is_proposed_as_one_gradient_under_both_acceptances() {
        let b = banded(48, 24, 6, None);
        let lambda = 0.5 * ((b.w * b.h) as f64).ln();
        let fits = band_fits(&b, lambda);
        let alive = vec![true; fits.len()];
        for mdl in [false, true] {
            let (found, work) = propose(&inputs(&b, &fits, &alive, mdl));
            assert!(work > 0, "the fits are charged");
            assert_eq!(found.len(), 1, "mdl {mdl}: one group");
            assert_eq!(found[0].0, (0..6).collect::<Vec<u32>>());
            assert!(found[0].1.model.is_gradient());
        }
    }

    #[test]
    fn mdl_refuses_a_union_across_a_weak_step_that_the_threshold_accepts() {
        // Two ramps meeting at column 24 with a weak step of four levels between them:
        // one ramp across both explains the pixels to within the threshold's two levels
        // on average, but much worse than the two ramps do, and the parameters it saves do
        // not pay for that.
        let (w, step) = (48, Some((24, 4.0 / 255.0)));
        let b = banded(w, 24, 2, step);
        let lambda = 0.5 * ((b.w * b.h) as f64).ln();
        let ramp = |x0: f64, x1: f64| -> FillFit {
            let model = FillModel::Linear {
                p0: (x0, 0.0),
                p1: (x1, 0.0),
                c0: ramp_at(x0, w, step),
                c1: ramp_at(x1, w, step),
                interp: Interp::Srgb,
                mids: vec![],
            };
            FillFit {
                params: model.params(),
                cost: lambda * model.params(),
                chi2: 0.0,
                model,
            }
        };
        let fits = vec![ramp(0.0, 23.0), ramp(24.0, 47.0)];
        let alive = vec![true; 2];
        let union = ramp(0.0, 47.0);
        let ids = [0u32, 1];
        for mdl in [false, true] {
            let inp = inputs(&b, &fits, &alive, mdl);
            let accepted = explains(&inp, &ids, &union, &own_parts(&inp, &ids), &mut 0);
            assert_eq!(accepted, !mdl, "mdl {mdl}");
        }
    }

    #[test]
    fn group_gain_prices_both_sides_on_the_same_pixels() {
        let b = banded(32, 16, 4, None);
        let lambda = 0.5 * ((b.w * b.h) as f64).ln();
        let fits = band_fits(&b, lambda);
        let alive = vec![true; fits.len()];
        let inp = inputs(&b, &fits, &alive, true);
        let ids: Vec<u32> = (0..4).collect();
        // Replacing the parts by themselves (one part per band, the union being band 0's
        // fill) can only gain the parameters it drops when the colours agree; here they
        // do not, so the gain is the split's advantage, negative.
        let parts = own_parts(&inp, &ids);
        let worse = group_gain(&inp, &ids, &parts, &fits[0]).expect("interior pixels");
        assert!(worse < 0.0);
        // No component of the union in any part: nothing to price.
        assert!(group_gain(&inp, &ids, &[], &fits[0]).is_none());
    }

    #[test]
    fn a_seam_pixel_is_priced_as_a_blend_of_its_two_fills() {
        let (a, b) = ([0.2f32, 0.4, 0.6], [0.8f32, 0.4, 0.0]);
        // Half way between: the blend reproduces it.
        let mid = [0.5, 0.4, 0.3];
        let q = seam_blend(mid, a, b);
        assert!((0..3).all(|k| (q[k] - mid[k]).abs() < 1e-6));
        // Beyond either end: clamped to that fill.
        assert_eq!(seam_blend([0.0, 0.4, 0.9], a, b), a);
        assert_eq!(seam_blend([1.0, 0.4, -0.3], a, b), b);
        // Equal fills: that fill.
        assert_eq!(seam_blend(mid, a, a), a);
    }

    #[test]
    fn mdl_refuses_a_ramp_across_the_anti_aliased_seam_of_two_flats() {
        // Two flat tiles meeting at column 24 with one anti-aliased column between them:
        // a 2 px ramp across the seam reproduces the blend column exactly, but the split
        // description reproduces it too (as a coverage blend), so the ramp buys nothing
        // for its parameters.
        let (w, h) = (48, 24);
        let (ca, cb) = ([0.8f32, 0.3, 0.3], [0.8f32, 0.5, 0.3]);
        let mut b = banded(w, h, 2, None);
        for y in 0..h {
            for x in 0..w {
                b.rgb[y * w + x] = match x.cmp(&24) {
                    std::cmp::Ordering::Less => ca,
                    std::cmp::Ordering::Equal => [0.8, 0.4, 0.3],
                    std::cmp::Ordering::Greater => cb,
                };
            }
        }
        let lambda = 0.5 * ((w * h) as f64).ln();
        let fits = vec![flat_only(ca, lambda), flat_only(cb, lambda)];
        let alive = vec![true; 2];
        let ramp = FillModel::Linear {
            p0: (23.0, 0.0),
            p1: (25.0, 0.0),
            c0: ca,
            c1: cb,
            interp: Interp::Srgb,
            mids: vec![],
        };
        let union = FillFit {
            params: ramp.params(),
            cost: lambda * ramp.params(),
            chi2: 0.0,
            model: ramp,
        };
        let inp = inputs(&b, &fits, &alive, true);
        let ids = [0u32, 1];
        let gain = group_gain(&inp, &ids, &own_parts(&inp, &ids), &union).expect("pixels");
        assert!(gain < 0.0, "gain {gain}");
    }

    #[test]
    fn connectivity_and_assignment() {
        let mut adj = vec![HashMap::new(); 3];
        adj[0].insert(1, 3);
        adj[1].insert(0, 3);
        assert!(connected(&adj, &[0, 1]));
        assert!(!connected(&adj, &[0, 1, 2]));
        assert!(connected(&adj, &[2]));
        // Component 0 mostly in segment 5; component 1 mostly discontinuity; 2 dead.
        let seg = [5, 5, DISCONTINUITY, DISCONTINUITY, DISCONTINUITY, 7];
        let members = vec![vec![0, 1, 2], vec![3, 4, 5], vec![0]];
        let by = assign_to_segments(&seg, &members, &[true, true, false]);
        assert_eq!(by.into_iter().collect::<Vec<_>>(), vec![(5, vec![0])]);
    }
}
