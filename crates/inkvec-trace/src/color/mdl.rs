//! The palette walk of [`super::extract_palette_mdl`], evaluated per distinct colour.
//!
//! The decisions are exactly those documented on `extract_palette_mdl`; what changes is the
//! unit of work. Every per-pixel quantity the walk reads is a function of the pixel's colour,
//! so it is computed once per colour ([`ClassicView`]), and the spatial tests read pixel
//! positions through the colour's list of visited pixels ([`DistinctImage`]). See
//! [`super::distinct`] for the literature this follows (Celebi 2011's unique-colour reduction,
//! Swain and Ballard 1991's backprojection, Korn and Muthukrishnan 2000's influence sets).
//!
//! Two things are kept pixel by pixel on purpose: the candidate means and the refined ink
//! means are summed in pixel order, because those sums become the inks and a sum taken per
//! colour is not provably the same `f64`.

use rayon::prelude::*;

use super::distinct::{side_of, Claim, ColourIds, DistinctImage, Neighbourhoods};
use super::{
    de00, oklab_to_rgb, rgb_to_oklab, same_ink_as_accepted, srgb_to_linear, to_hex, Oklab, Palette,
    PaletteEvidence, BLEND_INTERIOR_FRACTION, BLEND_STRADDLE_FRACTION, BLEND_TMIN, JND_FLOOR,
    MIN_INK_WEIGHT, PARAMS_PER_INK, STRADDLE_STEP,
};

/// Distinct colours per parallel task in the per-colour conversions.
const PAR_MIN_COLOURS: usize = 1024;

/// Every distinct colour in the three spaces the palette's tests read, converted once.
///
/// `srgb` is converted *from `lab`* and `lin` from `srgb`, exactly as the per-pixel caches
/// were, so the blend axis positions are bit-identical to what they were.
pub(crate) struct ClassicView<'a> {
    /// The ids, their visited pixels and the geometry.
    pub(crate) img: DistinctImage<'a>,
    /// Each colour in OKLab.
    pub(crate) lab: Vec<Oklab>,
    /// Each colour back in sRGB `[0, 1]`, from `lab`.
    srgb: Vec<[f32; 3]>,
    /// Each colour in linear light, from `srgb`.
    lin: Vec<[f32; 3]>,
}

/// sRGB to linear light, per channel.
fn linear3(r: [f32; 3]) -> [f32; 3] {
    [
        srgb_to_linear(r[0]),
        srgb_to_linear(r[1]),
        srgb_to_linear(r[2]),
    ]
}

impl<'a> ClassicView<'a> {
    /// Convert the distinct colours of `rgb` (sRGB `[0, 1]`, numbered by `ids`).
    pub(crate) fn new(rgb: &[[f32; 3]], ids: &'a ColourIds, width: usize, height: usize) -> Self {
        let lab: Vec<Oklab> = ids
            .reps
            .par_iter()
            .with_min_len(PAR_MIN_COLOURS)
            .map(|&i| rgb_to_oklab(rgb[i]))
            .collect();
        Self::from_lab(lab, ids, width, height)
    }

    /// The view of colours already in OKLab (`lab[d]` is colour `d`).
    pub(crate) fn from_lab(
        lab: Vec<Oklab>,
        ids: &'a ColourIds,
        width: usize,
        height: usize,
    ) -> Self {
        let srgb: Vec<[f32; 3]> = lab
            .par_iter()
            .with_min_len(PAR_MIN_COLOURS)
            .map(|&p| oklab_to_rgb(p))
            .collect();
        let lin: Vec<[f32; 3]> = srgb
            .par_iter()
            .with_min_len(PAR_MIN_COLOURS)
            .map(|&r| linear3(r))
            .collect();
        ClassicView {
            img: DistinctImage::new(ids, width, height),
            lab,
            srgb,
            lin,
        }
    }
}

/// Each accepted ink in the two blend spaces, converted once when it is accepted rather than
/// once per candidate pair.
#[derive(Default)]
pub(crate) struct InkAxes {
    /// Linear light.
    pub(crate) lin: Vec<[f32; 3]>,
    /// sRGB.
    pub(crate) srgb: Vec<[f32; 3]>,
}

impl InkAxes {
    /// The inks `accepted`, converted.
    #[cfg(test)]
    pub(crate) fn of(accepted: &[Oklab]) -> Self {
        let mut axes = InkAxes::default();
        for &a in accepted {
            axes.push(a);
        }
        axes
    }

    /// Convert and append one ink.
    pub(crate) fn push(&mut self, ink: Oklab) {
        let r = oklab_to_rgb(ink);
        self.srgb.push(r);
        self.lin.push(linear3(r));
    }
}

/// [`super::extract_palette_mdl`] on a prepared view. See there for every decision.
pub(crate) fn extract(
    view: &ClassicView,
    merge_distance: f32,
    max_colors: usize,
    ev: PaletteEvidence,
) -> Palette {
    let modes = frequency_modes(view);
    let colours = view.img.colours();
    let total_px = view.img.pixels().max(1) as f32;
    let mut walk = Walk {
        view,
        ev,
        merge_distance,
        total_px,
        colors: Vec::new(),
        axes: InkAxes::default(),
        nearest: vec![f32::INFINITY; colours],
        claim: Claim::new(colours),
        scratch: vec![0u8; colours + 1],
        paldbg: inkvec_core::env::flag("INKVEC_PALDBG"),
        // The perceptual-merge experiment, in a `research` build only.
        de00_radius: if cfg!(feature = "research") {
            inkvec_core::env::number("INKVEC_MERGE_DE00").map(|v| v as f32)
        } else {
            None
        },
    };
    for (tested, &(n, _key, c)) in modes.iter().enumerate() {
        if walk.colors.len() >= max_colors {
            break;
        }
        // Each candidate is a pass over the colours; most are turned down.
        inkvec_core::progress::step("colour candidates", tested as u64, modes.len() as u64);
        if walk.accepts(n, c) {
            walk.accept(c);
        }
    }
    let mut colors = walk.colors;
    if colors.is_empty() {
        colors.push(modes.first().map(|m| m.2).unwrap_or(Oklab {
            l: 1.0,
            a: 0.0,
            b: 0.0,
        }));
    }
    let weight = refine_to_members(view, &mut colors, merge_distance, total_px);
    let rgb_out: Vec<[f32; 3]> = colors.iter().map(|&c| oklab_to_rgb(c)).collect();
    let alpha = vec![1.0; colors.len()];
    Palette {
        colors,
        rgb: rgb_out,
        weight,
        alpha,
    }
}

/// The walk's state between candidates.
struct Walk<'v, 'a> {
    view: &'v ClassicView<'a>,
    ev: PaletteEvidence,
    merge_distance: f32,
    total_px: f32,
    /// Accepted inks, in acceptance order.
    colors: Vec<Oklab>,
    /// The same inks in the blend spaces.
    axes: InkAxes,
    /// Per colour, the OKLab distance to the nearest accepted ink.
    nearest: Vec<f32>,
    /// The current candidate's claimed colours.
    claim: Claim,
    /// One byte per colour for the straddle test.
    scratch: Vec<u8>,
    paldbg: bool,
    de00_radius: Option<f32>,
}

impl Walk<'_, '_> {
    /// Whether candidate `c` (from a bin of `n` pixels) passes every gate.
    fn accepts(&mut self, n: u32, c: Oklab) -> bool {
        let PaletteEvidence {
            sigma_noise,
            lambda,
            noise_sigmas,
            same_ink_de00,
        } = self.ev;
        let view = self.view;
        let claim = view
            .img
            .claim(&mut self.claim, &self.nearest, |d| view.lab[d].dist(c));
        // The spread only matters through `noise_sigmas * spread`; with the guard off that
        // is zero whatever the spread is, so it is not measured.
        let spread = if noise_sigmas == 0.0 {
            0.0
        } else {
            view.img.spread(&self.claim, self.merge_distance)
        };
        if (claim as f32 / self.total_px) < MIN_INK_WEIGHT && !self.colors.is_empty() {
            return false;
        }
        let nearest = self
            .colors
            .iter()
            .map(|&p| p.dist(c))
            .fold(f32::INFINITY, f32::min);
        let reach = noise_sigmas * spread;
        if same_ink_as_accepted(c, &self.colors, same_ink_de00, self.paldbg) {
            return false;
        }
        let merged = if let Some(rad) = self.de00_radius {
            let d = self
                .colors
                .iter()
                .map(|&p| de00(oklab_to_rgb(c), oklab_to_rgb(p)))
                .fold(f32::INFINITY, f32::min);
            d <= rad || nearest <= reach
        } else {
            nearest <= self.merge_distance.max(reach)
        };
        if merged {
            let worth_it = sigma_noise > 0.0
                && nearest > JND_FLOOR
                && nearest > reach
                && 0.5 * (claim as f64) * ((nearest as f64 / sigma_noise).powi(2))
                    > lambda * PARAMS_PER_INK;
            if !worth_it {
                return false;
            }
        }
        // From here on `merged` means "inside the merge radius and kept only by the escape".
        let shape = BlendEvidence::measure(self, c, merged);
        if self.paldbg {
            eprintln!(
                "  cand {:<9} bin={:<6} claim={:<6} w={:.4} sig={:.5} reach={:.4} near={:.4} blend={} chord={:.4} interior={:.3} straddle={:.3} escaped={}",
                to_hex(oklab_to_rgb(c)),
                n,
                claim,
                claim as f32 / self.total_px,
                sigma_noise,
                reach,
                nearest,
                shape.blend,
                if shape.blend { shape.chord_off } else { f32::NAN },
                shape.interior,
                shape.straddle,
                merged
            );
        }
        !shape.is_thin_escape() && !shape.is_coverage()
    }

    /// Accept `c`: lower every colour's nearest-ink distance and record the ink.
    fn accept(&mut self, c: Oklab) {
        let lab = &self.view.lab;
        if lab.len() >= super::distinct::PAR_COLOURS {
            self.nearest
                .par_iter_mut()
                .zip(lab.par_iter())
                .for_each(|(d, &q)| *d = d.min(q.dist(c)));
        } else {
            for (d, &q) in self.nearest.iter_mut().zip(lab) {
                *d = d.min(q.dist(c));
            }
        }
        self.axes.push(c);
        self.colors.push(c);
    }
}

/// Whether a candidate is anti-aliasing rather than an ink: a blend of two accepted inks,
/// thin (`interior < BLEND_INTERIOR_FRACTION`) and straddling
/// (`straddle >= BLEND_STRADDLE_FRACTION`); or a thin non-blend that only the
/// description-length escape admitted ([`BlendEvidence::is_thin_escape`]). Each
/// measurement is taken only when the one before it leaves the verdict open.
struct BlendEvidence {
    blend: bool,
    /// Inside the merge radius of an accepted ink, kept so far only by the MDL escape.
    escaped: bool,
    /// OKLab distance to the nearest qualifying chord; infinite when there is none.
    chord_off: f32,
    /// Share of the claimed pixels that are interior; 1.0 (not measured) unless the
    /// candidate is a blend or escaped.
    interior: f32,
    straddle: f32,
}

impl BlendEvidence {
    /// Measure candidate `c` against the walk's accepted inks and its current claim.
    /// `escaped`: `c` lies inside the merge radius of an accepted ink and passed the MDL
    /// escape, so its interior is measured even when it is not a blend (the escape rule
    /// reads it). A blend is measured exactly as before the rule: interior, then the
    /// straddle when thin. A candidate that is neither a blend nor escaped is not
    /// measured at all, as before.
    fn measure(walk: &mut Walk, c: Oklab, escaped: bool) -> Self {
        let pairs = super::blend_pairs_cached(c, &walk.axes, walk.merge_distance * 1.6, BLEND_TMIN);
        let blend = !pairs.is_empty();
        let chord_off = pairs
            .iter()
            .map(|&(_, _, _, off)| off)
            .fold(f32::INFINITY, f32::min);
        let img = &walk.view.img;
        let interior = if blend || escaped {
            img.interior(&walk.claim)
        } else {
            1.0
        };
        let straddle = if blend && interior < BLEND_INTERIOR_FRACTION {
            let hoods = img.neighbourhoods(&walk.claim);
            pairs
                .iter()
                .map(|&(i, j, linear, _)| {
                    straddle(
                        walk.view,
                        &hoods,
                        c,
                        (i, j),
                        &walk.axes,
                        linear,
                        &mut walk.scratch,
                    )
                })
                .fold(0.0f32, f32::max)
        } else {
            0.0
        };
        BlendEvidence {
            blend,
            escaped,
            chord_off,
            interior,
            straddle,
        }
    }

    /// A blend, thin and straddling: anti-aliasing, not an ink.
    fn is_coverage(&self) -> bool {
        self.blend
            && self.interior < BLEND_INTERIOR_FRACTION
            && self.straddle >= BLEND_STRADDLE_FRACTION
    }

    /// Admitted inside the merge radius by the description-length escape, not a blend, and
    /// thin: an edge artefact of an accepted ink, not an ink of its own. See
    /// [`super::escape_needs_interior`] for the rule, the case and the literature.
    fn is_thin_escape(&self) -> bool {
        super::escape_needs_interior(self.escaped, self.blend, self.interior)
    }
}

/// What fraction of the pixels `c` claims sit between a pixel nearer ink `a` and one nearer
/// ink `b` (inks `pair` of `axes`) along their axis in linear light (`linear`) or sRGB.
/// See [`super::BLEND_STRADDLE_FRACTION`] for why: anti-aliasing is a ramp, so each of its
/// pixels has a neighbour nearer each ink, while a thin band of ink has pixels touching one
/// ink and pixels touching the other and none touching both.
///
/// `t(p) = ((p − A) · (B − A)) / |B − A|²`; a claimed pixel straddles when its 3x3
/// neighbourhood has a pixel with `t < t_c − step_lo` and one with `t > t_c + step_hi`,
/// `step_lo = max(min(STRADDLE_STEP, t_c / 2), 0.02)` and `step_hi` likewise on the other
/// side. 0 without a full grid or for a degenerate axis (`|B − A|² < 1e-9`); 1 when `c`
/// claims nothing.
fn straddle(
    view: &ClassicView,
    hoods: &Neighbourhoods,
    c: Oklab,
    pair: (usize, usize),
    axes: &InkAxes,
    linear: bool,
    scratch: &mut [u8],
) -> f32 {
    if !hoods.has_geometry() {
        return 0.0;
    }
    let inks = if linear { &axes.lin } else { &axes.srgb };
    let (pa, pb) = (inks[pair.0], inks[pair.1]);
    let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
    let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if dd < 1e-9 {
        return 0.0;
    }
    let t_of =
        |p: [f32; 3]| ((p[0] - pa[0]) * d[0] + (p[1] - pa[1]) * d[1] + (p[2] - pa[2]) * d[2]) / dd;
    let pc = oklab_to_rgb(c);
    let tc = t_of(if linear { linear3(pc) } else { pc });
    // A candidate near one end of the axis has that ink within less than a full step:
    // the far side is judged by the step, the near side by half the room that is left.
    let step_lo = STRADDLE_STEP.min(0.5 * tc).max(0.02);
    let step_hi = STRADDLE_STEP.min(0.5 * (1.0 - tc)).max(0.02);
    let (lo, hi) = (tc - step_lo, tc + step_hi);
    let px = if linear { &view.lin } else { &view.srgb };
    hoods.straddle(|k| side_of(t_of(px[k]), lo, hi), scratch)
}

/// The palette candidates: occupied cells of a 24³ grid over OKLab (`L` over `[0, 1]`, `a`
/// and `b` over `[−0.4, 0.4]`, clamped; cell `round(x · 23)` per axis), each with its pixel
/// count and the mean colour of its pixels (summed in `f64`, in pixel order), sorted by count
/// descending then cell key ascending. The cell is computed per colour; the sums walk the
/// pixels in order, so the means are bit-identical to a per-pixel pass.
pub(crate) fn frequency_modes(view: &ClassicView) -> Vec<(u32, u32, Oklab)> {
    const BINS: usize = 24;
    let key: Vec<u32> = view
        .lab
        .iter()
        .map(|c| {
            let li =
                ((c.l.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u32).min(BINS as u32 - 1);
            let ai = (((c.a + 0.4) / 0.8).clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u32;
            let bi = (((c.b + 0.4) / 0.8).clamp(0.0, 1.0) * (BINS - 1) as f32).round() as u32;
            li * (BINS * BINS) as u32 + ai * BINS as u32 + bi
        })
        .collect();
    let mut dense: Vec<(u32, f64, f64, f64)> = vec![(0, 0.0, 0.0, 0.0); BINS * BINS * BINS];
    for &id in view.img.cid {
        let c = view.lab[id as usize];
        let e = &mut dense[key[id as usize] as usize];
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

/// Index of the nearest of `inks` to `c` within `merge_distance` (ties to the lower index),
/// or `u32::MAX` when the nearest is further.
fn member_of(c: Oklab, inks: &[Oklab], merge_distance: f32) -> u32 {
    let mut best = (0usize, f32::MAX);
    for (i, &p) in inks.iter().enumerate() {
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
}

/// Move each ink to the mean of the pixels that chose it (nearest ink within
/// `merge_distance`), and return each ink's share of the image. The choice is made per
/// colour; the means are summed over the pixels in order.
fn refine_to_members(
    view: &ClassicView,
    colors: &mut [Oklab],
    merge_distance: f32,
    total_px: f32,
) -> Vec<f32> {
    let chosen: Vec<u32> = view
        .lab
        .par_iter()
        .with_min_len(PAR_MIN_COLOURS)
        .map(|&c| member_of(c, colors, merge_distance))
        .collect();
    let mut acc = vec![(0.0f64, 0.0f64, 0.0f64, 0u32); colors.len()];
    for &id in view.img.cid {
        let k = chosen[id as usize];
        if k != u32::MAX {
            let c = view.lab[id as usize];
            let e = &mut acc[k as usize];
            e.0 += c.l as f64;
            e.1 += c.a as f64;
            e.2 += c.b as f64;
            e.3 += 1;
        }
    }
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
        weight.push(e.3 as f32 / total_px);
    }
    weight
}

/// Every pixel's nearest palette entry in OKLab (ties to the lower index), found once per
/// distinct colour and read back through the pixel's id.
pub(crate) fn label(rgb: &[[f32; 3]], ids: &ColourIds, pal: &Palette) -> Vec<u16> {
    let per_colour: Vec<u16> = ids
        .reps
        .par_iter()
        .with_min_len(PAR_MIN_COLOURS)
        .map(|&i| pal.nearest(rgb_to_oklab(rgb[i])).0 as u16)
        .collect();
    ids.cid
        .par_iter()
        .with_min_len(1 << 16)
        .map(|&d| per_colour[d as usize])
        .collect()
}
#[cfg(test)]
pub(crate) use adapters::{claim_spread, interior_fraction, straddle_fraction};

/// The per-pixel entry points the palette's unit tests were written against, answered by
/// the per-colour code. Colour ids are keyed on every per-pixel input, so a test may pass
/// caches or nearest-ink distances that are not functions of the colour.
#[cfg(test)]
mod adapters {
    use super::*;

    /// Pixels numbered by their OKLab colour, their cached sRGB and linear values (zero when
    /// absent) and their nearest-ink distance.
    fn ids_of(lab: &[Oklab], srgb: &[[f32; 3]], lin: &[[f32; 3]], nearest: &[f32]) -> ColourIds {
        let n = lab.len().min(nearest.len());
        let bits = |v: Option<&[f32; 3]>| v.map_or([0; 3], |v| v.map(f32::to_bits));
        ColourIds::build(n, |i| {
            (
                [lab[i].l.to_bits(), lab[i].a.to_bits(), lab[i].b.to_bits()],
                bits(srgb.get(i)),
                bits(lin.get(i)),
                nearest[i].to_bits(),
            )
        })
    }

    /// The old `straddle_fraction(lab, px_srgb, px_lin, width, height, c, nearest, a, b,
    /// linear, stride_px)`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn straddle_fraction(
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
        let ids = ids_of(lab, px_srgb, px_lin, nearest);
        let view = ClassicView {
            img: DistinctImage::with_stride(&ids, width, height, stride_px),
            lab: ids.reps.iter().map(|&i| lab[i]).collect(),
            srgb: ids.reps.iter().map(|&i| px_srgb[i]).collect(),
            lin: ids.reps.iter().map(|&i| px_lin[i]).collect(),
        };
        let near: Vec<f32> = ids.reps.iter().map(|&i| nearest[i]).collect();
        let mut claim = Claim::new(ids.len());
        view.img.claim(&mut claim, &near, |d| view.lab[d].dist(c));
        let hoods = view.img.neighbourhoods(&claim);
        let mut scratch = vec![0u8; ids.len() + 1];
        straddle(
            &view,
            &hoods,
            c,
            (0, 1),
            &InkAxes::of(&[a, b]),
            linear,
            &mut scratch,
        )
    }

    /// The old `interior_fraction(lab, width, height, c, nearest, stride_px)`.
    pub(crate) fn interior_fraction(
        lab: &[Oklab],
        width: usize,
        height: usize,
        c: Oklab,
        nearest: &[f32],
        stride_px: usize,
    ) -> f32 {
        let ids = ids_of(lab, &[], &[], nearest);
        let img = DistinctImage::with_stride(&ids, width, height, stride_px);
        let (labs, near): (Vec<Oklab>, Vec<f32>) =
            ids.reps.iter().map(|&i| (lab[i], nearest[i])).unzip();
        let mut claim = Claim::new(ids.len());
        img.claim(&mut claim, &near, |d| labs[d].dist(c));
        img.interior(&claim)
    }

    /// The old `claim_spread(lab, nearest_px, c, tol, stride_px)`: the claim in pixels and
    /// the members' median distance.
    pub(crate) fn claim_spread(
        lab: &[Oklab],
        nearest_px: &[f32],
        c: Oklab,
        tol: f32,
        stride_px: usize,
    ) -> (usize, f32) {
        let ids = ids_of(lab, &[], &[], nearest_px);
        let img = DistinctImage::with_stride(&ids, lab.len(), 1, stride_px);
        let (labs, near): (Vec<Oklab>, Vec<f32>) =
            ids.reps.iter().map(|&i| (lab[i], nearest_px[i])).unzip();
        let mut claim = Claim::new(ids.len());
        let n = img.claim(&mut claim, &near, |d| labs[d].dist(c));
        (n, img.spread(&claim, tol))
    }
}
