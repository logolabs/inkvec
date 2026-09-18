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
const K_KINK: f64 = 0.05;
const K_ANCHOR: f64 = 0.10;
/// Junction points are anchored harder, for the reason given in the module comment.
const JUNCTION_ANCHOR: f64 = 4.0;
/// Colour difference across a boundary below which a pixel carries no usable evidence.
const MIN_CONTRAST: f32 = 2.0 / 255.0;

fn env_usize(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_f64(var: &str, default: f64) -> f64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

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
    edge: u32,
    from: Point,
    to: Point,
    from_prov: Prov,
    to_prov: Prov,
    next: i32,
}

/// The unknowns: one per boundary point, with the endpoints several edges share collapsed
/// into one.
struct Vars {
    /// `var[edge][i]` is the unknown holding point `i` of that edge.
    var: Vec<Vec<u32>>,
    start: Vec<Point>,
    junction: Vec<bool>,
}

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

/// Gridline crossings of one segment, as parameters in `(0, 1)`, in order.
fn crossings(a: Point, b: Point, va: u32, vb: u32, out: &mut Vec<(f64, Prov)>) {
    out.clear();
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    if dx.abs() > 1e-12 {
        let (lo, hi) = if a.x < b.x { (a.x, b.x) } else { (b.x, a.x) };
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
    if dy.abs() > 1e-12 {
        let (lo, hi) = if a.y < b.y { (a.y, b.y) } else { (b.y, a.y) };
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

/// Signed shoelace of a closed loop.
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
/// crossing is always local.
fn crossings_count(map: &PlanarMap, vars: &Vars, pos: &[Point], _w: usize, _h: usize) -> usize {
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

/// Reused scratch, so an energy evaluation allocates nothing.
#[derive(Default)]
struct Scratch {
    cross: Vec<(f64, Prov)>,
    order: Vec<usize>,
    loop_pts: Vec<Point>,
    loop_prov: Vec<Prov>,
    corners: Vec<Point>,
}

struct Problem<'a> {
    map: &'a PlanarMap,
    vars: &'a Vars,
    rgb: &'a [[f32; 3]],
    face: &'a [FillModel],
    w: usize,
    h: usize,
    pieces: Vec<Piece>,
    head: Vec<i32>,
    scratch: Scratch,
    w_kink: f64,
    w_anchor: f64,
    /// Whether junction pixels take part in the data term.
    junctions: bool,
}

impl Problem<'_> {
    /// Sort every chain piece into the pixel it lies in.
    fn bucket(&mut self, pos: &[Point]) {
        self.pieces.clear();
        self.head.iter_mut().for_each(|s| *s = -1);
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
                            self.head[cell] = id;
                        }
                    }
                    t0 = t1;
                    from = to;
                    from_prov = to_prov;
                }
            }
        }
    }

    /// Data term, and its gradient when asked for. `bucket` must have run on `pos`.
    /// The data term over every pixel.
    ///
    /// Sequential by default, so the floating-point summation order and therefore the
    /// output are exactly what they always were. `INKVEC_BOPT_CHUNKS=16` sums the cells
    /// in that many fixed contiguous ranges on every core, each with its own scratch and
    /// gradient, added in range order: deterministic on any machine and about 60 ms
    /// faster at 2048 px, but the last-bit differences cascade through tie-sensitive fit
    /// decisions -- on one 300-path logo they cost 8 paths and 16 % more coordinates at
    /// the same colour error -- so it stays opt-in until the full set has priced it.
    fn data(&mut self, pos: &[Point], grad: Option<&mut [Point]>) -> f64 {
        let chunks = std::env::var("INKVEC_BOPT_CHUNKS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);
        let cells = self.w * self.h;
        if chunks == 1 || cells < 4096 {
            let mut scratch = std::mem::take(&mut self.scratch);
            let t = self.data_cells(0..cells, pos, &mut scratch, grad);
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

    fn data_cells(
        &self,
        cells: std::ops::Range<usize>,
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
            if order.windows(2).any(|pair| {
                let (a, b) = (self.pieces[pair[0]].to, self.pieces[pair[1]].from);
                (a.x - b.x).abs() > 1e-9 || (a.y - b.y).abs() > 1e-9
            }) {
                continue;
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
                continue;
            }
            let e = &self.map.edges[edge as usize];
            let cl = self.face[e.left as usize].color_at(px, py);
            let cr = self.face[e.right as usize].color_at(px, py);
            let contrast = (0..3).map(|k| (cl[k] - cr[k]).abs()).fold(0.0f32, f32::max);
            if contrast < MIN_CONTRAST {
                continue;
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
                continue;
            }
            if std::env::var_os("INKVEC_BOPT_CELLS").is_some() {
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
                total += r * r;
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
        total
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
        // Group the pieces by edge, in walk order, requiring each group to be contiguous.
        let order = scratch.order.clone();
        let mut chains: Vec<Vec<(Point, Prov)>> = Vec::new();
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
                chains.last_mut().unwrap().push((pc.to, pc.to_prov));
            } else {
                chains.push(vec![(pc.from, pc.from_prov), (pc.to, pc.to_prov)]);
                edges.push(pc.edge);
            }
        }
        if chains.len() < 2 || chains.len() > 6 {
            juncstat::bump(&juncstat::N_CHAINS);
            return 0.0;
        }
        // Every chain has to run from the shared interior node out to the border. Orient
        // it that way, and require them all to start at the same node.
        let mut node = Point::new(f64::NAN, f64::NAN);
        let mut outward: Vec<Vec<(Point, Prov)>> = Vec::with_capacity(chains.len());
        let mut reversed: Vec<bool> = Vec::with_capacity(chains.len());
        for c in chains.iter() {
            let (a, b) = (c[0].0, c[c.len() - 1].0);
            let (sa, sb) = (perim(a, px, py), perim(b, px, py));
            let rev = match (sa.is_finite(), sb.is_finite()) {
                (false, true) => false,
                (true, false) => true,
                _ => {
                    juncstat::bump(&juncstat::ENDS);
                    return 0.0;
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
                return 0.0;
            }
            outward.push(v);
            reversed.push(rev);
        }
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
    let (w, h) = (map.width, map.height);
    if w == 0 || h == 0 || map.edges.is_empty() || rgb.len() < w * h {
        return None;
    }
    let vars = build_vars(map);
    let n = vars.start.len();
    if n < 3 {
        return None;
    }
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
    // Nearly free: 573 ms/icon to 578 ms, and the 1200 ms budget below was never
    // the binding constraint. Measuring at a 60 s budget gave the same 0.4142.
    let iters = env_usize("INKVEC_BOPT_ITERS", 48);
    let budget = budget_ms
        .map(|b| b as u128)
        .unwrap_or_else(|| env_usize("INKVEC_BOPT_MS", 1200) as u128);
    let dbg = std::env::var_os("INKVEC_BOPTDBG").is_some();

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
            w_kink: 1.0,
            w_anchor: 0.0,
            junctions: std::env::var("INKVEC_BOPT_JUNC").is_ok_and(|v| v != "0"),
        };
        let mut pos: Vec<Point> = vars.start.clone();

        // The weights are set from the residual the measurement starts with, so that a
        // prior means the same thing whatever the icon's contrast and size.
        prob.bucket(&pos);
        let data0 = prob.data(&pos, None);
        let kink0 = prob.priors(&pos, None);
        if data0 <= 0.0 || kink0 <= 0.0 {
            return None;
        }
        prob.w_kink = env_f64("INKVEC_BOPT_KINK", K_KINK) * data0 / kink0;
        prob.w_anchor = env_f64("INKVEC_BOPT_ANCHOR", K_ANCHOR) * data0 / n as f64;

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
            if clock.elapsed().as_millis() > budget {
                break;
            }
            let dmax = dir.iter().map(|d| d.x.hypot(d.y)).fold(0.0f64, f64::max);
            if dmax < 1e-12 {
                break;
            }
            let mut step = MAX_STEP / dmax;
            let mut stop = true;
            for _ in 0..6 {
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
        (
            Report {
                before: e0,
                after: e,
                iters: done,
                moved: 0,
                scale: 1.0,
            },
            pos,
        )
    };

    // Scale the whole displacement back until it adds no fold of its own.
    let base = crossings_count(map, &vars, &vars.start, w, h);
    let mut scaled = pos.clone();
    let mut scale = 1.0;
    let mut ok = crossings_count(map, &vars, &scaled, w, h) <= base;
    while !ok && scale > 0.1 {
        scale *= 0.5;
        for v in 0..n {
            scaled[v] = Point::new(
                vars.start[v].x + (pos[v].x - vars.start[v].x) * scale,
                vars.start[v].y + (pos[v].y - vars.start[v].y) * scale,
            );
        }
        ok = crossings_count(map, &vars, &scaled, w, h) <= base;
    }
    if dbg {
        eprintln!("  [bopt] folds before {base}, scale kept {scale:.3}, accepted {ok}");
    }
    if !ok {
        return None;
    }
    let mut moved = 0usize;
    for (k, e) in map.edges.iter_mut().enumerate() {
        for i in 0..e.points.len() {
            let p = scaled[vars.var[k][i] as usize];
            if e.points[i].dist(p) > 1e-6 {
                moved += 1;
            }
            e.points[i] = p;
        }
    }
    if std::env::var_os("INKVEC_JUNCDBG").is_some() {
        juncstat::report();
    }
    Some(Report {
        moved,
        scale,
        ..report
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Coverage of a pixel split by a straight vertical boundary, read off the geometry the
    /// optimiser builds, against the area computed by hand.
    #[test]
    fn coverage_of_a_split_pixel() {
        // Two faces, boundary at x = 0.3 through a 3x1 strip of pixels.
        let mut map = PlanarMap {
            edges: vec![crate::planar::Edge {
                points: vec![Point::new(0.3, -0.5), Point::new(0.3, 2.5)],
                sigma: vec![0.5, 0.5],
                left: 0,
                right: 1,
                start_node: 0,
                end_node: 1,
                closed: false,
                lambda_scale: 1.0,
            }],
            width: 3,
            height: 3,
            n_labels: 2,
        };
        let vars = build_vars(&map);
        let face = vec![
            FillModel::Flat([1.0, 1.0, 1.0]),
            FillModel::Flat([0.0, 0.0, 0.0]),
        ];
        // Pixel (0, y) spans x in -0.5..0.5, so the boundary at x = 0.3 leaves a fifth of
        // it on the +x side, and for a chain walking +y that side is `left` (the planar
        // map's convention). The rendered value is therefore 0.2 of the left face's white.
        let rgb = vec![[0.2, 0.2, 0.2]; 9];
        let mut prob = Problem {
            map: &map,
            vars: &vars,
            rgb: &rgb,
            face: &face,
            w: 3,
            h: 3,
            pieces: Vec::new(),
            head: vec![-1; 9],
            scratch: Scratch::default(),
            w_kink: 0.0,
            w_anchor: 0.0,
            junctions: true,
        };
        let pos = vars.start.clone();
        prob.bucket(&pos);
        // Pixels (0,0), (0,1) and (0,2) are each cut in two, and each must read exactly
        // the mixture the geometry implies.
        let e = prob.data(&pos, None);
        assert!(e < 1e-9, "expected an exact fit, got {e}");
        assert_eq!(prob.pieces.len(), 3, "one piece per pixel crossed");
        // And the sign matters: a target of 0.8 would be the other side, and wrong.
        let rgb2 = vec![[0.8, 0.8, 0.8]; 9];
        prob.rgb = &rgb2;
        assert!(
            prob.data(&pos, None) > 1.0,
            "left/right must not be interchangeable"
        );
        let _ = &mut map;
    }

    /// A three-way junction: the wedges must partition the pixel, and the node must feel
    /// the image through them.
    #[test]
    fn junction_wedges_partition_the_pixel() {
        // One node at the centre of pixel (1, 1), three boundaries leaving it. Directions
        // are deliberately off the diagonals so that no chain exits through a corner.
        let edge = |to: Point, left: u16, right: u16, end: u32| crate::planar::Edge {
            points: vec![Point::new(1.0, 1.0), to],
            sigma: vec![0.5, 0.5],
            left,
            right,
            start_node: 0,
            end_node: end,
            closed: false,
            lambda_scale: 1.0,
        };
        let map = PlanarMap {
            edges: vec![
                edge(Point::new(1.0, -0.5), 2, 0, 1),
                edge(Point::new(-0.5, 2.0), 1, 2, 2),
                edge(Point::new(2.5, 2.2), 0, 1, 3),
            ],
            width: 3,
            height: 3,
            n_labels: 3,
        };
        let vars = build_vars(&map);
        // The three edges share their first point, so it is one unknown.
        assert_eq!(vars.var[0][0], vars.var[1][0]);
        assert_eq!(vars.var[0][0], vars.var[2][0]);
        assert!(vars.junction[vars.var[0][0] as usize]);
        let face = vec![
            FillModel::Flat([1.0, 0.0, 0.0]),
            FillModel::Flat([0.0, 1.0, 0.0]),
            FillModel::Flat([0.0, 0.0, 1.0]),
        ];
        let rgb = vec![[0.4, 0.4, 0.2]; 9];
        let mut prob = Problem {
            map: &map,
            vars: &vars,
            rgb: &rgb,
            face: &face,
            w: 3,
            h: 3,
            pieces: Vec::new(),
            head: vec![-1; 9],
            scratch: Scratch::default(),
            w_kink: 0.0,
            w_anchor: 0.0,
            junctions: false,
        };
        let pos = vars.start.clone();
        prob.bucket(&pos);
        let without = prob.data(&pos, None);
        prob.junctions = true;
        prob.bucket(&pos);
        let with = prob.data(&pos, None);
        // The junction pixel was accepted, so it added a residual of its own. Had the
        // wedges failed to partition the square, the term would have been skipped.
        assert!(
            with > without + 1e-9,
            "junction pixel contributed nothing: {without} vs {with}"
        );

        // And the node moves under the image: analytic gradient against a central
        // difference, at the junction unknown.
        let n = vars.start.len();
        let mut grad = vec![Point::new(0.0, 0.0); n];
        prob.w_kink = 0.3;
        prob.w_anchor = 0.2;
        let pos: Vec<Point> = vars
            .start
            .iter()
            .enumerate()
            .map(|(i, p)| Point::new(p.x + 0.013 * i as f64, p.y - 0.011 * i as f64))
            .collect();
        prob.energy(&pos, Some(&mut grad));
        let d = 1e-6;
        let v = vars.var[0][0] as usize;
        for axis in 0..2 {
            let mut plus = pos.clone();
            let mut minus = pos.clone();
            if axis == 0 {
                plus[v].x += d;
                minus[v].x -= d;
            } else {
                plus[v].y += d;
                minus[v].y -= d;
            }
            let num = (prob.energy(&plus, None) - prob.energy(&minus, None)) / (2.0 * d);
            let ana = if axis == 0 { grad[v].x } else { grad[v].y };
            assert!(
                (num - ana).abs() < 1e-3 * (1.0 + ana.abs()),
                "junction axis {axis}: analytic {ana}, numeric {num}"
            );
        }
    }

    /// The analytic gradient against a central difference of the energy.
    #[test]
    fn gradient_matches_finite_differences() {
        let map = PlanarMap {
            edges: vec![crate::planar::Edge {
                // Deliberately off the gridlines: a point sitting exactly on one is a
                // configuration where the objective is not differentiable, because moving
                // it changes which pixel its piece belongs to.
                points: vec![
                    Point::new(0.13, -0.42),
                    Point::new(0.35, 0.61),
                    Point::new(0.19, 1.43),
                    Point::new(0.31, 2.38),
                ],
                sigma: vec![0.5; 4],
                left: 0,
                right: 1,
                start_node: 0,
                end_node: 1,
                closed: false,
                lambda_scale: 1.0,
            }],
            width: 3,
            height: 3,
            n_labels: 2,
        };
        let vars = build_vars(&map);
        let face = vec![
            FillModel::Flat([1.0, 0.9, 0.8]),
            FillModel::Flat([0.1, 0.2, 0.3]),
        ];
        let rgb = vec![[0.55, 0.5, 0.45]; 9];
        let mut prob = Problem {
            map: &map,
            vars: &vars,
            rgb: &rgb,
            face: &face,
            w: 3,
            h: 3,
            pieces: Vec::new(),
            head: vec![-1; 9],
            scratch: Scratch::default(),
            w_kink: 0.7,
            w_anchor: 0.3,
            junctions: true,
        };
        let n = vars.start.len();
        // Away from the start, so the anchor term is not sitting at its minimum.
        let pos: Vec<Point> = vars
            .start
            .iter()
            .enumerate()
            .map(|(i, p)| Point::new(p.x + 0.01 * i as f64, p.y - 0.007 * i as f64))
            .collect();
        for (wk, wa) in [(0.0, 0.0), (0.7, 0.0), (0.0, 0.3), (0.7, 0.3)] {
            prob.w_kink = wk;
            prob.w_anchor = wa;
            let mut grad = vec![Point::new(0.0, 0.0); n];
            prob.energy(&pos, Some(&mut grad));
            let d = 1e-6;
            for v in 0..n {
                for axis in 0..2 {
                    let mut plus = pos.clone();
                    let mut minus = pos.clone();
                    if axis == 0 {
                        plus[v].x += d;
                        minus[v].x -= d;
                    } else {
                        plus[v].y += d;
                        minus[v].y -= d;
                    }
                    let ep = prob.energy(&plus, None);
                    let em = prob.energy(&minus, None);
                    let num = (ep - em) / (2.0 * d);
                    let ana = if axis == 0 { grad[v].x } else { grad[v].y };
                    assert!(
                        (num - ana).abs() < 1e-3 * (1.0 + ana.abs()),
                        "kink {wk} anchor {wa} var {v} axis {axis}: analytic {ana}, numeric {num}"
                    );
                }
            }
        }
    }
}
