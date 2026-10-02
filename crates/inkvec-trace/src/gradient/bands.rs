//! Band merging: merge adjacent palette regions described more cheaply by a gradient.
//!
//! The `merge_bands` stage of the Quality pipeline. `lib.rs` calls
//! [`merge_gradient_bands_with_ink`] after blend absorption; the native-alpha path in
//! `native.rs` calls [`merge_gradient_bands_guarded`]. Input: the label map from
//! [`crate::color::label_image`] (one palette entry per pixel), the sRGB image (0..1),
//! the palette, the noise estimate and the MDL price `λ`. Output: a relabelled map in
//! which each gradient region and each flat region with an interior of its own has a
//! fresh label, one [`FillFit`] per label, and each label's palette entry.
//!
//! The algorithm is greedy agglomerative clustering of connected components under an
//! MDL cost; see [`merge_gradient_bands`] for the steps and [`merge_bands_with`] for how
//! they map onto the code.

use super::*;
use crate::color::Palette;

// ---------------------------------------------------------------------------------------
// Band merging
// ---------------------------------------------------------------------------------------

/// Two regions must touch along at least this many pixel pairs to be merge candidates.
const MIN_SHARED_BOUNDARY: u32 = 3;

/// Research comparison on common observations. Both alternatives explain the same
/// union-interior pixels, including the former interface when evidence permits it.
/// The split prediction uses the current discrete membership at that pixel; this is
/// not an antialiased render, so promotion also requires rendered-image validation.
///
/// Over a strided subsample of `m` of the `n` interior evidence pixels of `a ∪ b`
/// (membership by `group`), with `e(q) = max(|o − q| − ½LSB, 0)` per sRGB channel:
///
/// `gain = ½ · (n/m) · Σ_i Σ_ch (e(split_i)² − e(merged_i)²) / σ² + λ·(params_a +
/// params_b − params_union)`
///
/// where `split_i` is the prediction of the side (`left` for `a`, `right` for `b`) the
/// pixel currently belongs to and `merged_i` the union's. Positive means the union is
/// cheaper. Unlike the cached costs, which were measured on each fit's own samples, both
/// terms here are on the same pixels, so a cost measured on another population cannot
/// leak in. `None` when the union has no evidence pixels or the gain is not finite.
#[allow(clippy::too_many_arguments)]
fn common_pixel_gain(
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pixels: &[usize],
    group: &[u32],
    evidence: &(dyn Fn(usize) -> bool + Sync),
    a: u32,
    b: u32,
    left: &FillFit,
    right: &FillFit,
    union: &FillFit,
    sigma: f64,
    lambda: f64,
) -> Option<f64> {
    let samples = collect_samples(
        rgb,
        w,
        h,
        pixels,
        |p| group[p] == a || group[p] == b,
        evidence,
        true,
    );
    if samples.len() == 0 {
        return None;
    }
    let stride = (samples.len() / fit_cap()).max(1);
    let mut difference = 0.0;
    let mut used = 0;
    let (left_eval, right_eval, union_eval) =
        (left.model.eval(), right.model.eval(), union.model.eval());
    for i in (0..samples.len()).step_by(stride) {
        let model = if group[samples.px[i]] == a {
            &left_eval
        } else {
            &right_eval
        };
        let split = model.color_at(samples.x[i], samples.y[i]);
        let merged = union_eval.color_at(samples.x[i], samples.y[i]);
        for c in 0..3 {
            let old = ((samples.srgb[i][c] - split[c]).abs() as f64 - QUANT_HALF_STEP).max(0.0);
            let new = ((samples.srgb[i][c] - merged[c]).abs() as f64 - QUANT_HALF_STEP).max(0.0);
            difference += old * old - new * new;
        }
        used += 1;
    }
    let gain = 0.5 * difference / (sigma * sigma) * samples.len() as f64 / used as f64
        + lambda * (left.params + right.params - union.params);
    gain.is_finite().then_some(gain)
}

/// Merge adjacent regions that one gradient describes more cheaply than two flat fills.
///
/// `label_image` assigns each pixel its nearest palette entry, so a region whose true
/// fill is a gradient comes out as a stack of bands, one per palette entry the ramp
/// crosses. This undoes that. The algorithm:
///
/// 1. Split the labelling into 4-connected components and record, for every pair of
///    components that touch, the length of their shared boundary. Components rather
///    than labels, because a band's palette colour may also be the colour of an
///    unrelated flat region elsewhere in the image, which must not be dragged in.
/// 2. Fit every component on its own ([`fit_fill`] semantics) to get its cost.
/// 3. Greedily: for every adjacent pair, fit their union (interior pixels of the union —
///    which now includes the pixels along the former band boundary) and take the pair
///    with the largest saving `cost(a) + cost(b) - cost(a ∪ b)`, provided the saving is
///    positive and the union is a gradient. Merge it, carry the union's fit forward,
///    re-route the merged component's adjacencies, and repeat until no pair saves.
///    Union fits are cached and only those touching the merged pair are recomputed, so
///    with `B` bands at most `O(B²)` fits are performed, each linear in the pixels it
///    covers.
/// 4. Write the result back. A thin flat component keeps its palette label; every
///    component whose fill is a gradient — merged or not — and every flat component
///    with an interior of its own gets a fresh label `pal.len() + i`, because a fill is
///    a property of one connected region and a palette label may cover several.
///
/// Returns one [`FillFit`] per label id in the updated `labels` — palette labels first,
/// always flat (the palette colour where the label no longer has pixels), then the
/// gradients — so `fills[labels[p]]` is the fill at pixel `p`. Only gradient unions
/// merge: two flat regions the palette should have merged are left to the palette.
pub fn merge_gradient_bands(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma_noise: f64,
    lambda: f64,
) -> Vec<FillFit> {
    merge_gradient_bands_with_ink(labels, rgb, w, h, pal, sigma_noise, lambda, None).0
}

/// [`merge_gradient_bands`] that also returns, for every label id in the updated
/// `labels`, the palette entry it came from — a palette label maps to itself, a fresh
/// label to the entry of the region it was cut from (a merged gradient's first band).
/// Every face keeps a palette index that way, whatever its fill.
/// Takes the image, its palette and its pricing; each is a separate input to a
/// separate test, and grouping them would name the group, not reduce it.
#[allow(clippy::too_many_arguments)]
pub fn merge_gradient_bands_with_ink(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma_noise: f64,
    lambda: f64,
    deadline: Option<inkvec_core::clock::Instant>,
) -> (Vec<FillFit>, Vec<usize>) {
    merge_gradient_bands_guarded(labels, rgb, w, h, pal, sigma_noise, lambda, deadline, None)
}

/// [`merge_gradient_bands_with_ink`], with a veto on which palette entries may ever share a
/// fill: two regions whose labels `same_class` rejects are never adjacent for merging. With
/// `None` the two functions are the same function.
#[allow(clippy::too_many_arguments)]
pub fn merge_gradient_bands_guarded(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma_noise: f64,
    lambda: f64,
    deadline: Option<inkvec_core::clock::Instant>,
    same_class: Option<&(dyn Fn(u16, u16) -> bool + Sync)>,
) -> (Vec<FillFit>, Vec<usize>) {
    merge_bands_with(
        labels,
        rgb,
        w,
        h,
        pal,
        sigma_noise,
        lambda,
        deadline,
        same_class,
        regions::enabled(),
    )
}

/// [`merge_gradient_bands_guarded`] with region recovery (see [`super::regions`]) switched
/// by the caller rather than by `INKVEC_GRAD_REGIONS`.
///
/// `inner_blends` turns region recovery on: seams judged smooth ([`regions::is_smooth`])
/// let blends across them count as evidence for the union, and flat pairs one ramp step
/// apart are tried as unions too. `deadline` stops the agglomeration early (every merge
/// accepted so far stands); `same_class` vetoes pairs of palette labels.
///
/// The four steps of [`merge_gradient_bands`], as named stages:
///
/// 1. [`label_components`] and [`component_adjacency`]: the components and their seams;
/// 2. the evidence mask and one fit per component, both held by a [`UnionFitter`];
/// 3. [`Agglomeration::run`]: the greedy merge, one accepted union per round;
/// 4. [`Agglomeration::write_back`]: fresh labels and one fill per label.
#[allow(clippy::too_many_arguments)]
pub(crate) fn merge_bands_with(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma_noise: f64,
    lambda: f64,
    deadline: Option<inkvec_core::clock::Instant>,
    same_class: Option<&(dyn Fn(u16, u16) -> bool + Sync)>,
    inner_blends: bool,
) -> (Vec<FillFit>, Vec<usize>) {
    let n_pal = pal
        .len()
        .max(labels.iter().map(|&l| l as usize + 1).max().unwrap_or(0));

    // 1. Connected components, and adjacency with shared-boundary lengths.
    let (comp, members, comp_label) = label_components(labels, w, h);
    let n_comp = members.len();
    let (adj, smooth) =
        component_adjacency(&comp, &comp_label, rgb, w, h, same_class, inner_blends);
    // Research prototype A10, part `seam`: how much of each seam is an edge. Empty (and
    // never read) when the part is off.
    let sharp = if gregions::parts().seam {
        gregions::sharp_seams(&comp, rgb, w, h, n_comp)
    } else {
        Vec::new()
    };

    // Which pixels may testify about a fill at all (see `fill_evidence`). Computed on
    // the palette inks of the labels as they stand before any band is merged.
    let ink_rgb: Vec<[f32; 3]> = (0..n_pal)
        .map(|l| pal.rgb.get(l).copied().unwrap_or([0.0; 3]))
        .collect();
    // A blend towards a region inside the fit is evidence for the fit; see
    // `blend_partners`.
    let partner = blend_partners(rgb, w, h, labels, &ink_rgb, sigma_noise, inner_blends);
    let pure: Vec<bool> = partner.iter().map(|q| q[0] == PURE).collect();
    let fitter = UnionFitter {
        rgb,
        w,
        h,
        sigma_noise,
        lambda,
        pure,
        partner,
    };

    // 2. Per-component fits. `group[p]` is the current component of pixel `p`.
    let group = comp;
    // Hundreds of components each pay a full model selection (the radial-centre search
    // alone is six hundred residual evaluations), and none depends on another: fit them
    // on every core.
    let live = inkvec_core::progress::handle();
    inkvec_core::progress::step("regions fitted", 0, n_comp as u64);
    let fits: Vec<FillFit> = {
        use rayon::prelude::*;
        (0..n_comp)
            .into_par_iter()
            .map(|c| {
                let fit = fitter.fit(&group, [&members[c], &[]], c as u32, c as u32, inner_blends);
                live.tick();
                fit
            })
            .collect()
    };

    // 3. Greedy agglomeration.
    let mut merge = Agglomeration {
        fitter,
        members,
        comp_label,
        ink_rgb,
        group,
        adj,
        smooth,
        sharp,
        fits,
        alive: vec![true; n_comp],
        cache: HashMap::new(),
        gains: HashMap::new(),
        inner_blends,
        mergedbg: inkvec_core::env::flag("INKVEC_MERGEDBG"),
        // Explicit experiment (research builds only); the measured production objective
        // remains the default.
        common_pixels: cfg!(feature = "research")
            && inkvec_core::env::flag("INKVEC_MERGE_COMMON_PIXELS"),
    };
    // Research prototype A10, part `segments` (`INKVEC_GREGIONS`, a `research` build
    // only): groups of components that one gradient explains inside a smooth segment are
    // merged before the pairwise rounds. Off, this is never entered. The deadline is only
    // read when there is one (no clock without a time budget).
    let gr = gregions::parts();
    if gr.segments && deadline.is_none_or(|d| inkvec_core::clock::Instant::now() < d) {
        merge.propose_regions(gr.mdl);
    }
    merge.run(deadline, &live);

    if let Some(win) = debug::window() {
        merge.dump(win);
    }

    // 4. Write back.
    merge.write_back(labels, pal, n_pal)
}

/// Diagnostic only: how many union fits the agglomeration asks for, and how many pixels
/// they walk in total. `fit_pixels` caps the *fitting* at MAX_FIT_SAMPLES, but
/// `collect_samples` still filters every pixel of the union first, so the two numbers say
/// whether the cost is fit count or fit size. Process-wide; printed under `INKVEC_TIMING`.
static FIT_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Pixels walked by all union fits; see [`FIT_CALLS`].
static FIT_PIXELS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Step 1a: the 4-connected components of the label map.
///
/// Returns, per pixel, its component id (`comp`); per component, its pixels in flood
/// order (`members`) and its label (`comp_label`). Components are numbered in raster
/// order of their first pixel, so the numbering is deterministic.
fn label_components(labels: &[u16], w: usize, h: usize) -> (Vec<u32>, Vec<Vec<usize>>, Vec<u16>) {
    let n = w * h;
    let mut comp = vec![u32::MAX; n];
    let mut members: Vec<Vec<usize>> = Vec::new();
    let mut comp_label: Vec<u16> = Vec::new();
    for start in 0..n {
        if comp[start] != u32::MAX {
            continue;
        }
        let id = members.len() as u32;
        let lab = labels[start];
        let mut stack = vec![start];
        let mut group = Vec::new();
        comp[start] = id;
        while let Some(p) = stack.pop() {
            group.push(p);
            let (x, y) = (p % w, p / w);
            let mut visit = |q: usize| {
                if comp[q] == u32::MAX && labels[q] == lab {
                    comp[q] = id;
                    stack.push(q);
                }
            };
            if x > 0 {
                visit(p - 1);
            }
            if x + 1 < w {
                visit(p + 1);
            }
            if y > 0 {
                visit(p - w);
            }
            if y + 1 < h {
                visit(p + w);
            }
        }
        members.push(group);
        comp_label.push(lab);
    }
    (comp, members, comp_label)
}

/// Per component, a count per neighbouring component: seam lengths in pixel pairs.
type SeamCounts = Vec<HashMap<u32, u32>>;

/// Step 1b: which components touch, and along how many 4-neighbour pixel pairs.
///
/// `adj[a][b]` counts the pixel pairs across the seam between components `a` and `b`
/// (symmetric). Pairs whose labels `same_class` rejects are left out, so those
/// components are never adjacent for merging. With `inner_blends`, `smooth[a][b]`
/// counts the pairs across which the colour steps less than a ramp step (see
/// [`regions::smooth_step`]); without it `smooth` stays empty.
fn component_adjacency(
    comp: &[u32],
    comp_label: &[u16],
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    same_class: Option<&(dyn Fn(u16, u16) -> bool + Sync)>,
    inner_blends: bool,
) -> (SeamCounts, SeamCounts) {
    let n = w * h;
    let n_comp = comp_label.len();
    let mut adj: Vec<HashMap<u32, u32>> = vec![HashMap::new(); n_comp];
    // Of those, the pixel pairs across which the colour changes by less than a ramp
    // step: see `regions::smooth_step`. Only kept when region recovery is on.
    let mut smooth: Vec<HashMap<u32, u32>> = vec![HashMap::new(); n_comp];
    for p in 0..n {
        let (x, y) = (p % w, p / w);
        for q in [
            if x + 1 < w { Some(p + 1) } else { None },
            if y + 1 < h { Some(p + w) } else { None },
        ]
        .into_iter()
        .flatten()
        {
            let (a, b) = (comp[p], comp[q]);
            if a != b
                && same_class.is_none_or(|f| f(comp_label[a as usize], comp_label[b as usize]))
            {
                *adj[a as usize].entry(b).or_insert(0) += 1;
                *adj[b as usize].entry(a).or_insert(0) += 1;
                if inner_blends && regions::smooth_step(rgb[p], rgb[q]) {
                    *smooth[a as usize].entry(b).or_insert(0) += 1;
                    *smooth[b as usize].entry(a).or_insert(0) += 1;
                }
            }
        }
    }
    (adj, smooth)
}

/// Step 2: the fixed inputs of every component and union fit — the image, the noise and
/// parameter price, and the evidence mask computed once before any merge.
struct UnionFitter<'a> {
    /// The composited sRGB image, 0..1.
    rgb: &'a [[f32; 3]],
    /// Image width, px.
    w: usize,
    /// Image height, px.
    h: usize,
    /// Per-channel noise, sRGB units.
    sigma_noise: f64,
    /// Price of one editable number.
    lambda: f64,
    /// Per pixel: evidence for its own fill (not a blend towards any neighbour's ink).
    pure: Vec<bool>,
    /// Per pixel: the pixels whose inks it is a blend towards ([`blend_partners`]).
    partner: Vec<[u32; PARTNERS]>,
}

impl UnionFitter<'_> {
    /// Every blend partner of `p` lies inside the fit of `a` and `b`.
    fn inner_of(&self, group: &[u32], p: usize, a: u32, b: u32) -> bool {
        regions::all_inside(&self.partner[p], |q| group[q] == a || group[q] == b)
    }

    /// Model selection over the pixels of components `a` and `b` (pass `a == b` for one
    /// component), where `group` maps each pixel to its current component and `parts`
    /// holds the pixels, `a`'s then `b`'s (the second part empty for one component). With
    /// `inner`, a blend whose partners all lie in the union counts as evidence too.
    fn fit(&self, group: &[u32], parts: [&[usize]; 2], a: u32, b: u32, inner: bool) -> FillFit {
        FIT_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let member = |p: usize| group[p] == a || group[p] == b;
        // A fit sees at most `FIT_PIXELS` pixels, taken uniformly, whatever the region's
        // size: the residual is already evaluated on a strided subsample, the medians
        // and least squares converge long before 65k samples, and the gather was the
        // one cost that still grew with the raster. The chi-square is scaled back up
        // by the subsampling factor so costs stay comparable across regions and with
        // the parameter terms. Measured identical in output on the two profiling logos
        // at 1024 and 2048 px against fitting every pixel.
        let total = parts[0].len() + parts[1].len();
        let seen = strided_union(parts);
        let seen = &seen[..];
        FIT_PIXELS.fetch_add(seen.len(), std::sync::atomic::Ordering::Relaxed);
        let evidence = |p: usize| self.pure[p] || (inner && self.inner_of(group, p, a, b));
        let mut fit = select(fit_pixels(
            self.rgb,
            self.w,
            self.h,
            seen,
            member,
            evidence,
            self.sigma_noise,
            self.lambda,
        ));
        if seen.len() < total {
            let k = total as f64 / seen.len() as f64;
            let params_term = fit.cost - 0.5 * fit.chi2;
            fit.chi2 *= k;
            fit.cost = params_term + 0.5 * fit.chi2;
        }
        fit
    }

    /// `model` priced on exactly the samples [`Self::fit`] would gather for component `a`
    /// alone (pixels `pixels`, evidence: pure, or with `inner` a blend whose partners all
    /// lie in `a`), with the same three tiers (strict evidence, any evidence, any pixel),
    /// the same subsampling and the same `total/seen` scaling: `cost = 0.5·chi² +
    /// λ·params`.
    ///
    /// For a fill fitted on *other* evidence -- a research proposal fitted on a smooth
    /// segment's pixels, prototype A10 -- so that its cost reads on the population every
    /// other component's cost was measured on, and the pairwise gains of the later rounds
    /// (`cost(a) + cost(b) − cost(a ∪ b)`) compare like with like. Not from the
    /// literature: bookkeeping for the MDL comparison.
    fn rescore(
        &self,
        group: &[u32],
        pixels: &[usize],
        a: u32,
        inner: bool,
        model: FillModel,
    ) -> FillFit {
        let sigma = if self.sigma_noise > 0.0 {
            self.sigma_noise
        } else {
            0.5 / 255.0
        };
        let total = pixels.len();
        let seen = strided_union([pixels, &[]]);
        let member = |p: usize| group[p] == a;
        let evidence = |p: usize| self.pure[p] || (inner && self.inner_of(group, p, a, a));
        let (rgb, w, h) = (self.rgb, self.w, self.h);
        let mut s = collect_samples(rgb, w, h, &seen, member, evidence, true);
        if s.len() == 0 {
            s = collect_samples(rgb, w, h, &seen, member, evidence, false);
        }
        if s.len() == 0 {
            s = collect_samples(rgb, w, h, &seen, member, |_| true, false);
        }
        let mut chi2 = Predictions::new(&model, &s).chi2(&s, sigma);
        if !seen.is_empty() && seen.len() < total {
            chi2 *= total as f64 / seen.len() as f64;
        }
        let params = model.params();
        FillFit {
            model,
            chi2,
            params,
            cost: 0.5 * chi2 + self.lambda * params,
        }
    }
}

/// The pixels a fit gathers from the concatenation `parts[0] ++ parts[1]`: all of them,
/// or, above [`FIT_PIXELS_CAP`], every `⌊len / FIT_PIXELS_CAP⌋`-th, starting at the first.
///
/// The `k`-th element of the concatenation is `parts[0][k]` below `parts[0].len()` and
/// `parts[1][k − parts[0].len()]` from there on, so the stride is taken by index
/// arithmetic without building the concatenation: the same pixels in the same order as
/// `concat.step_by(stride)`. On a poster whose background is one 1.26-million-pixel
/// component, every union with the background copied those 1.26 million indices to keep
/// 65 thousand of them. A single part at or under the cap is borrowed, not copied.
/// Not from the literature: an allocation removed.
fn strided_union<'p>(parts: [&'p [usize]; 2]) -> std::borrow::Cow<'p, [usize]> {
    use std::borrow::Cow;
    let (la, total) = (parts[0].len(), parts[0].len() + parts[1].len());
    if total <= FIT_PIXELS_CAP {
        return if parts[1].is_empty() {
            Cow::Borrowed(parts[0])
        } else if parts[0].is_empty() {
            Cow::Borrowed(parts[1])
        } else {
            Cow::Owned([parts[0], parts[1]].concat())
        };
    }
    let at = |k: usize| {
        if k < la {
            parts[0][k]
        } else {
            parts[1][k - la]
        }
    };
    Cow::Owned((0..total).step_by(total / FIT_PIXELS_CAP).map(at).collect())
}

/// Where the rounds of [`Agglomeration::run`] go, for the timing log: a greedy
/// agglomeration accepts one merge per round, so the round count is the merge count and
/// the wall time is the sum of the rounds. Which part of a round costs what is the
/// question the log answers. Only filled in under `INKVEC_TIMING`.
#[derive(Default)]
struct MergeTiming {
    /// Rounds started, including the last one that found nothing.
    rounds: u64,
    /// Nanoseconds spent scanning for unfitted candidate pairs.
    ns_scan: u64,
    /// Nanoseconds spent fitting waves of unions.
    ns_wave: u64,
    /// Nanoseconds spent picking the best merge, stale refits included.
    ns_stale: u64,
    /// Nanoseconds spent on the bookkeeping of an accepted merge.
    ns_book: u64,
    /// Rounds that had at least one union to fit.
    waves: u64,
    /// (fits in the wave, wall ms) per wave, to see whether the waves are wide enough to
    /// fill the cores or are one big fit with a few small ones behind it.
    wave_log: Vec<(usize, f64)>,
    /// Stale unions refitted one at a time because they would have won.
    stale_refits: u64,
}

impl MergeTiming {
    /// Print the three `[t] merge` lines to stderr.
    fn report(&self, n_comp: usize) {
        let MergeTiming {
            rounds,
            ns_scan,
            ns_wave,
            ns_stale,
            ns_book,
            waves,
            ref wave_log,
            stale_refits,
        } = *self;
        eprintln!("  [t] merge waves: {}", {
            let mut v = wave_log.clone();
            v.sort_by(|x, y| y.1.total_cmp(&x.1));
            let widest = v.iter().map(|w| w.0).max().unwrap_or(0);
            let sum = |lo: usize, hi: usize| -> (usize, f64) {
                let it = v.iter().filter(|w| w.0 >= lo && w.0 <= hi);
                (it.clone().count(), it.map(|w| w.1).sum())
            };
            let (n1, ms1) = sum(1, 1);
            let (n8, ms8) = sum(2, 8);
            let (nm, msm) = sum(9, usize::MAX);
            format!(
                    "widest {widest}; {n1} wave(s) of 1 fit ({ms1:.0} ms), {n8} of 2-8 ({ms8:.0} ms), {nm} of 9+ ({msm:.0} ms); slowest five {:?}",
                    v.iter().take(5).map(|w| (w.0, w.1.round() as u64)).collect::<Vec<_>>()
                )
        });
        eprintln!(
            "  [t] merge rounds: {rounds} ({waves} with a fit to do, {stale_refits} serial stale refits); wall ms: candidate scan {}, fit waves {}, pick+refit {}, bookkeeping {}",
            ns_scan / 1_000_000,
            ns_wave / 1_000_000,
            ns_stale / 1_000_000,
            ns_book / 1_000_000,
        );
        eprintln!(
            "  [t] merge: {} components, {} union fits over {} pixels total; fit ms (summed over threads): collect {} flat {} linear {} radial {} elliptic {}",
            n_comp,
            FIT_CALLS.load(std::sync::atomic::Ordering::Relaxed),
            FIT_PIXELS.load(std::sync::atomic::Ordering::Relaxed),
            FIT_NS_COLLECT.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000,
            FIT_NS_FLAT.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000,
            FIT_NS_LINEAR.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000,
            FIT_NS_RADIAL.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000,
            FIT_NS_ELLIPTIC.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000,
        );
    }
}

/// Step 3: the state of the greedy agglomeration over connected components.
///
/// Component ids never change: a merge of `b` into `a` moves `b`'s pixels, seams and
/// fit onto `a` and marks `b` dead, so `members`, `fits` and `alive` keep one entry per
/// original component.
struct Agglomeration<'a> {
    /// The fixed fit inputs and evidence mask.
    fitter: UnionFitter<'a>,
    /// Pixels of each component; empty once it has been absorbed.
    members: Vec<Vec<usize>>,
    /// Palette label each component started from.
    comp_label: Vec<u16>,
    /// sRGB ink of each palette label (black for a label beyond the palette).
    ink_rgb: Vec<[f32; 3]>,
    /// Current component of each pixel.
    group: Vec<u32>,
    /// Seam lengths between live components, in 4-neighbour pixel pairs (symmetric).
    adj: Vec<HashMap<u32, u32>>,
    /// Of those pairs, the smooth ones (region recovery only).
    smooth: Vec<HashMap<u32, u32>>,
    /// Of those pairs, the ones that step above the discontinuity threshold (research
    /// prototype A10, part `seam`; empty when it is off).
    sharp: Vec<HashMap<u32, u32>>,
    /// Current fit of each component.
    fits: Vec<FillFit>,
    /// Whether each component still exists.
    alive: Vec<bool>,
    /// A cached union is *stale* once one of its members has absorbed something else.
    /// It is not thrown away: a large gradient region swallowing a two-pixel fleck used
    /// to invalidate the union fit with every one of its other neighbours, and on a logo
    /// with one 13k-pixel region and fifty flecks along its edge that was 2,500 fits of
    /// 13k pixels each - 19 s of a 23 s trace. A stale union is refitted only when its
    /// cached gain would make it the merge of the round, so the accepted merge is always
    /// decided on a fresh fit while the rest wait. Keyed `(a, b)` with `a < b`; the bool
    /// is the stale flag.
    cache: HashMap<(u32, u32), (FillFit, bool)>,
    /// The common-pixel gain of each cached union, when it has been priced.
    gains: HashMap<(u32, u32), f64>,
    /// Region recovery on (see [`merge_bands_with`]).
    inner_blends: bool,
    /// `INKVEC_MERGEDBG`: print the merge decisions on large pairs.
    mergedbg: bool,
    /// `INKVEC_MERGE_COMMON_PIXELS` (research builds): price every pair on common pixels.
    common_pixels: bool,
}

impl Agglomeration<'_> {
    /// A pair whose seam is smooth (see `regions::is_smooth`) is judged as one region:
    /// blends between its members are evidence, and both sides are priced on the same
    /// pixels. Any other pair is judged exactly as before.
    fn smooth_pair(&self, a: usize, b: usize) -> bool {
        self.inner_blends && regions::is_smooth(&self.adj, &self.smooth, a, b)
    }

    /// Two flat bands of one quantised ramp are each flat -- a band is too thin to show
    /// its slope -- so the pair test never looks at them, and a ramp cut into flat bands
    /// stays cut. With region recovery on, a flat pair whose inks are one ramp step apart
    /// (CIEDE2000 below [`regions::RAMP_STEP_DE00`]) is looked at too; the union still
    /// has to win on the pixels.
    fn ramp_step(&self, a: usize, b: usize) -> bool {
        self.smooth_pair(a, b)
            && crate::color::de00(
                self.ink_rgb[self.comp_label[a] as usize],
                self.ink_rgb[self.comp_label[b] as usize],
            ) < regions::RAMP_STEP_DE00
    }

    /// Whether the pair `(a, b)` is worth a union fit at all.
    ///
    /// Two regions that are each *individually* flat are two regions, however well a ramp
    /// happens to interpolate between them — bands only exist because a smooth ramp had
    /// to be quantised, and a quantised band still carries the ramp inside it. Tested
    /// *before* the union is fitted: most pairs in a mostly-flat image are flat-flat, and
    /// fitting their union just to discard it was the single largest cost in the tracer.
    /// If one of the pair later absorbs a band and becomes a gradient, the union is
    /// fitted at that point instead. The exception is [`Self::ramp_step`].
    ///
    /// With the research part `seam` on, a pair of components of at least
    /// [`MIN_GRADIENT_PIXELS`] pixels each whose seam is mostly discontinuity
    /// ([`Self::across_edge`]) is never worth one.
    fn worth_a_union(&self, a: usize, b: usize) -> bool {
        (self.fits[a].model.is_gradient()
            || self.fits[b].model.is_gradient()
            || self.ramp_step(a, b))
            && !self.across_edge(a, b)
    }

    /// Research prototype A10, part `seam` (`gregions`): whether the seam between `a` and
    /// `b` is an edge two regions meet at, not a band boundary inside one: both components
    /// hold at least [`MIN_GRADIENT_PIXELS`] pixels and more than half of the pixel pairs
    /// across their seam step above the discontinuity threshold (`2·sharp > adj`).
    /// Always false with the part off (`sharp` is empty). Inspired by Chakraborty et al.
    /// 2025, doi:10.1111/cgf.70055, §3.2: segments facing each other across the
    /// discontinuity map are never one region. The size floor keeps anti-aliasing flecks,
    /// which are all edge, absorbable as before.
    fn across_edge(&self, a: usize, b: usize) -> bool {
        if self.sharp.is_empty()
            || self.members[a].len() < MIN_GRADIENT_PIXELS
            || self.members[b].len() < MIN_GRADIENT_PIXELS
        {
            return false;
        }
        let shared = self.adj[a].get(&(b as u32)).copied().unwrap_or(0);
        let sharp = self.sharp[a].get(&(b as u32)).copied().unwrap_or(0);
        2 * sharp > shared
    }

    /// The pixels of components `a` and `b`, `a`'s first, as the two parts a fit reads.
    fn union_parts(&self, a: usize, b: usize) -> [&[usize]; 2] {
        [&self.members[a], &self.members[b]]
    }

    /// The greedy loop: each round fits the unions it has not fitted yet, picks the pair
    /// with the largest positive gain and merges it, until no pair gains or `deadline`
    /// passes.
    ///
    /// Each round: fit every union the scan will want to look at but has not fitted yet —
    /// in parallel, since the fits are independent — then pick the best merge from the
    /// cache. The pair scan itself is cheap; the union fits are where the time goes, and
    /// computing them in waves puts every core on them without changing which merge wins.
    fn run(
        &mut self,
        deadline: Option<inkvec_core::clock::Instant>,
        live: &inkvec_core::progress::Handle,
    ) {
        let n_comp = self.members.len();
        let timing = inkvec_core::env::flag("INKVEC_TIMING");
        let mut t = MergeTiming::default();
        loop {
            t.rounds += 1;
            // A greedy merge accepts one union a round and cannot know how many it will find.
            inkvec_core::progress::step("merge rounds", t.rounds - 1, 0);
            let t_round = inkvec_core::clock::Instant::now();
            // Out of time: leave the remaining bands as the separate fills they already
            // are. Every merge accepted so far stands, so the output is a correct trace
            // with more fills than the best one -- the graceful end of a time budget.
            if let Some(d) = deadline {
                if inkvec_core::clock::Instant::now() >= d {
                    if self.mergedbg {
                        eprintln!("  merge: stopped at the time budget");
                    }
                    break;
                }
            }
            let mut missing = self.missing_unions();
            if timing {
                t.ns_scan += t_round.elapsed().as_nanos() as u64;
            }
            let t_wave = inkvec_core::clock::Instant::now();
            if !missing.is_empty() {
                t.waves += 1;
                missing.sort_unstable();
                self.fit_wave(&missing, live);
            }
            if timing {
                t.ns_wave += t_wave.elapsed().as_nanos() as u64;
                if !missing.is_empty() {
                    t.wave_log
                        .push((missing.len(), t_wave.elapsed().as_secs_f64() * 1e3));
                }
            }
            let t_pick = inkvec_core::clock::Instant::now();
            let best = self.pick_merge(&mut t.stale_refits);
            if timing {
                t.ns_stale += t_pick.elapsed().as_nanos() as u64;
            }
            let t_book = inkvec_core::clock::Instant::now();
            let Some((a, b)) = best else { break };
            self.apply_merge(a, b);
            if timing {
                t.ns_book += t_book.elapsed().as_nanos() as u64;
            }
        }
        if timing {
            t.report(n_comp);
        }
    }

    /// Candidate pairs `(a, b)`, `a < b`, both live, sharing at least
    /// [`MIN_SHARED_BOUNDARY`] pixel pairs, [`Self::worth_a_union`], and not yet in the
    /// cache. Unsorted (the adjacency is a hash map).
    fn missing_unions(&self) -> Vec<(u32, u32)> {
        let mut missing: Vec<(u32, u32)> = Vec::new();
        for a in 0..self.members.len() {
            if !self.alive[a] {
                continue;
            }
            for (&b, &shared) in self.adj[a].iter() {
                if (b as usize) <= a || shared < MIN_SHARED_BOUNDARY {
                    continue;
                }
                if !self.worth_a_union(a, b as usize) {
                    continue;
                }
                let key = (a as u32, b);
                if !self.cache.contains_key(&key) {
                    missing.push(key);
                }
            }
        }
        missing
    }

    /// Fit the unions `missing` in parallel and cache them fresh, dropping any gain
    /// priced on an earlier fit of the same pair.
    fn fit_wave(&mut self, missing: &[(u32, u32)], live: &inkvec_core::progress::Handle) {
        use rayon::prelude::*;
        let this = &*self;
        let computed: Vec<((u32, u32), (FillFit, bool))> = missing
            .par_iter()
            .map(|&(a, b)| {
                live.check();
                let (ai, bi) = (a as usize, b as usize);
                let px = this.union_parts(ai, bi);
                // Every fit already sees at most FIT_PIXELS_CAP pixels, so a
                // candidate union costs the same whatever its size and there is
                // nothing to refit: the cached fit is the fit.
                let inner = this.smooth_pair(ai, bi);
                (
                    (a, b),
                    (this.fitter.fit(&this.group, px, a, b, inner), false),
                )
            })
            .collect();
        for (k, _) in &computed {
            self.gains.remove(k);
        }
        self.cache.extend(computed);
    }

    /// The merge of this round: the pair with the largest positive gain, judged on a
    /// fresh fit. A winner whose cached union is stale is refitted (counted in
    /// `stale_refits`) and the choice made again. `None` when no pair gains.
    fn pick_merge(&mut self, stale_refits: &mut u64) -> Option<(u32, u32)> {
        loop {
            let Some((_, a, b)) = self.best_gain() else {
                break None;
            };
            if self.cache.get(&(a, b)).is_some_and(|(_, stale)| *stale) {
                // The winner was judged on a stale fit: refit it and choose again.
                inkvec_core::progress::checkpoint();
                let (ai, bi) = (a as usize, b as usize);
                let px = self.union_parts(ai, bi);
                let inner = self.smooth_pair(ai, bi);
                let fit = self.fitter.fit(&self.group, px, a, b, inner);
                self.cache.insert((a, b), (fit, false));
                self.gains.remove(&(a, b));
                *stale_refits += 1;
                continue;
            }
            break Some((a, b));
        }
    }

    /// Research prototype A10, part `segments`: find the smooth segments, ask
    /// [`super::proposals::propose`] which groups of components one gradient explains in
    /// each, and merge every accepted group into its lowest component
    /// ([`Self::absorb_group`]). Runs before the first round, on the per-component fits.
    /// With `mdl` the proposals are accepted by MDL gain, and each absorbed group's fit is
    /// re-priced on the merger's own evidence ([`UnionFitter::rescore`]); without it, by
    /// the ported residual threshold and with the proposal's own cost. Nothing happens
    /// when the segmentation exceeds its work bound.
    fn propose_regions(&mut self, mdl: bool) {
        let (rgb, w, h) = (self.fitter.rgb, self.fitter.w, self.fitter.h);
        let Some(seg) = segments::smooth_segments(rgb, w, h) else {
            return;
        };
        let found = proposals::propose(&proposals::Inputs {
            rgb,
            w,
            h,
            seg: &seg,
            members: &self.members,
            alive: &self.alive,
            adj: &self.adj,
            group: &self.group,
            fits: &self.fits,
            sigma: self.fitter.sigma_noise,
            lambda: self.fitter.lambda,
            mdl,
            debug: self.mergedbg,
        });
        for (ids, fit) in found {
            self.absorb_group(&ids, fit, mdl);
        }
    }

    /// Merge the components `ids[1..]` into `ids[0]` and give it `fit` (re-priced with
    /// [`UnionFitter::rescore`] when `reprice`). `ids` must be sorted and live, and no
    /// round may have run: every cached union and gain touching `ids` is dropped (before
    /// the first round there are none), and the pixels, seams and smooth-seam counts move
    /// exactly as [`Self::apply_merge`] moves them for a pair.
    fn absorb_group(&mut self, ids: &[u32], fit: FillFit, reprice: bool) {
        let a = ids[0];
        let ai = a as usize;
        for &b in &ids[1..] {
            let bi = b as usize;
            if !self.alive[bi] || bi == ai {
                continue;
            }
            let taken = std::mem::take(&mut self.members[bi]);
            for &p in &taken {
                self.group[p] = a;
            }
            self.members[ai].extend(taken);
            self.alive[bi] = false;
            let b_adj = std::mem::take(&mut self.adj[bi]);
            let mut b_adj: Vec<_> = b_adj.into_iter().collect();
            b_adj.sort_by_key(|&(c, _)| c);
            for (c, shared) in b_adj {
                if c == a {
                    continue;
                }
                *self.adj[ai].entry(c).or_insert(0) += shared;
                let e = &mut self.adj[c as usize];
                e.remove(&b);
                *e.entry(a).or_insert(0) += shared;
            }
            self.adj[ai].remove(&b);
            regions::absorb_counts(&mut self.smooth, ai, bi);
            if !self.sharp.is_empty() {
                regions::absorb_counts(&mut self.sharp, ai, bi);
            }
        }
        self.cache
            .retain(|&(x, y), _| !ids.contains(&x) && !ids.contains(&y));
        self.gains
            .retain(|&(x, y), _| !ids.contains(&x) && !ids.contains(&y));
        self.fits[ai] = if reprice {
            self.fitter.rescore(
                &self.group,
                &self.members[ai],
                a,
                self.inner_blends,
                fit.model,
            )
        } else {
            fit
        };
    }

    /// The best `(gain, a, b)` over every cached candidate pair, fresh or stale.
    ///
    /// Exact ties go to the lowest (a, b). `a` already runs in order, but `b` comes from
    /// a HashMap, whose iteration order changes from run to run, so without this a tie
    /// between two neighbours of one region would be decided by the hasher. The same
    /// class of bug was fixed in `contour`, `color` and `occlusion`.
    fn best_gain(&mut self) -> Option<(f64, u32, u32)> {
        let mut best: Option<(f64, u32, u32)> = None;
        for a in 0..self.members.len() {
            if !self.alive[a] {
                continue;
            }
            let neighbours: Vec<(u32, u32)> = self.adj[a]
                .iter()
                .map(|(&b, &shared)| (b, shared))
                .filter(|&(b, shared)| b as usize > a && shared >= MIN_SHARED_BOUNDARY)
                .collect();
            for (b, _) in neighbours {
                if !self.worth_a_union(a, b as usize) {
                    continue;
                }
                let Some(gain) = self.pair_gain(a, b) else {
                    continue;
                };
                let better = match best {
                    None => gain > 0.0,
                    Some((g, ba, bb)) => gain > g || (gain == g && (a as u32, b) < (ba, bb)),
                };
                if better {
                    best = Some((gain, a as u32, b));
                }
            }
        }
        best
    }

    /// What merging `a` and `b` saves, from the cached union: `None` when the union is
    /// not cached or is flat (only gradient unions merge).
    ///
    /// The legacy gain is `cost(a) + cost(b) − cost(a ∪ b)`, each cost `0.5·chi² +
    /// λ·params` on that fit's own samples. For a smooth pair (or with
    /// `INKVEC_MERGE_COMMON_PIXELS`) the gain is instead [`common_pixel_gain`], both
    /// alternatives priced on the same union-interior pixels; it is computed once per
    /// union fit and cached in `gains`, and a union with no evidence pixels scores
    /// negative infinity.
    fn pair_gain(&mut self, a: usize, b: u32) -> Option<f64> {
        let (union, _) = self.cache.get(&(a as u32, b))?;
        let (members, fits) = (&self.members, &self.fits);
        if !union.model.is_gradient() {
            if self.mergedbg && members[a].len() + members[b as usize].len() > 200 {
                eprintln!(
                    "merge: a={a}({}) {:?} | b={b}({}) {:?} | union FLAT chi2 {:.0} cost {:.0}",
                    members[a].len(),
                    fits[a].model.kind(),
                    members[b as usize].len(),
                    fits[b as usize].model.kind(),
                    union.chi2,
                    union.cost
                );
            }
            return None;
        }
        let legacy_gain = fits[a].cost + fits[b as usize].cost - union.cost;
        let inner = self.smooth_pair(a, b as usize);
        // Research prototype A10, part `cover` (`gregions`): the legacy gain compares costs
        // measured on three different pixel populations, so a member with no evidence of
        // its own (a thin band: every pixel a blend, its flat fit priced on all of them)
        // can pay for a union that wrecks its partner. On the princess emoji the crown's
        // lower band (139 px, cost 62672) bought a union with the hair (cost 67 alone,
        // union chi² 79251): +0.15 dE00. With the part on, a union must also win on common
        // pixels; the legacy gain still ranks the pairs that pass. (That union passes this
        // test too -- the band's own flat misfits the band worse -- and is stopped by the
        // part `seam`, `Self::across_edge`.)
        let cover = gregions::parts().cover && !(self.common_pixels || inner);
        let gain = if self.common_pixels || inner || cover {
            let (fitter, group) = (&self.fitter, &self.group);
            // Priced once per union fit: it reads only the pair's members and
            // fits, and both are fixed until one of them merges, which drops it.
            *self.gains.entry((a as u32, b)).or_insert_with(|| {
                let mut pixels = members[a].clone();
                pixels.extend_from_slice(&members[b as usize]);
                common_pixel_gain(
                    fitter.rgb,
                    fitter.w,
                    fitter.h,
                    &pixels,
                    group,
                    &|p: usize| fitter.pure[p] || (inner && fitter.inner_of(group, p, a as u32, b)),
                    a as u32,
                    b,
                    &fits[a],
                    &fits[b as usize],
                    union,
                    fitter.sigma_noise,
                    fitter.lambda,
                )
                .unwrap_or(f64::NEG_INFINITY)
            })
        } else {
            legacy_gain
        };
        let gain = if cover {
            if self.mergedbg && gain <= 0.0 && legacy_gain > 0.0 {
                eprintln!(
                    "gregions cover: a={a} b={b} legacy {legacy_gain:.0} common {gain:.0}: refused"
                );
            }
            if gain > 0.0 {
                legacy_gain
            } else {
                return None;
            }
        } else {
            gain
        };
        if self.mergedbg && self.common_pixels {
            eprintln!("common: a={a} b={b} legacy={legacy_gain:.3} common={gain:.3}");
        }
        if self.mergedbg && members[a].len() + members[b as usize].len() > 200 {
            eprintln!(
                "merge: a={a}({}) {:?} cost {:.0} | b={b}({}) {:?} cost {:.0} | union {:?} chi2 {:.0} cost {:.0} | gain {gain:.0}",
                members[a].len(), fits[a].model.kind(), fits[a].cost,
                members[b as usize].len(), fits[b as usize].model.kind(), fits[b as usize].cost,
                union.model.kind(), union.chi2, union.cost
            );
        }
        Some(gain)
    }

    /// Merge component `b` into `a`: carry the union's fit forward, move `b`'s pixels and
    /// seams onto `a`, and update the caches.
    ///
    /// Cached unions that involved `b` are dropped; those that involved `a` are marked
    /// stale (gradient) or dropped (flat, or any under common-pixel pricing), and every
    /// gain touching either is dropped.
    fn apply_merge(&mut self, a: u32, b: u32) {
        let (ai, bi) = (a as usize, b as usize);
        self.fits[ai] = self.cache.remove(&(a, b)).expect("cached union").0;
        let taken = std::mem::take(&mut self.members[bi]);
        for &p in &taken {
            self.group[p] = a;
        }
        self.members[ai].extend(taken);
        self.alive[bi] = false;
        let b_adj = std::mem::take(&mut self.adj[bi]);
        let mut b_adj: Vec<_> = b_adj.into_iter().collect();
        b_adj.sort_by_key(|&(c, _)| c);
        for (c, shared) in b_adj {
            if c == a {
                continue;
            }
            *self.adj[ai].entry(c).or_insert(0) += shared;
            let e = &mut self.adj[c as usize];
            e.remove(&b);
            *e.entry(a).or_insert(0) += shared;
        }
        self.adj[ai].remove(&b);
        regions::absorb_counts(&mut self.smooth, ai, bi);
        if !self.sharp.is_empty() {
            regions::absorb_counts(&mut self.sharp, ai, bi);
        }
        // A stale flat union is dropped rather than kept: it is never a candidate, so it
        // would never be refitted, and a union that came out flat before the region grew
        // can come out a gradient after (two noto icons moved 0.005 dE00 when these were
        // kept). Gradient unions are kept stale and refitted only when they would win.
        let (common_pixels, inner_blends) = (self.common_pixels, self.inner_blends);
        self.gains
            .retain(|&(x, y), _| x != a && y != a && x != b && y != b);
        self.cache.retain(|&(x, y), (fit, _)| {
            x != b && y != b && (fit.model.is_gradient() || (x != a && y != a))
                // Common-evidence gains do not inherit the legacy stale-cost bound.
                // Refit affected candidates before ranking, not only after winning.
                && (!(common_pixels || inner_blends) || (x != a && y != a))
        });
        // The stale cost is left as fitted without the absorbed pixels. That is an
        // *under*estimate of the true union cost, so the stale gain is an overestimate and
        // a stale entry that could win is always refitted before it does. Scoring the
        // absorbed pixels under the cached model instead over-estimates the cost (the
        // refit would absorb them) and stopped a radial gradient's bands from merging at
        // all (synthetic gradient_radial 0.04 -> 0.23 dE00, 18x the parameters).
        for (&(x, y), entry) in self.cache.iter_mut() {
            if x == a || y == a {
                entry.1 = true;
            }
        }
    }

    /// `INKVEC_GRADDBG`: print the components in `win` with a fresh union fit per
    /// neighbour (see [`debug::dump`]).
    fn dump(&self, win: [usize; 4]) {
        let fit = |a: u32, b: u32| {
            let px = self.union_parts(a as usize, b as usize);
            self.fitter.fit(&self.group, px, a, b, true)
        };
        debug::dump(
            win,
            self.fitter.w,
            &self.members,
            &self.alive,
            &self.fits,
            &self.adj,
            self.fitter.rgb,
            &fit,
        );
    }

    /// Step 4: write the result back into `labels` and build one fill per label id.
    ///
    /// Palette labels `0..n_pal` come first, each a flat fill of its palette colour (black
    /// past the palette). Every live component that is a gradient, or flat with at least
    /// [`MIN_GRADIENT_PIXELS`] interior pixels of its own, gets a fresh label and its own
    /// fit. The remaining thin flat components keep their palette label, whose fill is
    /// refitted flat over every flat component of that label (evidence pixels only).
    /// Also returns, per label id, the palette entry it came from.
    fn write_back(
        &self,
        labels: &mut [u16],
        pal: &Palette,
        n_pal: usize,
    ) -> (Vec<FillFit>, Vec<usize>) {
        let (rgb, w, h) = (self.fitter.rgb, self.fitter.w, self.fitter.h);
        let (sigma_noise, lambda) = (self.fitter.sigma_noise, self.fitter.lambda);
        let (members, fits, group, comp_label) =
            (&self.members, &self.fits, &self.group, &self.comp_label);
        let pure = &self.fitter.pure;
        let n = w * h;
        let mut out: Vec<FillFit> = (0..n_pal)
            .map(|i| flat_only(if i < pal.len() { pal.rgb[i] } else { [0.0; 3] }, lambda))
            .collect();
        let mut ink: Vec<usize> = (0..n_pal).collect();
        // Every flat component's pixels, by palette entry, and the entry each flat pixel
        // belongs to; `pooled[l]` when some component was left on its palette label.
        let mut flat_pixels: Vec<Vec<usize>> = vec![Vec::new(); n_pal];
        let mut flat_of = vec![u16::MAX; n];
        let mut pooled = vec![false; n_pal];
        for c in 0..members.len() {
            if !self.alive[c] {
                continue;
            }
            // A flat component with an interior of its own keeps its own colour. Two
            // disconnected regions that quantised to the same palette entry are two objects,
            // and pooling them gives each the mean of both: the lizard's belly plateau came
            // out 17 LSB too dark because it shared a palette entry with the darker end of
            // the ramp around it. A thin component has no interior to estimate a colour from
            // and keeps its palette label, coloured from every flat component of that entry —
            // the wide ones included, whose interiors say what the ink is under the
            // anti-aliasing that is all the thin one has.
            let own_colour = !fits[c].model.is_gradient()
                && out.len() < u16::MAX as usize
                && interior_count(&members[c], w, h, |p| group[p] == c as u32)
                    >= MIN_GRADIENT_PIXELS;
            if fits[c].model.is_gradient() || own_colour {
                let id = out.len() as u16;
                for &p in &members[c] {
                    labels[p] = id;
                }
                out.push(fits[c].clone());
                ink.push(comp_label[c] as usize);
            } else {
                pooled[comp_label[c] as usize] = true;
            }
            if !fits[c].model.is_gradient() {
                let l = comp_label[c];
                for &p in &members[c] {
                    flat_of[p] = l;
                }
                flat_pixels[l as usize].extend_from_slice(&members[c]);
            }
        }
        for (l, pixels) in flat_pixels.iter().enumerate() {
            if pooled[l] {
                let mut cands = fit_pixels(
                    rgb,
                    w,
                    h,
                    pixels,
                    |p| flat_of[p] as usize == l,
                    |p| pure[p],
                    sigma_noise,
                    lambda,
                );
                out[l] = cands.swap_remove(0);
            }
        }
        (out, ink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_score_ignores_incomparable_cached_residuals() {
        let (w, h) = (12, 8);
        let rgb = vec![[0.4; 3]; w * h];
        let group: Vec<u32> = (0..w * h).map(|p| u32::from(p % w >= 6)).collect();
        let pixels: Vec<_> = (0..w * h).collect();
        let pure = vec![true; w * h];
        let left = flat_only([0.4; 3], 2.0);
        let mut right = left.clone();
        right.cost = 1e12; // Measured on another population; must not enter comparison.
        right.chi2 = 1e12;
        let gain = common_pixel_gain(
            &rgb,
            w,
            h,
            &pixels,
            &group,
            &|p: usize| pure[p],
            0,
            1,
            &left,
            &right,
            &left,
            1.0 / 255.0,
            2.0,
        )
        .unwrap();
        assert!((gain - 2.0 * PARAMS_FLAT).abs() < 1e-6);
    }

    #[test]
    fn common_score_rejects_erasing_a_real_color_step() {
        let (w, h) = (12, 8);
        let group: Vec<u32> = (0..w * h).map(|p| u32::from(p % w >= 6)).collect();
        let rgb: Vec<_> = group
            .iter()
            .map(|&g| if g == 0 { [0.2; 3] } else { [0.8; 3] })
            .collect();
        let pixels: Vec<_> = (0..w * h).collect();
        let pure = vec![true; w * h];
        let gain = common_pixel_gain(
            &rgb,
            w,
            h,
            &pixels,
            &group,
            &|p: usize| pure[p],
            0,
            1,
            &flat_only([0.2; 3], 2.0),
            &flat_only([0.8; 3], 2.0),
            &flat_only([0.5; 3], 2.0),
            1.0 / 255.0,
            2.0,
        )
        .unwrap();
        assert!(gain < -1000.0);
    }

    #[test]
    fn common_score_declines_without_evidence() {
        let model = flat_only([0.0; 3], 1.0);
        assert!(common_pixel_gain(
            &[[0.0; 3]; 16],
            4,
            4,
            &(0..16).collect::<Vec<_>>(),
            &[0; 16],
            &|_: usize| false,
            0,
            1,
            &model,
            &model,
            &model,
            0.01,
            1.0
        )
        .is_none());
    }

    #[test]
    fn strided_union_takes_the_concatenations_stride_without_building_it() {
        for &(la, lb) in &[
            (0usize, 0usize),
            (5, 0),
            (0, 7),
            (3, 4),
            (FIT_PIXELS_CAP, 0),
            (FIT_PIXELS_CAP, 1),
            (1, FIT_PIXELS_CAP),
            (40_000, 40_000),
            (1_260_000, 9),
            (7, 200_001),
        ] {
            let a: Vec<usize> = (0..la).map(|i| 3 * i + 1).collect();
            let b: Vec<usize> = (0..lb).map(|i| 5 * i + 2).collect();
            let concat: Vec<usize> = a.iter().chain(&b).copied().collect();
            let want: Vec<usize> = if concat.len() > FIT_PIXELS_CAP {
                concat
                    .iter()
                    .copied()
                    .step_by(concat.len() / FIT_PIXELS_CAP)
                    .collect()
            } else {
                concat.clone()
            };
            assert_eq!(&strided_union([&a, &b])[..], &want[..], "{la} + {lb}");
        }
    }

    #[test]
    fn test_merge_gradient_bands_flat_regions() {
        let w = 4;
        let h = 4;
        let rgb = vec![[0.0f32, 0.0, 0.0]; w * h];
        let mut labels = vec![0u16; w * h];
        let pal = Palette {
            colors: vec![crate::color::rgb_to_oklab([0.0, 0.0, 0.0])],
            rgb: vec![[0.0, 0.0, 0.0]],
            weight: vec![1.0],
            alpha: vec![1.0],
        };
        let fits = merge_gradient_bands(&mut labels, &rgb, w, h, &pal, 1.0 / 255.0, 1.0);
        assert!(!fits.is_empty());
    }
}
