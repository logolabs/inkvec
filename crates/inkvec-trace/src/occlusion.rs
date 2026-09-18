//! T-junction analysis: which face occludes which, read off the exact planar map.
//!
//! This exists to answer a narrower question than it looks like it answers. It does
//! **not** recover anything hidden — no pixel behind an occluder is ever touched, and
//! nothing here invents geometry. It only reports a fact the visible boundary already
//! proves: at a point where one face's outline runs straight through and another face's
//! outline stops dead against it, the straight one was painted after. `docs/DESIGN.md`
//! §2.4 rules out the generative version of this idea — amodal inpainting of what a
//! shape looks like *behind* an occluder — as a correctness hazard for a logo, never
//! default-on. This is the strictly smaller claim that survives that rule: a pairwise
//! "in front of" fact between two faces that are both fully visible in the image, with
//! no content invented anywhere.
//!
//! # The geometry, worked from scratch
//!
//! A first guess at the test — "two of the three edges at a degree-3 node share the same
//! pair of faces" — is wrong. Work a rectangle `R` painted over a rod `D` on background
//! `Bg`, with `R`'s left edge running straight through the point where `D`'s visible top
//! edge meets it. The three edges at that node are `(D, R)`, `(R, Bg)` and `(D, Bg)` —
//! three *different* pairs, because the map splits a straight run the moment a third
//! face touches it (see `planar::build`'s merge invariant: two segments merge only when
//! they separate the same pair of faces). And a plain three-way meeting of unrelated
//! regions — three wedges of a pie chart, say — has exactly the same property: every
//! face there appears in exactly two of the three pairs too. Pair membership alone
//! cannot tell a T from a Y.
//!
//! What actually distinguishes them is **collinearity**. In the rectangle example, `R`'s
//! two edges — `(D, R)` and `(R, Bg)` — are the same straight line, split only by the
//! point of contact; their tangents at the node point in opposite directions, near 180
//! degrees apart. `D`'s two edges — `(D, R)` and `(D, Bg)` — meet at a real corner, and
//! so do `Bg`'s. A generic three-way meeting has no face whose pair of edges is
//! collinear at all. So: **the occluder is whichever one of the three faces has its two
//! incident edges running straight through the node; the face named only in the
//! remaining edge is the occluded one, because that is the edge whose boundary
//! terminates there instead of continuing.**
//!
//! # Why one junction is not enough, and the aggregate is
//!
//! The classical cue was built for photographs, where two boundaries lining up at a
//! point is *unlikely by chance* unless one really does pass behind the other. That
//! premise is false for flat vector logo art: a badge split into two colours by a
//! straight line, a flag's stripes, a letterform's counter — deliberately abutting flat
//! regions with perfectly aligned edges are the common case here, not the exception. A
//! single T-junction is real, local, and invents nothing, but on its own it is weak
//! evidence in this domain specifically because alignment is often intentional rather
//! than incidental.
//!
//! The risk is not hypothetical. `tests::a_three_way_partition_reads_as_a_single_uncorroborated_junction`
//! demonstrates a spurious reading on the plainest possible negative case — three flat
//! wedges meeting at a point, 120 degrees apart, no occlusion anywhere in the picture —
//! and it is not sampling noise: checked at 10, 20, 40 and 80 px, the same single false
//! reading recurs at the centre pixel unchanged. A square grid has four quadrants at any
//! corner, and a three-way meeting at a generic angle cannot land one label in each of
//! three of them without the fourth coinciding with one of the other two, which reads
//! locally as two collinear edges. It does not average away with resolution because it
//! is not noise — it is what a 120 degree meeting looks like on a square grid, every
//! time.
//!
//! What corroborates a reading is agreement. A genuine occlusion between two filled
//! regions typically produces more than one T-junction — entry and exit of the hidden
//! stretch — and every one of them has to name the same face as occluder for the same
//! pair. A spurious alignment, whether from deliberate design or from the grid artefact
//! above, has no reason to agree in direction from one junction to the next, and in the
//! demonstrated case there is only ever one junction to begin with. So [`aggregate`]
//! groups junctions by the unordered pair of faces involved and reports, per pair,
//! whether every junction that names them agrees on which is in front — a pair backed by
//! a single junction is exactly the case a caller must not trust. Whether that agreement
//! rate is high enough on *real* art, not just synthetic negatives, is measured in
//! `bench/occlusion_survey.py`.

use std::collections::HashMap;

use inkvec_core::{Point, Vec2};

use crate::planar::PlanarMap;

/// One candidate T-junction claim: `occluder` is locally in front of `occluded` at
/// `pos`.
///
/// A single node produces **two** of these, not one. The occluder's two collinear edges
/// each name a different neighbour — `{D, R}` and `{R, Bg}` in the module doc's worked
/// example — and each is an independent claim: `R` is in front of `D`, and separately
/// `R` is in front of `Bg`. There is no basis in the local geometry for picking only one;
/// a straight boundary with a shape's edge terminating against it from either side is the
/// same fact regardless of which side.
#[derive(Debug, Clone, Copy)]
pub struct TJunction {
    /// The planar-map node where the junction occurs.
    pub node: u32,
    /// Position of the junction.
    pub pos: Point,
    /// The face whose two edges are collinear at this node — read in front.
    pub occluder: u16,
    /// A face directly adjacent to the occluder along one of its two collinear edges.
    pub occluded: u16,
    /// Degrees the occluder's two edges deviate from a straight 180 degree line. Lower
    /// is a more confident single reading.
    pub bend_deg: f64,
}

/// Above this, a face's two edges are not "collinear" and it cannot be the occluder.
/// 30 degrees is a starting guess, not a measured value — `bench/occlusion_survey.py` is
/// what calibrates it against the real corpus, and this constant should move to whatever
/// that measurement says once it exists.
pub const MAX_BEND_DEG: f64 = 30.0;

/// The two edges at `node` that touch `face`, as unit tangents pointing *away* from the
/// node along each edge (so two collinear edges point in opposite directions and the
/// angle between them is near 180 degrees, not 0).
fn tangents_away(points: &[Point], at_start: bool, origin: Point) -> Option<Vec2> {
    // The first interior point in the direction the edge actually runs, skipping any
    // duplicate that sits on the node itself (refine_junctions can leave one).
    let seq: Box<dyn Iterator<Item = &Point>> = if at_start {
        Box::new(points.iter())
    } else {
        Box::new(points.iter().rev())
    };
    for &p in seq {
        let v = p - origin;
        if v.norm() > 1e-6 {
            return Some(Vec2 {
                x: v.x / v.norm(),
                y: v.y / v.norm(),
            });
        }
    }
    None
}

/// Every degree-3 node in `map` that reads as a T-junction, with the occluder named.
///
/// Reads the map only; changes nothing. A node with more than three incident edges is a
/// genuine multi-way meeting (or the two-shapes-touch-at-a-corner "kiss" the saddle pass
/// already splits, see `planar::saddle_tests`) and is skipped — the collinearity test
/// only has a clean answer for exactly three.
pub fn find(map: &PlanarMap) -> Vec<TJunction> {
    let mut inc: HashMap<u32, Vec<(usize, bool)>> = HashMap::new();
    for (k, e) in map.edges.iter().enumerate() {
        if e.closed || e.points.len() < 2 {
            continue;
        }
        inc.entry(e.start_node).or_default().push((k, true));
        inc.entry(e.end_node).or_default().push((k, false));
    }

    let mut out = Vec::new();
    let mut nodes: Vec<u32> = inc.keys().copied().collect();
    nodes.sort_unstable();

    for node in nodes {
        let incident = &inc[&node];
        if incident.len() != 3 {
            continue;
        }
        let origin = map.edges[incident[0].0].points[if incident[0].1 {
            0
        } else {
            map.edges[incident[0].0].points.len() - 1
        }];

        // Per incident edge: its face pair and its outward tangent at this node. A face
        // equal to the sentinel is the out-of-bounds virtual background `planar::build`
        // invents to close shapes at the image edge (`label_at` returns `u16::MAX`
        // there) -- a construction device, never a real face, and a shape merely
        // touching the canvas border must never be read as occluded by "the void".
        let mut arms: Vec<(u16, u16, Vec2)> = Vec::with_capacity(3);
        let mut ok = true;
        for &(k, at_start) in incident {
            let e = &map.edges[k];
            if e.left == u16::MAX || e.right == u16::MAX {
                ok = false;
                break;
            }
            let Some(t) = tangents_away(&e.points, at_start, origin) else {
                ok = false;
                break;
            };
            arms.push((e.left, e.right, t));
        }
        if !ok {
            continue;
        }

        // The three distinct faces present, however the pairs name them.
        let mut faces: Vec<u16> = Vec::with_capacity(3);
        for &(l, r, _) in &arms {
            for f in [l, r] {
                if !faces.contains(&f) {
                    faces.push(f);
                }
            }
        }
        if faces.len() != 3 {
            // Not three mutually distinct faces -- a degenerate or higher-multiplicity
            // node the merge/split invariants do not produce here; skip rather than guess.
            continue;
        }

        // For each face, find its two arms and the angle between their tangents.
        let mut best: Option<(u16, f64, [usize; 2])> = None;
        for &f in &faces {
            let idx: Vec<usize> = (0..3)
                .filter(|&i| arms[i].0 == f || arms[i].1 == f)
                .collect();
            if idx.len() != 2 {
                continue;
            }
            let (a, b) = (arms[idx[0]].2, arms[idx[1]].2);
            // Two edges of the same straight run point away from the node in opposite
            // directions, so the angle between their outward tangents is near 180 deg,
            // not 0.
            let cos = (a.dot(b) / (a.norm() * b.norm())).clamp(-1.0, 1.0);
            let bend = 180.0 - cos.acos().to_degrees();
            if best.map(|(_, bb, _)| bend < bb).unwrap_or(true) {
                best = Some((f, bend, [idx[0], idx[1]]));
            }
        }

        let Some((occluder, bend, used)) = best else {
            continue;
        };
        if bend > MAX_BEND_DEG {
            continue;
        }
        // Both of the occluder's arms are independent claims: whichever face pairs with
        // `occluder` on each of its two collinear edges is directly adjacent to it there,
        // and reads as behind it. `used` holds exactly those two arm indices.
        for &i in &used {
            let (l, r, _) = arms[i];
            let occluded = if l == occluder { r } else { l };
            out.push(TJunction {
                node,
                pos: origin,
                occluder,
                occluded,
                bend_deg: bend,
            });
        }
    }
    out
}

/// Per unordered face pair: how many junctions named them, and whether they agreed on
/// which is in front.
#[derive(Debug, Clone, Copy)]
pub struct PairVerdict {
    /// The smaller of the two face ids.
    pub a: u16,
    /// The larger of the two face ids.
    pub b: u16,
    /// `Some(x)` when every junction naming this pair agreed `x` occludes the other;
    /// `None` when they disagreed.
    pub occluder: Option<u16>,
    /// Number of junctions that named `a` as the occluder.
    pub for_a: usize,
    /// Number of junctions that named `b` as the occluder.
    pub for_b: usize,
}

/// Group T-junctions by the unordered pair of faces they concern, and report whether
/// every reading for a given pair points the same way.
///
/// This is the corroboration step the module doc argues for: a single T-junction is weak
/// evidence in vector art, because aligned edges are frequently intentional design rather
/// than occlusion. Agreement across every junction that names a given pair is what turns
/// a plausible local reading into a fact worth acting on -- and disagreement is equally
/// informative, since it names exactly the pairs where this signal should not be trusted.
pub fn aggregate(js: &[TJunction]) -> Vec<PairVerdict> {
    let mut tally: HashMap<(u16, u16), (usize, usize)> = HashMap::new();
    for j in js {
        let key = (j.occluder.min(j.occluded), j.occluder.max(j.occluded));
        let e = tally.entry(key).or_insert((0, 0));
        if j.occluder == key.0 {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }
    let mut out: Vec<PairVerdict> = tally
        .into_iter()
        .map(|((a, b), (for_a, for_b))| PairVerdict {
            a,
            b,
            occluder: if for_a > 0 && for_b > 0 {
                None
            } else if for_a > 0 {
                Some(a)
            } else {
                Some(b)
            },
            for_a,
            for_b,
        })
        .collect();
    out.sort_by_key(|v| (v.a, v.b));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planar;

    /// Rectangle `R` (label 2) painted over rod `D` (label 1) on background `Bg` (label
    /// 0), exactly the module doc's worked example. `D` occupies x in [2,4), y in [3,7);
    /// `R` occupies x in [3,12), y in [0,10); `R` is painted last, so it wins the
    /// overlap and only x in [2,3) of `D` survives.
    fn rod_behind_rectangle() -> PlanarMap {
        let (w, h) = (12usize, 10usize);
        let mut labels = vec![0u16; w * h];
        for y in 0..h {
            for x in 0..w {
                let in_d = (2..4).contains(&x) && (3..7).contains(&y);
                let in_r = (3..10).contains(&x);
                labels[y * w + x] = if in_r {
                    2
                } else if in_d {
                    1
                } else {
                    0
                };
            }
        }
        planar::build(&labels, w, h, 3)
    }

    #[test]
    fn the_rectangle_is_read_as_occluding_the_rod() {
        let map = rod_behind_rectangle();
        let js = find(&map);
        assert!(!js.is_empty(), "expected at least one T-junction");
        // Every claim is R (2) in front of something; that something is either the rod
        // (1) or the background (0), one claim per occluder arm -- never the rectangle
        // named as occluded, and never a claim about itself.
        for j in &js {
            assert_eq!(j.occluder, 2, "the rectangle should read as in front");
            assert!(
                j.occluded == 0 || j.occluded == 1,
                "unexpected occluded face {}",
                j.occluded
            );
        }
        assert!(
            js.iter().any(|j| j.occluded == 1),
            "expected an R-in-front-of-D claim"
        );

        let verdicts = aggregate(&js);
        let v = verdicts
            .iter()
            .find(|v| (v.a, v.b) == (1, 2))
            .expect("a verdict for the rod/rectangle pair");
        assert_eq!(v.occluder, Some(2), "should agree, not split");
    }

    /// Three flat wedges meeting at a point, 120 degrees apart (a plain partition, like a
    /// pie chart) -- and a demonstration of why one T-junction is never enough on its own.
    ///
    /// This is *not* a synthetic worst case chosen to embarrass the detector. It is the
    /// generic one: a square pixel grid has exactly four quadrants at any corner, and a
    /// three-way meeting at any angle other than the axes cannot land one label on each
    /// of three quadrants without the fourth coinciding with one of the other two --
    /// which reads, locally, as exactly two collinear edges. Checked at 10, 20, 40 and 80
    /// px (`bench/occlusion_survey.py`'s calibration script): the same spurious reading
    /// recurs at the centre pixel at every resolution, unchanged. It does not average
    /// away with more pixels, because it is not sampling noise -- it is what a 120 degree
    /// meeting looks like on a square grid, every time.
    ///
    /// So this test asserts the honest thing rather than a clean negative: away from the
    /// meeting point there is nothing (real design elsewhere must not trip the detector),
    /// and directly at it there is exactly one low-support reading per pair -- which is
    /// precisely the shape [`aggregate`] exists to catch. A caller trusting any single
    /// `TJunction` without corroboration will get this wrong on ordinary flat art with no
    /// occlusion in it at all; a caller requiring agreement across more than one junction
    /// per pair will not.
    #[test]
    fn a_three_way_partition_reads_as_a_single_uncorroborated_junction() {
        let (w, h) = (10usize, 10usize);
        let mut labels = vec![0u16; w * h];
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f64 - 5.0, y as f64 - 5.0);
                let ang = fy.atan2(fx);
                labels[y * w + x] = if ang < -std::f64::consts::PI / 3.0 {
                    0
                } else if ang < std::f64::consts::PI / 3.0 {
                    1
                } else {
                    2
                };
            }
        }
        let map = planar::build(&labels, w, h, 3);
        let js = find(&map);

        let is_far = |j: &TJunction| j.pos.dist(Point::new(4.5, 4.5)) > 0.75;
        assert!(
            js.iter().all(|j| !is_far(j)),
            "a spurious reading away from the centre: {js:?}"
        );

        let near: Vec<&TJunction> = js.iter().filter(|j| !is_far(j)).collect();
        assert!(
            !near.is_empty(),
            "expected the known centre-pixel discretisation artifact"
        );
        // Both claims come from the one node, so every pair they name has support 1 --
        // the exact signature `aggregate` cannot corroborate, and must not be asked to.
        for v in aggregate(&js) {
            if v.for_a + v.for_b <= 2 {
                assert!(
                    v.for_a == 0 || v.for_b == 0,
                    "a genuinely disagreeing pair should not arise from one node: {v:?}"
                );
            }
        }
    }

    #[test]
    fn a_lone_shape_on_background_has_no_t_junctions() {
        let (w, h) = (10usize, 10usize);
        let mut labels = vec![0u16; w * h];
        for y in 3..7 {
            for x in 3..7 {
                labels[y * w + x] = 1;
            }
        }
        let map = planar::build(&labels, w, h, 2);
        assert!(find(&map).is_empty());
    }
}
