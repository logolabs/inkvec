//! Nodes at extrema: each smooth run of a fitted path re-segmented so that its nodes sit
//! where the curve is horizontal or vertical and its joins are smooth, wherever the fit's own
//! objective prefers it.
//!
//! # The convention
//!
//! Type designers and icon artists put a curve's on-curve points at its x and y extrema.
//! Adobe's *Type 1 Font Format* (1990), §4.1 "Points at Extremes": an endpoint "should be
//! placed at most horizontal or vertical extremes. This implies that most curves should not
//! include more than 90 degrees of arc"; §4.2: at a smooth transition the node and the control
//! points either side "should all be collinear. This is especially important at horizontal
//! and vertical extremes". At a smooth node the handles lie along the tangent, and the tangent
//! is horizontal or vertical exactly where the curve has a y or an x extremum: nodes at extrema
//! and axis-aligned handles are one convention. On the 246-icon screen set at 512 px on white
//! (the human battery, `bench/human_stats.py`, family-macro means) the artists' files have
//! 0.38 of their cubic handles within 0.5° of an axis, 0.65 of their smooth joins at an
//! extremum (0.95 in material-icons, 0.33 in noto-emoji) and 0.035 of their joins between
//! curves kinked (3-30°); the trace had 0.061, 0.458 and 0.142.
//!
//! # Why the fit did not find it, and why it is cheaper
//!
//! The dynamic program's cubic takes its end directions from the tangent estimator at measured
//! points, so it puts a node only where a measured point is, with the estimated direction. A
//! node at an extremum with an axis direction is a model it never offers, and sliding a node
//! along a smooth curve barely changes the drawing (the likelihood's null space), so nothing
//! pulls it there. Yet with both end directions fixed a cubic has four free numbers, its end
//! point and two arm lengths, against five for a G1 one: at a node where the convention fixes
//! the direction, the direction is not written. [`FIT`] credits that one coordinate per
//! anchored node, `λ` nats, the price of a coordinate under the fit's objective
//! (`docs/theory/chain-representation.md`, R0.2).
//!
//! # What runs
//!
//! [`anchor_extrema`] finds the runs (consecutive cubics and circular arcs joined at under
//! `RUN_BREAK_DEGREES`), maps every node to a measured point, finds each run's extrema, and
//! searches the cheapest chain through two kinds of candidate node: the run's old interior
//! nodes made G1 on the bisector of their two directions, and the extrema with their axis
//! direction (`shortest_chain`). The chosen nodes are refined (`refine_nodes`), every span's
//! arms polished on all its points, and the chain replaces the run when its exact `χ²` plus
//! prices (the program's: `λ` per parameter, the wobble penalty, the break cost of every kinked
//! join) is lower. Under [`FIT`] it must also write as many numbers as the run: a run with an
//! arc keeps it (`A` is 7 numbers, a cubic 6), and a run of cubics keeps its count, so the
//! pass moves nodes and smooths joins without changing what the file costs. Where a run meets
//! a line at a kink, the chain is also offered leaving along the line, which makes that join
//! smooth.
//!
//! # Where it sits
//!
//! `crate::multimodel` calls it on every uncapped fit after `crate::merge`'s passes, on the
//! centred polyline, except inside [`without`] (a stroke's centreline, whose fit the stroke
//! decision weighs against the fill's); `inkvec-cli`'s `--editability` calls it again with a
//! stronger [`Preference`] on each finished edge. In: a fitted path and the measured polyline it was
//! fitted to, in px. Out: the same path, its smooth runs rewritten as cubics.

use inkvec_core::{Point, Polyline, Vec2};
use kurbo::{CubicBez, ParamCurveNearest, Point as KPoint};

use crate::candidates::{best_cubic, raw_moments_direct, unit, Cubic};
use crate::curves::{arc_ellipse_center, cubic_self_intersects, Segment};
use crate::structural::eval_segment;
use crate::tangents::{arc_lengths, break_cost, turn_angle};
use crate::{FitConfig, FittedPath};

/// How strongly a caller prefers nodes at extrema: what an anchored node is credited, and
/// whether a re-segmentation may change how many numbers the file writes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preference {
    /// Parameters credited for each node whose direction the convention fixes.
    pub credit: f64,
    /// Accept only a re-segmentation that writes exactly as many numbers as the run it
    /// replaces (the gate's count: line 2, cubic 6, arc 7).
    pub same_count: bool,
}

/// The fit's own preference: one parameter per anchored node, the coordinate (the tangent's
/// angle) the node no longer writes, and no change in the written count.
pub const FIT: Preference = Preference {
    credit: 1.0,
    same_count: true,
};

thread_local! {
    /// Set while [`without`] runs: the fits made on this thread skip the pass.
    static SKIP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with the pass switched off for every fit `f` makes on this thread. For a caller
/// whose fits feed a decision priced on the plain fit, such as the stroke detection's
/// centrelines, whose description competes with the outline's.
pub fn without<T>(f: impl FnOnce() -> T) -> T {
    let prev = SKIP.with(|s| s.replace(true));
    let out = f();
    SKIP.with(|s| s.set(prev));
    out
}

/// A join turning by at least this many degrees ends a run: a corner, kept where it is (the
/// artists' joins between curves are smooth or turn by more than this, R3.1 of the chain).
const RUN_BREAK_DEGREES: f64 = 30.0;
/// Two extrema of one axis whose extreme values differ by less than this (px) are a wiggle
/// on a flat stretch, not two features: both are dropped.
const WIGGLE_PX: f64 = 0.5;
/// An extremum closer than this (px of arc length) to a run's end gets no node of its own.
const END_GAP_PX: f64 = 1.0;
/// Two extrema of one axis and direction closer than this (px of arc length) are one: a
/// root of the derivative next to a node that is itself the extremum.
const DEDUP_PX: f64 = 0.25;
/// Fewest measured points strictly inside a span between two nodes.
const MIN_INTERIOR: usize = 2;
/// How many candidate nodes ahead one span may reach in the search.
const MAX_REACH: usize = 8;
/// A path node with no measured point within this distance (px) is not mapped, and the
/// path is left alone.
const FAR_PX: f64 = 3.0;
/// Rounds of the node refinement.
const REFINE_ROUNDS: usize = 3;
/// The refinement's compass steps, px; a node's direction turns by twice the step, in degrees.
const REFINE_STEPS: [f64; 5] = [0.5, 0.25, 0.12, 0.06, 0.03];

/// Which coordinate is extreme at a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// `x` is extreme: the tangent is vertical.
    X,
    /// `y` is extreme: the tangent is horizontal.
    Y,
}

/// A node of a proposed chain.
#[derive(Debug, Clone, Copy)]
struct Node {
    /// Position, px.
    p: Point,
    /// Unit direction of travel through the node.
    t: Vec2,
    /// Unrolled index of the measured point nearest the node.
    u: usize,
    /// Whether the extremum convention fixes the direction.
    anchored: bool,
}

/// An extremum of the old chain.
#[derive(Debug, Clone, Copy)]
struct Extremum {
    /// The run-local segment it lies on (`0` for the run's first).
    q: usize,
    /// Arc length from the run's start, px.
    s: f64,
    /// Position, px.
    p: Point,
    /// The coordinate that is extreme.
    axis: Axis,
    /// The direction of travel there: a unit vector along an axis.
    dir: Vec2,
}

/// What the search reads: the price of a parameter and the credit of an anchored node.
#[derive(Debug, Clone, Copy)]
struct Prices {
    /// `λ`, nats per parameter.
    lambda: f64,
    /// Parameters credited per anchored node.
    credit: f64,
    /// See [`Preference::same_count`].
    same_count: bool,
}

/// Re-segment every smooth run of `path` at its x and y extrema where that costs less under
/// the fit's objective, as `pref` prices an anchored node ([`FIT`] in the fit; a stronger
/// preference under `--editability`). `poly` is the measured boundary `path` was fitted to,
/// in the same units. Returns the number of runs replaced; the path is unchanged when that
/// is 0.
pub fn anchor_extrema(
    path: &mut FittedPath,
    poly: &Polyline,
    cfg: &FitConfig,
    pref: Preference,
) -> usize {
    if SKIP.with(std::cell::Cell::get) {
        return 0;
    }
    let Some(ring) = Ring::new(path, poly) else {
        return 0;
    };
    let prices = Prices {
        lambda: cfg.lambda,
        credit: pref.credit,
        same_count: pref.same_count,
    };
    let accepted: Vec<(Run, Proposal)> = ring
        .runs()
        .into_iter()
        .filter_map(|run| ring.improve(&run, poly, &prices).map(|p| (run, p)))
        .collect();
    let count = accepted.len();
    if count > 0 {
        let (start, segments) = ring.splice(&accepted);
        path.start = start;
        path.segments = segments;
    }
    count
}

/// A run: consecutive curved segments joined at less than [`RUN_BREAK_DEGREES`].
#[derive(Debug, Clone, Copy)]
struct Run {
    /// Index of its first segment in the path.
    first: usize,
    /// Number of segments.
    len: usize,
    /// The run is the whole closed path, with no end.
    cyclic: bool,
}

/// What re-segmenting one run proposes.
struct Proposal {
    /// The new chain's start.
    start: Point,
    /// Its segments, all cubics.
    segs: Vec<Segment>,
    /// Its price, nats, comparable with [`OldPrice::cost`].
    cost: f64,
    /// Nodes anchored on an axis.
    anchored: usize,
}

/// The price of a run as the path has it.
struct OldPrice {
    /// Nats: residual, parameters, wobble and the breaks at its joins.
    cost: f64,
    /// Numbers the segments write (line 2, cubic 6, arc 7).
    written: f64,
    /// Whether any join inside it turns by 3 degrees or more.
    kinked: bool,
}

/// A fitted path with what every run's search reads about it.
struct Ring<'a> {
    path: &'a FittedPath,
    /// Segments.
    m: usize,
    closed: bool,
    /// Node `q` is where segment `q` starts; `m + 1` of them.
    starts: Vec<Point>,
    /// Unrolled measured index of every node, increasing; on a closed path the last is the
    /// first plus `n`.
    uu: Vec<usize>,
    /// Segment `q` is a cubic or a circular arc.
    curved: Vec<bool>,
    /// Turn at node `q`, radians (infinite at an open path's ends).
    joins: Vec<f64>,
}

impl<'a> Ring<'a> {
    fn new(path: &'a FittedPath, poly: &Polyline) -> Option<Self> {
        let (n, m) = (poly.len(), path.segments.len());
        if m == 0 || n < 4 || path.closed != poly.closed {
            return None;
        }
        let closed = path.closed;
        let mut starts = Vec::with_capacity(m + 1);
        starts.push(path.start);
        starts.extend(path.segments.iter().map(Segment::end));
        let uu = node_indices(poly, path, &starts)?;
        let curved = path.segments.iter().map(is_curved).collect();
        let joins = (0..=m)
            .map(|q| {
                let (a, b) = match q {
                    0 if closed => (m - 1, 0),
                    _ if q == 0 || q == m => return f64::INFINITY,
                    _ => (q - 1, q),
                };
                match (
                    tangent_at(&path.segments[a], starts[a], 1.0),
                    tangent_at(&path.segments[b], starts[b], 0.0),
                ) {
                    (Some(x), Some(y)) => turn_angle(x, y),
                    _ => f64::INFINITY,
                }
            })
            .collect();
        Some(Ring {
            path,
            m,
            closed,
            starts,
            uu,
            curved,
            joins,
        })
    }

    /// Node `q` ends a run: a corner, a line on either side, or an open path's end.
    fn boundary(&self, q: usize) -> bool {
        let q = q % self.m;
        if !self.closed && q == 0 {
            return true;
        }
        let prev = (q + self.m - 1) % self.m;
        self.joins[q] >= RUN_BREAK_DEGREES.to_radians() || !self.curved[prev] || !self.curved[q]
    }

    /// The runs, walking from a boundary so that none wraps past the walk's start.
    fn runs(&self) -> Vec<Run> {
        let m = self.m;
        let b0 = if self.closed {
            match (0..m).find(|&q| self.boundary(q)) {
                Some(b) => b,
                None if self.curved.iter().all(|&c| c) => {
                    return vec![Run {
                        first: 0,
                        len: m,
                        cyclic: true,
                    }];
                }
                None => return Vec::new(),
            }
        } else {
            0
        };
        let mut runs = Vec::new();
        let mut k = 0;
        while k < m {
            let q = (b0 + k) % m;
            if !self.curved[q] {
                k += 1;
                continue;
            }
            let mut len = 1;
            while k + len < m && !self.boundary(q + len) {
                len += 1;
            }
            runs.push(Run {
                first: q,
                len,
                cyclic: false,
            });
            k += len;
        }
        runs
    }

    /// The run's own segments, the start of each and its end, and the unrolled index of
    /// every node.
    fn geometry(&self, run: &Run) -> RunGeom {
        let segs: Vec<Segment> = (0..run.len)
            .map(|k| self.path.segments[(run.first + k) % self.m].clone())
            .collect();
        let mut nodes_u = vec![self.uu[run.first]];
        for k in 0..run.len {
            let q = (run.first + k) % self.m;
            nodes_u.push(nodes_u[k] + self.uu[q + 1] - self.uu[q]);
        }
        RunGeom::new(segs, self.starts[run.first], nodes_u, run.cyclic)
    }

    /// The run's price as the path has it. Its end joins are priced by the caller.
    fn old_price(&self, run: &Run, g: &RunGeom, poly: &Polyline, prices: &Prices) -> OldPrice {
        let chi2 = g.chain_chi2(poly, g.start(), &g.segs, &g.nodes_u);
        let mut cost = 0.5 * chi2;
        let mut written = 0.0;
        for (q, s) in g.segs.iter().enumerate() {
            cost += prices.lambda * s.params() + wobble(g.starts[q], s, prices.lambda);
            written += written_count(s);
        }
        let inner = if run.cyclic { run.len } else { run.len - 1 };
        let mut kinked = false;
        for k in 0..inner {
            let turn = self.joins[(run.first + k + 1) % self.m];
            cost += break_cost_of(turn, prices.lambda);
            kinked |= turn >= 3f64.to_radians();
        }
        OldPrice {
            cost,
            written,
            kinked,
        }
    }

    /// Where a run meets a line at a kink (not a corner): the line's direction, which the
    /// run may take instead to make that join smooth, and the break the join pays now.
    fn line_ends(&self, run: &Run) -> [(Option<Vec2>, f64); 2] {
        let mut out = [(None, 0.0); 2];
        if run.cyclic {
            return out;
        }
        let m = self.m;
        let line_dir = |q: usize| match self.path.segments[q] {
            Segment::Line(e) => unit(e - self.starts[q]),
            _ => None,
        };
        let kink =
            |q: usize| self.joins[q] < RUN_BREAK_DEGREES.to_radians() && self.joins[q] > 1e-9;
        let last = run.first + run.len;
        if (self.closed || run.first > 0) && kink(run.first) {
            if let Some(d) = line_dir((run.first + m - 1) % m) {
                out[0] = (Some(d), self.joins[run.first]);
            }
        }
        if (self.closed || last < m) && kink(last % m) {
            if let Some(d) = line_dir(last % m) {
                out[1] = (Some(d), self.joins[last % m]);
            }
        }
        out
    }

    /// The best re-segmentation of one run, if it is cheaper than the run and writes no
    /// more numbers.
    fn improve(&self, run: &Run, poly: &Polyline, prices: &Prices) -> Option<Proposal> {
        let g = self.geometry(run);
        let old = self.old_price(run, &g, poly, prices);
        let ends = self.line_ends(run);
        let brk = |e: &(Option<Vec2>, f64)| break_cost_of(e.1, prices.lambda);
        let old_cost = old.cost + brk(&ends[0]) + brk(&ends[1]);
        // As the run leaves now, and (where a line meets it at a kink) along the line.
        let plain = g.propose(poly, prices, (None, None)).map(|mut p| {
            p.cost += brk(&ends[0]) + brk(&ends[1]);
            (p, false)
        });
        let smooth = (ends[0].0.is_some() || ends[1].0.is_some())
            .then(|| g.propose(poly, prices, (ends[0].0, ends[1].0)))
            .flatten()
            .map(|mut p| {
                p.cost += ends.iter().filter(|e| e.0.is_none()).map(brk).sum::<f64>();
                (p, true)
            });
        let (prop, smoothed) = match (plain, smooth) {
            (Some(p), Some(s)) => {
                if s.0.cost < p.0.cost {
                    s
                } else {
                    p
                }
            }
            (p, s) => p.or(s)?,
        };
        // Only a run that gains a node at an extremum or loses a kink is touched.
        if prop.anchored == 0 && !old.kinked && !smoothed {
            return None;
        }
        let written = 6.0 * prop.segs.len() as f64;
        let count_ok = if prices.same_count {
            (written - old.written).abs() < 1e-9
        } else {
            true
        };
        (prop.cost < old_cost && count_ok).then_some(prop)
    }

    /// The path's start and segments with the accepted runs written in. A cyclic run
    /// replaces it whole; otherwise the path is rebuilt from a boundary node, so a closed
    /// path may start at another node.
    fn splice(&self, accepted: &[(Run, Proposal)]) -> (Point, Vec<Segment>) {
        if let Some((_, prop)) = accepted.iter().find(|(r, _)| r.cyclic) {
            return (prop.start, prop.segs.clone());
        }
        let m = self.m;
        let b0 = if self.closed {
            (0..m).find(|&q| self.boundary(q)).unwrap_or(0)
        } else {
            0
        };
        let mut segs = Vec::with_capacity(m);
        let mut k = 0;
        while k < m {
            let q = (b0 + k) % m;
            if let Some((run, prop)) = accepted.iter().find(|(r, _)| r.first == q) {
                segs.extend(prop.segs.iter().cloned());
                k += run.len;
            } else {
                segs.push(self.path.segments[q].clone());
                k += 1;
            }
        }
        (self.starts[b0], segs)
    }
}

/// One run's segments, in its own order, and where everything on it is.
struct RunGeom {
    segs: Vec<Segment>,
    /// Start of each segment, and the run's end: `segs.len() + 1` points.
    starts: Vec<Point>,
    /// Unrolled measured index of each of those nodes.
    nodes_u: Vec<usize>,
    /// Arc length at each node from the run's start, px.
    cum: Vec<f64>,
    cyclic: bool,
}

impl RunGeom {
    fn new(segs: Vec<Segment>, start: Point, nodes_u: Vec<usize>, cyclic: bool) -> Self {
        let mut starts = vec![start];
        starts.extend(segs.iter().map(Segment::end));
        let mut cum = vec![0.0];
        for (q, s) in segs.iter().enumerate() {
            cum.push(cum[q] + length_to(s, starts[q], 1.0));
        }
        RunGeom {
            segs,
            starts,
            nodes_u,
            cum,
            cyclic,
        }
    }

    fn start(&self) -> Point {
        self.starts[0]
    }

    /// `χ²` of the measured points over a chain's index range: each point's squared
    /// distance to the nearest of the segment its index falls in and that segment's two
    /// neighbours, over its sigma squared. A cyclic chain scores each point once.
    fn chain_chi2(
        &self,
        poly: &Polyline,
        start: Point,
        segs: &[Segment],
        nodes_u: &[usize],
    ) -> f64 {
        let n = poly.len();
        let m = segs.len();
        let mut starts = vec![start];
        starts.extend(segs.iter().map(Segment::end));
        let end = if self.cyclic {
            nodes_u[m]
        } else {
            nodes_u[m] + 1
        };
        let mut acc = 0.0;
        let mut q = 0usize;
        for u in nodes_u[0]..end {
            while q + 1 < m && u > nodes_u[q + 1] {
                q += 1;
            }
            let p = poly.points[u % n];
            let sg = poly.sigma[u % n].max(1e-6);
            let near = [
                Some(q),
                if q > 0 {
                    Some(q - 1)
                } else {
                    self.cyclic.then(|| m - 1)
                },
                if q + 1 < m {
                    Some(q + 1)
                } else {
                    self.cyclic.then_some(0)
                },
            ];
            let best = near
                .into_iter()
                .flatten()
                .map(|r| dist2_to(&segs[r], starts[r], p))
                .fold(f64::INFINITY, f64::min);
            acc += best / (sg * sg);
        }
        acc
    }

    /// The anchored chain for this run, `ends` overriding the directions it leaves and
    /// arrives with, and its price (without the breaks at its ends).
    fn propose(
        &self,
        poly: &Polyline,
        prices: &Prices,
        ends: (Option<Vec2>, Option<Vec2>),
    ) -> Option<Proposal> {
        let ext = self.extrema();
        let nodes = self.candidates(poly, &ext, ends)?;
        let mut chosen = shortest_chain(poly, &nodes, prices)?;
        refine_nodes(poly, &mut chosen, self.cyclic, prices.lambda);
        let segs: Vec<Segment> = chosen
            .windows(2)
            .map(|w| fit_span(poly, &w[0], &w[1]))
            .collect::<Option<_>>()?;
        let nodes_u: Vec<usize> = chosen.iter().map(|nd| nd.u).collect();
        let start = chosen[0].p;
        let chi2 = self.chain_chi2(poly, start, &segs, &nodes_u);
        // The start of an open run is the old node, never credited; a cyclic run's first
        // node is also its last, credited once.
        let anchored = chosen[..chosen.len() - 1]
            .iter()
            .enumerate()
            .filter(|(k, nd)| nd.anchored && (*k > 0 || self.cyclic))
            .count();
        let mut cost = 0.5 * chi2 - prices.lambda * prices.credit * anchored as f64;
        let mut st = start;
        for s in &segs {
            cost += prices.lambda * s.params() + wobble(st, s, prices.lambda);
            st = s.end();
        }
        Some(Proposal {
            start,
            segs,
            cost,
            anchored,
        })
    }

    /// The x and y extrema of the run, in order along it: inside segments, and at nodes
    /// where a tangent component changes sign. Near-duplicates merged, wiggles on flat
    /// stretches dropped, and (on an open run) none within [`END_GAP_PX`] of its ends.
    fn extrema(&self) -> Vec<Extremum> {
        let (segs, starts) = (&self.segs, &self.starts);
        let len = segs.len();
        let mut ext: Vec<Extremum> = Vec::new();
        for q in 0..len {
            if q > 0 || self.cyclic {
                let prev = (q + len - 1) % len;
                if let (Some(ti), Some(to)) = (
                    tangent_at(&segs[prev], starts[prev], 1.0),
                    tangent_at(&segs[q], starts[q], 0.0),
                ) {
                    for axis in [Axis::X, Axis::Y] {
                        if comp(axis, ti) * comp(axis, to) <= 0.0 {
                            let d = Vec2 {
                                x: ti.x + to.x,
                                y: ti.y + to.y,
                            };
                            ext.push(Extremum {
                                q,
                                s: self.cum[q],
                                p: starts[q],
                                axis,
                                dir: axis_dir(axis, d),
                            });
                        }
                    }
                }
            }
            for (t, axis) in interior_extrema(&segs[q], starts[q]) {
                let Some(d) = tangent_at(&segs[q], starts[q], t) else {
                    continue;
                };
                ext.push(Extremum {
                    q,
                    s: self.cum[q] + length_to(&segs[q], starts[q], t),
                    p: eval_segment(&segs[q], starts[q], t),
                    axis,
                    dir: axis_dir(axis, d),
                });
            }
        }
        ext.sort_by(|x, y| x.s.total_cmp(&y.s));
        let mut out: Vec<Extremum> = Vec::new();
        for e in ext {
            let dup = out
                .iter()
                .rev()
                .find(|d| d.axis == e.axis)
                .is_some_and(|last| (e.s - last.s).abs() < DEDUP_PX && last.dir.dot(e.dir) > 0.0);
            if !dup {
                out.push(e);
            }
        }
        drop_wiggles(&mut out);
        if !self.cyclic {
            let total = self.cum[len];
            out.retain(|e| e.s > END_GAP_PX && e.s < total - END_GAP_PX);
        }
        out
    }

    /// The nodes a chain may use, in order: the run's ends (open run), its old interior
    /// nodes made G1 on the bisector of their two directions, and its extrema, anchored. A
    /// cyclic run starts and ends on its first anchored candidate.
    fn candidates(
        &self,
        poly: &Polyline,
        ext: &[Extremum],
        ends: (Option<Vec2>, Option<Vec2>),
    ) -> Option<Vec<Node>> {
        let n = poly.len();
        let (segs, starts) = (&self.segs, &self.starts);
        let len = segs.len();
        let (first_u, last_u) = (self.nodes_u[0], self.nodes_u[len]);
        let mut cands: Vec<(f64, Node)> = Vec::new();
        for q in 0..len {
            if q == 0 && !self.cyclic {
                continue;
            }
            let prev = (q + len - 1) % len;
            let (Some(ti), Some(to)) = (
                tangent_at(&segs[prev], starts[prev], 1.0),
                tangent_at(&segs[q], starts[q], 0.0),
            ) else {
                continue;
            };
            if let Some(t) = unit(Vec2 {
                x: ti.x + to.x,
                y: ti.y + to.y,
            }) {
                let node = Node {
                    p: starts[q],
                    t,
                    u: self.nodes_u[q],
                    anchored: false,
                };
                cands.push((self.cum[q], node));
            }
        }
        for e in ext {
            let lo = self.nodes_u[e.q].saturating_sub(1).max(first_u);
            let hi = (self.nodes_u[e.q + 1] + 1).min(last_u);
            let node = Node {
                p: e.p,
                t: e.dir,
                u: nearest_index(poly, e.p, lo, hi),
                anchored: true,
            };
            cands.push((e.s, node));
        }
        // By arc length; where two sit at one point, the anchored one first.
        cands.sort_by(|x, y| x.0.total_cmp(&y.0).then(y.1.anchored.cmp(&x.1.anchored)));
        let mut nodes: Vec<Node> = Vec::with_capacity(cands.len() + 2);
        if self.cyclic {
            if cands.len() < 2 {
                return None;
            }
            let k0 = cands.iter().position(|c| c.1.anchored).unwrap_or(0);
            cands.rotate_left(k0);
            let u0 = cands[0].1.u;
            for (_, c) in &cands {
                let mut c = *c;
                if c.u < u0 {
                    c.u += n;
                }
                nodes.push(c);
            }
            let mut last = nodes[0];
            last.u += n;
            nodes.push(last);
        } else {
            let t0 = match ends.0 {
                Some(d) => d,
                None => tangent_at(&segs[0], starts[0], 0.0)?,
            };
            let t1 = match ends.1 {
                Some(d) => d,
                None => tangent_at(&segs[len - 1], starts[len - 1], 1.0)?,
            };
            nodes.push(Node {
                p: starts[0],
                t: t0,
                u: first_u,
                anchored: false,
            });
            nodes.extend(cands.iter().map(|c| c.1));
            nodes.push(Node {
                p: starts[len],
                t: t1,
                u: last_u,
                anchored: false,
            });
        }
        Some(nodes)
    }
}

/// Drop pairs of consecutive extrema of one axis whose extreme values differ by less than
/// [`WIGGLE_PX`]: a bump of that size on a flat stretch is not a feature to put nodes on.
fn drop_wiggles(ext: &mut Vec<Extremum>) {
    'again: loop {
        for axis in [Axis::X, Axis::Y] {
            let idx: Vec<usize> = (0..ext.len()).filter(|&i| ext[i].axis == axis).collect();
            for w in idx.windows(2) {
                let v = |e: &Extremum| if axis == Axis::X { e.p.x } else { e.p.y };
                if (v(&ext[w[0]]) - v(&ext[w[1]])).abs() < WIGGLE_PX {
                    ext.remove(w[1]);
                    ext.remove(w[0]);
                    continue 'again;
                }
            }
        }
        return;
    }
}

/// The cheapest chain through the candidates from the first to the last: a shortest path
/// in which a span is the cubic with both end directions fixed ([`fit_span_quick`]), priced
/// as the dynamic program prices a cubic, less the credit of the node it ends on when that
/// node is anchored and is not the chain's last.
fn shortest_chain(poly: &Polyline, nodes: &[Node], prices: &Prices) -> Option<Vec<Node>> {
    let c = nodes.len();
    if c < 2 {
        return None;
    }
    let mut best = vec![f64::INFINITY; c];
    let mut from = vec![usize::MAX; c];
    best[0] = 0.0;
    for j in 1..c {
        let credit = if j + 1 < c && nodes[j].anchored {
            prices.lambda * prices.credit
        } else {
            0.0
        };
        for i in j.saturating_sub(MAX_REACH)..j {
            if !best[i].is_finite() || nodes[j].u < nodes[i].u + 1 + MIN_INTERIOR {
                continue;
            }
            let Some((chi2, wob)) = fit_span_quick(poly, &nodes[i], &nodes[j], prices.lambda)
            else {
                continue;
            };
            let cost =
                best[i] + 0.5 * chi2 + prices.lambda * crate::multimodel::params_cubic() + wob
                    - credit;
            if cost < best[j] {
                best[j] = cost;
                from[j] = i;
            }
        }
    }
    if !best[c - 1].is_finite() {
        return None;
    }
    let mut idx = vec![c - 1];
    while let Some(&k) = idx.last() {
        if k == 0 {
            break;
        }
        idx.push(from[k]);
    }
    idx.reverse();
    Some(idx.into_iter().map(|k| nodes[k]).collect())
}

/// Move each interior node (and turn one the convention does not anchor) to where its two
/// spans fit best: a compass search on the subsampled residual of the two cubics. A node
/// keeps at least [`MIN_INTERIOR`] measured points on either side.
fn refine_nodes(poly: &Polyline, nodes: &mut [Node], cyclic: bool, lambda: f64) {
    let c = nodes.len();
    if c < 3 {
        return;
    }
    let pair = |nodes: &[Node], k: usize| -> f64 {
        match (
            fit_span_quick(poly, &nodes[k - 1], &nodes[k], lambda),
            fit_span_quick(poly, &nodes[k], &nodes[k + 1], lambda),
        ) {
            (Some(a), Some(b)) => a.0 + b.0,
            _ => f64::INFINITY,
        }
    };
    for _ in 0..REFINE_ROUNDS {
        let mut moved = false;
        for k in 1..c - 1 {
            let mut cur = pair(nodes, k);
            let lo = nodes[k - 1].u + 1 + MIN_INTERIOR;
            let hi = nodes[k + 1].u.saturating_sub(1 + MIN_INTERIOR);
            if !cur.is_finite() || hi < lo {
                continue;
            }
            for step in REFINE_STEPS {
                let turn = if nodes[k].anchored {
                    0.0
                } else {
                    (2.0 * step).to_radians()
                };
                let tries = [
                    (step, 0.0, 0.0),
                    (-step, 0.0, 0.0),
                    (0.0, step, 0.0),
                    (0.0, -step, 0.0),
                    (0.0, 0.0, turn),
                    (0.0, 0.0, -turn),
                ];
                let mut improved = true;
                while improved {
                    improved = false;
                    for &(dx, dy, da) in &tries {
                        if dx == 0.0 && dy == 0.0 && da == 0.0 {
                            continue;
                        }
                        let saved = nodes[k];
                        let (s, co) = da.sin_cos();
                        nodes[k].p = Point::new(saved.p.x + dx, saved.p.y + dy);
                        nodes[k].t = Vec2 {
                            x: co * saved.t.x - s * saved.t.y,
                            y: s * saved.t.x + co * saved.t.y,
                        };
                        nodes[k].u = nearest_index(poly, nodes[k].p, lo, hi);
                        let v = pair(nodes, k);
                        if v < cur - 1e-9 {
                            cur = v;
                            improved = true;
                            moved = true;
                        } else {
                            nodes[k] = saved;
                        }
                    }
                }
            }
        }
        if !moved {
            break;
        }
    }
    if cyclic {
        let mut last = nodes[0];
        last.u += poly.len();
        nodes[c - 1] = last;
    }
}

/// The measured points strictly between two nodes, with the nodes themselves as the ends:
/// points, sigmas and cumulative arc lengths. `None` with fewer than [`MIN_INTERIOR`]
/// points inside.
fn span_points(poly: &Polyline, a: &Node, b: &Node) -> Option<(Vec<Point>, Vec<f64>, Vec<f64>)> {
    let n = poly.len();
    if b.u < a.u + 1 + MIN_INTERIOR {
        return None;
    }
    let mut pts = Vec::with_capacity(b.u - a.u + 1);
    let mut sig = Vec::with_capacity(b.u - a.u + 1);
    pts.push(a.p);
    sig.push(poly.sigma[a.u % n]);
    for u in a.u + 1..b.u {
        pts.push(poly.points[u % n]);
        sig.push(poly.sigma[u % n]);
    }
    pts.push(b.p);
    sig.push(poly.sigma[b.u % n]);
    let s = arc_lengths(&pts);
    Some((pts, sig, s))
}

/// The cubic from `a` to `b` with both end directions fixed, scored on a subsample for the
/// search: its residual estimate and wobble penalty. Arms by area and moment only.
fn fit_span_quick(poly: &Polyline, a: &Node, b: &Node, lambda: f64) -> Option<(f64, f64)> {
    let (pts, sig, s) = span_points(poly, a, b)?;
    let j = pts.len() - 1;
    let raw = raw_moments_direct(&pts, 0, j);
    let (chi2, d0, d1) = best_cubic(&pts, &sig, &s, 0, j, a.t, b.t, raw, true)?;
    let cb = Cubic::from_arms(a.p, b.p, a.t, b.t, a.p.dist(b.p), d0, d1);
    Some((chi2, cb.wobble_penalty(lambda)))
}

/// The cubic from `a` to `b` with both end directions fixed: arms by area and moment, then
/// polished on every point (`crate::multimodel::polish_arms`). `None` for a span too short
/// to fit or a cubic that crosses itself.
fn fit_span(poly: &Polyline, a: &Node, b: &Node) -> Option<Segment> {
    let (pts, sig, s) = span_points(poly, a, b)?;
    let j = pts.len() - 1;
    let raw = raw_moments_direct(&pts, 0, j);
    let init = best_cubic(&pts, &sig, &s, 0, j, a.t, b.t, raw, false)
        .map_or((1.0 / 3.0, 1.0 / 3.0), |(_, d0, d1)| (d0, d1));
    let ((d0, d1), _) = crate::multimodel::polish_arms(&pts, &sig, &s, 0, j, a.t, b.t, init);
    let chord = a.p.dist(b.p);
    if chord < 1e-9 {
        return None;
    }
    let cb = Cubic::from_arms(a.p, b.p, a.t, b.t, chord, d0, d1);
    if cubic_self_intersects(cb.p0, cb.p1, cb.p2, cb.p3) {
        return None;
    }
    Some(Segment::Cubic(cb.p1, cb.p2, cb.p3))
}

/// The measured point nearest each node of the path, as unrolled indices increasing along
/// it (a closed path's last node is its first plus `n`). `None` when a node has no
/// measured point within [`FAR_PX`] or the order cannot be kept.
fn node_indices(poly: &Polyline, path: &FittedPath, starts: &[Point]) -> Option<Vec<usize>> {
    let n = poly.len();
    let m = path.segments.len();
    let s = arc_lengths(&poly.points);
    let total = s[n - 1]
        + if path.closed {
            poly.points[n - 1].dist(poly.points[0])
        } else {
            0.0
        };
    let along = |u: usize| s[u % n] + (u / n) as f64 * total;
    let u0 = nearest_index(
        poly,
        starts[0],
        0,
        if path.closed { n - 1 } else { (n - 1).min(8) },
    );
    if poly.points[u0].dist(starts[0]) > FAR_PX {
        return None;
    }
    let end_u = if path.closed { u0 + n } else { n - 1 };
    let mut out = vec![u0];
    for q in 0..m - 1 {
        let prev = out[q];
        let reach = 1.5 * length_to(&path.segments[q], starts[q], 1.0) + 4.0;
        let (mut best, mut bd) = (usize::MAX, f64::INFINITY);
        let mut u = prev + 1;
        while u < end_u && along(u) - along(prev) <= reach {
            let d = poly.points[u % n].dist(starts[q + 1]);
            if d < bd {
                (best, bd) = (u, d);
            }
            u += 1;
        }
        if best == usize::MAX || bd > FAR_PX {
            return None;
        }
        out.push(best);
    }
    out.push(end_u);
    out.windows(2).all(|w| w[1] > w[0]).then_some(out)
}

/// The measured point (unrolled index in `lo..=hi`) nearest `p`.
fn nearest_index(poly: &Polyline, p: Point, lo: usize, hi: usize) -> usize {
    let n = poly.len();
    (lo..=hi)
        .min_by(|&a, &b| {
            poly.points[a % n]
                .dist(p)
                .total_cmp(&poly.points[b % n].dist(p))
        })
        .unwrap_or(lo)
}

/// The numbers a segment writes into the file, as the gate counts them: line 2, cubic 6,
/// arc 7.
fn written_count(seg: &Segment) -> f64 {
    match seg {
        Segment::Line(_) => 2.0,
        Segment::Cubic(..) => 6.0,
        Segment::Arc { .. } => 7.0,
    }
}

/// A cubic, or a circular arc. An elliptical arc ends a run like a line does.
fn is_curved(seg: &Segment) -> bool {
    match seg {
        Segment::Line(_) => false,
        Segment::Cubic(..) => true,
        Segment::Arc { .. } => seg.is_circular(),
    }
}

/// [`break_cost`] of a turn given as an angle (radians).
fn break_cost_of(turn: f64, lambda: f64) -> f64 {
    if !turn.is_finite() {
        return 0.0;
    }
    let (s, c) = turn.sin_cos();
    break_cost(Vec2 { x: 1.0, y: 0.0 }, Vec2 { x: c, y: s }, lambda)
}

/// The wobble penalty a cubic carries in the dynamic program's price; 0 for anything else.
fn wobble(p0: Point, seg: &Segment, lambda: f64) -> f64 {
    match *seg {
        Segment::Cubic(p1, p2, p3) => Cubic { p0, p1, p2, p3 }.wobble_penalty(lambda),
        _ => 0.0,
    }
}

/// The component of `v` along `axis`.
fn comp(axis: Axis, v: Vec2) -> f64 {
    match axis {
        Axis::X => v.x,
        Axis::Y => v.y,
    }
}

/// The direction an extremum of `axis` is traversed in, given a direction `d` near it: a
/// vertical unit vector at an x extremum, a horizontal one at a y extremum.
fn axis_dir(axis: Axis, d: Vec2) -> Vec2 {
    let sign = |v: f64| if v >= 0.0 { 1.0 } else { -1.0 };
    match axis {
        Axis::X => Vec2 {
            x: 0.0,
            y: sign(d.y),
        },
        Axis::Y => Vec2 {
            x: sign(d.x),
            y: 0.0,
        },
    }
}

/// Unit direction of travel at parameter `t` of a segment starting at `start`. A cubic
/// whose arm has collapsed takes the next leg of its control polygon.
fn tangent_at(seg: &Segment, start: Point, t: f64) -> Option<Vec2> {
    match *seg {
        Segment::Line(e) => unit(e - start),
        Segment::Cubic(c1, c2, e) => {
            let mt = 1.0 - t;
            let (a, b, c) = (c1 - start, c2 - c1, e - c2);
            let w = (mt * mt, 2.0 * mt * t, t * t);
            unit(Vec2 {
                x: w.0 * a.x + w.1 * b.x + w.2 * c.x,
                y: w.0 * a.y + w.1 * b.y + w.2 * c.y,
            })
            .or_else(|| {
                if t < 0.5 {
                    unit(c2 - start).or_else(|| unit(e - start))
                } else {
                    unit(e - c1).or_else(|| unit(e - start))
                }
            })
        }
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            let f = arc_ellipse_center(start, rx, ry, phi, large_arc, sweep, end);
            if f.delta == 0.0 {
                return unit(end - start);
            }
            let th = f.theta1 + f.delta * t;
            let (sp, cp) = f.phi.sin_cos();
            let (dx, dy) = (-f.rx * th.sin(), f.ry * th.cos());
            let sgn = f.delta.signum();
            unit(Vec2 {
                x: sgn * (cp * dx - sp * dy),
                y: sgn * (sp * dx + cp * dy),
            })
        }
    }
}

/// Parameters strictly inside a segment where one coordinate of its tangent vanishes, in
/// order, with the coordinate that is extreme there. A cubic's derivative is a quadratic
/// per coordinate; a circle's tangent is vertical at angles `kπ` and horizontal at
/// `π/2 + kπ`. Lines and elliptical arcs have none here.
fn interior_extrema(seg: &Segment, start: Point) -> Vec<(f64, Axis)> {
    const EPS: f64 = 1e-6;
    let mut out = Vec::new();
    match *seg {
        Segment::Line(_) => {}
        Segment::Cubic(c1, c2, e) => {
            for axis in [Axis::X, Axis::Y] {
                let (a, b, c) = (
                    comp(axis, c1 - start),
                    comp(axis, c2 - c1),
                    comp(axis, e - c2),
                );
                for r in quadratic_roots(a - 2.0 * b + c, 2.0 * (b - a), a) {
                    if r > EPS && r < 1.0 - EPS {
                        out.push((r, axis));
                    }
                }
            }
        }
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            if !seg.is_circular() {
                return out;
            }
            let f = arc_ellipse_center(start, rx, ry, phi, large_arc, sweep, end);
            if f.delta.abs() < 1e-12 {
                return out;
            }
            let quarter = std::f64::consts::FRAC_PI_2;
            let (lo, hi) = if f.delta > 0.0 {
                (f.theta1, f.theta1 + f.delta)
            } else {
                (f.theta1 + f.delta, f.theta1)
            };
            let mut k = (lo / quarter).ceil() as i64;
            while (k as f64) * quarter < hi {
                let t = (k as f64 * quarter - f.theta1) / f.delta;
                if t > EPS && t < 1.0 - EPS {
                    out.push((
                        t,
                        if k.rem_euclid(2) == 0 {
                            Axis::X
                        } else {
                            Axis::Y
                        },
                    ));
                }
                k += 1;
            }
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Real roots of `a·t² + b·t + c`, by the numerically stable pair of formulas; a vanishing
/// leading coefficient leaves the linear root.
fn quadratic_roots(a: f64, b: f64, c: f64) -> Vec<f64> {
    let scale = a.abs().max(b.abs()).max(c.abs()).max(1e-300);
    if a.abs() <= 1e-12 * scale {
        return if b.abs() > 1e-12 * scale {
            vec![-c / b]
        } else {
            Vec::new()
        };
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return Vec::new();
    }
    let q = -0.5 * (b + b.signum() * disc.sqrt());
    if q == 0.0 {
        vec![-b / (2.0 * a)]
    } else {
        vec![q / a, c / q]
    }
}

/// Approximate arc length of `seg` from `t = 0` to `t`, px (16 chords).
fn length_to(seg: &Segment, start: Point, t: f64) -> f64 {
    let mut acc = 0.0;
    let mut prev = start;
    for k in 1..=16 {
        let p = eval_segment(seg, start, t * k as f64 / 16.0);
        acc += prev.dist(p);
        prev = p;
    }
    acc
}

/// Squared distance (px²) from `p` to one segment: exact for a line, by kurbo's
/// nearest-point solver for a cubic, and through the centre for a circular arc.
fn dist2_to(seg: &Segment, start: Point, p: Point) -> f64 {
    let sq = |v: Vec2| v.dot(v);
    match *seg {
        Segment::Line(e) => {
            let d = e - start;
            let l2 = d.dot(d);
            if l2 <= 1e-24 {
                return sq(p - start);
            }
            let t = ((p - start).dot(d) / l2).clamp(0.0, 1.0);
            sq(p - Point::new(start.x + d.x * t, start.y + d.y * t))
        }
        Segment::Cubic(c1, c2, e) => {
            let k = |q: Point| KPoint::new(q.x, q.y);
            CubicBez::new(k(start), k(c1), k(c2), k(e))
                .nearest(k(p), 1e-7)
                .distance_sq
        }
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            let ends = sq(p - start).min(sq(p - end));
            let f = arc_ellipse_center(start, rx, ry, phi, large_arc, sweep, end);
            if f.delta == 0.0 || !seg.is_circular() {
                // Not reached from a run (an elliptical arc ends one); sampled.
                return (1..64)
                    .map(|k| sq(p - eval_segment(seg, start, k as f64 / 64.0)))
                    .fold(ends, f64::min);
            }
            let v = p - f.c;
            let rel = (v.y.atan2(v.x) - f.theta1).rem_euclid(std::f64::consts::TAU);
            let inside = if f.delta >= 0.0 {
                rel <= f.delta
            } else {
                rel == 0.0 || rel >= std::f64::consts::TAU + f.delta
            };
            if inside {
                let d = v.norm() - f.rx;
                d * d
            } else {
                ends
            }
        }
    }
}

#[cfg(test)]
mod tests;
