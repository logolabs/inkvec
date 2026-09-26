//! Editability mode (`--editability`): post-fit passes that spend parameters on
//! structure an artist can edit — G1-smooth joins, axis-aligned and equal-length
//! handles, aligned nodes, and self-symmetric rings locked into exact mirrors.
//!
//! Every change is accepted the same way: the whole ring is re-sampled and the
//! change kept only while every sample sits within `3×` the source point's own
//! sigma plus a small absolute slack (the deviation artists' own handles carry,
//! measured as the GT lift). Fidelity is never traded beyond that budget; structure
//! is gained, and parameters move either direction (mirror lock and G1 usually
//! *reduce* them; handle snapping can add a little). Acceptance is greedy and
//! per-item: one handle whose snap busts the budget does not veto its neighbours.
//!
//! The targets are measured, not assumed: the axis spike (29% of artist handles sit
//! within 0.05° of an axis), the 45° bump (5x lift) and the dead 10–30° trough come
//! from the committed artist GT corpus (313k handle angles, scratchpad/angle_prior.py);
//! the G1 and handle-length lifts from scratchpad/artist_priors.py.

use inkvec_core::Point;
use inkvec_fit::curves::Segment;
use inkvec_fit::primitives::PrimitiveFit;
use inkvec_fit::structural::eval_segment;
use inkvec_fit::FittedPath;

/// A handle direction is "on" an axis (0/90/180/270) within this many degrees.
const AXIS_SNAP: f64 = 2.5;
/// A handle direction is "on" the 45 family within this many degrees (5x measured
/// lift, broader than the axis spike).
const DIAG_SNAP: f64 = 1.5;
/// A join whose in/out tangents disagree by at most this many degrees is G1-smooth.
const G1_SNAP_DEG: f64 = 3.0;
/// Handle length ratio inside this window symmetrises to the mean.
const LEN_MATCH: f64 = 0.10;
/// Node coordinates within this distance of a shared target align to it.
const NODE_ALIGN: f64 = 0.30;
/// Self-mirror detection: share of reflected source points that must find a partner.
const MIRROR_KEEP: f64 = 0.95;
/// Absolute deviation (px at trace size) the mode carries on top of the input's own
/// noise floor: the GT lift is measured on files whose handles sit this far from the
/// pixel-optimal fit.
const EDIT_SLACK: f64 = 0.5;
/// Mirror lock moves geometry by the asymmetry between the two traced halves --
/// usually 1-3 px on icon-size art. Its own budget: asymmetry beyond this is real
/// content, not tracing noise, and the ring stays as drawn.
const MIRROR_SLACK: f64 = 1.5;
/// The emitter's coordinate step (it writes two decimals). The mirror axis snaps to
/// it so that reflected coordinates round to each other's reflections -- and so that
/// a node *on* the axis writes the axis exactly. Snapping to half a step is not
/// enough: `2·cx` lands on the grid either way, but the pole itself then rounds off
/// the axis and the whole shape emits 0.01 px out of true. The axis moves by at most
/// half a step, which no renderer resolves.
const EMIT_GRID: f64 = 0.01;
/// How close to the axis a node counts as sitting on it. Wider than the emitter's
/// step so a node the fit put a fraction off the axis is used as the crossing
/// rather than split beside it.
const AXIS_NODE: f64 = 1.0;

#[derive(Default, Debug, Clone)]
pub(crate) struct EditStats {
    pub(crate) rings: usize,
    pub(crate) g1_joins: usize,
    pub(crate) axis_snaps: usize,
    pub(crate) diag_snaps: usize,
    pub(crate) len_sym: usize,
    pub(crate) nodes_aligned: usize,
    pub(crate) mirror_rings: usize,
    pub(crate) reverted: usize,
}

impl EditStats {
    pub(crate) fn summary(&self) -> String {
        format!(
            "  editability   {} rings: {} G1 joins, {} axis + {} diagonal handle snaps, \
{} length-symmetric curves, {} nodes aligned, {} mirror-locked, {} reverted",
            self.rings,
            self.g1_joins,
            self.axis_snaps,
            self.diag_snaps,
            self.len_sym,
            self.nodes_aligned,
            self.mirror_rings,
            self.reverted
        )
    }
}

/// `starts[i]` is where segment i begins; segment i is `Cubic(c1, c2, starts[i+1])`.
fn chain_starts(fp: &FittedPath) -> Vec<Point> {
    let mut starts = Vec::with_capacity(fp.segments.len() + 1);
    starts.push(fp.start);
    for seg in &fp.segments {
        starts.push(seg.end());
    }
    starts
}

fn samples(segs: &[Segment], starts: &[Point], per: usize) -> Vec<Point> {
    let mut v = Vec::new();
    for (i, seg) in segs.iter().enumerate() {
        for k in 1..=per {
            v.push(eval_segment(seg, starts[i], k as f64 / per as f64));
        }
    }
    v
}

/// The changed geometry stays within the editability budget of the source points.
fn guarded(segs: &[Segment], starts: &[Point], pts: &[Point], sigma: &[f64]) -> bool {
    guarded_slack(segs, starts, pts, sigma, EDIT_SLACK)
}

fn guarded_slack(
    segs: &[Segment],
    starts: &[Point],
    pts: &[Point],
    sigma: &[f64],
    slack: f64,
) -> bool {
    samples(segs, starts, 8).iter().all(|p| {
        pts.iter()
            .enumerate()
            .any(|(j, q)| p.dist(*q) <= 3.0 * sigma[j.min(sigma.len() - 1)] + slack)
    })
}

/// Greedy per-item acceptance: apply one change, keep it if the whole ring still
/// guards, revert just that change otherwise.
fn keep_if_guarded(
    fp: &mut FittedPath,
    before: &[Segment],
    pts: &[Point],
    sigma: &[f64],
    count: &mut usize,
) -> bool {
    let starts = chain_starts(fp);
    if guarded(&fp.segments, &starts, pts, sigma) {
        *count += 1;
        true
    } else {
        fp.segments = before.to_vec();
        false
    }
}

fn cubic(seg: &Segment) -> Option<(Point, Point, Point)> {
    match *seg {
        Segment::Cubic(c1, c2, e) => Some((c1, c2, e)),
        _ => None,
    }
}

/// G1: joins whose tangents disagree by at most [`G1_SNAP_DEG`] rotate onto their
/// shared bisector, so an editor drags one tangent and the curve stays smooth.
fn pass_g1(
    fp: &mut FittedPath,
    starts: &[Point],
    pts: &[Point],
    sigma: &[f64],
    st: &mut EditStats,
) {
    let n = fp.segments.len();
    let mut count = 0;
    for i in 0..n {
        let j = (i + 1) % n;
        if !fp.closed && j == 0 {
            continue;
        }
        let (Some((c2, _, _)), Some((c1, _, _))) = (cubic(&fp.segments[i]), cubic(&fp.segments[j]))
        else {
            continue;
        };
        let node = starts[i + 1];
        let (dinx, diny) = (node.x - c2.x, node.y - c2.y);
        let (doux, douy) = (c1.x - starts[j].x, c1.y - starts[j].y);
        let (ln1, ln2) = (dinx.hypot(diny), doux.hypot(douy));
        if ln1 < 1e-9 || ln2 < 1e-9 {
            continue;
        }
        let cos = (dinx * doux + diny * douy) / (ln1 * ln2);
        let ang = cos.clamp(-1.0, 1.0).acos().to_degrees();
        if ang.abs() > G1_SNAP_DEG {
            continue;
        }
        let before = fp.segments.clone();
        let (mut bx, mut by) = (dinx / ln1 + doux / ln2, diny / ln1 + douy / ln2);
        let bn = bx.hypot(by);
        if bn < 1e-9 {
            continue;
        }
        bx /= bn;
        by /= bn;
        if bx * doux + by * douy < 0.0 {
            bx = -bx;
            by = -by;
        }
        if let Segment::Cubic(c2m, _, endm) = &mut fp.segments[i] {
            let l = (endm.x - c2m.x).hypot(endm.y - c2m.y);
            *c2m = Point::new(endm.x - bx * l, endm.y - by * l);
        }
        if let Segment::Cubic(c1m, _, _) = &mut fp.segments[j] {
            let l = (c1m.x - starts[j].x).hypot(c1m.y - starts[j].y);
            *c1m = Point::new(starts[j].x + bx * l, starts[j].y + by * l);
        }
        if !keep_if_guarded(fp, &before, pts, sigma, &mut count) {
            st.reverted += 1;
        }
    }
    st.g1_joins += count;
}

/// Axis and diagonal handle snaps, greedy per handle: each snap is applied alone
/// and kept only if the whole ring still guards, so one bad handle never vetoes
/// another and none ever escapes the budget.
fn pass_handles(
    fp: &mut FittedPath,
    starts: &[Point],
    pts: &[Point],
    sigma: &[f64],
    st: &mut EditStats,
) {
    let n = fp.segments.len();
    for i in 0..n {
        let end = starts[i + 1];
        let Some((c1, c2, _)) = cubic(&fp.segments[i]) else {
            continue;
        };
        let mut handles = [c1, c2];
        for (hi, base) in [(0usize, starts[i]), (1usize, end)] {
            let hx = handles[hi].x - base.x;
            let hy = handles[hi].y - base.y;
            let l = hx.hypot(hy);
            if l < 1e-9 {
                continue;
            }
            let deg = hy.atan2(hx).to_degrees();
            let rem = deg.rem_euclid(90.0);
            let dev_axis = rem.min(90.0 - rem);
            let dev_diag = (rem - 45.0).abs();
            let target = if dev_axis <= AXIS_SNAP {
                Some(deg - rem + if rem > 45.0 { 90.0 } else { 0.0 })
            } else if dev_diag <= DIAG_SNAP {
                Some(deg - rem + 45.0)
            } else {
                None
            };
            let Some(t) = target else { continue };
            if (t - deg).abs() <= 1e-9 {
                continue;
            }
            let axis = dev_axis <= AXIS_SNAP;
            let before = fp.segments.clone();
            let r = t.to_radians();
            handles[hi] = Point::new(base.x + l * r.cos(), base.y + l * r.sin());
            if let Segment::Cubic(a, b, _) = &mut fp.segments[i] {
                *a = handles[0];
                *b = handles[1];
            }
            let starts = chain_starts(fp);
            if guarded(&fp.segments, &starts, pts, sigma) {
                if axis {
                    st.axis_snaps += 1;
                } else {
                    st.diag_snaps += 1;
                }
            } else {
                handles[hi] = Point::new(base.x + hx, base.y + hy);
                if let Segment::Cubic(a, b, _) = &mut fp.segments[i] {
                    *a = handles[0];
                    *b = handles[1];
                }
                fp.segments = before;
                st.reverted += 1;
            }
        }
    }
}

/// Equal handle lengths within [`LEN_MATCH`]: the smooth-point tool's signature.
fn pass_equalize(
    fp: &mut FittedPath,
    starts: &[Point],
    pts: &[Point],
    sigma: &[f64],
    st: &mut EditStats,
) {
    let n = fp.segments.len();
    for i in 0..n {
        let end = starts[i + 1];
        let Some((c1, c2, _)) = cubic(&fp.segments[i]) else {
            continue;
        };
        let l1 = (c1.x - starts[i].x).hypot(c1.y - starts[i].y);
        let l2 = (end.x - c2.x).hypot(end.y - c2.y);
        if l1 < 1e-9 || l2 < 1e-9 {
            continue;
        }
        let r = l1.min(l2) / l1.max(l2);
        if !(1.0 - LEN_MATCH..=1.0).contains(&r) || (l1 - l2).abs() <= 1e-9 {
            continue;
        }
        let before = fp.segments.clone();
        let m = (l1 + l2) / 2.0;
        let nc1 = Point::new(
            starts[i].x + (c1.x - starts[i].x) / l1 * m,
            starts[i].y + (c1.y - starts[i].y) / l1 * m,
        );
        let nc2 = Point::new(
            end.x + (c2.x - end.x) / l2 * m,
            end.y + (c2.y - end.y) / l2 * m,
        );
        if let Segment::Cubic(a, b, _) = &mut fp.segments[i] {
            *a = nc1;
            *b = nc2;
        }
        let starts = chain_starts(fp);
        if guarded(&fp.segments, &starts, pts, sigma) {
            st.len_sym += 1;
        } else {
            fp.segments = before;
            st.reverted += 1;
        }
    }
}

/// The same curve reflected about `cx` and traversed backwards, so that it ends
/// where the original began. Reversing an arc flips its sweep and reflecting it
/// flips the sweep again, so the two cancel and only the rotation negates.
fn mirror_reverse(seg: &Segment, seg_start: Point, cx: f64) -> Segment {
    let mir = |p: Point| Point::new(2.0 * cx - p.x, p.y);
    match *seg {
        Segment::Cubic(c1, c2, _) => Segment::Cubic(mir(c2), mir(c1), mir(seg_start)),
        Segment::Line(_) => Segment::Line(mir(seg_start)),
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            ..
        } => Segment::Arc {
            rx,
            ry,
            phi: -phi,
            large_arc,
            sweep,
            end: mir(seg_start),
        },
    }
}

/// One segment cut in two at `t`, drawing exactly what it drew before. De
/// Casteljau for a cubic, a lerp for a line; an arc has no exact split here and
/// declines, so a ring whose axis crosses one is left alone.
fn split_at(seg: &Segment, start: Point, t: f64) -> Option<(Segment, Segment)> {
    let lerp = |p: Point, q: Point| Point::new(p.x + (q.x - p.x) * t, p.y + (q.y - p.y) * t);
    match *seg {
        Segment::Cubic(c1, c2, e) => {
            let (p01, p12, p23) = (lerp(start, c1), lerp(c1, c2), lerp(c2, e));
            let (p012, p123) = (lerp(p01, p12), lerp(p12, p23));
            let m = lerp(p012, p123);
            Some((Segment::Cubic(p01, p012, m), Segment::Cubic(p123, p23, e)))
        }
        Segment::Line(e) => Some((Segment::Line(lerp(start, e)), Segment::Line(e))),
        Segment::Arc { .. } => None,
    }
}

/// Where inside a segment the vertical line `x = cx` cuts it, if it does. Found by
/// bracketing a sign change and bisecting, which needs nothing of the segment but
/// the ability to evaluate it.
fn interior_crossing(seg: &Segment, start: Point, cx: f64) -> Option<f64> {
    const STEPS: usize = 32;
    let x = |t: f64| eval_segment(seg, start, t).x - cx;
    let f0 = x(0.0);
    if f0 == 0.0 {
        // the start node is already the crossing; the caller counts that case
        return None;
    }
    let (mut lo, mut flo) = (0.0_f64, f0);
    for k in 1..=STEPS {
        let hi = k as f64 / STEPS as f64;
        let fhi = x(hi);
        // `<=`, not `<`: a sample can land exactly on the axis, and a strict test
        // then walks straight past the crossing it is looking for. Axis-aligned
        // artwork makes that the common case, not the corner one -- a line crossing
        // at t = 1/2 was missed entirely.
        if flo * fhi <= 0.0 {
            let (mut a, mut b) = (lo, hi);
            for _ in 0..50 {
                let m = 0.5 * (a + b);
                if x(a) * x(m) <= 0.0 {
                    b = m;
                } else {
                    a = m;
                }
            }
            let t = 0.5 * (a + b);
            return (t > 1e-6 && t < 1.0 - 1e-6).then_some(t);
        }
        lo = hi;
        flo = fhi;
    }
    None
}

/// The ring with the axis crossings turned into nodes, splitting whichever segment
/// each one falls inside. Returns the segments, the chain of node positions and how
/// many splits it took; `None` when a crossing lands inside an arc, which has no
/// exact split here.
fn with_axis_nodes(
    fp: &FittedPath,
    starts: &[Point],
    cx: f64,
) -> Option<(Vec<Segment>, Vec<Point>, usize)> {
    let mut segs = fp.segments.clone();
    let mut chain = starts.to_vec();
    let on_axis = |p: Point| (p.x - cx).abs() <= AXIS_NODE;
    let mut splits: Vec<(usize, f64)> = Vec::new();
    for i in 0..segs.len() {
        if on_axis(chain[i]) || on_axis(chain[i + 1]) {
            continue; // an endpoint is already the crossing
        }
        if let Some(t) = interior_crossing(&segs[i], chain[i], cx) {
            splits.push((i, t));
        }
    }
    // Highest index first, so the earlier ones keep their positions.
    for &(i, t) in splits.iter().rev() {
        let (a, b) = split_at(&segs[i], chain[i], t)?;
        segs.splice(i..=i, [a, b]);
    }
    if !splits.is_empty() {
        chain = std::iter::once(fp.start)
            .chain(segs.iter().map(|s| s.end()))
            .collect();
    }
    Some((segs, chain, splits.len()))
}

/// The ring rebuilt from whichever of its two halves describes the traced points
/// better: that half kept as drawn, and the other replaced by its reflection.
/// Returns how far the result strays from those points, where it starts, and its
/// segments.
fn better_half(
    segs: &[Segment],
    chain: &[Point],
    poles: (usize, usize),
    cx: f64,
    pts: &[Point],
) -> Option<(f64, Point, Vec<Segment>)> {
    let (p, q) = poles;
    let n = segs.len();
    // A node the axis passes through is its own mirror, so put it exactly on the
    // axis. That is what lets the ring close: the reflected run ends on the mirror
    // of the first pole, which is the first pole itself only once it sits at `cx`.
    let mut nodes = chain.to_vec();
    nodes[p] = Point::new(cx, chain[p].y);
    nodes[q] = Point::new(cx, chain[q].y);

    let mut best: Option<(f64, Point, Vec<Segment>)> = None;
    for (from, to) in [(p, q), (q, p)] {
        // keep the segments from one pole to the other, then append that same run
        // reflected and reversed
        let mut keep = Vec::new();
        let mut k = from;
        while k != to {
            keep.push(k);
            k = (k + 1) % n;
        }
        let mut out: Vec<Segment> = keep.iter().map(|&k| segs[k].clone()).collect();
        if let Some(last) = out.last_mut() {
            set_end(last, nodes[to]); // the kept run finishes on the far pole
        }
        for &k in keep.iter().rev() {
            out.push(mirror_reverse(&segs[k], nodes[k], cx));
        }
        if out.len() < 2 {
            continue;
        }
        let start = nodes[from];
        let built: Vec<Point> = std::iter::once(start)
            .chain(out.iter().map(|s| s.end()))
            .collect();
        let stray = samples(&out, &built, 16)
            .iter()
            .map(|p| pts.iter().map(|q| p.dist(*q)).fold(f64::INFINITY, f64::min))
            .fold(0.0, f64::max);
        if best.as_ref().is_none_or(|(b, _, _)| stray < *b) {
            best = Some((stray, start, out));
        }
    }
    best
}

/// Mirror lock: a closed ring whose reflection about its own vertical centre axis
/// lands back on itself is rewritten so that it does so *exactly* -- one half is
/// kept and the other replaced by its reflection.
///
/// Replacing a half, rather than averaging the two, is what makes this work on
/// traced geometry. The fitter segments the two halves independently and rarely
/// gives them the same number of segments (5 against 6 on the test heart), so no
/// pairing between them exists to average: pairing by nearest mirrored midpoint
/// lets segment i pair with j while i+1 pairs with j-2, the shared endpoints the
/// two pairs compute then disagree, and the chain tears -- measured as a 21 px
/// stray on a ring that is symmetric to the pixel. Reflecting a whole half cannot
/// tear, because the seam sits on the two points where the axis meets the ring,
/// and each of those is its own mirror.
///
/// Those two points are rarely nodes. Measured over the corpus, every ring that
/// passed the symmetry test was then thrown away for want of them: 9 rings had one
/// node on the axis and 9 had none, and not one had two. A traced shape has no
/// reason to put a node where its axis happens to fall, so the crossing is found
/// inside whichever segment contains it and that segment is split there -- which
/// draws the same curve, and costs one node.
///
/// Both halves are tried and the one that strays less from the traced points wins,
/// so the sloppier half is the one that gets replaced.
fn pass_mirror(
    fp: &mut FittedPath,
    starts: &[Point],
    pts: &[Point],
    sigma: &[f64],
    st: &mut EditStats,
) {
    let geometrically_closed = fp
        .segments
        .last()
        .map(|s| s.end().dist(fp.start) <= 0.05)
        .unwrap_or(false);
    if (!fp.closed && !geometrically_closed) || fp.segments.len() < 4 {
        return;
    }
    let debug = inkvec_core::env::flag("INKVEC_EDIT_DEBUG");
    let xs: Vec<f64> = pts.iter().map(|p| p.x).collect();
    let cx_raw = (xs.iter().cloned().fold(f64::MAX, f64::min)
        + xs.iter().cloned().fold(f64::MIN, f64::max))
        / 2.0;
    // Put the axis on the grid the emitter writes, so that a mirrored pair rounds to
    // a mirrored pair and a node *on* the axis writes the axis exactly. Without it
    // an exactly-mirrored ring still emitted pairs 0.01 px apart.
    let cx = (cx_raw / EMIT_GRID).round() * EMIT_GRID;
    let mir = |p: Point| Point::new(2.0 * cx - p.x, p.y);

    // Is the drawing itself symmetric? Asked of the measured points, not of the
    // fit, so a ring is only locked when the evidence says it was drawn that way.
    let mut hit = 0;
    for p in pts {
        if pts
            .iter()
            .any(|q| mir(*p).dist(*q) <= 3.0 * sigma[0] + EDIT_SLACK)
        {
            hit += 1;
        }
    }
    if (hit as f64) < MIRROR_KEEP * pts.len() as f64 {
        return;
    }

    let Some((segs, chain, splits)) = with_axis_nodes(fp, starts, cx) else {
        if debug {
            eprintln!("    mirror: the axis crosses an arc -- declined");
        }
        return;
    };
    let n = segs.len();
    let poles: Vec<usize> = (0..n)
        .filter(|&k| (chain[k].x - cx).abs() <= AXIS_NODE)
        .collect();
    if poles.len() != 2 {
        if debug {
            eprintln!(
                "    mirror: {} points on the axis after {splits} split(s), want 2 -- declined",
                poles.len(),
            );
        }
        return;
    }

    let (p, q) = (poles[0], poles[1]);
    let Some((stray, start, out)) = better_half(&segs, &chain, (p, q), cx, pts) else {
        return;
    };
    if debug {
        eprintln!(
            "    mirror: {}/{} points reflect, {} split(s), poles {p},{q}, \
locked ring strays {stray:.3} px (budget {MIRROR_SLACK})",
            hit,
            pts.len(),
            splits
        );
    }

    // Judged against the mirror budget, not the handle one: locking moves geometry
    // by the asymmetry between the two traced halves, which is the whole point of
    // the pass and is larger than a handle snap's.
    let chain: Vec<Point> = std::iter::once(start)
        .chain(out.iter().map(|s| s.end()))
        .collect();
    if guarded_slack(&out, &chain, pts, sigma, MIRROR_SLACK) {
        st.mirror_rings += 1;
        fp.segments = out;
        fp.start = start;
    } else {
        st.reverted += 1;
    }
}

/// Move a segment's on-curve endpoint, whatever kind of segment it is.
fn set_end(seg: &mut Segment, p: Point) {
    match seg {
        Segment::Cubic(_, _, e) | Segment::Line(e) => *e = p,
        Segment::Arc { end, .. } => *end = p,
    }
}

/// Node alignment across the whole drawing: node coordinates within [`NODE_ALIGN`]
/// of a shared target move onto it, so that shapes an artist would expect to line
/// up actually do. 98% of the nodes in the artist corpus share an exact x or y with
/// another node, which is what this is reaching for.
///
/// A ring's on-curve nodes are its segment *ends*: `start`, then `segments[k].end()`.
/// There is no start field to write, and `Cubic`'s first field is a control point --
/// treating it as the node moved handles onto node positions and collapsed every
/// `Line` to zero length, which is how the heart lost its bottom point.
fn pass_nodes(
    fps: &mut [FittedPath],
    free: &[usize],
    polys: &[inkvec_core::Polyline],
    st: &mut EditStats,
) {
    #[derive(Clone, Copy)]
    struct NodeRef {
        fp: usize,
        node: usize, // 0 is the ring's start; k is the end of segment k-1
        x: f64,
        y: f64,
    }
    let mut nodes: Vec<NodeRef> = Vec::new();
    for &fi in free {
        let fp = &fps[fi];
        nodes.push(NodeRef {
            fp: fi,
            node: 0,
            x: fp.start.x,
            y: fp.start.y,
        });
        for (k, seg) in fp.segments.iter().enumerate() {
            // For a closed ring the last segment ends on node 0, which is already in.
            if fp.closed && k + 1 == fp.segments.len() {
                continue;
            }
            let e = seg.end();
            nodes.push(NodeRef {
                fp: fi,
                node: k + 1,
                x: e.x,
                y: e.y,
            });
        }
    }

    // Cluster each axis on its own: a node may share an x with one neighbour and a
    // y with another, and the two moves are independent.
    let mut moved: Vec<(usize, usize, f64, f64)> = Vec::new();
    for axis in 0..2 {
        let mut idx: Vec<usize> = (0..nodes.len()).collect();
        idx.sort_by(|&a, &b| {
            if axis == 0 {
                nodes[a].x.total_cmp(&nodes[b].x)
            } else {
                nodes[a].y.total_cmp(&nodes[b].y)
            }
        });
        let val = |nd: &NodeRef| if axis == 0 { nd.x } else { nd.y };
        let mut i = 0;
        while i < idx.len() {
            let mut j = i;
            while j + 1 < idx.len() && val(&nodes[idx[j + 1]]) - val(&nodes[idx[i]]) <= NODE_ALIGN {
                j += 1;
            }
            if j > i {
                let target = val(&nodes[idx[i + (j - i) / 2]]);
                for k in i..=j {
                    let nd = &mut nodes[idx[k]];
                    if (val(nd) - target).abs() > 1e-9 {
                        if axis == 0 {
                            nd.x = target;
                        } else {
                            nd.y = target;
                        }
                        moved.push((nd.fp, nd.node, nd.x, nd.y));
                    }
                }
            }
            i = j + 1;
        }
    }

    // Apply per ring, and hold each ring to the same budget as every other pass:
    // this one crosses ring borders, so it is the one most able to drag geometry
    // somewhere the evidence does not support.
    let mut by_ring: std::collections::HashMap<usize, Vec<(usize, f64, f64)>> =
        std::collections::HashMap::new();
    for (fi, node, x, y) in moved {
        by_ring.entry(fi).or_default().push((node, x, y));
    }
    for (fi, edits) in by_ring {
        let fp = &mut fps[fi];
        let before = fp.clone();
        let n = fp.segments.len();
        for (node, x, y) in &edits {
            let p = Point::new(*x, *y);
            if *node == 0 {
                fp.start = p;
                if fp.closed && n > 0 {
                    set_end(&mut fp.segments[n - 1], p);
                }
            } else if *node <= n {
                set_end(&mut fp.segments[*node - 1], p);
            }
        }
        let poly = &polys[fi];
        let starts = chain_starts(fp);
        if guarded(&fp.segments, &starts, &poly.points, &poly.sigma) {
            st.nodes_aligned += edits.len();
        } else {
            *fp = before;
            st.reverted += 1;
        }
    }
}

/// Apply every pass to every ring, in the order an artist would notice them:
/// mirror lock first (it rewrites halves wholesale), then G1, then the handle
/// snaps, then equal lengths, and node alignment last (it crosses ring borders).
/// Rings that are already a primitive element (circle/ellipse/rect) are finished
/// geometry — their nodes are load-bearing for the element and are left alone.
pub(crate) fn edit_all(
    polys: &[inkvec_core::Polyline],
    fps: &mut [FittedPath],
    prims: &[Option<PrimitiveFit>],
) -> EditStats {
    let mut st = EditStats {
        rings: fps.len(),
        ..Default::default()
    };
    let free: Vec<usize> = (0..fps.len()).filter(|&i| prims[i].is_none()).collect();
    // The passes that move on-curve NODES run first: mirror lock rewrites a whole
    // half, and node alignment slides nodes onto shared targets. Only then are the
    // handles pointed, because a handle's direction is measured from its node, and
    // moving the node afterwards turns the handle back off the axis it was snapped
    // to -- priced on the corpus, alignment last cost three fifths of what handle
    // snapping had just bought (5.0% of handles on an axis, down to 3.2%).
    for (fp, (poly, prim)) in fps.iter_mut().zip(polys.iter().zip(prims.iter())) {
        if prim.is_some() {
            continue;
        }
        if on("mirror") {
            pass_mirror(fp, &chain_starts(fp), &poly.points, &poly.sigma, &mut st);
        }
    }
    if on("nodes") {
        pass_nodes(fps, &free, polys, &mut st);
    }
    for (fp, (poly, prim)) in fps.iter_mut().zip(polys.iter().zip(prims.iter())) {
        if prim.is_some() {
            continue;
        }
        let sigma = &poly.sigma;
        // Each reads the chain the pass before it left: mirror lock can change how
        // many segments a ring has.
        if on("g1") {
            pass_g1(fp, &chain_starts(fp), &poly.points, sigma, &mut st);
        }
        if on("handles") {
            pass_handles(fp, &chain_starts(fp), &poly.points, sigma, &mut st);
        }
        if on("equalize") {
            pass_equalize(fp, &chain_starts(fp), &poly.points, sigma, &mut st);
        }
    }
    st
}

/// Whether a pass runs. In a `research` build `INKVEC_EDIT_PASSES` names the ones to keep,
/// comma separated, so each can be priced on its own against the corpus; unset (and every
/// release build) runs all of them, which is what the flag means.
fn on(pass: &str) -> bool {
    if !cfg!(feature = "research") {
        return true;
    }
    match inkvec_core::env::text("INKVEC_EDIT_PASSES") {
        Some(v) => v.split(',').any(|p| p.trim() == pass),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkvec_core::Polyline;

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    /// A ring sampled densely enough that the guard has real evidence to hold the
    /// passes to, with the sigma the tracer would give a clean edge.
    fn poly_of(fp: &FittedPath, per: usize) -> Polyline {
        let starts = chain_starts(fp);
        let pts = samples(&fp.segments, &starts, per);
        let n = pts.len();
        Polyline::new(pts, vec![0.25; n], true)
    }

    /// A diamond: four lines, symmetric about x = 50, with nodes on the axis.
    fn diamond() -> FittedPath {
        FittedPath {
            start: pt(50.0, 10.0),
            segments: vec![
                Segment::Line(pt(90.0, 50.0)),
                Segment::Line(pt(50.0, 90.0)),
                Segment::Line(pt(10.0, 50.0)),
                Segment::Line(pt(50.0, 10.0)),
            ],
            closed: true,
        }
    }

    #[test]
    fn reflecting_a_segment_twice_gives_it_back() {
        let seg = Segment::Cubic(pt(12.0, 3.0), pt(31.0, 47.0), pt(40.0, 60.0));
        let start = pt(5.0, 1.0);
        let once = mirror_reverse(&seg, start, 25.0);
        // the reflected copy runs backwards from the reflection of the original's
        // end, which is where its own start is
        let twice = mirror_reverse(&once, pt(2.0 * 25.0 - 40.0, 60.0), 25.0);
        let (Segment::Cubic(a1, a2, ae), Segment::Cubic(b1, b2, be)) = (&seg, &twice) else {
            panic!("expected cubics");
        };
        for (p, q) in [(a1, b1), (a2, b2), (ae, be)] {
            assert!(p.dist(*q) < 1e-9, "{p:?} != {q:?}");
        }
    }

    #[test]
    fn splitting_a_curve_draws_what_it_drew_before() {
        let seg = Segment::Cubic(pt(10.0, 0.0), pt(30.0, 80.0), pt(40.0, 40.0));
        let start = pt(0.0, 0.0);
        let (a, b) = split_at(&seg, start, 0.375).expect("a cubic splits");
        let mid = a.end();
        for k in 0..=40 {
            let t = f64::from(k) / 40.0;
            let whole = eval_segment(&seg, start, t);
            // the same parameter, expressed on whichever half holds it
            let part = if t <= 0.375 {
                eval_segment(&a, start, t / 0.375)
            } else {
                eval_segment(&b, mid, (t - 0.375) / 0.625)
            };
            assert!(whole.dist(part) < 1e-9, "t={t}: {whole:?} vs {part:?}");
        }
    }

    #[test]
    fn the_axis_crossing_is_found_inside_a_segment() {
        // a line from x=10 to x=90 crosses x=50 at its midpoint
        let seg = Segment::Line(pt(90.0, 7.0));
        let t = interior_crossing(&seg, pt(10.0, 7.0), 50.0).expect("it crosses");
        assert!((t - 0.5).abs() < 1e-6, "t = {t}");
        // and a segment that stays on one side has no crossing
        assert!(interior_crossing(&seg, pt(10.0, 7.0), 200.0).is_none());
    }

    #[test]
    fn a_symmetric_ring_is_locked_onto_its_axis_exactly() {
        let mut fp = diamond();
        // pull one side off true by more than the emitter's step, less than the budget
        fp.segments[0] = Segment::Line(pt(90.6, 50.3));
        let poly = poly_of(&fp, 24);
        let mut st = EditStats::default();
        let starts = chain_starts(&fp);
        pass_mirror(&mut fp, &starts, &poly.points, &poly.sigma, &mut st);
        assert_eq!(st.mirror_rings, 1, "the ring should lock: {st:?}");

        // every node now has a partner at its exact reflection
        let nodes = chain_starts(&fp);
        let xs: Vec<f64> = nodes.iter().map(|p| p.x).collect();
        let cx = (xs.iter().cloned().fold(f64::MAX, f64::min)
            + xs.iter().cloned().fold(f64::MIN, f64::max))
            / 2.0;
        for p in &nodes {
            let m = Point::new(2.0 * cx - p.x, p.y);
            let best = nodes
                .iter()
                .map(|q| m.dist(*q))
                .fold(f64::INFINITY, f64::min);
            assert!(best < 1e-9, "{p:?} has no exact mirror (nearest {best})");
        }
    }

    #[test]
    fn an_asymmetric_ring_is_left_alone() {
        let mut fp = diamond();
        fp.segments[0] = Segment::Line(pt(140.0, 50.0)); // a long spur on one side only
        let poly = poly_of(&fp, 24);
        let before = fp.clone();
        let mut st = EditStats::default();
        let starts = chain_starts(&fp);
        pass_mirror(&mut fp, &starts, &poly.points, &poly.sigma, &mut st);
        assert_eq!(st.mirror_rings, 0, "nothing should have locked");
        assert_eq!(fp.segments.len(), before.segments.len());
    }

    /// The nodes of a ring are its segment *ends*; `Cubic`'s first field is a
    /// control point. Writing node positions into it moved handles onto nodes and
    /// collapsed every `Line` to zero length -- which is how a heart lost its point.
    #[test]
    fn aligning_nodes_moves_nodes_and_not_handles() {
        let fp = FittedPath {
            start: pt(0.0, 0.0),
            segments: vec![
                Segment::Cubic(pt(10.0, 30.0), pt(30.0, 30.0), pt(40.0, 0.2)),
                Segment::Line(pt(40.0, 40.0)),
                Segment::Cubic(pt(20.0, 50.0), pt(5.0, 50.0), pt(0.0, 0.0)),
            ],
            closed: true,
        };
        let poly = poly_of(&fp, 32);
        let handles_before = fp
            .segments
            .iter()
            .filter_map(cubic)
            .map(|(c1, c2, _)| (c1, c2))
            .collect::<Vec<_>>();
        let mut fps = [fp.clone()];
        pass_nodes(
            &mut fps,
            &[0],
            std::slice::from_ref(&poly),
            &mut EditStats::default(),
        );
        let out = &fps[0];
        // the y = 0.2 node may align to y = 0; no line may collapse
        for (k, seg) in out.segments.iter().enumerate() {
            let starts = chain_starts(out);
            assert!(
                starts[k].dist(seg.end()) > 1e-6,
                "segment {k} collapsed to a point"
            );
        }
        // handles keep their own positions, give or take the node they hang from
        for (k, (c1, c2)) in out
            .segments
            .iter()
            .filter_map(cubic)
            .map(|(a, b, _)| (a, b))
            .enumerate()
        {
            let (b1, b2) = handles_before[k];
            assert!(
                c1.dist(b1) <= NODE_ALIGN + 1e-9,
                "handle 1 of {k} moved {}",
                c1.dist(b1)
            );
            assert!(
                c2.dist(b2) <= NODE_ALIGN + 1e-9,
                "handle 2 of {k} moved {}",
                c2.dist(b2)
            );
        }
    }

    #[test]
    fn a_handle_within_the_window_snaps_onto_the_axis() {
        // a curve whose first handle leaves its node 1.4 deg off horizontal
        let off = 1.4_f64.to_radians();
        let mut fp = FittedPath {
            start: pt(0.0, 0.0),
            segments: vec![
                Segment::Cubic(
                    pt(20.0 * off.cos(), 20.0 * off.sin()),
                    pt(60.0, 10.0),
                    pt(80.0, 0.0),
                ),
                Segment::Cubic(pt(90.0, -20.0), pt(20.0, -30.0), pt(0.0, 0.0)),
            ],
            closed: true,
        };
        let poly = poly_of(&fp, 48);
        let mut st = EditStats::default();
        let starts = chain_starts(&fp);
        pass_handles(&mut fp, &starts, &poly.points, &poly.sigma, &mut st);
        assert!(st.axis_snaps >= 1, "the handle should have snapped: {st:?}");
        let (c1, _, _) = cubic(&fp.segments[0]).expect("a cubic");
        assert!(c1.y.abs() < 1e-9, "handle is still {} off the axis", c1.y);
    }

    #[test]
    fn a_pass_that_would_stray_too_far_is_refused() {
        // sigma of zero and no room: any move at all breaks the guard
        let fp = diamond();
        let starts = chain_starts(&fp);
        let pts = samples(&fp.segments, &starts, 16);
        let sigma = vec![0.0; pts.len()];
        let mut moved = fp.segments.clone();
        moved[0] = Segment::Line(pt(200.0, 50.0));
        assert!(!guarded(&moved, &starts, &pts, &sigma));
        assert!(guarded(&fp.segments, &starts, &pts, &sigma));
    }
}
