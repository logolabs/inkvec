//! Centreline recovery: line art as **strokes**, not as filled outlines.
//!
//! An entire major category of icon art is drawn as centrelines with a stroke width.
//! Sampling the Lucide set (1791 icons): **100% use `stroke`, 100% set
//! `stroke-linecap`/`stroke-linejoin`, and none use a solid fill.** Feather, Heroicons
//! and the Material "outlined" family are the same. The artist drew one path and one
//! number.
//!
//! Converting that to a filled outline costs three things, in increasing order of
//! seriousness:
//!
//! 1. Two boundaries instead of one — roughly double the geometry.
//! 2. The topology of the drawing is destroyed. A crossing of two strokes becomes a
//!    single blob whose four arms are no longer separable.
//! 3. **Line weight stops being editable.** For the artist it is one scalar; for the
//!    outline it is implicit in every coordinate, and changing it is not an edit any
//!    tool can express.
//!
//! `DESIGN.md` scoped this out ("not centerline/sketch tracing"). This module brings it
//! in, and it does so under the same measurement discipline as the rest of the S2 stage:
//! every recovered quantity carries the uncertainty it was measured with, and the
//! decision to call something a stroke is made against that uncertainty rather than
//! against a taste threshold.
//!
//! # Method
//!
//! **Distance field, sub-pixel.** Everything here is a statement about the distance
//! from a point to the region boundary, so that field has to be as good as the boundary
//! is. A pixel-lattice distance transform is quantised to half a pixel, which is larger
//! than the width error we are trying to achieve. Instead the region's boundary is
//! extracted as the 0.5 level set of the coverage field — the same level set
//! `contour.rs` traces, and for the same reason — and the distance field is the exact
//! Euclidean distance to those line segments. The label map still decides *topology*
//! (which pixels belong to the region); coverage only supplies sub-pixel *position*.
//! That is the ordering `planar.rs` uses and it is what lets exactness and accuracy
//! coexist.
//!
//! **Skeleton.** Zhang–Suen thinning on the region mask, reduced to a graph: nodes at
//! endpoints and junctions, edges as pixel chains. Diagonal adjacencies that are
//! shadowed by an orthogonal pair are dropped, so vertex degree is a real graph degree
//! rather than a raw 8-neighbour count. Spurs shorter than the local width are pruned:
//! at a corner or a cap, thinning invents a 45-degree branch whose length is bounded by
//! the width, so "shorter than the local width" is the artefact's own scale rather than
//! an invented constant.
//!
//! **Sub-pixel ridge.** The centreline is where the distance field is locally maximal
//! across the stroke, so each skeleton point is moved along its local normal to the
//! sub-pixel ridge by a parabolic fit on three samples. The fit is iterated: a distance
//! field near its ridge is a *tent*, not a parabola, and one parabolic step on a tent
//! closes only half the gap. Iterating closes it geometrically.
//!
//! **Width.** Twice the median distance-field value along the centreline, measured on
//! the interior of the chain only — the field necessarily dips at a cap and bulges at a
//! junction, and including either would report a width variation that is an artefact of
//! the ends rather than a property of the stroke.
//!
//! # What makes something a stroke
//!
//! Three tests, in the order they discriminate:
//!
//! * **Aspect.** `length / width >= MIN_ASPECT`. A disc has skeleton length near zero
//!   and width equal to its diameter; a square has a skeleton whose branches are each
//!   shorter than the width. Blobs fail here and they fail by a wide margin.
//! * **Constant width.** The robust spread of the width samples must be within
//!   `TAU_WIDTH` of what the *measurement* can explain, where that measurement sigma is
//!   the coverage-derived positional sigma of the two boundary points the width was read
//!   between. A stroke that tapers is not a constant-width stroke and is returned for
//!   ordinary filled tracing rather than forced into a `stroke-width`.
//! * **Area.** `sum(length * width) + caps` must account for the region's measured area
//!   to within [`AREA_EXPLAINED_MIN`, `AREA_EXPLAINED_MAX`]. This is the test that
//!   catches a shape which is *partly* stroke-like — a lollipop, a stroke with a
//!   filled blob on the end — where the per-edge tests would happily accept the stick.
//!
//! ## False positives, honestly
//!
//! The criterion is a statement about *shape*, and a shape can be stroke-like without
//! having been drawn as a stroke. The accepted false positives are:
//!
//! * A long thin filled rectangle, or a hand-drawn filled ribbon of near-constant
//!   width, is indistinguishable from a stroke by any measurement of the raster, because
//!   it *is* one. Re-emitting it as a stroke is a lossless re-description, not an error;
//!   it only becomes wrong if the caller then lets a user change the width and expects
//!   the original silhouette back.
//! * A crescent or a tapering wedge whose taper is slower than `TAU_WIDTH` sigma over
//!   its length. This is the real failure mode, and it is bounded: the width is right to
//!   within a few times the measurement sigma by construction, so the error is small
//!   even when the decision is wrong.
//!
//! The failure the criterion refuses to make is the opposite one: it will not call a
//! blob a stroke, because [`MIN_ASPECT`] is checked against the region's *own* size
//! rather than against an absolute pixel count. That asymmetry is deliberate. Emitting a
//! filled region as a filled region is always safe; emitting a filled region as a stroke
//! is not.
//!
//! Callers should read [`StrokeAnalysis::stroke_fraction`] and decide globally. An icon
//! set is line art or it is not; deciding per region invites a document that is half
//! stroked and half filled, which is worse for editing than either.
//!
//! # Known limits
//!
//! * **Sharp joins are the weakest geometry.** Thinning does not put a vertex at the
//!   apex of a sharp join; it puts it on the notch side, short by `(W/2)/sin(theta/2)`.
//!   The ridge walk pulls it back out — measured on a 110-degree zig-zag it recovers the
//!   apex to under 1px against 2.6px before — but the residual error grows as the join
//!   gets sharper, and it is a *corner position* error rather than a width error. If a
//!   caller needs the apex exactly, the right fix is downstream: intersect the two
//!   fitted line segments, which is what `inkvec_fit::adjust_vertices` already does for
//!   contours.
//! * **Free ends are short by up to half a width.** A butt cap's medial axis stops
//!   `W/2` before the end and thinning trims a little more. The centreline is right
//!   where it exists; it does not extend to the cap. An emitter that draws
//!   `stroke-linecap="butt"` should extend each free end by `W/2`.
//! * **Sub-pixel-wide strokes are out of scope here.** Below about a pixel there is no
//!   region to thin, and `CoverageField::saturation` is already the honest signal that
//!   the colour axis is unidentifiable. That case belongs to the S2 joint solve, not to
//!   a skeleton.
//! * **Two ink regions that touch** share a boundary the coverage field cannot separate,
//!   so the level set there falls back to the pixel midpoint. Topology stays right
//!   (labels decide it); only the sub-pixel position of that one shared boundary
//!   degrades to what a lattice tracer would have said.

use inkvec_core::{Point, Polyline, Vec2};
use inkvec_fit::multimodel::optimal_multimodel;
use inkvec_fit::{FitConfig, FittedPath};

use crate::contour::inflate_for_curvature;
use crate::coverage::{CoverageField, DEFAULT_SIGMA_MODEL};

mod outline;
mod skeleton;

use outline::{
    cross, ms_pairs, offset_outline, others_cover, outline_cover, point_seg, stroke_cost,
};
use skeleton::{geometric_chains, prune_spurs, zhang_suen, RawChain};

// ---------------------------------------------------------------------------------
// Tunables. Each is either a scale read off the data or a confidence multiplier; none
// is a pixel count.
// ---------------------------------------------------------------------------------

/// Least `length / width` a stroke may have.
///
/// Three is where the shapes separate rather than where a sweep put the knee: a disc
/// scores 0 (its skeleton is a point), a square scores about 1.4 (the medial axis is the
/// two diagonals, each half-diagonal shorter than the width), and the thinnest thing an
/// artist would call a dash scores 3 or more. The gap between 1.4 and 3 is where the
/// threshold sits, and nothing real is in it.
pub const MIN_ASPECT: f64 = 3.0;

/// How [`analyse_with`] decides that a region is drawn line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Criteria {
    /// Least `length / width` of each skeleton edge in a region whose skeleton has more than
    /// one edge. [`MIN_ASPECT`] (the default) asks every edge to be a stroke on its own, which
    /// refuses any drawing with junctions closer than three widths: the legs of lucide's
    /// `bug` are one to two and a half widths between junctions, so the whole region goes
    /// back to filled tracing. With a smaller value the aspect test moves to the region —
    /// the summed edge length must still reach [`MIN_ASPECT`] widths — and a blob is still
    /// refused by the width-spread and area tests, which it fails by a wide margin.
    pub min_edge_aspect: f64,
    /// Multiple of the local width below which a dead-end branch is pruned as a thinning
    /// artefact, whatever its end looks like ([`SPUR_FACTOR`] by default). The length is a
    /// proxy for the cap test beside it and it is wrong for short drawn stubs: an antenna
    /// one width long off a junction ends in a round cap and is pruned by length alone.
    pub spur_factor: f64,
}

impl Default for Criteria {
    fn default() -> Self {
        Criteria {
            min_edge_aspect: MIN_ASPECT,
            spur_factor: SPUR_FACTOR,
        }
    }
}

/// [`Criteria`] for drawings with junctions: an edge may be half a width long, and a
/// stub off a junction stays when it ends in a cap and is at least a quarter width long.
/// Measured on a traced lucide `bug`, its antennae leave the junction as skeleton stubs of
/// 0.31 and 0.44 widths whose tips sit at 0.97 of the median distance: caps, not corners.
pub const GRAPH_CRITERIA: Criteria = Criteria {
    min_edge_aspect: 0.5,
    spur_factor: 0.25,
};

/// Confidence multiplier on the width-measurement sigma, above which a width variation
/// is real rather than measurement noise. The same role `FitConfig::tau` plays for
/// position.
pub const TAU_WIDTH: f64 = 3.0;

/// Least fraction of a region's area the recovered strokes must account for.
pub const AREA_EXPLAINED_MIN: f64 = 0.70;

/// Most of a region's area the recovered strokes may account for. Above 1.0 because
/// strokes meeting at a junction cover the junction twice.
pub const AREA_EXPLAINED_MAX: f64 = 1.50;

/// Regions smaller than this are speckle, not drawing. Below about this size the
/// skeleton is one or two pixels and carries no shape information at all.
pub const MIN_REGION_PIXELS: usize = 8;

/// Irreducible width error, in pixels: the level set locates each of the two boundaries
/// to about [`DEFAULT_SIGMA_MODEL`], and a width is a difference of two of them.
///
/// This is systematic along a stroke and therefore does **not** average down with the
/// number of samples, which is why it is carried separately from the sample spread.
pub const WIDTH_SIGMA_FLOOR: f64 = std::f64::consts::SQRT_2 * DEFAULT_SIGMA_MODEL;

/// Multiple of the local width below which a dead-end branch is a thinning artefact.
pub const SPUR_FACTOR: f64 = 1.0;

/// Least ratio of the distance-field value at a dead-end branch's tip to the median
/// along that branch, for the tip to be a genuine stroke end rather than a corner.
///
/// The length test alone is not enough and the failure is systematic rather than
/// marginal. A branch that thinning invents at a boundary corner runs *to* that corner,
/// where the distance field is zero by definition; a real stroke end runs to a cap,
/// where the distance field is still half the width. So the discriminating quantity is
/// not how long the branch is but whether it ends in a cap — and the length test is only
/// a proxy for it that happens to work when the corner is blunt. At a sharp corner the
/// invented branch is *long* (its length grows as `1 / sin(angle/2)`), and the length
/// test lets it through: measured on a 110-degree zig-zag, three real corners came back
/// as three separate strokes with the connecting branches silently dropped.
///
/// Compared against the branch's own median rather than against the width at its root,
/// because the distance field bulges at a junction — at a four-way crossing it is a
/// factor of `sqrt(2)` up — and comparing against the bulge would prune the real arms.
pub(crate) const CAP_FRACTION: f64 = 0.7;

/// Parabolic ridge steps. A tent halves the remaining offset per step, so eight steps
/// take a half-pixel initial error to well under a thousandth of a pixel.
const RIDGE_ITERS: usize = 8;

/// Furthest a skeleton point may be moved by ridge refinement, as a multiple of the
/// stroke width, before we conclude the normal was wrong and keep the unrefined
/// position. Scaled by the width because that is the only length in the problem: the
/// distance from a thinned junction to the true apex of a join grows with the width.
const MAX_RIDGE_SHIFT: f64 = 1.0;

/// Least shift the bound allows regardless of width, in pixels — half a pixel of
/// thinning quantisation plus room for the diagonal case.
const MIN_RIDGE_SHIFT: f64 = 1.5;

/// Samples either side of the current point in the fallback scan.
const SCAN_SAMPLES: usize = 12;

/// Half-window, in chain points, for the local tangent used to define the normal.
const TANGENT_WINDOW: usize = 2;

/// Side of a spatial-hash cell for the level-set segment index, in pixels.
const CELL: f64 = 1.0;

// ---------------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------------

/// One recovered stroke: a centreline and the single number that is its weight.
#[derive(Debug, Clone)]
pub struct Stroke {
    /// Sub-pixel centreline. For a closed stroke the first point is **not** repeated at
    /// the end, matching [`Polyline`] convention.
    pub path: Vec<Point>,
    /// Per-point positional uncertainty of the centreline, in pixels.
    ///
    /// Not in the original sketch of this type, but without it the centreline cannot be
    /// handed to the fitter at all — every stage of `inkvec-fit` reads its tolerance
    /// from `Polyline::sigma`, and inventing a uniform value here would throw away the
    /// one thing S2 exists to produce. Derived the same way `contour.rs` derives it: the
    /// coverage-gradient positional sigma of the boundary, halved in quadrature because
    /// a centreline is the midpoint of two independent boundary measurements, then
    /// inflated for local non-linearity by [`inflate_for_curvature`].
    pub sigma: Vec<f64>,
    /// Stroke width in pixels — the number `stroke-width` would carry.
    pub width: f64,
    /// Uncertainty of `width`, in pixels. Sample spread averaged down over the
    /// centreline, combined in quadrature with the systematic [`WIDTH_SIGMA_FLOOR`],
    /// which does not average down.
    pub width_sigma: f64,
    /// Whether the centreline is a closed loop.
    pub closed: bool,
    /// Region label this stroke was recovered from, so a caller can put the stroke back
    /// where the region was in z-order and give it the region's colour.
    pub label: u16,
}

impl Stroke {
    /// Arc length of the measured centreline, including the closing segment when closed.
    pub fn length(&self) -> f64 {
        let n = self.path.len();
        if n < 2 {
            return 0.0;
        }
        let mut l: f64 = self.path.windows(2).map(|w| w[0].dist(w[1])).sum();
        if self.closed {
            l += self.path[n - 1].dist(self.path[0]);
        }
        l
    }

    /// The centreline as a measured boundary the fitter can consume.
    pub fn polyline(&self) -> Polyline {
        Polyline::new(self.path.clone(), self.sigma.clone(), self.closed)
    }

    /// Fit the centreline with the S4 multi-model dynamic program.
    ///
    /// The same optimization the filled path gets — there is nothing special about a
    /// centreline as a curve. What is special is that it is *one* curve where the filled
    /// description needs two, and that the width it drops out of the geometry is a
    /// parameter the designer can then edit.
    pub fn fit(&self, cfg: &FitConfig) -> FittedPath {
        optimal_multimodel(&self.polyline(), cfg)
    }

    /// Parameters this stroke costs a document: the fitted path plus one for the width.
    pub fn params(&self, cfg: &FitConfig) -> f64 {
        self.fit(cfg).params() + 1.0
    }
}

/// What [`analyse`] concluded about an image.
#[derive(Debug, Clone, Default)]
pub struct StrokeAnalysis {
    /// Strokes recovered from the image.
    pub strokes: Vec<Stroke>,
    /// Fraction of the **ink** area explained as strokes, in `[0, 1]`.
    ///
    /// Ink is every region whose mean coverage is at least 0.5; background regions are
    /// in neither the numerator nor the denominator, because "what fraction of this
    /// drawing is line art" is not a question about the paper. Near 1 means the whole
    /// image is line art and should be emitted as strokes; near 0 means it is not and
    /// the strokes found, if any, are incidental.
    pub stroke_fraction: f64,
    /// Labels that were **not** recovered as strokes and should go through ordinary
    /// filled tracing. Includes background regions. Sorted and deduplicated.
    pub residual_regions: Vec<u16>,
}

/// Decide whether each region is stroke-like, and if so recover its centreline and width.
///
/// `coverage` must be the field in which the ink of interest is *high* — the output of
/// [`crate::coverage::bilevel_coverage`], or a per-ink field on the colour path.
/// `labels` is a per-pixel region id in the same layout as the image, as produced by
/// [`bilevel_labels`] or by the colour front end's face labelling.
pub fn analyse(coverage: &CoverageField, labels: &[u16], w: usize, h: usize) -> StrokeAnalysis {
    analyse_with(coverage, labels, w, h, Criteria::default())
}

/// [`analyse`] under explicit [`Criteria`].
pub fn analyse_with(
    coverage: &CoverageField,
    labels: &[u16],
    w: usize,
    h: usize,
    criteria: Criteria,
) -> StrokeAnalysis {
    if w == 0 || h == 0 || labels.len() != w * h {
        return StrokeAnalysis::default();
    }

    let stats = region_stats(coverage, labels, w, h);
    let mut out = StrokeAnalysis::default();
    let (mut ink_area, mut stroke_area) = (0.0f64, 0.0f64);

    for (label, st) in stats.iter().enumerate() {
        let Some(st) = st else { continue };
        let label = label as u16;

        // Background is not a stroke candidate and does not belong in the denominator of
        // "how much of this drawing is line art".
        let is_ink = st.count > 0 && st.cov_sum / st.count as f64 >= 0.5;
        if !is_ink || st.count < MIN_REGION_PIXELS {
            out.residual_regions.push(label);
            continue;
        }

        let region = RegionGrid::build(coverage, labels, w, h, label, st);
        ink_area += region.area();
        match analyse_region(coverage, &region, label, criteria) {
            Some(strokes) => {
                stroke_area += region.area();
                out.strokes.extend(strokes);
            }
            None => out.residual_regions.push(label),
        }
    }

    out.stroke_fraction = if ink_area > 0.0 {
        (stroke_area / ink_area).clamp(0.0, 1.0)
    } else {
        0.0
    };
    out.residual_regions.sort_unstable();
    out.residual_regions.dedup();
    out
}

/// Label a bilevel coverage field by 4-connected component of `alpha >= 0.5`.
///
/// A convenience for the bilevel front end and for tests: the colour path already
/// produces face labels of its own. Background components are labelled too, so every
/// pixel has a region and [`analyse`] can report the ones it declined.
pub fn bilevel_labels(coverage: &CoverageField) -> Vec<u16> {
    let (w, h) = (coverage.width, coverage.height);
    let n = w * h;
    let mut out = vec![u16::MAX; n];
    let mut next: u32 = 0;
    let mut stack: Vec<usize> = Vec::new();

    for seed in 0..n {
        if out[seed] != u16::MAX {
            continue;
        }
        if next >= u16::MAX as u32 {
            break;
        }
        let id = next as u16;
        next += 1;
        let ink = coverage.data[seed] >= 0.5;
        out[seed] = id;
        stack.push(seed);
        while let Some(p) = stack.pop() {
            let (x, y) = (p % w, p / w);
            let visit = |q: usize, out: &mut Vec<u16>, stack: &mut Vec<usize>| {
                if out[q] == u16::MAX && (coverage.data[q] >= 0.5) == ink {
                    out[q] = id;
                    stack.push(q);
                }
            };
            if x > 0 {
                visit(p - 1, &mut out, &mut stack);
            }
            if x + 1 < w {
                visit(p + 1, &mut out, &mut stack);
            }
            if y > 0 {
                visit(p - w, &mut out, &mut stack);
            }
            if y + 1 < h {
                visit(p + w, &mut out, &mut stack);
            }
        }
    }
    for v in out.iter_mut() {
        if *v == u16::MAX {
            *v = 0;
        }
    }
    out
}

// ---------------------------------------------------------------------------------
// Region bookkeeping
// ---------------------------------------------------------------------------------

struct RegionStat {
    count: usize,
    cov_sum: f64,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

fn region_stats(
    coverage: &CoverageField,
    labels: &[u16],
    w: usize,
    h: usize,
) -> Vec<Option<RegionStat>> {
    let max_label = labels.iter().copied().max().unwrap_or(0) as usize;
    let mut stats: Vec<Option<RegionStat>> = (0..=max_label).map(|_| None).collect();
    for y in 0..h {
        for x in 0..w {
            let l = labels[y * w + x] as usize;
            let a = coverage.data[y * w + x] as f64;
            match &mut stats[l] {
                Some(s) => {
                    s.count += 1;
                    s.cov_sum += a;
                    s.x0 = s.x0.min(x);
                    s.y0 = s.y0.min(y);
                    s.x1 = s.x1.max(x);
                    s.y1 = s.y1.max(y);
                }
                slot => {
                    *slot = Some(RegionStat {
                        count: 1,
                        cov_sum: a,
                        x0: x,
                        y0: y,
                        x1: x,
                        y1: y,
                    })
                }
            }
        }
    }
    stats
}

/// A region cropped out of the image with a two-pixel background pad.
///
/// `u` is the *effective* indicator: the coverage value, clamped so that its sign about
/// 0.5 always agrees with the label map. Where label and coverage agree — everywhere, on
/// clean input — coverage supplies the sub-pixel boundary position untouched. Where they
/// disagree the clamp puts the boundary at the pixel midpoint, which is exactly what a
/// lattice tracer would have said, and the disagreement is not allowed to change which
/// pixels are in the region.
pub(crate) struct RegionGrid {
    x0: isize,
    y0: isize,
    w: usize,
    h: usize,
    u: Vec<f32>,
    mask: Vec<bool>,
}

/// Keeps the clamped field strictly off the level so a crossing is never ambiguous.
const CLAMP_EPS: f32 = 1e-3;

impl RegionGrid {
    fn build(
        coverage: &CoverageField,
        labels: &[u16],
        iw: usize,
        ih: usize,
        label: u16,
        st: &RegionStat,
    ) -> RegionGrid {
        const PAD: isize = 2;
        let x0 = st.x0 as isize - PAD;
        let y0 = st.y0 as isize - PAD;
        let w = (st.x1 - st.x0) + 1 + 2 * PAD as usize;
        let h = (st.y1 - st.y0) + 1 + 2 * PAD as usize;

        let mut u = vec![0.0f32; w * h];
        let mut mask = vec![false; w * h];
        for j in 0..h {
            for i in 0..w {
                let (gx, gy) = (x0 + i as isize, y0 + j as isize);
                if gx < 0 || gy < 0 || gx >= iw as isize || gy >= ih as isize {
                    continue; // virtual background, as in planar.rs
                }
                let p = gy as usize * iw + gx as usize;
                let inside = labels[p] == label;
                let a = coverage.data[p];
                mask[j * w + i] = inside;
                u[j * w + i] = if inside {
                    a.max(0.5 + CLAMP_EPS)
                } else {
                    a.min(0.5 - CLAMP_EPS)
                };
            }
        }
        RegionGrid {
            x0,
            y0,
            w,
            h,
            u,
            mask,
        }
    }

    /// Sub-pixel area of the region: the integral of the effective coverage.
    ///
    /// Not the pixel count. A 3px stroke has two anti-aliased fringes carrying a real
    /// fraction of a pixel each, and rounding them away biases the area by roughly a
    /// third of a pixel per unit length — which is a tenth of the very quantity the area
    /// test is trying to check.
    fn area(&self) -> f64 {
        self.u.iter().map(|&a| a as f64).sum()
    }

    #[inline]
    pub(crate) fn point_of(&self, i: usize, j: usize) -> Point {
        Point::new((self.x0 + i as isize) as f64, (self.y0 + j as isize) as f64)
    }
}

// ---------------------------------------------------------------------------------
// Level set and exact distance to it
// ---------------------------------------------------------------------------------

/// The region boundary as line segments, plus a spatial hash for nearest-point queries.
///
/// Marching squares on the effective indicator field, exactly as `contour.rs` does it —
/// but the segments are wanted as *geometry*, not as linked contours, so nothing here
/// needs the grid-edge identity bookkeeping that makes contour linking exact.
pub(crate) struct LevelSet {
    segs: Vec<(Point, Point)>,
    ox: f64,
    oy: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl LevelSet {
    fn build(g: &RegionGrid) -> LevelSet {
        let mut segs: Vec<(Point, Point)> = Vec::new();
        if g.w < 2 || g.h < 2 {
            return LevelSet::index(segs);
        }
        let at = |i: usize, j: usize| g.u[j * g.w + i];
        for j in 0..g.h - 1 {
            for i in 0..g.w - 1 {
                let (u00, u10, u11, u01) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                let case = (u00 >= 0.5) as u8
                    | (((u10 >= 0.5) as u8) << 1)
                    | (((u11 >= 0.5) as u8) << 2)
                    | (((u01 >= 0.5) as u8) << 3);
                if case == 0 || case == 15 {
                    continue;
                }
                let p = g.point_of(i, j);
                let e = [
                    Point::new(p.x + cross(u00, u10), p.y),
                    Point::new(p.x + 1.0, p.y + cross(u10, u11)),
                    Point::new(p.x + cross(u01, u11), p.y + 1.0),
                    Point::new(p.x, p.y + cross(u00, u01)),
                ];
                let centre_in = 0.25 * (u00 + u10 + u11 + u01) >= 0.5;
                for &(a, b) in ms_pairs(case, centre_in) {
                    segs.push((e[a], e[b]));
                }
            }
        }
        LevelSet::index(segs)
    }

    fn index(segs: Vec<(Point, Point)>) -> LevelSet {
        if segs.is_empty() {
            return LevelSet {
                segs,
                ox: 0.0,
                oy: 0.0,
                nx: 0,
                ny: 0,
                cells: Vec::new(),
            };
        }
        let (mut lo, mut hi) = (
            (f64::INFINITY, f64::INFINITY),
            (f64::NEG_INFINITY, f64::NEG_INFINITY),
        );
        for &(a, b) in &segs {
            lo = (lo.0.min(a.x).min(b.x), lo.1.min(a.y).min(b.y));
            hi = (hi.0.max(a.x).max(b.x), hi.1.max(a.y).max(b.y));
        }
        let (ox, oy) = (lo.0 - CELL, lo.1 - CELL);
        let nx = (((hi.0 - ox) / CELL).ceil() as usize + 2).max(1);
        let ny = (((hi.1 - oy) / CELL).ceil() as usize + 2).max(1);
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); nx * ny];
        for (k, &(a, b)) in segs.iter().enumerate() {
            let i0 = (((a.x.min(b.x) - ox) / CELL).floor() as isize).clamp(0, nx as isize - 1);
            let i1 = (((a.x.max(b.x) - ox) / CELL).floor() as isize).clamp(0, nx as isize - 1);
            let j0 = (((a.y.min(b.y) - oy) / CELL).floor() as isize).clamp(0, ny as isize - 1);
            let j1 = (((a.y.max(b.y) - oy) / CELL).floor() as isize).clamp(0, ny as isize - 1);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    cells[j as usize * nx + i as usize].push(k as u32);
                }
            }
        }
        LevelSet {
            segs,
            ox,
            oy,
            nx,
            ny,
            cells,
        }
    }

    /// Distance from `p` to the boundary, and the boundary point achieving it.
    ///
    /// Exact against the piecewise-linear level set, and evaluated at arbitrary
    /// sub-pixel positions rather than interpolated off a grid — interpolating a
    /// distance field smooths the very crease the ridge walk is looking for.
    pub(crate) fn nearest(&self, p: Point) -> (f64, Point) {
        if self.segs.is_empty() {
            return (f64::INFINITY, p);
        }
        let gx = ((p.x - self.ox) / CELL).floor() as isize;
        let gy = ((p.y - self.oy) / CELL).floor() as isize;
        let (nx, ny) = (self.nx as isize, self.ny as isize);
        let max_r = nx.max(ny) + 1;

        let mut best = f64::INFINITY;
        let mut bp = p;
        let mut r: isize = 0;
        while r <= max_r {
            for jy in (gy - r).max(0)..=(gy + r).min(ny - 1) {
                for jx in (gx - r).max(0)..=(gx + r).min(nx - 1) {
                    if r > 0 && (jx - gx).abs() != r && (jy - gy).abs() != r {
                        continue;
                    }
                    for &k in &self.cells[jy as usize * self.nx + jx as usize] {
                        let (a, b) = self.segs[k as usize];
                        let (d, q) = point_seg(p, a, b);
                        if d < best {
                            best = d;
                            bp = q;
                        }
                    }
                }
            }
            // Anything not yet examined lies outside the ring-r block and is therefore
            // at least r cells away.
            if best.is_finite() && best <= r as f64 * CELL {
                break;
            }
            r += 1;
        }
        (best, bp)
    }
}

// ---------------------------------------------------------------------------------
// Refinement against the coverage field
// ---------------------------------------------------------------------------------

/// How far a point may travel, in pixels.
///
/// The centreline starts on the sub-pixel ridge, which is a good estimator, so a
/// correct refinement is a nudge and anything larger is the solve wandering. At
/// 0.75 it wandered: points on `book-heart` moved 0.63, an order of magnitude past
/// the 0.02-0.06 px the boundary solve works at, and the result got worse. A
/// quarter pixel is the most a correction to a ridge fit can honestly be.
const REFINE_MAX: f64 = 0.25;
/// Finite-difference step. Small enough that the coverage ramp is locally linear,
/// large enough to clear the coverage field's own quantisation.
const REFINE_H: f64 = 0.02;
/// Weight on the second difference of the offsets.
///
/// Without it this is coordinate descent with nothing coupling neighbours, and it
/// does what that always does: every point finds its own local minimum, the
/// centreline comes out crumpled, and the segment fitter then has to spend
/// parameters describing the crumple. Measured on four lucide icons, unpenalised
/// refinement moved dE00 the wrong way on all four and *raised* the parameter
/// count from 92 to 110 on `bell` -- a curve that costs more to state is not a
/// better curve. `boundary_opt` carries a kink term for the same reason.
const REFINE_KINK: f64 = 0.35;

/// How well the strokes, drawn as they will actually be drawn, reproduce the
/// coverage they came from. Root-mean-square residual per pixel over the ink and
/// its surround, in coverage units.
///
/// This is the honest gate. Ink balance -- total length times width against total
/// coverage -- is a proxy, and a loose one: `badge-pound-sterling` balances to
/// within a tenth and still traces at dE00 2.57 against the filled path's 0.15,
/// because two strokes in the right amount can be in the wrong places. Scoring
/// the union of the real outlines against the real coverage cannot be fooled that
/// way, and it costs one pass over the ink's bounding box.
/// How well one stroke reproduces the ink of *its own region*.
///
/// The pooled version below scores a set of strokes against the whole coverage
/// field, which is right for a set and wrong for a member: a single stroke
/// measured that way is charged for every pixel its neighbours cover, so on a
/// drawing with four strokes each one looks catastrophic and none survives a
/// threshold that ought to keep most. Restricting to the label the stroke came
/// from asks the question that actually decides whether to keep it.
pub fn stroke_residual_one(st: &Stroke, coverage: &CoverageField, labels: &[u16]) -> f64 {
    let (w, h) = (coverage.width, coverage.height);
    if w == 0 || h == 0 || labels.len() < w * h {
        return f64::INFINITY;
    }
    let (outer, inner) = offset_outline(&st.path, st.closed, st.width * 0.5);
    if outer.len() < 3 {
        return f64::INFINITY;
    }
    let (x0, y0, x1, y1) = stroke_window(&st.path, st.width * 0.5, w, h);
    let (mut ca, mut cb) = (Vec::with_capacity(32), Vec::with_capacity(32));
    let (mut acc, mut n) = (0.0f64, 0usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = y * w + x;
            let model = outline_cover(
                &outer,
                inner.as_ref(),
                x as f64 - 0.5,
                y as f64 - 0.5,
                &mut ca,
                &mut cb,
            );
            let mine = labels[i] == st.label;
            // Its own pixels, and anything it paints that is not its own.
            if mine || model > 0.001 {
                let target = if mine { coverage.data[i] as f64 } else { 0.0 };
                acc += (model - target) * (model - target);
                n += 1;
            }
        }
    }
    if n == 0 {
        return f64::INFINITY;
    }
    (acc / n as f64).sqrt()
}

/// RMS residual between the coverage rendered from `strokes` and `coverage` itself,
/// over each stroke's own pixels plus any pixel it paints outside them.
pub fn stroke_residual(strokes: &[Stroke], coverage: &CoverageField) -> f64 {
    let (w, h) = (coverage.width, coverage.height);
    if w == 0 || h == 0 || strokes.is_empty() {
        return f64::INFINITY;
    }
    let mut model = vec![0.0f32; w * h];
    for st in strokes {
        let (outer, inner) = offset_outline(&st.path, st.closed, st.width * 0.5);
        if outer.len() < 3 {
            continue;
        }
        let (x0, y0, x1, y1) = stroke_window(&st.path, st.width * 0.5, w, h);
        let (mut ca, mut cb) = (Vec::with_capacity(32), Vec::with_capacity(32));
        for y in y0..y1 {
            for x in x0..x1 {
                let c = outline_cover(
                    &outer,
                    inner.as_ref(),
                    x as f64 - 0.5,
                    y as f64 - 0.5,
                    &mut ca,
                    &mut cb,
                ) as f32;
                if c > model[y * w + x] {
                    model[y * w + x] = c;
                }
            }
        }
    }
    // Over every pixel that either side calls ink, plus what surrounds it: scoring
    // the whole canvas would divide the error by the empty background and report
    // any drawing as excellent.
    let (mut acc, mut n) = (0.0f64, 0usize);
    for i in 0..w * h {
        let (a, b) = (model[i] as f64, coverage.data[i] as f64);
        if a > 0.001 || b > 0.001 {
            acc += (a - b) * (a - b);
            n += 1;
        }
    }
    if n == 0 {
        return f64::INFINITY;
    }
    (acc / n as f64).sqrt()
}

/// Unit normal to the centreline at point `i`.
fn normal_at(path: &[Point], i: usize, closed: bool) -> Point {
    let n = path.len();
    let (a, b) = if closed {
        (path[(i + n - 1) % n], path[(i + 1) % n])
    } else if i == 0 {
        (path[0], path[1])
    } else if i + 1 == n {
        (path[n - 2], path[n - 1])
    } else {
        (path[i - 1], path[i + 1])
    };
    let t = b - a;
    let l = (t.x * t.x + t.y * t.y).sqrt();
    if l <= 1e-12 {
        return Point::new(0.0, 0.0);
    }
    Point::new(-t.y / l, t.x / l)
}

fn point_window(p: Point, hw: f64, w: usize, h: usize) -> (usize, usize, usize, usize) {
    let r = hw + 2.0;
    let x0 = (p.x - r).floor().max(0.0) as usize;
    let y0 = (p.y - r).floor().max(0.0) as usize;
    let x1 = (((p.x + r).ceil().max(0.0) as usize) + 1).min(w);
    let y1 = (((p.y + r).ceil().max(0.0) as usize) + 1).min(h);
    (x0.min(x1), y0.min(y1), x1, y1)
}

fn stroke_window(path: &[Point], hw: f64, w: usize, h: usize) -> (usize, usize, usize, usize) {
    let (mut lox, mut loy, mut hix, mut hiy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in path {
        lox = lox.min(p.x);
        loy = loy.min(p.y);
        hix = hix.max(p.x);
        hiy = hiy.max(p.y);
    }
    let r = hw + 2.0;
    let x0 = (lox - r).floor().max(0.0) as usize;
    let y0 = (loy - r).floor().max(0.0) as usize;
    let x1 = (((hix + r).ceil().max(0.0) as usize) + 1).min(w);
    let y1 = (((hiy + r).ceil().max(0.0) as usize) + 1).min(h);
    (x0.min(x1), y0.min(y1), x1, y1)
}

/// Move a recovered stroke onto the coverage it was measured from.
///
/// The centreline arrives from thinning and a parabolic ridge fit, which are
/// statements about the *distance field* and never about the rendered result. A
/// filled boundary gets `boundary_opt`, a least-squares solve against the exact
/// pixel coverage, and letting that solve actually converge was worth 2% of the
/// whole objective. A stroke got nothing equivalent, which is why its traces score
/// dE00 0.19 where the filled path scores 0.13 on the same icons.
///
/// This is the missing solve, in the same shape: one degree of freedom per point,
/// along the local normal, plus the width. Tangential motion is not offered
/// because it is not observable -- sliding a point along its own centreline
/// changes no coverage -- which is the same reason `boundary_opt` offers only
/// normal motion.
///
/// Coordinate descent with a finite-difference Newton step. Each point's influence
/// is local, so its cost is evaluated over a window around it rather than over the
/// image, which keeps a pass linear in the number of points.
pub fn refine_to_coverage(strokes: &mut [Stroke], coverage: &CoverageField, rounds: usize) {
    let (w, h) = (coverage.width, coverage.height);
    if w == 0 || h == 0 || coverage.data.len() < w * h {
        return;
    }
    for si in 0..strokes.len() {
        if strokes[si].path.len() < 2 {
            continue;
        }
        let mut st = strokes[si].clone();
        let hw0 = st.width * 0.5;
        let mut hw = hw0;
        let mut moved = vec![0.0f64; st.path.len()];

        for _ in 0..rounds {
            // Width first: it is one number for the whole stroke, and the points
            // are then fitted against whatever it currently says.
            let (bx0, by0, bx1, by1) = stroke_window(&st.path, hw, w, h);
            let ob = others_cover(strokes, si, bx0, by0, bx1, by1);
            let base = stroke_cost(coverage, &ob, &st.path, st.closed, hw, bx0, by0, bx1, by1);
            let up = stroke_cost(
                coverage,
                &ob,
                &st.path,
                st.closed,
                hw + REFINE_H,
                bx0,
                by0,
                bx1,
                by1,
            );
            let dn = stroke_cost(
                coverage,
                &ob,
                &st.path,
                st.closed,
                hw - REFINE_H,
                bx0,
                by0,
                bx1,
                by1,
            );
            let g = (up - dn) / (2.0 * REFINE_H);
            let curv = (up - 2.0 * base + dn) / (REFINE_H * REFINE_H);
            if curv > 1e-9 {
                hw = (hw - g / curv).clamp(hw0 - REFINE_MAX, hw0 + REFINE_MAX);
            }

            for i in 0..st.path.len() {
                let nrm = normal_at(&st.path, i, st.closed);
                if nrm.x == 0.0 && nrm.y == 0.0 {
                    continue;
                }
                let (x0, y0, x1, y1) = point_window(st.path[i], hw, w, h);
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let orig = st.path[i];
                let at = |d: f64| Point::new(orig.x + nrm.x * d, orig.y + nrm.y * d);

                // Neighbouring offsets, so the smoothness term can be priced
                // alongside the coverage one. An endpoint has one neighbour and is
                // left to the data.
                let n_pts = st.path.len();
                let nb = if st.closed && n_pts > 2 {
                    Some((moved[(i + n_pts - 1) % n_pts], moved[(i + 1) % n_pts]))
                } else if i > 0 && i + 1 < n_pts {
                    Some((moved[i - 1], moved[i + 1]))
                } else {
                    None
                };
                let kink = |u: f64| -> f64 {
                    match nb {
                        Some((a, b)) => {
                            let d = u - 0.5 * (a + b);
                            REFINE_KINK * d * d
                        }
                        None => 0.0,
                    }
                };

                let op = others_cover(strokes, si, x0, y0, x1, y1);
                let c0 = stroke_cost(coverage, &op, &st.path, st.closed, hw, x0, y0, x1, y1)
                    + kink(moved[i]);
                st.path[i] = at(REFINE_H);
                let cp = stroke_cost(coverage, &op, &st.path, st.closed, hw, x0, y0, x1, y1)
                    + kink(moved[i] + REFINE_H);
                st.path[i] = at(-REFINE_H);
                let cm = stroke_cost(coverage, &op, &st.path, st.closed, hw, x0, y0, x1, y1)
                    + kink(moved[i] - REFINE_H);
                st.path[i] = orig;

                let g = (cp - cm) / (2.0 * REFINE_H);
                let curv = (cp - 2.0 * c0 + cm) / (REFINE_H * REFINE_H);
                if curv <= 1e-9 {
                    continue;
                }
                // A Newton step on a ramp overshoots badly where the window is
                // nearly saturated, so cap the step, and cap the total travel from
                // where the ridge put the point.
                let want =
                    (moved[i] + (-g / curv).clamp(-0.35, 0.35)).clamp(-REFINE_MAX, REFINE_MAX);
                if (want - moved[i]).abs() > 1e-6 {
                    st.path[i] = Point::new(orig.x + nrm.x * want, orig.y + nrm.y * want);
                    moved[i] = want;
                }
            }
        }
        if std::env::var_os("INKVEC_REFINEDBG").is_some() {
            let tot: f64 = moved.iter().map(|d| d.abs()).sum();
            eprintln!(
                "  [refine] stroke {si}: {} pts, closed {}, width {:.3} -> {:.3},                  moved {:.4} px total, max {:.4}",
                st.path.len(),
                st.closed,
                hw0 * 2.0,
                hw * 2.0,
                tot,
                moved.iter().fold(0.0f64, |a, b| a.max(b.abs()))
            );
        }
        st.width = hw * 2.0;
        strokes[si] = st;
    }
}

// ---------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------
// Sub-pixel ridge, width, and the stroke decision
// ---------------------------------------------------------------------------------

fn analyse_region(
    cov: &CoverageField,
    g: &RegionGrid,
    label: u16,
    criteria: Criteria,
) -> Option<Vec<Stroke>> {
    let ls = LevelSet::build(g);
    if ls.segs.is_empty() {
        return None;
    }
    let skel = zhang_suen(&g.mask, g.w, g.h);
    let skel = prune_spurs(skel, g, &ls, g.w, g.h, criteria.spur_factor);
    let chains = geometric_chains(&skel, g, &ls, g.w, g.h);
    if chains.is_empty() {
        return None;
    }

    let min_aspect = if chains.len() > 1 {
        criteria.min_edge_aspect.min(MIN_ASPECT)
    } else {
        MIN_ASPECT
    };
    let mut strokes = Vec::new();
    let mut explained = 0.0f64;
    for c in &chains {
        if let Some(s) = measure_stroke(cov, &ls, c, label, min_aspect) {
            let l = s.length();
            explained += l * s.width;
            if !s.closed {
                // Round caps: a Minkowski sum of the centreline with a disc adds one
                // whole disc however many vertices the polyline has, so half a disc per
                // free end.
                explained += 0.5 * std::f64::consts::FRAC_PI_4 * s.width * s.width;
            }
            strokes.push(s);
        }
    }
    if strokes.is_empty() {
        return None;
    }
    if min_aspect < MIN_ASPECT {
        // The aspect test, moved from the edge to the region.
        let mut widths: Vec<f64> = strokes.iter().map(|s| s.width).collect();
        let length: f64 = strokes.iter().map(Stroke::length).sum();
        if length / median(&mut widths) < MIN_ASPECT {
            return None;
        }
    }

    let area = g.area();
    if area <= 0.0 {
        return None;
    }
    let ratio = explained / area;
    if !(AREA_EXPLAINED_MIN..=AREA_EXPLAINED_MAX).contains(&ratio) {
        return None;
    }
    Some(strokes)
}

fn measure_stroke(
    cov: &CoverageField,
    ls: &LevelSet,
    chain: &RawChain,
    label: u16,
    min_aspect: f64,
) -> Option<Stroke> {
    let n = chain.pts.len();
    if n < 3 {
        return None;
    }

    // Preliminary width sets the scale of everything below: the ridge sampling offset,
    // the end trim, the aspect test.
    let mut prelim: Vec<f64> = chain.pts.iter().map(|&p| 2.0 * ls.nearest(p).0).collect();
    let w_prelim = median(&mut prelim);
    if !(w_prelim.is_finite() && w_prelim > 0.3) {
        return None;
    }

    // Sample offset stays strictly inside the stroke (`< width/2`) so the three samples
    // straddle the ridge rather than the far boundary.
    let step = (0.45 * w_prelim).clamp(0.35, 1.0);
    let bound = (MAX_RIDGE_SHIFT * w_prelim).max(MIN_RIDGE_SHIFT);
    let pts = refine_ridge(ls, &chain.pts, chain.closed, step, bound);

    // Width and its measurement sigma at every centreline point.
    let mut widths = Vec::with_capacity(n);
    let mut wsig = Vec::with_capacity(n);
    let mut psig = Vec::with_capacity(n);
    for &p in &pts {
        let (d, b1) = ls.nearest(p);
        // The far side of the stroke: reflect through the centreline, then snap back to
        // the level set. The snap is what makes this safe at a cap, where the nearest
        // boundary is the cap itself and the naive reflection lands in the *interior* —
        // where the coverage gradient is zero and `position_sigma` correctly, and
        // uselessly, reports that a boundary there is unlocalizable.
        let b2 = ls.nearest(Point::new(2.0 * p.x - b1.x, 2.0 * p.y - b1.y)).1;
        let (s1, s2) = (cov.position_sigma(b1), cov.position_sigma(b2));
        widths.push(2.0 * d);
        // A width is the sum of two boundary offsets; a centreline is their midpoint.
        wsig.push((s1 * s1 + s2 * s2).sqrt());
        psig.push(0.5 * (s1 * s1 + s2 * s2).sqrt());
    }

    // Measure width away from the ends. The field necessarily dips into a cap and bulges
    // into a junction, so including either reports an end effect as a width variation.
    let keep = interior_samples(&pts, chain.closed, w_prelim);
    let mut w_int: Vec<f64> = keep.iter().map(|&k| widths[k]).collect();
    let width = median(&mut w_int);
    let spread = robust_spread(&w_int, width);
    let mut sig_int: Vec<f64> = keep.iter().map(|&k| wsig[k]).collect();
    let sigma_meas = median(&mut sig_int).max(WIDTH_SIGMA_FLOOR);

    let length = polyline_length(&pts, chain.closed);
    if width <= 0.0 || length / width < min_aspect {
        return None;
    }
    // Is the variation more than the measurement can explain?
    if spread > TAU_WIDTH * sigma_meas {
        return None;
    }

    let sigma: Vec<f64> = (0..pts.len())
        .map(|k| inflate_for_curvature(&pts, k, psig[k].max(1e-3), chain.closed))
        .collect();
    let width_sigma = (spread / (w_int.len().max(1) as f64).sqrt()).hypot(WIDTH_SIGMA_FLOOR);

    Some(Stroke {
        path: pts,
        sigma,
        width,
        width_sigma,
        closed: chain.closed,
        label,
    })
}

/// Move every point onto the sub-pixel ridge of the distance field along its own normal.
///
/// This is the same move S2 makes for a boundary — a boundary is where coverage crosses
/// a level, a centreline is where distance is locally maximal across the stroke — and it
/// is where the sub-pixel accuracy comes from. Thinning only ever returns pixel centres,
/// so without this step the centreline is quantised to half a pixel and the width with
/// it.
///
/// Three samples, parabolic vertex, **iterated**. The iteration is not a safety net: a
/// distance field near its ridge is `Dmax - |t - delta|`, and the parabolic vertex of
/// three samples of a tent lands at `delta/2` rather than at `delta`. One step would
/// leave half the error behind; the iteration is a geometric sequence that removes it.
fn refine_ridge(ls: &LevelSet, pts: &[Point], closed: bool, step: f64, bound: f64) -> Vec<Point> {
    let n = pts.len();
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let t = tangent(pts, k, closed);
        let nrm = Vec2 { x: -t.y, y: t.x };
        let mut p = pts[k];
        let mut shift = 0.0f64;
        for _ in 0..RIDGE_ITERS {
            let sample = |o: f64| ls.nearest(Point::new(p.x + nrm.x * o, p.y + nrm.y * o)).0;
            let (ym, y0, yp) = (sample(-step), sample(0.0), sample(step));
            let denom = ym - 2.0 * y0 + yp;
            let delta = if denom < -1e-9 {
                (0.5 * step * (ym - yp) / denom).clamp(-step, step)
            } else {
                // The three samples do not bracket a crease, so the parabola would be
                // extrapolating. This is not a rare degeneracy — it is what a sharp join
                // looks like, where thinning leaves the vertex short of the apex and the
                // ridge is a couple of pixels away rather than a fraction of one. Scan
                // for the maximum instead and let the next iteration refine it.
                scan_for_max(&sample, y0, bound.min(2.0 * step + 1.0))
            };
            if !delta.is_finite() || delta == 0.0 {
                break;
            }
            if (shift + delta).abs() > bound {
                break;
            }
            shift += delta;
            p = Point::new(p.x + nrm.x * delta, p.y + nrm.y * delta);
            if delta.abs() < 1e-6 {
                break;
            }
        }
        out.push(p);
    }
    out
}

/// Coarse search for the largest sample within `+-reach`, returning its offset, or zero
/// if nothing beats the value already at the current point.
fn scan_for_max(sample: &impl Fn(f64) -> f64, here: f64, reach: f64) -> f64 {
    let mut best = here;
    let mut best_o = 0.0;
    for i in 1..=SCAN_SAMPLES {
        let o = reach * i as f64 / SCAN_SAMPLES as f64;
        for signed in [-o, o] {
            let v = sample(signed);
            if v > best {
                best = v;
                best_o = signed;
            }
        }
    }
    best_o
}

fn tangent(pts: &[Point], k: usize, closed: bool) -> Vec2 {
    let n = pts.len() as i64;
    let idx = |d: i64| -> Point {
        let i = k as i64 + d;
        let i = if closed {
            i.rem_euclid(n)
        } else {
            i.clamp(0, n - 1)
        };
        pts[i as usize]
    };
    let w = TANGENT_WINDOW as i64;
    let mut t = idx(w) - idx(-w);
    if t.norm() < 1e-9 {
        t = idx(1) - idx(-1);
    }
    let l = t.norm();
    if l < 1e-9 {
        Vec2 { x: 1.0, y: 0.0 }
    } else {
        Vec2 {
            x: t.x / l,
            y: t.y / l,
        }
    }
}

/// Indices at least one width in from either free end.
fn interior_samples(pts: &[Point], closed: bool, width: f64) -> Vec<usize> {
    let n = pts.len();
    if closed {
        return (0..n).collect();
    }
    let mut s = vec![0.0; n];
    for k in 1..n {
        s[k] = s[k - 1] + pts[k - 1].dist(pts[k]);
    }
    let total = s[n - 1];
    let trim = width;
    let keep: Vec<usize> = (0..n)
        .filter(|&k| s[k] >= trim && s[k] <= total - trim)
        .collect();
    if keep.len() >= 3 {
        keep
    } else {
        // Too short to trim a width off each end: fall back to the middle third, which
        // still excludes the caps.
        let a = n / 3;
        let b = (2 * n / 3).max(a + 1);
        (a..b.min(n)).collect()
    }
}

fn polyline_length(pts: &[Point], closed: bool) -> f64 {
    let n = pts.len();
    if n < 2 {
        return 0.0;
    }
    let mut l: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    if closed {
        l += pts[n - 1].dist(pts[0]);
    }
    l
}

pub(crate) fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Median absolute deviation, scaled to a Gaussian sigma. Robust because a single
/// junction sample that escaped the trim should not decide whether a stroke is
/// constant-width.
fn robust_spread(v: &[f64], centre: f64) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let mut dev: Vec<f64> = v.iter().map(|x| (x - centre).abs()).collect();
    1.4826 * median(&mut dev)
}
