//! How an editability edit is accepted: priced by MDL against the evidence, with a learned
//! bonus per kind of edit for the structure artists pay for.
//!
//! # The rule
//!
//! An edit of kind `k` is kept when
//!
//! ```text
//!     ½·(χ²_after − χ²_before)  ≤  λ·(P_before − P_after + credit + b_k)
//! ```
//!
//! summed over the source points of every ring it touches. `χ²` is [`two_sided_chi2`],
//! `P` the numbers the ring writes ([`Segment::params`]), `λ` the fit's price of one number
//! (`FitConfig::lambda`), `credit` the numbers an edit makes implied under the fit's own code
//! (one direction for a G1 join, as `inkvec_fit::tangents::break_cost` charges a turn), and
//! `b_k` the bonus: how many numbers' worth of fidelity the artists' own files show they pay
//! for that structure. With every `b_k` at zero this is the fit's own objective, plain MDL.
//!
//! The fixed budget it replaces (every sample within `3·σ + slack` of the evidence,
//! [`super::guarded`]) accepted each edit on its own, so hundreds of edits each inside the
//! budget added up: on the 246-icon screen set it cost +33 % dE00 and +15.6 % geometric
//! distance from the artists' files, most of it from G1 snaps (+19 % dE00 alone).
//!
//! # The bonuses, and how they were found
//!
//! By inverse optimisation of this rule against the artists' files: the bonuses are chosen
//! so that the edits' own optimum reproduces the structure the artists drew (Maximum Margin
//! Planning, Ratliff, Bagnell & Zinkevich, ICML 2006, whose subgradient is the structured
//! perceptron's update, Collins, EMNLP 2002), each kind read through the structure it makes
//! (exactly smooth joins, nodes at extrema, arcs written as cubics, ...), with the gate's
//! geometric distance held to +1 % by a Lagrange multiplier and turning_gap to +2 %.
//! Nine kinds were learnt; backward elimination kept three that carry the gain (the rest,
//! zeroed, changed nothing: an axis handle comes free with a node at an extremum), and the
//! arc kind was split by sweep. The smooth bonus was then halved: on the training half that
//! moved edge placement 0.6–0.9 points closer to the artists at the same design gain.
//!
//! Measured against the plain trace (no `--editability`), all 246 screen icons: the design
//! battery 8.4–9.3 % closer to how the artists draw in every clean condition, edge placement
//! (geomatch's near-edge mismatch) −0.90 % / −0.31 % / −0.83 % at 512 px opaque / 128 px /
//! 512 px transparent, region errors, turning_gap and ratio_gap inside the gate's margins.
//! On 228 icons never used to learn them (128 px): edge placement −0.68 %, a structure-coded
//! wireframe compared by C-RADIOv4 −6.4 % closer to the artist's (131 icons closer, 30
//! further). Research scripts: `bench/research/inverse_mdl3.py`, `proxy.py`, `coded_wire.py`.
//!
//! `INKVEC_EDIT_PRICE=off` restores the fixed budget; `INKVEC_EDIT_PRICE=kind=b,...` sets
//! the bonuses (unlisted kinds 0), for research.

use std::cell::Cell;
use std::sync::OnceLock;

use inkvec_core::{Point, Polyline};
use inkvec_fit::curves::Segment;
use inkvec_fit::nearest::{BoxTree, Piece};
use inkvec_fit::structural::eval_segment;
use inkvec_fit::FittedPath;

use super::{chain_starts, guarded};

/// The kinds of edit, one bonus each. `smooth` covers G1 joins within a ring and at
/// junctions alike.
pub(crate) const KINDS: [&str; 10] = [
    "arcs",
    "extrema",
    "mirror",
    "nodes",
    "collinear",
    "smooth",
    "axis",
    "diag",
    "equal",
    "arcs_wide",
];
/// An arc of at most a quarter turn written as cubics cut at its extrema.
pub(crate) const ARCS: usize = 0;
/// A smooth run re-noded at its extrema.
pub(crate) const EXTREMA: usize = 1;
pub(crate) const MIRROR: usize = 2;
pub(crate) const NODES: usize = 3;
pub(crate) const COLLINEAR: usize = 4;
/// A G1 join, within a ring or at a junction.
pub(crate) const SMOOTH: usize = 5;
pub(crate) const AXIS: usize = 6;
pub(crate) const DIAG: usize = 7;
pub(crate) const EQUAL: usize = 8;
/// An arc turning more than a quarter turn written as cubics.
pub(crate) const ARCS_WIDE: usize = 9;

/// The learned bonuses, in numbers (module docs). Any arc bonus from about 8 up behaves the
/// same, every arc is written as cubics: the rewrite is exact to 1.1e-3 of the radius, so it
/// costs no fidelity and only its numbers. The smooth bonus is at a 512 px raster
/// ([`scaled`]).
const DEFAULT: [f64; 10] = [23.93, 11.85, 0.0, 0.0, 0.0, 11.20, 0.0, 0.0, 0.0, 40.0];

thread_local! {
    static LAMBDA: Cell<f64> = const { Cell::new(f64::NAN) };
    static SIDE: Cell<f64> = const { Cell::new(512.0) };
}

/// The fit's price of one number for the passes on this thread (`super::edit_all`).
pub(crate) fn set_lambda(l: f64) {
    LAMBDA.with(|c| c.set(l));
}

fn lambda() -> f64 {
    LAMBDA.with(|c| c.get())
}

/// The traced raster's longer side, px, for this thread's passes (`pipeline.rs`).
pub(crate) fn set_side(px: f64) {
    SIDE.with(|c| c.set(px));
}

/// The bonus for `kind`, or `None` when edits keep the fixed budget
/// (`INKVEC_EDIT_PRICE=off`).
pub(crate) fn bonus(kind: usize) -> Option<f64> {
    static B: OnceLock<Option<[f64; 10]>> = OnceLock::new();
    B.get_or_init(|| match inkvec_core::env::text("INKVEC_EDIT_PRICE") {
        None => Some(DEFAULT),
        Some(t) if t.trim().eq_ignore_ascii_case("off") => None,
        Some(t) => {
            let mut b = [0.0; 10];
            for item in t.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let (k, v) = item.split_once('=').expect("INKVEC_EDIT_PRICE: name=value");
                let i = KINDS
                    .iter()
                    .position(|n| *n == k.trim())
                    .unwrap_or_else(|| panic!("INKVEC_EDIT_PRICE: unknown kind {k:?}"));
                b[i] = v.trim().parse().expect("INKVEC_EDIT_PRICE: a number");
            }
            Some(b)
        }
    })
    .map(|b| b[kind])
}

/// The bonus `b` for `kind` at this thread's raster size. Only the smooth bonus scales,
/// with the raster's longer side over 512 px: a G1 snap bends the curve along its length,
/// the `χ²` a bonus `b` may spend is `2λb`, and a curve carries evidence points in
/// proportion to its length in pixels, so at a quarter of the size the same `b` lets every
/// point move twice as far (`δ ≈ σ·√(2λb/n)`). Measured at 128 px: the 512 px bonus moved
/// geom +1.3 % by itself, a quarter of it held geom and kept the gain. The arc and extremum
/// edits are rewrites that spend no fidelity, and their bonuses held geom at 128 px
/// unscaled.
pub(crate) fn scaled(kind: usize, b: f64) -> f64 {
    if kind == SMOOTH {
        b * SIDE.with(|c| c.get()) / 512.0
    } else {
        b
    }
}

/// The rule itself, in nats: keep an edit whose half change in `χ²` is at most `λ` times
/// the numbers it saves plus its credit and bonus.
fn keeps(dchi2: f64, dparams: f64, credit: f64, bonus: f64, lambda: f64) -> bool {
    0.5 * dchi2 <= lambda * (dparams + credit + bonus)
}

/// One ring an edit touches: its source points and sigma, and its chain before and after.
pub(crate) struct Touched<'a> {
    pub(crate) pts: &'a [Point],
    pub(crate) sigma: &'a [f64],
    pub(crate) before: (Point, &'a [Segment]),
    pub(crate) after: (Point, &'a [Segment]),
}

/// The MDL acceptance for an edit of `kind` touching `rings`, or `None` when edits keep the
/// fixed budget (the caller then applies it): under `INKVEC_EDIT_PRICE=off`, and for a pass
/// called with no price of a number set on its thread (outside `super::edit_all`).
pub(crate) fn priced(kind: usize, rings: &[Touched<'_>], credit: f64) -> Option<bool> {
    let b = scaled(kind, bonus(kind)?);
    let lambda = lambda();
    if !lambda.is_finite() {
        return None;
    }
    let mut dchi2 = 0.0;
    let mut dparams = 0.0;
    for r in rings {
        let c0 = two_sided_chi2(r.pts, r.sigma, r.before.0, r.before.1);
        let c1 = two_sided_chi2(r.pts, r.sigma, r.after.0, r.after.1);
        if !c0.is_finite() || !c1.is_finite() {
            return Some(false);
        }
        dchi2 += c1 - c0;
        dparams += r.before.1.iter().map(Segment::params).sum::<f64>()
            - r.after.1.iter().map(Segment::params).sum::<f64>();
    }
    Some(keeps(dchi2, dparams, credit, b, lambda))
}

/// The acceptance every single-ring pass uses: [`priced`], or the fixed budget
/// ([`guarded`]) under `INKVEC_EDIT_PRICE=off`. `before` is the ring as it was; `fp` as
/// edited.
pub(crate) fn accept_ring(
    kind: usize,
    before: (Point, &[Segment]),
    fp: &FittedPath,
    pts: &[Point],
    sigma: &[f64],
    credit: f64,
) -> bool {
    let ring = Touched {
        pts,
        sigma,
        before,
        after: (fp.start, &fp.segments),
    };
    priced(kind, &[ring], credit)
        .unwrap_or_else(|| guarded(&fp.segments, &chain_starts(fp), pts, sigma))
}

/// The distortion [`priced`] charges: `χ²` both ways between the source points and the
/// curve, both by exact distances.
///
/// The point-to-curve term is [`inkvec_fit::multimodel::path_chi2`] (exact nearest point),
/// not `inkvec_fit::curves::chi2`: that one samples the curve every 0.25 px, a floor of
/// about two units of `χ²` per point at σ = 0.05 px (its own docs), so an edit moving an
/// edge by 0.1 px, which the image sees, costs it nothing (measured: handle snaps it passed
/// cost 3.7 % dE00).
///
/// The curve-to-points term catches what the first cannot: a handle turned so the curve
/// overshoots between two nodes leaves every point as close to the curve as before. Each of
/// the curve's samples (every half pixel at most) is charged its squared distance to the
/// source polyline (to its segments, not its vertices, which would floor it the same way),
/// over the sigma interpolated there, weighted by the arc length it stands for over the
/// points' mean spacing, so a run of curve counts as much as the points that cover it: the
/// symmetric (chamfer) form of the same Gaussian residual. Infinite for an empty chain or
/// fewer than two points.
///
/// Both nearest searches go through bounding boxes ([`inkvec_fit::nearest`]) and return the
/// bits a scan of every segment returns. The source segments' boxes are weighted by one
/// over the larger sigma at their ends: the sigma interpolated along a segment is never
/// larger, so the weighted squared distance to the box bounds the term from below.
pub(crate) fn two_sided_chi2(pts: &[Point], sigma: &[f64], start: Point, segs: &[Segment]) -> f64 {
    if segs.is_empty() || pts.len() < 2 || sigma.is_empty() {
        return f64::INFINITY;
    }
    let sg = |j: usize| sigma[j.min(sigma.len() - 1)].max(1e-3);
    let poly = Polyline {
        points: pts.to_vec(),
        sigma: (0..pts.len()).map(sg).collect(),
        closed: false,
    };
    let path = FittedPath {
        start,
        segments: segs.to_vec(),
        closed: false,
    };
    let fwd = inkvec_fit::multimodel::path_chi2(&poly, &path);
    if !fwd.is_finite() {
        return fwd;
    }
    let poly_len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    let spacing = (poly_len / (pts.len() - 1) as f64).max(1e-6);
    let source = BoxTree::new(
        (0..pts.len() - 1)
            .map(|j| Piece::around(&pts[j..j + 2], 1.0 / sg(j).max(sg(j + 1)).powi(2)))
            .collect(),
    );
    let mut rev = 0.0;
    let mut cur = start;
    for seg in segs {
        let end = seg.end();
        let n = ((cur.dist(end) / 0.5).ceil() as usize).max(4);
        let mut prev = cur;
        for k in 1..=n {
            let s = eval_segment(seg, cur, k as f64 / n as f64);
            let step = prev.dist(s);
            prev = s;
            let best = source.min_by(s, |j| {
                let (a, b) = (pts[j], pts[j + 1]);
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let l2 = dx * dx + dy * dy;
                let t = if l2 > 0.0 {
                    (((s.x - a.x) * dx + (s.y - a.y) * dy) / l2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let q = Point::new(a.x + t * dx, a.y + t * dy);
                let sig = sg(j) * (1.0 - t) + sg(j + 1) * t;
                (s.dist(q) / sig).powi(2)
            });
            rev += best * step / spacing;
        }
        cur = end;
    }
    fwd + rev
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_points(n: usize) -> Vec<Point> {
        (0..=n).map(|i| Point::new(i as f64, 0.0)).collect()
    }

    #[test]
    fn a_curve_on_the_evidence_costs_nothing_either_way() {
        let pts = line_points(20);
        let c = two_sided_chi2(&pts, &[0.1], pts[0], &[Segment::Line(pts[20])]);
        assert!(c < 1e-9, "{c}");
    }

    #[test]
    fn an_overshoot_between_the_points_is_charged_by_the_reverse_term() {
        // A cubic from (0,0) to (20,0) whose handles swing it 3 px off the line between the
        // ends: every point is still close to *some* part of the curve near the ends, but
        // the bulge in the middle is far from the evidence.
        let pts = line_points(20);
        let flat = two_sided_chi2(&pts, &[0.1], pts[0], &[Segment::Line(pts[20])]);
        let bulge = Segment::Cubic(Point::new(5.0, 4.0), Point::new(15.0, 4.0), pts[20]);
        let bent = two_sided_chi2(&pts, &[0.1], pts[0], &[bulge]);
        assert!(bent > flat + 100.0, "{bent} vs {flat}");
    }

    #[test]
    fn the_rule_trades_misfit_for_numbers_and_bonus() {
        let lambda = 8.0;
        // Saves nothing, costs fidelity: refused without a bonus, kept with one large enough.
        assert!(!keeps(4.0, 0.0, 0.0, 0.0, lambda));
        assert!(keeps(4.0, 0.0, 0.0, 0.25, lambda));
        // A G1 join's one implied number pays for a small misfit.
        assert!(keeps(10.0, 0.0, 1.0, 0.0, lambda));
        // Adding numbers needs a bonus or a better fit.
        assert!(!keeps(0.0, -6.0, 0.0, 5.0, lambda));
        assert!(keeps(0.0, -6.0, 0.0, 6.0, lambda));
    }

    #[test]
    fn only_the_smooth_bonus_scales_with_the_raster() {
        set_side(128.0);
        assert_eq!(scaled(SMOOTH, 8.0), 2.0);
        assert_eq!(scaled(ARCS, 8.0), 8.0);
        assert_eq!(scaled(EXTREMA, 8.0), 8.0);
        set_side(1024.0);
        assert_eq!(scaled(SMOOTH, 8.0), 16.0);
        set_side(512.0);
    }

    #[test]
    fn the_shipped_bonuses_are_the_measured_ones() {
        if inkvec_core::env::text("INKVEC_EDIT_PRICE").is_some() {
            return;
        }
        assert_eq!(bonus(ARCS), Some(23.93));
        assert_eq!(bonus(ARCS_WIDE), Some(40.0));
        assert_eq!(bonus(EXTREMA), Some(11.85));
        assert_eq!(bonus(SMOOTH), Some(11.20));
        for k in [MIRROR, NODES, COLLINEAR, AXIS, DIAG, EQUAL] {
            assert_eq!(bonus(k), Some(0.0), "{}", KINDS[k]);
        }
    }
}
