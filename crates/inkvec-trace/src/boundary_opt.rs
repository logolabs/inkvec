//! Move every boundary point at once so that the *rendered* partition matches the image.
//!
//! Everything upstream of this decides each boundary point on its own: the level set puts
//! it on the pixel grid, and [`crate::planar::refine_subpixel`] slides it along its own
//! normal until the coverage it reads there is a half. That is a one-dimensional argument
//! per point, and it cannot see two facts that matter. A pixel's value is the area
//! coverage of *every* region that touches it, so a point's neighbours along the boundary
//! change what that pixel should read; and a pixel says nothing at all about motion
//! *along* the boundary, so a point is free to slide unless something holds it.
//!
//! So the boundary is solved as one problem. The unknowns are all the boundary points at
//! once, with an endpoint that several edges share counted once — which is what keeps the
//! map a partition however far the points move. The objective is
//!
//! ```text
//! E = sum over boundary pixels || a·c_left + (1-a)·c_right - target ||²
//!   + w_kink   · sum over points |p_{i-1} - 2p_i + p_{i+1}|
//!   + w_anchor · sum over points |p_i - p_i⁰|²
//! ```
//!
//! where `a` is the **exact area** of the pixel square on the left face's side of the
//! boundary — the pixel clipped by the chain that crosses it, closed along the pixel's own
//! border. That is the quantity a rasteriser computes, so the first term is the rendering
//! error itself rather than a proxy for it, and its gradient is analytic: the area is a
//! shoelace sum over the clipped polygon, and every vertex of that polygon is either a
//! boundary point (moving with its unknown), a crossing of a pixel gridline (moving as the
//! two points either side of it move) or a corner of the pixel (fixed).
//!
//! The two priors supply what a raster cannot. The kink term is the **absolute** value of
//! the second difference, not its square: a corner then costs in proportion to how sharply
//! it turns, so one sharp corner is cheaper than the many small kinks a squared term would
//! spread it into — the staircase is smoothed and the corner survives. The anchor term
//! removes the tangential freedom and holds a point the image cannot see (the interior of
//! a long straight run, a boundary between two nearly equal colours) where the measurement
//! put it.
//!
//! # The sawtooth, and three cures that do not work
//!
//! Area coverage does not determine a boundary. Any wiggle that preserves how much of each
//! pixel falls on either side leaves the data term exactly unchanged, and on a stroke about
//! two pixels wide — where both of its sides compete for the same pixels, and most of those
//! pixels are excluded as junction pixels anyway — the solver wanders into that null space
//! and returns a row of triangular teeth. They render almost as well as a straight edge and
//! look nothing like one, which is the whole problem: `openmoji/1F3A1` moves by 0.008 in
//! colour error while turning a smooth grey stroke into a saw.
//!
//! Measured 2026-09-03, all rejected, none shipped:
//!
//! * **More smoothing.** The teeth do clear, at about twenty times the shipped kink weight.
//!   They take real detail with them: on 246 icons DISTS goes 0.0310 to 0.0363.
//! * **A length term** on the boundary, the textbook cure for this null space, since a
//!   zigzag is much longer than the straight edge with the same per-pixel areas. At weights
//!   that leave the rest of the corpus alone it barely touches the teeth.
//! * **Anchoring each point by its own measured uncertainty**, which is appealing because
//!   `refine_subpixel` already marks thin-ribbon points uncertain (its coverage gradient is
//!   small there) and it needs no new threshold. It does not remove the teeth either.
//! * **Stopping the solver early.** The teeth grow with iteration count: one iteration is
//!   clean, three shows them, twenty-four is a saw. On thin-stroke icons four iterations
//!   beat twenty-four on every axis (dE00 0.7625 against 0.7852, DISTS 0.0568 against
//!   0.0633) — but on the 246-icon screening set the full count wins (0.2249 against
//!   0.2293), because everything that is not a thin ribbon is still converging usefully.
//!
//! The pattern in all four is the same: this is a *local* failure of the model and a global
//! knob cannot serve both cases. The model's own domain is the honest place to fix it — a
//! ribbon two pixels wide has no interior, so its two sides are not two independent
//! boundaries and should not be solved as though they were. That is LOG-44, fitting a thin
//! face as a centreline and a width, and it is the same root cause as the lumpy strokes and
//! the failure of a wider tangent window in `refine_subpixel`. Do not add a fifth knob here.
//!
//! Junction pixels are left out of the data term. Where three or more faces meet, one
//! chain no longer divides the pixel in two and the coverages need the full clipped
//! partition; the points there keep their priors and their anchor, so they move with their
//! neighbours but are not driven by the image. `planar::refine_junctions` has already
//! placed them by intersecting the boundaries that meet there, which is better evidence
//! than a single pixel's colour.
//!
//! # Where this sits, and how it is solved
//!
//! Quality mode only: the crate root's trace calls [`optimise_alpha`] after
//! `planar::refine_subpixel_alpha` and `planar::refine_junctions`, and before symmetry is
//! re-imposed and the edges go to the fitter. It takes the [`PlanarMap`], the source image
//! (sRGB `0..1`, row-major) and each face's fill model, and moves the map's points in
//! place. Coordinates are in px with pixel centres at integer coordinates, so pixel
//! `(x, y)` is the square `[x−0.5, x+0.5] x [y−0.5, y+0.5]`.
//!
//! The energy is minimised by **nonlinear conjugate gradient** (Fletcher–Reeves, with a
//! restart to steepest descent whenever the new direction is not downhill) and a
//! backtracking line search: the first trial step moves the furthest point `MAX_STEP`,
//! and each rejected trial shrinks it by 0.4, at most six times. Every point is also kept
//! within `MAX_TOTAL` of where the measurement put it. The solve stops after `ITERS`
//! iterations, when a step buys less than 1e-4 of the energy, or when the caller's time
//! budget runs out. Nothing is linearised: each trial re-renders the exact coverage.
//!
//! Afterwards a fold guard (`crossings_count`) scales the whole displacement back towards
//! the start until it introduces no new self-crossing; if even a tenth of it does, the
//! stage gives up and leaves the map as it was.
//!
//! The data term is accumulated in f64, and the mixture `a·c_left + (1−a)·c_right` is
//! formed in f64 too (see `cut_pixel`): in f32 the energy is a staircase the line search
//! cannot descend.

use inkvec_core::clock::Instant;
use std::collections::HashMap;

use inkvec_core::Point;

use crate::gradient::FillModel;
use crate::planar::PlanarMap;

/// How far one point may move in a single step, in pixels.
const MAX_STEP: f64 = 0.35;
/// How far a point may end up from where the measurement put it, in pixels. This is a
/// refinement of the boundary, not a search for it: a point a pixel away from its own level
/// set has stopped describing the same piece of the image.
const MAX_TOTAL: f64 = 1.0;
/// Kink and anchor weights, as a fraction of the data term's initial value. Scaling them to
/// the data term is what makes them mean the same thing on a flat two-colour logo and on a
/// crowded emoji, where the residual differs by orders of magnitude.
///
/// Concretely `w_kink = K_KINK · D0 / K0`, where `D0` is the initial data term and `K0` the
/// initial kink sum at unit weight, so the kink term starts at 5% of the data term.
const K_KINK: f64 = 0.05;
/// Anchor weight: `w_anchor = K_ANCHOR · D0 / n` for `n` unknowns, so a point one pixel from
/// its start costs a tenth of the average point's share of the initial data term.
const K_ANCHOR: f64 = 0.10;
/// Junction points are anchored harder, for the reason given in the module comment.
const JUNCTION_ANCHOR: f64 = 4.0;
/// Colour difference across a boundary below which a pixel carries no usable evidence.
const MIN_CONTRAST: f32 = 2.0 / 255.0;

/// Iterations of the solve; see the measurement where it is used.
const ITERS: usize = 48;

/// Where a vertex of a clipped polygon came from, and so how it moves.
#[derive(Clone, Copy)]
enum Prov {
    /// A boundary point, moving with its unknown.
    Vertex(u32),
    /// Where the segment `a`→`b` crosses the vertical gridline `line`.
    CrossV { line: f64, a: u32, b: u32 },
    /// Where the segment `a`→`b` crosses the horizontal gridline `line`.
    CrossH { line: f64, a: u32, b: u32 },
    /// A corner of the pixel square: fixed.
    Corner,
}

/// One piece of one boundary chain, lying inside one pixel.
#[derive(Clone, Copy)]
struct Piece {
    /// Index of the edge in the map.
    edge: u32,
    /// Where the piece enters the pixel (or starts inside it).
    from: Point,
    /// Where the piece leaves the pixel (or ends inside it).
    to: Point,
    from_prov: Prov,
    to_prov: Prov,
    /// Next piece in the same pixel's list (a singly linked list threaded through
    /// `Problem::pieces`, headed by `Problem::head`), or -1.
    next: i32,
}

/// The unknowns: one per boundary point, with the endpoints several edges share collapsed
/// into one.
struct Vars {
    /// `var[edge][i]` is the unknown holding point `i` of that edge.
    var: Vec<Vec<u32>>,
    /// Each unknown's starting position: where the earlier stages put it.
    start: Vec<Point>,
    /// Whether each unknown is a shared end point (a junction node).
    junction: Vec<bool>,
}

/// Number the unknowns. Every point of every edge gets its own, except that the end points
/// of open edges are keyed by their node id, so all edges meeting at a node share one.
/// Unknowns are numbered in edge order, then point order, which fixes the summation order
/// of everything downstream.
fn build_vars(map: &PlanarMap) -> Vars {
    let mut var: Vec<Vec<u32>> = Vec::with_capacity(map.edges.len());
    let mut start: Vec<Point> = Vec::new();
    let mut junction: Vec<bool> = Vec::new();
    let mut by_node: HashMap<u32, u32> = HashMap::new();
    for e in &map.edges {
        let n = e.points.len();
        let mut ids = Vec::with_capacity(n);
        for (i, &p) in e.points.iter().enumerate() {
            let shared = if e.closed {
                None
            } else if i == 0 {
                Some(e.start_node)
            } else if i + 1 == n {
                Some(e.end_node)
            } else {
                None
            };
            let id = match shared {
                Some(node) => *by_node.entry(node).or_insert_with(|| {
                    start.push(p);
                    junction.push(true);
                    (start.len() - 1) as u32
                }),
                None => {
                    start.push(p);
                    junction.push(false);
                    (start.len() - 1) as u32
                }
            };
            ids.push(id);
        }
        var.push(ids);
    }
    Vars {
        var,
        start,
        junction,
    }
}

/// Largest coordinate, in pixels, whose gridlines [`crossings`] walks. Far inside the range
/// where `m += 1.0` is exact, and far outside any image (labels are `u16`).
const GRID_LIMIT: f64 = 1e9;

/// Most gridlines [`crossings`] walks along one axis of one segment. A boundary segment
/// lies inside the image, so it crosses at most the image's width or height.
const GRID_MAX_SPAN: f64 = (1u64 << 20) as f64;

/// Whether the gridlines between `lo` and `hi` can be walked a step at a time. The walk
/// `while m < hi { m += 1.0 }` never ends when `hi` is infinite, or once `m` is so large
/// that adding one leaves it where it was; a NaN fails every comparison here and so is
/// refused as well. A segment that fails contributes no crossings: its geometry is already
/// meaningless, and the caller must not hang on it.
fn walkable(lo: f64, hi: f64) -> bool {
    lo > -GRID_LIMIT && hi < GRID_LIMIT && hi - lo < GRID_MAX_SPAN
}

/// Gridline crossings of one segment, as parameters in `(0, 1)`, in order.
///
/// Pixel borders are the half-integer lines `x = m + 0.5` and `y = m + 0.5`. For segment
/// `a → b`, each vertical border `x = m` strictly between `a.x` and `b.x` crosses at
/// `t = (m − a.x) / (b.x − a.x)`, and likewise for horizontal ones; each crossing records
/// its gridline and the two unknowns `va`, `vb` it moves with. Crossings within `1e-9` of
/// either end are dropped (the end itself is a vertex), and an axis the segment does not
/// move along, or cannot be walked (see [`walkable`]), contributes none. `out` is cleared
/// first and reused, so this allocates nothing in the solve loop.
fn crossings(a: Point, b: Point, va: u32, vb: u32, out: &mut Vec<(f64, Prov)>) {
    out.clear();
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (lo_x, hi_x) = if a.x < b.x { (a.x, b.x) } else { (b.x, a.x) };
    if dx.abs() > 1e-12 && walkable(lo_x, hi_x) {
        let (lo, hi) = (lo_x, hi_x);
        let mut m = (lo - 0.5).ceil() + 0.5;
        while m < hi {
            let t = (m - a.x) / dx;
            if t > 1e-9 && t < 1.0 - 1e-9 {
                out.push((
                    t,
                    Prov::CrossV {
                        line: m,
                        a: va,
                        b: vb,
                    },
                ));
            }
            m += 1.0;
        }
    }
    let (lo_y, hi_y) = if a.y < b.y { (a.y, b.y) } else { (b.y, a.y) };
    if dy.abs() > 1e-12 && walkable(lo_y, hi_y) {
        let (lo, hi) = (lo_y, hi_y);
        let mut m = (lo - 0.5).ceil() + 0.5;
        while m < hi {
            let t = (m - a.y) / dy;
            if t > 1e-9 && t < 1.0 - 1e-9 {
                out.push((
                    t,
                    Prov::CrossH {
                        line: m,
                        a: va,
                        b: vb,
                    },
                ));
            }
            m += 1.0;
        }
    }
    out.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap_or(std::cmp::Ordering::Equal));
}

/// Position on a pixel's border as a parameter in `[0, 4)`, running top, right, bottom,
/// left. `NaN` when the point is not on the border.
///
/// The pixel is centred on `(cx, cy)`. The parameter goes clockwise on screen (y down):
/// `0..1` along the top from the top-left corner, `1..2` down the right side, `2..3`
/// leftwards along the bottom, `3..4` up the left side, so integer values are corners.
/// "On the border" means within `1e-7` px of a side's line.
fn perim(p: Point, cx: f64, cy: f64) -> f64 {
    const EPS: f64 = 1e-7;
    let (x0, y0, x1, y1) = (cx - 0.5, cy - 0.5, cx + 0.5, cy + 0.5);
    if (p.y - y0).abs() <= EPS {
        return (p.x - x0).clamp(0.0, 1.0);
    }
    if (p.x - x1).abs() <= EPS {
        return 1.0 + (p.y - y0).clamp(0.0, 1.0);
    }
    if (p.y - y1).abs() <= EPS {
        return 2.0 + (x1 - p.x).clamp(0.0, 1.0);
    }
    if (p.x - x0).abs() <= EPS {
        return 3.0 + (y1 - p.y).clamp(0.0, 1.0);
    }
    f64::NAN
}

/// Corner `i` (mod 4) of the pixel centred on `(cx, cy)`, numbered as in [`perim`]:
/// 0 top-left, 1 top-right, 2 bottom-right, 3 bottom-left.
#[inline]
fn corner(i: i64, cx: f64, cy: f64) -> Point {
    let (x0, y0, x1, y1) = (cx - 0.5, cy - 0.5, cx + 0.5, cy + 0.5);
    match i.rem_euclid(4) {
        0 => Point::new(x0, y0),
        1 => Point::new(x1, y0),
        2 => Point::new(x1, y1),
        _ => Point::new(x0, y1),
    }
}

/// The pixel corners passed walking the border from `s_from` to `s_to`.
///
/// `s_from` and `s_to` are [`perim`] parameters; `forward` walks in increasing parameter
/// (clockwise on screen), otherwise decreasing. Corners are written to `out` (cleared
/// first) in walk order; a corner within `1e-9` of either end is not "passed". Closing a
/// chain's path through a pixel along these corners gives the clipped polygon whose
/// shoelace area is a face's coverage.
fn border_corners(s_from: f64, s_to: f64, forward: bool, cx: f64, cy: f64, out: &mut Vec<Point>) {
    out.clear();
    let span = if forward {
        (s_to - s_from).rem_euclid(4.0)
    } else {
        (s_from - s_to).rem_euclid(4.0)
    };
    let mut c = if forward {
        s_from.floor() as i64 + 1
    } else {
        s_from.ceil() as i64 - 1
    };
    for _ in 0..4 {
        let off = if forward {
            (c as f64 - s_from).rem_euclid(4.0)
        } else {
            (s_from - c as f64).rem_euclid(4.0)
        };
        if off <= 1e-9 || off >= span - 1e-9 {
            break;
        }
        out.push(corner(c, cx, cy));
        c += if forward { 1 } else { -1 };
    }
}

/// Why `junction_pixel` declined, counted for `INKVEC_JUNCDBG`. Purely diagnostic: the
/// construction below only handles one shape of junction, and the point of these is to
/// find out which shapes it is actually meeting.
pub mod juncstat {
    use std::sync::atomic::{AtomicUsize, Ordering};
    /// Junctions considered.
    pub static SEEN: AtomicUsize = AtomicUsize::new(0);
    /// Junctions `junction_pixel` accepted.
    pub static OK: AtomicUsize = AtomicUsize::new(0);
    /// Declines because the number of incident chains did not match the shape handled.
    pub static N_CHAINS: AtomicUsize = AtomicUsize::new(0);
    /// Declines because a chain did not end where expected.
    pub static ENDS: AtomicUsize = AtomicUsize::new(0);
    /// Declines at the node-classification step.
    pub static NODE: AtomicUsize = AtomicUsize::new(0);
    /// Declines at the face-classification step.
    pub static FACE: AtomicUsize = AtomicUsize::new(0);
    /// Declines at the partitioning step.
    pub static PARTITION: AtomicUsize = AtomicUsize::new(0);
    /// Increments one of the counters above.
    pub fn bump(c: &AtomicUsize) {
        c.fetch_add(1, Ordering::Relaxed);
    }
    /// Prints the accumulated counters to stderr.
    pub fn report() {
        eprintln!(
            "  [junc] seen {} ok {} | declined: chains {} ends {} node {} face {} partition {}",
            SEEN.load(Ordering::Relaxed),
            OK.load(Ordering::Relaxed),
            N_CHAINS.load(Ordering::Relaxed),
            ENDS.load(Ordering::Relaxed),
            NODE.load(Ordering::Relaxed),
            FACE.load(Ordering::Relaxed),
            PARTITION.load(Ordering::Relaxed),
        );
    }
}

/// Signed shoelace of a closed loop: `A = ½ Σ_i (x_i·y_{i+1} − x_{i+1}·y_i)`, indices
/// modulo the length. Positive for a loop that is anticlockwise in y-up axes, which on
/// screen (y down) is clockwise.
fn shoelace(pts: &[Point]) -> f64 {
    let n = pts.len();
    let mut s = 0.0;
    for i in 0..n {
        let (p, q) = (pts[i], pts[(i + 1) % n]);
        s += p.x * q.y - q.x * p.y;
    }
    0.5 * s
}

/// Push the gradient of a coverage with respect to one clipped vertex back onto the
/// unknowns it depends on.
///
/// `(gx, gy)` is `∂E/∂q` for the clipped polygon's vertex `q`; this adds `∂E/∂q · ∂q/∂p`
/// to `grad` for each unknown `p` that `q` depends on (the chain rule). A boundary point
/// is its own unknown; a pixel corner is fixed. A gridline crossing
/// `q = a + t(b − a)`, with `t = (line − a.x)/(b.x − a.x)` on a vertical gridline, has a
/// fixed `x` and a `y` that depends on both ends: `∂q.y/∂a.y = 1 − t`, `∂q.y/∂b.y = t`,
/// `∂q.y/∂a.x = dy(t − 1)/dx`, `∂q.y/∂b.x = −dy·t/dx`. Horizontal gridlines swap the
/// roles of x and y. A crossing on a segment parallel to its gridline (`|dx| < 1e-9`)
/// contributes nothing.
fn scatter(prov: Prov, gx: f64, gy: f64, pos: &[Point], grad: &mut [Point]) {
    match prov {
        Prov::Corner => {}
        Prov::Vertex(v) => {
            grad[v as usize].x += gx;
            grad[v as usize].y += gy;
        }
        // The crossing is `(line, a.y + t·dy)` with `t = (line - a.x)/dx`, so its x is
        // fixed by the gridline and only the y gradient propagates.
        Prov::CrossV { line, a, b } => {
            let (pa, pb) = (pos[a as usize], pos[b as usize]);
            let dx = pb.x - pa.x;
            if dx.abs() < 1e-9 {
                return;
            }
            let dy = pb.y - pa.y;
            let t = (line - pa.x) / dx;
            grad[a as usize].y += gy * (1.0 - t);
            grad[b as usize].y += gy * t;
            grad[a as usize].x += gy * dy * (t - 1.0) / dx;
            grad[b as usize].x -= gy * dy * t / dx;
        }
        Prov::CrossH { line, a, b } => {
            let (pa, pb) = (pos[a as usize], pos[b as usize]);
            let dy = pb.y - pa.y;
            if dy.abs() < 1e-9 {
                return;
            }
            let dx = pb.x - pa.x;
            let t = (line - pa.y) / dy;
            grad[a as usize].x += gx * (1.0 - t);
            grad[b as usize].x += gx * t;
            grad[a as usize].y += gx * dx * (t - 1.0) / dy;
            grad[b as usize].y -= gx * dx * t / dy;
        }
    }
}

/// Do the two segments cross, other than by sharing an endpoint?
///
/// The standard orientation test: `ab` and `cd` properly cross when each separates the
/// other's endpoints, i.e. `orient2d(a, b, c)` and `orient2d(a, b, d)` have strictly
/// opposite signs and so do `orient2d(c, d, a)` and `orient2d(c, d, b)`. `orient2d` is the
/// crate's robust (exact-sign) predicate, so near-collinear cases are decided correctly.
/// Collinear touching also counts, as explained below.
fn segments_cross(a: Point, b: Point, c: Point, d: Point) -> bool {
    use inkvec_core::predicates::orient2d;
    let (d1, d2) = (orient2d(a, b, c), orient2d(a, b, d));
    let (d3, d4) = (orient2d(c, d, a), orient2d(c, d, b));
    // Proper crossing: each segment separates the other's endpoints.
    if ((d1 > 0.0) != (d2 > 0.0))
        && ((d3 > 0.0) != (d4 > 0.0))
        && d1 != 0.0
        && d2 != 0.0
        && d3 != 0.0
        && d4 != 0.0
    {
        return true;
    }
    // A touch is a crossing too when it is a collinear overlap: the boundary has folded
    // back along itself, which the fitter cannot represent any better than a crossing.
    let on = |p: Point, q: Point, r: Point| -> bool {
        orient2d(p, q, r) == 0.0
            && r.x >= p.x.min(q.x) - 1e-12
            && r.x <= p.x.max(q.x) + 1e-12
            && r.y >= p.y.min(q.y) - 1e-12
            && r.y <= p.y.max(q.y) + 1e-12
    };
    (d1 == 0.0 && on(a, b, c))
        || (d2 == 0.0 && on(a, b, d))
        || (d3 == 0.0 && on(c, d, a))
        || (d4 == 0.0 && on(c, d, b))
}

/// How many pairs of boundary segments cross.
///
/// The solve can fold the boundary: on a ribbon two pixels wide the two sides are both
/// pulled towards the ink between them and can pass through each other. Downstream that
/// costs far more than the boundary error it bought — the repair stage refits the offending
/// rings round after round (2.5 s on one logo) and the emitter paints a face over its own
/// interior. So the displacement is scaled back until no *new* crossing remains.
///
/// The count is compared with the count before the solve rather than against zero: the
/// sub-pixel refinement and the junction solve can already have left a fold behind (which
/// is what the repair stage exists for), and refusing to improve a boundary because of a
/// crossing that was already there would give up most of the gain.
///
/// Only segments sharing a pixel are compared. A point moves less than a pixel, so a new
/// crossing is always local. Segments are bucketed by the cells of their bounding box
/// grown by half a pixel (a segment spanning more than 64 cells is skipped), pairs that
/// share an unknown are ignored, and each crossing pair is counted once.
fn crossings_count(map: &PlanarMap, vars: &Vars, pos: &[Point]) -> usize {
    let mut segs: Vec<(u32, u32)> = Vec::new();
    for (k, e) in map.edges.iter().enumerate() {
        let ids = &vars.var[k];
        let n = ids.len();
        if n < 2 {
            continue;
        }
        let last = if e.closed { n } else { n - 1 };
        for i in 0..last {
            segs.push((ids[i], ids[(i + 1) % n]));
        }
    }
    let mut cells: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
    for (i, &(a, b)) in segs.iter().enumerate() {
        let (p, q) = (pos[a as usize], pos[b as usize]);
        let x0 = (p.x.min(q.x) - 0.5).floor() as i64;
        let x1 = (p.x.max(q.x) + 0.5).ceil() as i64;
        let y0 = (p.y.min(q.y) - 0.5).floor() as i64;
        let y1 = (p.y.max(q.y) + 0.5).ceil() as i64;
        // A degenerate box would put a segment in every cell; the map never has one.
        if (x1 - x0) * (y1 - y0) > 64 {
            continue;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                cells.entry((x, y)).or_default().push(i as u32);
            }
        }
    }
    let mut found: Vec<(u32, u32)> = Vec::new();
    for bucket in cells.values() {
        for (ai, &i) in bucket.iter().enumerate() {
            for &j in bucket[ai + 1..].iter() {
                let (s, t) = (segs[i as usize], segs[j as usize]);
                // Segments sharing a point meet there legitimately.
                if s.0 == t.0 || s.0 == t.1 || s.1 == t.0 || s.1 == t.1 {
                    continue;
                }
                if segments_cross(
                    pos[s.0 as usize],
                    pos[s.1 as usize],
                    pos[t.0 as usize],
                    pos[t.1 as usize],
                ) {
                    found.push((i.min(j), i.max(j)));
                }
            }
        }
    }
    // A segment pair can share more than one pixel, so the same crossing can be seen
    // several times.
    found.sort_unstable();
    found.dedup();
    found.len()
}

/// Part of one boundary inside a junction pixel: its vertices in order, each with where it
/// came from.
type Chain = Vec<(Point, Prov)>;

/// Reused scratch, so an energy evaluation allocates nothing.
#[derive(Default)]
struct Scratch {
    /// Gridline crossings of the segment being bucketed.
    cross: Vec<(f64, Prov)>,
    /// The pieces in the current pixel, in walk order.
    order: Vec<usize>,
    /// The clipped polygon of the current pixel, and where each vertex came from.
    loop_pts: Vec<Point>,
    loop_prov: Vec<Prov>,
    /// Pixel corners closing the clipped polygon.
    corners: Vec<Point>,
}

/// The boundary solve's energy, and everything it needs to evaluate it.
///
/// `energy(pos)` is data + priors (see the module docs) at the unknown positions `pos`;
/// `bucket` must run first on the same `pos` to cut the chains into per-pixel pieces,
/// which `energy` does itself.
struct Problem<'a> {
    map: &'a PlanarMap,
    vars: &'a Vars,
    /// The source image, sRGB `0..1`, row-major `w x h`.
    rgb: &'a [[f32; 3]],
    /// Each face's fill model, indexed by face id.
    face: &'a [FillModel],
    w: usize,
    h: usize,
    /// Chain pieces, each lying in one pixel; rebuilt by `bucket`.
    pieces: Vec<Piece>,
    /// Per pixel, the index of the last piece bucketed into it (-1 for none); the rest of
    /// that pixel's pieces follow through `Piece::next`.
    head: Vec<i32>,
    scratch: Scratch,
    /// The source's alpha per pixel and each face's opacity, when alpha is a fourth
    /// channel of the data term.
    alpha: Option<(&'a [f32], &'a [f32])>,
    /// Weight of the kink prior (see `K_KINK`).
    w_kink: f64,
    /// Weight of the anchor prior (see `K_ANCHOR`).
    w_anchor: f64,
    /// Whether junction pixels take part in the data term.
    junctions: bool,
    /// The cells `head` holds a piece in, ascending once `bucket` has run. The data term
    /// visits these rather than all `w * h` cells, and `bucket` clears only these: at
    /// 2048 px the pieces sit in a few percent of the cells.
    touched: Vec<usize>,
    /// `INKVEC_BOPT_CELLS`, read once rather than once per boundary cell per evaluation.
    cells_dbg: bool,
    /// `INKVEC_BOPT_CHUNKS`, read once rather than once per evaluation.
    chunks: usize,
}

/// `INKVEC_BOPT_CHUNKS` in a `research` build; 1 (sequential) otherwise.
fn research_chunks() -> usize {
    if cfg!(feature = "research") {
        inkvec_core::env::count("INKVEC_BOPT_CHUNKS")
            .unwrap_or(1)
            .max(1)
    } else {
        1
    }
}

impl Problem<'_> {
    /// Sort every chain piece into the pixel it lies in.
    ///
    /// Each segment of each edge between two faces with fill models is cut at its
    /// gridline crossings; each resulting piece belongs to the pixel containing its
    /// midpoint and is pushed onto that pixel's list. Pieces outside the image are
    /// dropped. Within a pixel the list is newest first; `data_cells` reverses it.
    fn bucket(&mut self, pos: &[Point]) {
        self.pieces.clear();
        for &cell in &self.touched {
            self.head[cell] = -1;
        }
        self.touched.clear();
        let cr = &mut self.scratch.cross;
        for (k, e) in self.map.edges.iter().enumerate() {
            if e.left as usize >= self.face.len() || e.right as usize >= self.face.len() {
                continue;
            }
            let ids = &self.vars.var[k];
            let n = ids.len();
            if n < 2 {
                continue;
            }
            let last = if e.closed { n } else { n - 1 };
            for i in 0..last {
                let (va, vb) = (ids[i], ids[(i + 1) % n]);
                let (a, b) = (pos[va as usize], pos[vb as usize]);
                crossings(a, b, va, vb, cr);
                let mut t0 = 0.0;
                let mut from = a;
                let mut from_prov = Prov::Vertex(va);
                for idx in 0..=cr.len() {
                    let (t1, to, to_prov) = if idx == cr.len() {
                        (1.0, b, Prov::Vertex(vb))
                    } else {
                        let (t, prov) = cr[idx];
                        (
                            t,
                            Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t),
                            prov,
                        )
                    };
                    if t1 - t0 > 1e-9 {
                        let tm = 0.5 * (t0 + t1);
                        let px = (a.x + (b.x - a.x) * tm).round();
                        let py = (a.y + (b.y - a.y) * tm).round();
                        if px >= 0.0 && py >= 0.0 && px < self.w as f64 && py < self.h as f64 {
                            let cell = py as usize * self.w + px as usize;
                            let id = self.pieces.len() as i32;
                            self.pieces.push(Piece {
                                edge: k as u32,
                                from,
                                to,
                                from_prov,
                                to_prov,
                                next: self.head[cell],
                            });
                            if self.head[cell] < 0 {
                                self.touched.push(cell);
                            }
                            self.head[cell] = id;
                        }
                    }
                    t0 = t1;
                    from = to;
                    from_prov = to_prov;
                }
            }
        }
        // Ascending, so the data term visits the cells in exactly the order a scan over
        // every cell did, and sums the same terms in the same order.
        self.touched.sort_unstable();
    }

    /// The data term over every pixel, and its gradient (added into `grad`) when asked
    /// for. `bucket` must have run on `pos`.
    ///
    /// Sequential by default, so the floating-point summation order and therefore the
    /// output are exactly what they always were. `INKVEC_BOPT_CHUNKS=16` (research builds
    /// only) sums the cells
    /// in that many fixed contiguous ranges on every core, each with its own scratch and
    /// gradient, added in range order: deterministic on any machine and about 60 ms
    /// faster at 2048 px, but the last-bit differences cascade through tie-sensitive fit
    /// decisions -- on one 300-path logo they cost 8 paths and 16 % more coordinates at
    /// the same colour error -- so it stays opt-in until the full set has priced it.
    fn data(&mut self, pos: &[Point], grad: Option<&mut [Point]>) -> f64 {
        let chunks = self.chunks;
        let cells = self.w * self.h;
        if chunks == 1 || cells < 4096 {
            // Only the cells `bucket` put a piece in; every other cell was skipped anyway.
            let mut scratch = std::mem::take(&mut self.scratch);
            let touched = std::mem::take(&mut self.touched);
            let t = self.data_cells(touched.iter().copied(), pos, &mut scratch, grad);
            self.touched = touched;
            self.scratch = scratch;
            return t;
        }
        use rayon::prelude::*;
        let want_grad = grad.is_some();
        let n = grad.as_ref().map_or(0, |g| g.len());
        let step = cells.div_ceil(chunks);
        let parts: Vec<(f64, Vec<Point>)> = (0..chunks)
            .into_par_iter()
            .map(|k| {
                let lo = (k * step).min(cells);
                let hi = ((k + 1) * step).min(cells);
                let mut scratch = Scratch::default();
                let mut g = if want_grad {
                    vec![Point::new(0.0, 0.0); n]
                } else {
                    Vec::new()
                };
                let t = self.data_cells(
                    lo..hi,
                    pos,
                    &mut scratch,
                    if want_grad { Some(&mut g[..]) } else { None },
                );
                (t, g)
            })
            .collect();
        let mut total = 0.0;
        match grad {
            Some(dst) => {
                for (t, g) in parts {
                    total += t;
                    for (d, s) in dst.iter_mut().zip(g.iter()) {
                        d.x += s.x;
                        d.y += s.y;
                    }
                }
            }
            None => {
                for (t, _) in parts {
                    total += t;
                }
            }
        }
        total
    }

    /// Sum the data term over `cells`, in the order given, adding the gradient into
    /// `grad` when asked for. A pixel holding pieces of one edge goes to `cut_pixel`; one
    /// holding pieces of several is a junction, which counts only when `junctions` is on.
    /// `scratch` is this caller's own, so ranges can be summed on separate threads.
    fn data_cells(
        &self,
        cells: impl Iterator<Item = usize>,
        pos: &[Point],
        scratch: &mut Scratch,
        mut grad: Option<&mut [Point]>,
    ) -> f64 {
        let mut total = 0.0;
        for cell in cells {
            if self.head[cell] < 0 {
                continue;
            }
            let order = &mut scratch.order;
            order.clear();
            let mut id = self.head[cell];
            while id >= 0 {
                order.push(id as usize);
                id = self.pieces[id as usize].next;
            }
            order.reverse();
            // One chain of one boundary, entering and leaving the pixel exactly once.
            let edge = self.pieces[order[0]].edge;
            if order.iter().any(|&i| self.pieces[i].edge != edge) {
                // Several boundaries in one pixel: a junction. Its wedges are a separate
                // construction, and one this stage only attempts when asked for.
                if self.junctions {
                    total += self.junction_pixel(cell, pos, scratch, grad.as_deref_mut());
                }
                continue;
            }
            self.cut_pixel(cell, pos, scratch, grad.as_deref_mut(), &mut total);
        }
        total
    }

    /// A pixel cut in two by one chain of one boundary: its squared colour residual,
    /// added to `total`, and the residual's gradient, added to `grad` when asked for.
    ///
    /// `scratch.order` holds the pixel's pieces in walk order, all of one edge. The pixel
    /// is skipped (nothing added) unless the pieces join end to end, both ends of the
    /// chain lie on the pixel's border, the two faces differ by at least `MIN_CONTRAST`,
    /// and the clipped area is a plausible coverage. For coverage `a` of the left face,
    /// colours `c_l`, `c_r` at the pixel centre and target `t`:
    ///
    /// ```text
    ///     E_pixel = Σ_k (a·c_l[k] + (1−a)·c_r[k] − t[k])²
    ///     ∂E/∂q_i = (dE/da) · ∂a/∂q_i,   dE/da = Σ_k 2·r_k·(c_l[k] − c_r[k])
    /// ```
    ///
    /// with `∂a/∂q_i` from the shoelace formula at each vertex `q_i` of the clipped
    /// polygon, pushed onto the unknowns by [`scatter`]. The gradient is skipped when the
    /// pixel is (numerically) fully on one side.
    fn cut_pixel(
        &self,
        cell: usize,
        pos: &[Point],
        scratch: &mut Scratch,
        mut grad: Option<&mut [Point]>,
        total: &mut f64,
    ) {
        let order = &scratch.order;
        let edge = self.pieces[order[0]].edge;
        if order.windows(2).any(|pair| {
            let (a, b) = (self.pieces[pair[0]].to, self.pieces[pair[1]].from);
            (a.x - b.x).abs() > 1e-9 || (a.y - b.y).abs() > 1e-9
        }) {
            return;
        }
        let first = self.pieces[order[0]];
        let last = self.pieces[order[order.len() - 1]];
        let (px, py) = ((cell % self.w) as f64, (cell / self.w) as f64);
        // Both ends have to lie on the pixel's border, so that the chain really does
        // cut the square in two. An end inside the pixel is an open end or a junction
        // that was placed there, and then one chain does not divide it.
        let s_in = perim(first.from, px, py);
        let s_out = perim(last.to, px, py);
        if !s_in.is_finite() || !s_out.is_finite() {
            return;
        }
        let e = &self.map.edges[edge as usize];
        let cl = self.face[e.left as usize].color_at(px, py);
        let cr = self.face[e.right as usize].color_at(px, py);
        let contrast = (0..3).map(|k| (cl[k] - cr[k]).abs()).fold(0.0f32, f32::max);
        // The opacities either side, when alpha is a channel -- and only where the colour
        // over white has no contrast to measure: white paint against the clear ground,
        // two bands of one fade. Anywhere else the colour over white already carries
        // the alpha (`W = 1 - a` for black on clear) and counting it again as a fourth
        // channel moved every such edge: 0.07 px on `material-icons/table_rows`.
        let opac = self
            .alpha
            .filter(|_| contrast < MIN_CONTRAST)
            .map(|(img_a, fa)| {
                (
                    fa.get(e.left as usize).copied().unwrap_or(1.0),
                    fa.get(e.right as usize).copied().unwrap_or(1.0),
                    img_a[cell],
                )
            });
        let contrast = match opac {
            Some((al, ar, _)) => contrast.max((al - ar).abs()),
            None => contrast,
        };
        if contrast < MIN_CONTRAST {
            return;
        }
        // The pixel cut by the chain and closed along its own border: the side the
        // left face lies on. Built one way round; the sign says whether that was the
        // left side, and if not the other way round is the one.
        let mut area = f64::NAN;
        for &forward in &[true, false] {
            let lp = &mut scratch.loop_pts;
            let lv = &mut scratch.loop_prov;
            lp.clear();
            lv.clear();
            lp.push(first.from);
            lv.push(first.from_prov);
            for &i in scratch.order.iter() {
                lp.push(self.pieces[i].to);
                lv.push(self.pieces[i].to_prov);
            }
            border_corners(s_out, s_in, forward, px, py, &mut scratch.corners);
            for &p in scratch.corners.iter() {
                scratch.loop_pts.push(p);
                scratch.loop_prov.push(Prov::Corner);
            }
            let s = shoelace(&scratch.loop_pts);
            if s <= 0.0 {
                area = -s;
                break;
            }
        }
        if !area.is_finite() || !(-0.01..=1.01).contains(&area) {
            return;
        }
        if self.cells_dbg {
            eprintln!(
                "  [bopt cell] ({px},{py}) edge {edge} pieces {} area {area:.6} ends {s_in:.3}->{s_out:.3}",
                scratch.order.len()
            );
        }
        let a = area.clamp(0.0, 1.0);
        let t = self.rgb[cell];
        // In double precision: the mixture in f32 quantises the objective at a
        // hundredth of the coverage resolution the solver works at, which turns a
        // smooth energy into a staircase the line search cannot descend.
        let mut dda = 0.0;
        for k in 0..3 {
            let (l, r_) = (cl[k] as f64, cr[k] as f64);
            let c = a * l + (1.0 - a) * r_;
            let r = c - t[k] as f64;
            *total += r * r;
            dda += 2.0 * r * (l - r_);
        }
        if let Some((al, ar, ta)) = opac {
            let (l, r_) = (al as f64, ar as f64);
            let r = a * l + (1.0 - a) * r_ - ta as f64;
            *total += r * r;
            dda += 2.0 * r * (l - r_);
        }
        if let Some(g) = grad.as_deref_mut() {
            if a > 1e-9 && a < 1.0 - 1e-9 {
                let lp = &scratch.loop_pts;
                let n = lp.len();
                for i in 0..n {
                    let prev = lp[(i + n - 1) % n];
                    let next = lp[(i + 1) % n];
                    // The coverage is minus the shoelace, so its derivative at vertex
                    // `i` is minus the usual one.
                    let gx = -0.5 * (next.y - prev.y) * dda;
                    let gy = -0.5 * (prev.x - next.x) * dda;
                    scatter(scratch.loop_prov[i], gx, gy, pos, g);
                }
            }
        }
    }

    /// Group a junction pixel's pieces into chains: runs of consecutive pieces, in walk
    /// order, that belong to one edge and join end to end. Returns each chain's vertices
    /// with their provenance, and the edge each chain belongs to.
    fn group_chains(&self, order: &[usize]) -> (Vec<Chain>, Vec<u32>) {
        let mut chains: Vec<Chain> = Vec::new();
        let mut edges: Vec<u32> = Vec::new();
        for &i in order.iter() {
            let pc = self.pieces[i];
            let extend = match (chains.last(), edges.last()) {
                (Some(c), Some(&ed)) => {
                    let l = c[c.len() - 1].0;
                    ed == pc.edge
                        && (l.x - pc.from.x).abs() < 1e-9
                        && (l.y - pc.from.y).abs() < 1e-9
                }
                _ => false,
            };
            if extend {
                chains
                    .last_mut()
                    .expect("extend is only true when there is a previous chain")
                    .push((pc.to, pc.to_prov));
            } else {
                chains.push(vec![(pc.from, pc.from_prov), (pc.to, pc.to_prov)]);
                edges.push(pc.edge);
            }
        }
        (chains, edges)
    }

    /// A pixel where several boundaries meet, as the wedges they cut it into.
    ///
    /// The two-face construction above cannot describe this pixel: three or more faces
    /// share it, and one chain no longer divides it. But where all the chains run from the
    /// same interior point - the junction node - out to the pixel's border, they cut the
    /// square into wedges, one per pair of chains adjacent around the border, and each
    /// wedge is one face's exact coverage. Summing them gives the pixel's rendered colour,
    /// and the node itself then appears in the gradient, so the image places it and not
    /// only the intersection of the boundaries that meet there.
    ///
    /// Those coverages are the weights of that pixel's colour as a mixture over the faces
    /// around the junction, so this is the same statement as solving a non-negative
    /// mixture for an anti-aliased pixel - with the weights constrained to be areas of an
    /// actual partition rather than free numbers, which is what keeps them consistent with
    /// the geometry the fitter will be handed.
    fn junction_pixel(
        &self,
        cell: usize,
        pos: &[Point],
        scratch: &Scratch,
        grad: Option<&mut [Point]>,
    ) -> f64 {
        juncstat::bump(&juncstat::SEEN);
        let (px, py) = ((cell % self.w) as f64, (cell / self.w) as f64);
        let (chains, edges) = self.group_chains(&scratch.order);
        if chains.len() < 2 || chains.len() > 6 {
            juncstat::bump(&juncstat::N_CHAINS);
            return 0.0;
        }
        let Some((node, outward, reversed)) = orient_outward(&chains, px, py) else {
            return 0.0;
        };
        let m = outward.len();
        let exits: Vec<f64> = outward
            .iter()
            .map(|c| perim(c[c.len() - 1].0, px, py))
            .collect();
        let mut idx: Vec<usize> = (0..m).collect();
        idx.sort_by(|&a, &b| {
            exits[a]
                .partial_cmp(&exits[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut wedges: Vec<(f64, usize, Vec<Point>, Vec<Prov>)> = Vec::with_capacity(m);
        for t in 0..m {
            let (i, j) = (idx[t], idx[(t + 1) % m]);
            let mut lp: Vec<Point> = Vec::with_capacity(12);
            let mut lv: Vec<Prov> = Vec::with_capacity(12);
            for &(p, pr) in outward[i].iter() {
                lp.push(p);
                lv.push(pr);
            }
            let mut cs: Vec<Point> = Vec::with_capacity(4);
            border_corners(exits[i], exits[j], true, px, py, &mut cs);
            for p in cs {
                lp.push(p);
                lv.push(Prov::Corner);
            }
            // Back along the next chain, stopping short of the node they share.
            for &(p, pr) in outward[j].iter().skip(1).rev() {
                lp.push(p);
                lv.push(pr);
            }
            let sl = shoelace(&lp);
            // Which face fills the wedge: walking outward along chain `i`, the side where
            // `cross(tangent, offset)` is negative is the edge's `left` when the walk
            // follows the edge's own direction, and its `right` when it does not.
            let t_out = Point::new(outward[i][1].0.x - node.x, outward[i][1].0.y - node.y);
            let mid = {
                let ei = outward[i][outward[i].len() - 1].0;
                let ej = outward[j][outward[j].len() - 1].0;
                Point::new(0.5 * (ei.x + ej.x) - node.x, 0.5 * (ei.y + ej.y) - node.y)
            };
            let cross = t_out.x * mid.y - t_out.y * mid.x;
            let e = &self.map.edges[edges[i] as usize];
            let (left, right) = if reversed[i] {
                (e.right as usize, e.left as usize)
            } else {
                (e.left as usize, e.right as usize)
            };
            let face = if cross < 0.0 { left } else { right };
            if face >= self.face.len() {
                juncstat::bump(&juncstat::FACE);
                return 0.0;
            }
            wedges.push((sl, face, lp, lv));
        }
        // The wedges have to be a partition of the pixel. Anything else means the chains
        // did not meet the way this construction assumes, and the coverages would be wrong.
        let sum: f64 = wedges.iter().map(|a| a.0.abs()).sum();
        if (sum - 1.0).abs() > 1e-6 {
            juncstat::bump(&juncstat::PARTITION);
            return 0.0;
        }
        let mut c = [0.0f64; 3];
        for (sl, face, _, _) in wedges.iter() {
            let col = self.face[*face].color_at(px, py);
            for k in 0..3 {
                c[k] += sl.abs() * col[k] as f64;
            }
        }
        let t = self.rgb[cell];
        let mut total = 0.0;
        let mut resid = [0.0f64; 3];
        for k in 0..3 {
            resid[k] = c[k] - t[k] as f64;
            total += resid[k] * resid[k];
        }
        juncstat::bump(&juncstat::OK);
        if let Some(g) = grad {
            for (sl, face, lp, lv) in wedges.iter() {
                let col = self.face[*face].color_at(px, py);
                let mut dda = 0.0;
                for k in 0..3 {
                    dda += 2.0 * resid[k] * col[k] as f64;
                }
                // The area is the absolute shoelace, so its derivative carries the sign.
                let sign = if *sl < 0.0 { -1.0 } else { 1.0 };
                let n = lp.len();
                for i in 0..n {
                    let prev = lp[(i + n - 1) % n];
                    let next = lp[(i + 1) % n];
                    let gx = 0.5 * (next.y - prev.y) * dda * sign;
                    let gy = 0.5 * (prev.x - next.x) * dda * sign;
                    scatter(lv[i], gx, gy, pos, g);
                }
            }
        }
        total
    }

    /// Kink and anchor terms, and their gradient when asked for.
    ///
    /// ```text
    ///     P = w_kink · Σ sqrt(|p_{i−1} − 2p_i + p_{i+1}|² + 1e-4)
    ///       + Σ_v w_v · |p_v − p_v⁰|²,       w_v = w_anchor (×4 at a junction)
    /// ```
    ///
    /// The kink sum runs over interior points of open edges and every point of closed
    /// ones; edges under three points have none. The small constant under the root is a
    /// Charbonnier-style smoothing of the absolute value, so the gradient is defined on a
    /// straight run.
    fn priors(&self, pos: &[Point], mut grad: Option<&mut [Point]>) -> f64 {
        // A floor inside the square root, so the absolute value is differentiable where a
        // boundary is already straight.
        const EPS: f64 = 1e-4;
        let mut total = 0.0;
        for (k, e) in self.map.edges.iter().enumerate() {
            let ids = &self.vars.var[k];
            let n = ids.len();
            if n < 3 {
                continue;
            }
            let (lo, hi) = if e.closed { (0, n) } else { (1, n - 1) };
            for i in lo..hi {
                let (ia, ib, ic) = (
                    ids[(i + n - 1) % n] as usize,
                    ids[i] as usize,
                    ids[(i + 1) % n] as usize,
                );
                let (a, b, c) = (pos[ia], pos[ib], pos[ic]);
                let (dx, dy) = (a.x - 2.0 * b.x + c.x, a.y - 2.0 * b.y + c.y);
                let m = (dx * dx + dy * dy + EPS).sqrt();
                total += self.w_kink * m;
                if let Some(g) = grad.as_deref_mut() {
                    let s = self.w_kink / m;
                    g[ia].x += s * dx;
                    g[ia].y += s * dy;
                    g[ib].x -= 2.0 * s * dx;
                    g[ib].y -= 2.0 * s * dy;
                    g[ic].x += s * dx;
                    g[ic].y += s * dy;
                }
            }
        }
        for v in 0..pos.len() {
            let w = if self.vars.junction[v] {
                self.w_anchor * JUNCTION_ANCHOR
            } else {
                self.w_anchor
            };
            let (dx, dy) = (
                pos[v].x - self.vars.start[v].x,
                pos[v].y - self.vars.start[v].y,
            );
            total += w * (dx * dx + dy * dy);
            if let Some(g) = grad.as_deref_mut() {
                g[v].x += 2.0 * w * dx;
                g[v].y += 2.0 * w * dy;
            }
        }
        total
    }

    /// The whole objective at `pos` (data + priors), re-bucketing the chains first. With
    /// `grad`, it is zeroed and then filled with the objective's gradient per unknown.
    fn energy(&mut self, pos: &[Point], grad: Option<&mut [Point]>) -> f64 {
        self.bucket(pos);
        match grad {
            Some(g) => {
                g.iter_mut().for_each(|p| *p = Point::new(0.0, 0.0));
                let d = self.data(pos, Some(&mut *g));
                d + self.priors(pos, Some(g))
            }
            None => {
                let d = self.data(pos, None);
                d + self.priors(pos, None)
            }
        }
    }
}

/// A junction pixel's chains, each oriented from the shared interior node out to the
/// pixel's border.
///
/// Returns the node, the oriented chains, and for each whether it had to be reversed
/// (so that its edge's `left` and `right` swap). `None`, counted in [`juncstat`], when a
/// chain does not have exactly one end on the border or the chains do not all start at
/// the same node: then the wedge construction does not apply.
fn orient_outward(chains: &[Chain], px: f64, py: f64) -> Option<(Point, Vec<Chain>, Vec<bool>)> {
    // Every chain has to run from the shared interior node out to the border. Orient
    // it that way, and require them all to start at the same node.
    let mut node = Point::new(f64::NAN, f64::NAN);
    let mut outward: Vec<Chain> = Vec::with_capacity(chains.len());
    let mut reversed: Vec<bool> = Vec::with_capacity(chains.len());
    for c in chains.iter() {
        let (a, b) = (c[0].0, c[c.len() - 1].0);
        let (sa, sb) = (perim(a, px, py), perim(b, px, py));
        let rev = match (sa.is_finite(), sb.is_finite()) {
            (false, true) => false,
            (true, false) => true,
            _ => {
                juncstat::bump(&juncstat::ENDS);
                return None;
            }
        };
        let mut v = c.clone();
        if rev {
            v.reverse();
        }
        if node.x.is_nan() {
            node = v[0].0;
        } else if (node.x - v[0].0.x).abs() > 1e-9 || (node.y - v[0].0.y).abs() > 1e-9 {
            juncstat::bump(&juncstat::NODE);
            return None;
        }
        outward.push(v);
        reversed.push(rev);
    }
    Some((node, outward, reversed))
}

/// What the stage did, for the run log.
pub struct Report {
    /// Objective energy before the solve.
    pub before: f64,
    /// Objective energy after the solve.
    pub after: f64,
    /// Number of conjugate-gradient iterations that improved the energy.
    pub iters: usize,
    /// Number of vertices moved.
    pub moved: usize,
    /// How much of the solved displacement survived the self-crossing guard.
    pub scale: f64,
}

/// Solve the boundary against the image, in place. `None` when nothing was gained.
pub fn optimise(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    face: &[FillModel],
    budget_ms: Option<u64>,
) -> Option<Report> {
    optimise_alpha(map, rgb, face, budget_ms, None)
}

/// [`optimise`] with alpha as a fourth channel of the data term: `alpha` is the source's
/// alpha per pixel and each face's opacity. With `None` this is exactly [`optimise`].
///
/// `rgb` is the source image (sRGB `0..1`, row-major, the map's size) and `face[f]` face
/// `f`'s fill model. `budget_ms` is the caller's wall-clock budget for the descent; with
/// `None` the result depends only on the input. Returns `None`, leaving the map as it
/// was, when the input is degenerate, the energy did not fall, or the fold guard could
/// not keep enough of the displacement. On success every edge's points are overwritten
/// (sigmas are not touched) and the report says how much moved.
pub fn optimise_alpha(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    face: &[FillModel],
    budget_ms: Option<u64>,
    alpha: Option<(&[f32], &[f32])>,
) -> Option<Report> {
    let (w, h) = (map.width, map.height);
    if w == 0 || h == 0 || map.edges.is_empty() || rgb.len() < w * h {
        return None;
    }
    let vars = build_vars(map);
    let n = vars.start.len();
    if n < 3 {
        return None;
    }
    let budget: Option<u128> = budget_ms.map(u128::from);
    let dbg = inkvec_core::env::flag("INKVEC_BOPTDBG");

    let (report, pos) = {
        let mut prob = Problem {
            map: &*map,
            vars: &vars,
            rgb,
            face,
            w,
            h,
            pieces: Vec::with_capacity(4096),
            head: vec![-1; w * h],
            scratch: Scratch::default(),
            alpha,
            w_kink: 1.0,
            w_anchor: 0.0,
            // Junction wedges: an experiment, off unless asked for in a `research` build.
            junctions: cfg!(feature = "research") && inkvec_core::env::flag("INKVEC_BOPT_JUNC"),
            touched: Vec::new(),
            cells_dbg: inkvec_core::env::flag("INKVEC_BOPT_CELLS"),
            chunks: research_chunks(),
        };
        descend(&mut prob, &vars, budget, dbg)?
    };

    let (scaled, scale) = fold_guard(map, &vars, &pos, dbg)?;
    let moved = write_back(map, &vars, &scaled);
    if inkvec_core::env::flag("INKVEC_JUNCDBG") {
        juncstat::report();
    }
    Some(Report {
        moved,
        scale,
        ..report
    })
}

/// Minimise `prob`'s energy from the measured positions by nonlinear conjugate gradient
/// (see the module docs). Sets the prior weights from the starting residual first.
///
/// Returns the report (energies and iterations; `moved` and `scale` are filled in by the
/// caller) and the final positions, or `None` when there is no residual to reduce or no
/// iteration reduced it.
fn descend(
    prob: &mut Problem,
    vars: &Vars,
    budget: Option<u128>,
    dbg: bool,
) -> Option<(Report, Vec<Point>)> {
    let n = vars.start.len();
    // 48, not 24: at 24 this solve stops before it has converged, and the boundary
    // it hands on is still moving. `gt_diff` attributes 94% of the remaining error
    // to boundaries, so that mattered more than any threshold in the tracer.
    //
    // Full set, 980 icons: objective 0.4519 -> 0.4428, with dE00 0.1660 -> 0.1620,
    // DISTS 0.0286 -> 0.0281 and parameters against the artist 1.46 -> 1.42. Every
    // family improves or holds; simple-icons goes 1.31 -> 1.13 on parameters and
    // material-icons 0.0743 -> 0.0604 on dE00. Improving fidelity and cost together
    // is what says this is convergence rather than a trade.
    //
    // It is not monotone past that -- 96 reads 0.4157 and 192 reads 0.4160 on the
    // screen split against 48's 0.4142 -- so the step schedule drifts once the
    // residual stops driving it, and more iterations are not better iterations.
    //
    // Nearly free: 573 ms/icon to 578 ms. The iteration count is the bound. A wall clock
    // runs only under a caller's time budget: the fixed 1200 ms one this had never bound
    // on the corpus (a 60 s budget gave the same 0.4142) and made the answer depend on how
    // fast the machine was -- WebAssembly or a loaded CI runner stopped sooner and wrote
    // different bytes. (`INKVEC_BOPT_ITERS` and `INKVEC_BOPT_MS`, which overrode both, are
    // gone: the count is `ITERS`, and a clock is the caller's time budget.)
    let iters = ITERS;
    let mut pos: Vec<Point> = vars.start.clone();

    // The weights are set from the residual the measurement starts with, so that a
    // prior means the same thing whatever the icon's contrast and size.
    prob.bucket(&pos);
    let data0 = prob.data(&pos, None);
    let kink0 = prob.priors(&pos, None);
    if data0 <= 0.0 || kink0 <= 0.0 {
        return None;
    }
    prob.w_kink = K_KINK * data0 / kink0;
    prob.w_anchor = K_ANCHOR * data0 / n as f64;

    let mut grad = vec![Point::new(0.0, 0.0); n];
    let mut e = prob.energy(&pos, Some(&mut grad));
    let e0 = e;
    let mut dir: Vec<Point> = grad.iter().map(|g| Point::new(-g.x, -g.y)).collect();
    let mut gg: f64 = grad.iter().map(|g| g.x * g.x + g.y * g.y).sum();
    let mut trial = pos.clone();
    let mut newgrad = vec![Point::new(0.0, 0.0); n];
    let clock = Instant::now();
    let mut done = 0usize;

    for it in 0..iters {
        if budget.is_some_and(|b| clock.elapsed().as_millis() > b) {
            break;
        }
        // `iters` is the ceiling; a solve that converges stops well short of it.
        inkvec_core::progress::step("iterations", it as u64, iters as u64);
        let dmax = dir.iter().map(|d| d.x.hypot(d.y)).fold(0.0f64, f64::max);
        if dmax < 1e-12 {
            break;
        }
        // Backtracking line search along `dir`: the first trial moves the furthest point
        // `MAX_STEP`, each rejection shrinks the step by 0.4, and every trial point is
        // clamped to within `MAX_TOTAL` of its start. The first trial that lowers the
        // energy is taken.
        let mut step = MAX_STEP / dmax;
        let mut stop = true;
        for _ in 0..6 {
            // Each trial is a full render of the energy: worth a cancellation point.
            inkvec_core::progress::checkpoint();
            for v in 0..n {
                let mut q = Point::new(pos[v].x + dir[v].x * step, pos[v].y + dir[v].y * step);
                let (dx, dy) = (q.x - vars.start[v].x, q.y - vars.start[v].y);
                let d = dx.hypot(dy);
                if d > MAX_TOTAL {
                    let s = MAX_TOTAL / d;
                    q = Point::new(vars.start[v].x + dx * s, vars.start[v].y + dy * s);
                }
                trial[v] = q;
            }
            let et = prob.energy(&trial, Some(&mut newgrad));
            if et < e {
                let rel = (e - et) / e.max(1e-12);
                std::mem::swap(&mut pos, &mut trial);
                std::mem::swap(&mut grad, &mut newgrad);
                e = et;
                done = it + 1;
                if dbg {
                    eprintln!("  [bopt] it {it} step {step:.4} E {e:.2} rel {rel:.5}");
                }
                // Keep the step, and stop if it bought almost nothing.
                stop = rel < 1e-4;
                break;
            }
            step *= 0.4;
        }
        if stop {
            break;
        }
        // Fletcher-Reeves: beta = |g_new|² / |g_old|², d = beta·d − g_new.
        let gg_new: f64 = grad.iter().map(|g| g.x * g.x + g.y * g.y).sum();
        let beta = if gg > 1e-30 { gg_new / gg } else { 0.0 };
        gg = gg_new;
        let mut descent = 0.0;
        for v in 0..n {
            dir[v] = Point::new(beta * dir[v].x - grad[v].x, beta * dir[v].y - grad[v].y);
            descent += dir[v].x * grad[v].x + dir[v].y * grad[v].y;
        }
        // Fletcher-Reeves can hand back a direction that is not downhill; restart then.
        if descent >= 0.0 {
            for v in 0..n {
                dir[v] = Point::new(-grad[v].x, -grad[v].y);
            }
        }
    }
    if e >= e0 || done == 0 {
        return None;
    }
    Some((
        Report {
            before: e0,
            after: e,
            iters: done,
            moved: 0,
            scale: 1.0,
        },
        pos,
    ))
}

/// Scale the whole displacement back until it adds no fold of its own.
///
/// With `p⁰` the start and `p` the solution, tries `p⁰ + s(p − p⁰)` for
/// `s = 1, ½, ¼, …` while `s > 0.1`, and keeps the first whose self-crossing count
/// ([`crossings_count`]) is no higher than the start's. Returns those positions and `s`,
/// or `None` when no tried scale is clean.
fn fold_guard(map: &PlanarMap, vars: &Vars, pos: &[Point], dbg: bool) -> Option<(Vec<Point>, f64)> {
    let n = vars.start.len();
    let base = crossings_count(map, vars, &vars.start);
    let mut scaled = pos.to_vec();
    let mut scale = 1.0;
    let mut ok = crossings_count(map, vars, &scaled) <= base;
    while !ok && scale > 0.1 {
        scale *= 0.5;
        for v in 0..n {
            scaled[v] = Point::new(
                vars.start[v].x + (pos[v].x - vars.start[v].x) * scale,
                vars.start[v].y + (pos[v].y - vars.start[v].y) * scale,
            );
        }
        ok = crossings_count(map, vars, &scaled) <= base;
    }
    if dbg {
        eprintln!("  [bopt] folds before {base}, scale kept {scale:.3}, accepted {ok}");
    }
    if !ok {
        return None;
    }
    Some((scaled, scale))
}

/// Copy the solved unknowns back onto every edge's points (shared ends get the same
/// value on every edge) and count the points that moved by more than `1e-6` px.
fn write_back(map: &mut PlanarMap, vars: &Vars, pos: &[Point]) -> usize {
    let mut moved = 0usize;
    for (k, e) in map.edges.iter_mut().enumerate() {
        for i in 0..e.points.len() {
            let p = pos[vars.var[k][i] as usize];
            if e.points[i].dist(p) > 1e-6 {
                moved += 1;
            }
            e.points[i] = p;
        }
    }
    moved
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod solve_tests;
