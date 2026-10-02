//! Stage 5's last guard: a fitted gradient that no viewer could see is painted flat.
//!
//! Split out of `pipeline.rs` (2026-10-02), which calls [`demote_imperceptible_gradient`]
//! from `final_fills` for every face, after the fill fitter and before the emitter.

use inkvec_trace::gradient;

/// Reject a gradient whose stops a viewer could not tell apart.
///
/// MDL already charges a gradient for its extra parameters, but a gradient has more
/// freedom than a flat fill and will always explain sensor noise a little better. When
/// the noise estimate is even slightly low, that freedom wins on cost while describing
/// something that is not there — flat concentric rings came back as four radial
/// gradients, costing fidelity rather than buying it.
///
/// The guard is perceptual rather than statistical: if every stop of the fitted profile is
/// within a just-noticeable difference of every other in OKLab, there is no gradient to
/// see, whatever the residual says.
///
/// *Every* stop, not the two ends. The profile is piecewise linear through its interior
/// stops (`mids`), so a light-dark-light shading -- a highlight across a cheek, a crease in
/// a sleeve -- has ends of one colour and its whole contrast in the middle. Comparing only
/// `c0` and `c1` painted those flat: on the 168 corpus icons with artist gradients, 18 of
/// the 49 gradient faces demoted had interior stops, and keeping the ones whose stops
/// differ made 7 icons better and none worse, dE00 -0.8% on those icons (r2-gradients,
/// 2026-10-02, knob `INKVEC_R2_DEMOTE_MIDS`).
///
/// The test is the profile's colour range: `max over stop pairs (i, j) of |L_i − L_j|`, the
/// Euclidean distance between OKLab colours `L` (Ottosson 2020), against `JND` = 0.02. With no
/// interior stops it is the old end-to-end test exactly, so a two-stop gradient is judged
/// as it always was. The cost is quadratic in the stop count, which the fitter caps at four
/// (two interior stops). A demoted gradient is painted the mean of its two ends, as before;
/// every stop it had lies within one `JND` of both.
///
/// Not from the literature: a fix to the guard's own definition. See also: Ottosson, "A
/// perceptual color space for image processing", 2020, <https://bottosson.github.io/posts/oklab/>,
/// the space the JND is measured in.
pub(super) fn demote_imperceptible_gradient(f: gradient::FillFit) -> gradient::FillFit {
    use inkvec_trace::color::rgb_to_oklab;
    /// A conservative multiple of a just-noticeable difference in OKLab.
    const JND: f32 = 0.02;

    let (c0, c1, mids) = match &f.model {
        gradient::FillModel::Flat(_) => return f,
        gradient::FillModel::Linear { c0, c1, mids, .. } => (*c0, *c1, mids),
        gradient::FillModel::Radial { c0, c1, mids, .. } => (*c0, *c1, mids),
    };
    // Every stop of the profile in OKLab, ends first.
    let stops: Vec<_> = [c0, c1]
        .into_iter()
        .chain(mids.iter().map(|&(_, c)| c))
        .map(rgb_to_oklab)
        .collect();
    let visible = stops
        .iter()
        .enumerate()
        .any(|(i, a)| stops[i + 1..].iter().any(|b| a.dist(*b) >= JND));
    if visible {
        return f;
    }
    let mid = [
        0.5 * (c0[0] + c1[0]),
        0.5 * (c0[1] + c1[1]),
        0.5 * (c0[2] + c1[2]),
    ];
    gradient::FillFit {
        model: gradient::FillModel::Flat(mid),
        ..f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A linear gradient from `c0` to `c1` through `mids`, wrapped as a fill.
    fn linear(c0: [f32; 3], c1: [f32; 3], mids: Vec<(f64, [f32; 3])>) -> gradient::FillFit {
        gradient::FillFit {
            model: gradient::FillModel::Linear {
                p0: (0.0, 0.0),
                p1: (10.0, 0.0),
                c0,
                c1,
                interp: gradient::Interp::Srgb,
                mids,
            },
            chi2: 0.0,
            params: 10.0,
            cost: 0.0,
        }
    }

    /// Ends of one colour with a darker band between them: the contrast is all in the
    /// interior stop, and the gradient stays a gradient. Ends of one colour with nothing
    /// between, or with interior stops that agree with them, are painted flat at the mean of
    /// the ends, exactly as before.
    #[test]
    fn demotion_looks_at_every_stop() {
        let light = [0.9, 0.8, 0.7];
        let near = [0.9, 0.8, 0.705];
        let kept = demote_imperceptible_gradient(linear(light, near, vec![(0.5, [0.5, 0.4, 0.3])]));
        assert!(matches!(kept.model, gradient::FillModel::Linear { .. }));
        // Two interior stops that differ from each other but each sit close to the ends'.
        let a = [0.9, 0.8, 0.72];
        let b = [0.9, 0.8, 0.68];
        let pair = demote_imperceptible_gradient(linear(light, light, vec![(0.3, a), (0.7, b)]));
        let la = inkvec_trace::color::rgb_to_oklab(a);
        let lb = inkvec_trace::color::rgb_to_oklab(b);
        assert_eq!(
            matches!(pair.model, gradient::FillModel::Linear { .. }),
            la.dist(lb) >= 0.02
        );
        for mids in [vec![], vec![(0.5, [0.9, 0.8, 0.702])]] {
            let flat = demote_imperceptible_gradient(linear(light, near, mids));
            match flat.model {
                gradient::FillModel::Flat(c) => {
                    assert!((c[2] - 0.7025).abs() < 1e-6, "{c:?}");
                }
                other => panic!("not demoted: {other:?}"),
            }
        }
        // A gradient whose ends differ is never demoted, mids or not.
        let wide = demote_imperceptible_gradient(linear([0.1, 0.1, 0.1], light, vec![]));
        assert!(matches!(wide.model, gradient::FillModel::Linear { .. }));
    }
}
