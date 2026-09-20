//! Sub-pixel contour extraction from a coverage field.
//!
//! The boundary of the shape is the `alpha = 0.5` level set of the coverage field, and
//! marching squares with linear interpolation along cell edges locates it to a fraction
//! of a pixel. That is the whole trick: a thresholding tracer rounds each pixel to
//! inside-or-outside and then has only integer corners to work with, whereas the
//! crossing position along a cell edge is a continuous quantity carrying roughly a
//! tenth-of-a-pixel of real information.
//!
//! Crossings are identified by the **grid edge they lie on**, not by their coordinates.
//! Two cells sharing an edge therefore refer to the same crossing by construction, so
//! linking segments into closed contours is exact rather than a matter of comparing
//! floating-point positions within a tolerance. It is the same reasoning as the exact
//! predicates in `inkvec-core`: topology should not be decided by an epsilon.

use std::collections::HashMap;

use inkvec_core::{Point, Polyline};

use crate::coverage::CoverageField;

/// Coverage level set that marks the shape boundary.
pub const LEVEL: f32 = 0.5;

/// Grid of coverage samples, padded with background so every contour closes.
struct Grid {
    w: usize, // corners across, = image width + 2
    h: usize,
    v: Vec<f32>,
}

impl Grid {
    fn from_field(f: &CoverageField) -> Self {
        let (w, h) = (f.width + 2, f.height + 2);
        let mut v = vec![0.0f32; w * h];
        for y in 0..f.height {
            for x in 0..f.width {
                v[(y + 1) * w + (x + 1)] = f.get(x, y);
            }
        }
        Grid { w, h, v }
    }

    #[inline]
    fn at(&self, i: usize, j: usize) -> f32 {
        self.v[j * self.w + i]
    }

    #[inline]
    fn inside(&self, i: usize, j: usize) -> bool {
        self.at(i, j) >= LEVEL
    }

    /// Identifier of the horizontal grid edge from corner `(i, j)` to `(i+1, j)`.
    #[inline]
    fn h_id(&self, i: usize, j: usize) -> u64 {
        (j * (self.w - 1) + i) as u64
    }

    /// Identifier of the vertical grid edge from corner `(i, j)` to `(i, j+1)`.
    #[inline]
    fn v_id(&self, i: usize, j: usize) -> u64 {
        ((self.w - 1) * self.h + i * (self.h - 1) + j) as u64
    }

    /// Sub-pixel position of the level crossing on a horizontal edge.
    ///
    /// Coordinates are in image space: the grid is padded by one, so corner `(i, j)`
    /// sits at pixel centre `(i - 1, j - 1)`.
    fn h_point(&self, i: usize, j: usize) -> Point {
        let (a, b) = (self.at(i, j), self.at(i + 1, j));
        Point::new(i as f64 - 1.0 + lerp_t(a, b), j as f64 - 1.0)
    }

    fn v_point(&self, i: usize, j: usize) -> Point {
        let (a, b) = (self.at(i, j), self.at(i, j + 1));
        Point::new(i as f64 - 1.0, j as f64 - 1.0 + lerp_t(a, b))
    }
}

/// Where between two samples the level sits. This is the sub-pixel information a
/// thresholding tracer throws away.
#[inline]
fn lerp_t(a: f32, b: f32) -> f64 {
    let d = b - a;
    if d.abs() < 1e-12 {
        return 0.5;
    }
    (((LEVEL - a) / d) as f64).clamp(0.0, 1.0)
}

/// Which cell edge a crossing lies on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

/// Marching-squares case table, oriented consistently so that the interior stays on the
/// same side of every emitted segment. Cases 5 and 10 are the saddles and are resolved
/// separately from the cell centre.
fn segments_for(case: u8) -> &'static [(Side, Side)] {
    use Side::*;
    match case {
        1 => &[(Left, Top)],
        2 => &[(Top, Right)],
        3 => &[(Left, Right)],
        4 => &[(Right, Bottom)],
        6 => &[(Top, Bottom)],
        7 => &[(Left, Bottom)],
        8 => &[(Bottom, Left)],
        9 => &[(Bottom, Top)],
        11 => &[(Bottom, Right)],
        12 => &[(Right, Left)],
        13 => &[(Right, Top)],
        14 => &[(Top, Left)],
        _ => &[],
    }
}

/// Trace every closed contour at the 0.5 level, with sub-pixel vertex positions and a
/// per-vertex positional uncertainty read from the coverage gradient.
pub fn trace(field: &CoverageField) -> Vec<Polyline> {
    let g = Grid::from_field(field);

    // edge id -> (next edge id, position of this crossing)
    let mut next: HashMap<u64, (u64, Point)> = HashMap::new();

    for j in 0..g.h - 1 {
        for i in 0..g.w - 1 {
            let case = (g.inside(i, j) as u8)
                | ((g.inside(i + 1, j) as u8) << 1)
                | ((g.inside(i + 1, j + 1) as u8) << 2)
                | ((g.inside(i, j + 1) as u8) << 3);

            let center = 0.25 * (g.at(i, j) + g.at(i + 1, j) + g.at(i + 1, j + 1) + g.at(i, j + 1));
            let saddle_inside = center >= LEVEL;

            let segs: &[(Side, Side)] = match case {
                // Saddles: the cell centre decides whether the two inside corners are
                // joined or separated. Choosing arbitrarily here produces contours that
                // pinch across a diagonal and then fail to close.
                5 => {
                    if saddle_inside {
                        &[(Side::Left, Side::Bottom), (Side::Right, Side::Top)]
                    } else {
                        &[(Side::Left, Side::Top), (Side::Right, Side::Bottom)]
                    }
                }
                10 => {
                    if saddle_inside {
                        &[(Side::Top, Side::Right), (Side::Bottom, Side::Left)]
                    } else {
                        &[(Side::Top, Side::Left), (Side::Bottom, Side::Right)]
                    }
                }
                c => segments_for(c),
            };

            for &(from, to) in segs {
                let (fid, fpt) = side_edge(&g, i, j, from);
                let (tid, _) = side_edge(&g, i, j, to);
                next.insert(fid, (tid, fpt));
            }
        }
    }

    // Walk the successor map into closed loops.
    let mut visited: HashMap<u64, bool> = HashMap::new();
    let mut out = Vec::new();

    // Sorted: an identical input must give an identical file, and the order contours
    // are walked in decides their order in the output.
    let mut starts: Vec<u64> = next.keys().copied().collect();
    starts.sort_unstable();
    for start in starts {
        if visited.contains_key(&start) {
            continue;
        }
        let mut pts = Vec::new();
        let mut cur = start;
        loop {
            if visited.contains_key(&cur) {
                break;
            }
            visited.insert(cur, true);
            let Some(&(nxt, p)) = next.get(&cur) else {
                break;
            };
            pts.push(p);
            cur = nxt;
            if cur == start {
                break;
            }
        }
        if pts.len() >= 3 {
            let sigma = sigmas_for(field, &pts);
            out.push(Polyline::new(pts, sigma, true));
        }
    }
    out
}

fn side_edge(g: &Grid, i: usize, j: usize, s: Side) -> (u64, Point) {
    match s {
        Side::Top => (g.h_id(i, j), g.h_point(i, j)),
        Side::Bottom => (g.h_id(i, j + 1), g.h_point(i, j + 1)),
        Side::Left => (g.v_id(i, j), g.v_point(i, j)),
        Side::Right => (g.v_id(i + 1, j), g.v_point(i + 1, j)),
    }
}

/// Window radius, in points, over which local linearity is assessed.
const LINEARITY_WINDOW: usize = 3;

/// Scale factor from measured local non-linearity to positional uncertainty.
///
/// Chosen by sweep against both corpora, because the two disagree about what this term
/// is for. On synthetic circles and polygons, local non-linearity really does indicate a
/// corner the level set could not represent, and a large gain helps. On real emoji it
/// mostly indicates *detail*, and a large gain reports a well-measured boundary as badly
/// measured and flattens it — at 1.2 that cost DISTS@1x 0.117 on real content against
/// 0.092 here, while gaining nothing on synthetic.
///
/// Measured: gain 1.2 / 0.6 / 0.35 / 0.0 gives real DISTS 0.117 / 0.101 / 0.092 / 0.091
/// and synthetic 0.0048 / 0.0041 / 0.0038 / 0.0057. Zero is worse on both and inflates
/// the parameter count by half, so the term earns its place; it was simply too strong.
const NONLINEARITY_GAIN: f64 = 0.35;

/// Overridable for experiments with `INKVEC_CURV_GAIN`. Inflating sigma where the boundary
/// curves is what lets a straight segment through a bend look statistically acceptable, so
/// it is a suspect in why curves come out faceted.
fn curv_gain() -> f64 {
    static V: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("INKVEC_CURV_GAIN")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(NONLINEARITY_GAIN)
    })
}

/// Largest positional uncertainty local non-linearity may imply, in pixels.
///
/// This bound is physical, not a tuning knob. The systematic error being corrected for
/// is that a level set on a pixel grid cannot represent a sharp corner — it bevels it —
/// and the size of that bevel is bounded by the pixel: at worst about half a diagonal,
/// whatever the corner angle. Non-linearity beyond that cannot be explained as corner
/// rounding, so it is *signal*.
///
/// Without the cap the inflation is unbounded, and a genuinely detailed boundary reports
/// itself as badly measured and gets flattened. That never showed on the synthetic
/// corpus, whose shapes are circles and polygons where high curvature really does mean a
/// corner artefact; on real emoji it cost DISTS@1x 0.117 against 0.090.
const MAX_CURVATURE_SIGMA: f64 = 0.354;

/// Per-point positional uncertainty: measurement noise combined with the *systematic*
/// error of level-set extraction, which is not uniform along a boundary.
///
/// On a straight run, marching squares locates the boundary to about 0.05px (measured;
/// see the `straightedge` example). At a sharp corner it cannot: the level set cuts the
/// corner pixel and returns a small bevel instead of a point. Reporting a uniform 0.05px
/// tells the fitter it knows the corner ten times better than it does, and the fitter's
/// rational response is to spend a whole extra segment crossing the bevel — measured
/// effect, a hexagon coming back as 12 segments with six ~1px chamfers. That is the
/// fitter being right about a model that was lying to it.
///
/// The indicator is **local non-linearity**: fit a line to a small window around each
/// point and take that point's residual. Two properties make it the right choice.
///
/// * It measures the thing that actually drives the systematic error — how far the
///   boundary departs from locally straight at the scale the reconstruction works at.
/// * It is robust where a turning-angle test is not. Per-point turn reaches 18 degrees on
///   *straight* runs purely from staircase wobble, which is the same magnitude as a real
///   corner and makes it useless as a discriminator. Averaged over a window, the wobble
///   cancels and the corner does not.
///
/// A gently curved boundary is barely affected: a circle of radius 46px has a sagitta of
/// about 0.02px over a 3px window, so curves keep their fine tolerance and only genuine
/// corners are treated as poorly localized — which is what they are.
fn sigmas_for(field: &CoverageField, pts: &[Point]) -> Vec<f64> {
    let n = pts.len();
    let floor = sigma_floor();
    (0..n)
        .map(|k| {
            let base = field.position_sigma(pts[k]).max(floor);
            inflate_for_curvature(pts, k, base, true)
        })
        .collect()
}

/// Smallest uncertainty a boundary point may claim, in pixels.
///
/// Both front ends need this and there is one copy: the bilevel path reads it here, the
/// planar path through [`crate::planar`], and a floor that meant two different things in
/// the two would be a knob that could not be swept.
///
/// Disabled by default: coverage-derived positions can be more precise than whole
/// pixel coordinates. Applying a universal `1/sqrt(12)` pixel floor oversmooths
/// clean small artwork.
///
/// `INKVEC_SIGMA_FLOOR` remains an experimental, nonnegative positional floor.
/// Replace every per-point sigma with one constant, destroying all of its structure.
///
/// This exists to price that structure, and the price is low. `Edge.sigma` is the one
/// number the whole tree claims to derive its tolerances from — "nothing downstream
/// invents its own tolerance parameter; it reads this one"
/// (`docs/algorithm/00-overview.md`) — but measured over 86,060 boundary points on the
/// gate set, **79% of them sit between 0.050 and 0.060**, pinned at or just above
/// [`crate::coverage::DEFAULT_SIGMA_MODEL`]. Median 0.0512, p25 0.0501, p75 0.0576;
/// only 5.5% exceed 0.10. For four fifths of the corpus sigma is not a measurement, it
/// is that constant.
///
/// Flattening it to the median and re-running the 246-icon gate costs **+1.56% turning
/// and +0.89% ratio, and improves dE00 by 0.45%**. So the whole per-point structure —
/// the coverage inversion, the contrast division, the gradient division, the curvature
/// inflation — is worth about one percent on two axes and is slightly negative on the
/// third. What little it does buy comes from the thin tail that correctly marks a faint
/// boundary as not worth coordinates, not from the variation in the bulk.
///
/// The consequence for anything that would revise sigma downstream (recomputing it
/// after `boundary_opt`, say, which never does): the quantity being improved carries
/// about a percent of value, so the improvement is bounded by that. The *level* of
/// sigma matters enormously by comparison, and it is not a separate lever — scaling
/// every sigma by `k` scales chi2 by `1/k²`, which is exactly `lambda -> k²·lambda`.
/// Measured rather than assumed: sigma_model 0.10 gives dE00 +39.00% / ratio -10.28%
/// and `--lambda-scale 4.0` gives +44.34% / -11.68%, the same frontier to three
/// significant figures (ratio per dE00, 0.264 against 0.263). Use `--lambda-scale`.
pub fn sigma_flat() -> Option<f64> {
    static V: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("INKVEC_SIGMA_FLAT")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
    })
}

/// The experimental positional floor from `INKVEC_SIGMA_FLOOR`; `0.0`, meaning no floor,
/// unless it is set.
///
/// Applied as `.max(sigma_floor())` to a per-point positional sigma on both front ends --
/// `sigmas_for` here and the planar path in `planar.rs` -- so it bounds how certain any one
/// point is allowed to claim to be. Weight in chi2 goes as `1/sigma²`, so a sigma near zero
/// lets a single point outvote the rest of its boundary; a floor caps that leverage without
/// disturbing the rest of the distribution.
///
/// Read once and cached. A value that does not parse, is not finite, or is negative is
/// ignored rather than refused, as with `sigma_flat` above.
pub fn sigma_floor() -> f64 {
    static V: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("INKVEC_SIGMA_FLOOR")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(0.0)
    })
}

/// Combine a base positional uncertainty with the local non-linearity of the contour.
///
/// Shared by both front ends. The bilevel path and the planar path measure boundaries
/// differently, but both extract them as level sets on a pixel grid, and both are
/// therefore systematically wrong in the same place and for the same reason: wherever the
/// boundary is not locally straight at the scale of the reconstruction.
///
/// Omitting this on the planar path was a real defect. A ring of radius 11px came back as
/// 52 segments from 96 measured points — almost no simplification — because every point
/// claimed 0.05px accuracy that the reconstruction could not deliver at that curvature.
pub fn inflate_for_curvature(pts: &[Point], k: usize, base: f64, wrap: bool) -> f64 {
    let n = pts.len();
    let w = LINEARITY_WINDOW;
    {
        {
            if n < 2 * w + 3 {
                return base;
            }
            // Total-least-squares line over the window, then this point's residual.
            let idx = |d: i64| {
                let i = k as i64 + d;
                let i = if wrap {
                    i.rem_euclid(n as i64)
                } else {
                    i.clamp(0, n as i64 - 1)
                };
                pts[i as usize]
            };
            let m = (2 * w + 1) as f64;
            let (mut mx, mut my) = (0.0, 0.0);
            for d in -(w as i64)..=(w as i64) {
                let q = idx(d);
                mx += q.x;
                my += q.y;
            }
            mx /= m;
            my /= m;
            let (mut cxx, mut cyy, mut cxy) = (0.0, 0.0, 0.0);
            for d in -(w as i64)..=(w as i64) {
                let q = idx(d);
                let (dx, dy) = (q.x - mx, q.y - my);
                cxx += dx * dx;
                cyy += dy * dy;
                cxy += dx * dy;
            }
            let tr = cxx + cyy;
            let diff = cxx - cyy;
            let disc = (diff * diff + 4.0 * cxy * cxy).max(0.0).sqrt();
            let major = 0.5 * (tr + disc);
            let (ux, uy) = if cxy.abs() > 1e-12 {
                (major - cyy, cxy)
            } else if cxx >= cyy {
                (1.0, 0.0)
            } else {
                (0.0, 1.0)
            };
            let nrm = ux.hypot(uy);
            if nrm <= 1e-12 {
                return base;
            }
            let (ux, uy) = (ux / nrm, uy / nrm);
            let (dx, dy) = (pts[k].x - mx, pts[k].y - my);
            let residual = (dx * uy - dy * ux).abs();

            // Only the part of that residual which is *not* consistent turning counts as
            // uncertainty.
            //
            // The residual alone cannot tell a rasterised straight edge from a genuine
            // curve: both depart from a local straight line. But they depart differently.
            // A staircase alternates — left, right, left — so its turns cancel. An arc
            // turns the same way at every step. Comparing the signed sum of the turns
            // against the sum of their magnitudes separates the two with no threshold to
            // pick: the ratio is 1 for a pure arc and 0 for a pure staircase.
            //
            // Inflating on genuine curvature is not a harmless over-estimate. It tells the
            // fitter accuracy does not matter exactly where a viewer looks hardest, and
            // measurably so: on an 11px corner, arc sigma 0.35 fits four cubics and 0.45
            // fits eight straight chords instead — visible faceting on every rounded
            // rectangle, which is most icons. See `inkvec-fit/examples/corner_fit.rs`.
            //
            // The residual cannot be used raw for another reason: the total-least-squares
            // line passes through the window's centroid, so the signed residuals over the
            // window sum to zero by construction and their mean carries no information.
            let (mut turn_sum, mut turn_abs) = (0.0f64, 0.0f64);
            for d in -(w as i64 - 1)..=(w as i64 - 1) {
                let (a, b, c) = (idx(d - 1), idx(d), idx(d + 1));
                let cross = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
                turn_sum += cross;
                turn_abs += cross.abs();
            }
            let consistency = if turn_abs > 1e-12 {
                (turn_sum.abs() / turn_abs).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let wobble = residual * (1.0 - consistency);

            base.hypot((curv_gain() * wobble).min(MAX_CURVATURE_SIGMA))
        }
    }
}

/// Signed area of a closed polyline (shoelace). Positive is counter-clockwise in a
/// y-down image coordinate system.
pub fn signed_area(poly: &Polyline) -> f64 {
    let p = &poly.points;
    let n = p.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for k in 0..n {
        let q = p[(k + 1) % n];
        a += p[k].x * q.y - q.x * p[k].y;
    }
    0.5 * a
}
