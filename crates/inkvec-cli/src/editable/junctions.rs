//! Smooth joins across the planar map's junctions, read on the faces' rings.
//!
//! Every other editability pass works on one fitted edge at a time, between two junctions,
//! so it never sees the join where a face's outline passes from one edge to the next. That
//! join is where a drawing made of layers shows most: a shape painted over another leaves
//! the lower one's visible outline with a T-junction on each side, and the upper one's
//! outline runs straight through both. The two edges of the upper outline were fitted
//! apart, each with its own end direction, so the outline comes out kinked at the junction
//! where the artist's runs on smoothly.
//!
//! The pass walks each face's rings ([`crate::faces::Ring`]) and, at each join between two
//! different edges that turns by at most [`JUNCTION_G1_DEG`], turns the handles either side
//! onto one direction: the line's when one side is a line, a handle's when it alone is
//! exactly on an axis (the handle snaps run first), else the bisector of the two. Handles keep their lengths and no node moves, so the junction stays shared and
//! no seam opens. A change is kept only if both edges still pass the editability guard
//! (`3·σ + EDIT_SLACK`, one-sided, see the parent module); otherwise both are restored.

use inkvec_core::{Point, Polyline, Vec2};
use inkvec_fit::curves::Segment;
use inkvec_fit::primitives::PrimitiveFit;
use inkvec_fit::FittedPath;

use super::{chain_starts, guarded, EditStats};
use crate::faces::FaceRings;

/// Largest turn (degrees) at a junction that the pass makes smooth. The artists' joins
/// between curves are smooth or corners, with 8.6 % between 3 and 30 degrees against the
/// trace's 25 % (`bench/theory/node_stats.py`, 512 px on white); this reaches the kinked
/// band, and the guard decides which of them the pixels allow.
const JUNCTION_G1_DEG: f64 = 30.0;

/// Which end of an edge a junction is at.
#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    Start,
    Finish,
}

/// One edge's side of a join: the edge, which end, and whether the ring arrives along it
/// (`incoming`) or leaves along it.
#[derive(Clone, Copy)]
struct Side {
    edge: usize,
    end: End,
    incoming: bool,
}

impl Side {
    /// The junction point.
    fn node(&self, fps: &[FittedPath]) -> Point {
        match self.end {
            End::Start => fps[self.edge].start,
            End::Finish => fps[self.edge].end(),
        }
    }

    /// The segment at this end, its index, and the point it starts at.
    fn segment<'a>(&self, fp: &'a FittedPath) -> Option<(usize, &'a Segment, Point)> {
        let n = fp.segments.len();
        if n == 0 {
            return None;
        }
        let q = match self.end {
            End::Start => 0,
            End::Finish => n - 1,
        };
        let start = if q == 0 {
            fp.start
        } else {
            fp.segments[q - 1].end()
        };
        Some((q, &fp.segments[q], start))
    }

    /// The direction the ring travels in at the junction, on this side: arriving along an
    /// incoming side, leaving along an outgoing one.
    fn direction(&self, fps: &[FittedPath]) -> Option<Vec2> {
        let fp = &fps[self.edge];
        let (_, seg, start) = self.segment(fp)?;
        let node = self.node(fps);
        // The point next to the junction on this side: the handle, or a line's other end.
        let near = match (seg, self.end) {
            (&Segment::Cubic(c1, _, e), End::Start) => first_distinct(node, &[c1, e]),
            (&Segment::Cubic(_, c2, _), End::Finish) => first_distinct(node, &[c2, start]),
            (&Segment::Line(e), End::Start) => first_distinct(node, &[e]),
            (Segment::Line(_), End::Finish) => first_distinct(node, &[start]),
            (Segment::Arc { .. }, _) => return None,
        }?;
        // `near − node` points away from the junction into this edge.
        let away = unit(near - node)?;
        Some(if self.incoming {
            Vec2 {
                x: -away.x,
                y: -away.y,
            }
        } else {
            away
        })
    }

    /// Turn this side's handle at the junction so the ring travels along `dir` there. A
    /// line has no handle and is left alone (its direction is the target); `false` for
    /// anything else that cannot be turned.
    fn turn_to(&self, fps: &mut [FittedPath], dir: Vec2) -> bool {
        let node = self.node(fps);
        let fp = &mut fps[self.edge];
        let Some((q, seg, _)) = self.segment(fp) else {
            return false;
        };
        // The handle points into the edge: against the travel when incoming, with it when
        // leaving.
        let away = if self.incoming {
            Vec2 {
                x: -dir.x,
                y: -dir.y,
            }
        } else {
            dir
        };
        match (seg.clone(), self.end) {
            (Segment::Line(_), _) => true,
            (Segment::Cubic(c1, c2, e), End::Start) => {
                let l = node.dist(c1);
                fp.segments[q] =
                    Segment::Cubic(Point::new(node.x + away.x * l, node.y + away.y * l), c2, e);
                l > 1e-9
            }
            (Segment::Cubic(c1, c2, e), End::Finish) => {
                let l = node.dist(c2);
                fp.segments[q] =
                    Segment::Cubic(c1, Point::new(node.x + away.x * l, node.y + away.y * l), e);
                l > 1e-9
            }
            (Segment::Arc { .. }, _) => false,
        }
    }

    fn is_line(&self, fps: &[FittedPath]) -> bool {
        self.segment(&fps[self.edge])
            .is_some_and(|(_, s, _)| matches!(s, Segment::Line(_)))
    }
}

/// The first of `cands` not at `node`.
fn first_distinct(node: Point, cands: &[Point]) -> Option<Point> {
    cands.iter().copied().find(|p| p.dist(node) > 1e-9)
}

fn unit(v: Vec2) -> Option<Vec2> {
    let n = v.norm();
    (n > 1e-12 && n.is_finite()).then(|| Vec2 {
        x: v.x / n,
        y: v.y / n,
    })
}

/// Make the joins between consecutive edges of every face's rings smooth where they turn by
/// at most [`JUNCTION_G1_DEG`] and both edges still guard. Primitives are not touched.
pub(super) fn pass_junctions(
    order: &[FaceRings],
    fps: &mut [FittedPath],
    polys: &[Polyline],
    prims: &[Option<PrimitiveFit>],
    st: &mut EditStats,
) {
    let n = fps.len();
    let free = |e: usize| e < n && prims.get(e).is_none_or(|p| p.is_none());
    for ring in order.iter().flatten() {
        let k = ring.len();
        if k < 2 {
            continue;
        }
        for i in 0..k {
            let (ea, ra) = ring[i];
            let (eb, rb) = ring[(i + 1) % k];
            if ea == eb || !free(ea) || !free(eb) {
                continue;
            }
            let a = Side {
                edge: ea,
                end: if ra { End::Start } else { End::Finish },
                incoming: true,
            };
            let b = Side {
                edge: eb,
                end: if rb { End::Finish } else { End::Start },
                incoming: false,
            };
            smooth_join(a, b, fps, polys, st);
        }
    }
}

/// Make one join smooth if it is nearly so and both edges still guard.
fn smooth_join(a: Side, b: Side, fps: &mut [FittedPath], polys: &[Polyline], st: &mut EditStats) {
    let debug = inkvec_core::env::flag("INKVEC_EDIT_DEBUG");
    let gap = a.node(fps).dist(b.node(fps));
    if gap > 1e-6 {
        crate::diag::debug(debug, || {
            format!("    junction: ends {gap:.3} px apart -- skipped")
        });
        return;
    }
    let (Some(da), Some(db)) = (a.direction(fps), b.direction(fps)) else {
        return;
    };
    let cos = (da.x * db.x + da.y * db.y).clamp(-1.0, 1.0);
    let turn = cos.acos().to_degrees();
    crate::diag::debug(debug, || {
        format!(
            "    junction: turn {turn:.1} deg, lines {} {}",
            a.is_line(fps),
            b.is_line(fps)
        )
    });
    if turn <= 1e-6 || turn > JUNCTION_G1_DEG {
        return;
    }
    // A line's direction is fixed; a handle already exactly on an axis (the handle snaps ran
    // before this pass) keeps its snap; otherwise the two meet half way.
    let on_axis = |d: Vec2| d.x.abs() < 1e-12 || d.y.abs() < 1e-12;
    let target = match (a.is_line(fps), b.is_line(fps)) {
        (true, true) => return,
        (true, false) => da,
        (false, true) => db,
        (false, false) if on_axis(da) && !on_axis(db) => da,
        (false, false) if on_axis(db) && !on_axis(da) => db,
        (false, false) => match unit(Vec2 {
            x: da.x + db.x,
            y: da.y + db.y,
        }) {
            Some(t) => t,
            None => return,
        },
    };
    let saved = (fps[a.edge].clone(), fps[b.edge].clone());
    let ok = a.turn_to(fps, target)
        && b.turn_to(fps, target)
        && [a.edge, b.edge].iter().all(|&e| {
            guarded(
                &fps[e].segments,
                &chain_starts(&fps[e]),
                &polys[e].points,
                &polys[e].sigma,
            )
        });
    if ok {
        st.junction_g1 += 1;
    } else {
        fps[a.edge] = saved.0;
        fps[b.edge] = saved.1;
        st.reverted += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth S of two quarter circles meeting at the origin, as two edges whose
    /// handles at the shared node turn by `kink_deg` between them.
    fn two_edges(kink_deg: f64) -> (Vec<FittedPath>, Vec<Polyline>) {
        let r = 30.0;
        let k = 0.5523 * r;
        let off = kink_deg.to_radians();
        // Edge 0: from (-r, -r) down to the origin, arriving horizontally (+x).
        let e0 = FittedPath {
            start: Point::new(-r, -r),
            segments: vec![Segment::Cubic(
                Point::new(-r, -r + k),
                Point::new(-k * off.cos(), -k * off.sin()),
                Point::new(0.0, 0.0),
            )],
            closed: false,
        };
        // Edge 1: from the origin, leaving horizontally (+x), up to (r, r).
        let e1 = FittedPath {
            start: Point::new(0.0, 0.0),
            segments: vec![Segment::Cubic(
                Point::new(k, 0.0),
                Point::new(r, r - k),
                Point::new(r, r),
            )],
            closed: false,
        };
        let arc = |cx: f64, cy: f64, a0: f64| -> Vec<Point> {
            (0..=60)
                .map(|i| {
                    let t = a0 + std::f64::consts::FRAC_PI_2 * i as f64 / 60.0;
                    Point::new(cx + r * t.cos(), cy + r * t.sin())
                })
                .collect()
        };
        let p0 = arc(0.0, -r, std::f64::consts::PI);
        let p0: Vec<Point> = p0
            .into_iter()
            .map(|p| Point::new(p.x, -r - (p.y + r)))
            .collect();
        let p1 = arc(0.0, r, -std::f64::consts::FRAC_PI_2);
        let polys = vec![
            Polyline::with_uniform_sigma(p0, 0.05, false),
            Polyline::with_uniform_sigma(p1, 0.05, false),
        ];
        (vec![e0, e1], polys)
    }

    fn turn_at_origin(fps: &[FittedPath]) -> f64 {
        let (Segment::Cubic(_, c2, _), Segment::Cubic(c1, _, _)) =
            (&fps[0].segments[0], &fps[1].segments[0])
        else {
            panic!("cubics");
        };
        let (a, b) = ((-c2.x, -c2.y), (c1.x, c1.y));
        let cos = (a.0 * b.0 + a.1 * b.1) / (a.0.hypot(a.1) * b.0.hypot(b.1));
        cos.clamp(-1.0, 1.0).acos().to_degrees()
    }

    #[test]
    fn a_kink_where_two_edges_meet_is_made_smooth() {
        let (mut fps, polys) = two_edges(4.0);
        assert!(turn_at_origin(&fps) > 3.9);
        let order = vec![vec![vec![(0usize, false), (1usize, false)]]];
        let mut st = EditStats::default();
        pass_junctions(&order, &mut fps, &polys, &[None, None], &mut st);
        assert_eq!(
            st.junction_g1, 1,
            "one join at the junction; the ring's other join is a corner"
        );
        assert!(turn_at_origin(&fps) < 1e-9);
        assert_eq!(fps[0].end(), fps[1].start, "the junction did not move");
    }

    #[test]
    fn a_corner_at_a_junction_is_left_alone() {
        let (mut fps, polys) = two_edges(50.0);
        let before = turn_at_origin(&fps);
        let order = vec![vec![vec![(0usize, false), (1usize, false)]]];
        let mut st = EditStats::default();
        pass_junctions(&order, &mut fps, &polys, &[None, None], &mut st);
        assert_eq!(st.junction_g1, 0);
        assert!((turn_at_origin(&fps) - before).abs() < 1e-12);
    }
}
