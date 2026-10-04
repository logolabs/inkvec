//! Scoring a fill model against a region's samples: chi², visible contrast and ramp support,
//! and the test ([`imperceptible`]) that refuses a gradient the emitter would paint flat.
//!
//! [`super::fit_samples`] scores every candidate three ways -- the contrast it draws, the
//! share of the region it shades, and its chi² -- and all three read the model's
//! prediction at the same strided subsample (stride `max(n / fit_cap(), 1)`, indices
//! `0, stride, 2·stride, ..`). Each used to evaluate the model on its own, so every
//! candidate was evaluated three times at every scored sample, and a linear-light model
//! pays three `powf` per evaluation. [`Predictions`] evaluates it once and the three
//! scores read the buffer.
//!
//! Not from the literature: a plain common-subexpression hoist, because the three scores
//! were written separately. Every score is formed by the same expressions on the same
//! values in the same order as before, so each is bit for bit what it was (the tests below
//! hold that against the one-pass-per-score forms).

use super::{fit_cap, FillModel, Samples, QUANT_HALF_STEP};

/// A model's predicted sRGB colour at each sample of the strided scoring subsample.
pub(super) struct Predictions {
    /// Prediction at subsample `j`, which is sample `j · stride`.
    p: Vec<[f32; 3]>,
    /// The subsample stride, `max(n / fit_cap(), 1)`.
    stride: usize,
}

impl Predictions {
    /// Evaluate `model` at every `stride`-th sample of `s`, starting at the first.
    pub(super) fn new(model: &FillModel, s: &Samples) -> Self {
        let stride = (s.len() / fit_cap()).max(1);
        let e = model.eval();
        let p = (0..s.len())
            .step_by(stride)
            .map(|i| e.color_at(s.x[i], s.y[i]))
            .collect();
        Predictions { p, stride }
    }

    /// chi² of a model against the samples: per channel in sRGB, the part of the residual
    /// beyond the half-LSB quantisation dead zone, in units of `sigma`.
    ///
    /// `chi² = (n/m) · Σ_i Σ_ch max(|o_i,ch − p_ch(x_i, y_i)| − ½LSB, 0)² / σ²`
    ///
    /// where `o` is the observed sRGB colour, `p` the model's prediction ([`FillModel::eval`], held in `self`),
    /// `½LSB = 0.5/255` ([`QUANT_HALF_STEP`]), and the sum runs over a strided subsample of
    /// `m` of the `n` samples (at most [`fit_cap`]). The `n/m` factor restores the full
    /// count so a large region's chi² is comparable with its parameter cost and with smaller
    /// regions. The dead zone is the exact likelihood of an 8-bit-rounded Gaussian
    /// observation, as the module docs of `gradient` explain. No samples gives 0.
    pub(super) fn chi2(&self, s: &Samples, sigma: f64) -> f64 {
        let inv = 1.0 / (sigma * sigma);
        let n = s.len();
        let mut sum = 0.0;
        for (j, p) in self.p.iter().enumerate() {
            for (obs, pred) in s.srgb[j * self.stride].iter().zip(p.iter()) {
                let d = ((obs - pred).abs() as f64 - QUANT_HALF_STEP).max(0.0);
                sum += d * d;
            }
        }
        let used = self.p.len();
        if used == 0 {
            return 0.0;
        }
        // Scaled back to the full pixel count so the MDL cost stays comparable with
        // parameter costs and with fits of other regions.
        sum * inv * (n as f64 / used as f64)
    }

    /// Share of the predictions a quarter of `contrast` or more away from `mean`: see
    /// [`ramp_support`].
    pub(super) fn support(&self, mean: [f32; 3], contrast: f64) -> f64 {
        let thr = (0.25 * contrast) as f32;
        let thr2 = thr * thr;
        let k = self
            .p
            .iter()
            .filter(|p| {
                let d = [p[0] - mean[0], p[1] - mean[1], p[2] - mean[2]];
                d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > thr2
            })
            .count();
        if self.p.is_empty() {
            0.0
        } else {
            k as f64 / self.p.len() as f64
        }
    }

    /// Largest per-channel range of the predictions: see [`visible_contrast`].
    pub(super) fn contrast(&self) -> f64 {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &self.p {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (0..3).map(|k| (hi[k] - lo[k]) as f64).fold(0.0, f64::max)
    }
}

/// Fraction of the samples at which `model` predicts a colour at least a quarter of its
/// own contrast away from the region's mean colour — how much of the region the ramp
/// actually shades. Half for a linear ramp; near zero for a ramp that is flat except at
/// one end.
///
/// `support = #{ i : |p(x_i, y_i) − mean| > contrast/4 } / m`, Euclidean distance in
/// sRGB, over a strided subsample of `m` samples. `mean` is the flat fit's colour
/// (a per-channel median), `contrast` comes from [`visible_contrast`]. Compared with
/// `MIN_RAMP_SUPPORT`. No samples gives 0.
pub(super) fn ramp_support(model: &FillModel, s: &Samples, mean: [f32; 3], contrast: f64) -> f64 {
    Predictions::new(model, s).support(mean, contrast)
}

/// Largest per-channel sRGB range the model spans over the samples:
/// `max_ch (max_i p_ch − min_i p_ch)` over a strided subsample, where `p` is the model's
/// prediction at each sample position. It measures the gradient the model *draws* on
/// this region, not the data's range, so a ramp whose visible part is below the noise
/// can be refused (see `min_contrast` in `fit_samples`). A flat model gives 0; so does
/// an empty sample set, whose sentinel range is negative and loses to the fold's 0.
pub(super) fn visible_contrast(model: &FillModel, s: &Samples) -> f64 {
    Predictions::new(model, s).contrast()
}

// ---------------------------------------------------------------------------------------
// What the emitter will draw
// ---------------------------------------------------------------------------------------

/// Largest OKLab distance between any two stops of a gradient that the emitter still
/// paints flat (`demote_imperceptible_gradient` in `inkvec-cli/src/pipeline/demote.rs`,
/// whose `JND` this is: "a conservative multiple of a just-noticeable difference in
/// OKLab"). The two must stay equal; the emitter can call [`imperceptible`] instead of
/// keeping its own copy.
pub const IMPERCEPTIBLE_STOP_OKLAB: f32 = 0.02;

/// Whether the emitter would paint `model` flat: it is a gradient and every pair of its
/// stops (the two ends and every interior one) lies closer than
/// [`IMPERCEPTIBLE_STOP_OKLAB`] in OKLab. A flat fill is not.
///
/// `fit_samples` refuses such a candidate, as it refuses one below the contrast floor,
/// because model selection must price what is drawn: an imperceptible gradient is emitted
/// as the flat midpoint of its end stops, not as the gradient whose residual won the
/// selection, and not as the region's own best flat colour (the per-channel median the flat
/// candidate carries). Measured on `noto-emoji/emoji_u1f36a` at 512 px (2026-10-03): the
/// profile-aware candidates (`fit::profile_geometries`) fitted the cookie's body, a flat
/// core with a two-level ramp, by a radial whose stops all lie within 0.02 of each other
/// (chi² 252 over 48,313 px); the emitter painted it #f09c56, and the icon went from
/// dE00 0.087 to 0.216. Refusing the candidate leaves the body flat in the merger, which
/// then joins it to the ring around it into one gradient the emitter keeps (0.126). On
/// the regression gate, with the profile-aware candidates, quality-512ssop dE00 against
/// v0.2.5 went from −1.62 % to −2.85 % and quality-512ss from −2.45 % (inside its interval)
/// to −1.99 % ("better").
///
/// Not from the literature: a consistency rule between this crate's model selection and
/// its emitter. Complexity O(stops²), at most four stops.
pub fn imperceptible(model: &FillModel) -> bool {
    let (c0, c1, mids) = match model {
        FillModel::Flat(_) => return false,
        FillModel::Linear { c0, c1, mids, .. } | FillModel::Radial { c0, c1, mids, .. } => {
            (*c0, *c1, mids)
        }
    };
    let stops: Vec<_> = [c0, c1]
        .into_iter()
        .chain(mids.iter().map(|&(_, c)| c))
        .map(crate::color::rgb_to_oklab)
        .collect();
    !stops.iter().enumerate().any(|(i, a)| {
        stops[i + 1..]
            .iter()
            .any(|b| a.dist(*b) >= IMPERCEPTIBLE_STOP_OKLAB)
    })
}

#[cfg(test)]
mod tests {
    use super::super::{to_lin, Interp};
    use super::*;

    /// The one-pass-per-score forms these replace, verbatim.
    fn chi2_ref(model: &FillModel, s: &Samples, sigma: f64) -> f64 {
        let inv = 1.0 / (sigma * sigma);
        let n = s.len();
        let stride = (n / fit_cap()).max(1);
        let mut sum = 0.0;
        let mut used = 0usize;
        let mut i = 0;
        let model = model.eval();
        while i < n {
            let p = model.color_at(s.x[i], s.y[i]);
            for (obs, pred) in s.srgb[i].iter().zip(p.iter()) {
                let d = ((obs - pred).abs() as f64 - QUANT_HALF_STEP).max(0.0);
                sum += d * d;
            }
            used += 1;
            i += stride;
        }
        if used == 0 {
            return 0.0;
        }
        sum * inv * (n as f64 / used as f64)
    }

    fn support_ref(model: &FillModel, s: &Samples, mean: [f32; 3], contrast: f64) -> f64 {
        let thr = (0.25 * contrast) as f32;
        let thr2 = thr * thr;
        let stride = (s.len() / fit_cap()).max(1);
        let (mut n, mut k) = (0usize, 0usize);
        let model = model.eval();
        for i in (0..s.len()).step_by(stride) {
            let p = model.color_at(s.x[i], s.y[i]);
            let d = [p[0] - mean[0], p[1] - mean[1], p[2] - mean[2]];
            n += 1;
            if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > thr2 {
                k += 1;
            }
        }
        if n == 0 {
            0.0
        } else {
            k as f64 / n as f64
        }
    }

    fn contrast_ref(model: &FillModel, s: &Samples) -> f64 {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        let stride = (s.len() / fit_cap()).max(1);
        let model = model.eval();
        for i in (0..s.len()).step_by(stride) {
            let p = model.color_at(s.x[i], s.y[i]);
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (0..3).map(|k| (hi[k] - lo[k]) as f64).fold(0.0, f64::max)
    }

    /// A small deterministic generator, uniform in `[0, 1)`.
    fn lcg(state: &mut u64) -> f64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (*state >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `n` samples scattered over a 64 px square with 8-bit colours.
    fn samples(n: usize, st: &mut u64) -> Samples {
        let mut s = Samples {
            px: vec![],
            x: vec![],
            y: vec![],
            srgb: vec![],
            lin: vec![],
        };
        for i in 0..n {
            let q = |st: &mut u64| (lcg(st) * 255.0).round() as f32 / 255.0;
            let c = [q(st), q(st), q(st)];
            s.px.push(i);
            s.x.push((64.0 * lcg(st)).floor());
            s.y.push((64.0 * lcg(st)).floor());
            s.lin.push(to_lin(c));
            s.srgb.push(c);
        }
        s
    }

    fn model(case: usize, st: &mut u64) -> FillModel {
        let col = |st: &mut u64| [lcg(st) as f32, lcg(st) as f32, lcg(st) as f32];
        let interp = if case.is_multiple_of(2) {
            Interp::LinearRgb
        } else {
            Interp::Srgb
        };
        let mids: Vec<(f64, [f32; 3])> = if case.is_multiple_of(3) {
            vec![(0.3 + 0.4 * lcg(st), col(st))]
        } else {
            Vec::new()
        };
        match (case / 2) % 4 {
            0 => FillModel::Flat(col(st)),
            1 => FillModel::Linear {
                p0: (4.0, 6.0),
                p1: (60.0 * lcg(st), 50.0),
                c0: col(st),
                c1: col(st),
                interp,
                mids,
            },
            k => FillModel::Radial {
                c: (30.0, 28.0),
                r: 10.0 + 30.0 * lcg(st),
                c0: col(st),
                c1: col(st),
                interp,
                aspect: if k == 2 { 1.0 } else { 1.2 + lcg(st) },
                angle: 3.0 * lcg(st),
                mids,
            },
        }
    }

    #[test]
    fn shared_predictions_score_bit_for_bit_like_one_pass_per_score() {
        let mut st = 5u64;
        // Sizes on both sides of the stride threshold (fit_cap() samples), and empty.
        for &n in &[0usize, 1, 17, 300, 4096, 4097, 9000, 12_289] {
            let s = samples(n, &mut st);
            for case in 0..24 {
                let m = model(case, &mut st);
                let sigma = (0.5 + 3.0 * lcg(&mut st)) / 255.0;
                let mean = [lcg(&mut st) as f32, 0.5, lcg(&mut st) as f32];
                let p = Predictions::new(&m, &s);
                let c = p.contrast();
                assert_eq!(
                    c.to_bits(),
                    contrast_ref(&m, &s).to_bits(),
                    "contrast {m:?} n {n}"
                );
                assert_eq!(
                    p.support(mean, c).to_bits(),
                    support_ref(&m, &s, mean, c).to_bits(),
                    "support {m:?} n {n}"
                );
                assert_eq!(
                    p.chi2(&s, sigma).to_bits(),
                    chi2_ref(&m, &s, sigma).to_bits(),
                    "chi2 {m:?} n {n}"
                );
            }
        }
    }

    #[test]
    fn a_gradient_the_emitter_paints_flat_is_imperceptible() {
        let lin = |c0: [f32; 3], c1: [f32; 3], mids: Vec<(f64, [f32; 3])>| FillModel::Linear {
            p0: (0.0, 0.0),
            p1: (10.0, 0.0),
            c0,
            c1,
            interp: Interp::Srgb,
            mids,
        };
        // Two levels apart: within the JND.
        let near = [0.94, 0.62, 0.34];
        let next = [0.95, 0.63, 0.35];
        assert!(imperceptible(&lin(near, next, vec![])));
        // Equal ends but a visible interior stop: the emitter keeps it.
        assert!(!imperceptible(&lin(
            near,
            near,
            vec![(0.5, [0.5, 0.3, 0.2])]
        )));
        // A visible ramp.
        assert!(!imperceptible(&lin([0.1; 3], [0.9; 3], vec![])));
        assert!(!imperceptible(&FillModel::Flat([0.5; 3])));
    }
}
