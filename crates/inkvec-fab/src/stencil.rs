//! Stencils: the sheet with the artwork cut out, held together by bridges.
//!
//! Cut the artwork out of a sheet and every enclosed counter — the middle of an O, the eye
//! of an e, a ring inside a ring — is a loose island that falls out and takes that part of
//! the design with it. Stencil makers keep islands attached with bridges: thin strips of
//! sheet left uncut across the artwork. Which bridges to add is a spanning-tree problem
//! (Bronson, Rheingans and Walsh, "Semi-automatic stencil creation through error
//! minimization", NPAR 2008 and C&G 2015, formulate it over region adjacency): here every
//! island is joined Prim-style, shortest first, to the material already connected to the
//! sheet's frame, so the bridges added are few and short and each crosses the least
//! artwork there is to cross.

use crate::geom::{self, Contour, Pt, Region, Shape};

/// Islands larger than this, in square millimetres, hang from two bridges: one bridge is a
/// hinge, and a large island on a hinge lifts and flaps under spray.
pub const TWO_BRIDGE_AREA_MM2: f64 = 100.0;

/// A bridge: the two points it joins, on the island and on the connected material.
#[derive(Clone, Copy, Debug)]
pub struct Bridge {
    /// End on the island.
    pub from: Pt,
    /// End on the material it is joined to.
    pub to: Pt,
}

/// Points along a shape's contours, no further apart than `step`.
fn samples(s: &Shape, step: f64) -> Vec<Pt> {
    let mut out = Vec::new();
    for c in s {
        for i in 0..c.len() {
            let (a, b) = (c[i], c[(i + 1) % c.len()]);
            let n = ((b[0] - a[0]).hypot(b[1] - a[1]) / step).ceil().max(1.0) as usize;
            for k in 0..n {
                let t = k as f64 / n as f64;
                out.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
            }
        }
    }
    out
}

fn nearest(a: &[Pt], b: &[Pt]) -> (f64, Pt, Pt) {
    let mut best = (f64::INFINITY, [0.0; 2], [0.0; 2]);
    for p in a {
        for q in b {
            let d = (p[0] - q[0]).hypot(p[1] - q[1]);
            if d < best.0 {
                best = (d, *p, *q);
            }
        }
    }
    best
}

/// A strip `width` wide from `from` to `to`, run a little into both ends so it joins what
/// it bridges rather than touching it.
fn strip(from: Pt, to: Pt, width: f64) -> Region {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let l = dx.hypot(dy).max(1e-9);
    let ext = width / 2.0;
    let a = [from[0] - dx / l * ext, from[1] - dy / l * ext];
    let z = [to[0] + dx / l * ext, to[1] + dy / l * ext];
    geom::stroke(&[a, z], width, false)
}

/// The stencil sheet for `art` inside a frame `margin` wide, with bridges `width` wide
/// joining every island to the frame. Returns the sheet and the bridges it needed.
pub fn stencil(art: &Region, margin: f64, width: f64) -> (Region, Vec<Bridge>) {
    let Some(b) = geom::bounds(art) else {
        return (Vec::new(), Vec::new());
    };
    let frame = geom::rect(b[0] - margin, b[1] - margin, b[2] + margin, b[3] + margin);
    let sheet = geom::difference(&frame, art);
    // The piece that touches the frame's edge is the sheet; every other is an island.
    let touches = |s: &Shape| {
        s.first().is_some_and(|c: &Contour| {
            c.iter().any(|p| {
                (p[0] - (b[0] - margin)).abs() < 1e-6 || (p[1] - (b[1] - margin)).abs() < 1e-6
            })
        })
    };
    let (mut connected, mut islands): (Vec<Shape>, Vec<Shape>) =
        sheet.iter().cloned().partition(|s| touches(s));
    if connected.is_empty() {
        return (sheet, Vec::new());
    }
    let step = (width / 2.0).max(0.2);
    let mut connected_pts: Vec<Pt> = connected.iter().flat_map(|s| samples(s, step)).collect();
    let mut bridges = Vec::new();
    let mut strips: Vec<Region> = Vec::new();
    while !islands.is_empty() {
        // Prim: the island closest to anything already connected goes next.
        let (k, (_, from, to)) = islands
            .iter()
            .enumerate()
            .map(|(k, s)| (k, nearest(&samples(s, step), &connected_pts)))
            .min_by(|x, y| x.1 .0.total_cmp(&y.1 .0))
            .expect("islands is not empty");
        let island = islands.swap_remove(k);
        strips.push(strip(from, to, width));
        bridges.push(Bridge { from, to });
        // A large island gets a second bridge, at least a quarter turn round from the first.
        if geom::shape_area(&island) > TWO_BRIDGE_AREA_MM2 {
            let pts = samples(&island, step);
            let n = pts.len().max(1) as f64;
            let c = pts
                .iter()
                .fold([0.0, 0.0], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n]);
            let (ux, uy) = (from[0] - c[0], from[1] - c[1]);
            let apart: Vec<Pt> = pts
                .into_iter()
                .filter(|p| {
                    let (vx, vy) = (p[0] - c[0], p[1] - c[1]);
                    ux * vx + uy * vy <= 0.0
                })
                .collect();
            if !apart.is_empty() {
                let (_, f2, t2) = nearest(&apart, &connected_pts);
                strips.push(strip(f2, t2, width));
                bridges.push(Bridge { from: f2, to: t2 });
            }
        }
        connected_pts.extend(samples(&island, step));
        connected.push(island);
    }
    let bridged = geom::union(&sheet, &geom::union_all(strips.iter()));
    (bridged, bridges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counter_of_an_o_is_bridged_to_the_sheet() {
        // An "O": a 20 mm square ring 4 mm thick.
        let o = geom::difference(
            &geom::rect(0.0, 0.0, 20.0, 20.0),
            &geom::rect(4.0, 4.0, 16.0, 16.0),
        );
        let (sheet, bridges) = stencil(&o, 5.0, 1.0);
        // The counter is 144 mm², over the two-bridge threshold.
        assert_eq!(bridges.len(), 2, "one large island, two bridges");
        assert_eq!(sheet.len(), 1, "the sheet is one piece");
        // Both cross the 4 mm ring, on different sides.
        for b in &bridges {
            let d = (b.from[0] - b.to[0]).hypot(b.from[1] - b.to[1]);
            assert!((d - 4.0).abs() < 0.3, "{d}");
        }
        let (u, v) = (bridges[0].from, bridges[1].from);
        let (cu, cv) = ([u[0] - 10.0, u[1] - 10.0], [v[0] - 10.0, v[1] - 10.0]);
        assert!(
            cu[0] * cv[0] + cu[1] * cv[1] <= 0.0,
            "a quarter turn or more apart"
        );
    }

    #[test]
    fn artwork_without_counters_needs_no_bridges() {
        let (sheet, bridges) = stencil(&geom::rect(0.0, 0.0, 10.0, 10.0), 5.0, 1.0);
        assert!(bridges.is_empty());
        assert_eq!(sheet.len(), 1);
    }
}
