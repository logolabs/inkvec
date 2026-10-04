//! The per-pixel palette passes as they shipped before the per-colour rewrite, kept
//! verbatim as the oracle the rewrite is tested against (`color/reference_tests.rs` is
//! compiled only for tests). Every function here must give the same bits as its
//! counterpart in `color/mdl.rs` and `color/distinct.rs`; see the equality tests at the end.
#![allow(dead_code, clippy::too_many_arguments, unreachable_pub)]

use rayon::prelude::*;

use super::*;

/// The shipped per-candidate rayon grain.
const PAR_MIN_LEN: usize = 8192;

/// [`super::represent::unexplained`], per pixel: written independently of it over the
/// visited pixels `0, s, 2s, ...` (`s = stride_px`) of an `n`-pixel image, for both oracles.
///
/// A visited pixel `p` on the `w × h` grid that the candidate claims (`claimed(p)`) votes
/// when its value is farther than `tol` from the nearest mixture of the inks around it: the
/// ink `ink_of(q)` of each in-image 8-neighbour `q` it does not claim, the [`MIX_INKS`] most
/// frequent first (ties to the lower ink), then `ground`. Returns votes times `s`; 0
/// without a full grid.
pub(crate) fn unexplained_per_pixel<const N: usize>(
    (w, h, n, stride_px): (usize, usize, usize, usize),
    claimed: impl Fn(usize) -> bool,
    ink_of: impl Fn(usize) -> Option<usize>,
    value: impl Fn(usize) -> [f32; N],
    inks: &[[f32; N]],
    ground: Option<[f32; N]>,
    tol: f32,
) -> usize {
    use super::represent::MIX_INKS;
    if w == 0 || h == 0 || n < w * h {
        return 0;
    }
    let mut votes = 0;
    for p in (0..w * h).step_by(stride_px.max(1)) {
        if !claimed(p) {
            continue;
        }
        let (x, y) = ((p % w) as i64, (p / w) as i64);
        let mut tally: Vec<(usize, u32)> = Vec::new();
        for dy in -1..=1i64 {
            for dx in -1..=1i64 {
                let (nx, ny) = (x + dx, y + dy);
                if (dx == 0 && dy == 0) || nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let q = ny as usize * w + nx as usize;
                if claimed(q) {
                    continue;
                }
                if let Some(k) = ink_of(q) {
                    match tally.iter_mut().find(|e| e.0 == k) {
                        Some(e) => e.1 += 1,
                        None => tally.push((k, 1)),
                    }
                }
            }
        }
        tally.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut cols: Vec<[f32; N]> = tally.iter().take(MIX_INKS).map(|e| inks[e.0]).collect();
        cols.extend(ground);
        let v = value(p);
        let r = if cols.is_empty() {
            f32::INFINITY
        } else if cols.len() == 1 {
            (0..N)
                .map(|c| (v[c] - cols[0][c]).powi(2))
                .sum::<f32>()
                .sqrt()
        } else {
            crate::native::mixture(v, &cols).map_or(f32::INFINITY, |m| m.0)
        };
        // The overshoot exception is the shipped function itself, as `mixture` is: this
        // oracle checks which pixels and which inks are asked, not the geometry.
        if r > tol && represent::overshoot_residual(v, &cols, ground.is_some()) > tol {
            votes += 1;
        }
    }
    votes * stride_px.max(1)
}

/// What fraction of the pixels `c` would claim sit between a pixel nearer ink `a` and a
/// pixel nearer ink `b`? See [`BLEND_STRADDLE_FRACTION`].
///
/// Position along the axis is measured in the space the blend was accepted in, by
/// projecting every pixel onto the A–B segment.
///
/// # The formula
///
/// In linear RGB when `linear`, else sRGB, with `A`, `B` the two inks in that space:
/// `t(p) = ((p − A) · (B − A)) / |B − A|²`, so `t = 0` at A and `t = 1` at B. A claimed
/// pixel (one with `|lab_i − c| < nearest_i` in OKLab, visited every `stride_px` pixels)
/// *straddles* when its 3x3 neighbourhood holds a pixel with `t < t_c − step_lo` and one with
/// `t > t_c + step_hi`, where `t_c = t(c)` and
/// `step_lo = max(min(STRADDLE_STEP, t_c / 2), 0.02)`,
/// `step_hi = max(min(STRADDLE_STEP, (1 − t_c) / 2), 0.02)`.
/// Returns `straddling / claimed`.
///
/// `lab`, `px_srgb` and `px_lin` are the same image in three spaces, precomputed by
/// [`extract_palette_mdl`]; `nearest[i]` is pixel `i`'s OKLab distance to the nearest ink
/// accepted so far.
///
/// # Edge cases
///
/// A size mismatch or an empty image, and a degenerate axis (`|B − A|² < 1e-9`), return 0
/// (never straddles, so the candidate is kept). A candidate that claims no pixel returns 1:
/// with nothing of its own to protect, the interior test's verdict stands.
#[allow(clippy::too_many_arguments)]
fn straddle_fraction(
    lab: &[Oklab],
    px_srgb: &[[f32; 3]],
    px_lin: &[[f32; 3]],
    width: usize,
    height: usize,
    c: Oklab,
    nearest: &[f32],
    a: Oklab,
    b: Oklab,
    linear: bool,
    stride_px: usize,
) -> f32 {
    if width == 0 || height == 0 || lab.len() < width * height {
        return 0.0;
    }
    let f = |x: Oklab| -> [f32; 3] {
        let r = oklab_to_rgb(x);
        if linear {
            [
                srgb_to_linear(r[0]),
                srgb_to_linear(r[1]),
                srgb_to_linear(r[2]),
            ]
        } else {
            r
        }
    };
    let (pa, pb) = (f(a), f(b));
    let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
    let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if dd < 1e-9 {
        return 0.0;
    }
    let t_of = |x: Oklab| -> f32 {
        let p = f(x);
        ((p[0] - pa[0]) * d[0] + (p[1] - pa[1]) * d[1] + (p[2] - pa[2]) * d[2]) / dd
    };
    let tc = t_of(c);
    // A candidate near one end of the axis has that ink within less than a full step:
    // the far side is judged by the step, the near side by half the room that is left.
    let step_lo = STRADDLE_STEP.min(0.5 * tc).max(0.02);
    let step_hi = STRADDLE_STEP.min(0.5 * (1.0 - tc)).max(0.02);
    // Each pixel's position along the axis, from its *cached* colour rather than
    // by converting it here. `t_of` costs an oklab_to_rgb and, in linear space,
    // three powf calls, and it was being paid per pixel per candidate per pair --
    // 200 million cube roots on a 512 px input, which is where the palette stage's
    // 36 seconds went. The pixel's own colour does not depend on the pair, so it
    // is converted once for the whole image in `extract_palette_mdl`.
    //
    // Territory and axis position are both evaluated only where the reduction reads
    // them -- the sampled pixels inside the candidate's territory and their neighbours
    // -- rather than as two whole-image arrays built per call.
    let px: &[[f32; 3]] = if linear { px_lin } else { px_srgb };
    let t = |j: usize| {
        let p = px[j];
        ((p[0] - pa[0]) * d[0] + (p[1] - pa[1]) * d[1] + (p[2] - pa[2]) * d[2]) / dd
    };
    let (total, straddle) = (0..width * height)
        .into_par_iter()
        .step_by(stride_px)
        .with_min_len(PAR_MIN_LEN)
        .filter(|&i| lab[i].dist(c) < nearest[i])
        .map(|i| {
            let (x, y) = (i % width, i / width);
            let (mut lower, mut higher) = (false, false);
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                        continue;
                    }
                    let tn = t(ny as usize * width + nx as usize);
                    lower |= tn < tc - step_lo;
                    higher |= tn > tc + step_hi;
                }
            }
            (1u32, (lower && higher) as u32)
        })
        .reduce(|| (0u32, 0u32), |a, b| (a.0 + b.0, a.1 + b.1));
    if total == 0 {
        // Nothing of its own to protect: let the interior test's verdict stand.
        return 1.0;
    }
    straddle as f32 / total as f32
}

/// What fraction of the pixels this colour would *claim* are interior to the claim?
///
/// The set has to be the pixels `c` would take from the palette as it currently stands —
/// those nearer to `c` than to anything already accepted — and not simply the pixels
/// within some radius of `c`. A ball is the obvious choice and it is wrong: an
/// anti-aliased colour sits close to one end of its ramp, so a ball around it swallows
/// the solid region as well as the band, and the band then measures as solid. Tried that
/// way, the green-circle case got worse rather than better, 29 faces to 40.
///
/// Near zero for an anti-aliased boundary band; near one for a filled region.
///
/// Formally: with the claim `M = { i : |lab_i − c| < nearest_i }` (OKLab), visited every
/// `stride_px` pixels, returns `|{ i ∈ M : all in-image 4-neighbours of i are in M }| / |M|`.
/// This is one step of binary erosion with a 4-neighbour cross. Returns 0 when `c` claims
/// nothing, and 1 when the geometry is missing (size mismatch or empty image).
fn interior_fraction(
    lab: &[Oklab],
    width: usize,
    height: usize,
    c: Oklab,
    nearest: &[f32],
    stride_px: usize,
) -> f32 {
    if width == 0 || height == 0 || lab.len() < width * height {
        // Without the geometry there is nothing to measure; claim solidity so the caller
        // falls back on its other evidence rather than discarding the colour.
        return 1.0;
    }
    // Evaluated only where the reduction reads it: the sampled pixels and their four
    // neighbours, not as a whole-image array built per call.
    let mask = |i: usize| lab[i].dist(c) < nearest[i];
    let (total, interior) = (0..width * height)
        .into_par_iter()
        .step_by(stride_px)
        .with_min_len(PAR_MIN_LEN)
        .filter(|&i| mask(i))
        .map(|i| {
            let (x, y) = (i % width, i / width);
            // A border pixel has no neighbour outside the image to disqualify it; treat
            // the outside as matching, so a region touching the edge is not penalised.
            let ok = (x == 0 || mask(i - 1))
                && (x + 1 == width || mask(i + 1))
                && (y == 0 || mask(i - width))
                && (y + 1 == height || mask(i + width));
            (1u32, ok as u32)
        })
        .reduce(|| (0u32, 0u32), |a, b| (a.0 + b.0, a.1 + b.1));
    if total == 0 {
        return 0.0;
    }
    interior as f32 / total as f32
}

/// Is `c` explained as a mixture of two colours already accepted?
///
/// This is the principled test, and the one that matters. An anti-aliased pixel is *by
/// definition* a coverage-weighted blend of the two inks it sits between, so it lies on
/// the segment joining them. A colour that lands near the middle of such a segment is
/// not a new ink no matter how often it occurs — it is evidence about geometry, which is
/// what S2 will use it for.
///
/// Compositing is linear in *linear* light, so the test is done there rather than in
/// OKLab (whose cube root would bend a straight blend line) or in sRGB. Some pipelines do
/// composite in sRGB anyway, so a blend is accepted in either space.
///
/// Returns every pair of inks `c` could be a blend of, each with whether the blend was
/// found in linear light. A pale pink is within tolerance of the white–grey axis as well
/// as the white–red one, and only the pair its pixels actually lie between can say
/// whether it straddles them; the caller tries them all.
///
/// `tmin` is [`BLEND_TMIN`].
///
/// # The formula
///
/// For each pair `(A, B)` of accepted inks and each space (linear RGB first, then sRGB),
/// with `p` the candidate in that space: `t = ((p − A) · (B − A)) / |B − A|²`, the nearest
/// point on the chord `q = A + t (B − A)`, and the residual
/// `off = |c − OKLab(clamp(q, 0, 1))|` measured in OKLab. The pair is returned as
/// `(i, j, linear, off)` when `tmin ≤ t ≤ 1 − tmin` and `off ≤ tol`. Pairs whose inks
/// coincide in that space (`|B − A|² < 1e-9`) are skipped; fewer than two inks give an
/// empty list. A pair can appear twice, once per space.
fn blend_pairs(
    c: Oklab,
    accepted: &[Oklab],
    tol: f32,
    tmin: f32,
) -> Vec<(usize, usize, bool, f32)> {
    let mut out: Vec<(usize, usize, bool, f32)> = Vec::new();
    if accepted.len() < 2 {
        return out;
    }
    let to_lin = |x: Oklab| {
        let r = oklab_to_rgb(x);
        [
            srgb_to_linear(r[0]),
            srgb_to_linear(r[1]),
            srgb_to_linear(r[2]),
        ]
    };
    let srgb = |x: Oklab| oklab_to_rgb(x);

    for space in 0..2 {
        let f = |x: Oklab| if space == 0 { to_lin(x) } else { srgb(x) };
        let p = f(c);
        for i in 0..accepted.len() {
            for j in i + 1..accepted.len() {
                let (a, b) = (f(accepted[i]), f(accepted[j]));
                let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                if dd < 1e-9 {
                    continue;
                }
                let t = ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1] + (p[2] - a[2]) * d[2]) / dd;
                // Only interior mixtures count; t outside (0, 1) is a different colour,
                // not a blend of these two.
                if !(tmin..=1.0 - tmin).contains(&t) {
                    continue;
                }
                let q = [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t];
                // Compare the residual perceptually, in OKLab.
                let back = if space == 0 {
                    rgb_to_oklab([
                        linear_to_srgb(q[0].clamp(0.0, 1.0)),
                        linear_to_srgb(q[1].clamp(0.0, 1.0)),
                        linear_to_srgb(q[2].clamp(0.0, 1.0)),
                    ])
                } else {
                    rgb_to_oklab([
                        q[0].clamp(0.0, 1.0),
                        q[1].clamp(0.0, 1.0),
                        q[2].clamp(0.0, 1.0),
                    ])
                };
                let off = c.dist(back);
                if off <= tol {
                    out.push((i, j, space == 0, off));
                }
            }
        }
    }
    out
}

/// How many pixels candidate `c` would claim, and how tightly its own members sit.
///
/// Returns `(claim, spread)`:
///
/// * `claim`: the number of pixels strictly nearer to `c` than to any accepted ink
///   (`|lab_i − c| < nearest_px_i`, OKLab), counted every `stride_px` pixels and scaled
///   back up by `stride_px`, so it estimates a full-image pixel count;
/// * `spread`: the **median** OKLab distance from `c` of the claimed pixels that also lie
///   within `tol` of it (its *members*), taken only at pixel indices that are multiples of
///   `max(⌊n / 8192⌋, 1)` to bound the sort. 0 when there are none.
///
/// # Why the spread
///
/// This is the scale at which the image itself says "these pixels are the same colour",
/// and it is measured rather than assumed, which matters because no single number can
/// stand in for it. The merge threshold is a distance in OKLab and OKLab's lightness is
/// cube-root-like, so one sRGB level near black is a far larger distance than one level
/// near white: on a logo upscaled with under two levels of error, seven separate inks were
/// accepted that were all, in fact, black -- `#000000`, `#010300`, `#000002`, `#020000`,
/// `#000100`, `#010002`, `#030100` -- each claiming tens of thousands of pixels.
///
/// A global noise estimate cannot supply this either. `coverage::estimate_noise` takes a
/// median over the whole image, and an icon is mostly empty, so more than half its pixels
/// are exactly flat and the median is zero however noisy the artwork is. Measured on that
/// same upscaled logo it returned its floor of half a level. Selecting flatter pixels
/// first does not rescue it: the empty background is flat too, and it is the majority.
///
/// The spread of the pixels a candidate actually claims has neither problem. It is local,
/// so it scales with the colour space where the colour is; it is zero on an exact-coverage
/// intake, so the shipped behaviour is unchanged; and it needs nothing to be estimated
/// globally at all.
fn claim_spread(
    lab: &[Oklab],
    nearest_px: &[f32],
    c: Oklab,
    tol: f32,
    stride_px: usize,
) -> (usize, f32) {
    const MAX_SAMPLES: usize = 8192;
    let stride = (lab.len() / MAX_SAMPLES).max(1);
    // Every core, in one ordered pass: territory count and spread sample test the same
    // distance at the same pixels. rayon's collect keeps sequential order, so `d_in` and
    // therefore its median are exactly what one thread would have produced.
    let claimed: Vec<(usize, f32)> = (0..lab.len())
        .into_par_iter()
        .step_by(stride_px)
        .with_min_len(PAR_MIN_LEN)
        .filter_map(|i| {
            let dist = lab[i].dist(c);
            (dist < nearest_px[i]).then_some((i, dist))
        })
        .collect();
    let n = claimed.len();
    // Members, not territory. Territory is whatever has no closer ink yet, which
    // for the first candidate is the whole image, and a spread measured over that
    // is the mean distance from every pixel to white -- enormous, and it rejected
    // every colour after the first. Measured: the screen set went from 0.4328 to
    // 1.2461 before this was restricted to `tol`.
    let mut d_in: Vec<f32> = claimed
        .iter()
        .filter(|&&(i, dist)| dist < tol && i % stride == 0)
        .map(|&(_, dist)| dist)
        .collect();
    // `n` is compared against absolute pixel counts downstream, so a strided
    // visit is scaled back up to estimate what a full one would have counted.
    let n = n * stride_px;
    if d_in.is_empty() {
        return (n, 0.0);
    }
    // The MEDIAN of those distances, not the mean. An anti-aliased edge puts a ramp of
    // blend pixels inside `tol` on a perfectly clean image, and a mean is pulled up by
    // them: that alone cost the screen set 2 % before this was a median. Blends are a
    // minority of any region's members, so the median ignores them and reads zero on an
    // exact-coverage intake, which is what keeps this from touching the shipped corpus.
    d_in.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (n, d_in[d_in.len() / 2])
}

/// The shipped `extract_palette_mdl`, per pixel.
pub fn extract_palette_mdl(
    rgb: &[[f32; 3]],
    width: usize,
    height: usize,
    merge_distance: f32,
    max_colors: usize,
    ev: PaletteEvidence,
) -> Palette {
    let PaletteEvidence {
        sigma_noise,
        lambda,
        noise_sigmas,
        same_ink_de00,
    } = ev;

    let view = PixelViews::new(rgb, width, height);
    let modes = frequency_modes(&view.lab);

    let total_px = view.lab.len().max(1) as f32;
    let mut colors: Vec<Oklab> = Vec::new();
    // Distance from each pixel to the nearest ink accepted so far, so a candidate's own
    // territory can be read off without rescanning the whole palette.
    let mut nearest_px: Vec<f32> = vec![f32::INFINITY; view.lab.len()];
    let paldbg = inkvec_core::env::flag("INKVEC_PALDBG");
    // The perceptual-merge experiment below, in a `research` build only.
    let de00_radius: Option<f32> = if cfg!(feature = "research") {
        inkvec_core::env::number("INKVEC_MERGE_DE00").map(|v| v as f32)
    } else {
        None
    };
    for (tested, (n, _key, c)) in modes.iter().enumerate() {
        if colors.len() >= max_colors {
            break;
        }
        // Each candidate is a pass over the pixels; most are turned down.
        inkvec_core::progress::step("colour candidates", tested as u64, modes.len() as u64);
        // How many pixels would this candidate actually take? Not `n`, which counts one
        // bin of a 24-cubed grid in OKLab.
        //
        // The difference is not cosmetic. A colour whose pixels straddle a bin boundary is
        // split across several bins and every one of them is counted small, so both tests
        // below -- the rarity floor and the description-length escape -- see a fraction of
        // the evidence that exists and refuse an ink the image plainly contains. That is
        // measurable downstream: the regions the tracer paints flat where the artwork
        // varies are two inks merged into one, and fitting them showed the best pair of
        // flat colours removing 66 % of the error there against 13 % for the best linear
        // ramp. Counting the territory costs one pass over the image per candidate and
        // answers the question asked.
        let (claim, spread) =
            claim_spread(&view.lab, &nearest_px, *c, merge_distance, view.stride_px);
        // Rare candidates must be represented (`super::represent`), checked below.
        let rare = (claim as f32 / total_px) < MIN_INK_WEIGHT && !colors.is_empty();
        let nearest = colors
            .iter()
            .map(|&p| p.dist(*c))
            .fold(f32::INFINITY, f32::min);
        // Two inks are two inks only if they are further apart than the noise, and the
        // noise has to be measured where they are. On a clean intake `sigma_noise` sits at
        // its floor of half a level and this term is far below `merge_distance`, so
        // nothing changes; on a degraded or upscaled input it grows, and it grows most
        // where the colour space is most stretched, which is where the spurious inks were.
        // Apart by more than their own spread, or they are one ink measured twice.
        let reach = noise_sigmas * spread;
        // Two inks nobody can tell apart are one ink. Decided perceptually, before any
        // description-length argument, because the argument counts pixels and pixels
        // are exactly what an anti-aliasing ramp near an ink has plenty of.
        if same_ink_as_accepted(*c, &colors, same_ink_de00, paldbg) {
            continue;
        }
        // EXPERIMENT (`INKVEC_MERGE_DE00=<radius>`, research builds only): decide the merge PERCEPTUALLY rather
        // than by Euclidean distance in OKLab.
        //
        // The motivation is the failure documented on `SAME_INK_DE00`: OKLab's lightness is
        // a cube root, so the first sRGB level above black spans thirty times the step at
        // mid-grey, and a one-ink black logo minted #020202, #040404 and #070707 as three
        // more inks at 0.078, 0.028 and 0.049 -- over twice `merge_distance`. In CIEDE2000
        // those sit at 0.31, 0.63 and 1.11, differences no viewer can see. The shipped fix
        // is a dE00 FLOOR applied after the fact; this asks whether using dE00 as the
        // distance itself removes the problem at its root instead of patching it.
        //
        // Measured at 512 px, so a reader can check whether it was worth it:
        // switching the palette to plain sRGB instead (`INKVEC_PALETTE_RGB`, research) cost +15%
        // parameters and +1% colour on JPEG q40 and changed nothing on clean input, which
        // is why the space is not the lever and the metric might be.
        let merged = if let Some(rad) = de00_radius {
            let d = colors
                .iter()
                .map(|&p| de00(oklab_to_rgb(*c), oklab_to_rgb(p)))
                .fold(f32::INFINITY, f32::min);
            // `reach` stays in OKLab units and still applies, so the noise guard behaves
            // exactly as before on a degraded intake.
            d <= rad || nearest <= reach
        } else {
            nearest <= merge_distance.max(reach)
        };
        if merged {
            // Inside the fixed threshold. Keep it anyway if the evidence is overwhelming:
            // enough pixels, separated far enough above the noise, that explaining them
            // with the nearest ink would cost more residual than a new ink costs to state.
            let worth_it = sigma_noise > 0.0
                && nearest > JND_FLOOR
                && nearest > reach
                && 0.5 * (claim as f64) * ((nearest as f64 / sigma_noise).powi(2))
                    > lambda * PARAMS_PER_INK;
            if !worth_it {
                continue;
            }
        }
        if rare && !represented_per_pixel(&view, *c, &colors, &nearest_px, sigma_noise) {
            continue;
        }
        // Explained as a blend of inks already accepted *and* shaped like a boundary
        // band rather than a region: coverage evidence, not a new colour.
        let shape = BlendEvidence::measure(&view, *c, &colors, &nearest_px, merge_distance, merged);
        // Why every candidate was kept or dropped. A wrong palette does not look like a
        // palette bug downstream — the green-circle case surfaced as a spurious radial
        // gradient and twenty-seven junk paths — so the decision has to be readable
        // directly. `INKVEC_PALDBG=1`.
        if paldbg {
            eprintln!(
                "  cand {:<9} bin={:<6} claim={:<6} w={:.4} sig={:.5} reach={:.4} near={:.4} blend={} chord={:.4} interior={:.3} straddle={:.3}",
                to_hex(oklab_to_rgb(*c)),
                n,
                claim,
                claim as f32 / total_px,
                sigma_noise,
                reach,
                nearest,
                shape.blend,
                if shape.blend { shape.chord_off } else { f32::NAN },
                shape.interior,
                shape.straddle
            );
        }
        // Both spatial tests are required, and the colour-space distance does not override
        // them. **Letting a conclusive chord decide alone was tried on 2026-09-09 and is a
        // 20 % regression** -- screen-set objective 0.4005 -> 0.4826 at 128, and worse on
        // every tier, losing on colour error and parameter count at once. (The rule tried
        // was: a candidate within 0.006 of a chord is a blend whatever its shape. It was
        // motivated by a brand mark upscaled x4, where seven invented tones sat 0.0000 to
        // 0.0037 from a chord and several survived because the wider ramp had an interior.)
        //
        // The reason is worth keeping, because the idea is seductive and correct in theory:
        // a colour lying exactly on the chord between two inks *is* a mixture of them, and
        // in a three-dimensional space that is not a coincidence. But a designer may also
        // simply choose that colour, and then it is an ink that happens to be a mixture --
        // and there are far more of those in real artwork than the geometry suggests. The
        // interior test is what protects them: a chosen tint covers area, an anti-aliased
        // ramp does not.
        //
        // The measurement that appeared to clear this was wrong, and the mistake is easy to
        // repeat: it checked that the chord rule agreed with the existing rule on every
        // candidate the existing rule *dropped* (1284 of 1284 across thirty icons) and never
        // asked what the chord rule would newly drop. Agreement on the accepted set says
        // nothing about the rejected set.
        if shape.is_coverage() {
            continue;
        }
        // The escape rule (`crate::color::escape_needs_interior`), as the shipped walk.
        if escape_needs_interior(merged, shape.blend, shape.interior) {
            continue;
        }
        nearest_px
            .par_iter_mut()
            .zip(view.lab.par_iter())
            .for_each(|(d, &q)| *d = d.min(q.dist(*c)));
        colors.push(*c);
    }
    if colors.is_empty() {
        colors.push(modes.first().map(|m| m.2).unwrap_or(Oklab {
            l: 1.0,
            a: 0.0,
            b: 0.0,
        }));
    }

    let weight = refine_to_members(&view.lab, &mut colors, merge_distance, total_px);
    let rgb_out: Vec<[f32; 3]> = colors.iter().map(|&c| oklab_to_rgb(c)).collect();
    let alpha = vec![1.0; colors.len()];
    Palette {
        colors,
        rgb: rgb_out,
        weight,
        alpha,
    }
}

/// Whether rare candidate `c` is represented (`super::represent`), per pixel: its
/// unexplained claimed pixels, with each neighbour's ink the accepted ink nearest it in
/// OKLab (ties to the earlier one), against the share floor.
fn represented_per_pixel(
    view: &PixelViews,
    c: Oklab,
    colors: &[Oklab],
    nearest_px: &[f32],
    sigma_noise: f64,
) -> bool {
    let srgb: Vec<[f32; 3]> = colors.iter().map(|&k| oklab_to_rgb(k)).collect();
    let ink_of = |q: usize| {
        let mut best: Option<(usize, f32)> = None;
        for (k, &ink) in colors.iter().enumerate() {
            let d = view.lab[q].dist(ink);
            if best.is_none_or(|b| d < b.1) {
                best = Some((k, d));
            }
        }
        best.map(|b| b.0)
    };
    let votes = unexplained_per_pixel(
        (view.width, view.height, view.lab.len(), view.stride_px),
        |q| view.lab[q].dist(c) < nearest_px[q],
        ink_of,
        |q| view.px_srgb[q],
        &srgb,
        None,
        represent::mixture_tolerance(sigma_noise),
    );
    represent::represented(votes, view.lab.len().max(1) as f32)
}

/// The image in the three colour spaces the palette's tests read, converted once.
///
/// `straddle_fraction` places a pixel on a colour axis that changes with every candidate
/// pair, while the pixel's own colour does not, and the conversion is a cube root plus
/// three `powf` calls; converting per call was 200 million cube roots on a 512 px input.
struct PixelViews {
    /// Every pixel in OKLab.
    lab: Vec<Oklab>,
    /// Every pixel back in sRGB `[0, 1]`, converted *from `lab`* rather than copied from
    /// the input, so the arithmetic is bit-identical to what a per-pixel call computed.
    px_srgb: Vec<[f32; 3]>,
    /// Every pixel in linear RGB, decoded from `px_srgb`.
    px_lin: Vec<[f32; 3]>,
    width: usize,
    height: usize,
    /// Stride of the statistical passes, from [`stat_stride`].
    stride_px: usize,
}

impl PixelViews {
    /// Convert `rgb` (sRGB `[0, 1]`, row-major `width × height`) on every core.
    fn new(rgb: &[[f32; 3]], width: usize, height: usize) -> Self {
        let lab: Vec<Oklab> = rgb.par_iter().map(|&c| rgb_to_oklab(c)).collect();

        // How many pixels each statistical pass visits. These passes run once per
        // palette candidate, so leaving them unbounded makes the stage grow with
        // resolution on top of everything else: a 512 px input spent 36 s here and a
        // 1024 px one over four minutes. At or below the cap this is 1 and nothing
        // changes, which is every image in the corpus.
        let stride_px = stat_stride(lab.len(), width);

        let px_srgb: Vec<[f32; 3]> = lab.par_iter().map(|&p| oklab_to_rgb(p)).collect();
        let px_lin: Vec<[f32; 3]> = px_srgb
            .par_iter()
            .map(|r| {
                [
                    srgb_to_linear(r[0]),
                    srgb_to_linear(r[1]),
                    srgb_to_linear(r[2]),
                ]
            })
            .collect();
        PixelViews {
            lab,
            px_srgb,
            px_lin,
            width,
            height,
            stride_px,
        }
    }
}

/// The palette candidates: occupied OKLab bins, most populous first.
///
/// OKLab is cut into a 24 × 24 × 24 grid (`L` over `[0, 1]`, `a` and `b` over
/// `[−0.4, 0.4]`, each clamped; bin index `round(x · 23)` per axis). Each occupied bin
/// yields `(count, key, mean)`, where `mean` is the average OKLab colour of the pixels in
/// the bin (summed in `f64`, in pixel order) and `key = L_i · 24² + a_i · 24 + b_i`.
///
/// Sorted by count descending, then key ascending. Sorting by count alone left
/// equal-frequency colours in hash-map order, which differed between runs of the same
/// binary; palette order decides which colour is accepted first and so everything
/// downstream, and the junction accuracy test measured 0.054-0.134 px across ten
/// consecutive runs of one binary. An identical input must give an identical file.
fn frequency_modes(lab: &[Oklab]) -> Vec<(u32, u32, Oklab)> {
    const BINS: usize = 24;
    // Bin keys on every core; the accumulation stays sequential and in pixel order so
    // the centroid sums are bit-identical to the single-threaded version. A dense table
    // over the 24^3 bins replaces the hash map, which was most of this pass's cost.
    let keys: Vec<u32> = lab
        .par_iter()
        .map(|c| {
            let li =
                ((c.l.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u32).min(BINS as u32 - 1);
            let ai = (((c.a + 0.4) / 0.8).clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u32;
            let bi = (((c.b + 0.4) / 0.8).clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u32;
            li * (BINS * BINS) as u32 + ai * BINS as u32 + bi
        })
        .collect();
    let mut dense: Vec<(u32, f64, f64, f64)> = vec![(0, 0.0, 0.0, 0.0); BINS * BINS * BINS];
    for (c, &key) in lab.iter().zip(keys.iter()) {
        let e = &mut dense[key as usize];
        e.0 += 1;
        e.1 += c.l as f64;
        e.2 += c.a as f64;
        e.3 += c.b as f64;
    }
    let mut modes: Vec<(u32, u32, Oklab)> = dense
        .into_iter()
        .enumerate()
        .filter(|(_, e)| e.0 > 0)
        .map(|(key, (n, sl, sa, sb))| {
            let f = n as f64;
            (
                n,
                key as u32,
                Oklab {
                    l: (sl / f) as f32,
                    a: (sa / f) as f32,
                    b: (sb / f) as f32,
                },
            )
        })
        .collect();
    modes.sort_by_key(|&(n, key, _)| (std::cmp::Reverse(n), key));
    modes
}

/// Whether a palette candidate is anti-aliasing (coverage evidence) rather than an ink.
///
/// Three measurements, each computed only when the one before it leaves the verdict open:
///
/// * `blend`: [`blend_pairs`] finds `c` on a chord between two accepted inks, within
///   `merge_distance · 1.6` in OKLab and at least [`BLEND_TMIN`] in from either end;
/// * `interior`: [`interior_fraction`] of the pixels `c` would claim (1 when not a blend);
/// * `straddle`: the largest [`straddle_fraction`] over the blend pairs (0 unless a blend
///   with `interior < BLEND_INTERIOR_FRACTION`).
///
/// The candidate is coverage when all three agree ([`BlendEvidence::is_coverage`]).
struct BlendEvidence {
    blend: bool,
    /// OKLab distance to the nearest qualifying chord; infinite when there is none.
    chord_off: f32,
    interior: f32,
    straddle: f32,
}

impl BlendEvidence {
    /// Measure candidate `c` against the accepted inks `colors`; `nearest_px` is each
    /// pixel's OKLab distance to its nearest accepted ink.
    fn measure(
        view: &PixelViews,
        c: Oklab,
        colors: &[Oklab],
        nearest_px: &[f32],
        merge_distance: f32,
        escaped: bool,
    ) -> Self {
        let pairs = blend_pairs(c, colors, merge_distance * 1.6, BLEND_TMIN);
        let blend = !pairs.is_empty();
        // How far the candidate sits from the nearest chord between two accepted inks.
        // Zero means it lies exactly on the line between them, which in a three-dimensional
        // colour space is not a coincidence -- it is what a blend *is*.
        let chord_off = pairs
            .iter()
            .map(|&(_, _, _, off)| off)
            .fold(f32::INFINITY, f32::min);
        // Measured for an escaped candidate too: the escape rule reads it.
        let interior = if blend || escaped {
            interior_fraction(
                &view.lab,
                view.width,
                view.height,
                c,
                nearest_px,
                view.stride_px,
            )
        } else {
            1.0
        };
        // The pairs on every core: each is a count ratio and a max of finite values does
        // not depend on the order it is taken in.
        let straddle = if blend && interior < BLEND_INTERIOR_FRACTION {
            pairs
                .par_iter()
                .map(|&(i, j, linear, _off)| {
                    straddle_fraction(
                        &view.lab,
                        &view.px_srgb,
                        &view.px_lin,
                        view.width,
                        view.height,
                        c,
                        nearest_px,
                        colors[i],
                        colors[j],
                        linear,
                        view.stride_px,
                    )
                })
                .reduce(|| 0.0f32, f32::max)
        } else {
            0.0
        };
        BlendEvidence {
            blend,
            chord_off,
            interior,
            straddle,
        }
    }

    /// A blend, thin (`interior < BLEND_INTERIOR_FRACTION`) and straddling
    /// (`straddle ≥ BLEND_STRADDLE_FRACTION`): anti-aliasing, not an ink.
    fn is_coverage(&self) -> bool {
        self.blend
            && self.interior < BLEND_INTERIOR_FRACTION
            && self.straddle >= BLEND_STRADDLE_FRACTION
    }
}

/// Move each accepted ink to the mean of the pixels that chose it, and return each ink's
/// share of the image.
///
/// A pixel chooses its nearest ink in OKLab (ties to the lower index) only when that ink
/// is within `merge_distance`; anti-aliased pixels sit far from every entry, so excluding
/// them keeps blends from dragging a palette colour off its true value. An ink no pixel
/// chose keeps its candidate colour and gets weight 0. Weights are `members / total_px`,
/// so they sum to less than 1 when some pixels chose nothing.
fn refine_to_members(
    lab: &[Oklab],
    colors: &mut [Oklab],
    merge_distance: f32,
    total_px: f32,
) -> Vec<f32> {
    let mut acc = vec![(0.0f64, 0.0f64, 0.0f64, 0u32); colors.len()];
    // The nearest-entry search is the cost and runs on every core; the sums are
    // taken in pixel order afterwards so the means are bit-identical.
    let chosen: Vec<u32> = lab
        .par_iter()
        .map(|c| {
            let mut best = (0usize, f32::MAX);
            for (i, &p) in colors.iter().enumerate() {
                let d = c.dist(p);
                if d < best.1 {
                    best = (i, d);
                }
            }
            if best.1 <= merge_distance {
                best.0 as u32
            } else {
                u32::MAX
            }
        })
        .collect();
    for (c, &k) in lab.iter().zip(chosen.iter()) {
        if k != u32::MAX {
            let e = &mut acc[k as usize];
            e.0 += c.l as f64;
            e.1 += c.a as f64;
            e.2 += c.b as f64;
            e.3 += 1;
        }
    }
    let total = total_px;
    let mut weight = Vec::with_capacity(colors.len());
    for (i, e) in acc.iter().enumerate() {
        if e.3 > 0 {
            let f = e.3 as f64;
            colors[i] = Oklab {
                l: (e.0 / f) as f32,
                a: (e.1 / f) as f32,
                b: (e.2 / f) as f32,
            };
        }
        weight.push(e.3 as f32 / total);
    }
    weight
}

/// Assign every pixel to its nearest palette entry.
///
/// `rgb` is sRGB `[0, 1]`; the distance is Euclidean in OKLab ([`Palette::nearest`], ties
/// to the lower index). This is hard nearest-ink labelling: an anti-aliased pixel gets
/// whichever ink is closest, often a third colour, which is what
/// [`crate::regions::absorb_blend_slivers`] later repairs. Returns one `u16` label per
/// pixel, in the same order.
pub fn label_image(rgb: &[[f32; 3]], pal: &Palette) -> Vec<u16> {
    rgb.par_iter()
        .map(|&c| pal.nearest(rgb_to_oklab(c)).0 as u16)
        .collect()
}

// ---------------------------------------------------------------------------------------
// The per-colour rewrite against this reference
// ---------------------------------------------------------------------------------------

/// A small deterministic generator (PCG-style LCG), so the tests need no dependency.
pub(crate) struct Lcg(pub(crate) u64);

impl Lcg {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    pub(crate) fn unit(&mut self) -> f32 {
        self.below(1 << 20) as f32 / (1 << 20) as f32
    }
}

/// A random piece of flat art: a few inks painted as rectangles and discs, anti-aliased
/// edges (a blend of the two inks, in linear light or sRGB), some pixels jittered by a level
/// or two, and a few pure-noise pixels. Colours are 8-bit values over 255, as decoded.
pub(crate) fn random_art(rng: &mut Lcg, w: usize, h: usize) -> Vec<[f32; 3]> {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() / 255.0;
    let k = 2 + rng.below(5) as usize;
    let inks: Vec<[f32; 3]> = (0..k)
        .map(|_| [q(rng.unit()), q(rng.unit()), q(rng.unit())])
        .collect();
    let mut ink_of = vec![0usize; w * h];
    for s in 1..k {
        let (cx, cy) = (rng.below(w as u64) as f32, rng.below(h as u64) as f32);
        let r = 1.0 + rng.unit() * (w.max(h) as f32) * 0.4;
        let disc = rng.below(2) == 0;
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let inside = if disc {
                    dx * dx + dy * dy < r * r
                } else {
                    dx.abs() < r && dy.abs() < r * 0.6
                };
                if inside {
                    ink_of[y * w + x] = s;
                }
            }
        }
    }
    let mut out: Vec<[f32; 3]> = ink_of.iter().map(|&i| inks[i]).collect();
    for y in 0..h {
        for x in 1..w {
            let (a, b) = (ink_of[y * w + x - 1], ink_of[y * w + x]);
            if a != b && rng.below(3) != 0 {
                let t = rng.unit();
                let (ca, cb) = (inks[a], inks[b]);
                let lin = rng.below(2) == 0;
                out[y * w + x] = std::array::from_fn(|c| {
                    if lin {
                        let m = srgb_to_linear(ca[c]) * (1.0 - t) + srgb_to_linear(cb[c]) * t;
                        q(linear_to_srgb(m))
                    } else {
                        q(ca[c] * (1.0 - t) + cb[c] * t)
                    }
                });
            }
        }
    }
    for p in out.iter_mut() {
        match rng.below(40) {
            0 => *p = [q(rng.unit()), q(rng.unit()), q(rng.unit())],
            1..=3 => {
                let j = (rng.below(5) as f32 - 2.0) / 255.0;
                *p = p.map(|v| q(v + j));
            }
            _ => {}
        }
    }
    out
}

/// Bitwise equality of two palettes.
pub(crate) fn same_palette(a: &Palette, b: &Palette) -> bool {
    let bits3 = |v: &[[f32; 3]]| v.iter().map(|c| c.map(f32::to_bits)).collect::<Vec<_>>();
    let lab = |v: &[Oklab]| {
        v.iter()
            .map(|c| [c.l.to_bits(), c.a.to_bits(), c.b.to_bits()])
            .collect::<Vec<_>>()
    };
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    lab(&a.colors) == lab(&b.colors)
        && bits3(&a.rgb) == bits3(&b.rgb)
        && bits(&a.weight) == bits(&b.weight)
        && bits(&a.alpha) == bits(&b.alpha)
}

/// The evidence the pipeline hands the palette, clean or soft.
fn evidence(rng: &mut Lcg, n: usize) -> PaletteEvidence {
    let soft = rng.below(3) == 0;
    PaletteEvidence {
        sigma_noise: if rng.below(4) == 0 {
            0.0
        } else {
            [0.5, 2.0, 6.0][rng.below(3) as usize] / 255.0
        },
        lambda: crate::gradient::bic_lambda(n),
        noise_sigmas: if soft {
            SOFT_NOISE_SIGMAS
        } else {
            NOISE_SIGMAS
        },
        same_ink_de00: if soft {
            SOFT_SAME_INK_DE00
        } else {
            SAME_INK_DE00
        },
    }
}

#[test]
fn the_per_colour_palette_and_labels_equal_the_per_pixel_ones() {
    let mut rng = Lcg(0x5eed);
    for case in 0..160 {
        let (w, h) = match case % 16 {
            0 => (1, 1),
            1 => (1, 1 + rng.below(40) as usize),
            2 => (300 + rng.below(40) as usize, 230),
            _ => (2 + rng.below(60) as usize, 2 + rng.below(60) as usize),
        };
        let rgb = random_art(&mut rng, w, h);
        let ev = evidence(&mut rng, w * h);
        let max_colors = [1, 3, 64][rng.below(3) as usize];
        let old = extract_palette_mdl(&rgb, w, h, DEFAULT_MERGE_DISTANCE, max_colors, ev);
        let new = super::extract_palette_mdl(&rgb, w, h, DEFAULT_MERGE_DISTANCE, max_colors, ev);
        assert!(
            same_palette(&old, &new),
            "case {case}: {w}x{h} palettes differ"
        );
        assert_eq!(
            label_image(&rgb, &old),
            super::label_image(&rgb, &new),
            "case {case}"
        );
    }
}

#[test]
fn the_rewrite_matches_on_degenerate_shapes() {
    let mut rng = Lcg(99);
    let ev = PaletteEvidence {
        sigma_noise: 0.002,
        lambda: 3.0,
        noise_sigmas: SOFT_NOISE_SIGMAS,
        same_ink_de00: SAME_INK_DE00,
    };
    let art = random_art(&mut rng, 9, 7);
    // Empty; a buffer shorter than the grid; one longer than it; a zero width.
    for (rgb, w, h) in [
        (vec![], 0, 0),
        (art[..40].to_vec(), 9, 7),
        (art.clone(), 5, 7),
        (art.clone(), 0, 7),
        (art.clone(), 63, 0),
    ] {
        let old = extract_palette_mdl(&rgb, w, h, DEFAULT_MERGE_DISTANCE, 64, ev);
        let new = super::extract_palette_mdl(&rgb, w, h, DEFAULT_MERGE_DISTANCE, 64, ev);
        assert!(
            same_palette(&old, &new),
            "{w}x{h} over {} pixels",
            rgb.len()
        );
    }
}
