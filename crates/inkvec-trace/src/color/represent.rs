//! Representation: a rare candidate is an ink when enough of its pixels are colours that no
//! mixture of the inks around them explains.
//!
//! # The problem
//!
//! The palette walks (`color/mdl.rs`, `native/palette.rs`) used to drop every candidate
//! claiming less than [`super::MIN_INK_WEIGHT`] (0.4 %) of the image: 65.5 px at 128², over
//! a thousand at 512². The share is what keeps anti-aliasing out, because a blend colour
//! is individually rare. It also drops small inks that are nothing like a blend. The
//! r2-palette research (2026-10-02, 362 icons of screen + held_a at 128 px) found 44 thick
//! artist inks merged away, 75 % of them under that share (area quartiles 20 / 32 / 47 px),
//! at a median 7.5 dE00 from the ink that painted them instead: pupils, a red mouth, a
//! yellow star. With the artist's palette supplied those merges fell from 34 to 2 on the
//! icons without gradients. Lowering the share alone (0.001) recovered a third of the
//! oracle's gain, -1.8 % dE00 on the union and -1.0 % out of sample, at the price of extra
//! invented inks, because it also lets rare blends through.
//!
//! # The method
//!
//! A pixel votes for a new ink only when the inks already accepted cannot explain it, which
//! is Aksoy et al.'s rule for growing a colour model: a pixel already explained as a mixture
//! of the current colours gets no vote. Here, for a candidate the share test would drop:
//!
//! 1. for each pixel it claims (`DistinctImage::claimed_pixels`, the visited pixels on the
//!    grid), the inks around it: the accepted ink nearest to each of its eight neighbours that
//!    the candidate does not itself claim, the [`MIX_INKS`] most frequent (ties to the lower
//!    ink index), plus the clear ground on the two-ground walk;
//! 2. its residual against them: the distance to the nearest convex mixture of two or three
//!    of them (`native::mixture`, the same model `regions::absorb_blend_slivers` and
//!    `reassign_blend_pixels` use to tell anti-aliasing from ink), or to the single ink when
//!    only one is around, or infinite when none is (a pixel surrounded by the candidate);
//! 3. the pixel votes when the residual exceeds `max(3 σ, 0.025)` (sRGB units, the
//!    absorption stages' tolerance, [`mixture_tolerance`]) *and* it is not resampling
//!    overshoot of those inks either ([`overshoot_residual`]: up to [`OVERSHOOT`], 15 %,
//!    beyond the end of a chord between two of them, or one of them scaled by up to 1.15).
//!    Without that exception the rule admitted the ringing rims of a Lanczos upscale as
//!    small inks: on the resampled screen set (`bench/resampled_eval.py`, ring2x) invented
//!    fills rose 12.8 % and dE00 0.88 %, worst `openmoji/1F1FF-1F1F2` +0.26;
//! 4. the candidate is represented when its votes, scaled by the visiting stride, reach
//!    `max(MIN_VOTES, VOTE_SHARE · n)`: 8 px at 128², the same share at other sizes
//!    ([`represented`]).
//!
//! A represented candidate is not admitted by that alone: it then meets every other gate
//! (the same-ink floor, the merge radius and its escape, the blend, straddle and escape
//! interior tests), exactly as a common candidate does. A candidate above the share is
//! unaffected, and so is the walks' first ink and the two-ground walk's first visible ink
//! (Wave A's `rarity_exempt`, kept as it is: it decides before any of this runs).
//!
//! # Adapted, and why
//!
//! * Aksoy et al. vote with a soft weight `e^{-c_p} (1 − e^{-r_p})` over 10³ RGB bins, with
//!   `r_p` the pixel's unmixing energy under the *whole* model and `c_p` its image gradient,
//!   and re-solve the unmixing after each new colour. Here the model is local, the inks
//!   around the pixel (Yang et al.'s two-colour edge model widened to three), because an
//!   icon's palette mixes only where two inks touch; the vote is binary against the noise;
//!   and there is no gradient weight, because a pixel on an edge between accepted inks is
//!   already explained by their mixture, which is what the gradient weight approximates.
//! * The residual is measured in sRGB (and the six two-ground sRGB coordinates), not in
//!   linear light: renderers and resamplers blend the encoded values, the absorption stages
//!   measure there, and linear light compresses the dark end so far that a dark ink beside
//!   black (a brown pupil on a black outline) would sit within 0.025 of the black-to-skin
//!   chord and lose its vote.
//! * The vote floor is in pixels, as VTracer's `good_min_area` is, but scaled with the
//!   image like the share it replaces, so a 512 px trace of the same drawing asks the same
//!   evidence of a pupil as a 128 px one.
//!
//! # The frame: the greedy walk with its perceptual floor, not the two-level palette
//!
//! Two other admission rules exist. [`super::SAME_INK_DE00`] (shipped) is a perceptual
//! floor: below 1.5 dE00 a candidate is the same ink whatever any count says. The two-level
//! palette (`e6fe352`, experiment branch, opt-in, never merged) replaces candidate
//! *generation*: level one seeds inks from pixels with same-colour neighbours on both axes,
//! level two runs the old walk seeded with them. This module keeps the walk and the floor and
//! changes only what a rare candidate must show, because (a) the floor answers a different
//! question (can anyone see the difference) that no count or mixture can answer, and it is
//! applied before this test; (b) the two-level palette's seeds are a spatial same-colour test,
//! which the interior and escape tests already make inside the walk, and its measured gain was
//! on gradient icons, where the research's ramp oracle shows the palette is not the lever
//! (+0.009 dE00 with the artist's ramp colours supplied); (c) one walk keeps the opaque and
//! two-ground forks in step, which the two-level palette did for the opaque path only.
//!
//! # Literature
//!
//! Method from: Y. Aksoy, T. O. Aydın, A. Smolić, M. Pollefeys (2017), "Unmixing-Based Soft
//! Color Segmentation for Image Manipulation", *ACM TOG* 36(2):19, doi:10.1145/3002176,
//! section 5 (colour model estimation: votes only from pixels the current model does not
//! explain). Adapted as listed above.
//! Inspired by: L. Yang, P. V. Sander, J. Lawrence, H. Hoppe (2011), "Antialiasing
//! Recovery", *ACM TOG* 30(3), doi:10.1145/1966394.1966401: an edge pixel as a mixture of
//! the extreme colours of its neighbourhood, which is where the local inks come from.
//! See also: A. Delong, A. Osokin, H. N. Isack, Y. Boykov (2012), "Fast Approximate Energy
//! Minimization with Label Costs", *IJCV* 96(1):1-27, doi:10.1007/s11263-011-0437-z, the
//! joint palette-and-labels objective with a cost per label used, which this greedy walk
//! approximates; VTracer / visioncortex `color_clusters` (`good_min_area`, an area floor in
//! pixels, competitor practice).

use super::distinct::{Claim, DistinctImage};

/// Most inks around a pixel that its mixture is drawn from (before the clear ground).
pub(crate) const MIX_INKS: usize = 4;

/// Fewest votes, in pixels, that make a rare candidate represented, at any image size.
pub(crate) const MIN_VOTES: f32 = 8.0;

/// Votes as a share of the image that make a rare candidate represented: `MIN_VOTES` at
/// 128² (8 / 16384, 0.049 %). The research measured the share alone at 0.001 and 0.0005
/// with the same result (union -0.0025 and -0.0027 dE00), so the floor sits at the lower.
pub(crate) const VOTE_SHARE: f32 = MIN_VOTES / 16384.0;

/// The residual, in sRGB units, above which a pixel is not explained by the inks around it:
/// `max(3 σ, 0.025)`, the tolerance of `regions::absorb_blend_slivers` and
/// `reassign_blend_pixels`, so a pixel those stages would hand back to its neighbours as
/// anti-aliasing does not vote here either. `sigma_noise` is per-channel sRGB noise.
pub(crate) fn mixture_tolerance(sigma_noise: f64) -> f32 {
    (3.0 * sigma_noise).max(0.025) as f32
}

/// Whether `votes` (in pixels) make a candidate represented in an image of `total_px`
/// pixels: `votes ≥ max(MIN_VOTES, VOTE_SHARE · total_px)`.
pub(crate) fn represented(votes: usize, total_px: f32) -> bool {
    votes as f32 >= MIN_VOTES.max(VOTE_SHARE * total_px)
}

/// The claimed pixels no mixture of the inks around them explains, in pixels (the count of
/// claimed visited pixels that vote, times the visiting stride). See the module docs.
///
/// * `img`, `claim`: the image index and the candidate's claimed colours;
/// * `nearest_ink[d]`: per colour id, the index of its nearest accepted ink (`u32::MAX` when
///   none is accepted, which is never the case here: the first ink skips this test);
/// * `value(d)`: colour `d` in the space the residual is measured in (sRGB over white on
///   the opaque walk, the six two-ground coordinates on the native one);
/// * `inks`: the accepted inks in the same space; `ground`: an extra mixture end that is
///   always around (the clear ground on the native walk), or `None`;
/// * `tol`: [`mixture_tolerance`].
///
/// A pixel votes when both the mixture residual and [`overshoot_residual`] exceed `tol`.
///
/// Cost: per claimed visited pixel, eight neighbour reads, one mixture over at most five
/// inks (ten pairs, ten triples) and, for a pixel the mixture leaves unexplained, the
/// overshoot residual (ten extended pairs, five scalings). A rare candidate claims under
/// 0.4 % of the visited pixels,
/// so this is bounded by `0.004 · min(n, STAT_PIXELS)` pixels per candidate. A pixel on the
/// picture edge simply has fewer neighbours; the outside is not an ink.
pub(crate) fn unexplained<const N: usize>(
    img: &DistinctImage,
    claim: &Claim,
    nearest_ink: &[u32],
    value: impl Fn(usize) -> [f32; N],
    inks: &[[f32; N]],
    ground: Option<[f32; N]>,
    tol: f32,
) -> usize {
    let (w, h) = (img.width, img.height);
    let mut votes = 0usize;
    let mut cols: Vec<[f32; N]> = Vec::with_capacity(MIX_INKS + 1);
    for i in img.claimed_pixels(claim) {
        let (x, y) = (i % w, i / w);
        // (ink, count) of the accepted inks nearest to the unclaimed neighbours; at most 8.
        let mut around: [(u32, u8); 8] = [(u32::MAX, 0); 8];
        let mut k = 0usize;
        for (dx, dy) in [
            (-1i64, -1i64),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ] {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                continue;
            }
            let d = img.cid[ny as usize * w + nx as usize] as usize;
            if claim.claimed[d] {
                continue;
            }
            let ink = nearest_ink[d];
            if ink == u32::MAX || ink as usize >= inks.len() {
                continue;
            }
            match around[..k].iter_mut().find(|e| e.0 == ink) {
                Some(e) => e.1 += 1,
                None => {
                    around[k] = (ink, 1);
                    k += 1;
                }
            }
        }
        // Most frequent first, ties to the lower ink index.
        let near = &mut around[..k];
        near.sort_unstable_by_key(|&(ink, n)| (std::cmp::Reverse(n), ink));
        cols.clear();
        cols.extend(
            near.iter()
                .take(MIX_INKS)
                .map(|&(ink, _)| inks[ink as usize]),
        );
        if let Some(g) = ground {
            cols.push(g);
        }
        let v = value(img.cid[i] as usize);
        let r = match cols.len() {
            0 => f32::INFINITY,
            1 => dist(v, cols[0]),
            _ => crate::native::mixture(v, &cols).map_or(f32::INFINITY, |(r, _)| r),
        };
        if r > tol && overshoot_residual(v, &cols, ground.is_some()) > tol {
            votes += 1;
        }
    }
    votes * img.stride_px
}

/// How far, as a share of the step, resampling overshoots an edge: [`overshoot_residual`].
///
/// Measured 2026-10-03 with Pillow's `LANCZOS` at 2x (the ring2x set's filter): a hard grey
/// step overshoots by 10.0-10.5 % of the step on either side (64/192, 100/150, 30/220), an
/// anti-aliased one by 3.7-4.0 %, and an opaque gold edge on a clear ground, resized
/// premultiplied, leaves rim pixels at up to 1.108 times the gold. 0.15 covers all three with
/// room for a sharper kernel, and is far below the separation of two inks an artist chose.
pub(crate) const OVERSHOOT: f32 = 0.15;

/// Euclidean distance in `N` channels.
fn dist<const N: usize>(a: [f32; N], b: [f32; N]) -> f32 {
    let mut s = 0.0f32;
    for c in 0..N {
        let e = a[c] - b[c];
        s += e * e;
    }
    s.sqrt()
}

/// The residual of `v` as resampling overshoot of the inks around it: the smaller of
///
/// * the distance to a chord between two of `cols` extended beyond either end by up to
///   [`OVERSHOOT`] of its length (`t ∈ [−OVERSHOOT, 1 + OVERSHOOT]`, `q = a + t (b − a)`):
///   ringing at an edge between two opaque inks, which a resampler applied to the encoded
///   values overshoots along their chord;
/// * the distance to an ink `s` of `cols` scaled by `k ∈ [1, 1 + OVERSHOOT]`
///   (`k = clamp(v·s / s·s)`): ringing at an edge between an opaque ink and the clear
///   ground, resized premultiplied, which leaves an opaque rim at `k · s` (the crest's
///   1.067). The last of `cols` is skipped when `has_ground` (it is the ground).
///
/// Infinite with no inks. O(|cols|²) with `|cols| ≤ 5`.
///
/// Not from the literature: an exception for overshoot, because the two-colour edge model
/// this stands on (Yang et al. 2011, `cols` as the neighbourhood's extremes) takes an
/// overshoot pixel for an extreme itself; the measured sizes are on [`OVERSHOOT`]. See also
/// Pillow `Image.resize` (RGBA resized premultiplied) and "Lanczos resampling", Wikipedia,
/// Limitations (ringing), the two sources the r2-palette research traced the crest rim to.
pub(crate) fn overshoot_residual<const N: usize>(
    v: [f32; N],
    cols: &[[f32; N]],
    has_ground: bool,
) -> f32 {
    let mut best = f32::INFINITY;
    for i in 0..cols.len() {
        for j in i + 1..cols.len() {
            let (a, b) = (cols[i], cols[j]);
            let mut uu = 0.0f32;
            let mut wu = 0.0f32;
            for c in 0..N {
                let u = b[c] - a[c];
                uu += u * u;
                wu += (v[c] - a[c]) * u;
            }
            if uu < 1e-12 {
                continue;
            }
            let t = (wu / uu).clamp(-OVERSHOOT, 1.0 + OVERSHOOT);
            let mut q = a;
            for c in 0..N {
                q[c] += (b[c] - a[c]) * t;
            }
            best = best.min(dist(v, q));
        }
    }
    let inks = if has_ground {
        &cols[..cols.len().saturating_sub(1)]
    } else {
        cols
    };
    for &s in inks {
        let mut ss = 0.0f32;
        let mut vs = 0.0f32;
        for c in 0..N {
            ss += s[c] * s[c];
            vs += v[c] * s[c];
        }
        if ss < 1e-12 {
            continue;
        }
        let k = (vs / ss).clamp(1.0, 1.0 + OVERSHOOT);
        let mut q = s;
        for c in q.iter_mut() {
            *c *= k;
        }
        best = best.min(dist(v, q));
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::distinct::ColourIds;

    /// The index of an image of the given sRGB colours, each id's colour, and the claim of
    /// every colour equal to `cand`.
    fn setup(px: &[[f32; 3]], cand: [f32; 3]) -> (ColourIds, Vec<[f32; 3]>, Claim) {
        let ids = ColourIds::of_rgb(px);
        let colours: Vec<[f32; 3]> = ids.reps.iter().map(|&i| px[i]).collect();
        let mut claim = Claim::new(ids.len());
        for (d, c) in colours.iter().enumerate() {
            claim.claimed[d] = *c == cand;
        }
        (ids, colours, claim)
    }

    const RED: [f32; 3] = [0.9, 0.1, 0.1];
    const BLUE: [f32; 3] = [0.1, 0.2, 0.9];
    const GREEN: [f32; 3] = [0.1, 0.8, 0.2];

    /// Nearest of `inks` to each colour (Euclidean in sRGB; enough for these fixtures).
    fn nearest(colours: &[[f32; 3]], inks: &[[f32; 3]]) -> Vec<u32> {
        colours
            .iter()
            .map(|c| {
                (0..inks.len())
                    .min_by(|&a, &b| {
                        let da: f32 = (0..3).map(|k| (c[k] - inks[a][k]).powi(2)).sum();
                        let db: f32 = (0..3).map(|k| (c[k] - inks[b][k]).powi(2)).sum();
                        da.partial_cmp(&db).unwrap()
                    })
                    .unwrap() as u32
            })
            .collect()
    }

    #[test]
    fn a_distinct_blob_votes_and_a_blend_between_its_neighbours_does_not() {
        // Red left half, blue right half, a 3x3 green blob inside the red, and a column of
        // the red-blue mid colour along the seam.
        let (w, h) = (16, 12);
        let mid = [0.5, 0.15, 0.5];
        let px: Vec<[f32; 3]> = (0..w * h)
            .map(|p| {
                let (x, y) = (p % w, p / w);
                if (3..6).contains(&x) && (4..7).contains(&y) {
                    GREEN
                } else if x == 8 {
                    mid
                } else if x < 8 {
                    RED
                } else {
                    BLUE
                }
            })
            .collect();
        let inks = [RED, BLUE];
        let value = |cols: &Vec<[f32; 3]>| {
            let cols = cols.clone();
            move |d: usize| cols[d]
        };
        let tol = mixture_tolerance(0.5 / 255.0);
        // The green blob: every pixel votes (no mixture of red reaches green).
        let (ids, colours, claim) = setup(&px, GREEN);
        let img = DistinctImage::new(&ids, w, h);
        let near = nearest(&colours, &inks);
        assert_eq!(
            unexplained(&img, &claim, &near, value(&colours), &inks, None, tol),
            9
        );
        // The seam: every pixel is the red-blue mixture of its neighbours.
        let (ids, colours, claim) = setup(&px, mid);
        let img = DistinctImage::new(&ids, w, h);
        let near = nearest(&colours, &inks);
        assert_eq!(
            unexplained(&img, &claim, &near, value(&colours), &inks, None, tol),
            0
        );
    }

    #[test]
    fn overshoot_beyond_a_chord_or_a_scaled_rim_is_explained_and_a_new_colour_is_not() {
        let gold = [176.0 / 255.0, 138.0 / 255.0, 74.0 / 255.0];
        let white = [1.0f32; 3];
        let tol = mixture_tolerance(0.5 / 255.0);
        // The crest's rim: the gold scaled by 1.067 (premultiplied ringing), clear ground
        // (white) as the ground entry. The plain mixture misses it; the exception does not.
        let rim = gold.map(|v| v * 1.067);
        let cols = [gold, white];
        let mix = crate::native::mixture(rim, &cols).unwrap().0;
        assert!(mix > tol, "{mix}");
        assert!(overshoot_residual(rim, &cols, true) <= tol);
        // Ringing between two opaque inks: 10 % beyond red, away from blue.
        let ring: [f32; 3] = std::array::from_fn(|c| RED[c] - 0.1 * (BLUE[c] - RED[c]));
        assert!(overshoot_residual(ring, &[RED, BLUE], false) <= tol);
        // 40 % beyond is not ringing, and green is nobody's overshoot.
        let far: [f32; 3] = std::array::from_fn(|c| RED[c] - 0.4 * (BLUE[c] - RED[c]));
        assert!(overshoot_residual(far, &[RED, BLUE], false) > tol);
        assert!(overshoot_residual(GREEN, &[RED, BLUE, white], true) > tol);
        // The ground is never scaled, and no inks explain nothing.
        assert!(overshoot_residual([0.9; 3], &[white], true) > tol);
        assert_eq!(overshoot_residual(GREEN, &[], false), f32::INFINITY);
    }

    #[test]
    fn the_floor_is_eight_pixels_at_128_and_scales_with_the_image() {
        assert!(represented(8, 128.0 * 128.0));
        assert!(!represented(7, 128.0 * 128.0));
        assert!(!represented(7, 40.0 * 40.0), "never under eight pixels");
        assert!(represented(128, 512.0 * 512.0));
        assert!(!represented(127, 512.0 * 512.0));
        assert!((mixture_tolerance(0.0) - 0.025).abs() < 1e-7);
        assert!((mixture_tolerance(0.02) - 0.06).abs() < 1e-6);
    }
}
