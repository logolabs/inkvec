//! Rings as geometry: which contains which, which way round, and where they cross.
//!
//! A face is a set of rings and the emitter has to know how they nest — an even-odd
//! compound path is only correct if the parity of each ring is. These answer that from the
//! fitted curves themselves rather than from the planar map, because by this point the
//! curves are what will actually be drawn and they can differ from the polyline the map
//! recorded by a fraction of a pixel.
//!
//! Two jobs live here, both on rings of fitted edges ([`crate::faces::Ring`]) in the traced
//! image's pixel coordinates:
//!
//! * **Nesting** ([`nesting`]), for the emitters: each ring is flattened to a polygon that
//!   follows its curves ([`ring_points`]), measured once ([`RingInfo`]: shoelace area,
//!   bounding box, interior probe points), and tested for containment by the even-odd
//!   crossing rule ([`point_in_ring`]) on a majority of probes ([`ring_inside`]). From
//!   that come each face's outline rings and the smallest face containing it
//!   ([`containment`]). Called from [`crate::emit`] and [`crate::mono`].
//! * **Repair** ([`repair_ring_crossings`]), for the colour pipeline after the fit: find
//!   rings whose assembled curves cross themselves, and refit the guilty edges -- pinned at
//!   a measured point beside each crossing, or under a tightening cap on the span of each
//!   segment, whichever the objective prices lower -- until none do. Called from
//!   [`crate::pipeline`].

use crate::diag;
use crate::faces::{FaceRings, Ring};
use inkvec_core::Point;
use inkvec_fit::{curves::Segment, multimodel, simple, FitConfig, FittedPath};

/// Smallest area, in square pixels, that a ring has to enclose to be worth emitting.
///
/// Far below a pixel, so that genuinely thin features survive (see [`nesting`]); what it
/// removes are the zero-area walks out along a chain and straight back that the planar
/// map can produce. Also the floor the monochrome emitter ([`crate::mono`]) writes with.
pub(crate) const MIN_RING_AREA: f64 = 0.25;

/// Containment forest over faces: `parent[i]` is the smallest face strictly containing
/// face `i`, if any.
///
/// "Contains" means some outline ring of the candidate encloses more area than face `i`'s
/// outline does and holds a majority of the interior probes of `i`'s first outline ring
/// ([`ring_inside`]); "smallest" is by total outline area. A face with no outline has no
/// parent. Quadratic in the number of faces, with each pair's test cut short by the
/// bounding box.
///
/// This is structure the planar map already knows and the output was throwing away. An
/// artist's file is a tree — a body with its markings nested inside it — and 42% of the
/// artist-authored SVGs surveyed use `<g>`. Emitting a flat list of paths renders
/// identically and is materially worse to work with: selecting a shape and its
/// decorations together becomes a manual rubber-band instead of one click.
///
/// `info` is [`ring_infos`] of `pts`: the same answer as testing the rings themselves, with
/// each ring's area and probes measured once instead of once per pair.
pub(crate) fn containment(
    pts: &[Vec<Vec<Point>>],
    info: &[Vec<RingInfo>],
    outer: &[Vec<usize>],
) -> Vec<Option<usize>> {
    let n = pts.len();
    let area: Vec<f64> = (0..n)
        .map(|i| outer[i].iter().map(|&k| info[i][k].area).sum())
        .collect();

    let mut parent = vec![None; n];
    for i in 0..n {
        if outer[i].is_empty() {
            continue;
        }
        let probe = &info[i][outer[i][0]];
        let mut best: Option<(usize, f64)> = None;
        for j in 0..n {
            if i == j || outer[j].is_empty() || area[j] <= area[i] {
                continue;
            }
            let inside = outer[j]
                .iter()
                .any(|&k| pts[j][k].len() >= 3 && ring_inside(probe, &pts[j][k], &info[j][k]));
            if inside && best.is_none_or(|(_, a)| area[j] < a) {
                best = Some((j, area[j]));
            }
        }
        parent[i] = best.map(|(j, _)| j);
    }
    parent
}

/// How the faces of a drawing nest: every ring as points, what the containment tests read
/// from each, each face's outline, and the face each one sits in.
pub(crate) struct Nesting {
    /// Every ring of every face, as the points [`ring_points`] follows it through.
    pub(crate) pts: Vec<Vec<Vec<Point>>>,
    /// [`ring_infos`] of `pts`.
    pub(crate) info: Vec<Vec<RingInfo>>,
    /// Per face, its outline: the rings enclosing some area that are not inside another of
    /// its own rings.
    pub(crate) outer: Vec<Vec<usize>>,
    /// Per face, the smallest face containing it: [`containment`].
    pub(crate) parent: Vec<Option<usize>>,
}

/// The [`Nesting`] of the faces `order` describes, drawn with `fitted`.
pub(crate) fn nesting(order: &[FaceRings], fitted: &[FittedPath]) -> Nesting {
    let pts: Vec<Vec<Vec<Point>>> = order
        .iter()
        .map(|face| face.iter().map(|r| ring_points(r, fitted)).collect())
        .collect();

    // Painted rings per face (holes dropped: in a partition every hole is another
    // face's outer boundary, painted later and on top).
    //
    // A ring enclosing no area is dropped outright. The planar map can produce faces one
    // pixel wide whose boundary walks out along a chain and straight back, and those were
    // being emitted as paths like `M35.5,11.5 L37.5,10.5 Z` — twenty-seven of them on a
    // plain green circle. They paint nothing at any resolution and cost coordinates, an
    // id, and a line in the document a person has to read past.
    //
    // The threshold is far below a pixel so that genuinely thin features survive: a
    // sliver forty pixels long and a third of a pixel wide still encloses about 13px^2.
    let solid: Vec<Vec<usize>> = (0..order.len())
        .map(|i| {
            (0..order[i].len())
                .filter(|&k| pts[i][k].len() >= 3 && ring_area(&pts[i][k]) > MIN_RING_AREA)
                .collect()
        })
        .collect();
    // `outer` is the face's outline: the rings not contained in another of its own. It
    // decides containment and paint order.
    // Every containment test below reads these, measured once per ring: a face with a
    // thousand holes, or a document of four thousand faces, asks about each ring thousands
    // of times, and measuring it afresh each time was two thirds of a fast trace.
    let info = ring_infos(&pts);
    let outer: Vec<Vec<usize>> = (0..order.len())
        .map(|i| {
            solid[i]
                .iter()
                .copied()
                .filter(|&k| {
                    !solid[i]
                        .iter()
                        .any(|&m| m != k && ring_inside(&info[i][k], &pts[i][m], &info[i][m]))
                })
                .collect()
        })
        .collect();
    let parent = containment(&pts, &info, &outer);
    Nesting {
        pts,
        info,
        outer,
        parent,
    }
}

/// What the containment tests read from a ring, measured once: its area, its bounding box
/// and its [`interior_probes`].
pub(crate) struct RingInfo {
    /// Enclosed area, px² ([`ring_area`]).
    pub(crate) area: f64,
    /// `[min x, min y, max x, max y]`, widened by [`BOX_SLACK`].
    bbox: [f64; 4],
    /// Points strictly inside the ring ([`interior_probes`]).
    pub(crate) probes: Vec<Point>,
}

/// How far a bounding box is widened before a point outside it is called outside the ring.
/// [`point_in_ring`] can only count a crossing to the right of a point that lies within the
/// ring's extent, up to the rounding of one interpolation (a few ulps, 1e-11 px at this
/// scale); this is far above that, so the box never rejects a point the full test accepts.
const BOX_SLACK: f64 = 1e-6;

impl RingInfo {
    /// Measure `ring` (a closed polygon, px): its area, widened bounding box and probes.
    fn new(ring: &[Point]) -> Self {
        let mut bbox = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for p in ring {
            bbox = [
                bbox[0].min(p.x),
                bbox[1].min(p.y),
                bbox[2].max(p.x),
                bbox[3].max(p.y),
            ];
        }
        Self {
            area: ring_area(ring),
            bbox: [
                bbox[0] - BOX_SLACK,
                bbox[1] - BOX_SLACK,
                bbox[2] + BOX_SLACK,
                bbox[3] + BOX_SLACK,
            ],
            probes: interior_probes(ring),
        }
    }

    /// False only where [`point_in_ring`] is false too: outside the ring's extent the
    /// crossings to the right of a point are none or all, and all is an even number.
    fn may_contain(&self, p: Point) -> bool {
        p.x >= self.bbox[0] && p.y >= self.bbox[1] && p.x <= self.bbox[2] && p.y <= self.bbox[3]
    }
}

/// [`RingInfo`] for every ring of every face.
pub(crate) fn ring_infos(pts: &[Vec<Vec<Point>>]) -> Vec<Vec<RingInfo>> {
    use rayon::prelude::*;
    pts.par_iter()
        .map(|face| face.iter().map(|r| RingInfo::new(r)).collect())
        .collect()
}

/// Refit whichever edges take part in a self-crossing, each either pinned where it crosses
/// or under a tightening span cap, whichever the objective prices lower, until the assembled
/// rings stop crossing themselves. Returns the number of refits made.
///
/// An edge is shared by the two faces either side of it, and it is refitted *once* — so
/// both faces continue to reference the same curve and the property the planar map exists
/// to guarantee is preserved. The repair cannot fail to terminate: an edge gets at most
/// [`LOCAL_ROUNDS`] pinned refits, every other refit halves its cap, and at a cap of one an
/// edge's fit reproduces its measured polyline, and the measured boundary of a face on a
/// partition is simple.
///
/// The algorithm runs in rounds, at most ten:
///
/// 1. Every ring (after the first round, only rings touching an edge refitted in the last
///    round) is assembled into one path ([`ring_as_located_path`]) and tested for pairs of
///    crossing segments with where they cross (`inkvec_fit::simple::self_crossing_points`,
///    32 samples per segment); both segments' edges are guilty.
/// 2. Each guilty edge gets two candidate refits, both by the same dynamic program:
///    * **pinned** ([`pin_crossings`]): each of its crossing segments must break at the
///      measured point nearest where it crosses (the first crossing found on it), on top
///      of the pins it already has, with no cap of its own
///      (`inkvec_fit::multimodel::optimal_multimodel_forced`), and the usual merge of
///      free cubics afterwards with the pins kept;
///    * **halved**: its cap -- the most measured points one segment may span, starting at
///      the edge's point count -- halved, its existing pins kept, no merge.
///
///    The one with the lower objective (`MultimodelFit::cost`, the program's own
///    `½χ² + λ·params + breaks` before refinement) is kept, and only its constraint is
///    remembered. An edge with nothing to pin (each crossing segment spans adjacent measured
///    points) or already pinned `LOCAL_ROUNDS` times is only halved.
///
/// Pinning adds just the breakpoints that separate the two curves where they cross, which
/// is what the topology-preserving simplification literature does — vertices restored only
/// where a simplified chain would cross another (de Berg, van Kreveld & Schirra 1998;
/// Saalfeld 1999; cited in full at `optimal_multimodel_forced`) — instead of tightening
/// every segment of the edge. Halving is what this repair did alone before 2026-10. Neither
/// dominates: a pin near the end of a long curve can cost more segments than a halved cap
/// (openmoji/1F9B3: 175 numbers pinned against 154 halved), and a halved cap re-segments
/// the whole edge where one pin would do (openmoji/1F517 at 512 px: 2.87x the artist's
/// parameters halved, 2.09x with the choice). Letting the objective choose between two
/// valid constraints is the program's own rule applied one level up. Not from the
/// literature: the choice by cost, because the papers above add vertices by a fixed rule.
///
/// Measured on the 246-icon gate set (2026-10-04, against halving alone): parameter
/// ratio -0.77 % at 128 px, -0.35 % at 512 px, -0.20 % at 512 px opaque; dE00 within
/// ±0.22 %; rings still crossing after repair 11 on 6 icons, against 14 on 9 (128 px).
/// Trying only the pin (no halving candidate) gained -0.50 / -0.29 / -0.19 %; pinning at
/// every crossing rather than one per segment, -0.09 / -0.33 / -0.12 %; requiring a break
/// anywhere inside the crossing segment instead of at a point left 19 rings crossing.
///
/// A refit is correct but can be faceted, so afterwards each refitted edge is offered,
/// in turn, its original unconstrained fit, its pins alone with no cap (its latest proposed
/// pins, merged with the pins kept; only for an edge whose cap was halved), and a smoothed
/// version of its refit (free cubics merged, corners sharpened, under a segment budget), and
/// keeps the first that crosses nothing in the rings it belongs to. The pins-only offer
/// measured dE00 -0.20 / -0.15 / -0.11 % against the repair without it (128 / 512 /
/// 512 px opaque), parameter ratio within ±0.04 %. A capped refit that exploded to many
/// times the segments of the original fit is replaced by the original outright; see the
/// comment at that test for why.
///
/// `polys` are the measured boundaries the fits came from (indexed like `fitted`), `cfg`
/// the fit configuration the refits use. `INKVEC_TIMING` prints the time of each phase.
pub(crate) fn repair_ring_crossings(
    order: &[FaceRings],
    fitted: &mut [FittedPath],
    polys: &[inkvec_core::Polyline],
    cfg: &FitConfig,
) -> usize {
    const ROUNDS: usize = 10;
    let mut st = RepairState::new(fitted, polys);
    let rings: Vec<&Ring> = order.iter().flatten().collect();
    let mut changed: Option<std::collections::HashSet<usize>> = None;
    let timing = inkvec_core::env::flag("INKVEC_TIMING");
    for round in 0..ROUNDS {
        inkvec_core::progress::step("repair rounds", round as u64, ROUNDS as u64);
        let round_t = inkvec_core::clock::Instant::now();
        // Detection per ring and refits per edge are both independent; run each wave on
        // every core. The refits are the expensive half — a capped refit re-runs the
        // whole dynamic program on that boundary.
        let hits = located_crossings(&rings, fitted, changed.as_ref());
        let mut guilty: Vec<usize> = hits.iter().map(|h| h.0).collect();
        guilty.sort_unstable();
        guilty.dedup();
        guilty.retain(|&k| st.cap[k] > 1);
        if guilty.is_empty() {
            break;
        }
        let mut proposed = st.propose_pins(&guilty, &hits, fitted, polys);
        inkvec_core::progress::note(|| {
            format!(
                "round {}: {} crossing boundar{}, refitting",
                round + 1,
                guilty.len(),
                if guilty.len() == 1 { "y" } else { "ies" }
            )
        });
        diag::debug(timing, || {
            let sizes: Vec<usize> = guilty.iter().map(|&k| polys[k].len()).collect();
            format!(
                "  [t] repair round: {} guilty, sizes {:?}, detect {:.1} ms",
                guilty.len(),
                sizes,
                round_t.elapsed().as_secs_f64() * 1e3
            )
        });
        for (k, f, by_pin) in st.refit(&guilty, &proposed, polys, cfg) {
            if by_pin {
                st.pins[k] = proposed.remove(&k).unwrap_or_default();
                st.pinned_rounds[k] += 1;
                st.n_pinned += 1;
            } else {
                st.cap[k] = (st.cap[k] / 2).max(1);
                st.n_halved += 1;
            }
            fitted[k] = f.path;
            st.refit_vertices.insert(k, f.vertices);
            st.repaired += 1;
        }
        diag::debug(timing, || {
            format!(
                "  [t] repair round total {:.1} ms ({} pinned, {} halved so far)",
                round_t.elapsed().as_secs_f64() * 1e3,
                st.n_pinned,
                st.n_halved
            )
        });
        changed = Some(guilty.iter().copied().collect());
    }
    if !st.refit_vertices.is_empty() {
        st.restore(&rings, fitted, polys, cfg, timing);
    }
    st.repaired
}

/// Each crossing in the rings (only those touching an edge in `changed`, when given) as
/// `(edge, segment of that edge's own fit, where it crosses)`, sorted by edge, then segment,
/// then x. After the first round only rings touching a refitted edge can have changed;
/// re-testing the rest re-derives the same answer at full price. Rings are tested on every
/// core.
fn located_crossings(
    rings: &[&Ring],
    fitted: &[FittedPath],
    changed: Option<&std::collections::HashSet<usize>>,
) -> Vec<(usize, usize, Point)> {
    use rayon::prelude::*;
    let mut hits: Vec<(usize, usize, Point)> = rings
        .par_iter()
        .filter(|ring| changed.is_none_or(|c| ring.iter().any(|&(k, _)| c.contains(&k))))
        .flat_map_iter(|ring| {
            let mut g: Vec<(usize, usize, Point)> = Vec::new();
            if ring.len() >= 2 {
                let (path, owner) = ring_as_located_path(ring, fitted);
                for (i, j, at) in simple::self_crossing_points(&path, 32) {
                    for s in [i, j] {
                        if let Some(&(k, q)) = owner.get(s) {
                            g.push((k, q, at));
                        }
                    }
                }
            }
            g
        })
        .collect();
    hits.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then(a.2.x.total_cmp(&b.2.x)));
    hits
}

/// What [`repair_ring_crossings`] keeps between rounds, per edge (indexed like `fitted`).
struct RepairState {
    /// The unconstrained optimum. A cap is a topology emergency brake, not a better
    /// description of the boundary; after the offending neighbours have been repaired we
    /// can often put this compact path back without bringing the crossing with it.
    full_fit: Vec<FittedPath>,
    /// Each edge's span cap, starting at its point count (no cap).
    cap: Vec<usize>,
    /// Each edge's pins so far (measured-point indices every later refit keeps).
    pins: Vec<Vec<usize>>,
    /// How many of each edge's refits were pinned ones.
    pinned_rounds: Vec<usize>,
    /// Each edge's latest proposed pins, kept or not, for the pins-only offer at the end.
    last_proposed: std::collections::HashMap<usize, Vec<usize>>,
    /// Vertices of every boundary the loop refitted, for the merge pass afterwards.
    refit_vertices: std::collections::HashMap<usize, Vec<usize>>,
    /// Refits made.
    repaired: usize,
    /// Of those, how many were pinned and how many halved (for `INKVEC_TIMING`).
    n_pinned: usize,
    n_halved: usize,
}

impl RepairState {
    /// The state before any refit: every fit unconstrained, no pins.
    fn new(fitted: &[FittedPath], polys: &[inkvec_core::Polyline]) -> Self {
        RepairState {
            full_fit: fitted.to_vec(),
            cap: polys.iter().map(|p| p.len().max(2)).collect(),
            pins: vec![Vec::new(); polys.len()],
            pinned_rounds: vec![0; polys.len()],
            last_proposed: std::collections::HashMap::new(),
            refit_vertices: std::collections::HashMap::new(),
            repaired: 0,
            n_pinned: 0,
            n_halved: 0,
        }
    }

    /// The pinned candidate's pins, per guilty edge that has something new to pin and has
    /// been pinned fewer than [`LOCAL_ROUNDS`] times: its pins so far plus one per crossing
    /// segment, at the first crossing found on it (`hits` is sorted by edge, then segment).
    /// Each proposal is also remembered in `last_proposed`.
    fn propose_pins(
        &mut self,
        guilty: &[usize],
        hits: &[(usize, usize, Point)],
        fitted: &[FittedPath],
        polys: &[inkvec_core::Polyline],
    ) -> std::collections::HashMap<usize, Vec<usize>> {
        let mut proposed = std::collections::HashMap::new();
        for &k in guilty {
            if self.pinned_rounds[k] >= LOCAL_ROUNDS {
                continue;
            }
            let mut mine: Vec<(usize, Point)> = hits
                .iter()
                .filter(|h| h.0 == k)
                .map(|h| (h.1, h.2))
                .collect();
            mine.dedup_by_key(|h| h.0);
            let mut trial = self.pins[k].clone();
            if pin_crossings(&fitted[k], &polys[k], &mine, &mut trial) > 0 {
                self.last_proposed.insert(k, trial.clone());
                proposed.insert(k, trial);
            }
        }
        proposed
    }

    /// Each guilty edge's refit, and whether it is the pinned candidate (else the halved
    /// cap's): the halved cap with its existing pins, and where pins were proposed the pinned
    /// program with no new cap too, the cheaper kept (ties to the pin, the local change).
    /// The refits of different edges are independent, so they run on every core.
    fn refit(
        &self,
        guilty: &[usize],
        proposed: &std::collections::HashMap<usize, Vec<usize>>,
        polys: &[inkvec_core::Polyline],
        cfg: &FitConfig,
    ) -> Vec<(usize, multimodel::MultimodelFit, bool)> {
        use rayon::prelude::*;
        let live = inkvec_core::progress::handle();
        guilty
            .par_iter()
            .map(|&k| {
                live.check();
                // The program under a cap of `c` with the pins `p`; with no pins, the capped
                // program this repair always used.
                let fit = |c: usize, p: &[usize]| {
                    live.scoped(|| {
                        if p.is_empty() {
                            multimodel::optimal_multimodel_capped_full(&polys[k], cfg, c)
                        } else {
                            multimodel::optimal_multimodel_forced(&polys[k], cfg, c, p)
                        }
                    })
                };
                let halved = fit((self.cap[k] / 2).max(1), &self.pins[k]);
                match proposed.get(&k) {
                    Some(p) => {
                        let pinned = fit(self.cap[k], p);
                        if halved.cost < pinned.cost {
                            (k, halved, false)
                        } else {
                            (k, pinned, true)
                        }
                    }
                    None => (k, halved, false),
                }
            })
            .collect()
    }

    /// The merged ("smoothed") version of every refitted edge the merge budget can afford:
    /// free cubics merged and corners sharpened, shortest boundary first.
    fn smoothed(
        &self,
        fitted: &[FittedPath],
        polys: &[inkvec_core::Polyline],
        cfg: &FitConfig,
        timing: bool,
    ) -> Vec<(usize, FittedPath)> {
        use rayon::prelude::*;
        let live = inkvec_core::progress::handle();
        let refit_vertices = &self.refit_vertices;
        // A capped refit is the constrained program's raw answer: chords and G1 cubics
        // with the corner chamfers left in. Under the cap the merge pass was skipped
        // because it re-joined runs into cubics that crossed again and its cost grew
        // with the segment count. Now that the rings are simple, merge and sharpen each
        // refitted boundary once, and keep the result only where the rings it belongs to
        // stay simple. Without this, 1f9d1-1f3ff-200d-1f680 and 1f640 (twemoji) came
        // back faceted at +0.12 and +0.10 dE00.
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
        diag::debug(timing && affordable.len() < refit_vertices.len(), || {
            format!(
                "  [t] repair merge budget: {} of {} boundary/ies, {} segment(s)",
                affordable.len(),
                refit_vertices.len(),
                spent
            )
        });
        inkvec_core::progress::step("refits smoothed", 0, affordable.len() as u64);
        let merged: Vec<(usize, FittedPath)> = affordable
            .par_iter()
            .map(|&(k, verts)| {
                live.check();
                let mut path = fitted[k].clone();
                inkvec_fit::merge::merge_free_cubics(&mut path, &polys[k], verts, cfg);
                inkvec_fit::merge::sharpen_corners(&mut path);
                live.tick();
                (k, path)
            })
            .collect();
        diag::debug(timing, || {
            format!(
                "  [t] repair merge {} edge(s) {:.1} ms",
                merged.len(),
                merge_t.elapsed().as_secs_f64() * 1e3
            )
        });
        merged
    }

    /// After the rounds: offer every refitted edge, in sorted key order, its original fit,
    /// its pins alone with no cap, and its merged ("smoothed") refit, keeping the first that
    /// crosses nothing in the rings it belongs to (see [`repair_ring_crossings`]).
    fn restore(
        &self,
        rings: &[&Ring],
        fitted: &mut [FittedPath],
        polys: &[inkvec_core::Polyline],
        cfg: &FitConfig,
        timing: bool,
    ) {
        use rayon::prelude::*;
        let live = inkvec_core::progress::handle();
        let (full_fit, refit_vertices) = (&self.full_fit, &self.refit_vertices);
        let (pins, last_proposed, cap) = (&self.pins, &self.last_proposed, &self.cap);
        let merged = self.smoothed(fitted, polys, cfg, timing);
        // Test candidates one at a time. The previous all-at-once trial was needlessly
        // pessimistic: one unsafe cubic caused every other candidate in the same face
        // ring to be discarded too, leaving a full staircase of capped pixel chords.
        // Fixed endpoints mean accepted candidates cannot open seams; re-checking each
        // incident ring preserves the same no-crossing invariant as the repair itself.
        let safety_t = inkvec_core::clock::Instant::now();
        // Every refitted edge gets its compact fit offered back, not only the ones the
        // merge pass could afford. The edges the budget refused are precisely the ones
        // whose capped refit exploded, and they were the only ones never offered it.
        let smoothed: std::collections::HashMap<usize, FittedPath> = merged.into_iter().collect();
        let mut keys: Vec<usize> = refit_vertices.keys().copied().collect();
        keys.sort_unstable();
        // The pins alone, with no cap: the smallest change that separates the curves where
        // they crossed, merged like the original fit with the pins kept. An edge the
        // objective moved on to halving still had its crossing located, so its last
        // proposal is offered too; an edge whose last refit already was this (never halved)
        // is not offered it twice. Independent per edge, so fitted on every core.
        let pins_only: std::collections::HashMap<usize, FittedPath> = keys
            .par_iter()
            .filter_map(|&k| {
                let p = last_proposed.get(&k).unwrap_or(&pins[k]);
                (!p.is_empty() && cap[k] < polys[k].len().max(2)).then(|| {
                    live.check();
                    let fit = multimodel::optimal_multimodel_forced(&polys[k], cfg, usize::MAX, p);
                    (k, fit.path)
                })
            })
            .collect();
        for k in keys {
            inkvec_core::progress::checkpoint();
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
                diag::debug(timing, || {
                    format!("  [t] repair: edge {k} refit exploded, full fit restored")
                });
                continue;
            }
            // First try the original, MDL-optimal boundary. Most repaired rings have a
            // single bad edge, so restoring their other edges removes the visible
            // staircase without weakening the topology constraint. If that would cross,
            // the pins alone are the next smallest change, and the locally smoothed
            // capped path a third, still-safe opportunity.
            let mut candidates = vec![full_fit[k].clone()];
            if let Some(p) = pins_only.get(&k) {
                candidates.push(p.clone());
            }
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
        diag::debug(timing, || {
            format!(
                "  [t] repair safety {:.1} ms",
                safety_t.elapsed().as_secs_f64() * 1e3
            )
        });
    }
}

/// Pinned refits one edge may have before the repair only halves its cap.
///
/// A bound on how many vertices pinning can add, so the repair keeps its guarantee: after
/// these, every refit halves the cap, which reaches the measured polyline. Not derived and
/// not swept: on the 246-icon gate set an edge almost never needs more than two pinned
/// refits, and the cost choice usually prefers halving before this bound matters.
const LOCAL_ROUNDS: usize = 4;

/// Pin an edge's fit where it crosses: for each `(segment, point)` in `hits` (segments of
/// `path`, the edge's current fit, in its own direction; one point per segment), the
/// measured point of `poly` nearest the crossing among those strictly inside the segment's
/// measured run ([`segment_ranges`]), added to `pins` (ascending, no duplicates). A segment
/// whose run has no interior point (it joins adjacent measured points) cannot be pinned.
/// Returns how many new pins were added; none when the path's joins cannot be placed on
/// the polyline.
///
/// O(Σ run lengths + n·segments) for the run search. The pin is a vertex of every later
/// refit, so the refitted curve passes through a measured point beside the crossing; the
/// measured boundaries of a planar partition do not cross, so pinned there the two curves
/// are held apart at the place they crossed.
fn pin_crossings(
    path: &FittedPath,
    poly: &inkvec_core::Polyline,
    hits: &[(usize, Point)],
    pins: &mut Vec<usize>,
) -> usize {
    let Some(ranges) = segment_ranges(path, poly) else {
        return 0;
    };
    let n = poly.len();
    let mut added = 0;
    for &(q, at) in hits {
        let Some(&(a, b)) = ranges.get(q) else {
            continue;
        };
        // Offsets strictly inside the run; `a`, `b` are unwrapped (`b` may pass `n` on a
        // closed boundary), so each is reduced to an index. On ties the first wins.
        let best = (a + 1..b)
            .map(|o| o % n)
            .min_by(|&x, &y| poly.points[x].dist(at).total_cmp(&poly.points[y].dist(at)));
        if let Some(v) = best {
            if let Err(pos) = pins.binary_search(&v) {
                pins.insert(pos, v);
                added += 1;
            }
        }
    }
    added
}

/// The measured run of each segment of `path` on `poly`: `(a, b)` per segment, in unwrapped
/// indices (a closed boundary's runs may pass its seam, so `b` can exceed `n`; reduce mod
/// `n`), or `None` when the path's joins cannot be placed on the polyline in order.
///
/// The fit keeps no record of which measured points each segment came from (the post-fit
/// merge and the corner sharpening change the segmentation after the program), so it is
/// recovered from the geometry: the path's start is matched to its nearest measured point,
/// then each segment's end to the nearest measured point *ahead* of the previous match (the
/// first of any within 1e-6 px of that distance), up to one lap of a closed boundary, whose
/// last segment ends where it began. A join the program chose is a measured point, or one
/// moved by at most a few sigma (a corner between two lines meets at their intersection), so
/// the walk lands on it or beside it. O(n·segments).
fn segment_ranges(path: &FittedPath, poly: &inkvec_core::Polyline) -> Option<Vec<(usize, usize)>> {
    let n = poly.len();
    if n < 2 || path.segments.is_empty() {
        return None;
    }
    // The first offset in `lo..=hi` (reduced mod `n`) at the least distance from `p`.
    let nearest_from = |p: Point, lo: usize, hi: usize| -> usize {
        let d: Vec<f64> = (lo..=hi).map(|o| poly.points[o % n].dist(p)).collect();
        let m = d.iter().copied().fold(f64::INFINITY, f64::min);
        lo + d.iter().position(|&x| x <= m + 1e-6).unwrap_or(0)
    };
    let start = nearest_from(path.start, 0, n - 1);
    // How far the walk may go: one lap on a closed boundary, to the end on an open one.
    let last = if poly.closed { start + n } else { n - 1 };
    let mut at = start;
    let mut out = Vec::with_capacity(path.segments.len());
    for (q, s) in path.segments.iter().enumerate() {
        let next = if poly.closed && q + 1 == path.segments.len() {
            last
        } else {
            if at + 1 > last {
                return None;
            }
            nearest_from(s.end(), at + 1, last)
        };
        out.push((at, next));
        at = next;
    }
    Some(out)
}

/// A ring as a polygon that follows its curves, not only its joins.
///
/// Each edge is walked in the ring's direction; every segment contributes its end point,
/// and a cubic or arc also [`RING_SAMPLES`] points evenly spaced in its parameter. A cubic
/// with ends `P0`, `P3` and controls `P1`, `P2` is sampled from the Bernstein form
/// `B(t) = (1-t)³P0 + 3(1-t)²t P1 + 3(1-t)t² P2 + t³P3`; an arc through its centre
/// parameterisation (SVG 1.1 F.6.5, `inkvec_fit::curves::arc_ellipse_center`), at evenly
/// spaced angles. The polygon is left open: its last point is the ring's start again.
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

/// The area a closed polygon encloses, px², by the shoelace formula:
/// `A = |Σ_k (x_k·y_(k+1) - x_(k+1)·y_k)| / 2`, indices modulo `n`.
///
/// Unsigned, so either winding gives the same answer; zero for fewer than three points. A
/// self-crossing polygon gets the net of its lobes, which is fine here because the rings
/// it measures come from a planar partition.
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

/// Points strictly inside `ring`, for asking whether the ring lies within another: up to
/// five, never empty for a ring of three or more points (the first vertex is the fallback),
/// empty for fewer.
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
///
/// Concretely: the forty longest polygon edges, longest first; from each edge's midpoint a
/// step of 1, 0.5, 0.25, 0.1 or 0.03 px along either normal, keeping the first point that
/// [`point_in_ring`] puts inside.
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

/// Even-odd test for a point against one ring (a closed polygon, px).
///
/// The crossing-number rule: cast a ray from `p` towards +x and count the polygon edges it
/// crosses; `p` is inside when the count is odd. An edge from `a` to `b` is counted when
/// it straddles the ray's height -- exactly one of `a.y`, `b.y` above `p.y`, which counts a
/// vertex on the ray once and ignores horizontal edges -- and the crossing
/// `x = a.x + (p.y - a.y)(b.x - a.x)/(b.y - a.y)` lies to the right of `p.x`. A point
/// exactly on the boundary may go either way, which is why [`interior_probes`] keeps its
/// probes off the boundary. False for fewer than three points.
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

/// Is the ring `inner` measures inside the ring `outer`? A majority of its interior probes
/// has to be, and it has to enclose less area; `outer_info` is `outer` measured.
pub(crate) fn ring_inside(inner: &RingInfo, outer: &[Point], outer_info: &RingInfo) -> bool {
    // Rings of a planar map never cross, so a ring inside another encloses strictly less
    // area. Without this the probe test alone could call a face's outer boundary a child
    // of its own hole: on a flag whose emblem is a thin dark rim around a disc plus the
    // figure inside it, the rim's outer ring is the disc's circle and its probes, taken a
    // pixel inside the circle, land in the yellow hole wherever the rim is thinner than a
    // pixel. Both rings were then "inside" the other, the face had no outer ring left, and
    // the emblem vanished — a 0.1 px shift of the rim from the sub-pixel refinement was
    // enough to flip the majority (dE00 0.44 -> 4.59 on twemoji 1f1f3-1f1e8).
    if outer.len() < 3 || inner.area >= outer_info.area {
        return false;
    }
    let probes = &inner.probes;
    if probes.is_empty() {
        return false;
    }
    // A probe outside the box is outside the ring, so a majority has to be inside the box
    // before any probe is worth the full test. The count is the full test's either way.
    let boxed = probes
        .iter()
        .filter(|&&p| outer_info.may_contain(p))
        .count();
    if boxed * 2 <= probes.len() {
        return false;
    }
    let hits = probes
        .iter()
        .filter(|&&p| outer_info.may_contain(p) && point_in_ring(p, outer))
        .count();
    hits * 2 > probes.len()
}

/// Points along each curved segment, besides its endpoints, when a ring is flattened for
/// area and containment tests.
const RING_SAMPLES: usize = 4;

/// Assemble a face ring into one path, remembering where each segment came from: `(edge,
/// index of the segment in that edge's own fit)`, the index counted in the edge's own
/// direction even where the ring walks it reversed (segment `r` of a reversed fit of `m`
/// segments is the fit's segment `m − 1 − r`).
///
/// Needed because a self-crossing on a real boundary is almost never inside one edge's
/// fit. Repairing per edge — refitting any edge whose own curve crossed itself — changed
/// 5 images out of 180 and left the count at 76. The crossings are between *different*
/// edges of the same face, so the ring is the smallest unit at which the defect is even
/// visible. The segment index is what the local repair pins ([`pin_crossings`]).
pub(crate) fn ring_as_located_path(
    ring: &Ring,
    fitted: &[FittedPath],
) -> (FittedPath, Vec<(usize, usize)>) {
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
        let m = path.segments.len();
        for (r, s) in path.segments.into_iter().enumerate() {
            segments.push(s);
            owner.push((k, if rev { m - 1 - r } else { r }));
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

/// [`ring_as_located_path`] with only the edge of each segment: what the repair's safety
/// check reads.
pub(crate) fn ring_as_path(ring: &Ring, fitted: &[FittedPath]) -> (FittedPath, Vec<usize>) {
    let (path, owner) = ring_as_located_path(ring, fitted);
    (path, owner.into_iter().map(|(k, _)| k).collect())
}

#[cfg(test)]
mod tests {
    //! The local repair's bookkeeping: where each segment of a fit lies on its measured
    //! boundary, where a crossing is pinned, and which segment of which edge a ring's
    //! segment is.
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    /// An open run of 11 points along the x axis, and a closed 2x2 square of 8.
    fn open_run() -> inkvec_core::Polyline {
        let pts: Vec<Point> = (0..=10).map(|k| p(k as f64, 0.0)).collect();
        inkvec_core::Polyline::with_uniform_sigma(pts, 0.05, false)
    }

    fn square() -> inkvec_core::Polyline {
        let pts = vec![
            p(0.0, 0.0),
            p(1.0, 0.0),
            p(2.0, 0.0),
            p(2.0, 1.0),
            p(2.0, 2.0),
            p(1.0, 2.0),
            p(0.0, 2.0),
            p(0.0, 1.0),
        ];
        inkvec_core::Polyline::with_uniform_sigma(pts, 0.05, true)
    }

    #[test]
    fn segment_runs_follow_the_measured_boundary() {
        let path = FittedPath {
            start: p(0.0, 0.0),
            segments: vec![
                Segment::Line(p(4.0, 0.0)),
                Segment::Cubic(p(6.0, 0.1), p(8.0, 0.1), p(10.0, 0.0)),
            ],
            closed: false,
        };
        assert_eq!(
            segment_ranges(&path, &open_run()),
            Some(vec![(0, 4), (4, 10)])
        );
        // A loop fitted from index 2: its runs pass the seam, unwrapped (8 is index 0 again,
        // 10 the start once more).
        let ring = FittedPath {
            start: p(2.0, 0.0),
            segments: vec![
                Segment::Line(p(2.0, 2.0)),
                Segment::Line(p(0.0, 2.0)),
                Segment::Line(p(0.0, 0.0)),
                Segment::Line(p(2.0, 0.0)),
            ],
            closed: true,
        };
        assert_eq!(
            segment_ranges(&ring, &square()),
            Some(vec![(2, 4), (4, 6), (6, 8), (8, 10)])
        );
        // A join behind the previous one cannot be placed.
        let back = FittedPath {
            start: p(0.0, 0.0),
            segments: vec![Segment::Line(p(10.0, 0.0)), Segment::Line(p(3.0, 0.0))],
            closed: false,
        };
        assert_eq!(segment_ranges(&back, &open_run()), None);
    }

    #[test]
    fn a_crossing_is_pinned_inside_its_segment_at_the_nearest_point() {
        let ring = FittedPath {
            start: p(2.0, 0.0),
            segments: vec![
                Segment::Line(p(2.0, 2.0)),
                Segment::Line(p(0.0, 2.0)),
                Segment::Line(p(0.0, 0.0)),
                Segment::Line(p(2.0, 0.0)),
            ],
            closed: true,
        };
        let sq = square();
        let mut pins = vec![6];
        // Segment 1 runs over indices 4..6, so 5 is the only point inside it; segment 3
        // runs 8..10 across the seam, so its inside point is index 1. A repeat adds nothing.
        let added = pin_crossings(
            &ring,
            &sq,
            &[(1, p(1.2, 2.3)), (3, p(0.9, -0.2)), (1, p(1.0, 2.0))],
            &mut pins,
        );
        assert_eq!(added, 2);
        assert_eq!(pins, vec![1, 5, 6]);
        // A segment between adjacent measured points has nothing inside it to pin.
        let tight = FittedPath {
            start: p(0.0, 0.0),
            segments: vec![Segment::Line(p(1.0, 0.0)), Segment::Line(p(10.0, 0.0))],
            closed: false,
        };
        let mut none = Vec::new();
        assert_eq!(
            pin_crossings(&tight, &open_run(), &[(0, p(0.5, 0.1))], &mut none),
            0
        );
        assert!(none.is_empty());
    }

    #[test]
    fn a_reversed_edge_names_its_segments_in_its_own_direction() {
        let a = FittedPath {
            start: p(0.0, 0.0),
            segments: vec![Segment::Line(p(1.0, 0.0)), Segment::Line(p(2.0, 0.0))],
            closed: false,
        };
        let b = FittedPath {
            start: p(0.0, 0.0),
            segments: vec![
                Segment::Line(p(0.0, 1.0)),
                Segment::Line(p(1.0, 1.0)),
                Segment::Line(p(2.0, 0.0)),
            ],
            closed: false,
        };
        // Along `a`, then back along `b` reversed.
        let ring: Ring = vec![(0, false), (1, true)];
        let (path, owner) = ring_as_located_path(&ring, &[a.clone(), b.clone()]);
        assert_eq!(owner, vec![(0, 0), (0, 1), (1, 2), (1, 1), (1, 0)]);
        assert_eq!(path.segments.len(), 5);
        assert_eq!(
            path.segments[2].end(),
            p(1.0, 1.0),
            "b's last segment, walked back"
        );
        let (_, edges) = ring_as_path(&ring, &[a, b]);
        assert_eq!(edges, vec![0, 0, 1, 1, 1]);
    }
}
