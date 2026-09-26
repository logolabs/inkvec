//! Rings as geometry: which contains which, which way round, and where they cross.
//!
//! A face is a set of rings and the emitter has to know how they nest — an even-odd
//! compound path is only correct if the parity of each ring is. These answer that from the
//! fitted curves themselves rather than from the planar map, because by this point the
//! curves are what will actually be drawn and they can differ from the polyline the map
//! recorded by a fraction of a pixel.

use crate::{FaceRings, Ring};
use inkvec_core::Point;
use inkvec_fit::{curves::Segment, multimodel, simple, FitConfig, FittedPath};

/// Containment forest over faces: `parent[i]` is the smallest face strictly containing
/// face `i`, if any.
///
/// This is structure the planar map already knows and the output was throwing away. An
/// artist's file is a tree — a body with its markings nested inside it — and 42% of the
/// artist-authored SVGs surveyed use `<g>`. Emitting a flat list of paths renders
/// identically and is materially worse to work with: selecting a shape and its
/// decorations together becomes a manual rubber-band instead of one click.
pub(crate) fn containment(pts: &[Vec<Vec<Point>>], outer: &[Vec<usize>]) -> Vec<Option<usize>> {
    let n = pts.len();
    let area: Vec<f64> = (0..n)
        .map(|i| outer[i].iter().map(|&k| ring_area(&pts[i][k])).sum())
        .collect();

    let mut parent = vec![None; n];
    for i in 0..n {
        if outer[i].is_empty() {
            continue;
        }
        let probe = &pts[i][outer[i][0]];
        let mut best: Option<(usize, f64)> = None;
        for j in 0..n {
            if i == j || outer[j].is_empty() || area[j] <= area[i] {
                continue;
            }
            let inside = outer[j]
                .iter()
                .any(|&k| pts[j][k].len() >= 3 && ring_inside(probe, &pts[j][k]));
            if inside && best.is_none_or(|(_, a)| area[j] < a) {
                best = Some((j, area[j]));
            }
        }
        parent[i] = best.map(|(j, _)| j);
    }
    parent
}

/// Refit whichever edges take part in a self-crossing, under a tightening span cap, until
/// the assembled rings stop crossing themselves.
///
/// An edge is shared by the two faces either side of it, and it is refitted *once* — so
/// both faces continue to reference the same curve and the property the planar map exists
/// to guarantee is preserved. Tightening cannot fail to terminate: at a cap of one, an
/// edge's fit reproduces its measured polyline, and the measured boundary of a face on a
/// partition is simple.
pub(crate) fn repair_ring_crossings(
    order: &[FaceRings],
    fitted: &mut [FittedPath],
    polys: &[inkvec_core::Polyline],
    cfg: &FitConfig,
) -> usize {
    const ROUNDS: usize = 10;
    // Keep the unconstrained optimum. A cap is a topology emergency brake, not a better
    // description of the boundary; after the offending neighbours have been repaired we
    // can often put this compact path back without bringing the crossing with it.
    let full_fit = fitted.to_vec();
    let mut cap: Vec<usize> = polys.iter().map(|p| p.len().max(2)).collect();
    let mut repaired = 0usize;
    // Vertices of every boundary the loop refitted, for the merge pass afterwards.
    let mut refit_vertices: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();

    use rayon::prelude::*;
    let rings: Vec<&Ring> = order.iter().flatten().collect();
    let mut changed: Option<std::collections::HashSet<usize>> = None;
    for _ in 0..ROUNDS {
        let round_t = inkvec_core::clock::Instant::now();
        // Detection per ring and refits per edge are both independent; run each wave on
        // every core. The refits are the expensive half — a capped refit re-runs the
        // whole dynamic program on that boundary.
        let mut guilty: Vec<usize> = rings
            .par_iter()
            .filter(|ring| {
                // After the first round only rings touching a refitted edge can have
                // changed; re-testing the rest re-derives the same answer at full price.
                changed
                    .as_ref()
                    .is_none_or(|c| ring.iter().any(|&(k, _)| c.contains(&k)))
            })
            .flat_map_iter(|ring| {
                let mut g: Vec<usize> = Vec::new();
                if ring.len() >= 2 {
                    let (path, owner) = ring_as_path(ring, fitted);
                    for (i, j) in simple::self_crossings(&path, 32) {
                        if let Some(&a) = owner.get(i) {
                            g.push(a);
                        }
                        if let Some(&b) = owner.get(j) {
                            g.push(b);
                        }
                    }
                }
                g
            })
            .collect();
        guilty.sort_unstable();
        guilty.dedup();
        guilty.retain(|&k| cap[k] > 1);
        if guilty.is_empty() {
            break;
        }
        if inkvec_core::env::flag("INKVEC_TIMING") {
            let sizes: Vec<usize> = guilty.iter().map(|&k| polys[k].len()).collect();
            eprintln!(
                "  [t] repair round: {} guilty, sizes {:?}, detect {:.1} ms",
                guilty.len(),
                sizes,
                round_t.elapsed().as_secs_f64() * 1e3
            );
        }
        let refits: Vec<(usize, multimodel::MultimodelFit)> = guilty
            .par_iter()
            .map(|&k| {
                let c = (cap[k] / 2).max(1);
                (
                    k,
                    multimodel::optimal_multimodel_capped_full(&polys[k], cfg, c),
                )
            })
            .collect();
        for (k, f) in refits {
            cap[k] = (cap[k] / 2).max(1);
            fitted[k] = f.path;
            refit_vertices.insert(k, f.vertices);
            repaired += 1;
        }
        if inkvec_core::env::flag("INKVEC_TIMING") {
            eprintln!(
                "  [t] repair round total {:.1} ms",
                round_t.elapsed().as_secs_f64() * 1e3
            );
        }
        changed = Some(guilty.iter().copied().collect());
    }

    // A capped refit is the constrained program's raw answer: chords and G1 cubics
    // with the corner chamfers left in. Under the cap the merge pass was skipped
    // because it re-joined runs into cubics that crossed again and its cost grew
    // with the segment count. Now that the rings are simple, merge and sharpen each
    // refitted boundary once, and keep the result only where the rings it belongs to
    // stay simple. Without this, 1f9d1-1f3ff-200d-1f680 and 1f640 (twemoji) came
    // back faceted at +0.12 and +0.10 dE00.
    if !refit_vertices.is_empty() {
        let merge_t = inkvec_core::clock::Instant::now();
        // The merge searches a grid of tangent directions and arm lengths per candidate
        // run, which costs about eight milliseconds for every segment it looks at. That is
        // affordable on the handful of segments a normal repair touches and not affordable
        // at all on a capped refit of a boundary with hundreds of them: on
        // simple-icons/biome it was 4.8 s of a 5.8 s trace.
        //
        // So the pass gets a budget, counted in segments and spent shortest boundary
        // first, which is deterministic and keeps the case it was written for. A boundary
        // it cannot afford keeps its capped refit: chords and G1 cubics with the corner
        // chamfers in, which is safe and merely more faceted than it could be.
        const MERGE_BUDGET: usize = 96;
        // And no single boundary may exceed the budget on its own: the pathological case
        // *is* one boundary with hundreds of segments, so an escape hatch that always
        // merges the first one would keep paying exactly the cost this avoids.
        let mut affordable: Vec<(usize, &Vec<usize>)> =
            refit_vertices.iter().map(|(&k, v)| (k, v)).collect();
        affordable.sort_by_key(|&(k, _)| (fitted[k].segments.len(), k));
        let mut spent = 0usize;
        affordable.retain(|&(k, _)| {
            let n = fitted[k].segments.len();
            if n <= MERGE_BUDGET && spent + n <= 4 * MERGE_BUDGET {
                spent += n;
                true
            } else {
                false
            }
        });
        if inkvec_core::env::flag("INKVEC_TIMING") && affordable.len() < refit_vertices.len() {
            eprintln!(
                "  [t] repair merge budget: {} of {} boundary/ies, {} segment(s)",
                affordable.len(),
                refit_vertices.len(),
                spent
            );
        }
        let merged: Vec<(usize, FittedPath)> = affordable
            .par_iter()
            .map(|&(k, verts)| {
                let mut path = fitted[k].clone();
                inkvec_fit::merge::merge_free_cubics(&mut path, &polys[k], verts, cfg);
                inkvec_fit::merge::sharpen_corners(&mut path);
                (k, path)
            })
            .collect();
        // Test candidates one at a time. The previous all-at-once trial was needlessly
        // pessimistic: one unsafe cubic caused every other candidate in the same face
        // ring to be discarded too, leaving a full staircase of capped pixel chords.
        // Fixed endpoints mean accepted candidates cannot open seams; re-checking each
        // incident ring preserves the same no-crossing invariant as the repair itself.
        if inkvec_core::env::flag("INKVEC_TIMING") {
            eprintln!(
                "  [t] repair merge {} edge(s) {:.1} ms",
                merged.len(),
                merge_t.elapsed().as_secs_f64() * 1e3
            );
        }
        let safety_t = inkvec_core::clock::Instant::now();
        // Every refitted edge gets its compact fit offered back, not only the ones the
        // merge pass could afford. The edges the budget refused are precisely the ones
        // whose capped refit exploded, and they were the only ones never offered it.
        let smoothed: std::collections::HashMap<usize, FittedPath> = merged.into_iter().collect();
        let mut keys: Vec<usize> = refit_vertices.keys().copied().collect();
        keys.sort_unstable();
        for k in keys {
            // A capped refit that came back with many times the segments of the
            // unconstrained fit is not a repaired boundary; it is the cap having
            // collapsed toward one, where a refit reproduces the measured polyline point
            // by point. That staircase is the worst answer on every axis. bulma, a
            // seven-line polygon the artist wrote in 22 numbers: halving to the floor
            // produced 484 line segments and 1,920 parameters at dE00 0.0934, where the
            // 15-segment fit the repair had discarded scores 0.0880 -- better -- at 44.
            // The crossing it was refusing to ship renders better than the cure. So an
            // exploded refit is replaced by the full fit whether or not that crosses:
            // a crossing that survived halving to a cap of one is not one capping fixes.
            let exploded = {
                let (c, f) = (fitted[k].segments.len(), full_fit[k].segments.len().max(1));
                c > 32 && c > 4 * f
            };
            if exploded {
                fitted[k] = full_fit[k].clone();
                if inkvec_core::env::flag("INKVEC_TIMING") {
                    eprintln!("  [t] repair: edge {k} refit exploded, full fit restored");
                }
                continue;
            }
            // First try the original, MDL-optimal boundary. Most repaired rings have a
            // single bad edge, so restoring their other edges removes the visible
            // staircase without weakening the topology constraint. If that would cross,
            // the locally smoothed capped path is a second, still-safe opportunity.
            let mut candidates = vec![full_fit[k].clone()];
            if let Some(sc) = smoothed.get(&k) {
                candidates.push(sc.clone());
            }
            for candidate in candidates {
                let previous = std::mem::replace(&mut fitted[k], candidate);
                // Every ring is simple at this point - the loop above only exits when
                // none of them cross - so a crossing this candidate introduces has to
                // involve one of edge `k`'s own segments, and only those pairs are worth
                // testing. The all-pairs form of this check was 5 s of a 5.8 s trace on
                // simple-icons/biome.
                let safe = rings
                    .iter()
                    .filter(|ring| ring.iter().any(|&(edge, _)| edge == k))
                    .all(|ring| {
                        let (path, owner) = ring_as_path(ring, fitted);
                        let mask: Vec<bool> = owner.iter().map(|&o| o == k).collect();
                        simple::self_crossings_touching(&path, 32, &mask).is_empty()
                    });
                if safe {
                    break;
                }
                fitted[k] = previous;
            }
        }
        if inkvec_core::env::flag("INKVEC_TIMING") {
            eprintln!(
                "  [t] repair safety {:.1} ms",
                safety_t.elapsed().as_secs_f64() * 1e3
            );
        }
    }
    repaired
}

/// A ring as a polygon that follows its curves, not only its joins.
///
/// Endpoints alone are not the shape. A circle closed by two arcs, or a disc fitted as
/// one edge of two cubics, has its joins at two opposite points: the polygon through them
/// encloses nothing, so the ring failed the area test, the face had no outline, and it
/// was never painted — a clock face vanished under the rim around it.
pub(crate) fn ring_points(ring: &Ring, fitted: &[FittedPath]) -> Vec<Point> {
    let mut out = Vec::new();
    for &(k, rev) in ring {
        let path = if rev {
            fitted[k].reversed()
        } else {
            fitted[k].clone()
        };
        let mut at = path.start;
        if out.is_empty() {
            out.push(at);
        }
        for s in &path.segments {
            match *s {
                Segment::Line(_) => {}
                Segment::Cubic(c1, c2, p) => {
                    for i in 1..=RING_SAMPLES {
                        let t = i as f64 / (RING_SAMPLES + 1) as f64;
                        let u = 1.0 - t;
                        let b = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
                        out.push(Point::new(
                            b[0] * at.x + b[1] * c1.x + b[2] * c2.x + b[3] * p.x,
                            b[0] * at.y + b[1] * c1.y + b[2] * c2.y + b[3] * p.y,
                        ));
                    }
                }
                Segment::Arc {
                    rx,
                    ry,
                    phi,
                    large_arc,
                    sweep,
                    end,
                } => {
                    let f = inkvec_fit::curves::arc_ellipse_center(
                        at, rx, ry, phi, large_arc, sweep, end,
                    );
                    for i in 1..=RING_SAMPLES {
                        out.push(f.at(f.theta1 + f.delta * i as f64 / (RING_SAMPLES + 1) as f64));
                    }
                }
            }
            at = s.end();
            out.push(at);
        }
    }
    out
}

pub(crate) fn ring_area(r: &[Point]) -> f64 {
    let n = r.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for k in 0..n {
        let q = r[(k + 1) % n];
        a += r[k].x * q.y - q.x * r[k].y;
    }
    (0.5 * a).abs()
}

/// Is `inner` strictly inside `outer`? A point-in-polygon on one vertex suffices, because
/// planar-map rings never cross.
/// A point strictly inside `ring`, for asking whether the ring lies within another.
///
/// Probing with the ring's first vertex is wrong wherever two faces share a boundary,
/// which in a planar map is everywhere: the vertex lies exactly *on* the neighbour's ring,
/// and an even-odd parity test on a point on the edge answers arbitrarily. On a rounded
/// badge that made the transparent corner wedge a child of the square it merely touches,
/// so the wedge could not be dropped and the corner came out opaque white over what should
/// have been nothing.
///
/// The point is taken just inside the ring's own boundary rather than at its centre. A
/// centroid is the obvious choice and is wrong for exactly the question being asked: the
/// centre of a ring that has a hole lies *in the hole*, so the same test that decides
/// nesting would declare a face's outer boundary to be inside its own hole. It did, and
/// every face with a hole then had no outer ring left to draw — the badge lost its square
/// and rendered as a single stray arc.
///
/// Long edges are tried first because their midpoints sit furthest from other geometry,
/// the offset is taken as large as the ring will accept, and several points are returned
/// rather than one. A single probe is fragile precisely where it matters: a point half a
/// pixel from a boundary the two rings *share* is on neither side in particular, and two of
/// four corner wedges were still being called children of the square they only touch. The
/// caller takes a majority, which no single ambiguous point can overturn.
pub(crate) fn interior_probes(ring: &[Point]) -> Vec<Point> {
    if ring.len() < 3 {
        return Vec::new();
    }
    let mut edges: Vec<(f64, usize)> = (0..ring.len())
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            ((b.x - a.x).hypot(b.y - a.y), i)
        })
        .filter(|(l, _)| *l > 1e-9)
        .collect();
    edges.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut out = Vec::new();
    for &(len, i) in edges.iter().take(40) {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        let (dx, dy) = ((b.x - a.x) / len, (b.y - a.y) / len);
        let mid = Point::new(0.5 * (a.x + b.x), 0.5 * (a.y + b.y));
        // As far in as the ring allows: distance from the shared boundary is what makes
        // the test unambiguous. Both normals are tried rather than deriving the inward one
        // from the winding — relying on that convention left two of four corner wedges with
        // no usable probe at all, so every candidate failed and the fallback was the very
        // vertex this exists to avoid.
        let mut found = None;
        'eps: for eps in [1.0, 0.5, 0.25, 0.1, 0.03] {
            for s in [1.0, -1.0] {
                let q = Point::new(mid.x - s * dy * eps, mid.y + s * dx * eps);
                if point_in_ring(q, ring) {
                    found = Some(q);
                    break 'eps;
                }
            }
        }
        if let Some(q) = found {
            out.push(q);
        }
        if out.len() >= 5 {
            break;
        }
    }
    if out.is_empty() {
        out.push(ring[0]);
    }
    out
}

/// Even-odd test for a point against one ring.
pub(crate) fn point_in_ring(p: Point, outer: &[Point]) -> bool {
    if outer.len() < 3 {
        return false;
    }
    let mut inside = false;
    let n = outer.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (outer[i], outer[j]);
        if (a.y > p.y) != (b.y > p.y) {
            let t = (p.y - a.y) / (b.y - a.y);
            if p.x < a.x + t * (b.x - a.x) {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

pub(crate) fn ring_inside(inner: &[Point], outer: &[Point]) -> bool {
    // Rings of a planar map never cross, so a ring inside another encloses strictly less
    // area. Without this the probe test alone could call a face's outer boundary a child
    // of its own hole: on a flag whose emblem is a thin dark rim around a disc plus the
    // figure inside it, the rim's outer ring is the disc's circle and its probes, taken a
    // pixel inside the circle, land in the yellow hole wherever the rim is thinner than a
    // pixel. Both rings were then "inside" the other, the face had no outer ring left, and
    // the emblem vanished — a 0.1 px shift of the rim from the sub-pixel refinement was
    // enough to flip the majority (dE00 0.44 -> 4.59 on twemoji 1f1f3-1f1e8).
    if outer.len() < 3 || ring_area(inner) >= ring_area(outer) {
        return false;
    }
    let probes = interior_probes(inner);
    if probes.is_empty() {
        return false;
    }
    let hits = probes.iter().filter(|&&p| point_in_ring(p, outer)).count();
    hits * 2 > probes.len()
}

/// Points along each curved segment, besides its endpoints, when a ring is flattened for
/// area and containment tests.
const RING_SAMPLES: usize = 4;

/// Assemble a face ring into one path, remembering which edge each segment came from.
///
/// Needed because a self-crossing on a real boundary is almost never inside one edge's
/// fit. Repairing per edge — refitting any edge whose own curve crossed itself — changed
/// 5 images out of 180 and left the count at 76. The crossings are between *different*
/// edges of the same face, so the ring is the smallest unit at which the defect is even
/// visible.
pub(crate) fn ring_as_path(ring: &Ring, fitted: &[FittedPath]) -> (FittedPath, Vec<usize>) {
    let mut segments = Vec::new();
    let mut owner = Vec::new();
    let mut start = None;
    for &(k, rev) in ring {
        let path = if rev {
            fitted[k].reversed()
        } else {
            fitted[k].clone()
        };
        if start.is_none() {
            start = Some(path.start);
        }
        for s in path.segments {
            segments.push(s);
            owner.push(k);
        }
    }
    (
        FittedPath {
            start: start.unwrap_or(Point::new(0.0, 0.0)),
            segments,
            closed: true,
        },
        owner,
    )
}
