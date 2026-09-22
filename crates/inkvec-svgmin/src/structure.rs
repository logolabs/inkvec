//! How editable a drawing is, measured from the SVG and nothing else.
//!
//! An artist who opens a tracer's output in a vector editor meets its structure, not its
//! picture: where the nodes are, whether the handles line up with anything, whether a
//! curve passes through a node smoothly. Hand-drawn files have habits here — most of their
//! nodes share a coordinate with another node, a large share of handles sit exactly on an
//! axis, and most joins between two curves are smooth — and a traced file, fitted for
//! pixels alone, has none of them. This counts those habits so that a mode built to acquire
//! them (`--editability`) can be shown to have done so, and shown at what price.
//!
//! Three ratios, each defined so that it can be measured on a file somebody drew and on a
//! file a machine wrote, with the same rule:
//!
//! * **axis handles** — the share of cubic handles that point exactly along the x or y axis
//!   (to the two-decimal resolution the tracer writes, or within 0.05°).
//! * **smooth joins** — the share of joins between two cubics whose tangents agree to
//!   within 1°.
//! * **aligned nodes** — the share of nodes that share an exact x or an exact y with
//!   another node of the drawing.
//!
//! It reads `d` attributes only, as the rest of this crate does, and counts arcs and
//! quadratics as segments and nodes but not as cubics: an arc has no handles to be on an
//! axis, which is the reason to draw one.

use svgtypes::{PathParser, PathSegment as S};

/// The counts behind the three ratios.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Structure {
    /// On-curve points: the start of each subpath and the end of each segment.
    pub nodes: usize,
    /// Cubic Bézier segments.
    pub cubics: usize,
    /// Handles of those cubics that have any length.
    pub handles: usize,
    /// Of those, the ones lying along an axis.
    pub axis_handles: usize,
    /// Joins between two consecutive cubics (including the one that closes a loop).
    pub joins: usize,
    /// Of those, the ones smooth to within a degree.
    pub smooth_joins: usize,
    /// Nodes that share an exact x or y with another node.
    pub aligned_nodes: usize,
}

impl Structure {
    /// The counts as a JSON object, for callers that have no serializer of their own.
    pub fn to_json(&self) -> String {
        format!(
            "{{\"nodes\":{},\"cubics\":{},\"handles\":{},\"axisHandles\":{},\"joins\":{},\"smoothJoins\":{},\"alignedNodes\":{}}}",
            self.nodes,
            self.cubics,
            self.handles,
            self.axis_handles,
            self.joins,
            self.smooth_joins,
            self.aligned_nodes
        )
    }
}

/// A handle shorter than this is the node it hangs from, not a direction.
const MIN_HANDLE: f64 = 0.05;
/// Off-axis distance below which a handle is on the axis: half a step of the tracer's
/// two-decimal coordinates, and a hair more.
const AXIS_EPS: f64 = 0.006;
/// The same test as an angle, for handles long enough that rounding cannot explain it.
const AXIS_DEGREES: f64 = 0.05;
/// Turn between two tangents at or below which a join counts as smooth.
const SMOOTH_DEGREES: f64 = 1.0;

type P = (f64, f64);

#[derive(Clone, Copy)]
enum Seg {
    Cubic { a: P, c1: P, c2: P, b: P },
    Other { b: P },
}

impl Seg {
    fn end(&self) -> P {
        match *self {
            Seg::Cubic { b, .. } | Seg::Other { b } => b,
        }
    }
}

/// Measure every `<path>` in `svg`. A document that does not parse, or has no paths,
/// measures as all zeros.
pub fn structure(svg: &str) -> Structure {
    let Ok(doc) = roxmltree::Document::parse_with_options(
        svg,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    ) else {
        return Structure::default();
    };
    let mut out = Structure::default();
    let mut nodes: Vec<P> = Vec::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("path")) {
        if let Some(d) = node.attribute("d") {
            walk(d, &mut out, &mut nodes);
        }
    }
    out.nodes = nodes.len();
    out.aligned_nodes = aligned(&nodes);
    out
}

/// Add one `d` attribute's handles, joins and nodes to the running totals.
fn walk(d: &str, out: &mut Structure, nodes: &mut Vec<P>) {
    let mut sub: Vec<Seg> = Vec::new();
    let mut start: P = (0.0, 0.0);
    let mut pen = start;
    // The reflected control point `S` needs, when the previous segment was a cubic.
    let mut last_c2: Option<P> = None;

    for seg in PathParser::from(d) {
        let Ok(seg) = seg else { break };
        let abs = |abs: bool, base: f64, v: f64| if abs { v } else { base + v };
        match seg {
            S::MoveTo { abs: a, x, y } => {
                finish(&mut sub, start, false, out, nodes);
                start = (abs(a, pen.0, x), abs(a, pen.1, y));
                pen = start;
                last_c2 = None;
            }
            S::LineTo { abs: a, x, y } => {
                let p = (abs(a, pen.0, x), abs(a, pen.1, y));
                sub.push(Seg::Other { b: p });
                pen = p;
                last_c2 = None;
            }
            S::HorizontalLineTo { abs: a, x } => {
                pen = (abs(a, pen.0, x), pen.1);
                sub.push(Seg::Other { b: pen });
                last_c2 = None;
            }
            S::VerticalLineTo { abs: a, y } => {
                pen = (pen.0, abs(a, pen.1, y));
                sub.push(Seg::Other { b: pen });
                last_c2 = None;
            }
            S::CurveTo {
                abs: a,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let c1 = (abs(a, pen.0, x1), abs(a, pen.1, y1));
                let c2 = (abs(a, pen.0, x2), abs(a, pen.1, y2));
                let b = (abs(a, pen.0, x), abs(a, pen.1, y));
                sub.push(Seg::Cubic { a: pen, c1, c2, b });
                pen = b;
                last_c2 = Some(c2);
            }
            S::SmoothCurveTo {
                abs: a,
                x2,
                y2,
                x,
                y,
            } => {
                let c1 = match last_c2 {
                    Some(p) => (2.0 * pen.0 - p.0, 2.0 * pen.1 - p.1),
                    None => pen,
                };
                let c2 = (abs(a, pen.0, x2), abs(a, pen.1, y2));
                let b = (abs(a, pen.0, x), abs(a, pen.1, y));
                sub.push(Seg::Cubic { a: pen, c1, c2, b });
                pen = b;
                last_c2 = Some(c2);
            }
            S::Quadratic { abs: a, x, y, .. } | S::SmoothQuadratic { abs: a, x, y } => {
                pen = (abs(a, pen.0, x), abs(a, pen.1, y));
                sub.push(Seg::Other { b: pen });
                last_c2 = None;
            }
            S::EllipticalArc { abs: a, x, y, .. } => {
                pen = (abs(a, pen.0, x), abs(a, pen.1, y));
                sub.push(Seg::Other { b: pen });
                last_c2 = None;
            }
            S::ClosePath { .. } => {
                finish(&mut sub, start, true, out, nodes);
                pen = start;
                last_c2 = None;
            }
        }
    }
    finish(&mut sub, start, false, out, nodes);
}

/// Close one subpath's books: its nodes, its cubics' handles, and the joins between
/// consecutive cubics. `closed` says a `Z` ended it; the join from its last segment back to
/// its first only exists where that last segment really arrives at the start, because
/// otherwise `Z` draws a straight line there and no two curves meet.
fn finish(sub: &mut Vec<Seg>, start: P, closed: bool, out: &mut Structure, nodes: &mut Vec<P>) {
    if sub.is_empty() {
        return;
    }
    let returns = dist(sub[sub.len() - 1].end(), start) < 1e-9;
    nodes.push(start);
    nodes.extend(sub.iter().map(Seg::end));
    // A loop ends where it began; that is one node, not two.
    if closed && returns {
        nodes.pop();
    }
    for seg in sub.iter() {
        if let Seg::Cubic { a, c1, c2, b } = *seg {
            out.cubics += 1;
            for (from, to) in [(a, c1), (c2, b)] {
                let v = (to.0 - from.0, to.1 - from.1);
                if v.0.hypot(v.1) >= MIN_HANDLE {
                    out.handles += 1;
                    out.axis_handles += usize::from(on_axis(v));
                }
            }
        }
    }
    let n = sub.len();
    let joins = if closed && returns {
        n
    } else {
        n.saturating_sub(1)
    };
    for k in 0..joins {
        if let (Seg::Cubic { c2, b, .. }, Seg::Cubic { a, c1, .. }) = (sub[k], sub[(k + 1) % n]) {
            let t_in = (b.0 - c2.0, b.1 - c2.1);
            let t_out = (c1.0 - a.0, c1.1 - a.1);
            if t_in.0.hypot(t_in.1) >= MIN_HANDLE && t_out.0.hypot(t_out.1) >= MIN_HANDLE {
                out.joins += 1;
                out.smooth_joins += usize::from(turn_degrees(t_in, t_out) <= SMOOTH_DEGREES);
            }
        }
    }
    sub.clear();
}

fn dist(a: P, b: P) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Whether a handle vector runs along the x or the y axis.
fn on_axis(v: P) -> bool {
    let (ax, ay) = (v.0.abs(), v.1.abs());
    let (along, across) = if ax >= ay { (ax, ay) } else { (ay, ax) };
    across <= AXIS_EPS || (across / along).atan().to_degrees() <= AXIS_DEGREES
}

/// The angle between two directions, in degrees, 0 when they agree.
fn turn_degrees(a: P, b: P) -> f64 {
    let cross = a.0 * b.1 - a.1 * b.0;
    let dot = a.0 * b.0 + a.1 * b.1;
    cross.atan2(dot).abs().to_degrees()
}

/// Nodes that share an exact x or an exact y with another node, at the two-decimal
/// resolution the tracer writes.
fn aligned(nodes: &[P]) -> usize {
    use std::collections::HashMap;
    let key = |v: f64| (v * 100.0).round() as i64;
    let mut xs: HashMap<i64, usize> = HashMap::new();
    let mut ys: HashMap<i64, usize> = HashMap::new();
    for &(x, y) in nodes {
        *xs.entry(key(x)).or_default() += 1;
        *ys.entry(key(y)).or_default() += 1;
    }
    nodes
        .iter()
        .filter(|&&(x, y)| xs[&key(x)] > 1 || ys[&key(y)] > 1)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svg(d: &str) -> String {
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"{d}\"/></svg>")
    }

    /// A rounded square drawn the way an artist draws it: every handle on an axis, every
    /// join smooth, every node sharing a coordinate with a neighbour.
    const ARTIST: &str = "M20,0H80C100,0 100,0 100,20V80C100,100 100,100 80,100H20C0,100 0,100 0,80V20C0,0 0,0 20,0Z";

    #[test]
    fn a_hand_drawn_rounded_square_is_all_structure() {
        let s = structure(&svg(ARTIST));
        assert_eq!(s.cubics, 4);
        assert_eq!(s.handles, 8);
        assert_eq!(s.axis_handles, 8, "{s:?}");
        // The straight edges between the corners are lines, so no join has two cubics.
        assert_eq!(s.joins, 0);
        assert_eq!(s.nodes, 8);
        assert_eq!(s.aligned_nodes, 8, "{s:?}");
    }

    #[test]
    fn a_fitted_blob_has_none_of_it() {
        // The same four corners with handles a few degrees off the axes and nodes that
        // share nothing: what a pixel-only fit writes.
        let d = "M20.13,0.41C39.2,1.7 99.4,-2.2 100.31,20.05C101.8,45.5 98.3,79.2 80.07,100.62C55.2,99.1 30.6,103.4 19.9,99.47C1.2,98.6 -1.4,40.9 0.52,20.33C0.1,8.2 9.4,-0.3 20.13,0.41Z";
        let s = structure(&svg(d));
        assert_eq!(s.cubics, 5);
        assert_eq!(s.axis_handles, 0, "{s:?}");
        // Nodes: five, and the loop closes on its own start, which counts once.
        assert_eq!(s.nodes, 5);
        assert_eq!(s.aligned_nodes, 0, "{s:?}");
        assert_eq!(s.joins, 5);
        assert!(s.smooth_joins < s.joins, "{s:?}");
    }

    #[test]
    fn collinear_handles_at_a_join_are_smooth_and_a_kink_is_not() {
        let smooth = structure(&svg("M0,0C10,10 20,10 30,0C40,-10 50,-10 60,0"));
        assert_eq!((smooth.joins, smooth.smooth_joins), (1, 1), "{smooth:?}");
        let kinked = structure(&svg("M0,0C10,10 20,10 30,0C40,10 50,10 60,0"));
        assert_eq!((kinked.joins, kinked.smooth_joins), (1, 0), "{kinked:?}");
    }

    #[test]
    fn the_closing_join_of_a_loop_counts() {
        // Two cubics that make a loop: the join at the start is the closing one.
        let s = structure(&svg("M0,10C0,-3 20,-3 20,10C20,23 0,23 0,10Z"));
        assert_eq!(s.joins, 2, "{s:?}");
        assert_eq!(s.smooth_joins, 2, "{s:?}");
    }

    #[test]
    fn a_z_that_draws_a_line_closes_no_join() {
        // The last cubic ends away from the start, so `Z` is a straight line: the curves at
        // the two ends of the subpath do not meet.
        let s = structure(&svg("M0,0C10,10 20,10 30,0C40,-10 50,-10 60,0Z"));
        assert_eq!((s.joins, s.smooth_joins), (1, 1), "{s:?}");
    }

    #[test]
    fn relative_and_smooth_commands_read_as_their_absolute_forms() {
        let absolute = structure(&svg("M0,0C10,0 20,10 30,10C40,10 50,20 60,20"));
        let written_short = structure(&svg("m0,0c10,0 20,10 30,10s20,10 30,10"));
        assert_eq!(absolute, written_short);
    }

    #[test]
    fn arcs_are_segments_without_handles() {
        let s = structure(&svg("M0,50A50,50 0 1 1 100,50A50,50 0 1 1 0,50Z"));
        assert_eq!((s.cubics, s.handles, s.joins), (0, 0, 0));
        assert_eq!(s.nodes, 2);
    }

    #[test]
    fn nothing_parses_as_nothing() {
        assert_eq!(structure("not svg"), Structure::default());
        assert_eq!(
            structure("<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            Structure::default()
        );
    }

    #[test]
    fn json_is_the_counts_under_the_names_the_apps_use() {
        let j = Structure {
            nodes: 1,
            cubics: 2,
            handles: 3,
            axis_handles: 4,
            joins: 5,
            smooth_joins: 6,
            aligned_nodes: 7,
        }
        .to_json();
        assert_eq!(
            j,
            "{\"nodes\":1,\"cubics\":2,\"handles\":3,\"axisHandles\":4,\"joins\":5,\"smoothJoins\":6,\"alignedNodes\":7}"
        );
    }
}
