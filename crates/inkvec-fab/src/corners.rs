//! Inside corners a router bit can reach: dogbones.
//!
//! A router bit is round. Cutting around a part it leaves every inside corner filled with
//! a fillet of the bit's radius, so a square tenon will not seat in a square mortise cut
//! with the same bit. The standard cure is a *dogbone*: at each inside corner the cut runs
//! on, along the corner's bisector, until the bit's edge touches the drawn corner. Its
//! centre then sits one radius from the corner, inside the material, and the corner is
//! cleared to the point.
//!
//! A corner is found by the turn of the outline measured across a short window rather than
//! at one vertex, so a curve flattened into many small turns is not mistaken for corners,
//! and it is an inside corner when the outline turns away from the material there. Holes
//! count too: every corner of a square hole is an inside corner of the plate.

use crate::geom::{self, Pt, Region};

/// Least turn, degrees, across [`TURN_WINDOW_MM`] either side, for a corner.
const MIN_TURN_DEG: f64 = 30.0;
/// How far either side of a vertex its turn is measured, millimetres. Small, so the turn
/// has to be concentrated to count: a circle turns 30 degrees over this only when its
/// radius is under a millimetre, while across a bit's radius a small round hole would
/// read as a ring of corners.
const TURN_WINDOW_MM: f64 = 0.25;

fn sub(a: Pt, b: Pt) -> Pt {
    [a[0] - b[0], a[1] - b[1]]
}
fn unit(a: Pt) -> Option<Pt> {
    let n = a[0].hypot(a[1]);
    (n > 1e-12).then(|| [a[0] / n, a[1] / n])
}

/// The point `reach` along the closed contour `c` from vertex `i`, forwards or back.
fn walk(c: &[Pt], i: usize, reach: f64, forward: bool) -> Pt {
    let n = c.len();
    let mut at = c[i];
    let mut left = reach;
    let mut k = i;
    for _ in 0..n {
        let next = if forward {
            (k + 1) % n
        } else {
            (k + n - 1) % n
        };
        let d = sub(c[next], at);
        let len = d[0].hypot(d[1]);
        if len >= left {
            return [at[0] + d[0] * left / len, at[1] + d[1] * left / len];
        }
        left -= len;
        at = c[next];
        k = next;
    }
    at
}

/// Inside corners of `r`: each corner point and the unit direction into the material
/// along its bisector. Corners closer together than `window` count once.
pub fn inside_corners(r: &Region, window: f64) -> Vec<(Pt, Pt)> {
    let mut out = Vec::new();
    for shape in r {
        // Material lies on the same side of every contour of a shape as it does of the
        // outer one: the outer's orientation says which turns are away from it.
        let Some(outer) = shape.first() else { continue };
        let side = geom::contour_area(outer).signum();
        for c in shape {
            let n = c.len();
            if n < 3 {
                continue;
            }
            // Turn at every vertex across the window, then the corners: turns past the
            // threshold that are the sharpest within the window around them.
            let turns: Vec<(f64, Pt, Pt)> = (0..n)
                .map(|i| {
                    let (a, b) = (
                        walk(c, i, TURN_WINDOW_MM, false),
                        walk(c, i, TURN_WINDOW_MM, true),
                    );
                    let (Some(u), Some(v)) = (unit(sub(c[i], a)), unit(sub(b, c[i]))) else {
                        return (0.0, [0.0, 0.0], [0.0, 0.0]);
                    };
                    let cross = u[0] * v[1] - u[1] * v[0];
                    let dot = u[0] * v[0] + u[1] * v[1];
                    (cross.atan2(dot), u, v)
                })
                .collect();
            let min = MIN_TURN_DEG.to_radians();
            for i in 0..n {
                let (turn, u, v) = turns[i];
                // Turning away from the material: against the outer contour's own sense.
                if turn * side >= 0.0 || turn.abs() < min {
                    continue;
                }
                // Keep only the sharpest vertex within the window: a corner rounded by the
                // flattening turns at several vertices, and gets one dogbone. Ties go to the
                // first, so walking forward a tie loses and walking back it wins.
                let len = |j: usize| {
                    let d = sub(c[j], c[(j + n - 1) % n]);
                    d[0].hypot(d[1])
                };
                let sharper = |j: usize, tie: bool| {
                    let t = turns[j].0.abs();
                    t > turn.abs() || (tie && t == turn.abs())
                };
                let mut best = true;
                let (mut ahead, mut behind) = (0.0, 0.0);
                for step in 1..n {
                    let (f, r) = ((i + step) % n, (i + n - step) % n);
                    ahead += len(f);
                    behind += len((r + 1) % n);
                    if (ahead <= window && sharper(f, false))
                        || (behind <= window && sharper(r, true))
                    {
                        best = false;
                        break;
                    }
                    if ahead > window && behind > window {
                        break;
                    }
                }
                if !best {
                    continue;
                }
                if let Some(into) = unit(sub(u, v)) {
                    out.push((c[i], into));
                }
            }
        }
    }
    out
}

/// `r` with a dogbone of `radius` cut at every inside corner.
pub fn dogbones(r: &Region, radius: f64) -> (Region, usize) {
    if radius <= 0.0 {
        return (r.clone(), 0);
    }
    let corners = inside_corners(r, radius);
    if corners.is_empty() {
        return (r.clone(), 0);
    }
    let discs: Vec<Region> = corners
        .iter()
        .map(|(p, into)| {
            let c = [p[0] + into[0] * radius, p[1] + into[1] * radius];
            let n = 48;
            vec![vec![(0..n)
                .map(|k| {
                    let t = k as f64 / n as f64 * std::f64::consts::TAU;
                    // A hair larger than the bit, so the corner itself is cut through.
                    let rr = radius * 1.001;
                    [c[0] + rr * t.cos(), c[1] + rr * t.sin()]
                })
                .collect()]]
        })
        .collect();
    (
        geom::difference(r, &geom::union_all(discs.iter())),
        corners.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_hole_gets_four_dogbones_and_a_square_part_none() {
        let plate = geom::difference(
            &geom::rect(0.0, 0.0, 40.0, 40.0),
            &geom::rect(10.0, 10.0, 30.0, 30.0),
        );
        let corners = inside_corners(&plate, 3.0);
        assert_eq!(corners.len(), 4, "{corners:?}");
        // Each points into the plate, away from the hole's middle.
        for (p, into) in &corners {
            let away = [p[0] - 20.0, p[1] - 20.0];
            assert!(away[0] * into[0] + away[1] * into[1] > 0.0);
        }
        let (cut, n) = dogbones(&plate, 3.0);
        assert_eq!(n, 4);
        // The corner itself is now open: a point just inside the plate at the corner is cut.
        assert!(!geom::contains(&cut, [9.9, 9.9]));
        assert!(
            geom::contains(&cut, [5.0, 20.0]),
            "the rest of the plate stays"
        );
        assert!(inside_corners(&geom::rect(0.0, 0.0, 10.0, 10.0), 3.0).is_empty());
    }

    #[test]
    fn an_l_has_one_inside_corner_and_a_disc_has_none() {
        let l = geom::union(
            &geom::rect(0.0, 0.0, 30.0, 10.0),
            &geom::rect(0.0, 0.0, 10.0, 30.0),
        );
        let c = inside_corners(&l, 2.0);
        assert_eq!(c.len(), 1, "{c:?}");
        assert!((c[0].0[0] - 10.0).abs() < 1e-6 && (c[0].0[1] - 10.0).abs() < 1e-6);
        let disc: Region = vec![vec![(0..720)
            .map(|i| {
                let t = (i as f64 / 2.0).to_radians();
                [20.0 + 10.0 * t.cos(), 20.0 + 10.0 * t.sin()]
            })
            .collect()]];
        let ring = geom::difference(&geom::rect(0.0, 0.0, 40.0, 40.0), &disc);
        // A round hole narrower than the bit (a 2.6 mm eye, a 3 mm bit) is not a ring of
        // corners either, however small against the dogbone window.
        let eye: Region = vec![vec![(0..96)
            .map(|i| {
                let t = (i as f64 * 3.75).to_radians();
                [20.0 + 1.3 * t.cos(), 20.0 + 1.3 * t.sin()]
            })
            .collect()]];
        let plate = geom::difference(&geom::rect(0.0, 0.0, 40.0, 40.0), &eye);
        assert!(
            inside_corners(&plate, 1.5).is_empty(),
            "the eye has no corners"
        );
        assert!(
            inside_corners(&ring, 2.0).is_empty(),
            "a round hole has no corners"
        );
    }
}
