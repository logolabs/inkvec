//! The editability passes on rings whose answer is known.

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
/// A moved node carries its handles, so each handle keeps its offset from its node.
#[test]
fn aligning_nodes_moves_nodes_with_their_handles() {
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
    // Each cubic's handles as offsets from the nodes they hang from.
    let offsets = |fp: &FittedPath| -> Vec<(f64, f64, f64, f64)> {
        let starts = chain_starts(fp);
        fp.segments
            .iter()
            .enumerate()
            .filter_map(|(k, s)| {
                cubic(s).map(|(c1, c2, e)| {
                    (
                        c1.x - starts[k].x,
                        c1.y - starts[k].y,
                        c2.x - e.x,
                        c2.y - e.y,
                    )
                })
            })
            .collect()
    };
    let offsets_before = offsets(&fp);
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
    for (a, b) in offsets(out).iter().zip(&offsets_before) {
        for (x, y) in [(a.0, b.0), (a.1, b.1), (a.2, b.2), (a.3, b.3)] {
            assert!((x - y).abs() < 1e-9, "a handle left its node: {a:?} {b:?}");
        }
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

#[test]
fn test_edit_all_on_diamond() {
    let fp = diamond();
    let poly = poly_of(&fp, 48);
    let mut fps = vec![fp];
    let polys = vec![poly];
    let prims = vec![None];
    let st = edit_all(
        &polys,
        &mut fps,
        &prims,
        &[],
        &inkvec_fit::FitConfig::default(),
        512,
    );
    assert!(st.rings > 0);
    let summary = st.summary();
    assert!(summary.contains("editability"));
}

#[test]
fn test_pass_g1() {
    let p0 = pt(10.0, 50.0);
    let p_mid = pt(50.0, 50.0);
    let p_end = pt(90.0, 50.0);
    let seg1 = Segment::Cubic(pt(30.0, 50.0), pt(40.0, 50.0), p_mid);
    let seg2 = Segment::Cubic(pt(60.0, 50.3), pt(70.0, 50.0), p_end);
    let mut fp = FittedPath {
        start: p0,
        segments: vec![seg1, seg2],
        closed: false,
    };
    let poly = poly_of(&fp, 48);
    let mut st = EditStats::default();
    let starts = chain_starts(&fp);
    pass_g1(&mut fp, &starts, &poly.points, &poly.sigma, &mut st);
    assert_eq!(st.g1_joins, 1);
}

/// The handles that turn are the two at the join: the incoming cubic's second control point
/// and the outgoing one's first. (The pass once turned the incoming cubic's *first* control
/// point, at its far end, so the join kept its kink and the far end gained one.)
#[test]
fn g1_turns_the_two_handles_at_the_join_and_no_other() {
    let a = 10f64.to_radians();
    let mut fp = FittedPath {
        start: pt(0.0, 0.0),
        segments: vec![
            Segment::Cubic(pt(15.0, 0.0), pt(35.0, 0.0), pt(50.0, 0.0)),
            Segment::Cubic(
                pt(50.0 + 15.0 * a.cos(), 15.0 * a.sin()),
                pt(85.0, 6.0),
                pt(100.0, 8.0),
            ),
        ],
        closed: false,
    };
    let poly = poly_of(&fp, 48);
    let before = fp.segments.clone();
    let mut st = EditStats::default();
    let starts = chain_starts(&fp);
    pass_g1(&mut fp, &starts, &poly.points, &poly.sigma, &mut st);
    assert_eq!(st.g1_joins, 1);
    let starts = chain_starts(&fp);
    let din = end_direction(&fp.segments[0], starts[0], true).expect("a direction");
    let dout = end_direction(&fp.segments[1], starts[1], false).expect("a direction");
    assert!(
        (din.0 - dout.0).abs() < 1e-9 && (din.1 - dout.1).abs() < 1e-9,
        "{din:?} {dout:?}"
    );
    let (c1, _, _) = cubic(&fp.segments[0]).expect("a cubic");
    let (b1, _, _) = cubic(&before[0]).expect("a cubic");
    assert_eq!(c1, b1, "the far handle stays");
    let (_, c2, _) = cubic(&fp.segments[1]).expect("a cubic");
    let (_, b2, _) = cubic(&before[1]).expect("a cubic");
    assert_eq!(c2, b2, "the far handle stays");
}

/// A boundary with no junction comes as an open path ending where it starts; its closing
/// join is a join like any other, and the per-ring passes see it.
#[test]
fn the_closing_join_of_a_loop_edge_is_edited() {
    // A trapezoid (no mirror, which would put nodes of its own on an axis) whose path
    // starts in the middle of its top side: the closing join joins two halves of one
    // straight line.
    let fp = FittedPath {
        start: pt(30.0, 0.0),
        segments: vec![
            Segment::Line(pt(60.0, 0.0)),
            Segment::Line(pt(45.0, 40.0)),
            Segment::Line(pt(0.0, 40.0)),
            Segment::Line(pt(0.0, 0.0)),
            Segment::Line(pt(30.0, 0.0)),
        ],
        closed: false,
    };
    let poly = poly_of(&fp, 48);
    let mut fps = vec![fp];
    let st = edit_all(
        &[poly],
        &mut fps,
        &[None],
        &[],
        &inkvec_fit::FitConfig::default(),
        512,
    );
    assert_eq!(st.collinear_merged, 1);
    assert_eq!(fps[0].segments.len(), 4);
    assert!(!fps[0].closed, "the flag is put back");
    assert_eq!(fps[0].segments[3].end(), fps[0].start, "still a loop");
}

/// The same path is not a loop when another edge ends at its start: that is a junction.
#[test]
fn a_loop_through_a_junction_is_not_isolated() {
    let ring = FittedPath {
        start: pt(30.0, 0.0),
        segments: vec![
            Segment::Line(pt(60.0, 0.0)),
            Segment::Line(pt(60.0, 40.0)),
            Segment::Line(pt(30.0, 0.0)),
        ],
        closed: false,
    };
    let spoke = FittedPath {
        start: pt(30.0, 0.0),
        segments: vec![Segment::Line(pt(30.0, -20.0))],
        closed: false,
    };
    assert_eq!(isolated_loops(std::slice::from_ref(&ring), &[0]), vec![0]);
    assert!(isolated_loops(&[ring, spoke], &[0, 1]).is_empty());
}

#[test]
fn a_kink_between_a_line_and_a_curve_is_made_smooth_along_the_line() {
    // A horizontal line into a curve whose handle leaves 8 degrees off it; the points
    // are those of the smooth curve, so the kink is the fit's, not the drawing's.
    let off = 8f64.to_radians();
    let smooth = FittedPath {
        start: pt(0.0, 0.0),
        segments: vec![
            Segment::Line(pt(40.0, 0.0)),
            Segment::Cubic(pt(50.0, 0.0), pt(60.0, 5.0), pt(70.0, 20.0)),
        ],
        closed: false,
    };
    let poly = poly_of(&smooth, 48);
    let mut fp = smooth.clone();
    fp.segments[1] = Segment::Cubic(
        pt(40.0 + 10.0 * off.cos(), 10.0 * off.sin()),
        pt(60.0, 5.0),
        pt(70.0, 20.0),
    );
    let mut st = EditStats::default();
    let starts = chain_starts(&fp);
    pass_g1(&mut fp, &starts, &poly.points, &poly.sigma, &mut st);
    assert_eq!(st.g1_joins, 1);
    let (c1, _, _) = cubic(&fp.segments[1]).expect("a cubic");
    assert!(
        c1.y.abs() < 1e-9,
        "the handle now runs along the line: {c1:?}"
    );
}

#[test]
fn a_smooth_nodes_handles_snap_together_or_not_at_all() {
    // Two cubics meeting smoothly at (50, 0), the shared direction 2 degrees off
    // horizontal: both handles snap, and the join stays smooth.
    let a = 2f64.to_radians();
    let (dx, dy) = (a.cos(), a.sin());
    let mut fp = FittedPath {
        start: pt(0.0, 20.0),
        segments: vec![
            Segment::Cubic(
                pt(10.0, 0.0),
                pt(50.0 - 15.0 * dx, -15.0 * dy),
                pt(50.0, 0.0),
            ),
            Segment::Cubic(
                pt(50.0 + 12.0 * dx, 12.0 * dy),
                pt(90.0, 0.0),
                pt(100.0, 20.0),
            ),
        ],
        closed: false,
    };
    let poly = poly_of(&fp, 48);
    let mut st = EditStats::default();
    let starts = chain_starts(&fp);
    pass_handles(&mut fp, &starts, &poly.points, &poly.sigma, &mut st);
    let (_, c2, _) = cubic(&fp.segments[0]).expect("a cubic");
    let (c1, _, _) = cubic(&fp.segments[1]).expect("a cubic");
    assert!(
        c2.y.abs() < 1e-9 && c1.y.abs() < 1e-9,
        "both on the axis: {c2:?} {c1:?}"
    );
}

#[test]
fn two_lines_end_to_end_on_one_axis_become_one() {
    let mut fp = FittedPath {
        start: pt(0.0, 0.0),
        segments: vec![
            Segment::Line(pt(30.0, 0.0)),
            Segment::Line(pt(60.0, 0.0)),
            Segment::Line(pt(60.0, 40.0)),
            Segment::Line(pt(0.0, 0.0)),
        ],
        closed: true,
    };
    let poly = poly_of(&fp, 48);
    let mut st = EditStats::default();
    pass_collinear(&mut fp, &poly.points, &poly.sigma, &mut st);
    assert_eq!(st.collinear_merged, 1);
    assert_eq!(fp.segments.len(), 3);
    assert_eq!(fp.segments[0].end(), pt(60.0, 0.0));
    // A real corner stays.
    assert!(fp.segments.iter().all(|s| matches!(s, Segment::Line(_))));
}

/// The merged line is judged against the two it replaces, not the whole ring: a ring that
/// strays elsewhere (a mirror lock is allowed to) still loses a node in a straight run.
#[test]
fn a_straight_run_merges_on_a_ring_that_strays_elsewhere() {
    let truth = FittedPath {
        start: pt(0.0, 0.0),
        segments: vec![
            Segment::Line(pt(30.0, 0.0)),
            Segment::Line(pt(60.0, 0.0)),
            Segment::Line(pt(60.0, 40.0)),
            Segment::Line(pt(0.0, 0.0)),
        ],
        closed: true,
    };
    let poly = poly_of(&truth, 48);
    let mut fp = truth.clone();
    // The right side 1.4 px out: beyond EDIT_SLACK, within MIRROR_SLACK.
    fp.segments[1] = Segment::Line(pt(61.4, 0.0));
    fp.segments[2] = Segment::Line(pt(61.4, 40.0));
    fp.segments[0] = Segment::Line(pt(30.0, 0.0));
    assert!(!guarded(
        &fp.segments,
        &chain_starts(&fp),
        &poly.points,
        &poly.sigma
    ));
    let mut st = EditStats::default();
    pass_collinear(&mut fp, &poly.points, &poly.sigma, &mut st);
    assert_eq!(st.collinear_merged, 1, "{:?}", fp.segments);
    assert_eq!(fp.segments[0].end(), pt(61.4, 0.0));
}

#[test]
fn test_pass_equalize() {
    let p0 = pt(0.0, 0.0);
    let p_end = pt(100.0, 0.0);
    let seg = Segment::Cubic(pt(10.0, 0.0), pt(90.5, 0.0), p_end);
    let mut fp = FittedPath {
        start: p0,
        segments: vec![seg],
        closed: false,
    };
    let poly = poly_of(&fp, 48);
    let mut st = EditStats::default();
    let starts = chain_starts(&fp);
    pass_equalize(&mut fp, &starts, &poly.points, &poly.sigma, &mut st);
    assert_eq!(st.len_sym, 1);
}

#[test]
fn a_centrelines_arcs_become_cubics_and_its_lines_stay() {
    // A line, then a half circle of radius 10 round (10, 0): cut at its extremum.
    let path = FittedPath {
        start: pt(-10.0, 0.0),
        segments: vec![
            Segment::Line(pt(0.0, 0.0)),
            Segment::circular_arc(10.0, false, true, pt(20.0, 0.0)),
        ],
        closed: false,
    };
    let out = arcs_as_cubics(&path);
    assert_eq!(out.start, path.start);
    assert!(matches!(out.segments[0], Segment::Line(_)));
    assert_eq!(out.segments.len(), 3, "{:?}", out.segments);
    assert_eq!(out.end(), pt(20.0, 0.0));
    let Segment::Cubic(_, c2, mid) = out.segments[1] else {
        panic!("a cubic");
    };
    assert!(
        (c2.y - mid.y).abs() < 1e-9,
        "a horizontal handle at the extremum"
    );
}

#[test]
fn test_pass_mirror_on_diamond() {
    let mut fp = diamond();
    let poly = poly_of(&fp, 48);
    let mut st = EditStats::default();
    let starts = chain_starts(&fp);
    pass_mirror(&mut fp, &starts, &poly.points, &poly.sigma, &mut st);
    assert!(st.mirror_rings >= 1);
}

/// Under MDL pricing (`price`, what `edit_all` runs) a G1 snap that moves the curve off its
/// own evidence is kept only when what it saves, one implied direction and the smooth
/// bonus, is worth the misfit at the fit's price of a number: refused when numbers are
/// nearly free, kept when they are dear.
#[test]
fn a_priced_snap_is_kept_only_when_it_pays_at_the_price_of_a_number() {
    if inkvec_core::env::text("INKVEC_EDIT_PRICE").is_some() {
        return;
    }
    let a = 10f64.to_radians();
    let kinked = || FittedPath {
        start: pt(0.0, 0.0),
        segments: vec![
            Segment::Cubic(pt(15.0, 0.0), pt(35.0, 0.0), pt(50.0, 0.0)),
            Segment::Cubic(
                pt(50.0 + 15.0 * a.cos(), 15.0 * a.sin()),
                pt(85.0, 6.0),
                pt(100.0, 8.0),
            ),
        ],
        closed: false,
    };
    let poly = poly_of(&kinked(), 48);
    let mut joins = Vec::new();
    for lambda in [1e-6, 1e6] {
        price::set_lambda(lambda);
        let mut fp = kinked();
        let mut st = EditStats::default();
        let starts = chain_starts(&fp);
        pass_g1(&mut fp, &starts, &poly.points, &poly.sigma, &mut st);
        joins.push(st.g1_joins);
    }
    price::set_lambda(f64::NAN);
    assert_eq!(joins, vec![0, 1]);
}
