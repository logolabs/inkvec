//! Keep the symmetry the artist drew.
//!
//! A fifth of the corpus is mirror-symmetric, and the artist's symmetry is *exact*: render
//! one of those icons, compare it with its own mirror, and the residual is around 1e-5,
//! which is floating point and nothing else. Ours came back around 4e-3, two to four
//! hundred times worse, and it is the kind of error a person sees immediately and a colour
//! metric barely registers — one bar of a three-bar glyph a tenth of a pixel wider than the
//! other two.
//!
//! None of that asymmetry is in the evidence. Measured through the pipeline, the input
//! raster is symmetric to 1e-6 and **the label map is symmetric to exactly zero**. Every
//! bit of it is made downstream, by ties broken in whatever order the code happens to walk:
//! which pixel a boundary piece exactly on a pixel border is assigned to, which direction a
//! chain is traversed, which of two equal candidates a search reaches first. Those are
//! genuine coin flips, they are everywhere in a pipeline this size, and fixing them one at
//! a time is endless — the stage responsible differs from icon to icon.
//!
//! So the symmetry is enforced structurally rather than defended stage by stage. The label
//! map says exactly which mirrors hold, and it says so with no tolerance and no threshold:
//! a mirror is admitted only when *every* pixel maps to a pixel of the same ink. That test
//! cannot be fooled into snapping a logo the artist drew asymmetric, which is the failure
//! that would matter. Given a mirror, the boundaries pair off, and each pair is replaced by
//! the average of the two and its exact reflection.
//!
//! What this does not do is make the *fit* symmetric. Two mirror-paired boundaries with
//! exactly mirrored points can still be fitted differently, because the dynamic program
//! walks a chain in one direction and its partner in the other. That is handled where the
//! fitting happens, by fitting one of each pair and reflecting the result, which is both
//! exact and half the work — see `mirror_of` and its use in the CLI.

use std::collections::HashMap;

use inkvec_core::Point;

use inkvec_fit::curves::Segment;
use inkvec_fit::primitives::{PrimitiveFit, PrimitiveKind};
use inkvec_fit::FittedPath;

use crate::planar::PlanarMap;

/// A mirror of the pixel grid: `V(k)` maps pixel `x` to `k - x`, `H(k)` maps `y` to `k - y`.
///
/// Only integer `k` can be a symmetry of a pixel grid, which is what makes the test exact:
/// the mirror axis then falls either on a pixel centre (`k` even) or on the border between
/// two (`k` odd), and every pixel has a partner.
///
/// Grid nodes reflect differently from pixels and there is no helper for them here: nodes
/// sit at pixel corners, node `i` at coordinate `i - 0.5`, so the pixel map `x -> k - x`
/// carries node `i` to node `k - i + 1`. Anything that reflects nodes has to apply that
/// offset itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mirror {
    /// Vertical mirror axis: reflects `x` to `k - x`, leaves `y` unchanged.
    V(i64),
    /// Horizontal mirror axis: reflects `y` to `k - y`, leaves `x` unchanged.
    H(i64),
}

impl Mirror {
    #[inline]
    fn pixel(self, x: i64, y: i64) -> (i64, i64) {
        match self {
            Mirror::V(k) => (k - x, y),
            Mirror::H(k) => (x, k - y),
        }
    }

    /// Reflect a point in continuous image coordinates, where pixel `x` has centre `x`.
    #[inline]
    pub fn point(self, p: Point) -> Point {
        match self {
            Mirror::V(k) => Point::new(k as f64 - p.x, p.y),
            Mirror::H(k) => Point::new(p.x, k as f64 - p.y),
        }
    }
}

/// Where one boundary's reflection lands on another.
///
/// Point `i` of the reflected boundary is point `(shift + i) mod n` of `edge`, or
/// `(shift - i) mod n` when `rev`. A boundary may pair with itself.
#[derive(Clone, Copy, Debug)]
pub struct Pairing {
    /// Index of the edge this one pairs with.
    pub edge: usize,
    /// Whether `edge` is traversed in reverse to align with this one.
    pub rev: bool,
    /// Index offset between the two boundaries' point sequences.
    pub shift: usize,
}

impl Pairing {
    /// Maps point index `i` of the reflected boundary to the corresponding index into
    /// the paired edge's `n`-point sequence.
    #[inline]
    pub fn index(self, i: usize, n: usize) -> usize {
        if self.rev {
            (self.shift + n - i % n) % n
        } else {
            (self.shift + i) % n
        }
    }
}

/// The mirrors a label map actually has, and how the boundaries pair off under them.
pub struct Symmetry {
    /// The mirrors found in the label map.
    pub mirrors: Vec<Mirror>,
    /// `partner[m][e]` is where edge `e` reflects to under `mirrors[m]`.
    partner: Vec<Vec<Option<Pairing>>>,
}

impl Symmetry {
    /// Whether the label map has no mirrors.
    pub fn is_empty(&self) -> bool {
        self.mirrors.is_empty()
    }

    /// Mirrors that carry edge `e` onto itself.
    ///
    /// A shape lying across the axis is its own reflection, so it never gets a partner to
    /// copy from and has to be made symmetric in its own right.
    pub fn self_mirrors(&self, e: usize) -> Vec<Mirror> {
        let mut out = Vec::new();
        for (m, part) in self.partner.iter().enumerate() {
            if part[e].is_some_and(|pr| pr.edge == e) {
                out.push(self.mirrors[m]);
            }
        }
        out
    }

    /// The edge whose fit can be reflected to give edge `e`'s, if there is one that is
    /// *earlier* — so that following this map always terminates.
    pub fn mirror_of(&self, e: usize) -> Option<(Mirror, Pairing)> {
        for (m, part) in self.partner.iter().enumerate() {
            if let Some(pr) = part[e] {
                if pr.edge < e {
                    return Some((self.mirrors[m], pr));
                }
            }
        }
        None
    }
}

/// Every mirror of the pixel grid under which each pixel keeps its ink.
///
/// `ink[label]` is what makes two faces interchangeable: face ids are connected components
/// and a shape's two halves are never the same component, so comparing them directly would
/// reject every symmetry there is.
fn mirrors_of(labels: &[u16], ink: &[usize], w: usize, h: usize) -> Vec<Mirror> {
    let at = |x: i64, y: i64| -> Option<usize> {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            return None;
        }
        labels
            .get(y as usize * w + x as usize)
            .and_then(|&l| ink.get(l as usize))
            .copied()
    };
    let holds = |m: Mirror| -> bool {
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let (mx, my) = m.pixel(x, y);
                if at(x, y) != at(mx, my) {
                    return false;
                }
            }
        }
        true
    };
    let mut out = Vec::new();
    // Only the exact centre can hold. A pixel inside the image is Some and one outside
    // is None, so for every x both x and k - x must be inside: x = 0 needs k < w and
    // x = w - 1 needs k >= w - 1. That leaves k = w - 1, and likewise k = h - 1 for the
    // horizontal axis. The loop that used to try every k was pure cost: each off-centre
    // horizontal candidate matched white margin rows against white margin rows before it
    // could fail, O(h^2 w) in all -- 1.1 s of a 4 s trace at 2048 px, for nothing.
    if holds(Mirror::V(w as i64 - 1)) {
        out.push(Mirror::V(w as i64 - 1));
    }
    if holds(Mirror::H(h as i64 - 1)) {
        out.push(Mirror::H(h as i64 - 1));
    }
    out
}

/// Pair the boundaries off under one mirror.
///
/// Matching is by the points themselves, not by the endpoints, because a closed boundary
/// has no meaningful endpoints: the extractor starts its walk at whichever crack it reached
/// first, and that choice is not mirror-equivariant. So a ring and its reflection agree only
/// up to where the walk began and which way round it went, and the pairing has to recover
/// both. This runs on the lattice points the extractor produced, where every coordinate is
/// an exact half-integer, so the comparison is exact and needs no tolerance.
fn pair_edges(map: &PlanarMap, m: Mirror) -> Option<Vec<Option<Pairing>>> {
    // Exact key for a lattice point: coordinates are half-integers, so doubling is integral.
    let key = |p: Point| -> (i64, i64) { ((p.x * 2.0).round() as i64, (p.y * 2.0).round() as i64) };
    // Every edge indexed by its first point and length, which is enough to shortlist.
    let mut by_len: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, e) in map.edges.iter().enumerate() {
        by_len.entry(e.points.len()).or_default().push(k);
    }
    let mut out: Vec<Option<Pairing>> = vec![None; map.edges.len()];
    for (k, e) in map.edges.iter().enumerate() {
        let n = e.points.len();
        if n == 0 {
            return None;
        }
        let refl: Vec<(i64, i64)> = e.points.iter().map(|&p| key(m.point(p))).collect();
        let cands = by_len.get(&n)?;
        let mut found = None;
        'cand: for &c in cands {
            let q = &map.edges[c];
            if q.closed != e.closed || q.points.len() != n {
                continue;
            }
            let qk: Vec<(i64, i64)> = q.points.iter().map(|&p| key(p)).collect();
            // Where in `q` the reflected first point sits, and which way it then runs.
            let starts: Vec<usize> = if e.closed {
                (0..n).filter(|&i| qk[i] == refl[0]).collect()
            } else {
                // An open boundary can only meet its partner end to end.
                [0, n - 1]
                    .iter()
                    .copied()
                    .filter(|&i| qk[i] == refl[0])
                    .collect()
            };
            for s in starts {
                for &rev in &[false, true] {
                    let idx = |i: usize| -> usize {
                        if rev {
                            (s + n - i % n) % n
                        } else {
                            (s + i) % n
                        }
                    };
                    if !e.closed {
                        // Without wraparound the run has to stay inside the list.
                        let last = if rev {
                            s as i64 - (n as i64 - 1)
                        } else {
                            s as i64 + n as i64 - 1
                        };
                        if last < 0 || last >= n as i64 {
                            continue;
                        }
                    }
                    if (0..n).all(|i| qk[idx(i)] == refl[i]) {
                        found = Some(Pairing {
                            edge: c,
                            rev,
                            shift: s,
                        });
                        break 'cand;
                    }
                }
            }
        }
        out[k] = Some(found?);
    }
    Some(out)
}

/// Find the mirrors of a traced label map and how its boundaries pair off.
///
/// Returns nothing at all unless every boundary pairs cleanly: a mirror that holds on the
/// pixels but not on the extracted boundaries would be a bug, and acting on half of one
/// would be worse than ignoring it.
pub fn detect(map: &PlanarMap, labels: &[u16], ink: &[usize]) -> Symmetry {
    let (w, h) = (map.width, map.height);
    if w == 0 || h == 0 || labels.len() < w * h {
        return Symmetry {
            mirrors: Vec::new(),
            partner: Vec::new(),
        };
    }
    let mut mirrors = Vec::new();
    let mut partner = Vec::new();
    let timing = inkvec_core::env::flag("INKVEC_TIMING");
    let t0 = inkvec_core::clock::Instant::now();
    let found = mirrors_of(labels, ink, w, h);
    if timing {
        eprintln!(
            "  [t] symmetry: mirrors_of {:.1} ms ({} candidates hold)",
            t0.elapsed().as_secs_f64() * 1e3,
            found.len()
        );
    }
    for m in found {
        let t1 = inkvec_core::clock::Instant::now();
        let p = pair_edges(map, m);
        if timing {
            eprintln!(
                "  [t] symmetry: pair_edges {:.1} ms ({} edges)",
                t1.elapsed().as_secs_f64() * 1e3,
                map.edges.len()
            );
        }
        if let Some(p) = p {
            mirrors.push(m);
            partner.push(p);
        }
    }
    Symmetry { mirrors, partner }
}

/// Replace each boundary by the average of itself and its partner's reflection.
///
/// Averaging rather than copying one onto the other keeps the result unbiased: the two
/// sides carry the same evidence and disagree only by whatever tie was broken between
/// them, so the midpoint is the estimate both of them support.
pub fn enforce(map: &mut PlanarMap, sym: &Symmetry) -> usize {
    let mut moved = 0usize;
    for (mi, &m) in sym.mirrors.iter().enumerate() {
        let partner = &sym.partner[mi];
        let before: Vec<Vec<Point>> = map.edges.iter().map(|e| e.points.clone()).collect();
        for (k, e) in map.edges.iter_mut().enumerate() {
            let Some(pr) = partner[k] else { continue };
            let other = &before[pr.edge];
            let n = e.points.len();
            if other.len() != n {
                continue;
            }
            for i in 0..n {
                // Reflect the partner's corresponding point back onto this one, and take
                // the midpoint: both carry the same evidence, so neither should win.
                let q = m.point(other[pr.index(i, n)]);
                let mid = Point::new(0.5 * (e.points[i].x + q.x), 0.5 * (e.points[i].y + q.y));
                if e.points[i].dist(mid) > 1e-12 {
                    moved += 1;
                }
                e.points[i] = mid;
            }
        }
    }
    moved
}

/// The same fitted curve, reflected.
///
/// A reflection reverses handedness, so a circular arc keeps its radius and its
/// large-arc flag and flips its sweep: same circle, same side of the chord, the other way
/// round the plane.
pub fn reflect_path(path: &FittedPath, m: Mirror) -> FittedPath {
    FittedPath {
        start: m.point(path.start),
        segments: path
            .segments
            .iter()
            .map(|s| match *s {
                Segment::Line(p) => Segment::Line(m.point(p)),
                Segment::Cubic(c1, c2, p) => Segment::Cubic(m.point(c1), m.point(c2), m.point(p)),
                // A reflection about either axis keeps the radii and negates the tilt,
                // and reverses the direction of travel around the ellipse.
                Segment::Arc {
                    rx,
                    ry,
                    phi,
                    large_arc,
                    sweep,
                    end,
                } => Segment::Arc {
                    rx,
                    ry,
                    phi: -phi,
                    large_arc,
                    sweep: !sweep,
                    end: m.point(end),
                },
            })
            .collect(),
        closed: path.closed,
    }
}

/// The same primitive, reflected. A circle stays a circle and a rectangle a rectangle;
/// only an ellipse's axis angle has to turn, because a mirror negates the angle it makes
/// with the mirror's own axis.
pub fn reflect_primitive(pf: &PrimitiveFit, m: Mirror) -> PrimitiveFit {
    use std::f64::consts::PI;
    let kind = match pf.kind {
        PrimitiveKind::Circle { c, r } => PrimitiveKind::Circle { c: m.point(c), r },
        PrimitiveKind::Ellipse { c, rx, ry, angle } => PrimitiveKind::Ellipse {
            c: m.point(c),
            rx,
            ry,
            angle: match m {
                Mirror::V(_) => PI - angle,
                Mirror::H(_) => -angle,
            },
        },
        PrimitiveKind::RoundRect { x, y, w, h, rx } => match m {
            Mirror::V(k) => PrimitiveKind::RoundRect {
                x: k as f64 - (x + w),
                y,
                w,
                h,
                rx,
            },
            Mirror::H(k) => PrimitiveKind::RoundRect {
                x,
                y: k as f64 - (y + h),
                w,
                h,
                rx,
            },
        },
    };
    PrimitiveFit {
        kind,
        chi2: pf.chi2,
        params: pf.params,
    }
}

/// Centre a primitive on a mirror it is supposed to lie across.
///
/// A shape that is its own reflection has no partner to copy, but if it was fitted as a
/// circle, an ellipse or a rounded rectangle then being symmetric is one equation on a
/// couple of its numbers, and imposing it is exact. This is where "make the circle round"
/// actually happens: three bars of a glyph come back 21.3, 21.4 and 21.3 pixels wide
/// because each was fitted alone, and the middle one straddles the axis.
pub fn centre_primitive(pf: &PrimitiveFit, m: Mirror) -> PrimitiveFit {
    use std::f64::consts::PI;
    let kind = match pf.kind {
        PrimitiveKind::Circle { c, r } => PrimitiveKind::Circle {
            c: match m {
                Mirror::V(k) => Point::new(k as f64 * 0.5, c.y),
                Mirror::H(k) => Point::new(c.x, k as f64 * 0.5),
            },
            r,
        },
        // An ellipse across a mirror has one axis along it, so the angle is a right angle
        // away from the mirror's own or along it; snap to whichever it is already nearer.
        PrimitiveKind::Ellipse { c, rx, ry, angle } => {
            let snapped = (angle / (PI * 0.5)).round() * PI * 0.5;
            PrimitiveKind::Ellipse {
                c: match m {
                    Mirror::V(k) => Point::new(k as f64 * 0.5, c.y),
                    Mirror::H(k) => Point::new(c.x, k as f64 * 0.5),
                },
                rx,
                ry,
                angle: snapped,
            }
        }
        PrimitiveKind::RoundRect { x, y, w, h, rx } => match m {
            Mirror::V(k) => PrimitiveKind::RoundRect {
                x: (k as f64 - w) * 0.5,
                y,
                w,
                h,
                rx,
            },
            Mirror::H(k) => PrimitiveKind::RoundRect {
                x,
                y: (k as f64 - h) * 0.5,
                w,
                h,
                rx,
            },
        },
    };
    PrimitiveFit {
        kind,
        chi2: pf.chi2,
        params: pf.params,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planar;

    /// Three bars on a background, exactly mirror-symmetric: the mirror must be found, the
    /// boundaries must pair, and a nudge to one side must come back.
    fn bars() -> (Vec<u16>, usize, usize) {
        let (w, h) = (16usize, 8usize);
        let mut labels = vec![0u16; w * h];
        for y in 1..h - 1 {
            for x in [2usize, 3, 7, 8, 12, 13] {
                labels[y * w + x] = 1;
            }
        }
        (labels, w, h)
    }

    #[test]
    fn finds_the_mirror_and_pairs_the_boundaries() {
        let (labels, w, h) = bars();
        let map = planar::build(&labels, w, h, 2);
        let sym = detect(&map, &labels, &[0, 1]);
        // Pixel x maps to 15 - x, so the axis sits between columns 7 and 8.
        assert!(
            sym.mirrors.contains(&Mirror::V(15)),
            "mirrors: {:?}",
            sym.mirrors
        );
        assert!(!sym.is_empty());
    }

    #[test]
    fn averages_a_nudge_back_out() {
        let (labels, w, h) = bars();
        let mut map = planar::build(&labels, w, h, 2);
        let sym = detect(&map, &labels, &[0, 1]);
        assert!(!sym.is_empty());
        let before: Vec<Vec<Point>> = map.edges.iter().map(|e| e.points.clone()).collect();
        // Push every point of one boundary a tenth of a pixel to the right.
        for p in map.edges[0].points.iter_mut() {
            p.x += 0.1;
        }
        enforce(&mut map, &sym);
        // Each side now carries half the nudge, and the pair is an exact reflection.
        let m = sym.mirrors[0];
        for (k, e) in map.edges.iter().enumerate() {
            if let Some((_, pr)) = sym.mirror_of(k) {
                let other = &map.edges[pr.edge].points;
                let n = e.points.len();
                for i in 0..n {
                    let q = m.point(other[pr.index(i, n)]);
                    assert!(
                        e.points[i].dist(q) < 1e-9,
                        "edge {k} point {i}: {:?} vs reflected {:?}",
                        e.points[i],
                        q
                    );
                }
            }
        }
        // And the untouched boundaries did not move.
        assert!(map.edges.len() == before.len());
    }
}
