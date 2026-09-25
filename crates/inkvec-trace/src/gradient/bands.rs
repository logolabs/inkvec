//! Band merging: merge adjacent palette regions described more cheaply by a gradient.

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
    for i in (0..samples.len()).step_by(stride) {
        let model = if group[samples.px[i]] == a {
            &left.model
        } else {
            &right.model
        };
        let split = model.color_at(samples.x[i], samples.y[i]);
        let merged = union.model.color_at(samples.x[i], samples.y[i]);
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
    let n = w * h;
    let n_pal = pal
        .len()
        .max(labels.iter().map(|&l| l as usize + 1).max().unwrap_or(0));

    // 1. Connected components.
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
    let n_comp = members.len();

    // Adjacency with shared-boundary lengths.
    let mut adj: Vec<HashMap<u32, u32>> = vec![HashMap::new(); n_comp];
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
            }
        }
    }

    // Which pixels may testify about a fill at all (see `fill_evidence`). Computed on
    // the palette inks of the labels as they stand before any band is merged.
    let ink_rgb: Vec<[f32; 3]> = (0..n_pal)
        .map(|l| pal.rgb.get(l).copied().unwrap_or([0.0; 3]))
        .collect();
    let partner = blend_partners(rgb, w, h, labels, &ink_rgb, sigma_noise);
    let pure: Vec<bool> = partner.iter().map(|&q| q == PURE).collect();
    // A blend towards a region inside the fit is evidence for the fit; see
    // `blend_partners`.
    let inner_blends = regions::enabled();

    // 2. Per-component fits. `group[p]` is the current component of pixel `p`.
    let mut group = comp;
    // Diagnostic only: how many union fits the agglomeration asks for, and how
    // many pixels they walk in total. `fit_pixels` caps the *fitting* at
    // MAX_FIT_SAMPLES, but `collect_samples` still filters every pixel of the
    // union first, so the two numbers say whether the cost is fit count or fit size.
    static FIT_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static FIT_PIXELS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let fit_group = |group: &[u32], pixels: &[usize], a: u32, b: u32| -> FillFit {
        FIT_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let member = |p: usize| group[p] == a || group[p] == b;
        // A fit sees at most `FIT_PIXELS` pixels, taken uniformly, whatever the region's
        // size: the residual is already evaluated on a strided subsample, the medians
        // and least squares converge long before 65k samples, and the gather was the
        // one cost that still grew with the raster. The chi-square is scaled back up
        // by the subsampling factor so costs stay comparable across regions and with
        // the parameter terms. Measured identical in output on the two profiling logos
        // at 1024 and 2048 px against fitting every pixel.
        let sub: Vec<usize> = if pixels.len() > fit_pixels_cap() {
            pixels
                .iter()
                .copied()
                .step_by(pixels.len() / fit_pixels_cap())
                .collect()
        } else {
            Vec::new()
        };
        let seen = if sub.is_empty() { pixels } else { &sub[..] };
        FIT_PIXELS.fetch_add(seen.len(), std::sync::atomic::Ordering::Relaxed);
        let evidence = |p: usize| {
            pure[p] || (inner_blends && {
                let q = partner[p] as usize;
                group[q] == a || group[q] == b
            })
        };
        let mut fit = select(fit_pixels(
            rgb,
            w,
            h,
            seen,
            member,
            evidence,
            sigma_noise,
            lambda,
        ));
        if seen.len() < pixels.len() {
            let k = pixels.len() as f64 / seen.len() as f64;
            let params_term = fit.cost - 0.5 * fit.chi2;
            fit.chi2 *= k;
            fit.cost = params_term + 0.5 * fit.chi2;
        }
        fit
    };
    // Hundreds of components each pay a full model selection (the radial-centre search
    // alone is six hundred residual evaluations), and none depends on another: fit them
    // on every core.
    let mut fits: Vec<FillFit> = {
        use rayon::prelude::*;
        (0..n_comp)
            .into_par_iter()
            .map(|c| fit_group(&group, &members[c], c as u32, c as u32))
            .collect()
    };

    // 3. Greedy agglomeration.
    //
    // Each round: fit every union the scan will want to look at but has not fitted yet —
    // in parallel, since the fits are independent — then pick the best merge from the
    // cache. The pair scan itself is cheap; the union fits are where the time goes, and
    // computing them in waves puts every core on them without changing which merge wins.
    let mergedbg = std::env::var_os("INKVEC_MERGEDBG").is_some();
    // Explicit experiment; the measured production objective remains the default.
    let common_pixels =
        inner_blends || std::env::var("INKVEC_MERGE_COMMON_PIXELS").as_deref() == Ok("1");
    let mut alive = vec![true; n_comp];
    // Two flat bands of one quantised ramp are each flat -- a band is too thin to show
    // its slope -- so the pair test above never looks at them, and a ramp cut into
    // flat bands stays cut. With region recovery on, a flat pair whose inks are one
    // ramp step apart is looked at too; the union still has to win on the pixels.
    let ramp_step = |a: usize, b: usize| {
        inner_blends
            && crate::color::de00(
                ink_rgb[comp_label[a] as usize],
                ink_rgb[comp_label[b] as usize],
            ) < *regions::RAMP_STEP_DE00
    };
    // A cached union is *stale* once one of its members has absorbed something else.
    // It is not thrown away: a large gradient region swallowing a two-pixel fleck used
    // to invalidate the union fit with every one of its other neighbours, and on a logo
    // with one 13k-pixel region and fifty flecks along its edge that was 2,500 fits of
    // 13k pixels each - 19 s of a 23 s trace. A stale union is refitted only when its
    // cached gain would make it the merge of the round, so the accepted merge is always
    // decided on a fresh fit while the rest wait.
    let mut cache: HashMap<(u32, u32), (FillFit, bool)> = HashMap::new();
    // Where the rounds go, for the timing log: a greedy agglomeration accepts one merge per
    // round, so the round count is the merge count and the wall time is the sum of the
    // rounds. Which part of a round costs what is the question the log answers.
    let timing = std::env::var_os("INKVEC_TIMING").is_some();
    let (mut rounds, mut ns_scan, mut ns_wave, mut ns_stale, mut ns_book) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut waves = 0u64;
    // (fits in the wave, wall ms) per wave, to see whether the waves are wide enough to fill
    // the cores or are one big fit with a few small ones behind it.
    let mut wave_log: Vec<(usize, f64)> = Vec::new();
    let mut stale_refits = 0u64;
    loop {
        rounds += 1;
        let t_round = inkvec_core::clock::Instant::now();
        // Out of time: leave the remaining bands as the separate fills they already
        // are. Every merge accepted so far stands, so the output is a correct trace
        // with more fills than the best one -- the graceful end of a time budget.
        if let Some(d) = deadline {
            if inkvec_core::clock::Instant::now() >= d {
                if mergedbg {
                    eprintln!("  merge: stopped at the time budget");
                }
                break;
            }
        }
        let mut missing: Vec<(u32, u32)> = Vec::new();
        for a in 0..n_comp {
            if !alive[a] {
                continue;
            }
            for (&b, &shared) in adj[a].iter() {
                if (b as usize) <= a || shared < MIN_SHARED_BOUNDARY {
                    continue;
                }
                // Two regions that are each *individually* flat are two regions, however
                // well a ramp happens to interpolate between them — bands only exist
                // because a smooth ramp had to be quantised, and a quantised band still
                // carries the ramp inside it. Tested *before* the union is fitted: most
                // pairs in a mostly-flat image are flat-flat, and fitting their union
                // just to discard it was the single largest cost in the tracer. If one
                // of the pair later absorbs a band and becomes a gradient, the union is
                // fitted at that point instead.
                if !fits[a].model.is_gradient()
                    && !fits[b as usize].model.is_gradient()
                    && !ramp_step(a, b as usize)
                {
                    continue;
                }
                let key = (a as u32, b);
                if !cache.contains_key(&key) {
                    missing.push(key);
                }
            }
        }
        if timing {
            ns_scan += t_round.elapsed().as_nanos() as u64;
        }
        let t_wave = inkvec_core::clock::Instant::now();
        if !missing.is_empty() {
            waves += 1;
            missing.sort_unstable();
            use rayon::prelude::*;
            let computed: Vec<((u32, u32), (FillFit, bool))> = missing
                .par_iter()
                .map(|&(a, b)| {
                    let (ai, bi) = (a as usize, b as usize);
                    let mut px: Vec<usize> =
                        Vec::with_capacity(members[ai].len() + members[bi].len());
                    px.extend_from_slice(&members[ai]);
                    px.extend_from_slice(&members[bi]);
                    // Every fit already sees at most FIT_PIXELS_CAP pixels, so a
                    // candidate union costs the same whatever its size and there is
                    // nothing to refit: the cached fit is the fit.
                    ((a, b), (fit_group(&group, &px, a, b), false))
                })
                .collect();
            cache.extend(computed);
        }

        if timing {
            ns_wave += t_wave.elapsed().as_nanos() as u64;
            if !missing.is_empty() {
                wave_log.push((missing.len(), t_wave.elapsed().as_secs_f64() * 1e3));
            }
        }
        let t_pick = inkvec_core::clock::Instant::now();
        let best = loop {
            let mut best: Option<(f64, u32, u32)> = None;
            for a in 0..n_comp {
                if !alive[a] {
                    continue;
                }
                let neighbours: Vec<(u32, u32)> = adj[a]
                    .iter()
                    .map(|(&b, &shared)| (b, shared))
                    .filter(|&(b, shared)| b as usize > a && shared >= MIN_SHARED_BOUNDARY)
                    .collect();
                for (b, _) in neighbours {
                    if !fits[a].model.is_gradient()
                        && !fits[b as usize].model.is_gradient()
                        && !ramp_step(a, b as usize)
                    {
                        continue;
                    }
                    let Some((union, _)) = cache.get(&(a as u32, b)) else {
                        continue;
                    };
                    if !union.model.is_gradient() {
                        if mergedbg && members[a].len() + members[b as usize].len() > 200 {
                            eprintln!(
                                "merge: a={a}({}) {:?} | b={b}({}) {:?} | union FLAT chi2 {:.0} cost {:.0}",
                                members[a].len(), fits[a].model.kind(),
                                members[b as usize].len(), fits[b as usize].model.kind(),
                                union.chi2, union.cost
                            );
                        }
                        continue;
                    }
                    let legacy_gain = fits[a].cost + fits[b as usize].cost - union.cost;
                    let gain = if common_pixels {
                        let mut pixels = members[a].clone();
                        pixels.extend_from_slice(&members[b as usize]);
                        common_pixel_gain(
                            rgb,
                            w,
                            h,
                            &pixels,
                            &group,
                            &|p: usize| {
                                pure[p]
                                    || (inner_blends && {
                                        let q = group[partner[p] as usize];
                                        q == a as u32 || q == b
                                    })
                            },
                            a as u32,
                            b,
                            &fits[a],
                            &fits[b as usize],
                            union,
                            sigma_noise,
                            lambda,
                        )
                        .unwrap_or(f64::NEG_INFINITY)
                    } else {
                        legacy_gain
                    };
                    if mergedbg && common_pixels {
                        eprintln!("common: a={a} b={b} legacy={legacy_gain:.3} common={gain:.3}");
                    }
                    if mergedbg && members[a].len() + members[b as usize].len() > 200 {
                        eprintln!(
                            "merge: a={a}({}) {:?} cost {:.0} | b={b}({}) {:?} cost {:.0} | union {:?} chi2 {:.0} cost {:.0} | gain {gain:.0}",
                            members[a].len(), fits[a].model.kind(), fits[a].cost,
                            members[b as usize].len(), fits[b as usize].model.kind(), fits[b as usize].cost,
                            union.model.kind(), union.chi2, union.cost
                        );
                    }
                    // Exact ties go to the lowest (a, b). `a` already runs in order, but `b`
                    // comes from a HashMap, whose iteration order changes from run to run, so
                    // without this a tie between two neighbours of one region would be
                    // decided by the hasher. The same class of bug was fixed in `contour`,
                    // `color` and `occlusion`.
                    let better = match best {
                        None => gain > 0.0,
                        Some((g, ba, bb)) => gain > g || (gain == g && (a as u32, b) < (ba, bb)),
                    };
                    if better {
                        best = Some((gain, a as u32, b));
                    }
                }
            }
            let Some((_, a, b)) = best else { break None };
            if cache.get(&(a, b)).is_some_and(|(_, stale)| *stale) {
                // The winner was judged on a stale fit: refit it and choose again.
                let (ai, bi) = (a as usize, b as usize);
                let mut px: Vec<usize> = Vec::with_capacity(members[ai].len() + members[bi].len());
                px.extend_from_slice(&members[ai]);
                px.extend_from_slice(&members[bi]);
                cache.insert((a, b), (fit_group(&group, &px, a, b), false));
                stale_refits += 1;
                continue;
            }
            break Some((a, b));
        };
        if timing {
            ns_stale += t_pick.elapsed().as_nanos() as u64;
        }
        let t_book = inkvec_core::clock::Instant::now();
        let Some((a, b)) = best else { break };
        let (ai, bi) = (a as usize, b as usize);
        fits[ai] = cache.remove(&(a, b)).expect("cached union").0;
        let taken = std::mem::take(&mut members[bi]);
        for &p in &taken {
            group[p] = a;
        }
        members[ai].extend(taken);
        alive[bi] = false;
        let b_adj = std::mem::take(&mut adj[bi]);
        let mut b_adj: Vec<_> = b_adj.into_iter().collect();
        b_adj.sort_by_key(|&(c, _)| c);
        for (c, shared) in b_adj {
            if c == a {
                continue;
            }
            *adj[ai].entry(c).or_insert(0) += shared;
            let e = &mut adj[c as usize];
            e.remove(&b);
            *e.entry(a).or_insert(0) += shared;
        }
        adj[ai].remove(&b);
        // A stale flat union is dropped rather than kept: it is never a candidate, so it
        // would never be refitted, and a union that came out flat before the region grew
        // can come out a gradient after (two noto icons moved 0.005 dE00 when these were
        // kept). Gradient unions are kept stale and refitted only when they would win.
        cache.retain(|&(x, y), (fit, _)| {
            x != b && y != b && (fit.model.is_gradient() || (x != a && y != a))
                // Common-evidence gains do not inherit the legacy stale-cost bound.
                // Refit affected candidates before ranking, not only after winning.
                && (!common_pixels || (x != a && y != a))
        });
        // The stale cost is left as fitted without the absorbed pixels. That is an
        // *under*estimate of the true union cost, so the stale gain is an overestimate and
        // a stale entry that could win is always refitted before it does. Scoring the
        // absorbed pixels under the cached model instead over-estimates the cost (the
        // refit would absorb them) and stopped a radial gradient's bands from merging at
        // all (synthetic gradient_radial 0.04 -> 0.23 dE00, 18x the parameters).
        for (&(x, y), entry) in cache.iter_mut() {
            if x == a || y == a {
                entry.1 = true;
            }
        }
        if timing {
            ns_book += t_book.elapsed().as_nanos() as u64;
        }
    }

    if timing {
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

    if let Some(win) = debug::window() {
        let fit = |a: u32, b: u32| {
            let mut px = members[a as usize].clone();
            px.extend_from_slice(&members[b as usize]);
            fit_group(&group, &px, a, b)
        };
        debug::dump(win, w, &members, &alive, &fits, &adj, rgb, &fit);
    }

    // 4. Write back.
    let mut out: Vec<FillFit> = (0..n_pal)
        .map(|i| flat_only(if i < pal.len() { pal.rgb[i] } else { [0.0; 3] }, lambda))
        .collect();
    let mut ink: Vec<usize> = (0..n_pal).collect();
    // Every flat component's pixels, by palette entry, and the entry each flat pixel
    // belongs to; `pooled[l]` when some component was left on its palette label.
    let mut flat_pixels: Vec<Vec<usize>> = vec![Vec::new(); n_pal];
    let mut flat_of = vec![u16::MAX; n];
    let mut pooled = vec![false; n_pal];
    for c in 0..n_comp {
        if !alive[c] {
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
            && interior_count(&members[c], w, h, |p| group[p] == c as u32) >= MIN_GRADIENT_PIXELS;
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
