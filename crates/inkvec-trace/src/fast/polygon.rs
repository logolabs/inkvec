//! The optimal polygon of a measured boundary: the fewest straight sides that stay within a
//! tolerance of every point, and among those the one closest to the points.
//!
//! This is the first stage of Selinger's Potrace (2003, section 2.2), restated for sub-pixel
//! input. Potrace asks whether a run of *pixel corners* is straight by an L-infinity test
//! against the integer lattice; the points here have already been moved to their sub-pixel
//! positions, so the question becomes metric: does every point of the run lie within `tol`
//! of the chord between its ends? The run is scanned with an incremental cone of admissible
//! directions (each point narrows it by the angle it subtends at `tol`), which keeps the
//! test O(1) per candidate side, and the dynamic program over sides is Potrace's: fewest
//! sides first, then the smallest sum of squared distances, read in O(1) from prefix sums.
//!
//! In the field's terms this is the min-# problem of polygonal approximation: the fewest
//! segments within a tolerance of every point, stated by Imai, H. & Iri, M. (1986),
//! "Computational-geometric methods for polygonal approximations of a curve", *Computer
//! Vision, Graphics, and Image Processing* 36:31–41,
//! doi:10.1016/S0734-189X(86)80027-5, as a shortest path over the admissible sides, and
//! solved in O(n²) by Chan, W. S. & Chin, F. (1996), "Approximation of polygonal curves
//! with minimum number of line segments or minimum error", *IJCGA* 6:59–77,
//! doi:10.1142/S0218195996000058. The spans here are capped at [`MAX_SPAN`] points, so
//! the program is O(n · MAX_SPAN). The cone test is the cone-intersection idea of the
//! greedy scan-along fitters -- Williams, C. M. (1978), "An efficient algorithm for the
//! piecewise linear approximation of planar curves", *CGIP* 8:286–293; Sklansky, J. &
//! Gonzalez, V. (1980), "Fast polygonal approximation of digitized curves", *Pattern
//! Recognition* 12:327–331, doi:10.1016/0031-3203(80)90031-X -- used here only to decide
//! which sides are admissible. The greedy fitters themselves are not used: they are
//! faster but not min-#, so the polygon, and the drawing, would change. Neither is
//! Agarwal, P. K. & Varadarajan, K. R. (2000), "Efficient algorithms for approximating
//! polygonal chains", *Discrete Comput. Geom.* 23:273–291, doi:10.1007/PL00009500, whose
//! subquadratic bound is for a different error metric.
//!
//! # The passes, in order
//!
//! 1. Prefix sums of the points' coordinates and their products ([`Sums`]), and the
//!    lattice runs, the stretches of equal steps ([`lattice_runs`]); one pass each.
//! 2. For each anchor `i` in index order, unless the dynamic program has fathomed it (see
//!    [`open`]): scan `j = i+1, i+2, …` while the cone of directions from `p_i` that pass
//!    within `tol` of every point seen is not empty, at most [`MAX_SPAN`] points on.
//!    Every `j` whose direction lies in the cone is an admissible side `i → j`, and
//!    relaxes `best[j]` ([`Table::offer`]). Where the anchor's next points are one
//!    lattice run, the scan jumps to the run's end in closed form, offering the same
//!    sides in the same order.
//! 3. The polygon is read back from the last point along the winning predecessors.
//!
//! A closed ring ([`closed`]) is cut at its sharpest point and solved as an open run from
//! there back to it.
//!
//! Stage 5a of the Fast fit, run by [`super::fit_points`] after [`super::smooth::denoise`].
//! In: one boundary's sub-pixel points, in px. Out: the indices of the polygon's vertices,
//! which [`super::smooth::adjust_vertices`] then moves off the points.

use inkvec_core::{Point, Vec2};

/// Running sums of the points' coordinates, relative to one origin, so the sum of squared
/// distances from any run of points to any line costs O(1).
struct Sums {
    origin: Point,
    /// Prefix sums of x, y, x^2, x*y, y^2; entry `k` covers points `0..k`.
    s: Vec<[f64; 5]>,
}

impl Sums {
    /// Prefix sums over `pts`, taken relative to the first point so that the squares of
    /// large coordinates (a 2048 px image) do not swamp the small differences
    /// [`Sums::sq_dist`] subtracts.
    fn new(pts: &[Point]) -> Self {
        let origin = pts.first().copied().unwrap_or(Point::new(0.0, 0.0));
        let mut s = Vec::with_capacity(pts.len() + 1);
        let mut acc = [0.0f64; 5];
        s.push(acc);
        for p in pts {
            let (x, y) = (p.x - origin.x, p.y - origin.y);
            acc[0] += x;
            acc[1] += y;
            acc[2] += x * x;
            acc[3] += x * y;
            acc[4] += y * y;
            s.push(acc);
        }
        Self { origin, s }
    }

    /// Sum of squared distances from points `lo..hi` to the line through `a` and `b`.
    ///
    /// With `n = (u, v) = (−Δy, Δx)` the (unnormalised) normal of `b − a` and
    /// `c = −n · (a − origin)`, the signed distance of a point `p` times `|b − a|` is
    /// `u x + v y + c`. Expanding its square and summing over the run:
    ///
    /// `Σ d² = (u² Σx² + v² Σy² + 2uv Σxy + 2uc Σx + 2vc Σy + c² m) / |b − a|²`
    ///
    /// with every `Σ` read as a difference of two prefix sums and `m = hi − lo`. Rounding
    /// can make the numerator a hair negative, so it is clamped at 0. An empty run or a
    /// degenerate line (`|b − a|² < 1e-18`) gives 0.
    fn sq_dist(&self, lo: usize, hi: usize, a: Point, b: Point) -> f64 {
        if hi <= lo {
            return 0.0;
        }
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = dx * dx + dy * dy;
        if len2 < 1e-18 {
            return 0.0;
        }
        let m = (hi - lo) as f64;
        let d = |k: usize| self.s[hi][k] - self.s[lo][k];
        let (sx, sy, sxx, sxy, syy) = (d(0), d(1), d(2), d(3), d(4));
        // Signed distance times |b - a| is  u x + v y + c  with (u, v) the normal.
        let (u, v) = (-dy, dx);
        let (ax, ay) = (a.x - self.origin.x, a.y - self.origin.y);
        let c = -(u * ax + v * ay);
        let q = u * u * sxx
            + v * v * syy
            + 2.0 * u * v * sxy
            + 2.0 * u * c * sx
            + 2.0 * v * c * sy
            + c * c * m;
        q.max(0.0) / len2
    }
}

/// The cone of directions from one anchor that still pass within `tol` of every point seen
/// so far, as its two bounding unit vectors (`right` clockwise of `left`).
struct Cone {
    right: Vec2,
    left: Vec2,
    open: bool,
}

impl Cone {
    /// The cone before any point has narrowed it: every direction is admissible
    /// (`open == false`; the two bounds are unused until the first [`Cone::narrow`]).
    fn full() -> Self {
        Self {
            right: Vec2 { x: 0.0, y: 0.0 },
            left: Vec2 { x: 0.0, y: 0.0 },
            open: false,
        }
    }

    /// True when direction `d` (unit) is admissible.
    fn admits(&self, d: Vec2) -> bool {
        const EPS: f64 = 1e-9;
        !self.open || (self.right.cross(d) >= -EPS && d.cross(self.left) >= -EPS)
    }

    /// Narrow the cone by a point at offset `v`, distance `r > tol` from the anchor.
    /// Returns false when nothing is left.
    ///
    /// A ray from the anchor passes within `tol` of the point exactly when its direction is
    /// within `θ = asin(tol / r)` of `u = v / r`. The point's own cone is `u` rotated by
    /// `±θ` (using `sin θ = tol / r`, `cos θ = sqrt(1 − sin² θ)`), and the running cone is
    /// intersected with it by keeping the tighter bound on each side (compared by the sign
    /// of the 2-D cross product). The cone is empty once its bounds cross
    /// (`right × left < 0`) or it has opened to half a turn or more
    /// (`right · left <= 0`), which also rejects a degenerate wrap-around.
    fn narrow(&mut self, v: Vec2, r: f64, tol: f64) -> bool {
        let s = (tol / r).min(1.0);
        let c = (1.0 - s * s).max(0.0).sqrt();
        let (ux, uy) = (v.x / r, v.y / r);
        // `u` rotated by -theta (clockwise, in a y-up sense) and by +theta.
        let right = Vec2 {
            x: ux * c + uy * s,
            y: uy * c - ux * s,
        };
        let left = Vec2 {
            x: ux * c - uy * s,
            y: uy * c + ux * s,
        };
        if !self.open {
            *self = Self {
                right,
                left,
                open: true,
            };
            return true;
        }
        if self.right.cross(right) > 0.0 {
            self.right = right;
        }
        if left.cross(self.left) > 0.0 {
            self.left = left;
        }
        self.right.cross(self.left) >= 0.0 && self.right.dot(self.left) > 0.0
    }
}

/// The most points one side may span. A longer straight run is cut into sides of this
/// length, which the line merge after smoothing joins back into one; without the cap the
/// scan is quadratic on long straight boundaries, which a 2048 px raster is full of.
const MAX_SPAN: usize = 160;

/// Longest lattice step, in px, that [`open`] scans in closed form. Boundary points are
/// about a pixel apart, so this never binds on real input; it bounds the step in the
/// proof that the closed form is exact (see [`open`]).
const RUN_MAX_STEP: f64 = 4.0;

/// Smallest tolerance, in px, at which [`open`] scans lattice runs in closed form; the
/// fitter's own is at least 0.5. Like [`RUN_MAX_STEP`], it bounds the proof's margin.
const RUN_MIN_TOL: f64 = 1.0 / 16.0;

/// The lattice runs of a boundary: for each step `k` (from `p_k` to `p_{k+1}`, for `k` in
/// `0..n−1`), how many consecutive steps starting at `k` equal it, compared as IEEE values
/// (`p_{m+1} − p_m == p_{k+1} − p_k`, so `0.0 == −0.0` and a NaN step is a run of one).
///
/// `runs[k] = runs[k+1] + 1` when step `k+1` equals step `k`, else 1, filled from the end
/// in one pass: O(n) time and one `u32` per step. So the points `p_{k+1} ..= p_{k+runs[k]}`
/// all lie one equal step apart, starting from `p_k`. Fewer than two points give no steps.
///
/// Inspired by: the straight-run walker of VTracer (visioncortex, "VTracer" documentation,
/// <https://www.visioncortex.org/vtracer-docs>, and `walker.rs` in
/// <https://github.com/visioncortex/vtracer>), which compresses a pixel path's straight
/// runs before fitting, and by Freeman, H. (1961), "On the encoding of arbitrary geometric
/// configurations", *IRE Trans. Electronic Computers* EC-10:260–268,
/// doi:10.1109/TEC.1961.5219197, whose chain code makes a lattice run one repeated symbol.
/// Here the runs are only looked up: the points are not compressed, because the dynamic
/// program must still see every point to keep its output bit for bit.
fn lattice_runs(pts: &[Point]) -> Vec<u32> {
    let steps = pts.len().saturating_sub(1);
    let mut runs = vec![1u32; steps];
    for k in (0..steps.saturating_sub(1)).rev() {
        if pts[k + 2] - pts[k + 1] == pts[k + 1] - pts[k] {
            runs[k] = runs[k + 1] + 1;
        }
    }
    runs
}

/// The dynamic program's table: for each point, the best `(sides, penalty)` found so far
/// for a polygon from point 0 to it, compared lexicographically, and the vertex that
/// best path came from (`usize::MAX` while unreached).
struct Table {
    best: Vec<(u32, f64)>,
    prev: Vec<usize>,
}

impl Table {
    /// Offer side `i → j` to the table: a path with `count = best[i].sides + 1` sides,
    /// penalty `pen = best[i].penalty` plus the side's own squared distances, from `i`.
    ///
    /// The side is priced only when its count does not already lose at `j` (branch and
    /// bound; see [`open`]); it replaces `best[j]` when it has fewer sides, or as many and
    /// a strictly smaller penalty, so on a tie the earlier anchor keeps `j`. O(1).
    #[inline(always)]
    fn offer(&mut self, sums: &Sums, i: usize, j: usize, count: u32, pen: f64, a: Point, b: Point) {
        let (sides, best_pen) = self.best[j];
        if count > sides {
            return;
        }
        let cand = pen + sums.sq_dist(i + 1, j, a, b);
        if count < sides || cand < best_pen {
            self.best[j] = (count, cand);
            self.prev[j] = i;
        }
    }
}

/// Polygon vertices of an open run, as indices into `pts`: always the first and the last
/// point, and the fewest interior points such that every point lies within `tol` of the
/// side that spans it.
///
/// Dynamic programming over vertices in index order. A side `i → j` is admissible when
/// `j − i <= MAX_SPAN`, the direction of `p_j − p_i` lies in the cone of every point
/// strictly between them seen from `p_i` ([`Cone`]), and no point before `j` has fallen
/// back towards `p_i` by more than `tol`. The value of reaching `j` is the pair
///
/// `best[j] = min_i (best[i].sides + 1, best[i].pen + Σ_{i<k<j} dist(p_k, line p_i p_j)²)`
///
/// compared lexicographically, fewest sides first -- Potrace's optimal-polygon criterion,
/// with the sum from [`Sums::sq_dist`]. `tol` is in px. Two or fewer points are their own
/// polygon. A zero-length side is never admissible, so a run ending in points that all
/// coincide with a vertex could leave the last point unreachable; the fallback then makes
/// every point a vertex rather than fail.
///
/// # Work the table can never use
///
/// Two cuts, both branch and bound inside the dynamic program. Method from: Morin, T. L.
/// & Marsten, R. E. (1976), "Branch-and-bound strategies for dynamic programming",
/// *Operations Research* 24(4):611–627, doi:10.1287/opre.24.4.611 -- a state whose bound
/// shows it cannot lead to an optimal policy is fathomed. Adapted to this table's
/// lexicographic value, where the side count is an exact integer bound, so no relaxation
/// has to be solved to get one:
///
/// * a side `i → j` whose count `best[i].sides + 1` already exceeds `best[j].sides` cannot
///   win at `j` whatever its penalty, so the penalty is not computed. Measured on the
///   phase-1 replay, 89% of the admitted sides on the 246-icon screen set and 55% at
///   2048 px are of this kind;
/// * an anchor with `best[i].sides >= best[n−1].sides` is not scanned: every path through
///   it ends with more sides than one the end already has (18.9% of the scan steps on the
///   screen set start at such an anchor).
///
/// Both keep the output bit for bit. The first skips an expression nothing reads. For the
/// second, let `c*` be the final side count at the end. The end's running count is always
/// that of a real path, so never below `c*`, and an anchor is fathomed only when its own
/// count is at least that. By induction over the index, every entry with fewer than `c*`
/// sides is therefore decided by the same unfathomed anchors, through the same IEEE
/// expressions in the same order, as without the cut; the candidates the cut drops all
/// carry more sides than such an entry has. Every vertex of the optimal path but the end
/// is such an entry, and the end's winning candidates come from them, so the path is
/// unchanged. The tests check this against the unfathomed program
/// (`tests::open_ref`) on random, degenerate and lattice runs.
///
/// # Lattice runs in closed form
///
/// Most scan steps lie on a *lattice run*: the anchor's next points all one equal step
/// apart ([`lattice_runs`]). Measured on the phase-1 replay, 62–72% of all steps, and 98%
/// of the image frame's, which is the slowest boundary of 161 of 246 screen icons and of
/// every 2048 px test image. On such a run the scan's outcome is known in advance, so
/// once it is safely under way the per-point cone work (a `hypot`, three divisions, a
/// square root and four cross products, on a loop-carried dependency) is skipped: every
/// point to the run's end is admitted, and the cone and the reach after it are those
/// of the run's last point alone. The sides are still offered one by one, in order,
/// through the same [`Table::offer`], because their penalties decide ties between
/// equal side counts by rounding and must be the same IEEE values.
///
/// Why it is exact. Let the run start at the anchor `a = p_i` with step `s` (the IEEE
/// value of every `p_{k+1} − p_k` on it), and let the closed form take over at point `j`
/// once the cone is open and `r_{j−1} = |p_{j−1} − a| > 2 tol`, with `|s| <=`
/// [`RUN_MAX_STEP`] and `tol >=` [`RUN_MIN_TOL`]. Each exact step differs from `s` by at
/// most `2⁻⁵³|s|` per coordinate (it rounds to `s`), so every run point lies on the ray
/// from `a` along `s` to a relative error below `10⁻¹⁵`, and its computed direction and
/// cone bounds are off by less than `10⁻¹⁴` rad. Against that:
///
/// * the distances `r_k` grow by `|s|` per point, and `|s| > 2 tol / MAX_SPAN`, so the
///   reach test never fires and the reach is the last point's distance;
/// * a point's own cone is `±θ_k`, `θ_k = asin(tol / r_k)`, about the common direction,
///   and `θ_{k−1} − θ_k >= tol |s| / (r_{k−1} r_k) >= tol / (161² |s|) >= 6·10⁻⁷` rad, so
///   each point's bounds are strictly tighter than the last's and replace both: the cone
///   after the run is the last point's own;
/// * every direction lies inside the previous point's cone by `sin θ_{k−1} >= tol / (161
///   |s|) >= 9·10⁻⁵`, far beyond the admission slack of `10⁻⁹`, so every point is
///   admitted;
/// * `r_k > 2 tol` gives `sin θ_k <= ½`, so the cone spans at most 60° and never empties.
///
/// The generic scan would therefore admit the same sides in the same order and leave the
/// same cone and reach behind; the closed form differs only in not computing what it
/// would have found. Not from the literature: the closed form is ours, because the
/// published speed-ups for straight runs change what is fitted. Inspired by: VTracer's
/// straight-run walker, which compresses runs before fitting instead (see
/// [`lattice_runs`]). See also: Debled-Rennesson, I. & Reveillès, J.-P. (1995), "A linear
/// algorithm for segmentation of digital curves", *IJPRAI* 9:635–662,
/// doi:10.1142/S0218001495000249, whose digital straight segments recognise the lattice
/// run of an arbitrary slope; on sub-pixel points only an exactly repeated step is a
/// run, so the simpler test suffices. A tangential-cover bound on how far a scan can
/// reach (Faure, Buzer & Feschet 2009, "Tangential cover for thick digital curves") was
/// measured and dropped: it would have saved at most 9.5% of the steps.
pub(crate) fn open(pts: &[Point], tol: f64) -> Vec<usize> {
    let n = pts.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let sums = Sums::new(pts);
    let runs = lattice_runs(pts);
    let may_run = tol >= RUN_MIN_TOL;
    let mut t = Table {
        best: vec![(u32::MAX, f64::INFINITY); n],
        prev: vec![usize::MAX; n],
    };
    t.best[0] = (0, 0.0);
    for i in 0..n - 1 {
        let (sides, pen) = t.best[i];
        // Unreached, or fathomed: nothing through `i` can beat what reaches the end.
        if sides == u32::MAX || sides >= t.best[n - 1].0 {
            continue;
        }
        let count = sides + 1;
        let a = pts[i];
        let end = n.min(i + MAX_SPAN + 1);
        // The last point of the anchor's lattice run the closed form may take, or `i`
        // for none: `p_{i+1} ..= p_{run_last}` are one step `s` apart, starting at `a`.
        let s = pts[i + 1] - a;
        let run_last = if may_run && s.dot(s) <= RUN_MAX_STEP * RUN_MAX_STEP {
            (i + runs[i] as usize).min(end - 1)
        } else {
            i
        };
        let mut cone = Cone::full();
        let mut reach = 0.0f64;
        // Distance of the point before `j` from the anchor (0 for the anchor itself).
        let mut last_r = 0.0f64;
        let mut j = i + 1;
        while j < end {
            if j < run_last && cone.open && last_r > 2.0 * tol {
                // The rest of the lattice run, in closed form (see the proof above):
                // every point is admitted, and the cone and reach are the last point's.
                for k in j..=run_last {
                    t.offer(&sums, i, k, count, pen, a, pts[k]);
                }
                let v = pts[run_last] - a;
                let r = v.norm();
                reach = reach.max(r);
                // Always alive here (the cone spans at most 60°); the test keeps the scan
                // correct even if it were not.
                if !cone.narrow(v, r, tol) {
                    break;
                }
                last_r = r;
                j = run_last + 1;
                continue;
            }
            let v = pts[j] - a;
            let r = v.norm();
            // The run must move away from its anchor: a point that falls back by more than
            // the tolerance is doubling back, and no chord from `a` describes it.
            if r + tol < reach {
                break;
            }
            let d = if r > 1e-12 {
                Vec2 {
                    x: v.x / r,
                    y: v.y / r,
                }
            } else {
                Vec2 { x: 1.0, y: 0.0 }
            };
            // A count that loses at `j` loses whatever its penalty: fathomed unpriced
            // (checked before the cone, which costs more).
            if r > 1e-12 && count <= t.best[j].0 && cone.admits(d) {
                t.offer(&sums, i, j, count, pen, a, pts[j]);
            }
            reach = reach.max(r);
            if r > tol && !cone.narrow(v, r, tol) {
                break;
            }
            last_r = r;
            j += 1;
        }
    }
    let prev = t.prev;
    let mut out = vec![n - 1];
    let mut k = n - 1;
    while k != 0 {
        k = prev[k];
        if k == usize::MAX {
            // Unreachable: the next point is always admissible from any point.
            return (0..n).collect();
        }
        out.push(k);
    }
    out.reverse();
    out
}

/// Polygon vertices of a closed ring, as indices into `pts` in ring order.
///
/// The ring is cut at its sharpest point, which a polygon of any quality has as a vertex
/// anyway, and solved as an open run back to that point.
pub(crate) fn closed(pts: &[Point], tol: f64) -> Vec<usize> {
    let n = pts.len();
    if n < 4 {
        return (0..n).collect();
    }
    let s = sharpest(pts);
    let run: Vec<Point> = (0..=n).map(|k| pts[(s + k) % n]).collect();
    let mut v = open(&run, tol);
    v.pop(); // the start again
    v.into_iter().map(|k| (s + k) % n).collect()
}

/// The point of a closed ring where it turns hardest over a short window.
///
/// For each `k`, with `m = max(1, min(2, n/3))`, `u = p_k − p_{k−m}` and
/// `v = p_{k+m} − p_k`, the turn is `1 − cos∠(u, v) = 1 − u·v / (|u||v|)`, 0 for straight on
/// and 2 for a full reversal. The first maximum wins (a later one must beat it by 1e-12).
/// Zero-length windows are skipped; if every window is, point 0 is returned.
fn sharpest(pts: &[Point]) -> usize {
    let n = pts.len();
    let m = 2.min(n / 3).max(1);
    let mut best = (0, f64::NEG_INFINITY);
    for k in 0..n {
        let a = pts[(k + n - m) % n];
        let b = pts[k];
        let c = pts[(k + m) % n];
        let (u, v) = (b - a, c - b);
        let (nu, nv) = (u.norm(), v.norm());
        if nu < 1e-12 || nv < 1e-12 {
            continue;
        }
        let turn = 1.0 - u.dot(v) / (nu * nv);
        if turn > best.1 + 1e-12 {
            best = (k, turn);
        }
    }
    best.0
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    /// [`open`] as it was before branch and bound: every anchor scanned, every admissible
    /// side priced. The reference the exact rewrites are checked against, bit for bit.
    pub(crate) fn open_ref(pts: &[Point], tol: f64) -> Vec<usize> {
        let n = pts.len();
        if n <= 2 {
            return (0..n).collect();
        }
        let sums = Sums::new(pts);
        let mut best: Vec<(u32, f64)> = vec![(u32::MAX, f64::INFINITY); n];
        let mut prev = vec![usize::MAX; n];
        best[0] = (0, 0.0);
        for i in 0..n - 1 {
            if best[i].0 == u32::MAX {
                continue;
            }
            let a = pts[i];
            let mut cone = Cone::full();
            let mut reach = 0.0f64;
            for j in i + 1..n.min(i + MAX_SPAN + 1) {
                let v = pts[j] - a;
                let r = v.norm();
                if r + tol < reach {
                    break;
                }
                let d = if r > 1e-12 {
                    Vec2 {
                        x: v.x / r,
                        y: v.y / r,
                    }
                } else {
                    Vec2 { x: 1.0, y: 0.0 }
                };
                if r > 1e-12 && cone.admits(d) {
                    let cand = (best[i].0 + 1, best[i].1 + sums.sq_dist(i + 1, j, a, pts[j]));
                    if cand.0 < best[j].0 || (cand.0 == best[j].0 && cand.1 < best[j].1) {
                        best[j] = cand;
                        prev[j] = i;
                    }
                }
                reach = reach.max(r);
                if r > tol && !cone.narrow(v, r, tol) {
                    break;
                }
            }
        }
        let mut out = vec![n - 1];
        let mut k = n - 1;
        while k != 0 {
            k = prev[k];
            if k == usize::MAX {
                return (0..n).collect();
            }
            out.push(k);
        }
        out.reverse();
        out
    }

    /// [`closed`] on top of [`open_ref`].
    pub(crate) fn closed_ref(pts: &[Point], tol: f64) -> Vec<usize> {
        let n = pts.len();
        if n < 4 {
            return (0..n).collect();
        }
        let s = sharpest(pts);
        let run: Vec<Point> = (0..=n).map(|k| pts[(s + k) % n]).collect();
        let mut v = open_ref(&run, tol);
        v.pop();
        v.into_iter().map(|k| (s + k) % n).collect()
    }

    /// A small deterministic generator (xorshift64), so the random cases need no crate.
    pub(crate) struct Rng(pub u64);

    impl Rng {
        /// Uniform in `[0, 1)`.
        pub(crate) fn unit(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Point runs that exercise every branch of the scan: random walks, lattice staircases
    /// and straight lattice runs (the image frame's shape), a checkerboard zigzag, points
    /// that coincide, a run that doubles back, one point, two, one row and one column.
    pub(crate) fn cases() -> Vec<Vec<Point>> {
        let mut out: Vec<Vec<Point>> = vec![
            vec![p(0.0, 0.0)],
            vec![p(0.0, 0.0), p(1.0, 0.0)],
            vec![p(3.0, 3.0); 7],
            (0..40).map(|k| p(k as f64 - 0.5, -0.5)).collect(),
            (0..40).map(|k| p(-0.5, k as f64 - 0.5)).collect(),
            (0..30)
                .map(|k| p(k as f64, if k % 2 == 0 { 0.0 } else { 1.0 }))
                .collect(),
        ];
        // The image frame: four straight lattice runs round a rectangle.
        for (w, h) in [(5usize, 3usize), (40, 25), (200, 170), (400, 3)] {
            let mut f = Vec::new();
            for x in 0..w {
                f.push(p(x as f64 - 0.5, -0.5));
            }
            for y in 0..h {
                f.push(p(w as f64 - 0.5, y as f64 - 0.5));
            }
            for x in (1..=w).rev() {
                f.push(p(x as f64 - 0.5, h as f64 - 0.5));
            }
            for y in (1..=h).rev() {
                f.push(p(-0.5, y as f64 - 0.5));
            }
            out.push(f);
        }
        // Staircases: lattice runs of several slopes, joined.
        let mut s = vec![p(0.0, 0.0)];
        for (dx, dy, len) in [
            (1.0, 0.0, 12),
            (1.0, 1.0, 9),
            (0.0, 1.0, 30),
            (1.0, 0.5, 20),
        ] {
            for _ in 0..len {
                let q = *s.last().expect("non-empty");
                s.push(p(q.x + dx, q.y + dy));
            }
        }
        out.push(s);
        let mut back: Vec<Point> = (0..=10).map(|k| p(k as f64, 0.0)).collect();
        back.extend((0..10).rev().map(|k| p(k as f64, 0.2)));
        out.push(back);
        // Runs of sub-pixel and of long steps (past RUN_MAX_STEP), accumulated in floating
        // point so that some steps round equal and some do not, joined at corners.
        for (sx, sy) in [(0.7, 0.3), (0.1, 0.1), (4.5, 0.0), (1.0 / 3.0, 2.0 / 3.0)] {
            let mut q = vec![p(100.3, 7.9)];
            for leg in 0..4 {
                let (dx, dy) = if leg % 2 == 0 { (sx, sy) } else { (-sy, sx) };
                for _ in 0..70 {
                    let last = *q.last().expect("non-empty");
                    q.push(p(last.x + dx, last.y + dy));
                }
            }
            out.push(q);
        }
        // A long lattice line with one point nudged off it, past and within tolerance.
        for nudge in [0.2, 0.9] {
            let mut q: Vec<Point> = (0..300).map(|k| p(k as f64 * 0.5, 3.0)).collect();
            q[150].y += nudge;
            out.push(q);
        }
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for len in [3usize, 5, 17, 64, 170, 400] {
            for rough in [0.05, 0.3, 1.0] {
                let mut q = Vec::with_capacity(len);
                let (mut x, mut y, mut ang) = (10.0, 10.0, 0.0f64);
                for _ in 0..len {
                    q.push(p(x, y));
                    ang += (rng.unit() - 0.5) * rough;
                    let step = 0.6 + 0.8 * rng.unit();
                    x += step * ang.cos();
                    y += step * ang.sin();
                    // Some repeated points and some exact lattice steps.
                    if rng.unit() < 0.05 {
                        q.push(p(x, y));
                    }
                    if rng.unit() < 0.1 {
                        x = x.round();
                        y = y.round();
                    }
                }
                out.push(q);
            }
        }
        out
    }

    /// The rewritten program (fathoming, lattice runs in closed form) against
    /// [`open_ref`] on every case, open and closed, at the default tolerance, loosened
    /// ones, and one below [`RUN_MIN_TOL`] where the closed form stands aside.
    #[test]
    fn the_rewritten_program_keeps_the_polygon_bit_for_bit() {
        for pts in cases() {
            for tol in [0.01, 0.5, 0.8, 1.5, 3.0] {
                assert_eq!(open(&pts, tol), open_ref(&pts, tol), "open, tol {tol}");
                assert_eq!(
                    closed(&pts, tol),
                    closed_ref(&pts, tol),
                    "closed, tol {tol}"
                );
            }
        }
    }

    /// Runs count equal steps from each step to the first that differs.
    #[test]
    fn lattice_runs_count_equal_steps() {
        let pts = [
            p(0.0, 0.0),
            p(1.0, 0.0),
            p(2.0, 0.0),
            p(3.0, 0.0),
            p(3.0, 1.0),
            p(3.0, 2.0),
            p(4.0, 2.0),
        ];
        assert_eq!(lattice_runs(&pts), vec![3, 2, 1, 2, 1, 1]);
        assert!(lattice_runs(&pts[..1]).is_empty());
        assert_eq!(lattice_runs(&pts[..2]), vec![1]);
        // Coincident points are a run of zero steps; the scan never takes it in closed
        // form, because no point of it is far enough from the anchor.
        assert_eq!(lattice_runs(&[p(1.0, 1.0); 4]), vec![3, 2, 1]);
    }

    #[test]
    fn a_straight_run_is_one_side() {
        let pts: Vec<Point> = (0..50)
            .map(|k| p(k as f64, 0.5 * k as f64 + 0.05 * ((k % 3) as f64)))
            .collect();
        assert_eq!(open(&pts, 0.5), vec![0, 49]);
    }

    #[test]
    fn an_l_is_two_sides_meeting_at_the_corner() {
        let mut pts: Vec<Point> = (0..=20).map(|k| p(k as f64, 0.0)).collect();
        pts.extend((1..=20).map(|k| p(20.0, k as f64)));
        assert_eq!(open(&pts, 0.5), vec![0, 20, 40]);
    }

    #[test]
    fn a_doubling_back_run_is_split() {
        let mut pts: Vec<Point> = (0..=10).map(|k| p(k as f64, 0.0)).collect();
        pts.extend((0..10).rev().map(|k| p(k as f64, 0.2)));
        let v = open(&pts, 0.5);
        assert!(v.len() >= 3, "{v:?}");
    }

    #[test]
    fn a_square_ring_has_its_four_corners() {
        let mut pts = Vec::new();
        for k in 0..10 {
            pts.push(p(k as f64, 0.0));
        }
        for k in 0..10 {
            pts.push(p(10.0, k as f64));
        }
        for k in 0..10 {
            pts.push(p(10.0 - k as f64, 10.0));
        }
        for k in 0..10 {
            pts.push(p(0.0, 10.0 - k as f64));
        }
        let mut v = closed(&pts, 0.5);
        v.sort_unstable();
        assert_eq!(v, vec![0, 10, 20, 30]);
    }

    #[test]
    fn a_circle_gets_a_modest_polygon_within_tolerance() {
        let n = 200;
        let pts: Vec<Point> = (0..n)
            .map(|k| {
                let t = k as f64 / n as f64 * std::f64::consts::TAU;
                p(40.0 + 30.0 * t.cos(), 40.0 + 30.0 * t.sin())
            })
            .collect();
        let v = closed(&pts, 0.5);
        // A regular polygon within 0.5 of a radius-30 circle needs about pi/acos(29.5/30).
        assert!((10..=24).contains(&v.len()), "{}", v.len());
    }

    #[test]
    fn squared_distances_match_the_direct_sum() {
        let pts: Vec<Point> = (0..9)
            .map(|k| p(k as f64, ((k * 7) % 5) as f64 * 0.3))
            .collect();
        let s = Sums::new(&pts);
        let (a, b) = (pts[1], pts[7]);
        let dir = b - a;
        let direct: f64 = pts[2..7]
            .iter()
            .map(|q| {
                let c = dir.cross(*q - a) / dir.norm();
                c * c
            })
            .sum();
        assert!((s.sq_dist(2, 7, a, b) - direct).abs() < 1e-9);
    }
}
