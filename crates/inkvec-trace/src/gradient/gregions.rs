//! Research prototype A10 "gregions": recovering the artist's gradient *regions*, behind
//! `INKVEC_GREGIONS` in a `research` build. Off, and in every release build, nothing here
//! runs and the trace is byte for byte the one without it.
//!
//! # The problem, as the round-2 gradient research defined it
//!
//! A gradient region in the artwork reaches the band merger ([`super::bands`]) as a stack
//! of palette bands, and the merger asks two questions of it: *which pixels form one
//! region* and *which fill explains them*. On the 168 corpus icons whose artist SVG has
//! gradients, v0.2.4 recovers 42 % of the artist's visible gradients as one of ours and
//! paints 30 % flat. Handing our pipeline the artist's own regions lifts that to 73 %
//! (the segmentation oracle), so the region question is the main lever; the fill question
//! is the rest. Four parts address them, each switched separately for ablation:
//!
//! 1. **`profile`: a centre search scored under the artist's kind of profile.** The radial
//!    and elliptic fitters ([`super::fit`]) choose their geometry by the residual of a
//!    *straight* colour line in the gradient coordinate `t`, but 73 % of the artist's
//!    gradients are clamped (their stops do not span 0..1: a flat core and a ramp at the
//!    rim, or a bump on a pad), and a straight-line score pulls the centre to wherever a
//!    straight ramp fits best, not to the ramp's real centre. The search is run a second
//!    time scored by a continuous piecewise-linear profile (variable projection with a
//!    fixed hat basis), by Levenberg–Marquardt ([`super::fit::profile_geometries`]); the
//!    results are *extra* candidates, so model selection still decides.
//! 2. **`guard`: a step-like profile is an edge, not shading** ([`step_like`]). A flexible
//!    profile can mimic a step, and the round-2 report read the princess emoji's +0.152
//!    dE00 (held_a) as a spline-geometry gradient fusing face and hair that way. A
//!    candidate whose profile changes by a visible amount over less than [`EDGE_WIDTH_PX`]
//!    is refused, exactly as one below the contrast floor is.
//! 3. **`cover`: a union must win on common pixels.** Measured here, the princess loss is
//!    not a step: the crown's lower band, a 139 px component with no evidence of its own
//!    (every pixel a blend, so its flat fit is priced on all of them, cost 62672), bought a
//!    spline-geometry union with the hair (cost 67 alone, union chi² 79251) because the
//!    band merger's legacy gain `cost(a) + cost(b) − cost(a ∪ b)` adds costs measured on
//!    three different pixel populations. With `cover`, a gradient union must also show a
//!    positive gain when both alternatives are priced on the union's interior evidence
//!    (`common_pixel_gain`, which the merger already uses for smooth seams); the legacy
//!    gain still ranks the pairs that pass. Measured on the princess, neither this nor the
//!    step-profile test changes the result (the union wins on common pixels too, because
//!    the band's own flat fit misfits its pixels worse); part 4 is what fixes it.
//! 4. **`seam`: no union across a discontinuity.** What did not merge the band into the
//!    hair before the spline candidates was only that no gradient fitted the union; the
//!    two are separated by an edge, steps of 20+ OKLab units per pixel along their whole
//!    seam. Chakraborty et al. (§3.2) never put two segments that face each other across
//!    the discontinuity map into one region; with `seam` the band merger does not fit a
//!    union of two components of at least `MIN_GRADIENT_PIXELS` pixels each whose seam is
//!    mostly discontinuity ([`sharp_seams`], the threshold [`TAU_D`]). Bands of a
//!    quantised ramp are crossed in small steps and never meet it; a fleck under 16 px is
//!    exempt, so anti-aliasing remnants are still absorbed as before.
//!
//! The prototype's parts 5 and 6, `segments` (smooth-segment region proposals,
//! Chakraborty et al. §3.2, ported from acda7ac) and `mdl` (the proposals accepted by MDL
//! gain), were removed in Wave B: on top of the four parts above they gave no demonstrable
//! gain on the regression gate against v0.2.5 (quality-128ss dE00 −0.76 % against −1.08 %
//! without them, 9 icons worse against 4; quality-512ss −2.85 % against −2.71 % and
//! quality-512ssop −1.92 % against −1.43 %, both inside the gate's intervals), 34 of the 168
//! gradient icons worse against 19, and their multicut never cut as ported (the A10 report,
//! F2). They are in the history at e1bfcb5 (branch `impl2/gregions`).
//!
//! # The switch
//!
//! `INKVEC_GREGIONS=1` (or `on`, `all`) turns on all four parts; a comma-separated list
//! of part names (`profile,guard,cover,seam`) turns on those only; unset,
//! empty or `0` is off. It is read once per process ([`inkvec_core::env`]) and only in a build with
//! the `research` feature: the engine reads experiments' variables there only.
//!
//! # Literature
//!
//! * Inspired by: S. Chakraborty et al. (2025), Image Vectorization via Gradient
//!   Reconstruction, Computer Graphics Forum 44(2), doi:10.1111/cgf.70055 -- §3.2, the
//!   rule that segments facing each other across the discontinuity map are never one
//!   region (part 4); §3.3 (geometry from the gradient field, independent of the profile)
//!   inspired part 1.
//! * Method from: G. H. Golub, V. Pereyra (1973), The differentiation of pseudo-inverses
//!   and nonlinear least squares problems whose variables separate, SIAM J. Numer. Anal.
//!   10(2), doi:10.1137/0710036 -- variable projection: the profile is solved in closed
//!   form for each candidate geometry and only the geometry is searched (part 1).
//! * See also: M. Lukáč et al., US 12,340,441 B2 (2025), Reconstructing concentric radial
//!   gradients -- a profile-agnostic centre from the orthogonality of the colour gradient
//!   and the position vector, which is what [`super::fit`]'s gradient-line seed already is.

use super::FillModel;

/// Which parts of the prototype are on; see the module docs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Parts {
    /// Part 1: the extra spline-scored radial and elliptic candidates.
    pub(crate) profile: bool,
    /// Part 2: refuse candidates whose profile is an edge ([`step_like`]).
    pub(crate) guard: bool,
    /// Part 3: a gradient union must also gain on common pixels in the band merger.
    pub(crate) cover: bool,
    /// Part 4: no union across a seam that is mostly discontinuity ([`sharp_seams`]).
    pub(crate) seam: bool,
}

/// The parts switched on for this process: `INKVEC_GREGIONS`, read once, in a `research`
/// build only; all off otherwise.
pub(crate) fn parts() -> Parts {
    static P: std::sync::OnceLock<Parts> = std::sync::OnceLock::new();
    *P.get_or_init(|| {
        if cfg!(feature = "research") {
            parse(inkvec_core::env::text("INKVEC_GREGIONS"))
        } else {
            Parts::default()
        }
    })
}

/// The rule behind [`parts`], separated from the cached read so it can be tested.
///
/// Unset, empty (after trimming) or `0`: everything off. `1`, `on` or `all`: everything
/// on. Otherwise a comma-separated list of part names, each turning that part on; unknown
/// words are ignored, so a typo turns nothing on rather than everything.
fn parse(v: Option<&str>) -> Parts {
    let v = v.map(str::trim).unwrap_or("");
    match v {
        "" | "0" => Parts::default(),
        "1" | "on" | "all" => Parts {
            profile: true,
            guard: true,
            cover: true,
            seam: true,
        },
        list => {
            let mut p = Parts::default();
            for word in list.split(',').map(str::trim) {
                match word {
                    "profile" => p.profile = true,
                    "guard" => p.guard = true,
                    "cover" => p.cover = true,
                    "seam" => p.seam = true,
                    _ => {}
                }
            }
            p
        }
    }
}

/// Colour step to a 4-neighbour, OKLab × 100 (about CIELAB units), above which a pixel
/// pair is a discontinuity: Chakraborty et al.'s `τ_d` for the discontinuity map `D`, with
/// acda7ac's value for 128 px icons (the paper's 10 is for 512–2048 px images; the round-2
/// research found 4 no better). Not tuned.
pub(crate) const TAU_D: f32 = 6.0;

/// Narrowest stretch of a profile piece, px, that still reads as shading rather than as an
/// edge.
///
/// An anti-aliased edge crosses from one colour to the other within one to one and a half
/// pixels (a box-filtered step at a sub-pixel position covers at most two pixels, and the
/// middle of that span carries most of the change); shading spreads its change over many.
/// The round-2 report proposed "about 1.5 px along t" for the guard; it is that value,
/// not tuned. Not from the literature: the published gradient vectorisers place stops by
/// a 1-D Mumford-Shah cut (Chakraborty et al. 2025, eq. 5) and never meet a step inside a
/// region, because their regions are cut at discontinuities first.
pub(crate) const EDGE_WIDTH_PX: f64 = 1.5;

/// Pixels over which the coordinate `t` of `model` advances by 1 where it advances
/// fastest: `1 / max_x |∇t(x)|`, inside the ramp (outside it `t` is padded and constant).
///
/// For a linear gradient `t = ((P − p0)·d) / |d|²` with `d = p1 − p0`, so `|∇t| = 1/|d|`
/// everywhere and the width is the axis length `|d|`. For a radial gradient
/// `t = √(u² + (v·k)²) / r` in the ellipse's frame (`k` the aspect, see
/// [`super::eval::radial_t_rot`]); `|∇t|` is `1/r` along the `r` semi-axis and `k/r` across
/// it, so the narrowest width is `r / max(k, 1)` -- `r` for a circle. `None` for a flat
/// fill, or a degenerate axis or radius (no ramp to measure).
fn unit_width(model: &FillModel) -> Option<f64> {
    let w = match *model {
        FillModel::Flat(_) => return None,
        FillModel::Linear { p0, p1, .. } => ((p1.0 - p0.0).powi(2) + (p1.1 - p0.1).powi(2)).sqrt(),
        FillModel::Radial { r, aspect, .. } => r / aspect.max(1.0),
    };
    (w.is_finite() && w > 0.0).then_some(w)
}

/// Whether a gradient's colour profile contains an edge: some piece between two adjacent
/// stops changes by at least `min_contrast` (largest sRGB channel difference of its two
/// stop colours, 0..1) over fewer than [`EDGE_WIDTH_PX`] pixels.
///
/// The stops are `(0, c0)`, the interior `mids`, and `(1, c1)`; piece `k` runs from offset
/// `o_k` to `o_{k+1}` and is `(o_{k+1} − o_k) · W` pixels wide at its narrowest, `W` being
/// [`unit_width`]. `min_contrast` is the caller's visible-contrast floor
/// (`max(3σ, 1.5/255)` in `fit_samples`), so a piece that changes by less than the noise
/// can be as narrow as it likes. A flat fill is never step-like.
///
/// Why this is the right test: the band merger already prices a step against two flats
/// (the ramp-or-step test in `fit_samples`), but a gradient with two interior stops a
/// pixel apart *is* a step with shading on both sides, and wins that comparison while
/// drawing an edge where the artist has one -- the face-and-hair fusion. A profile that
/// changes a visible amount within the width of an anti-aliased edge is describing that
/// edge, which the boundary geometry owns.
///
/// Complexity O(stops). Ties: a piece exactly [`EDGE_WIDTH_PX`] wide is shading.
pub(crate) fn step_like(model: &FillModel, min_contrast: f64) -> bool {
    let Some(width) = unit_width(model) else {
        return false;
    };
    let (c0, c1, mids) = match model {
        FillModel::Flat(_) => return false,
        FillModel::Linear { c0, c1, mids, .. } | FillModel::Radial { c0, c1, mids, .. } => {
            (*c0, *c1, mids)
        }
    };
    let mut prev = (0.0f64, c0);
    for &(off, col) in mids.iter().chain(std::iter::once(&(1.0, c1))) {
        let span_px = (off - prev.0).max(0.0) * width;
        let delta = (0..3)
            .map(|k| (col[k] - prev.1[k]).abs() as f64)
            .fold(0.0, f64::max);
        if span_px < EDGE_WIDTH_PX && delta >= min_contrast {
            return true;
        }
        prev = (off, col);
    }
    false
}

/// Per pair of touching components, how many of the 4-neighbour pixel pairs across their
/// seam step by more than the discontinuity threshold [`TAU_D`] (OKLab × 100):
/// `sharp[a][b]` counts the pairs `(p, q)`, `p` in `a` and `q` in `b`, with
/// `|lab(p) − lab(q)| > τ_d` (symmetric; pairs that never step that much are absent).
/// Compared with the seam's full length (`adj[a][b]`, every pair across it), it says
/// whether the seam is an edge: a pixel pair stepping above `τ_d` is exactly what puts a
/// pixel in Chakraborty et al.'s discontinuity map `D`.
///
/// `comp` is the component of each pixel, `n_comp` the number of components. One pass
/// over the image, rows then columns (no `%` per pixel). Inspired by: Chakraborty et al.
/// 2025, doi:10.1111/cgf.70055, §3.2; the band merger already counts the *smooth* pairs of
/// each seam the same way (`regions::smooth_step`).
pub(crate) fn sharp_seams(
    comp: &[u32],
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    n_comp: usize,
) -> Vec<std::collections::HashMap<u32, u32>> {
    let lab = |c: [f32; 3]| {
        let o = crate::color::rgb_to_oklab(c);
        [o.l * 100.0, o.a * 100.0, o.b * 100.0]
    };
    let tau2 = TAU_D * TAU_D;
    let mut sharp = vec![std::collections::HashMap::new(); n_comp];
    let count = |p: usize, q: usize, sharp: &mut Vec<std::collections::HashMap<u32, u32>>| {
        let (a, b) = (comp[p], comp[q]);
        if a == b {
            return;
        }
        let (lp, lq) = (lab(rgb[p]), lab(rgb[q]));
        let d2 = (lp[0] - lq[0]).powi(2) + (lp[1] - lq[1]).powi(2) + (lp[2] - lq[2]).powi(2);
        if d2 > tau2 {
            *sharp[a as usize].entry(b).or_insert(0) += 1;
            *sharp[b as usize].entry(a).or_insert(0) += 1;
        }
    };
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            if x + 1 < w {
                count(p, p + 1, &mut sharp);
            }
            if y + 1 < h {
                count(p, p + w, &mut sharp);
            }
        }
    }
    sharp
}

#[cfg(test)]
mod tests {
    use super::super::Interp;
    use super::*;

    #[test]
    fn the_switch_reads_all_none_or_a_list() {
        let all = Parts {
            profile: true,
            guard: true,
            cover: true,
            seam: true,
        };
        assert_eq!(parse(None), Parts::default());
        assert_eq!(parse(Some("")), Parts::default());
        assert_eq!(parse(Some(" 0 ")), Parts::default());
        assert_eq!(parse(Some("1")), all);
        assert_eq!(parse(Some("all")), all);
        assert_eq!(
            parse(Some("profile, guard")),
            Parts {
                profile: true,
                guard: true,
                ..Parts::default()
            }
        );
        assert_eq!(
            parse(Some("segmnts")),
            Parts::default(),
            "a typo turns nothing on"
        );
    }

    #[test]
    fn a_seam_counts_only_its_sharp_pixel_pairs() {
        // Three columns of components 0 | 1 | 2 on a 6x2 image: 0 and 1 differ by a hard
        // step, 1 and 2 by a step far below the threshold.
        let (w, h) = (6, 2);
        let comp: Vec<u32> = (0..w * h).map(|p| ((p % w) / 2) as u32).collect();
        let rgb: Vec<[f32; 3]> = comp
            .iter()
            .map(|&c| match c {
                0 => [0.1, 0.1, 0.1],
                1 => [0.8, 0.6, 0.2],
                _ => [0.81, 0.6, 0.2],
            })
            .collect();
        let sharp = sharp_seams(&comp, &rgb, w, h, 3);
        assert_eq!(sharp[0].get(&1), Some(&2), "both rows of the 0|1 seam");
        assert_eq!(sharp[1].get(&0), Some(&2), "symmetric");
        assert_eq!(sharp[1].get(&2), None, "a gentle seam is not an edge");
    }

    fn radial(r: f64, aspect: f64, mids: Vec<(f64, [f32; 3])>) -> FillModel {
        FillModel::Radial {
            c: (10.0, 10.0),
            r,
            c0: [0.2; 3],
            c1: [0.8; 3],
            interp: Interp::Srgb,
            aspect,
            angle: 0.3,
            mids,
        }
    }

    #[test]
    fn a_step_between_close_stops_is_an_edge_and_shading_is_not() {
        let floor = 1.5 / 255.0;
        // A plain ramp over 20 px of radius: shading.
        assert!(!step_like(&radial(20.0, 1.0, vec![]), floor));
        // Two interior stops 0.02 apart on a 40 px radius (0.8 px) carrying the whole
        // change: the face-and-hair step.
        let step = vec![(0.60, [0.2; 3]), (0.62, [0.8; 3])];
        assert!(step_like(&radial(40.0, 1.0, step.clone()), floor));
        // The same profile on an ellipse is narrower across: still an edge.
        assert!(step_like(&radial(40.0, 2.0, step), floor));
        // Close stops whose colours agree change nothing over that piece.
        let calm = vec![(0.60, [0.5; 3]), (0.62, [0.5; 3])];
        assert!(!step_like(&radial(40.0, 1.0, calm), floor));
        // A whole ramp narrower than an edge (r / aspect = 1.25 px) is an edge.
        assert!(step_like(&radial(2.5, 2.0, vec![]), floor));
        // Exactly 1.5 px is shading (the tie rule).
        assert!(!step_like(&radial(3.0, 2.0, vec![]), floor));
        assert!(!step_like(&FillModel::Flat([0.1; 3]), floor));
    }

    #[test]
    fn a_linear_ramp_is_measured_along_its_axis() {
        let lin = |len: f64| FillModel::Linear {
            p0: (0.0, 0.0),
            p1: (len * 0.6, len * 0.8),
            c0: [0.0; 3],
            c1: [1.0; 3],
            interp: Interp::LinearRgb,
            mids: vec![],
        };
        assert!(step_like(&lin(1.0), 1.5 / 255.0));
        assert!(!step_like(&lin(10.0), 1.5 / 255.0));
        // Degenerate axis: nothing to measure, not an edge.
        assert!(!step_like(&lin(0.0), 1.5 / 255.0));
    }
}
