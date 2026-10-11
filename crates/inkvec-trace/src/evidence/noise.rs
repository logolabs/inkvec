//! The window variance of one image, estimated from the image's own edges
//! (`docs/theory/chain-boundary.md`, "Noise: the window variance from the edges").
//!
//! Every window's sum is the area its face holds there, plus an error: 8-bit rounding, which
//! the variance model of [`super::windows`] knows exactly, and whatever else the intake's
//! history added (a renderer's sampling lattice and curve flattening, a resampler's phase
//! ripple, a JPEG's blocks and ringing). That error is a function of the edge's local
//! sub-pixel phase and of what lies near it, so it has three parts, each measured or bounded
//! here from the image alone:
//!
//! * **Replicas.** Consecutive windows whose mean positions differ by a whole number of
//!   pixels see the edge at the same phase: on an axis or a diagonal every window is the same
//!   measurement repeated, with the same error (bench: correlation 0.84 to 0.95 between such
//!   neighbours, whatever the input). A stretch of `k` replicas counts as one measurement:
//!   each gets `k` times the variance ([`replica_stretches`]).
//! * **The rough part**, independent between windows at different phases: the fourth
//!   differences `D` of consecutive windows' mean positions see it whole, since `D`
//!   annihilates every cubic, so a smooth boundary contributes nothing. `D` is the residual
//!   of the local cubic through five windows: the fitted model's residual, with no fit to
//!   iterate. Its variance is `Σ wᵢ² (Vᵢ + s²)`, `Vᵢ` the known rounding variances and `s²`
//!   the extra, solved per class of window from a winsorised second moment (exact for normal
//!   errors, within 2 % for the bounded errors of rounding, and not carried away by a JPEG's
//!   heavy tails). Only stencils with no replica step, no phase step under 0.06 px (where
//!   rounding errors anticorrelate) and a local second derivative under 0.08 px⁻¹ count: on
//!   tighter curves the geometry's own fourth difference leaks into `D` (bench: the `D` of the
//!   measured positions and of the truth's errors agree below 0.08 and part above it).
//! * **The smooth part**, shared along a run: an offset no difference sees, the same as
//!   moving the edge, so it cannot be measured from one image. Its variance is set by the
//!   phase-locking argument above: an axis-aligned edge's constant error *is* one window's
//!   error, replicated, so each edge gets an offset of the variance of one of its windows
//!   (the trait's `chi2_floor`). A fitted candidate absorbs it in its own position.
//!
//! Never from flat regions: on the web tier the noise within 3 px of an edge is 5 to 9 times
//! the interiors', where `coverage::estimate_noise` reads (`docs/theory/noise.md`).
//!
//! The tail is read from the same `D`s: the ratio of the 90th to the 50th percentile of `|z|`
//! is 2.44 for a normal and grows as the tail thickens; matched to Student-t's, then diluted
//! back to one window's (a `D` mixes five windows, which keeps `Σw⁴/(Σw²)² = 0.37` of their
//! excess kurtosis).

use inkvec_core::likelihood::RunObs;

/// Weights of the fourth difference.
const W4: [f64; 5] = [1.0, -4.0, 6.0, -4.0, 1.0];
/// `Σ wᵢ²`.
const W4_SQ: f64 = 70.0;
/// Winsorisation of the standardised `D²`, and its expectation for a standard normal
/// (`E min(z², c²)` at `c = 2.5`).
const WINSOR: f64 = 2.5;
const WINSOR_NORMAL: f64 = 0.977_6;
/// A window whose local second derivative (of the boundary's position along the strip, per
/// window) is below this is on a straight stretch.
const STRAIGHT_CURVATURE: f64 = 0.02;
/// At or above this the geometry's fourth difference leaks into `D`: such stencils are not
/// used for the estimate.
const LEAK_CURVATURE: f64 = 0.08;
/// A phase step below this between neighbouring windows (and not a replica) makes their
/// rounding errors anticorrelate: such stencils are not used.
const MIN_PHASE_STEP: f64 = 0.06;
/// Two windows' positions differing by a whole number of pixels to within this are replicas.
const REPLICA_TOL: f64 = 1e-6;
/// Fewest `D`s for a class's own estimate; fewer and the pooled estimate stands in.
const MIN_D: usize = 12;

/// Window classes the variance is estimated for separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A locally straight boundary.
    Straight = 0,
    /// A curved one.
    Curved = 1,
}

/// What one image's windows say about their error beyond rounding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    /// Per class, the variance each window's sum has beyond the rounding model, px².
    pub extra: [f64; 2],
    /// Per class, how many fourth differences the estimate rests on.
    pub count: [usize; 2],
    /// Student-t degrees of freedom of the windows' standardised errors; infinite when they
    /// are no heavier-tailed than a normal's.
    pub nu: f64,
}

impl Calibration {
    /// Nothing beyond rounding.
    pub const NONE: Calibration = Calibration {
        extra: [0.0; 2],
        count: [0; 2],
        nu: f64::INFINITY,
    };
}

/// Whether windows `a` and `b` (in order) are neighbours: one axis, consecutive lines.
fn neighbours(a: &RunObs, b: &RunObs) -> bool {
    a.window.axis == b.window.axis && (b.window.line - a.window.line).abs() == 1
}

/// The phase step between neighbouring windows: how far their mean positions differ from a
/// whole number of pixels, in `[0, ½]`.
fn phase_step(a: &RunObs, b: &RunObs) -> f64 {
    let d = (b.mean_position() - a.mean_position()).abs().fract();
    d.min(1.0 - d)
}

/// Per window, the size of the replica stretch it belongs to (1 for a window seen at a phase
/// of its own): maximal runs of neighbours whose phase step is zero.
pub fn replica_stretches(obs: &[RunObs]) -> Vec<usize> {
    let n = obs.len();
    let mut out = vec![1usize; n];
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j + 1 < n
            && neighbours(&obs[j], &obs[j + 1])
            && phase_step(&obs[j], &obs[j + 1]) < REPLICA_TOL
        {
            j += 1;
        }
        for o in &mut out[i..=j] {
            *o = j - i + 1;
        }
        i = j + 1;
    }
    out
}

/// Per window, its class and its local second derivative (`None` where too few neighbours):
/// a least-squares quadratic through up to three windows either side, along one axis.
pub fn classify(obs: &[RunObs]) -> Vec<(Class, Option<f64>)> {
    let n = obs.len();
    (0..n)
        .map(|i| {
            let mut lo = i;
            while lo > 0 && i - lo < 3 && neighbours(&obs[lo - 1], &obs[lo]) {
                lo -= 1;
            }
            let mut hi = i;
            while hi + 1 < n && hi - i < 3 && neighbours(&obs[hi], &obs[hi + 1]) {
                hi += 1;
            }
            if hi - lo < 4 {
                return (Class::Curved, None);
            }
            let u0 = obs[i].window.line as f64;
            let pts: Vec<(f64, f64)> = (lo..=hi)
                .map(|k| (obs[k].window.line as f64 - u0, obs[k].mean_position()))
                .collect();
            let k = (2.0 * quadratic_coefficient(&pts)).abs();
            let class = if k < STRAIGHT_CURVATURE {
                Class::Straight
            } else {
                Class::Curved
            };
            (class, Some(k))
        })
        .collect()
}

/// The `u²` coefficient of the least-squares quadratic through `pts`.
fn quadratic_coefficient(pts: &[(f64, f64)]) -> f64 {
    let n = pts.len() as f64;
    let mean = |f: &dyn Fn(f64, f64) -> f64| pts.iter().map(|&(u, v)| f(u, v)).sum::<f64>() / n;
    let (mu, mu2, mv) = (mean(&|u, _| u), mean(&|u, _| u * u), mean(&|_, v| v));
    let suu = mean(&|u, _| (u - mu) * (u - mu));
    let su2u = mean(&|u, _| (u * u - mu2) * (u - mu));
    let su2u2 = mean(&|u, _| (u * u - mu2) * (u * u - mu2));
    let svu = mean(&|u, v| (v - mv) * (u - mu));
    let svu2 = mean(&|u, v| (v - mv) * (u * u - mu2));
    let det = suu * su2u2 - su2u * su2u;
    if det.abs() < 1e-12 {
        return 0.0;
    }
    (suu * svu2 - su2u * svu) / det
}

/// One five-window stencil for the rough part: the centre's class, the largest local second
/// difference of positions inside it (a corner or a tight curve shows there), `D`, and
/// `Σ wᵢ² Vᵢ`.
#[derive(Debug, Clone, Copy)]
pub struct Stencil {
    class: Class,
    bend: f64,
    d: f64,
    v: f64,
}

/// The fourth-difference stencils of an edge's windows that can see its rough error: five
/// consecutive windows (one axis, consecutive lines, one direction) with no replica or small
/// phase step among them. Which of them are free of the geometry is decided in [`calibrate`],
/// against the noise. `quant` holds the windows' rounding variances.
pub fn fourth_differences(
    obs: &[RunObs],
    quant: &[f64],
    classes: &[(Class, Option<f64>)],
) -> Vec<Stencil> {
    let mut out = Vec::new();
    if obs.len() < 5 {
        return out;
    }
    for i in 2..obs.len() - 2 {
        let five = &obs[i - 2..=i + 2];
        let step = five[1].window.line - five[0].window.line;
        let ok = five.windows(2).all(|p| {
            neighbours(&p[0], &p[1])
                && p[1].window.line - p[0].window.line == step
                && phase_step(&p[0], &p[1]) >= MIN_PHASE_STEP
        });
        if !ok {
            continue;
        }
        let h: Vec<f64> = five.iter().map(|o| o.mean_position()).collect();
        let bend = (1..4)
            .map(|j| (h[j - 1] - 2.0 * h[j] + h[j + 1]).abs())
            .fold(0.0, f64::max);
        let d: f64 = h.iter().zip(W4).map(|(h, w)| w * h).sum();
        let v: f64 = (0..5).map(|j| W4[j] * W4[j] * quant[i - 2 + j]).sum();
        out.push(Stencil {
            class: classes[i].0,
            bend,
            d,
            v,
        });
    }
    out
}

/// The extra variance `s²` whose winsorised second moment of `D/√(V + 70 s²)` is a normal's,
/// or 0 when the rounding model already explains the `D`s to within three standard errors of
/// that moment (`√(1.5/n)`: the variance of `min(z², c²)` is about 1.5): a few `D`s that a
/// tight curve's geometry or a corner leaks into must not raise every window's variance.
fn solve_extra(ds: &[(f64, f64)]) -> f64 {
    let moment = |s2: f64| -> f64 {
        ds.iter()
            .map(|&(d, v)| (d * d / (v + W4_SQ * s2).max(1e-300)).min(WINSOR * WINSOR))
            .sum::<f64>()
            / ds.len() as f64
    };
    let se = (1.5 / ds.len().max(1) as f64).sqrt();
    if ds.is_empty() || moment(0.0) <= WINSOR_NORMAL + 3.0 * se {
        return 0.0;
    }
    let mut hi = ds.iter().map(|&(d, _)| d * d).fold(0.0, f64::max) / W4_SQ;
    let mut lo = 0.0;
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        if moment(mid) > WINSOR_NORMAL {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Student-t's `q90/q50` of `|t|` (`t₀.₉₅/t₀.₇₅`), for `ν` from 3 up; the normal's is 2.4387.
const T_TAIL: [(f64, f64); 9] = [
    (3.0, 3.0767),
    (4.0, 2.8782),
    (5.0, 2.7729),
    (6.0, 2.7080),
    (8.0, 2.6325),
    (10.0, 2.5899),
    (20.0, 2.5107),
    (40.0, 2.4738),
    (f64::INFINITY, 2.4387),
];

/// Degrees of freedom of a `D`'s tail from its `q90/q50` ratio, then one window's: a `D`
/// keeps 0.37 of its windows' excess kurtosis (`6/(ν − 4)` for Student-t), so
/// `ν_window = 4 + 0.37 (ν_D − 4)`, at least 3.
fn tail_nu(z: &mut [f64]) -> f64 {
    if z.len() < 4 * MIN_D {
        return f64::INFINITY;
    }
    for v in z.iter_mut() {
        *v = v.abs();
    }
    let q = |z: &mut [f64], p: f64| {
        let k = ((z.len() - 1) as f64 * p).round() as usize;
        *z.select_nth_unstable_by(k, f64::total_cmp).1
    };
    let ratio = q(z, 0.9) / q(z, 0.5).max(1e-300);
    // A standard error of about 0.1 on the ratio for a few hundred Ds: within it, normal.
    if ratio <= T_TAIL[T_TAIL.len() - 1].1 + 0.1 {
        return f64::INFINITY;
    }
    let mut nu_d = 3.0;
    for p in T_TAIL.windows(2) {
        let ((n0, r0), (n1, r1)) = (p[0], p[1]);
        if ratio <= r0 && ratio > r1 {
            nu_d = if n1.is_finite() {
                n0 + (r0 - ratio) / (r0 - r1) * (n1 - n0)
            } else {
                n0
            };
            break;
        }
    }
    (4.0 + 0.37 * (nu_d - 4.0)).max(3.0)
}

/// Calibrate from every edge's stencils: per class `s²`, and the pooled tail.
///
/// A stencil whose positions bend by more than [`LEAK_CURVATURE`] (a corner, a tight curve)
/// carries the geometry's own fourth difference, but on a noisy intake the noise bends every
/// stencil that much: so the gate is the larger of that and three standard deviations of a
/// second difference under the noise found so far, starting from all stencils and settling
/// in a few rounds.
pub fn calibrate(stencils: &[Stencil]) -> Calibration {
    let pairs = |sel: &dyn Fn(&Stencil) -> bool| -> Vec<(f64, f64)> {
        stencils
            .iter()
            .filter(|s| sel(s))
            .map(|s| (s.d, s.v))
            .collect()
    };
    // A typical window's rounding variance: Σ wᵢ² Vᵢ / 70.
    let mut vq: Vec<f64> = stencils.iter().map(|s| s.v / W4_SQ).collect();
    let v_typ = if vq.is_empty() {
        0.0
    } else {
        let k = vq.len() / 2;
        *vq.select_nth_unstable_by(k, f64::total_cmp).1
    };
    let mut s2 = solve_extra(&pairs(&|_| true));
    let mut gate = LEAK_CURVATURE;
    for _ in 0..4 {
        gate = LEAK_CURVATURE.max(6.0 * (6.0 * (v_typ + s2)).sqrt());
        s2 = solve_extra(&pairs(&|s| s.bend < gate));
    }
    let all = s2;
    let mut out = Calibration::NONE;
    for c in [Class::Straight, Class::Curved] {
        let mine = pairs(&|s| s.class == c && s.bend < gate);
        out.count[c as usize] = mine.len();
        out.extra[c as usize] = if mine.len() >= MIN_D {
            solve_extra(&mine)
        } else {
            all
        };
    }
    let mut z: Vec<f64> = stencils
        .iter()
        .filter(|s| s.bend < gate)
        .map(|s| {
            s.d / (s.v + W4_SQ * out.extra[s.class as usize])
                .max(1e-300)
                .sqrt()
        })
        .collect();
    out.nu = tail_nu(&mut z);
    out
}

/// Candidate sampling lattices, samples per pixel along each axis.
const LATTICES: [u32; 5] = [4, 8, 16, 32, 64];

/// The sampling lattice of a point-sampled renderer, read from the image: on an axis-aligned
/// edge (a replica stretch whose windows hold one partial pixel) such a renderer's coverage is
/// a multiple of `1/n`, then rounded to 8 bits. `values` are those coverages with the
/// half-width of their rounding. The smallest `n` that at least 90 % of them sit on, where a
/// random coverage would sit on it at most half the time; `None` for an exact-area renderer
/// (or too few informative stretches to tell, fewer than 8).
pub fn detect_lattice(values: &[(f64, f64)]) -> Option<u32> {
    // A coverage of one half sits on every lattice, and drawings put edges on half pixels by
    // design (a 64-unit icon at 128 px): such values say nothing about `n`, and an edge there
    // has no lattice error anyway.
    let values: Vec<(f64, f64)> = values
        .iter()
        .copied()
        .filter(|&(c, half)| (c - 0.5).abs() > half + 1e-9)
        .collect();
    if values.len() < 8 {
        return None;
    }
    for n in LATTICES {
        let nf = n as f64;
        let (mut hits, mut chance) = (0usize, 0.0);
        for &(c, half) in &values {
            let d = (c * nf - (c * nf).round()).abs() / nf;
            if d <= half + 1e-9 {
                hits += 1;
            }
            chance += (2.0 * (half + 1e-9) * nf).min(1.0);
        }
        let k = values.len() as f64;
        if hits as f64 >= 0.9 * k && chance <= 0.5 * k {
            return Some(n);
        }
    }
    None
}
