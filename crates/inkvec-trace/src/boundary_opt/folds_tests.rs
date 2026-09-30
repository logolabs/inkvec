//! The fold counter against the per-cell hash grid it replaced, which is kept here as the
//! reference: the two must return the same integer on every input, since the fold guard's
//! decision (and so the output) depends on nothing else.

use super::super::*;
use super::FoldCounter;
use crate::planar::Edge;
use std::collections::HashMap;

/// The fold count as the stage computed it before the spatial join: bucket every segment in
/// each 1-px cell of its range, test pairs per cell, deduplicate.
fn reference_count(map: &PlanarMap, vars: &Vars, pos: &[Point]) -> usize {
    let mut segs: Vec<(u32, u32)> = Vec::new();
    for (k, e) in map.edges.iter().enumerate() {
        let ids = &vars.var[k];
        let n = ids.len();
        if n < 2 {
            continue;
        }
        let last = if e.closed { n } else { n - 1 };
        for i in 0..last {
            segs.push((ids[i], ids[(i + 1) % n]));
        }
    }
    let mut cells: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
    for (i, &(a, b)) in segs.iter().enumerate() {
        let (p, q) = (pos[a as usize], pos[b as usize]);
        let x0 = (p.x.min(q.x) - 0.5).floor() as i64;
        let x1 = (p.x.max(q.x) + 0.5).ceil() as i64;
        let y0 = (p.y.min(q.y) - 0.5).floor() as i64;
        let y1 = (p.y.max(q.y) + 0.5).ceil() as i64;
        // A degenerate box would put a segment in every cell; the map never has one.
        if (x1 - x0) * (y1 - y0) > 64 {
            continue;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                cells.entry((x, y)).or_default().push(i as u32);
            }
        }
    }
    let mut found: Vec<(u32, u32)> = Vec::new();
    for bucket in cells.values() {
        for (ai, &i) in bucket.iter().enumerate() {
            for &j in bucket[ai + 1..].iter() {
                let (s, t) = (segs[i as usize], segs[j as usize]);
                // Segments sharing a point meet there legitimately.
                if s.0 == t.0 || s.0 == t.1 || s.1 == t.0 || s.1 == t.1 {
                    continue;
                }
                if segments_cross(
                    pos[s.0 as usize],
                    pos[s.1 as usize],
                    pos[t.0 as usize],
                    pos[t.1 as usize],
                ) {
                    found.push((i.min(j), i.max(j)));
                }
            }
        }
    }
    // A segment pair can share more than one pixel, so the same crossing can be seen
    // several times.
    found.sort_unstable();
    found.dedup();
    found.len()
}

/// A small deterministic generator, so the random maps are the same on every run.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn edge(points: Vec<Point>, nodes: (u32, u32), closed: bool) -> Edge {
    let n = points.len();
    Edge {
        points,
        sigma: vec![0.5; n],
        left: 0,
        right: 1,
        start_node: nodes.0,
        end_node: nodes.1,
        closed,
        lambda_scale: 1.0,
    }
}

/// A random map: wiggly open chains between shared nodes and a few closed rings, packed
/// into a small image so that many segments share cells. With `snap`, coordinates are
/// rounded to the half-pixel grid, so collinear overlaps, touches and points exactly on
/// gridlines are common.
fn random_map(rng: &mut Lcg, size: f64, snap: bool) -> PlanarMap {
    let q = |v: f64| if snap { (v * 2.0).round() / 2.0 } else { v };
    let mut edges = Vec::new();
    let nodes: Vec<Point> = (0..6)
        .map(|_| Point::new(q(rng.next() * size), q(rng.next() * size)))
        .collect();
    for k in 0..8 {
        let (a, b) = (k % 6, (k * 5 + 1) % 6);
        let (pa, pb) = (nodes[a], nodes[b]);
        let m = 3 + (rng.next() * 12.0) as usize;
        let mut pts = vec![pa];
        for i in 1..m {
            let t = i as f64 / m as f64;
            pts.push(Point::new(
                q(pa.x + (pb.x - pa.x) * t + (rng.next() - 0.5) * 3.0),
                q(pa.y + (pb.y - pa.y) * t + (rng.next() - 0.5) * 3.0),
            ));
        }
        pts.push(pb);
        edges.push(edge(pts, (a as u32, b as u32), false));
    }
    for r in 0..3 {
        let (cx, cy) = (rng.next() * size, rng.next() * size);
        let m = 4 + (rng.next() * 10.0) as usize;
        let pts: Vec<Point> = (0..m)
            .map(|i| {
                let a = i as f64 / m as f64 * std::f64::consts::TAU;
                let rad = 1.0 + rng.next() * 3.0;
                Point::new(q(cx + rad * a.cos()), q(cy + rad * a.sin()))
            })
            .collect();
        edges.push(edge(pts, (100 + r, 100 + r), true));
    }
    PlanarMap {
        edges,
        width: size as usize + 1,
        height: size as usize + 1,
        n_labels: 2,
    }
}

#[test]
fn fold_counter_equals_the_hash_grid_on_random_maps() {
    let mut rng = Lcg(7);
    let mut crossing_maps = 0;
    for trial in 0..400 {
        let snap = trial % 2 == 0;
        let size = [6.0, 12.0, 30.0][trial % 3];
        let map = random_map(&mut rng, size, snap);
        let vars = build_vars(&map);
        // A solution up to a pixel from the start, as the solve produces.
        let sol: Vec<Point> = vars
            .start
            .iter()
            .map(|p| {
                let (a, r) = (rng.next() * std::f64::consts::TAU, rng.next());
                let d = Point::new(p.x + r * a.cos(), p.y + r * a.sin());
                if snap {
                    Point::new((d.x * 2.0).round() / 2.0, (d.y * 2.0).round() / 2.0)
                } else {
                    d
                }
            })
            .collect();
        let fc = FoldCounter::new(&map, &vars, &vars.start, &sol);
        let mut scale = 1.0;
        for _ in 0..5 {
            let pos: Vec<Point> = vars
                .start
                .iter()
                .zip(&sol)
                .map(|(s, p)| Point::new(s.x + (p.x - s.x) * scale, s.y + (p.y - s.y) * scale))
                .collect();
            let want = reference_count(&map, &vars, &pos);
            assert_eq!(fc.count(&pos), want, "trial {trial} scale {scale}");
            crossing_maps += (want > 0) as usize;
            scale *= 0.5;
        }
        assert_eq!(
            fc.count(&vars.start),
            reference_count(&map, &vars, &vars.start)
        );
        assert_eq!(fc.count(&sol), reference_count(&map, &vars, &sol));
    }
    // The random maps must actually exercise crossings, not only their absence.
    assert!(crossing_maps > 200, "{crossing_maps}");
}

#[test]
fn fold_counter_equals_the_hash_grid_on_degenerate_segments() {
    let p = Point::new;
    // A segment far larger than the 64-cell limit, which neither counts; a zero-length
    // segment; collinear overlaps; a touch on a gridline; far from the origin.
    let edges = vec![
        edge(vec![p(0.0, 0.0), p(90.0, 0.2), p(1.0, 1.0)], (0, 1), false),
        edge(vec![p(2.0, 2.0), p(2.0, 2.0), p(4.0, 2.0)], (2, 3), false),
        edge(vec![p(3.0, 2.0), p(5.0, 2.0)], (4, 5), false),
        edge(vec![p(3.5, 0.5), p(3.5, 2.0), p(3.5, 4.5)], (6, 7), false),
        edge(vec![p(1.0, -3.0), p(1.0, 5.0)], (8, 9), false),
        edge(
            vec![
                p(300.5, 300.5),
                p(302.5, 300.5),
                p(300.5, 302.5),
                p(302.5, 302.5),
            ],
            (10, 10),
            true,
        ),
    ];
    let map = PlanarMap {
        edges,
        width: 400,
        height: 400,
        n_labels: 2,
    };
    let vars = build_vars(&map);
    let want = reference_count(&map, &vars, &vars.start);
    assert!(want > 0);
    let fc = FoldCounter::new(&map, &vars, &vars.start, &vars.start);
    assert_eq!(fc.count(&vars.start), want);
}
