//! Order-first decoding of faces the free-form solve cannot resolve.
//!
//! # Why this stage exists
//!
//! Every other geometry stage refines a boundary that already has as many degrees of
//! freedom as it has points, and decides how many curve segments to spend only at the very
//! end, in the fitter. That order is backwards for a face too thin to own a fully covered
//! pixel. Such a face has no pixel that reads its colour directly, so the palette's
//! estimate of that colour is biased toward the ground by one minus its best coverage;
//! the boundary is then fitted against the wrong colour, and nothing revisits either.
//!
//! Measured on a 2 px diagonal stroke: baseline emits fifteen
//! cubic arcs, a core colour of `#41557d` where the truth is `#204080`, and keeps 91 % of
//! the ink mass, violating the first-order condition that a least-squares fill must leave
//! the coverage-weighted residual orthogonal to its own coverage column. Fixing the model
//! order first -- one quadrilateral, two flat fills -- eliminating the fills by least
//! squares and running damped Gauss-Newton on the eight vertex coordinates reaches the
//! truth to 0.02 px from starts at a quarter or twice the true width.
//!
//! # What it does
//!
//! For each face that owns no fully covered pixel, or whose ink mass leaks by more than
//! `LEAK_GATE`:
//!
//!   1. Assemble its ring and ask the fitter, at a deliberately coarse price per segment,
//!      how many vertices it wants. That count is the model order, and it is chosen from
//!      the data before any geometry moves.
//!   2. Freeze every junction node. Only points interior to an edge become free, and an
//!      interior point belongs to exactly one edge, so no neighbouring face can see a
//!      boundary move that its own copy did not make. The partition survives by
//!      construction rather than by repair.
//!   3. Solve: Levenberg-damped Gauss-Newton on the free vertices, with the face's fill
//!      and each neighbour's fill eliminated by least squares at every trial step.
//!   4. Accept only if the objective falls, measured by re-rendering the touched pixels,
//!      and only if the polygon stayed simple.
//!
//! # What it does not do
//!
//! Curved faces are skipped: a face whose ring does not concentrate its turning at the
//! vertices the fitter picked is left alone. Faces with no interior points on some edge
//! are skipped, since there is nothing free to move. Both are detected, not guessed.
//!
//! Off by default. Set `INKVEC_DECODE=1`.

use inkvec_core::clock::Instant;
use std::collections::HashMap;

use inkvec_core::{Point, Polyline};
use inkvec_fit::{optimal_polygon, FitConfig};

/// A run of one edge inside a face's ring: `(edge, reversed, first ring index, count)`.
pub(crate) type EdgeSpan = (usize, bool, usize, usize);

/// One candidate segmentation of a face: which ring points became vertices, and where
/// those vertices sit.
type CandidateOrder = (Vec<usize>, Vec<Point>);

use crate::gradient::{FillFit, FillModel, PARAMS_FLAT};
use crate::planar::{face_edge_order, PlanarMap};

/// A face leaking more ink than this is not at a least-squares optimum for its geometry.
const LEAK_GATE: f64 = 0.05;
/// Reject a decode whose ring wants fewer or more vertices than this.
const MIN_VERTS: usize = 3;
const MAX_VERTS: usize = 16;
/// A ring may stray this far from its polygon before the face is something else entirely.
const MAX_DEV: f64 = 2.0;
/// Spread of the offsets above which a SMOOTH departure from the chord is a real curve.
const CURVE_BIAS_PX: f64 = 0.35;
/// A decode must cut the residual to this fraction of what it was, or not be made.
///
/// Measured. At 1.0 (any improvement at all) the stage fires on marginal cases and the
/// screen set comes out 10 icons worse against 7 better; at 0.5 it fires only where it has
/// something to say, and the same set comes out better on every axis. Override with
/// `INKVEC_DECODE_GAIN`.
const MIN_GAIN: f64 = 0.5;
/// Residual ratio below which a decode may skip the post-solve shape check.
///
/// Zero, i.e. off, and that is a measured trade rather than a preference.
///
/// With it at 0.5 the synthetic 2 px diagonal stroke decodes: its core comes back #1f3f80
/// against a truth of #204080, where master emits #41557d, and the file shrinks. It also
/// costs corpus quality, because it lets a decode move a boundary away from the ring it
/// was extracted from on evidence that is sometimes local noise. Off is the setting that
/// leaves the screen set better on every axis. Set `INKVEC_DECODE_OVERRIDE=0.5` to trade
/// the other way.
const EVIDENCE_OVERRIDE: f64 = 0.0;
/// and how many samples a segment needs before that judgement is made at all.
const CURVE_MIN_SAMPLES: usize = 8;
/// A face this thin, measured as twice its area over its perimeter, is a stroke: too
/// narrow for any pixel to have read its colour cleanly, whatever the label map says.
/// Measured: 4.0 lets in faces wide enough to own a clean witness pixel and cost more
/// than it gains; 2.5 is where the conditioning cliff sits and where the
/// corpus probe turns from losing to winning.
pub(crate) const THIN_PX: f64 = 2.5;
/// A four-sided ribbon costs a move plus four line segments.
pub(crate) const PARAMS_PER_RIBBON: f64 = 10.0;
/// Widest ribbon worth pooling. Above this a single stroke is already identifiable and
/// sharing a width can only move it off an answer it already had.
pub(crate) const SHARE_MAX_PX: f64 = 1.75;
/// Guards against decoding an entire background.
const MAX_RING_POINTS: usize = 512;
/// How many partially covered band pixels each free coordinate needs before the decode is
/// asked to determine it.
///
/// Only pixels the boundary actually cuts carry information about where it is: interior
/// and exterior pixels are flat in every direction (the Hadamard structure theorem, and
/// the reason `boundary_opt` sums over boundary pixels alone). Below this ratio the
/// problem is under-determined and the solver is fitting noise -- which is what it was
/// doing on faces of one to five square pixels, turning specks into triangles for a
/// thousandth of an objective.
const PIXELS_PER_UNKNOWN: usize = 4;
pub(crate) const MAX_BBOX_PIXELS: usize = 20_000;
/// The stage's slice of wall-clock time when the caller set a time budget. Without one the
/// stage has no clock at all; see [`decode_faces`].
pub(crate) const BUDGETED_MS: f64 = 600.0;
/// Levenberg schedule.
const GN_ITERS: usize = 14;
const FD_STEP: f64 = 0.01;
const MAX_STEP: f64 = 0.35;
/// Total displacement a vertex may accumulate, from where the label map put it.
///
/// `boundary_opt` caps its own points the same way and for the same reason: topology was
/// decided on the integer lattice, and a point that walks far from its lattice evidence is
/// no longer describing the feature it was extracted from. Without this cap the solver
/// drifted vertices up to five pixels, turning the rounded frame of one emoji into a
/// thirteen-sided polygon whose local residual had improved and whose rendered colour
/// error had quadrupled.
const MAX_TOTAL: f64 = 1.0;

/// What the decode stage did, for the run log.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Faces looked at.
    pub considered: usize,
    /// Faces that passed the pre-filters and had a decode attempted.
    pub attempted: usize,
    /// Faces successfully decoded.
    pub decoded: usize,
    /// Faces whose decode attempt was rejected.
    pub rejected: usize,
    /// Straight ribbons pooled under one shared width, and the width they agreed on.
    pub pooled: usize,
    /// The width straight ribbons were pooled to, in pixels.
    pub shared_width: f64,
    /// Change in the objective from this stage.
    pub d_objective: f64,
    /// Time spent in this stage, in milliseconds.
    pub ms: f64,
}

pub(crate) mod ribbon;

pub(crate) use crate::clip::{coverage, shoelace, simple, Bbox};
pub(crate) use ribbon::share_widths;

// ---------------------------------------------------------------------------------------
// The per-face problem
// ---------------------------------------------------------------------------------------

/// One point of a face's ring, and where it came from.
#[derive(Clone, Copy)]
pub(crate) struct RingPt {
    pub(crate) p: Point,
    pub(crate) edge: usize,
    pub(crate) junction: bool,
}

/// Assemble a face's single ring, plus the span of ring indices each edge contributed.
pub(crate) fn ring_of(
    map: &PlanarMap,
    rings: &[Vec<(usize, bool)>],
) -> Option<(Vec<RingPt>, Vec<EdgeSpan>)> {
    if rings.len() != 1 {
        return None;
    }
    let mut out: Vec<RingPt> = Vec::new();
    let mut spans: Vec<EdgeSpan> = Vec::new();
    let single_closed = rings[0].len() == 1 && map.edges[rings[0][0].0].closed;
    for &(k, rev) in &rings[0] {
        let e = &map.edges[k];
        let n = e.points.len();
        if n < 2 {
            return None;
        }
        let start = out.len();
        // A closed edge owns all of its points; an open one hands its last point to the
        // next edge, which is the junction they share.
        let take = if single_closed { n } else { n - 1 };
        for s in 0..take {
            let idx = if rev { n - 1 - s } else { s };
            out.push(RingPt {
                p: e.points[idx],
                edge: k,
                junction: !single_closed && s == 0,
            });
        }
        spans.push((k, rev, start, take));
    }
    if out.len() < 4 || out.len() > MAX_RING_POINTS {
        return None;
    }
    Some((out, spans))
}

/// One edge as a polyline, traversed the way this face sees it, sigma and all.
fn edge_polyline(e: &crate::planar::Edge, rev: bool) -> Polyline {
    if rev {
        Polyline::new(
            e.points.iter().rev().copied().collect(),
            e.sigma.iter().rev().copied().collect(),
            e.closed,
        )
    } else {
        Polyline::new(e.points.clone(), e.sigma.clone(), e.closed)
    }
}

/// Corners of a ring, found by turning rather than by line fitting.
///
/// The segment-fitting DP is the wrong instrument here. Asked for four vertices on a thin
/// ribbon it puts them where four straight pieces fit best, which on a shape whose two long
/// sides are two pixels apart is not the four corners: one of its "segments" then cuts
/// across the ribbon and sits 1.3 px off the boundary all the way along. Turning has no
/// such failure mode. A corner is where the ring changes direction, measured over a short
/// window so a staircase of single-pixel steps does not register as one.
fn turning_corners(ring: &[Point], k: usize) -> Vec<(usize, f64)> {
    let n = ring.len();
    if n < 4 * k + 4 {
        return Vec::new();
    }
    let mut turn = vec![0.0f64; n];
    for i in 0..n {
        let a = ring[(i + n - k) % n];
        let b = ring[i];
        let c = ring[(i + k) % n];
        let (ux, uy) = (b.x - a.x, b.y - a.y);
        let (vx, vy) = (c.x - b.x, c.y - b.y);
        let (nu, nv) = ((ux * ux + uy * uy).sqrt(), (vx * vx + vy * vy).sqrt());
        if nu < 1e-9 || nv < 1e-9 {
            continue;
        }
        let cross = (ux * vy - uy * vx) / (nu * nv);
        let dot = (ux * vx + uy * vy) / (nu * nv);
        turn[i] = cross.abs().atan2(dot).abs();
    }
    // Non-maximum suppression over a window, so one corner yields one vertex.
    let win = k.max(1);
    let mut peaks: Vec<(usize, f64)> = Vec::new();
    for i in 0..n {
        if turn[i] < 0.4 {
            continue;
        }
        let mut best = true;
        for d in 1..=win {
            if turn[(i + d) % n] > turn[i] || turn[(i + n - d) % n] > turn[i] {
                best = false;
                break;
            }
        }
        if best {
            peaks.push((i, turn[i]));
        }
    }
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    peaks
}

/// Candidate model orders, and what the shipped fitter would spend on this ring.
///
/// The order is not guessed from one fit. A line-segmentation DP run on a long thin
/// ribbon happily describes the whole ring as two long lines -- geometrically true, and
/// useless, because the polygon those two lines bound has no area. So propose several
/// orders, from a ladder of prices per segment, and let the objective choose between them
/// further down. That is model selection by exact delta J, which is the only rule this
/// stage trusts.
///
/// Each edge is fitted separately, so every junction node is a vertex by construction
/// rather than by luck: those points are shared with faces this stage is not solving.
pub(crate) fn candidate_orders(
    map: &PlanarMap,
    ring: &[RingPt],
    spans: &[EdgeSpan],
    cfg: &FitConfig,
) -> (Vec<CandidateOrder>, f64) {
    let mut params_before = 0.0f64;
    for &(k, rev, _, _) in spans {
        let e = &map.edges[k];
        // The edge's OWN sigma, not a made-up constant. The fitter prices a segment
        // against how well the points are known, so inventing sigma = 0.5 made the
        // shipped fit look four times cheaper than it is and the comparison below
        // meaningless -- it rejected a decode that cut eighteen parameters to ten.
        let poly = edge_polyline(e, rev);
        params_before += inkvec_fit::multimodel::optimal_multimodel(&poly, cfg).params();
    }

    let mut out: Vec<(Vec<usize>, Vec<Point>)> = Vec::new();
    let mut seen: Vec<Vec<usize>> = Vec::new();
    for mult in [4.0f64, 1.0, 0.25, 0.0625, 0.015] {
        let price = FitConfig {
            lambda: cfg.lambda * mult,
            ..*cfg
        };
        let mut cuts: Vec<usize> = Vec::new();
        for &(k, rev, start, take) in spans {
            let e = &map.edges[k];
            let poly = edge_polyline(e, rev);
            for &v in &optimal_polygon(&poly, &price).vertices {
                if v < take {
                    cuts.push(start + v);
                }
            }
            if !e.closed && !cuts.contains(&start) {
                cuts.push(start);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        if cuts.len() < MIN_VERTS || cuts.len() > MAX_VERTS || seen.contains(&cuts) {
            continue;
        }
        if ring
            .iter()
            .enumerate()
            .any(|(i, r)| r.junction && !cuts.contains(&i))
        {
            continue;
        }
        seen.push(cuts.clone());
        let verts: Vec<Point> = cuts.iter().map(|&i| ring[i].p).collect();
        out.push((cuts, verts));
    }

    // Turning-based proposals, alongside the price-ladder ones.
    let ring_pts: Vec<Point> = ring.iter().map(|r| r.p).collect();
    let mut peaks = turning_corners(&ring_pts, 2);
    if peaks.len() < 4 {
        peaks = turning_corners(&ring_pts, 1);
    }
    let junctions: Vec<usize> = ring
        .iter()
        .enumerate()
        .filter(|(_, r)| r.junction)
        .map(|(i, _)| i)
        .collect();
    for m in [3usize, 4, 5, 6, 8, 10] {
        let mut cuts: Vec<usize> = junctions.clone();
        for &(i, _) in peaks.iter() {
            if cuts.len() >= m {
                break;
            }
            if !cuts.contains(&i) {
                cuts.push(i);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        if cuts.len() < MIN_VERTS || cuts.len() > MAX_VERTS || seen.contains(&cuts) {
            continue;
        }
        seen.push(cuts.clone());
        let verts: Vec<Point> = cuts.iter().map(|&i| ring[i].p).collect();
        out.push((cuts, verts));
    }
    (out, params_before)
}

/// Least squares for `k` fills against the coverage columns, one solve for three channels.
///
/// `cols[c][p]` is the weight of fill `c` in band pixel `p`. Returns the fills and the
/// residual sum of squares. A column the data cannot separate is left at its prior.
pub(crate) fn varpro(
    cols: &[Vec<f64>],
    target: &[[f32; 3]],
    prior: &[[f32; 3]],
) -> (Vec<[f32; 3]>, f64) {
    let k = cols.len();
    let n = target.len();
    let mut ata = vec![0.0f64; k * k];
    let mut atb = vec![[0.0f64; 3]; k];
    for i in 0..k {
        for j in i..k {
            let mut s = 0.0;
            for p in 0..n {
                s += cols[i][p] * cols[j][p];
            }
            ata[i * k + j] = s;
            ata[j * k + i] = s;
        }
        for p in 0..n {
            let c = cols[i][p];
            if c != 0.0 {
                for ch in 0..3 {
                    atb[i][ch] += c * target[p][ch] as f64;
                }
            }
        }
    }
    // Ridge against the prior: a column with no support keeps the colour it had, and the
    // solve cannot answer a question the pixels did not ask.
    let trace: f64 = (0..k).map(|i| ata[i * k + i]).sum::<f64>().max(1e-9);
    let ridge = 1e-6 * trace / k as f64;
    for i in 0..k {
        ata[i * k + i] += ridge;
        for ch in 0..3 {
            atb[i][ch] += ridge * prior[i][ch] as f64;
        }
    }
    let sol = solve_sym(&mut ata, &atb, k);
    let mut fills: Vec<[f32; 3]> = Vec::with_capacity(k);
    for i in 0..k {
        let mut c = [0.0f32; 3];
        for ch in 0..3 {
            c[ch] = sol[i][ch].clamp(0.0, 1.0) as f32;
        }
        fills.push(c);
    }
    let mut sse = 0.0;
    for p in 0..n {
        for ch in 0..3 {
            let mut m = 0.0;
            for i in 0..k {
                m += cols[i][p] * fills[i][ch] as f64;
            }
            let d = m - target[p][ch] as f64;
            sse += d * d;
        }
    }
    (fills, sse)
}

/// Gauss-Jordan on a small symmetric system with three right-hand sides.
fn solve_sym(a: &mut [f64], b: &[[f64; 3]], k: usize) -> Vec<[f64; 3]> {
    let mut x: Vec<[f64; 3]> = b.to_vec();
    for c in 0..k {
        let mut piv = c;
        for r in c + 1..k {
            if a[r * k + c].abs() > a[piv * k + c].abs() {
                piv = r;
            }
        }
        if a[piv * k + c].abs() < 1e-12 {
            continue;
        }
        if piv != c {
            for j in 0..k {
                a.swap(c * k + j, piv * k + j);
            }
            x.swap(c, piv);
        }
        let d = a[c * k + c];
        for j in 0..k {
            a[c * k + j] /= d;
        }
        for ch in 0..3 {
            x[c][ch] /= d;
        }
        for r in 0..k {
            if r == c {
                continue;
            }
            let f = a[r * k + c];
            if f == 0.0 {
                continue;
            }
            for j in 0..k {
                a[r * k + j] -= f * a[c * k + j];
            }
            for ch in 0..3 {
                x[r][ch] -= f * x[c][ch];
            }
        }
    }
    x
}

/// Everything fixed about one face's decode problem.
pub(crate) struct Problem {
    pub(crate) bb: Bbox,
    pub(crate) target: Vec<[f32; 3]>,
    /// For each band pixel, the neighbour column it belongs to, or `usize::MAX`.
    pub(crate) owner: Vec<usize>,
    /// Prior colours: index 0 is the face itself, then one per neighbour.
    pub(crate) prior: Vec<[f32; 3]>,
    /// Coverage, and painted colour, of every OTHER face that reaches into this band.
    ///
    /// Without these the fit is biased. A stroke the palette shattered into three faces
    /// has its two ends painted by faces this one shares no edge with; a model that
    /// pretends they are absent explains their ink by shrinking the middle face, which is
    /// exactly what happened: a 2 px stroke came back 1.71 px wide. They are held fixed --
    /// this stage solves one face at a time -- but they are not ignored.
    pub(crate) rest: Vec<f64>,
    pub(crate) rest_rgb: Vec<[f32; 3]>,
}

impl Problem {
    /// Residual of a candidate polygon under FIXED fills: what the pipeline would emit.
    fn eval_fixed(&self, cov: &[f64], fills: &[[f32; 3]]) -> f64 {
        let mut sse = 0.0;
        for p in 0..self.target.len() {
            let a = cov[p].clamp(0.0, 1.0);
            let o = self.owner[p];
            let other = if o == usize::MAX {
                [0.0f32; 3]
            } else {
                fills[o + 1]
            };
            let wo = if o == usize::MAX {
                0.0
            } else {
                (1.0 - a - self.rest[p]).max(0.0)
            };
            for ch in 0..3 {
                let m =
                    a * fills[0][ch] as f64 + wo * other[ch] as f64 + self.rest_rgb[p][ch] as f64;
                let d = m - self.target[p][ch] as f64;
                sse += d * d;
            }
        }
        sse
    }

    /// Residual and fills for a candidate polygon.
    pub(crate) fn eval(
        &self,
        poly: &[Point],
        cov: &mut [f64],
        mark: &mut [bool],
    ) -> (f64, Vec<[f32; 3]>) {
        coverage(poly, self.bb, cov, mark);
        let n = self.target.len();
        let k = self.prior.len();
        let mut cols: Vec<Vec<f64>> = vec![vec![0.0; n]; k];
        for p in 0..n {
            let a = cov[p].clamp(0.0, 1.0);
            cols[0][p] = a;
            let o = self.owner[p];
            if o != usize::MAX {
                cols[o + 1][p] = (1.0 - a - self.rest[p]).max(0.0);
            }
        }
        let mut tgt = self.target.clone();
        for p in 0..n {
            for ch in 0..3 {
                tgt[p][ch] -= self.rest_rgb[p][ch];
            }
        }
        let (fills, sse) = varpro(&cols, &tgt, &self.prior);
        (sse, fills)
    }
}

// ---------------------------------------------------------------------------------------
// The stage
// ---------------------------------------------------------------------------------------

/// Run the order-first decode over every face with no fully covered pixel, or whose ink
/// mass leaks past `LEAK_GATE`. See the module docs. Returns `None` when nothing changed.
pub fn decode_faces(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    labels: &[u16],
    face_fill: &mut [FillFit],
    lambda: f64,
    budget_ms: Option<f64>,
) -> Option<Report> {
    let t0 = Instant::now();
    // A wall clock makes the answer depend on the machine: the same image traced in
    // WebAssembly, on a slower CPU or under load stops sooner and writes different bytes.
    // So the clock runs only under a caller's time budget (see `BUDGETED_MS`); otherwise
    // what bounds the stage is the candidate list itself, faces of at most
    // `MAX_BBOX_PIXELS` with a handful of orders each. `INKVEC_DECODE_MS` still forces one.
    let budget = inkvec_core::env::number("INKVEC_DECODE_MS").or(budget_ms);
    let out_of_time = |t0: &Instant| budget.is_some_and(|b| t0.elapsed().as_secs_f64() * 1e3 > b);
    let leak_gate = inkvec_core::env::number("INKVEC_DECODE_LEAK").unwrap_or(LEAK_GATE);
    let dbg = inkvec_core::env::flag("INKVEC_DECODEDBG");
    let (w, h) = (map.width, map.height);
    let n_faces = face_fill.len();
    if n_faces == 0 || w == 0 || h == 0 {
        return None;
    }

    // Area only, as a prefilter. Whether a face owns a fully covered PIXEL is a question
    // about coverage, not about labels: a 2 px diagonal stroke has interior labels all
    // along its length and yet its best pixel is only ~85 % covered, so its colour was
    // never read off the image. That test is made below, on the geometry.
    let mut area = vec![0usize; n_faces];
    for &l in labels.iter() {
        let f = l as usize;
        if f < n_faces {
            area[f] += 1;
        }
    }

    let order = face_edge_order(map);
    let mut rep = Report::default();
    let mut cand: Vec<usize> = (0..n_faces.min(order.len()))
        .filter(|&f| area[f] > 0 && area[f] <= MAX_BBOX_PIXELS)
        .collect();
    // Worst first: the budget should buy the faces that are most wrong.
    cand.sort_by_key(|&f| area[f]);
    rep.considered = cand.len();
    if dbg {
        eprintln!("  [decode] {} candidate faces of {n_faces}", cand.len());
    }

    let mut cov: Vec<f64> = Vec::new();
    let mut cov2: Vec<f64> = Vec::new();
    let mut mark: Vec<bool> = Vec::new();

    for f in cand {
        if out_of_time(&t0) {
            break;
        }
        let Some((ring, spans)) = ring_of(map, &order[f]) else {
            if dbg {
                eprintln!("  [decode] face {f}: no single ring");
            }
            continue;
        };
        let pts: Vec<Point> = ring.iter().map(|r| r.p).collect();
        let Some(bb_old) = Bbox::of(&pts, w, h, 1.0) else {
            continue;
        };
        let need = bb_old.len();
        if cov.len() < need {
            cov.resize(need, 0.0);
            cov2.resize(need, 0.0);
            mark.resize(need, false);
        }
        if bb_old.len() > MAX_BBOX_PIXELS {
            if dbg {
                eprintln!("  [decode] face {f}: bbox {} too big", bb_old.len());
            }
            continue;
        }

        let extent = (w.max(h)) as f64;
        let cfg = FitConfig::from_precision(extent, 0.1, 2.0);
        let (orders, params_before) = candidate_orders(map, &ring, &spans, &cfg);
        if dbg {
            eprintln!(
                "  [decode] face {f}: {} orders {:?}, params_before {params_before:.1}",
                orders.len(),
                orders.iter().map(|(c, _)| c.len()).collect::<Vec<_>>()
            );
        }
        if orders.is_empty() {
            if dbg {
                eprintln!("  [decode] face {f}: no usable model order");
            }
            continue;
        }

        let Some(prob) = build_problem(map, &order, rgb, labels, face_fill, f, bb_old, w, h) else {
            if dbg {
                eprintln!("  [decode] face {f}: no usable neighbour set");
            }
            continue;
        };

        // Which faces is this stage for? The ones whose colour the image never showed
        // cleanly. The label map cannot answer that: it hard-assigns whole pixels, so a
        // 2 px stroke has interior labels all along its length while its widest pixel is
        // only ~85 % covered and every sample of its colour is a blend. Two tests:
        //   thin: twice the area over the perimeter is the width of a long face, and
        //     below a few pixels no pixel can be a clean witness.
        //   leak: does refitting the fills alone, geometry untouched, lower the residual?
        //     At a least-squares optimum it cannot. If it does, the face is not at one.
        // Neither is a threshold on the answer -- only on whether to ask the question.
        // What decides is delta J, computed exactly, below.
        let perim: f64 = (0..ring.len())
            .map(|i| ring[i].p.dist(ring[(i + 1) % ring.len()].p))
            .sum();
        let thin = if perim > 1e-9 {
            2.0 * area[f] as f64 / perim
        } else {
            0.0
        };
        let thin_px = inkvec_core::env::number("INKVEC_DECODE_THIN").unwrap_or(THIN_PX);
        coverage(&pts, bb_old, &mut cov, &mut mark);
        let prior = prob.prior.clone();
        let sse_fixed = prob.eval_fixed(&cov[..bb_old.len()], &prior);
        let (sse0, _) = prob.eval(&pts, &mut cov, &mut mark);
        let leak = if sse_fixed > 1e-12 {
            (sse_fixed - sse0) / sse_fixed
        } else {
            0.0
        };
        // Narrowness alone decides whether to ask. Leak was tried as a second trigger and
        // was withdrawn: it fires on ordinary faces whose fills are merely a little off,
        // and decoding one of those as a polygon damaged three case-suite shapes that a
        // width test leaves alone (corner_right, edge_axis, junction_quad). Leak is kept
        // as a diagnostic because it is the exact optimality condition, but the disease
        // this stage treats is a face too narrow to have shown its colour.
        if thin > thin_px {
            if dbg {
                eprintln!("  [decode] face {f}: skip (width {thin:.2} px, leak {leak:.3})");
            }
            continue;
        }
        rep.attempted += 1;
        let j0 = sse0 + lambda * params_before;

        // ---- decode, once per candidate order, and keep the best ------------------------
        let mut winner: Option<(Vec<Point>, Vec<usize>, f64)> = None;
        for (cuts, verts) in orders {
            if out_of_time(&t0) {
                break;
            }
            if !is_polygonal(&pts, &verts, &cuts) {
                if dbg {
                    eprintln!("      face {f}: order {} not polygonal", verts.len());
                }
                continue;
            }
            let free: Vec<usize> = (0..verts.len())
                .filter(|&j| !ring[cuts[j]].junction)
                .collect();
            if free.is_empty() {
                continue;
            }
            // Enough evidence for the unknowns? Count the pixels the boundary cuts.
            let cut_px = cov[..bb_old.len()]
                .iter()
                .filter(|&&a| a > 0.01 && a < 0.99)
                .count();
            if cut_px < PIXELS_PER_UNKNOWN * 2 * free.len() {
                if dbg {
                    eprintln!(
                        "      face {f}: order {} under-determined ({cut_px} cut pixels for {} unknowns)",
                        verts.len(), 2 * free.len()
                    );
                }
                continue;
            }
            let (best, sse1) = gauss_newton(&prob, &verts, &free, &mut cov, &mut cov2, &mut mark);
            // Simplicity is checked on the solved polygon; polygonality is not, and must
            // not be. The solver moves vertices away from the extracted ring on purpose --
            // that ring is the thing being corrected -- so measuring the answer against it
            // rejects every successful decode. Drift is bounded by MAX_TOTAL instead.
            // The post-solve shape check is a prior against drift, measured against the
            // ring the solver was correcting, so it must not be absolute: on a sawtoothed
            // ribbon the corrected boundary is *supposed* to sit a pixel off the ring it
            // came from. Strong evidence overrides the prior. A decode that cuts the
            // residual in half has earned the move; one that shaves a few per cent has not.
            let earned = sse1
                < inkvec_core::env::number("INKVEC_DECODE_OVERRIDE").unwrap_or(EVIDENCE_OVERRIDE)
                    * sse0;
            let recheck =
                inkvec_core::env::number("INKVEC_DECODE_RECHECK").unwrap_or(1.0) > 0.5 && !earned;
            if !simple(&best) || (recheck && !is_polygonal(&pts, &best, &cuts)) {
                if dbg {
                    eprintln!(
                        "      face {f}: order {} solved shape rejected",
                        verts.len()
                    );
                }
                continue;
            }
            // What the fitter will really spend on this polygon, not what a line count
            // says it should.
            let Some(params_after) = fitted_params(map, &ring, &spans, &cuts, &best, &cfg) else {
                continue;
            };
            let j1 = sse1 + lambda * params_after;
            // Two axes, not one. A falling J alone lets the stage buy a cheaper
            // description with a worse picture, and that is what it did: it polygonised
            // curved thin faces, spending FEWER parameters and quadrupling the colour
            // error, because the objective is happy to trade them. Require both -- a
            // strictly better fit and no more parameters -- so a decode can only be a
            // Pareto improvement over what the pipeline already had.
            let gain = inkvec_core::env::number("INKVEC_DECODE_GAIN").unwrap_or(MIN_GAIN);
            if sse1 >= gain * sse0 || params_after > params_before {
                if dbg {
                    eprintln!(
                        "      face {f}: order {} rejected (sse {sse0:.3}->{sse1:.3},                          params {params_before:.0}->{params_after:.0})",
                        verts.len()
                    );
                }
                continue;
            }
            if dbg {
                eprintln!(
                    "      face {f}: order {} -> sse {sse0:.4} to {sse1:.4}, J {j0:.4} to {j1:.4}",
                    verts.len()
                );
            }
            if j1 < j0 && winner.as_ref().is_none_or(|(_, _, jb)| j1 < *jb) {
                winner = Some((best, cuts, j1));
            }
        }

        let Some((best, cuts, j1)) = winner else {
            rep.rejected += 1;
            if dbg {
                eprintln!("  [decode] face {f}: no order beat J {j0:.4}");
            }
            continue;
        };

        // ---- accept: write back, junctions untouched --------------------------------------
        let (_, fills) = prob.eval(&best, &mut cov, &mut mark);
        write_back(map, &ring, &spans, &cuts, &best);
        if dbg {
            let ek = ring[0].edge;
            eprintln!(
                "      after write_back: edge {ek} now has {} pts",
                map.edges[ek].points.len()
            );
        }
        if !inkvec_core::env::flag("INKVEC_DECODE_KEEPFILL") {
            if let Some(c) = fills.first() {
                face_fill[f].model = FillModel::Flat(*c);
                face_fill[f].params = PARAMS_FLAT;
            }
        }
        rep.decoded += 1;
        rep.d_objective += j1 - j0;
        if dbg {
            eprintln!(
                "  [decode] face {f} area {} verts {} accept J {j0:.4} -> {j1:.4} poly {:?} shoelace {:.3} fill {:?}",
                area[f],
                best.len(),
                best.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(),
                shoelace(&best),
                fills.first()
            );
        }
    }

    // Off by default. See `share_widths`: the prior is sound and the pipeline does not
    // need it, because the label map already supplies what the raster alone leaves out.
    if inkvec_core::env::flag("INKVEC_DECODE_SHARE") {
        share_widths(
            map, rgb, labels, face_fill, lambda, &mut rep, dbg, leak_gate,
        );
    }

    rep.ms = t0.elapsed().as_secs_f64() * 1e3;
    if rep.attempted == 0 {
        None
    } else {
        Some(rep)
    }
}

/// Is this face a polygon that the pipeline drew badly, or a curve that it drew well?
///
/// Not a test on how far the ring strays from the polygon. The rings this stage exists to
/// repair are exactly the ones that stray: a thin stroke comes back with a sawtooth, and
/// requiring the input to be clean before cleaning it rejects every case of the disease.
///
/// What separates the two is the SIGN of the straying. A curve leaves its chord on one
/// side all the way along, so the signed offsets have a large mean. A sawtooth crosses
/// back and forth, so they have a mean near zero and a large spread. Test the mean against
/// the spread, not the spread against a constant.
pub(crate) fn is_polygonal(ring: &[Point], verts: &[Point], cuts: &[usize]) -> bool {
    // A shallow vertex is not evidence of a curve -- it only costs two parameters, and
    // the parameter test downstream already prices that. The turning threshold that used
    // to live here rejected every proposal on the very ribbon this stage exists for.
    let m = verts.len();
    if m < MIN_VERTS {
        return false;
    }
    let n = ring.len();
    for j in 0..m {
        let (i0, i1) = (cuts[j], cuts[(j + 1) % m]);
        let (a, b) = (verts[j], verts[(j + 1) % m]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = dx * dx + dy * dy;
        if len2 < 1e-12 {
            continue;
        }
        let inv = 1.0 / len2.sqrt();
        let (nx, ny) = (-dy * inv, dx * inv);
        let (mut sum, mut sq, mut cnt, mut worst) = (0.0f64, 0.0f64, 0usize, 0.0f64);
        let mut i = i0;
        loop {
            i = (i + 1) % n;
            if i == i1 {
                break;
            }
            let p = ring[i];
            let off = (p.x - a.x) * nx + (p.y - a.y) * ny;
            sum += off;
            sq += off * off;
            worst = worst.max(off.abs());
            cnt += 1;
            if cnt > n {
                return false;
            }
        }
        // A curve worth protecting spans many samples. Six points at the end cap of a
        // ribbon are a rounded pixel staircase, and calling that a curve rejected every
        // proposal on the shape this stage exists for.
        if cnt < CURVE_MIN_SAMPLES {
            continue;
        }
        let mean = sum / cnt as f64;
        let rms = (sq / cnt as f64).sqrt();
        // Systematically to one side, by an amount that matters: a curve, leave it alone.
        // A sawtooth has mean near zero and spread; a curve has both.
        if mean.abs() > CURVE_BIAS_PX && mean.abs() > 0.5 * rms {
            if inkvec_core::env::flag("INKVEC_DECODEDBG") {
                eprintln!("        polyfail: seg {j} curved, mean {mean:.3} rms {rms:.3} n {cnt}");
            }
            return false;
        }
        // And nothing absurd, whatever the sign pattern.
        if worst > MAX_DEV {
            if inkvec_core::env::flag("INKVEC_DECODEDBG") {
                eprintln!("        polyfail: seg {j} worst {worst:.3} > {MAX_DEV}");
            }
            return false;
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_problem(
    map: &PlanarMap,
    order: &[Vec<Vec<(usize, bool)>>],
    rgb: &[[f32; 3]],
    labels: &[u16],
    face_fill: &[FillFit],
    f: usize,
    bb: Bbox,
    w: usize,
    _h: usize,
) -> Option<Problem> {
    // The neighbours across this face's own edges: each band pixel is unmixed against the
    // face on the other side of the nearest boundary, which the map already knows exactly.
    let mut neigh: Vec<usize> = Vec::new();
    for e in map.edges.iter() {
        let (l, r) = (e.left as usize, e.right as usize);
        if l == f && r < face_fill.len() && !neigh.contains(&r) {
            neigh.push(r);
        }
        if r == f && l < face_fill.len() && !neigh.contains(&l) {
            neigh.push(l);
        }
    }
    if neigh.is_empty() || neigh.len() > 4 {
        return None;
    }
    let mut slot: HashMap<usize, usize> = HashMap::new();
    for (i, &g) in neigh.iter().enumerate() {
        slot.insert(g, i);
    }
    let n = bb.len();
    let bw = bb.x1 - bb.x0;
    let mut target = Vec::with_capacity(n);
    let mut owner = vec![usize::MAX; n];
    let mut rest = vec![0.0f64; n];
    for j in 0..bb.y1 - bb.y0 {
        for i in 0..bw {
            let (x, y) = (bb.x0 + i, bb.y0 + j);
            let k = j * bw + i;
            target.push(rgb[y * w + x]);
            // Which neighbour does this pixel belong to? The nearest labelled one that is
            // not the face itself; pixels owned by a third party contribute to `rest` and
            // are held at their current colour.
            let mut best: Option<usize> = None;
            'search: for r in 0..=2i64 {
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx.abs().max(dy.abs()) != r {
                            continue;
                        }
                        let (px, py) = (x as i64 + dx, y as i64 + dy);
                        if px < 0 || py < 0 || px >= w as i64 || py >= bb.y1 as i64 {
                            continue;
                        }
                        let l = labels[py as usize * w + px as usize] as usize;
                        if l != f {
                            if let Some(&s) = slot.get(&l) {
                                best = Some(s);
                                break 'search;
                            }
                        }
                    }
                }
            }
            match best {
                Some(s) => owner[k] = s,
                None => rest[k] = 0.0,
            }
        }
    }
    // Every other face that reaches into the band, held fixed at its current geometry and
    // colour. Excludes the face itself and its edge-neighbours, which are free columns.
    let mut rest_rgb = vec![[0.0f32; 3]; n];
    let mut tmp = vec![0.0f64; n];
    let mut tmark = vec![false; n];
    for g in 0..face_fill.len() {
        if g == f || slot.contains_key(&g) || g >= order.len() {
            continue;
        }
        let Some((gring, _)) = ring_of(map, &order[g]) else {
            continue;
        };
        let gpts: Vec<Point> = gring.iter().map(|r| r.p).collect();
        let Some(gbb) = Bbox::of(&gpts, w, _h, 1.0) else {
            continue;
        };
        if gbb.x1 <= bb.x0 || gbb.x0 >= bb.x1 || gbb.y1 <= bb.y0 || gbb.y0 >= bb.y1 {
            continue;
        }
        coverage(&gpts, bb, &mut tmp, &mut tmark);
        let c = face_fill[g].model.representative();
        for p in 0..n {
            let a = tmp[p].clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            rest[p] += a;
            for ch in 0..3 {
                rest_rgb[p][ch] += (a as f32) * c[ch];
            }
        }
    }
    for p in 0..n {
        rest[p] = rest[p].min(1.0);
    }

    let mut prior: Vec<[f32; 3]> = Vec::with_capacity(1 + neigh.len());
    prior.push(face_fill[f].model.representative());
    for &g in &neigh {
        prior.push(face_fill[g].model.representative());
    }
    Some(Problem {
        bb,
        target,
        owner,
        prior,
        rest,
        rest_rgb,
    })
}

/// Damped Gauss-Newton on the free vertices, fills eliminated at every trial step.
///
/// The residual has a closed-form derivative, which is what makes this cheap. With the
/// fills held at their least-squares values, band pixel `p` in channel `ch` models as
/// `a_p c_face + (1-a_p) c_other`, so moving any vertex changes it only through the
/// coverage: `d r / d u = (d a_p / d u) (c_face - c_other)`. Only the coverage
/// derivatives need finite differences, and the normal equations assemble from them
/// directly -- no Jacobian is ever stored in full.
fn gauss_newton(
    prob: &Problem,
    verts: &[Point],
    free: &[usize],
    cov: &mut [f64],
    cov2: &mut [f64],
    mark: &mut [bool],
) -> (Vec<Point>, f64) {
    let n = prob.target.len();
    let nf = free.len() * 2;
    let mut p: Vec<Point> = verts.to_vec();
    let (mut best, mut fills) = prob.eval(&p, cov, mark);
    let mut base = vec![0.0f64; n];
    let mut dcov: Vec<Vec<f64>> = vec![vec![0.0; n]; nf];
    let mut plus = vec![0.0f64; n];
    let mut mu = 1e-3f64;

    for _ in 0..GN_ITERS {
        // Coverage at the current point, and its derivative in every free coordinate.
        coverage(&p, prob.bb, &mut base, mark);
        for u in 0..nf {
            let (j, ax) = (free[u / 2], u % 2);
            let mut pp = p.clone();
            let mut pm = p.clone();
            if ax == 0 {
                pp[j].x += FD_STEP;
                pm[j].x -= FD_STEP;
            } else {
                pp[j].y += FD_STEP;
                pm[j].y -= FD_STEP;
            }
            coverage(&pp, prob.bb, &mut plus, mark);
            coverage(&pm, prob.bb, cov2, mark);
            for q in 0..n {
                dcov[u][q] = (plus[q] - cov2[q]) / (2.0 * FD_STEP);
            }
        }
        // Per-pixel colour step across the boundary, and the current residual.
        let mut delta = vec![[0.0f64; 3]; n];
        let mut resid = vec![[0.0f64; 3]; n];
        for q in 0..n {
            let o = prob.owner[q];
            let other = if o == usize::MAX {
                [0.0f32; 3]
            } else {
                fills[o + 1]
            };
            let a = base[q].clamp(0.0, 1.0);
            let wo = if o == usize::MAX {
                0.0
            } else {
                (1.0 - a - prob.rest[q]).max(0.0)
            };
            for ch in 0..3 {
                delta[q][ch] = fills[0][ch] as f64 - other[ch] as f64;
                resid[q][ch] =
                    a * fills[0][ch] as f64 + wo * other[ch] as f64 + prob.rest_rgb[q][ch] as f64
                        - prob.target[q][ch] as f64;
            }
        }
        // Normal equations.
        let mut ata = vec![0.0f64; nf * nf];
        let mut atb = vec![0.0f64; nf];
        for u in 0..nf {
            for v in u..nf {
                let mut s = 0.0;
                for q in 0..n {
                    let du = dcov[u][q];
                    if du == 0.0 {
                        continue;
                    }
                    let dv = dcov[v][q];
                    if dv == 0.0 {
                        continue;
                    }
                    let d = &delta[q];
                    s += du * dv * (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
                }
                ata[u * nf + v] = s;
                ata[v * nf + u] = s;
            }
            let mut s = 0.0;
            for q in 0..n {
                let du = dcov[u][q];
                if du == 0.0 {
                    continue;
                }
                let (d, r) = (&delta[q], &resid[q]);
                s += du * (d[0] * r[0] + d[1] * r[1] + d[2] * r[2]);
            }
            atb[u] = -s;
        }
        let trace: f64 = (0..nf).map(|i| ata[i * nf + i]).sum::<f64>().max(1e-12);

        let mut moved = false;
        for _ in 0..14 {
            let mut m = ata.clone();
            for i in 0..nf {
                m[i * nf + i] += mu * (ata[i * nf + i] + 1e-9 * trace / nf as f64);
            }
            let step = solve1(&mut m, &atb, nf);
            let mut cand = p.clone();
            let mut maxs = 0.0f64;
            for u in 0..nf {
                let (j, ax) = (free[u / 2], u % 2);
                let d = step[u].clamp(-MAX_STEP, MAX_STEP);
                maxs = maxs.max(d.abs());
                if ax == 0 {
                    cand[j].x += d;
                } else {
                    cand[j].y += d;
                }
            }
            // Cumulative leash back to the lattice evidence.
            for &j in free {
                let (dx, dy) = (cand[j].x - verts[j].x, cand[j].y - verts[j].y);
                let r = (dx * dx + dy * dy).sqrt();
                if r > MAX_TOTAL {
                    let k = MAX_TOTAL / r;
                    cand[j] = Point::new(verts[j].x + dx * k, verts[j].y + dy * k);
                }
            }
            if maxs < 1e-5 {
                return (p, best);
            }
            let (sse, f2) = prob.eval(&cand, cov2, mark);
            if sse < best {
                p = cand;
                best = sse;
                fills = f2;
                mu = (mu / 3.0).max(1e-7);
                moved = true;
                break;
            }
            mu *= 4.0;
            if mu > 1e9 {
                break;
            }
        }
        if !moved {
            break;
        }
    }
    (p, best)
}

/// Gauss-Jordan with partial pivoting, one right-hand side.
fn solve1(a: &mut [f64], b: &[f64], k: usize) -> Vec<f64> {
    let mut x = b.to_vec();
    for c in 0..k {
        let mut piv = c;
        for r in c + 1..k {
            if a[r * k + c].abs() > a[piv * k + c].abs() {
                piv = r;
            }
        }
        if a[piv * k + c].abs() < 1e-14 {
            x[c] = 0.0;
            continue;
        }
        if piv != c {
            for j in 0..k {
                a.swap(c * k + j, piv * k + j);
            }
            x.swap(c, piv);
        }
        let d = a[c * k + c];
        for j in 0..k {
            a[c * k + j] /= d;
        }
        x[c] /= d;
        for r in 0..k {
            if r == c {
                continue;
            }
            let f = a[r * k + c];
            if f == 0.0 {
                continue;
            }
            for j in 0..k {
                a[r * k + j] -= f * a[c * k + j];
            }
            x[r] -= f * x[c];
        }
    }
    x
}

/// Replace each edge's points with the decoded vertices it owns.
///
/// Not a projection of the old samples onto the new polygon: the decoded boundary *is* the
/// polygon, and 336 samples of a five-sided shape are 331 redundant numbers. Keeping them
/// is also unsafe. The curve fitter downstream is judged on boundary error and segment
/// count, not on the image, and a thin ribbon's two long sides fit a pair of lines so well
/// that it will happily drop the caps between them -- leaving a path that runs out and
/// back along itself, encloses nothing, and is dropped for having no area. That is exactly
/// what happened to a decoded 2 px stroke before this function stored vertices instead of
/// samples.
///
/// Junction nodes are vertices by construction (see `candidate_orders`) and keep their
/// positions exactly, so the faces on the other side of a shared edge see the same
/// boundary they always did, and the partition is preserved.
///
/// The polygon is written back sampled at about one point per pixel, not as bare corners.
/// Corners alone are not enough information: five points let the fitter run one cubic
/// through them that bulges twenty pixels off the true boundary at no cost to its own
/// objective, which is what it did. Samples along the edges with a small `sigma` state the
/// truth -- this boundary is known to a twentieth of a pixel *everywhere*, not only at its
/// corners -- and under that statement straight lines are the cheapest description and the
/// caps cannot be deleted.
const DECODED_SIGMA: f64 = 0.05;
const SAMPLE_PX: f64 = 1.0;

/// The points one edge would get, in ring order, for a decoded polygon. `None` when the
/// edge owns fewer than two of the polygon's vertices.
fn decoded_edge_points(
    ring: &[RingPt],
    start: usize,
    take: usize,
    closed: bool,
    cuts: &[usize],
    verts: &[Point],
) -> Option<Vec<Point>> {
    let mut corners: Vec<Point> = Vec::new();
    for (j, &c) in cuts.iter().enumerate() {
        if c >= start && c < start + take {
            corners.push(verts[j]);
        }
    }
    if corners.len() < 2 {
        return None;
    }
    if !closed {
        corners.push(ring[(start + take) % ring.len()].p);
    }
    let mut pts: Vec<Point> = Vec::new();
    let n_seg = if closed {
        corners.len()
    } else {
        corners.len() - 1
    };
    for i in 0..n_seg {
        let a = corners[i];
        let b = corners[(i + 1) % corners.len()];
        let steps = (a.dist(b) / SAMPLE_PX).floor().max(1.0) as usize;
        for t in 0..steps {
            let u = t as f64 / steps as f64;
            pts.push(Point::new(a.x + u * (b.x - a.x), a.y + u * (b.y - a.y)));
        }
    }
    if !closed {
        pts.push(*corners.last().unwrap());
    }
    if pts.len() < 2 {
        return None;
    }
    Some(pts)
}

/// What the shipped fitter would actually spend on a decoded polygon, once written back.
///
/// This closes the gap between what the stage optimises and what the document gets. The
/// acceptance test compares parameter counts, and it was comparing its own arithmetic
/// (two per line segment) against the fitter's real answer on the old ring. The fitter
/// does not have to agree: it re-fits the written-back points and may merge segments or
/// spend cubics. Ask it.
fn fitted_params(
    map: &PlanarMap,
    ring: &[RingPt],
    spans: &[EdgeSpan],
    cuts: &[usize],
    verts: &[Point],
    cfg: &FitConfig,
) -> Option<f64> {
    let mut total = 0.0;
    for &(k, rev, start, take) in spans {
        let closed = map.edges[k].closed;
        let mut pts = decoded_edge_points(ring, start, take, closed, cuts, verts)?;
        if rev {
            pts.reverse();
        }
        let n = pts.len();
        let poly = Polyline::new(pts, vec![DECODED_SIGMA; n], closed);
        total += inkvec_fit::multimodel::optimal_multimodel(&poly, cfg).params();
    }
    Some(total)
}

pub(crate) fn write_back(
    map: &mut PlanarMap,
    ring: &[RingPt],
    spans: &[EdgeSpan],
    cuts: &[usize],
    verts: &[Point],
) {
    for &(k, rev, start, take) in spans {
        let closed = map.edges[k].closed;
        let Some(mut pts) = decoded_edge_points(ring, start, take, closed, cuts, verts) else {
            continue;
        };
        if rev {
            pts.reverse();
        }
        let n = pts.len();
        let e = &mut map.edges[k];
        e.points = pts;
        e.sigma = vec![DECODED_SIGMA; n];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(w: usize, h: usize) -> Bbox {
        Bbox {
            x0: 0,
            y0: 0,
            x1: w,
            y1: h,
        }
    }

    #[test]
    fn half_pixel_is_exactly_half() {
        let poly = [
            Point::new(10.0, 10.0),
            Point::new(10.5, 10.0),
            Point::new(10.5, 11.0),
            Point::new(10.0, 11.0),
        ];
        let mut cov = vec![0.0; 32 * 32];
        let mut mark = vec![false; 32 * 32];
        coverage(&poly, bb(32, 32), &mut cov, &mut mark);
        assert!(
            (cov[10 * 32 + 10] - 0.5).abs() < 1e-12,
            "got {}",
            cov[10 * 32 + 10]
        );
        assert!((cov.iter().sum::<f64>() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn triangle_area_is_exact() {
        let poly = [
            Point::new(2.0, 2.0),
            Point::new(12.0, 2.0),
            Point::new(2.0, 12.0),
        ];
        let mut cov = vec![0.0; 32 * 32];
        let mut mark = vec![false; 32 * 32];
        coverage(&poly, bb(32, 32), &mut cov, &mut mark);
        assert!((cov.iter().sum::<f64>() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn decodes_a_thin_diagonal_quad_from_a_wrong_start() {
        // The case the stage exists for: a 2 px stroke on a uniform ground, started from
        // a quad that is half as wide and shifted, with the fills eliminated.
        let (w, h) = (64usize, 64usize);
        let ground = [0.91f32, 0.75, 0.44];
        let ink = [0.13f32, 0.25, 0.50];
        let truth = [
            Point::new(9.3, 54.7),
            Point::new(54.7, 9.3),
            Point::new(56.1, 10.7),
            Point::new(10.7, 56.1),
        ];
        let full = Bbox {
            x0: 0,
            y0: 0,
            x1: w,
            y1: h,
        };
        let mut cov = vec![0.0; w * h];
        let mut mark = vec![false; w * h];
        coverage(&truth, full, &mut cov, &mut mark);
        let target: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let a = cov[i] as f32;
                [
                    a * ink[0] + (1.0 - a) * ground[0],
                    a * ink[1] + (1.0 - a) * ground[1],
                    a * ink[2] + (1.0 - a) * ground[2],
                ]
            })
            .collect();
        let prob = Problem {
            bb: full,
            target,
            owner: vec![0; w * h],
            prior: vec![[0.7, 0.6, 0.45], ground],
            rest: vec![0.0; w * h],
            rest_rgb: vec![[0.0; 3]; w * h],
        };
        // Every vertex starts within MAX_TOTAL of the truth: that leash is what the
        // solver is allowed to correct, and a start outside it is not a case this stage
        // claims to handle. Widths here are 0.7 px out and positions 0.6 px out.
        let start = [
            Point::new(9.9, 54.1),
            Point::new(54.1, 9.9),
            Point::new(55.4, 10.0),
            Point::new(10.0, 55.4),
        ];
        let free = vec![0usize, 1, 2, 3];
        let mut c2 = vec![0.0; w * h];
        let (got, sse) = gauss_newton(&prob, &start, &free, &mut cov, &mut c2, &mut mark);
        assert!(sse < 1e-4, "residual {sse}");
        for (g, t) in got.iter().zip(truth.iter()) {
            assert!(g.dist(*t) <= MAX_TOTAL + 1e-9, "vertex escaped the leash");
        }
        let err = got
            .iter()
            .zip(truth.iter())
            .map(|(a, b)| (a.x - b.x).abs().max((a.y - b.y).abs()))
            .fold(0.0f64, f64::max);
        assert!(err < 0.05, "vertex error {err} px, got {got:?}");
    }
}
