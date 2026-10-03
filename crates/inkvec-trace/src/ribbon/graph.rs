//! The stroke graph of one face: which centrelines it is drawn with, where they meet and
//! where they end, as polylines ready for the curve fitter.
//!
//! The order of the passes:
//!
//! 1. **Skeleton** ([`skeleton`]). Zhang-Suen thinning of the face's pixel mask, reduced to
//!    a graph of branches between nodes (degree 1: a free end; degree 3 or more: a
//!    junction), with the existing [`crate::centerline`] machinery. The skeleton is only
//!    trusted for *topology*: which pixels are joined to which.
//! 2. **Cross-sections** ([`cross_section`]). Every skeleton pixel is replaced by a centre
//!    sample read off the solved boundary: the nearest boundary point, then a walk along
//!    its inward normal to the other side ([`Boundary::across`]). The midpoint is the
//!    centre and the walk's length a width. A sample is *reliable* when the far side is
//!    anti-parallel and the width agrees with the face's paired width; that is a sleeve.
//!    Near a junction or a cap the cross-section is not a pair of parallel sides and the
//!    sample is unreliable.
//! 3. **Cores** ([`cores`]). Each branch keeps its reliable samples, with a corner rebuilt
//!    wherever an interior gap turns sharply ([`core_samples`]). A branch with almost no
//!    core between two junctions is a thinning artefact of one thick junction (its two
//!    nodes are merged); one with a free end and no core is a spur (dropped)
//!    ([`classify`]).
//! 4. **Ends** ([`ends`]). A junction is rebuilt from the sleeves that reach it: the
//!    point nearest, in least squares, to the lines the sleeve ends point along. A free
//!    end is extended along its sleeve to the centre of its round cap, half a width short
//!    of the boundary's farthest point.
//! 5. **Continuation** ([`continuation`]). At each junction, two sleeves that leave it in
//!    opposite directions continue each other (good continuation): they become one path
//!    through the junction point. Two sleeves alone at a node always do (a corner).
//! 6. **Chains** ([`assemble`]). Branches are walked port to port into open or closed
//!    polylines of centre samples with their sigmas.
//!
//! Method from: Zhang, Liu, Li, Wu, Wen (2022), Vectorizing line drawings of arbitrary
//! thickness via boundary-based topology reconstruction, Computer Graphics Forum 41,
//! doi:10.1111/cgf.14485: thin to a skeleton, mark skeleton points reliable or not from
//! the boundaries, keep the reliable ones and rebuild the unreliable ones (the junctions)
//! from the boundaries. Adapted: reliability here is the anti-parallel pairing test on
//! our own sub-pixel outline, and a junction is rebuilt as the least-squares meeting point
//! of the reliable sleeve ends rather than by their boundary curves' continuity, because
//! with round joins (lucide, openmoji) the meeting point is all the drawing needs.
//! Inspired by: Prasad (2005), Rectification of the chordal axis transform and a new
//! criterion for shape decomposition, DGCI, LNCS, doi:10.1007/978-3-540-31965-8_25, and
//! Prasad (2007), Image and Vision Computing 25, doi:10.1016/j.imavis.2006.06.025: the
//! decomposition of a shape into sleeves, junctions and terminals, with junction and
//! terminal pieces rebuilt from their sleeves. We get the sleeves from pairing instead of
//! from a constrained Delaunay triangulation, which this crate does not have.
//! Inspired by: Favreau, Lafarge, Bousseau (2016), Fidelity vs. simplicity: a global
//! approach to line drawing vectorization, ACM TOG 35(4), doi:10.1145/2897824.2925946:
//! curves continue through junctions (their hyperedges); ours pairs sleeves greedily by
//! opposite direction instead of their Metropolis-Hastings search, which is enough when
//! the joins are round and either reading renders the same pixels.
//! See also: Berio, Leymarie, Asente, Echevarria (2022), StrokeStyles: stroke-based
//! segmentation and stylization of fonts, ACM TOG 41(3), doi:10.1145/3505246: glyph
//! outlines to overlapping strokes from the medial axis and seven junction types; the
//! junction catalogue is richer than the two cases (continue / end) used here.

use inkvec_core::{Point, Vec2};

use super::boundary::{along, unit, Boundary};
use super::FaceMask;
use crate::centerline::skeleton::{trace_branches, zhang_suen, Branch, SkelGraph};

/// Sigma given to a rebuilt junction point, px. It is extrapolated from the sleeves on a
/// few pixels of straight line, so it is known to a few tenths of a pixel, not to the
/// boundary's hundredths.
const SIGMA_JUNCTION: f64 = 0.25;

/// Sigma given to a rebuilt cap centre, px: half a width back from the boundary's
/// farthest point along the sleeve, which the boundary solve placed to hundredths.
const SIGMA_CAP: f64 = 0.15;

/// Cosine below which two sleeve ends at one junction continue each other: their
/// directions into the junction differ from opposite by at most 40°.
const COS_CONTINUE: f64 = 0.766;

/// A centreline as measured: centre samples with their sigmas, open or closed.
pub(crate) struct Chain {
    /// Centre points in order, px.
    pub(crate) pts: Vec<Point>,
    /// Each point's sigma, px.
    pub(crate) sigma: Vec<f64>,
    /// The chain closes on its first point.
    pub(crate) closed: bool,
}

/// How [`centrelines`] reads the skeleton: the two rules that differ between the round
/// and the miter hypothesis of a face ([`super::fit_face`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct TopoOptions {
    /// Sigma of a corner rebuilt in an interior gap of samples, px; 0 rebuilds none
    /// ([`core_samples`]).
    pub(crate) corner_sigma: f64,
    /// A branch with one free end and less core than this, px, is a spur and dropped
    /// ([`classify`]); 0 drops only branches with no core at all.
    pub(crate) spur_len: f64,
}

impl TopoOptions {
    /// Round joins (lucide, openmoji): no corners -- a tight turn is an arc of about the
    /// half-width there, which a bridged gap lets the fitter bend -- and every branch with
    /// a core is kept.
    pub(crate) fn round() -> TopoOptions {
        TopoOptions {
            corner_sigma: 0.0,
            spur_len: 0.0,
        }
    }

    /// Round joins with rebuilt corners: the fallback reading when the cornerless one
    /// leaves the centrelines far from the boundary ([`super::fit_face`]); corners at
    /// the miter reading's sigma, every branch with a core kept.
    pub(crate) fn round_cornered() -> TopoOptions {
        TopoOptions {
            corner_sigma: 0.1,
            spur_len: 0.0,
        }
    }

    /// Miter joins: a sharp corner *is* a vertex, at the meeting point of the sleeves
    /// either side, and thinning puts a spur into every convex corner (up to about half
    /// a width long, carrying samples of the arms it sits between), which must go.
    pub(crate) fn miter(w: f64) -> TopoOptions {
        TopoOptions {
            corner_sigma: 0.1,
            spur_len: 0.5 * w,
        }
    }
}

/// What [`centrelines`] found.
pub(crate) struct Topology {
    /// The centrelines.
    pub(crate) chains: Vec<Chain>,
    /// Junctions rebuilt (clusters of skeleton nodes with two or more sleeves).
    pub(crate) junctions: usize,
    /// Free ends extended to a round cap's centre.
    pub(crate) caps: usize,
    /// Skeleton branches dropped as spurs, connectors or empty.
    pub(crate) dropped: usize,
}

/// One reliable centre sample.
#[derive(Clone, Copy)]
struct Sample {
    /// Centre, px.
    c: Point,
    /// Its sigma: half the root-sum-square of the two sides' sigmas, px.
    sigma: f64,
    /// The sleeve's direction there (unit, sign arbitrary): the side's tangent, which a
    /// stroke's sides share with its centreline.
    t: Vec2,
}

/// A skeleton branch reduced to its reliable core.
struct Core {
    /// Skeleton node at the branch's start (a `SkelGraph::pix` index).
    a: usize,
    /// Skeleton node at its end; equal to `a` for a cycle.
    b: usize,
    /// A pure cycle with no node on it.
    closed: bool,
    /// Reliable samples, start to end.
    s: Vec<Sample>,
    /// Polyline length of the samples, px.
    len: f64,
    /// Unit direction *into* node `a` at the start and into node `b` at the end.
    dir: [Vec2; 2],
}

/// The centrelines of a face with pixel `mask`, boundary `b` and paired stroke width `w`
/// px. See the module documentation for the passes.
///
/// Cost: the thinning is O(iterations x mask area) with about `w/2` iterations; each
/// skeleton pixel costs one nearest query and one ray walk in the boundary's grid.
pub(crate) fn centrelines(b: &Boundary, mask: &FaceMask, w: f64, opts: TopoOptions) -> Topology {
    let (g, branches) = skeleton(mask);
    let mut topo = Topology {
        chains: Vec::new(),
        junctions: 0,
        caps: 0,
        dropped: 0,
    };
    if g.pix.is_empty() {
        return topo;
    }
    let samples: Vec<Option<Sample>> = g
        .pix
        .iter()
        .map(|&p| cross_section(b, mask.point_of(p), w))
        .collect();
    let all = cores(&g, &branches, &samples, mask, w, opts.corner_sigma);
    let (kept, mut dsu) = classify(&g, all, w, mask, opts.spur_len, &mut topo);
    let (end_pt, end_sigma, link) = ends(b, mask, &g, &kept, &mut dsu, w, &mut topo);
    topo.chains = assemble(&kept, &end_pt, &end_sigma, &link);
    topo
}

/// The face mask thinned (Zhang-Suen) and split into branches between nodes.
fn skeleton(mask: &FaceMask) -> (SkelGraph, Vec<Branch>) {
    let skel = zhang_suen(&mask.on, mask.w, mask.h);
    let g = SkelGraph::build(&skel, mask.w, mask.h);
    let branches = trace_branches(&g);
    (g, branches)
}

/// The centre sample under skeleton point `s`, or `None` when the cross-section there is
/// not a stroke of width `w`.
///
/// `a` = the boundary point nearest `s`, with the inward normal `n` of its segment (the
/// averaged vertex normal when `a` is a segment end, so a walk from a corner does not pick
/// one side). Walking from `a` along `n` must meet an anti-parallel side after `t` px with
/// `|t - w| <= max(0.12·w, 0.35)`; the sample is then `c = a + n·t/2`, accepted only within
/// `0.6·w + 1` px of `s` (the skeleton pixel it stands for), with sigma
/// `sqrt(σ_a² + σ_b²)/2` from the two sides' interpolated sigmas.
fn cross_section(b: &Boundary, s: Point, w: f64) -> Option<Sample> {
    let near = b.grid.nearest(s, 1.5 * w + 2.0)?;
    let k = near.seg;
    let n = if near.u <= 1e-9 {
        b.pt_n[k]
    } else if near.u >= 1.0 - 1e-9 {
        b.pt_n[b.next[k]]
    } else {
        b.seg_n[k]
    };
    let (t, k2, u2) = b.across(near.p, n, k, 2.0 * w + 2.0)?;
    if (t - w).abs() > (0.12 * w).max(0.35) {
        return None;
    }
    let c = along(near.p, n, 0.5 * t);
    if c.dist(s) > 0.6 * w + 1.0 {
        return None;
    }
    let (sa, sb) = (b.sigma_at(k, near.u), b.sigma_at(k2, u2));
    Some(Sample {
        c,
        sigma: 0.5 * (sa * sa + sb * sb).sqrt(),
        t: Vec2 { x: n.y, y: -n.x },
    })
}

/// Every branch reduced to its reliable samples ([`core_samples`]), with its length and
/// end directions.
///
/// The direction into a node is taken over the last few samples (up to 4, about 4 px at
/// the skeleton's spacing) so pixel jitter in one sample does not swing it; a core of one
/// sample takes the skeleton chain's own direction.
fn cores(
    g: &SkelGraph,
    branches: &[Branch],
    samples: &[Option<Sample>],
    mask: &FaceMask,
    w: f64,
    corner_sigma: f64,
) -> Vec<Core> {
    branches
        .iter()
        .map(|br| {
            let s = core_samples(&br.chain, samples, mask, w, corner_sigma);
            let len: f64 = s.windows(2).map(|p| p[0].c.dist(p[1].c)).sum();
            let chain_dir = unit(
                mask.point_of(g.pix[br.chain[0]])
                    - mask.point_of(g.pix[br.chain[br.chain.len() - 1]]),
            )
            .unwrap_or(Vec2 { x: 1.0, y: 0.0 });
            let back_dir = Vec2 {
                x: -chain_dir.x,
                y: -chain_dir.y,
            };
            // Under three samples the core is too short to give a direction of its own
            // (a thick stroke's short arm: lucide `circle-arrow-right` at 512 px keeps
            // one sample per chevron arm, and the skeleton chain there points 30° off the
            // arm). The sleeve's side tangent at the end sample is used instead, signed
            // to agree with the skeleton chain.
            let along = |x: &Sample, want: Vec2| {
                if x.t.dot(want) >= 0.0 {
                    x.t
                } else {
                    Vec2 {
                        x: -x.t.x,
                        y: -x.t.y,
                    }
                }
            };
            let dir = if s.len() >= 3 {
                let back = (s.len() - 1).min(4);
                let m = s.len() - 1;
                [
                    unit(s[0].c - s[back].c).unwrap_or(chain_dir),
                    unit(s[m].c - s[m - back].c).unwrap_or(back_dir),
                ]
            } else if let (Some(first), Some(last)) = (s.first(), s.last()) {
                [along(first, chain_dir), along(last, back_dir)]
            } else {
                [chain_dir, back_dir]
            };
            Core {
                a: br.a,
                b: br.b,
                closed: br.closed,
                s,
                len,
                dir,
            }
        })
        .collect()
}

/// The reliable centre samples along one skeleton chain, with a rebuilt corner in every
/// interior gap where the stroke turns.
///
/// Samples closer than 0.05 px to the previous kept one are merged (two skeleton pixels
/// can share a foot point). A *gap* is two consecutive kept samples more than
/// `max(2, w/4)` px apart: the cross-sections between them were not a pair of parallel
/// sides. Inside a sleeve that happens where the centreline turns more tightly than the
/// half-width (its inner side then folds to a sharp notch), so when the runs either side
/// of the gap turn by more than 35° the corner is rebuilt as the meeting point of their
/// end lines ([`meeting_point`], each line through its run's last 5 samples), accepted
/// within `2·w + 2` px of both runs and on the face, with sigma [`SIGMA_JUNCTION`]. A
/// gentler gap is left for the fitter to bridge.
///
/// Inspired by: Zhang et al. (2022), cited in the module documentation -- unreliable
/// skeleton points rebuilt from the reliable ones around them.
fn core_samples(
    chain: &[usize],
    samples: &[Option<Sample>],
    mask: &FaceMask,
    w: f64,
    corner_sigma: f64,
) -> Vec<Sample> {
    let gap = (0.25 * w).max(2.0);
    // Runs of reliable samples, split at gaps.
    let mut runs: Vec<Vec<Sample>> = vec![Vec::new()];
    for &k in chain {
        let Some(x) = samples[k] else { continue };
        let run = runs.last_mut().expect("there is always a current run");
        match run.last() {
            Some(l) if l.c.dist(x.c) < 0.05 => {}
            Some(l) if l.c.dist(x.c) > gap => runs.push(vec![x]),
            _ => run.push(x),
        }
    }
    // Corners only under the miter hypothesis ([`TopoOptions`]). Measured for round joins
    // on the 40 lucide icons of the screen set at 128 px (2026-10-03): without corners
    // dE00 0.0355 at ratio 1.443; with them at sigma 0.25, 0.0410 at 1.407; at 1.0, 0.0406
    // at 1.407. Two icons gain (message-circle-dashed-check 0.091 -> 0.041) and eight lose
    // (bell 0.023 -> 0.067, save-pen 0.038 -> 0.087): most tight turns in lucide are arcs of
    // about the half-width, and a vertex there locks the fitter into a line-line corner the
    // stroke solve cannot round, where a bridged gap lets it bend a cubic.
    let mut out: Vec<Sample> = Vec::new();
    for (i, run) in runs.iter().enumerate() {
        if i > 0 && corner_sigma > 0.0 {
            if let Some(c) = corner(&out, run, mask, w) {
                out.push(Sample {
                    c,
                    sigma: corner_sigma,
                    t: unit(run[0].c - c).unwrap_or(run[0].t),
                });
            }
        }
        out.extend_from_slice(run);
    }
    out
}

/// The corner between the samples kept so far (`before`, ending at the gap) and the next
/// run (`after`, starting at it): see [`core_samples`]. `None` when either side has under
/// two samples, the turn is under 35°, or the lines' meeting point is implausible.
fn corner(before: &[Sample], after: &[Sample], mask: &FaceMask, w: f64) -> Option<Point> {
    if before.len() < 2 || after.len() < 2 {
        return None;
    }
    let (b0, b1) = (
        before[before.len() - 1].c,
        before[before.len().saturating_sub(5)].c,
    );
    let (a0, a1) = (after[0].c, after[(after.len() - 1).min(4)].c);
    // `da` leaves the run before into the gap, `db` leaves the run after into it.
    let (da, db) = (unit(b0 - b1)?, unit(a0 - a1)?);
    if da.dot(Vec2 { x: -db.x, y: -db.y }) > 0.819 {
        return None;
    }
    let c = meeting_point(&[(b0, da), (a0, db)])?;
    (mask.near(c) && c.dist(b0) <= 2.0 * w + 2.0 && c.dist(a0) <= 2.0 * w + 2.0).then_some(c)
}

/// Disjoint sets over skeleton nodes: the junction clusters.
struct Dsu(Vec<usize>);

impl Dsu {
    /// The representative of `x`'s set, compressing the path to it.
    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.0[r] != r {
            r = self.0[r];
        }
        let mut y = x;
        while self.0[y] != r {
            let nxt = self.0[y];
            self.0[y] = r;
            y = nxt;
        }
        r
    }
    /// Merge the sets of `a` and `b`.
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

/// Keep the branches that are sleeves; merge the nodes of junction connectors.
///
/// With `L` a branch's core length and `deg` the skeleton degree of its end nodes:
/// * a cycle is kept when its core covers at least two widths;
/// * both ends junctions (`deg >= 3`) and no core or `L < w/5`: a connector, the two
///   halves of one thick junction that thinning split -- its nodes join one cluster, the
///   branch goes;
/// * no samples at all: dropped (a spur from a cap or a corner, or a connector as above);
/// * one free end and less core than `spur_len` (the miter hypothesis only, see
///   [`TopoOptions`]): a corner spur, dropped;
/// * everything else is kept, however short.
///
/// The tests are on the *reliable core*, not on the skeleton branch: a thinning spur has
/// no parallel sides under it and so no core at all, while a real stub -- a bath's leg,
/// the end of a bar beyond the stroke it crosses -- has some, though the cap at one end
/// and the concave corners at the other leave only 1-2 px of it on lucide's `bath`
/// (5.3 px of leg). Length rules (`L < w/2`, then `L < 0.15·w`) dropped exactly those
/// stubs and folded what was left into one chain; a kept spur, should one have a core,
/// costs five parameters and is judged with everything else by the chi-squared.
fn classify(
    g: &SkelGraph,
    all: Vec<Core>,
    w: f64,
    mask: &FaceMask,
    spur_len: f64,
    topo: &mut Topology,
) -> (Vec<Core>, Dsu) {
    let mut dsu = Dsu((0..g.pix.len()).collect());
    let mut kept = Vec::new();
    for c in all {
        let (da, db) = (g.degree(c.a), g.degree(c.b));
        let keep = if c.closed {
            c.len >= 2.0 * w && c.s.len() >= 3
        } else if c.s.is_empty() {
            if da >= 3 && db >= 3 {
                dsu.union(c.a, c.b);
            }
            false
        } else if da >= 3 && db >= 3 && c.len < 0.2 * w && c.a != c.b {
            dsu.union(c.a, c.b);
            false
        } else {
            !((da == 1) != (db == 1) && c.len < spur_len)
        };
        if inkvec_core::env::flag("INKVEC_RIBBONS_BRANCHES") {
            let (pa, pb) = (mask.point_of(g.pix[c.a]), mask.point_of(g.pix[c.b]));
            eprintln!(
                "    branch ({:.0},{:.0}) deg {da} - ({:.0},{:.0}) deg {db} closed {} samples {} core {:.1} px: {}",
                pa.x,
                pa.y,
                pb.x,
                pb.y,
                c.closed,
                c.s.len(),
                c.len,
                if keep { "kept" } else { "dropped" }
            );
        }
        if keep {
            kept.push(c);
        } else {
            topo.dropped += 1;
        }
    }
    (kept, dsu)
}

/// Where every sleeve ends, and which sleeves continue each other.
///
/// Ports are numbered `2·branch + side` (side 0 at the core's first sample, node `a`;
/// side 1 at its last, node `b`). Returns per port its end point and that point's sigma,
/// and the port it continues into, if any. Ends at a free node (or at a junction no other
/// sleeve reaches) get a cap centre ([`cap_centre`]); ends meeting at a junction cluster
/// get the cluster's meeting point ([`meeting_point`]) and are paired by
/// [`continuation`].
#[allow(clippy::type_complexity)]
fn ends(
    b: &Boundary,
    mask: &FaceMask,
    g: &SkelGraph,
    kept: &[Core],
    dsu: &mut Dsu,
    w: f64,
    topo: &mut Topology,
) -> (Vec<Point>, Vec<f64>, Vec<Option<usize>>) {
    let n_ports = 2 * kept.len();
    let mut end_pt = vec![Point::new(0.0, 0.0); n_ports];
    let mut end_sigma = vec![SIGMA_CAP; n_ports];
    let mut link: Vec<Option<usize>> = vec![None; n_ports];
    // Ports by junction cluster.
    let mut clusters: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (bi, c) in kept.iter().enumerate() {
        if c.closed {
            continue;
        }
        for (side, node) in [(0usize, c.a), (1usize, c.b)] {
            let port = 2 * bi + side;
            if g.degree(node) >= 3 {
                clusters.entry(dsu.find(node)).or_default().push(port);
            } else {
                end_pt[port] = cap_centre(b, port_point(kept, port), port_dir(kept, port), w);
                topo.caps += 1;
            }
        }
    }
    for (root, ports) in clusters {
        if ports.len() == 1 {
            let p = ports[0];
            end_pt[p] = cap_centre(b, port_point(kept, p), port_dir(kept, p), w);
            topo.caps += 1;
            continue;
        }
        let lines: Vec<(Point, Vec2)> = ports
            .iter()
            .map(|&p| (port_point(kept, p), port_dir(kept, p)))
            .collect();
        let fallback = mask.point_of(g.pix[root]);
        let j = meeting_point(&lines)
            .filter(|&j| mask.near(j) && lines.iter().all(|(p, _)| p.dist(j) <= 3.0 * w + 2.0))
            .unwrap_or(fallback);
        for &p in &ports {
            end_pt[p] = j;
            end_sigma[p] = SIGMA_JUNCTION;
        }
        topo.junctions += 1;
        for (p, q) in continuation(&ports, &lines) {
            link[p] = Some(q);
            link[q] = Some(p);
        }
    }
    (end_pt, end_sigma, link)
}

/// The core sample at `port`'s end.
fn port_point(kept: &[Core], port: usize) -> Point {
    let c = &kept[port / 2];
    if port & 1 == 0 {
        c.s[0].c
    } else {
        c.s[c.s.len() - 1].c
    }
}

/// The unit direction of `port`'s sleeve, pointing out of its core into the node.
fn port_dir(kept: &[Core], port: usize) -> Vec2 {
    kept[port / 2].dir[port & 1]
}

/// The point nearest, in least squares, to every line `p + s·d` in `lines`, or `None`
/// when the lines are too close to parallel to meet (or there are none).
///
/// Minimises `Σ_i |(I - d_i d_iᵀ)(J - p_i)|²`, the summed squared perpendicular distance
/// from `J` to each line; the normal equations are `A J = c` with
/// `A = Σ (I - d_i d_iᵀ)` and `c = Σ (I - d_i d_iᵀ) p_i`. For two lines at angle θ,
/// `det A = sin²θ`; below `1e-3·trace(A)²` (about 3.6° for two lines) the meeting point
/// is ill-posed and the caller falls back. Two collinear sleeves (a straight run split by
/// a spur) take the midpoint of their ends instead, which is where they meet.
fn meeting_point(lines: &[(Point, Vec2)]) -> Option<Point> {
    let (mut a11, mut a12, mut a22, mut c1, mut c2) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for &(p, d) in lines {
        let (m11, m12, m22) = (1.0 - d.x * d.x, -d.x * d.y, 1.0 - d.y * d.y);
        a11 += m11;
        a12 += m12;
        a22 += m22;
        c1 += m11 * p.x + m12 * p.y;
        c2 += m12 * p.x + m22 * p.y;
    }
    let det = a11 * a22 - a12 * a12;
    let tr = a11 + a22;
    if lines.len() >= 2 && det > 1e-3 * tr * tr {
        return Some(Point::new(
            (a22 * c1 - a12 * c2) / det,
            (a11 * c2 - a12 * c1) / det,
        ));
    }
    if lines.len() == 2 {
        let (p, q) = (lines[0].0, lines[1].0);
        return Some(Point::new(0.5 * (p.x + q.x), 0.5 * (p.y + q.y)));
    }
    None
}

/// The centre of the round cap that ends a sleeve whose last centre sample is `p`,
/// heading along `d`.
///
/// A round cap of half-width `h = w/2` around end point `E` reaches farthest along `d` at
/// `E + h·d`. So `E = p + d·max(0, s_max - h)`, where `s_max` is the largest `(q - p)·d`
/// over the boundary points `q` of the cap: ahead of `p` (`0 < s <= 3·w + 2`; a thick
/// stroke's short arm can keep its only reliable sample far from its tip), within
/// `h + 0.75` px of the sleeve's axis, and facing back along the sleeve (inward normal
/// with `n·d < 0.2`), which keeps a neighbouring stroke's facing side out.
fn cap_centre(b: &Boundary, p: Point, d: Vec2, w: f64) -> Point {
    let h = 0.5 * w;
    let mut s_max = 0.0f64;
    for (i, &q) in b.pts.iter().enumerate() {
        let v = q - p;
        let s = v.dot(d);
        if s <= 0.0 || s > 3.0 * w + 2.0 || v.cross(d).abs() > h + 0.75 || b.pt_n[i].dot(d) >= 0.2 {
            continue;
        }
        s_max = s_max.max(s);
    }
    along(p, d, (s_max - h).max(0.0))
}

/// Which sleeve ends at one junction continue each other: pairs of ports.
///
/// Two ends alone at a junction always continue (a corner: one path with a round join
/// draws the same pixels as two paths with round caps, and is one move-to cheaper).
/// With three or more, a branch whose *both* ends are here is a loop and closes on itself
/// first -- a ring with a tail (lucide's `git-compare`) is then a closed centreline the
/// primitive search can call a circle, three numbers, instead of one open chain that
/// runs round the ring and out along the tail. The remaining ends are paired greedily,
/// most nearly opposite first, while their directions `d_i·d_j < -cos 40°`
/// ([`COS_CONTINUE`]); the rest end at the junction.
fn continuation(ports: &[usize], lines: &[(Point, Vec2)]) -> Vec<(usize, usize)> {
    if ports.len() == 2 {
        return vec![(ports[0], ports[1])];
    }
    let mut used = vec![false; ports.len()];
    let mut out = Vec::new();
    for i in 0..ports.len() {
        for j in i + 1..ports.len() {
            if !used[i] && !used[j] && ports[i] / 2 == ports[j] / 2 {
                used[i] = true;
                used[j] = true;
                out.push((ports[i], ports[j]));
            }
        }
    }
    let mut cand: Vec<(f64, usize, usize)> = Vec::new();
    for i in 0..ports.len() {
        for j in i + 1..ports.len() {
            let c = lines[i].1.dot(lines[j].1);
            if c < -COS_CONTINUE {
                cand.push((c, i, j));
            }
        }
    }
    cand.sort_by(|x, y| x.0.total_cmp(&y.0));
    for (_, i, j) in cand {
        if !used[i] && !used[j] {
            used[i] = true;
            used[j] = true;
            out.push((ports[i], ports[j]));
        }
    }
    out
}

/// Walk the kept branches port to port into chains.
///
/// Open chains start at every port that continues nowhere; whatever is left after them
/// is a cycle of continued branches, or a skeleton cycle, and is walked as a closed
/// chain. Traversing branch `k` entered at side `s` appends its entry end point, its core
/// samples in that direction, and its exit end point; a point within 1e-9 px of the
/// previous one (the shared junction point of two continued branches) is not repeated,
/// and a closed chain does not repeat its first point at the end.
fn assemble(
    kept: &[Core],
    end_pt: &[Point],
    end_sigma: &[f64],
    link: &[Option<usize>],
) -> Vec<Chain> {
    let nb = kept.len();
    let mut used = vec![false; nb];
    let mut chains = Vec::new();
    let push = |pts: &mut Vec<Point>, sig: &mut Vec<f64>, p: Point, s: f64| {
        if pts.last().is_none_or(|l: &Point| l.dist(p) > 1e-9) {
            pts.push(p);
            sig.push(s);
        }
    };
    let walk = |start: usize, used: &mut Vec<bool>| -> Chain {
        let (mut pts, mut sig) = (Vec::new(), Vec::new());
        let mut port = start;
        let mut closed = kept[start / 2].closed;
        loop {
            let k = port / 2;
            used[k] = true;
            let c = &kept[k];
            if !c.closed {
                push(&mut pts, &mut sig, end_pt[port], end_sigma[port]);
            }
            let forward = port & 1 == 0;
            for i in 0..c.s.len() {
                let x = if forward {
                    c.s[i]
                } else {
                    c.s[c.s.len() - 1 - i]
                };
                push(&mut pts, &mut sig, x.c, x.sigma);
            }
            if c.closed {
                break;
            }
            let exit = port ^ 1;
            push(&mut pts, &mut sig, end_pt[exit], end_sigma[exit]);
            match link[exit] {
                Some(q) if q == start => {
                    closed = true;
                    break;
                }
                Some(q) if !used[q / 2] => port = q,
                _ => break,
            }
        }
        if closed && pts.len() > 2 && pts[0].dist(pts[pts.len() - 1]) <= 1e-9 {
            pts.pop();
            sig.pop();
        }
        Chain {
            pts,
            sigma: sig,
            closed,
        }
    };
    for port in 0..2 * nb {
        if !used[port / 2] && !kept[port / 2].closed && link[port].is_none() {
            chains.push(walk(port, &mut used));
        }
    }
    for k in 0..nb {
        if !used[k] {
            let mut ch = walk(2 * k, &mut used);
            ch.closed = true;
            chains.push(ch);
        }
    }
    chains
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_point_of_a_tee_is_on_the_bar_axis() {
        // Bar halves along x meeting at the origin, stem coming down from +y.
        let lines = [
            (Point::new(-6.0, 0.1), Vec2 { x: 1.0, y: 0.0 }),
            (Point::new(6.0, -0.1), Vec2 { x: -1.0, y: 0.0 }),
            (Point::new(0.0, 7.0), Vec2 { x: 0.0, y: -1.0 }),
        ];
        let j = meeting_point(&lines).expect("three lines meet");
        assert!(j.x.abs() < 1e-9 && j.y.abs() < 0.1, "{j:?}");
        // Ports 0, 2 and 4: three different branches (port / 2).
        let pairs = continuation(&[0, 2, 4], &lines);
        assert_eq!(pairs, vec![(0, 2)], "the bar halves continue each other");
        // A loop's two ends at the junction close on each other before anything continues.
        let pairs = continuation(&[0, 1, 4], &lines);
        assert_eq!(pairs, vec![(0, 1)], "a loop closes on itself");
    }

    #[test]
    fn collinear_pair_meets_at_the_midpoint_and_parallel_triple_does_not() {
        let lines = [
            (Point::new(-1.0, 0.0), Vec2 { x: 1.0, y: 0.0 }),
            (Point::new(1.0, 0.0), Vec2 { x: -1.0, y: 0.0 }),
        ];
        let j = meeting_point(&lines).expect("a midpoint");
        assert!(j.x.abs() < 1e-12 && j.y.abs() < 1e-12);
        let three = [
            lines[0],
            lines[1],
            (Point::new(0.0, 3.0), Vec2 { x: 1.0, y: 0.0 }),
        ];
        assert!(meeting_point(&three).is_none());
        assert!(meeting_point(&[]).is_none());
    }

    #[test]
    fn dsu_merges_and_finds() {
        let mut d = Dsu((0..5).collect());
        d.union(3, 1);
        d.union(4, 3);
        assert_eq!(d.find(4), d.find(1));
        assert_ne!(d.find(0), d.find(1));
    }
}
