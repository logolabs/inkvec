//! What the boundary chain reports where runs end: corner proposals along runs, junctions
//! with their arms in cyclic order and candidate continuations, and which edges share the
//! renderer floor's per-edge offset (`docs/theory/chain-boundary.md`, B3; amendments A3, A4).

use std::f64::consts::PI;

use inkvec_core::likelihood::{Arm, CornerProposal, JunctionReport, RunObs};
use inkvec_core::Point;

use crate::planar::PlanarMap;

use super::windows::crossing;

/// The fourth difference `h₋₂ − 4h₋₁ + 6h₀ − 4h₁ + h₂` of five consecutive windows' mean
/// positions annihilates every cubic (so it is zero on a smooth run to the quartic term) and
/// has variance `Σ wᵢ² Vᵢ`, `70 V` for equal variances; a slope change `Δs` at its centre gives
/// it `1.25 Δs` at most. `strip.rs`'s one-sided corner test is this divided by 12. A window
/// whose `|D|` is at least `z_min` standard deviations and a local maximum along the run is
/// proposed; a constant or linear bias shared by the windows (the renderer floor's per-edge
/// offset) cancels in `D`.
pub(super) fn corners(edge: u32, obs: &[RunObs], closed: bool, z_min: f64) -> Vec<CornerProposal> {
    const W: [f64; 5] = [1.0, -4.0, 6.0, -4.0, 1.0];
    let n = obs.len();
    let mut z = vec![0.0f64; n];
    if n < 5 {
        return Vec::new();
    }
    // On a closed edge the stencil wraps round its seam.
    let centres: Vec<usize> = if closed {
        (0..n).collect()
    } else {
        (2..n - 2).collect()
    };
    for i in centres {
        let five: Vec<RunObs> = (0..5).map(|k| obs[(i + n + k - 2) % n]).collect();
        let five = &five[..];
        let a0 = five[0].window.axis;
        let consecutive = five
            .windows(2)
            .all(|p| p[1].window.axis == a0 && (p[1].window.line - p[0].window.line).abs() == 1)
            && five.iter().all(|o| o.window.axis == a0);
        let monotone = (five[1].window.line - five[0].window.line)
            == (five[4].window.line - five[3].window.line);
        if !consecutive || !monotone {
            continue;
        }
        let d: f64 = five.iter().zip(W).map(|(o, w)| w * o.mean_position()).sum();
        let v: f64 = five.iter().zip(W).map(|(o, w)| w * w * o.var).sum();
        z[i] = d.abs() / v.max(1e-300).sqrt();
    }
    let mut out = Vec::new();
    for i in 0..n {
        if z[i] < z_min {
            continue;
        }
        let left = if i > 0 {
            z[i - 1]
        } else if closed {
            z[n - 1]
        } else {
            0.0
        };
        let right = if i + 1 < n {
            z[i + 1]
        } else if closed {
            z[0]
        } else {
            0.0
        };
        // The checker generated from `cornerExcessK` must agree that |D| ≥ z_min·σ_D.
        let five: Vec<&RunObs> = (0..5).map(|k| &obs[(i + n + k - 2) % n]).collect();
        let h = [0, 1, 2, 3, 4].map(|k| five[k].mean_position());
        let v = [0, 1, 2, 3, 4].map(|k| five[k].var);
        if z[i] >= left && z[i] > right && super::checks::certify_corner(h, v, z_min) {
            out.push(CornerProposal {
                edge,
                index: i,
                z: z[i],
                at: crossing(&obs[i]),
            });
        }
    }
    out
}

/// Whether an edge's windows run within one lattice step of an axis or a diagonal (`n`
/// samples per pixel): there the renderer's per-sample-column rounding errors coincide and
/// add to one offset per edge. Decided by the majority of its windows, from the slope
/// between neighbouring windows' mean positions.
pub(super) fn on_lattice(obs: &[RunObs], n: u32) -> bool {
    let step = 1.0 / n.max(1) as f64;
    let (mut near, mut all) = (0usize, 0usize);
    for p in obs.windows(2) {
        let (a, b) = (&p[0], &p[1]);
        if a.window.axis != b.window.axis || (b.window.line - a.window.line).abs() != 1 {
            continue;
        }
        let t = (b.mean_position() - a.mean_position()).abs();
        all += 1;
        if t < step || (t - 1.0).abs() < step {
            near += 1;
        }
    }
    all > 0 && 2 * near >= all
}

/// One arm's outward direction and its standard deviation, from the edge's points within
/// `reach` px of the junction (a weighted least-squares line through them, its direction
/// pointing away from the node), or `None` when fewer than three points lie there.
fn arm(
    map: &PlanarMap,
    e: usize,
    at_start: bool,
    node: Point,
    reach: f64,
    sigma: f64,
) -> Option<Arm> {
    let pts = &map.edges[e].points;
    let ordered: Vec<Point> = if at_start {
        pts.to_vec()
    } else {
        pts.iter().rev().copied().collect()
    };
    let near: Vec<Point> = ordered
        .iter()
        .skip(1)
        .take_while(|p| p.dist(node) <= reach)
        .copied()
        .collect();
    if near.len() < 3 {
        return None;
    }
    let n = near.len() as f64;
    let (mx, my) = near
        .iter()
        .fold((0.0, 0.0), |(a, b), p| (a + p.x / n, b + p.y / n));
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in &near {
        let (dx, dy) = (p.x - mx, p.y - my);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let th = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (mut ux, mut uy) = (th.cos(), th.sin());
    // Point away from the node.
    if (mx - node.x) * ux + (my - node.y) * uy < 0.0 {
        ux = -ux;
        uy = -uy;
    }
    // Spread along the line and residual across it: the slope's standard error.
    let along: f64 = near
        .iter()
        .map(|p| ((p.x - mx) * ux + (p.y - my) * uy).powi(2))
        .sum();
    let across: f64 = near
        .iter()
        .map(|p| ((p.x - mx) * -uy + (p.y - my) * ux).powi(2))
        .sum();
    let resid_var = (across / (n - 2.0).max(1.0)).max(sigma * sigma);
    let sd = (resid_var / along.max(1e-12)).sqrt().max(1e-4);
    Some(Arm {
        edge: e as u32,
        at_start,
        direction: uy.atan2(ux),
        direction_sd: sd,
    })
}

/// Junction reports: every node where three or more edge ends meet, its arms in increasing
/// direction, and each pair of arms leaving in opposite directions to within `z_max`
/// standard deviations (a curve that may continue through the junction, R2.9).
pub(super) fn junctions(
    map: &PlanarMap,
    reach: f64,
    sigma: f64,
    z_max: f64,
) -> Vec<JunctionReport> {
    use std::collections::BTreeMap;
    let mut ends: BTreeMap<u32, Vec<(usize, bool)>> = BTreeMap::new();
    for (k, e) in map.edges.iter().enumerate() {
        if e.closed || e.points.len() < 2 {
            continue;
        }
        ends.entry(e.start_node).or_default().push((k, true));
        ends.entry(e.end_node).or_default().push((k, false));
    }
    let mut out = Vec::new();
    for (node, list) in ends {
        if list.len() < 3 {
            continue;
        }
        let first = list[0];
        let e0 = &map.edges[first.0];
        let at = if first.1 {
            e0.points[0]
        } else {
            *e0.points.last().expect("two points")
        };
        let mut arms: Vec<Arm> = list
            .iter()
            .filter_map(|&(e, s)| arm(map, e, s, at, reach, sigma))
            .collect();
        arms.sort_by(|a, b| a.direction.total_cmp(&b.direction));
        let mut cont = Vec::new();
        for i in 0..arms.len() {
            for j in i + 1..arms.len() {
                let mut d = arms[j].direction - arms[i].direction - PI;
                d = (d + PI).rem_euclid(2.0 * PI) - PI;
                let sd = arms[i].direction_sd.hypot(arms[j].direction_sd);
                let z = d.abs() / sd;
                if z <= z_max {
                    cont.push((i, j, z));
                }
            }
        }
        out.push(JunctionReport {
            node,
            at,
            arms,
            continuations: cont,
        });
    }
    out
}

/// Boundary length of the starting geometry inside a window, from its windows' slope:
/// `√(1 + t²)` for a slope `t` across the strip (B2.5).
pub(super) fn window_lengths(obs: &[RunObs]) -> Vec<f64> {
    let n = obs.len();
    (0..n)
        .map(|i| {
            let slope = |a: &RunObs, b: &RunObs| {
                if a.window.axis == b.window.axis && (b.window.line - a.window.line).abs() == 1 {
                    Some((b.mean_position() - a.mean_position()).abs())
                } else {
                    None
                }
            };
            let t = match (
                (i > 0).then(|| slope(&obs[i - 1], &obs[i])).flatten(),
                (i + 1 < n).then(|| slope(&obs[i], &obs[i + 1])).flatten(),
            ) {
                (Some(a), Some(b)) => 0.5 * (a + b),
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => 0.0,
            };
            (1.0 + t * t).sqrt()
        })
        .collect()
}
