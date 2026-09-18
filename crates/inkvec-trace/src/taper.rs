//! Locate a junction where two boundaries meet tangentially, from the region that tapers.
//!
//! # Why the usual method cannot do it
//!
//! A junction is normally placed by intersecting the boundaries that meet there, which is
//! well conditioned when they cross at an angle and unusable when they do not: two
//! near-parallel lines intersect with condition number `1/sin θ`. Where a rounded corner
//! meets a straight side the two are tangent, and the position is not identifiable from
//! the intersection at all.
//!
//! It is not a small error. On a traced rounded square the junction landed 3.2px past the
//! true tangent point, so the straight side absorbed the first 3.2px of the arc and was
//! fitted as a line, leaving the corner to start 13 degrees into its turn — a visible kink
//! that no amount of curve fitting can remove, because the curves are faithfully reporting
//! a boundary that really does turn there.
//!
//! # What identifies it instead
//!
//! The region between the two boundaries tapers to nothing at the junction, and the points
//! along its curving side are points on that boundary. Take the straight boundary as
//! `y = 0`; then the curving one is a circle tangent to it, and where it touches is the
//! junction.
//!
//! Tangency is **imposed rather than checked**. The first version fitted the general
//! algebraic circle `x² + y² + Dx + Ey + F = 0`, took the junction at the double root
//! `x = −D/2`, and kept `D² − 4F` as evidence that the boundaries touched rather than
//! crossed. That works on exact data and fails on real data for a reason worth recording:
//! nothing in the fit knows about the tangency, so the leftover is never small — it came
//! in at 5.7 on the icon this was built for — and judging the fit by it means choosing a
//! bound for a quantity with no natural scale. The bound chosen threw away a good estimate.
//!
//! Putting the constraint in the model instead: a circle touching `y = 0` at `x = a` with
//! radius `R` satisfies
//!
//! ```text
//!     (x − a)² + y² − 2Ry = 0
//! ```
//!
//! in which `R` is linear given `a` and drops out in closed form, leaving a search in one
//! variable. Tangency now holds by construction, and what is left over is a real geometric
//! residual — how far the points lie from a tangent circle, in pixels. Refusing a fit on
//! that is refusing a model that does not describe the data, which is a different act from
//! tuning a threshold: a true taper comes in at 0.05px, two boundaries actually crossing at
//! 0.21, a strip of constant width at 1.33.
//!
//! The contact order says the same thing to leading order and is worth knowing, because it
//! is what makes the problem well posed at all:
//!
//! ```text
//!   transversal, angle θ        w ≈ θ·u        (intersection works)
//!   tangential, curvature Δκ    w ≈ Δκ·u²/2    (intersection fails; √w is linear in u)
//!   osculating                  w ∝ u³
//! ```
//!
//! Fitting `√w` against `u` is simpler and was tried first. It is only accurate for
//! `u ≪ R`: taken out to `u/R ≈ 0.75` it biased the intercept by 0.33px on exact data,
//! where the circle form is exact throughout. The asymptotic version is kept in this note
//! rather than in the code.
//!
//! # What the samples are
//!
//! Any `(u, w)` — position along the shared tangent, width of the tapering region there.
//! Two sources give them, and they are worth distinguishing:
//!
//! - **Coverage.** Summing `1 − alpha` down a column is a sub-pixel area measurement, so
//!   it keeps working where the region is thinner than a pixel and has no label at all.
//!   This is how the estimator was validated, and it recovers the boundary across the
//!   stretch that labelling never saw.
//! - **The planar map's own geometry**, which is what [`crate::planar`] actually feeds it:
//!   the branching edge's points, projected into the through direction's frame. No image
//!   access, so it costs nothing at the point of use. The contour still stops short of the
//!   junction — that is the unlabelled stretch — and the fit extrapolates across it.
//!
//! Validated against a known ground truth on the icon that motivated it, a 36-unit box at
//! 128px with corner radius 4, so the tangent point is at 13.72. From the measured
//! coverage the fit says 13.93; in the tracer, from map geometry alone, all eight corners
//! land within 0.15px of the true correction. The intersection put the junction at 10.5.

/// Result of fitting the taper.
#[derive(Debug, Clone, Copy)]
pub struct Taper {
    /// Where the tapering region vanishes, in the same units as the sample positions.
    pub vanish: f64,
    /// Standard error of `vanish`, from the regression's own residuals.
    pub sigma: f64,
    /// Radius implied by the slope. A cross-check, not an output.
    pub implied_radius: f64,
    /// Samples the fit used.
    pub used: usize,
    /// How far the samples sit from the fitted tangent circle, in pixels. Zero for a true
    /// tangency; large for a shape that is not one, which is what tells a touch from a
    /// crossing.
    pub tangency_defect: f64,
}

/// Smallest width worth trusting, in pixels of area per unit length. Below this the
/// measurement is dominated by the noise in a single pixel's coverage.
const MIN_WIDTH: f64 = 0.05;

/// Largest width to admit. Far from the junction the tapering region stops being bounded
/// by a single arc, so points out there describe some other part of the shape.
const MAX_WIDTH: f64 = 6.0;

/// Fewest samples that make the regression worth believing.
const MIN_SAMPLES: usize = 4;

/// How far the samples may sit from the fitted tangent circle, in pixels, before the model
/// is judged not to describe them.
///
/// This is a comparison against the measurement's own noise, not a tuned knob: coverage
/// widths carry a few hundredths of a pixel of error, so a genuine taper fits to well
/// inside a tenth. The icon that motivated the work comes in at 0.05. What the bound
/// excludes is a different shape entirely — a wedge of two boundaries actually crossing
/// gives 0.21, and a strip of constant width 1.33.
const MAX_DEFECT: f64 = 0.15;

/// Fit the tapering boundary and return where it meets the straight one.
///
/// `samples` are `(u, w)`: position along the shared tangent, and the tapering region's
/// width there. Positions may run in either direction; the fit does not care.
///
/// The estimate is exact for a circular corner rather than asymptotic. The points `(u, w)`
/// lie on the curving boundary itself, and in the algebraic circle
///
/// ```text
///     x² + y² + Dx + Ey + F = 0
/// ```
///
/// the straight boundary is `y = 0`. Tangency is imposed rather than hoped for. Letting it
/// float and then checking `D² − 4F` afterwards means choosing a bound for a leftover that
/// is never small on real data, and on the icon this was built for that check threw away a
/// good estimate. So the constraint goes into the model: a circle touching `y = 0` at
/// `x = a` with radius `R` satisfies
///
/// ```text
///     (x − a)² + y² − 2Ry = 0
/// ```
///
/// where `R` is linear given `a` and drops out in closed form, leaving a search in one
/// variable. What is left over is then a real residual — how far the points lie from a
/// tangent circle — and refusing a fit on that is refusing a model that does not describe
/// the data, which is a different thing from tuning a threshold.
///
/// Fitting `√w` against `u` instead is the same idea to leading order and is what the
/// contact analysis suggests, but it is only accurate for `u ≪ R`: taken out to `u/R ≈
/// 0.75` it biased the intercept by 0.33px on exact data, where this form is exact
/// throughout.
///
/// Returns `None` when too few samples fall in the trustworthy band, or when the fit does
/// not describe a plausible corner.
pub fn fit(samples: &[(f64, f64)]) -> Option<Taper> {
    let pts: Vec<(f64, f64)> = samples
        .iter()
        .copied()
        .filter(|(_, w)| *w > MIN_WIDTH && *w < MAX_WIDTH)
        .collect();
    if pts.len() < MIN_SAMPLES {
        return None;
    }

    // For a given tangent point `a`, the radius is linear and drops out in closed form;
    // what is left is a search in one variable.
    let residual = |a: f64| -> (f64, f64) {
        let (mut num, mut den) = (0.0, 0.0);
        for &(x, y) in &pts {
            let dx = x - a;
            num += (dx * dx + y * y) * y;
            den += y * y;
        }
        let r = num / (2.0 * den);
        let sse: f64 = pts
            .iter()
            .map(|&(x, y)| {
                let e = (x - a) * (x - a) + y * y - 2.0 * r * y;
                e * e
            })
            .sum();
        (sse, r)
    };

    let (lo, hi) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), &(x, _)| (l.min(x), h.max(x)));
    let span = (hi - lo).max(1.0);
    // The tangent point lies beyond the thin end of the samples rather than among them, so
    // the search has to reach past both ends.
    let (from, to) = (lo - 3.0 * span, hi + 3.0 * span);

    let steps = 2000;
    let mut best = (f64::MAX, from, 0.0);
    for i in 0..=steps {
        let a = from + (to - from) * (i as f64 / steps as f64);
        let (v, r) = residual(a);
        if v.is_finite() && v < best.0 {
            best = (v, a, r);
        }
    }
    let (mut sse, mut a, mut radius) = best;
    let mut step = (to - from) / steps as f64;
    for _ in 0..50 {
        step *= 0.5;
        for cand in [a - step, a + step] {
            let (v, r) = residual(cand);
            if v.is_finite() && v < sse {
                sse = v;
                a = cand;
                radius = r;
            }
        }
    }
    if !a.is_finite() || !radius.is_finite() || radius <= 0.5 {
        return None;
    }

    // The algebraic residual is in length squared; dividing by `2R` — the gradient of that
    // form at the circle — turns it back into a distance, so the defect is how far the
    // samples sit from the fitted tangent circle, in pixels.
    let n = pts.len() as f64;
    let defect = (sse / n).sqrt() / (2.0 * radius);
    if defect > MAX_DEFECT {
        return None;
    }

    // Standard error of `a` from the curvature of its own objective: with `s²` the residual
    // variance, `Var(a) = 2s² / F''(a)`.
    let h = (span * 1e-3).max(1e-4);
    let f2 = (residual(a + h).0 - 2.0 * sse + residual(a - h).0) / (h * h);
    let s2 = sse / (n - 2.0).max(1.0);
    let sigma = if f2 > 1e-12 {
        (2.0 * s2 / f2).sqrt()
    } else {
        f64::INFINITY
    };

    Some(Taper {
        vanish: a,
        sigma,
        implied_radius: radius,
        used: pts.len(),
        tangency_defect: defect,
    })
}
