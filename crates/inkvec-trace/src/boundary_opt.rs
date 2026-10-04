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
//! E = Σ_{p ∈ B} w_p · ‖ Σ_f cov_f(p)·c_f(p) − t_p ‖²
//!   + w_kink   · Σ over points sqrt(|p_{i−1} − 2p_i + p_{i+1}|² + 10⁻⁴)
//!   + w_anchor · Σ over points |p_i − p_i⁰|²
//! ```
//!
//! where `cov_f(p)` is the **exact area** of face `f` inside pixel `p`, rendered from the
//! current points, `c_f(p)` the face's fill at the pixel centre and `t_p` the image. The
//! sum runs over a band `B` of pixels fixed for the whole solve: every pixel within one
//! pixel of the boundary where it starts, which no point can leave since none moves more
//! than `MAX_TOTAL`. The `band` module has the construction and its sources (Chan–Vese
//! region fidelity, a narrow band, exact coverage by signed-area accumulation). Rendering
//! every band pixel, rather than only the pixels a boundary happens to cut, is what makes
//! the energy continuous: leaving a pixel costs what being a pure pixel of the other face
//! costs. The first form of this stage summed only the cut pixels, deciding in each which
//! side was which; that made the energy jump whenever a boundary left a pixel, and put a
//! piece lying on a pixel border on the wrong side (a quarter of the start's residual on
//! the screen set was that tie), and its line search spent most of its solves stopping on
//! those jumps rather than converging.
//!
//! The two priors supply what a raster cannot. The kink term is the **absolute** value of
//! the second difference, not its square: a corner then costs in proportion to how sharply
//! it turns, so one sharp corner is cheaper than the many small kinks a squared term would
//! spread it into — the staircase is smoothed and the corner survives. The anchor term
//! removes the tangential freedom and holds a point the image cannot see (the interior of
//! a long straight run, a boundary between two nearly equal colours) where the measurement
//! put it.
//!
//! # The sawtooth, and four cures that do not work
//!
//! Area coverage does not determine a boundary. Any wiggle that preserves how much of each
//! pixel falls on either side leaves the data term exactly unchanged, and on a stroke about
//! two pixels wide — where both of its sides compete for the same pixels — a solver can
//! wander into that null space and return a row of triangular teeth. They render almost as
//! well as a straight edge and look nothing like one: `openmoji/1F3A1` moved by 0.008 in
//! colour error while turning a smooth grey stroke into a saw.
//!
//! Measured 2026-09-03, all rejected, none shipped: twenty times the kink weight (clears
//! the teeth, costs detail: DISTS 0.0310 to 0.0363 on 246 icons); a length term (barely
//! touches them); anchoring each point by its measured uncertainty (does not remove them);
//! stopping early (wins on thin strokes, loses on the rest). A ribbon two pixels wide has no
//! interior, so its two sides are not two independent boundaries: that is LOG-44, fitting a
//! thin face as a centreline and a width. Until then the pixels such a ribbon's two sides
//! share are in the data term like any other (see `band` for why they no longer are left
//! out); the kink prior and the fold guard are what hold its two sides apart.
//!
//! # Where this sits, and how it is solved
//!
//! Quality mode, and Fast's `balanced` mode capped by iterations ([`optimise_alpha_capped`]):
//! the crate root's trace calls it after
//! `planar::refine_subpixel_alpha` and `planar::refine_junctions`, and before symmetry is
//! re-imposed and the edges go to the fitter. It takes the [`PlanarMap`], the source image
//! (sRGB `0..1`, row-major) and each face's fill model, and moves the map's points in
//! place. Coordinates are in px with pixel centres at integer coordinates, so pixel
//! `(x, y)` is the square `[x−0.5, x+0.5] x [y−0.5, y+0.5]`.
//!
//! The energy is minimised by limited-memory BFGS with a Moré–Thuente (strong Wolfe) line
//! search, separately on each group of boundaries that share no pixel and no point (the
//! `lbfgs` and `linesearch` modules have the method, its sources, and what it replaced). A
//! step moves no point more than `MAX_STEP`, every point stays within `MAX_TOTAL` of where
//! the measurement put it, and points on the image frame only slide along it. A group stops
//! on the projected gradient or the relative decrease of the energy, both measured against
//! the whole problem at the start, after 64 iterations (or the caller's lower cap,
//! [`optimise_alpha_capped`]), when no step of sufficient decrease
//! is found, or when the caller's time budget runs out; never on the length of a step.
//! Nothing is linearised: each trial re-renders the exact coverage.
//!
//! Afterwards the fold guard ([`fold_guard_local`]) backs off, by halves and at most to
//! where they started, only the boundaries that take part in a self-crossing the solve
//! made, and keeps every other boundary's solution.
//!
//! The data term is accumulated in f64, and the mixture `Σ cov_f·c_f` is formed in f64 too:
//! in f32 the energy is a staircase the line search cannot descend.
//!
//! Measured against the per-pixel term with Fletcher–Reeves conjugate gradients it
//! replaced: on the 246-icon screen set, objective 0.3873 to 0.3585, mean dE00 0.1563 to
//! 0.1427 (155 icons better, 82 worse), the worst tenth 0.5048 to 0.4669, DISTS 0.0259 to
//! 0.0242, parameters against the artist 1.488 to 1.503; on the held-out 156 (`held_a`),
//! objective 0.3958 to 0.3366, dE00 0.1480 to 0.1366 (104 better, 46 worse), the worst tenth
//! 0.4626 to 0.4183, DISTS 0.0243 to 0.0218, parameters 1.477 to 1.472. The stage takes
//! about as long as before (a fifth less on the screen set's icons; within 7 % on three of
//! the four large standard inputs, a fifth more on a Noto emoji with twenty gradients).

use inkvec_core::clock::Instant;
use std::collections::HashMap;

use inkvec_core::Point;

use crate::gradient::FillModel;
use crate::planar::PlanarMap;

mod band;
mod folds;
mod lbfgs;
mod linesearch;

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
/// Junction points are anchored harder: `planar::refine_junctions` has already placed them.
const JUNCTION_ANCHOR: f64 = 4.0;
/// Colour difference across a boundary below which a pixel carries no usable evidence.
const MIN_CONTRAST: f32 = 2.0 / 255.0;

/// Where an end of a piece came from, and so how it moves.
#[derive(Clone, Copy)]
enum Prov {
    /// A boundary point, moving with its unknown.
    Vertex(u32),
    /// Where the segment `a`→`b` crosses the vertical gridline `line`.
    CrossV { line: f64, a: u32, b: u32 },
    /// Where the segment `a`→`b` crosses the horizontal gridline `line`.
    CrossH { line: f64, a: u32, b: u32 },
}

/// One piece of one boundary chain, lying inside one pixel.
#[derive(Clone, Copy)]
struct Piece {
    /// Index of the edge in the map.
    edge: u32,
    /// The edge's left and right faces.
    l: u16,
    r: u16,
    /// Where the piece enters the pixel (or starts inside it).
    from: Point,
    /// Where the piece leaves the pixel (or ends inside it).
    to: Point,
    from_prov: Prov,
    to_prov: Prov,
    /// Next piece in the same pixel's list (a singly linked list threaded through
    /// `Problem::pieces`, headed by `Problem::head` or `Problem::vhead`), or -1.
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
    /// Coordinates held on the image frame: bit 0 x, bit 1 y (see `band::pin_frame`).
    pin: Vec<u8>,
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
    let pin = vec![0; start.len()];
    Vars {
        var,
        start,
        junction,
        pin,
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

/// Push the gradient with respect to one end of a piece back onto the unknowns it depends
/// on.
///
/// `(gx, gy)` is `∂E/∂q` for the piece's end `q`; this adds `∂E/∂q · ∂q/∂p` to `grad`
/// for each unknown `p` that `q` depends on (the chain rule). A boundary point is its own
/// unknown. A gridline crossing
/// `q = a + t(b − a)`, with `t = (line − a.x)/(b.x − a.x)` on a vertical gridline, has a
/// fixed `x` and a `y` that depends on both ends: `∂q.y/∂a.y = 1 − t`, `∂q.y/∂b.y = t`,
/// `∂q.y/∂a.x = dy(t − 1)/dx`, `∂q.y/∂b.x = −dy·t/dx`. Horizontal gridlines swap the
/// roles of x and y. A crossing on a segment parallel to its gridline (`|dx| < 1e-9`)
/// contributes nothing.
fn scatter(prov: Prov, gx: f64, gy: f64, pos: &[Point], grad: &mut [Point]) {
    match prov {
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

/// Reused scratch, so an energy evaluation allocates little.
#[derive(Default)]
struct Scratch {
    /// Gridline crossings of the segment being cut.
    cross: Vec<(f64, Prov)>,
}

/// The boundary solve's energy, and everything it needs to evaluate it.
///
/// `energy(pos)` is data + priors (see the module docs) at the unknown positions `pos`,
/// over the band set up by `band::setup`, restricted to `active` when that is set.
struct Problem<'a> {
    map: &'a PlanarMap,
    vars: &'a Vars,
    /// The source image, sRGB `0..1`, row-major `w x h`.
    rgb: &'a [[f32; 3]],
    /// Each face's fill model, indexed by face id.
    face: &'a [FillModel],
    w: usize,
    h: usize,
    /// Chain pieces, each lying in one pixel; rebuilt by `bucket_band`, in pixel order.
    pieces: Vec<Piece>,
    /// Per pixel, the first of its pieces (-1 for none); the rest follow through
    /// `Piece::next`.
    head: Vec<i32>,
    /// Per row, the first of the pieces lying left of the image.
    vhead: Vec<i32>,
    /// The pixels `head` holds a piece in, ascending once `bucket_band` has run; it clears
    /// only these.
    touched: Vec<usize>,
    /// The previous evaluation's piece buffer, reused.
    spare: Vec<Piece>,
    scratch: Scratch,
    /// The source's alpha per pixel and each face's opacity, when alpha is a fourth
    /// channel of the data term.
    alpha: Option<(&'a [f32], &'a [f32])>,
    /// Weight of the kink prior (see `K_KINK`).
    w_kink: f64,
    /// Weight of the anchor prior (see `K_ANCHOR`).
    w_anchor: f64,
    /// The fixed band the data term is summed over (see [`band`]).
    band: Option<band::Band>,
    /// Scratch for the band term.
    bscratch: band::BandScratch,
    /// The starting residual of the band pixels the boundary cuts (what the prior weights
    /// are scaled to), and of the rest.
    band_norm: (f64, f64),
    /// The independent part being solved, when the energy is restricted to one.
    active: Option<lbfgs::Active>,
}

impl Problem<'_> {
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
        let all_edges: Vec<u32>;
        let edges: &[u32] = match &self.active {
            Some(a) => &a.edges,
            None => {
                all_edges = (0..self.map.edges.len() as u32).collect();
                &all_edges
            }
        };
        for &k in edges {
            let (k, e) = (k as usize, &self.map.edges[k as usize]);
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
        let all_vars: Vec<u32>;
        let vs: &[u32] = match &self.active {
            Some(a) => &a.vars,
            None => {
                all_vars = (0..pos.len() as u32).collect();
                &all_vars
            }
        };
        for &v in vs {
            let v = v as usize;
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

    /// The whole objective at `pos` (data + priors), cutting the chains into pieces first.
    /// With `grad`, it is zeroed and then filled with the objective's gradient per unknown
    /// (zero along a coordinate pinned to the frame).
    fn energy(&mut self, pos: &[Point], mut grad: Option<&mut [Point]>) -> f64 {
        self.bucket_band(pos);
        if let Some(g) = grad.as_deref_mut() {
            match &self.active {
                Some(a) => a
                    .vars
                    .iter()
                    .for_each(|&v| g[v as usize] = Point::new(0.0, 0.0)),
                None => g.iter_mut().for_each(|p| *p = Point::new(0.0, 0.0)),
            }
        }
        let band = self.band.take().expect("band mode");
        let mut bs = std::mem::take(&mut self.bscratch);
        let d = self.band_data(&band, pos, &mut bs, grad.as_deref_mut());
        self.bscratch = bs;
        self.band = Some(band);
        let d = d + self.priors(pos, grad.as_deref_mut());
        if let Some(g) = grad {
            let pin = &self.vars.pin;
            let mut clear = |v: usize| {
                if pin[v] & 1 != 0 {
                    g[v].x = 0.0;
                }
                if pin[v] & 2 != 0 {
                    g[v].y = 0.0;
                }
            };
            match &self.active {
                Some(a) => a.vars.iter().for_each(|&v| clear(v as usize)),
                None => (0..pin.len()).for_each(clear),
            }
        }
        d
    }
}

/// What the stage did, for the run log.
pub struct Report {
    /// Objective energy before the solve.
    pub before: f64,
    /// Objective energy after the solve.
    pub after: f64,
    /// Most iterations any independent part of the boundary took.
    pub iters: usize,
    /// Number of vertices moved.
    pub moved: usize,
    /// How much of the solved displacement survived the self-crossing guard: the share
    /// `Σ σ_v·|p_v − p⁰_v| / Σ |p_v − p⁰_v|` it kept (1 when it backed nothing off; see
    /// `fold_guard_local`).
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
/// was, when the input is degenerate or the energy did not fall. On success every edge's
/// points are overwritten (sigmas are not touched; the boundaries the fold guard backed off
/// keep part or none of their displacement) and the report says how much moved.
pub fn optimise_alpha(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    face: &[FillModel],
    budget_ms: Option<u64>,
    alpha: Option<(&[f32], &[f32])>,
) -> Option<Report> {
    optimise_alpha_capped(map, rgb, face, budget_ms, alpha, None)
}

/// [`optimise_alpha`] with the descent stopped after at most `max_iters` iterations per
/// independent part of the boundary (`None`: the solver's own ceiling, which is exactly
/// [`optimise_alpha`]). Everything else -- the band, the stopping tests, the fold guard --
/// is the same, so a cap only ever ends the descent earlier.
///
/// Fast's `balanced` mode runs the solve this way, as an anytime stage stopped at the knee
/// of its measured profile (r2-fastq, 2026-10-02: on the 128 px screen set 2 iterations
/// buy 50 % of the full solve's dE00 gain, 4 buy 81 %, 8 buy 89 %, 16 buy 93 %).
/// Inspired by: S. Zilberstein (1996), "Using anytime algorithms in intelligent systems",
/// AI Magazine 17(3):73, <https://ojs.aaai.org/aimagazine/index.php/aimagazine/article/view/1232>:
/// a contract algorithm given a fixed budget chosen from its performance profile. Here
/// the budget is an iteration count, not a time, so the output is the same on every
/// machine.
pub fn optimise_alpha_capped(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    face: &[FillModel],
    budget_ms: Option<u64>,
    alpha: Option<(&[f32], &[f32])>,
    max_iters: Option<usize>,
) -> Option<Report> {
    let (w, h) = (map.width, map.height);
    if w == 0 || h == 0 || map.edges.is_empty() || rgb.len() < w * h {
        return None;
    }
    let mut vars = build_vars(map);
    band::pin_frame(map, &mut vars);
    let n = vars.start.len();
    if n < 3 {
        return None;
    }
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
            vhead: vec![-1; h],
            touched: Vec::new(),
            spare: Vec::new(),
            scratch: Scratch::default(),
            alpha,
            w_kink: 1.0,
            w_anchor: 0.0,
            band: None,
            bscratch: band::BandScratch::default(),
            band_norm: (0.0, 0.0),
            active: None,
        };
        // A band whose tables would pass `band::table_budget` is not solved at all: the map
        // keeps the measured boundary, as when the solve gains nothing.
        if !band::setup(&mut prob) {
            return None;
        }
        let deadline = budget_ms.map(|ms| (Instant::now(), u128::from(ms)));
        lbfgs::descend(&mut prob, &vars, deadline, max_iters, dbg)?
    };
    let (scaled, scale) = fold_guard_local(map, &vars, &pos, dbg);
    let moved = write_back(map, &vars, &scaled);
    Some(Report {
        moved,
        scale,
        ..report
    })
}

/// Back off only the boundaries that take part in a fold the solve made.
///
/// The solve can fold the boundary: on a ribbon two pixels wide the two sides are both
/// pulled towards the ink between them and can pass through each other. Downstream that
/// costs far more than the boundary error it bought — the repair stage refits the offending
/// rings round after round (2.5 s on one logo) and the emitter paints a face over its own
/// interior. So no *new* crossing may survive the stage.
///
/// But a fold is a property of one pair of segments. The guard this replaces scaled the
/// *whole* displacement back by halves (to 1/16, else it discarded the solve) until the
/// crossing count was back to the start's, giving up the solve's gain on every boundary of
/// the image for one crossing somewhere. Measured on the 128 px screen set and `held_a`
/// (r2-fidelity research, 2026-10-02): it scaled 46 of 362 icons back and discarded one
/// solve outright (`looker`), and 23 of the 64 icons the v0.2.4 solve made worse were
/// scaled back where the solve before it had been kept in full; reverting only the
/// boundaries in a new crossing was worth −2.9 % dE00 on the screen set and −2.3 % on
/// `held_a` by itself. With the Wolfe-search solver (2026-10-03, the 246-icon screen set
/// against v0.2.5, every band pixel kept, 128 iterations): −4.35 % with this guard against
/// −3.88 % with the global one at 512 px, −7.58 % against −7.51 % at 128 px.
///
/// The rule. Each edge `k` carries a scale `s_k`, starting at 1; an unknown `v` sits at
/// `p⁰_v + σ_v·(p_v − p⁰_v)` with `σ_v = min s_k` over the edges it belongs to (so a
/// junction follows the most cautious of its edges, and an edge at scale 0 is exactly where
/// it started, ends included). While some pair of segments crosses that did not cross at
/// the start ([`folds::FoldCounter::new_crossings`]), halve the scale of every edge with a
/// segment in such a pair, from ½ down to 1/16 and then to 0. Every round lowers at least
/// one scale, and a pair whose two edges are both at 0 is the start's own pair, so it cannot
/// be new: the loop ends, after at most six rounds per edge, with no new crossing. Crossings
/// that were there at the start (left by `refine_subpixel` or `refine_junctions` for the
/// repair stage) are allowed to stay: refusing to improve a boundary because of a crossing
/// that was already there would give up most of the gain. Every position tried lies, point
/// by point, on the segment from its start to its solution, so one candidate list
/// ([`folds::FoldCounter::new`]) serves every round.
///
/// Returns the positions and the share of the solved displacement kept,
/// `Σ_v σ_v·|p_v − p⁰_v| / Σ_v |p_v − p⁰_v|` (1 when nothing was backed off).
///
/// Inspired by: J. Smith, S. Schaefer (2015), *Bijective parameterization with free
/// boundaries*, ACM TOG 34(4), <https://doi.org/10.1145/2766947>, and M. Li, Z.
/// Ferguson, T. Schneider, T. Langlois, D. Zorin, D. Panozzo, C. Jiang, D. M. Kaufman
/// (2020), *Incremental potential contact*, ACM TOG 39(4),
/// <https://doi.org/10.1145/3386569.3392425>, which never take a step that makes two
/// boundary elements pass through each other, deciding it per pair of elements (the largest
/// step before the pair collides) rather than for the mesh as a whole. Adapted: they limit
/// the step inside the line search and add a barrier energy that keeps a colliding pair
/// apart, so the descent can continue; this energy has no barrier (two sides of a thin
/// ribbon are *meant* to approach each other), and a step bound without one stops the
/// whole part's descent at the first contact. So the per-pair decision is taken once, after
/// the solve, and backs off only the boundaries the colliding pairs belong to.
fn fold_guard_local(map: &PlanarMap, vars: &Vars, pos: &[Point], dbg: bool) -> (Vec<Point>, f64) {
    let n = vars.start.len();
    let folds = folds::FoldCounter::new(map, vars, &vars.start, pos);
    let seg_edge = folds::segment_edges(map, vars);
    let mut scale = vec![1.0f64; map.edges.len()];
    let mut cur = pos.to_vec();
    let mut rounds = 0usize;
    loop {
        let bad = folds.new_crossings(&vars.start, &cur);
        if bad.is_empty() {
            break;
        }
        rounds += 1;
        // The edges with a segment in a new crossing, each once.
        let mut hit = vec![false; map.edges.len()];
        for &(i, j) in &bad {
            hit[seg_edge[i as usize] as usize] = true;
            hit[seg_edge[j as usize] as usize] = true;
        }
        for (k, s) in scale.iter_mut().enumerate() {
            if hit[k] {
                *s = if *s > 1.0 / 16.0 { *s * 0.5 } else { 0.0 };
            }
        }
        // σ_v = the smallest scale of the edges v belongs to.
        let mut sigma = vec![1.0f64; n];
        for (k, ids) in vars.var.iter().enumerate() {
            for &v in ids {
                sigma[v as usize] = sigma[v as usize].min(scale[k]);
            }
        }
        for v in 0..n {
            let (s0, p) = (vars.start[v], pos[v]);
            cur[v] = Point::new(
                s0.x + (p.x - s0.x) * sigma[v],
                s0.y + (p.y - s0.y) * sigma[v],
            );
        }
    }
    let (mut kept, mut total) = (0.0f64, 0.0f64);
    for v in 0..n {
        total += vars.start[v].dist(pos[v]);
        kept += vars.start[v].dist(cur[v]);
    }
    let share = if total > 0.0 { kept / total } else { 1.0 };
    if dbg {
        let backed = scale.iter().filter(|&&s| s < 1.0).count();
        eprintln!(
            "  [bopt] local fold guard: {rounds} round(s), {backed} of {} edge(s) backed off, {:.1}% of the displacement kept",
            map.edges.len(),
            share * 100.0
        );
    }
    (cur, share)
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

#[cfg(test)]
mod cap_tests;
