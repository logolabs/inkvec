//! Layers: each face completed behind what is painted over it.
//!
//! **The problem.** The emitter paints faces back to front, so a face's outline is only
//! seen where nothing painted later covers it. It already uses that where faces nest -- a
//! face draws its outer rings and the faces inside it paint its holes -- but not where they
//! sit side by side: there a face draws exactly what it shows, its shared edge with the
//! neighbour painted over it is written twice, and the anti-aliasing seam that leaves is
//! patched by moving it half a pixel under the neighbour ([`crate::seams`]). An artist draws
//! the lower shape whole instead: a blue rectangle with two white triangles over it, not
//! the blue that shows between them (`openmoji/1F3F4-E0069-E0074-E0062-E0061-E007F`: 20
//! numbers in the artist's file, 54 in the trace before this stage).
//!
//! **The theorem.** With the paint order fixed, a shape `E` drawn for a face that shows `V`,
//! under the regions `U` painted after it, leaves every visible point unchanged exactly
//! when `V ⊆ E ⊆ V ∪ U` (`docs/theory/chain-representation.md`, R4.1; in Lean,
//! `InkvecTheory.Design.Layers.painter_interval_iff`). The condition names no other face's
//! shape, so each face is completed on its own; and inside the interval the pixels do not
//! care, so the shape is the cheapest one in it: here, the one with the fewest numbers the
//! gate counts.
//!
//! Anti-aliasing adds one effect the theorem's exact areas do not have: where a completed
//! edge meets the rim of what covers it and a third paint, the edge pixel composites all
//! three (R4.1b of the chain page). The inputs are renders of the artists' own layered
//! files, which have exactly that mix wherever the artist layered (a face's disc under a
//! shading crescent that reaches its rim), so the upper bound is the exact interval `V ∪ U`
//! ([`allowed_region`], with a margin [`MARGIN`] of zero), and
//! the candidates try first to reach [`REACH`] under the cover, which keeps their edge out
//! of the cover's own anti-aliasing where there is room.
//!
//! **The method.** For every painted, opaque face with something painted over it, in the
//! paint ranks the first writing pass settled ([`crate::emit`]):
//!
//! 1. the regions: `E` the face as written now, `U` everything painted later that paints
//!    opaque (faces and the strokes of [`crate::ribbons`], which go on top of everything),
//!    `V = E \ U`, and the allowed region `H = V ∪ U`;
//! 2. candidate shapes, cheapest first: a circle or an ellipse through the face's own
//!    boundary (the part no cover reaches), the rectangle round what it shows; and the
//!    face's own rings with each run of covered segments replaced by the corner its owned
//!    lines meet at, or by a chord reaching a pixel under the cover;
//! 3. each candidate is accepted only by [`check::certify_completion`], the painter-interval
//!    check, and only when it writes fewer numbers than the face does now
//!    ([`check::gate_count`]).
//!
//! A completed face is written from its completion and not underlapped: its edge already
//! lies under what covers it. Everything else is as before.
//!
//! **Where it sits.** Inside [`crate::emit::emit_color`], after the first writing pass (which
//! fixes the paint ranks) and before the seams: Quality mode, unless `INKVEC_COMPLETION=0`.
//!
//! Not from the literature in this form; the closest are layered vectorisers that recover
//! occluded shapes by generative inpainting (LayerPeeler, AmodalSVG; `docs/DESIGN.md` §2.4),
//! which this is not: no pixel anyone can see is invented, by the theorem.

mod check;
mod region;
mod shape;

use std::collections::HashMap;

use inkvec_core::Point;
use inkvec_fit::curves::Segment;
use inkvec_fit::primitives::{PrimitiveFit, PrimitiveKind};
use inkvec_fit::FittedPath;

use crate::faces::FaceRings;
use crate::pathdata::{fmt_segments, ring_to_segments};
pub(crate) use check::gate_count;
use region::{Region, Rule};
pub(crate) use shape::{EndCap, JoinKind};

/// How far a completed edge first tries to reach under its cover, px: a pixel, so the
/// cover's anti-aliased pixels have the completed face under them and not the ground.
pub(crate) const REACH: f64 = 1.0;

/// The cover shrunk by this much, px, before it bounds a completion: none, the exact
/// interval (measured against 0.5 px in phase 2 of chain R: no gain at any condition).
const MARGIN: f64 = 0.0;

/// How far around a face the cover is read, px.
const PAD: f64 = 8.0;

/// How far around what one run's replacement changes it is judged, px: past the reach and
/// the junction taper of the lower bound.
const WINDOW: f64 = 3.0;

/// Whether every candidate's verdict is printed (`INKVEC_COMPLETION_DEBUG=2`; `1` prints
/// one summary line per trace).
fn trace() -> bool {
    inkvec_core::env::number("INKVEC_COMPLETION_DEBUG").is_some_and(|v| v >= 2.0)
}

/// Whether the stage runs: on, unless `INKVEC_COMPLETION=0` (the A/B switch).
pub(crate) fn enabled() -> bool {
    inkvec_core::env::number("INKVEC_COMPLETION").is_none_or(|v| v != 0.0)
}

/// A stroke painted over every face ([`crate::ribbons`]): its centrelines, width and ends.
#[derive(Clone, Debug)]
pub(crate) struct StrokeShape {
    /// Centrelines, px.
    pub(crate) lines: Vec<FittedPath>,
    /// Closed primitive centrelines (a stroked circle or rectangle), px.
    pub(crate) prims: Vec<PrimitiveKind>,
    /// `stroke-width`, px.
    pub(crate) width: f64,
    /// How open ends are drawn.
    pub(crate) cap: EndCap,
    /// How pieces meet.
    pub(crate) join: JoinKind,
}

/// What the stage reads, per face where indexed by face.
pub(crate) struct Input<'a> {
    /// The rings of each face, as walks over shared edges.
    pub(crate) order: &'a [FaceRings],
    /// The fitted curve of each edge.
    pub(crate) fitted: &'a [FittedPath],
    /// Per edge, the primitive the whole edge fitted, when it did.
    pub(crate) prims: &'a [Option<PrimitiveFit>],
    /// Per face, the rings it draws (its outline).
    pub(crate) outer: &'a [Vec<usize>],
    /// Per face, the faces punched out of it.
    pub(crate) holes: &'a [Vec<usize>],
    /// Per face, its paint rank in the first writing pass; `None` when not painted.
    pub(crate) rank: &'a [Option<usize>],
    /// Per face, whether it paints opaque everywhere it paints (it can cover).
    pub(crate) covers: &'a [bool],
    /// Per face, whether it may be completed (opaque, written from its own rings, nothing
    /// punched out of it).
    pub(crate) candidate: &'a [bool],
    /// Per face, the gate's count of the element it is written as now.
    pub(crate) cost_now: &'a dyn Fn(usize) -> f64,
    /// Strokes painted over every face.
    pub(crate) strokes: &'a [StrokeShape],
    /// The faces those strokes are written for: their regions in the map lie above every
    /// face, though the stroke drawn for one may be narrower than its region.
    pub(crate) stroke_faces: &'a [usize],
    /// Decimals per written coordinate.
    pub(crate) decimals: usize,
    /// The input's blur beyond a clean render's, px: the noise model's `psf_radius` (0 on a
    /// clean intake). Where three paints meet, the picture inside the blur is not
    /// identifiable, and the interval is relaxed there ([`junction_zone`]).
    pub(crate) psf: f64,
}

/// How far from a junction, in blur radii, the picture is not identifiable: a wedge of
/// angle `θ` loses its tip to a blur of radius `r` over `r / sin(θ/2)`, which is four radii
/// at 29°, about the most acute corner the corpus's artists draw.
const JUNCTION_REACH: f64 = 4.0;

/// The zone round the junctions near `bbox` where a blur of radius `psf` hides which paint
/// shows: discs of [`JUNCTION_REACH`] radii. Empty when `psf` is 0 (a clean intake).
fn junction_zone(junctions: &[Point], psf: f64, bbox: (f64, f64, f64, f64)) -> Region {
    if psf <= 0.0 {
        return Region::empty();
    }
    let r = JUNCTION_REACH * psf;
    let discs: Vec<Vec<Point>> = junctions
        .iter()
        .filter(|j| j.x >= bbox.0 - r && j.x <= bbox.2 + r && j.y >= bbox.1 - r && j.y <= bbox.3 + r)
        .map(|j| shape::flatten_primitive(&PrimitiveKind::Circle { c: *j, r }))
        .collect();
    if discs.is_empty() {
        return Region::empty();
    }
    Region::from_polygons(&discs, Rule::NonZero)
}

/// Where the edges of the map meet: the ends of every open fitted edge.
fn junction_points(fitted: &[FittedPath]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::new();
    for f in fitted.iter().filter(|f| !f.closed) {
        for q in [f.start, f.segments.last().map(Segment::end).unwrap_or(f.start)] {
            if !out.iter().any(|o| o.dist(q) < 1e-6) {
                out.push(q);
            }
        }
    }
    out
}

/// Whether every ring face `f` draws is one edge fitted by a primitive.
fn is_primitive_face(inp: &Input, f: usize) -> bool {
    let Some(rings) = inp.order.get(f) else {
        return false;
    };
    let outer = &inp.outer[f];
    !outer.is_empty()
        && outer.iter().all(|&k| {
            let ring = &rings[k];
            ring.len() == 1 && inp.prims.get(ring[0].0).and_then(|p| p.as_ref()).is_some()
        })
}

/// A completed face's new shape.
#[derive(Clone, Debug)]
pub(crate) enum Shape {
    /// One primitive element.
    Prim(PrimitiveKind),
    /// Path data.
    Path(String),
}

/// One face's completion, with the gate's count before and after.
#[derive(Clone, Debug)]
pub(crate) struct Completion {
    pub(crate) shape: Shape,
    pub(crate) before: f64,
    pub(crate) after: f64,
}

/// The region a face is written as now: its outline rings, its punched holes cut out.
fn face_region(inp: &Input, f: usize) -> Region {
    let mut polys = face_rings(inp, f);
    for &c in &inp.holes[f] {
        polys.extend(face_rings(inp, c));
    }
    Region::from_polygons(&polys, Rule::EvenOdd)
}

/// The closed polygons of a face's outline rings.
fn face_rings(inp: &Input, f: usize) -> Vec<Vec<Point>> {
    let Some(rings) = inp.order.get(f) else {
        return Vec::new();
    };
    inp.outer[f]
        .iter()
        .filter_map(|&k| {
            let ring = &rings[k];
            if ring.len() == 1 {
                if let Some(pf) = inp.prims.get(ring[0].0).and_then(|p| p.as_ref()) {
                    return Some(shape::flatten_primitive(&pf.kind));
                }
            }
            let (start, segs) = ring_to_segments(ring, inp.fitted);
            if segs.is_empty() {
                return None;
            }
            let mut pts = shape::flatten_path(&FittedPath {
                start,
                segments: segs,
                closed: true,
            });
            if pts.len() > 1 && pts.first() == pts.last() {
                pts.pop();
            }
            (pts.len() >= 3).then_some(pts)
        })
        .collect()
}

/// Everything the strokes paint.
fn stroke_cover(strokes: &[StrokeShape]) -> Region {
    let mut all = Region::empty();
    for s in strokes {
        let mut band = shape::stroke_band(&s.lines, s.width, s.cap, s.join);
        for p in &s.prims {
            let mut pts = shape::flatten_primitive(p);
            if let Some(&first) = pts.first() {
                pts.push(first);
            }
            let path = FittedPath {
                start: pts[0],
                segments: pts[1..].iter().map(|&q| Segment::Line(q)).collect(),
                closed: true,
            };
            band = band.union(&shape::stroke_band(&[path], s.width, s.cap, JoinKind::Round));
        }
        all = all.union(&band);
    }
    all
}

/// How far a completion must reach under its cover wherever the face meets it, px: the
/// half pixel the underlap of [`crate::seams`] reaches, so a completed face, which the
/// underlap no longer moves, keeps the ground out of the cover's anti-aliased edge.
pub(crate) const UNDER: f64 = 0.5;

/// Over how many pixels from a junction the reach under the cover ramps up to [`UNDER`]:
/// where the face, its cover and a third paint meet, the reach tapers to nothing, as the
/// underlap's does over the same distance (`crate::seams`'s taper).
const TAPER: f64 = 12.0;

/// Steps of the ramp in [`required_region`].
const RAMP: usize = 4;

/// The region a completion of a face that shows `v` must cover: what it shows, and the
/// cover `u` within [`UNDER`] of it, the reach ramping from nothing at a third paint to
/// [`UNDER`] at [`TAPER`] from it (in [`RAMP`] steps). The lower end of the interval,
/// raised from `V` so that no completion opens a seam where the face meets what is painted
/// over it, and no higher than the underlap it replaces.
pub(crate) fn required_region(v: &Region, u: &Region) -> Region {
    let third = v.dilate(UNDER).minus(v).minus(u);
    let mut need = v.clone();
    for k in 1..=RAMP {
        let s = k as f64 / RAMP as f64;
        let reach = v.dilate(UNDER * s).intersect(u);
        need = need.union(&reach.minus(&third.dilate(TAPER * s)));
    }
    need
}

/// The region a completion of a face that shows `v` may reach: what it shows and the cover
/// `u`, `V ∪ U` (the face as written now lies in it). With a margin `m > 0`, the cover only
/// where it is at least `m` from a third paint: `V ∪ (V ∪ U) ⊖ m`. Shrinking `V ∪ U` and
/// not `U` alone keeps the boundary between the face and its cover inside, so a completion
/// may run along it.
pub(crate) fn allowed_region(e_now: &Region, v: &Region, u: &Region, m: f64) -> Region {
    if m <= 0.0 {
        return e_now.union(u).union(v);
    }
    v.union(&e_now.union(u).erode(m))
}

/// Completes every face it can; returns the completed faces.
pub(crate) fn complete(inp: &Input) -> HashMap<usize, Completion> {
    let n = inp.order.len();
    let mut out = HashMap::new();
    let mut painted: Vec<usize> = (0..n).filter(|&f| inp.rank.get(f).copied().flatten().is_some()).collect();
    if painted.is_empty() {
        return out;
    }
    // From the top down, with the running union of everything above.
    painted.sort_by_key(|&f| std::cmp::Reverse(inp.rank[f]));
    let regions: HashMap<usize, Region> = painted.iter().map(|&f| (f, face_region(inp, f))).collect();
    let mut above = stroke_cover(inp.strokes);
    // What the faces above cover in the map, as against what they paint (`above`): where
    // the two differ next to a face, its underlap painted the gap, and so must its completion.
    let mut map_above = inp
        .stroke_faces
        .iter()
        .fold(Region::empty(), |acc, &s| acc.union(&face_region(inp, s)));
    let junctions = if inp.psf > 0.0 { junction_points(inp.fitted) } else { Vec::new() };
    let mut i = 0;
    while i < painted.len() {
        let r = inp.rank[painted[i]];
        let mut j = i;
        while j < painted.len() && inp.rank[painted[j]] == r {
            j += 1;
        }
        for &f in &painted[i..j] {
            if inp.candidate.get(f).copied().unwrap_or(false) {
                if let Some(c) = complete_face(inp, f, &regions[&f], &above, &map_above, &junctions) {
                    out.insert(f, c);
                }
            }
        }
        for &f in &painted[i..j] {
            if inp.covers.get(f).copied().unwrap_or(false) {
                above = above.union(&regions[&f]);
                map_above = map_above.union(&regions[&f]);
            }
        }
        i = j;
    }
    out
}

/// The cheapest certified completion of face `f`, written now as `e_now`, under `above`.
fn complete_face(
    inp: &Input,
    f: usize,
    e_now: &Region,
    above: &Region,
    map_above: &Region,
    junctions: &[Point],
) -> Option<Completion> {
    let (x0, y0, x1, y1) = e_now.bbox()?;
    let u = above.crop(x0 - PAD, y0 - PAD, x1 + PAD, y1 + PAD);
    if u.is_empty() || e_now.dilate(0.75).intersect(&u).area() < 0.5 {
        return None;
    }
    // What the face shows: its own region outside the cover, and the seam between its region
    // and a cover drawn narrower than the face above's region in the map, which the face's
    // underlap paints today.
    let gap = e_now
        .dilate(UNDER)
        .intersect(&map_above.crop(x0 - PAD, y0 - PAD, x1 + PAD, y1 + PAD))
        .minus(&u)
        .minus(e_now);
    let v = e_now.minus(&u).union(&gap);
    if v.area() < 1.0 {
        return None;
    }
    let mut h = allowed_region(e_now, &v, &u, MARGIN);
    let mut l = required_region(&v, &u);
    // Near a junction of a blurred input the interval holds off a zone `N` the blur leaves
    // unidentified, and only within one blur radius of the interval's own ends:
    // `L \ N ⊆ E ⊆ H ∪ N` (`Inkvec.Design.painter_interval_off`).
    let zone = junction_zone(junctions, inp.psf, (x0, y0, x1, y1));
    if !zone.is_empty() {
        let spill = zone.intersect(&h.dilate(inp.psf)).minus(&h);
        let loss = zone.minus(&l.erode(inp.psf));
        h = h.union(&spill);
        l = l.minus(&loss);
    }
    let hb = h.bbox()?;
    let before = (inp.cost_now)(f);
    let mut best: Option<Completion> = None;
    // A face written as a primitive gets no underlap, so a tie buys it nothing.
    let prim_now = is_primitive_face(inp, f);
    let mut consider = |shape: Shape, after: f64, allow: f64, region: &Region| {
        // Never more than the face writes now. A tie is kept: the completed face reaches under
        // its cover and is spared the underlap, which only adds numbers.
        if after > before + allow
            || (prim_now && after >= before)
            || best.as_ref().is_some_and(|b| b.after <= after)
        {
            return;
        }
        // The solver proposes only what its own measure passes; the checker decides.
        let m = check::measure(&l, region, &h);
        let certified = m.holds() && check::certify_completion(&l, region, &h);
        if trace() {
            eprintln!(
                "layers face {f}: {} -> {after}: lost {:.3} spilled {:.3} certified {certified}",
                match &shape {
                    Shape::Prim(k) => format!("{k:?}").chars().take(140).collect::<String>(),
                    Shape::Path(_) => "path".to_string(),
                },
                m.lost,
                m.spilled
            );
        }
        if certified {
            best = Some(Completion { shape, before, after });
        }
    };

    // Primitives through the boundary the face owns.
    let owned = owned_points(inp, f, &u);
    if owned.len() >= 8 {
        let sigma = vec![0.05; owned.len()];
        if let Some(c) = inkvec_fit::primitives::fit_circle(&owned, &sigma) {
            for e in [REACH, 0.5, 0.0] {
                let kind = PrimitiveKind::Circle { c: c.c, r: c.r + e };
                if let Some(reg) = bounded_region(&[shape::flatten_primitive(&kind)], hb) {
                    consider(Shape::Prim(kind), 3.0, 0.0, &reg);
                }
            }
        }
        if let Some(el) = inkvec_fit::primitives::fit_ellipse(&owned, &sigma) {
            for e in [REACH, 0.5, 0.0] {
                let kind = PrimitiveKind::Ellipse {
                    c: el.c,
                    rx: el.rx + e,
                    ry: el.ry + e,
                    angle: el.angle,
                };
                if let Some(reg) = bounded_region(&[shape::flatten_primitive(&kind)], hb) {
                    consider(Shape::Prim(kind), 4.0, 0.0, &reg);
                }
            }
        }
    }
    // The rectangle round what the face shows. A region's box is exact across a scan line
    // but known along it only to a line's spacing, so first each side is put on the face's
    // own geometry when a vertex lies that close, then on the box as sampled.
    // The box of the lower bound comes first: what the face shows with its reach under the
    // cover, and nothing round the sides that meet no cover.
    let pts: Vec<Point> = face_rings(inp, f).into_iter().flatten().collect();
    let snap = |t: f64, of: fn(&Point) -> f64| -> f64 {
        pts.iter()
            .map(of)
            .filter(|q| (q - t).abs() <= region::DY)
            .min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs()))
            .unwrap_or(t)
    };
    for (b, grow) in [(l.bbox(), &[0.0][..]), (v.bbox(), &[REACH, 0.5, 0.0][..])] {
        let Some((bx0, by0, bx1, by1)) = b else {
            continue;
        };
        let snapped = (snap(bx0, |q| q.x), snap(by0, |q| q.y), snap(bx1, |q| q.x), snap(by1, |q| q.y));
        for (vx0, vy0, vx1, vy1) in [snapped, (bx0, by0, bx1, by1)] {
            for &e in grow {
                let kind = PrimitiveKind::RoundRect {
                    x: vx0 - e,
                    y: vy0 - e,
                    w: vx1 - vx0 + 2.0 * e,
                    h: vy1 - vy0 + 2.0 * e,
                    rx: 0.0,
                };
                if let Some(reg) = bounded_region(&[shape::flatten_primitive(&kind)], hb) {
                    consider(Shape::Prim(kind), 6.0, 0.0, &reg);
                }
            }
        }
    }
    // The face's own rings with the covered runs replaced.
    let simplified = simplified_rings(inp, f, e_now, &u, &l, &h, hb);
    if let Some((d, reg, allow)) = &simplified {
        consider(Shape::Path(d.clone()), gate_count(d), *allow, reg);
    }
    if inkvec_core::env::number("INKVEC_COMPLETION_DUMP").is_some_and(|x| x as usize == f) {
        dump(f, &[(&u, [90, 90, 200]), (&v, [200, 200, 90]), (&l.minus(&v), [90, 200, 90])],
             simplified.as_ref().map(|s| &s.1), (x0 - PAD, y0 - PAD, x1 + PAD, y1 + PAD));
    }
    best
}

/// Debugging (`INKVEC_COMPLETION_DUMP=<face>`): the regions as colours, the candidate's edge in
/// red, eight pixels per pixel, written to `layers-<face>.ppm` in the working directory.
fn dump(f: usize, layers: &[(&Region, [u8; 3])], cand: Option<&Region>, b: (f64, f64, f64, f64)) {
    const S: f64 = 8.0;
    let (w, h) = (((b.2 - b.0) * S) as usize + 1, ((b.3 - b.1) * S) as usize + 1);
    if w * h > 40_000_000 {
        return;
    }
    let mut img = vec![[255u8, 255, 255]; w * h];
    let inside = |r: &Region, x: f64, y: f64| r.contains(Point::new(x, y));
    for py in 0..h {
        for px in 0..w {
            let (x, y) = (b.0 + px as f64 / S, b.1 + py as f64 / S);
            for (r, c) in layers {
                if inside(r, x, y) {
                    img[py * w + px] = *c;
                }
            }
            if let Some(c) = cand {
                let here = inside(c, x, y);
                let right = inside(c, x + 1.0 / S, y);
                let down = inside(c, x, y + 1.0 / S);
                if here != right || here != down {
                    img[py * w + px] = [220, 30, 30];
                }
            }
        }
    }
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    out.extend(img.iter().flatten());
    let _ = std::fs::write(format!("layers-{f}.ppm"), out);
}

/// The region of `polys`, or `None` when they leave the box `hb` (the allowed region's) by
/// more than the check's slack: such a shape spills for certain, and a primitive fitted to
/// nearly straight points can be large enough to exhaust memory as a region.
fn bounded_region(polys: &[Vec<Point>], hb: (f64, f64, f64, f64)) -> Option<Region> {
    let s = check::LINE_SLACK + region::DY;
    let inside = polys.iter().flatten().all(|p| {
        p.x.is_finite()
            && p.y.is_finite()
            && p.x >= hb.0 - s
            && p.x <= hb.2 + s
            && p.y >= hb.1 - s
            && p.y <= hb.3 + s
    });
    inside.then(|| Region::from_polygons(polys, Rule::EvenOdd))
}

/// Points of the face's outline that no cover reaches within [`REACH`]: the boundary it
/// owns, through which a primitive completion must pass.
fn owned_points(inp: &Input, f: usize, u: &Region) -> Vec<Point> {
    let near = u.dilate(REACH);
    let mut pts: Vec<Point> = face_rings(inp, f)
        .into_iter()
        .flatten()
        .filter(|&p| !near.contains(p))
        .collect();
    // Evenly thinned, so a long straight run does not outweigh a curved one.
    if pts.len() > 400 {
        let step = pts.len() as f64 / 400.0;
        pts = (0..400).map(|i| pts[(i as f64 * step) as usize]).collect();
    }
    pts
}

/// A ring as cyclic vertices and the segment leaving each one.
#[derive(Clone)]
struct CycRing {
    verts: Vec<Point>,
    segs: Vec<Segment>,
    /// Covered by something painted over the face, and not yet tried.
    covered: Vec<bool>,
}

impl CycRing {
    fn len(&self) -> usize {
        self.verts.len()
    }

    /// The segment leaving vertex `i`, re-targeted to end at vertex `i + 1`.
    fn seg(&self, i: usize) -> Segment {
        let to = self.verts[(i + 1) % self.len()];
        match self.segs[i] {
            Segment::Line(_) => Segment::Line(to),
            Segment::Cubic(a, b, _) => Segment::Cubic(a, b, to),
            Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                ..
            } => Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end: to,
            },
        }
    }

    fn polygon(&self) -> Vec<Point> {
        let mut pts = Vec::new();
        for i in 0..self.len() {
            if i == 0 {
                pts.push(self.verts[0]);
            }
            shape::flatten_segment(self.verts[i], &self.seg(i), &mut pts);
        }
        if pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        pts
    }

    /// Path data, closed by `Z`: a final line back to the start is left to the `Z`.
    fn d(&self, decimals: usize, out: &mut String) {
        let mut segs: Vec<Segment> = (0..self.len()).map(|i| self.seg(i)).collect();
        if matches!(segs.last(), Some(Segment::Line(_))) {
            segs.pop();
        }
        fmt_segments(self.verts[0], &segs, decimals, out);
    }
}

/// The face's outline rings with each run of covered segments replaced by a cheaper piece
/// that keeps the face within its interval: the corner its owned neighbours meet at, else a
/// chord a margin under the cover, else a chord at the run's ends. Each replacement is kept
/// only when the whole face still passes the check. `None` when no ring changed, or when a
/// ring is a primitive.
fn simplified_rings(
    inp: &Input,
    f: usize,
    e_now: &Region,
    u: &Region,
    v: &Region,
    h: &Region,
    hb: (f64, f64, f64, f64),
) -> Option<(String, Region, f64)> {
    let rings = inp.order.get(f)?;
    let mut cyc: Vec<CycRing> = Vec::new();
    for &k in &inp.outer[f] {
        let ring = &rings[k];
        if ring.len() == 1 && inp.prims.get(ring[0].0).and_then(|p| p.as_ref()).is_some() {
            return None;
        }
        let (start, mut segs) = ring_to_segments(ring, inp.fitted);
        if segs.len() < 2 {
            return None;
        }
        if segs.last().map(|s| s.end().dist(start)).unwrap_or(0.0) > 1e-6 {
            segs.push(Segment::Line(start));
        }
        let mut verts = vec![start];
        for s in &segs[..segs.len() - 1] {
            verts.push(s.end());
        }
        let covered = (0..segs.len())
            .map(|i| segment_covered(verts[i], &segs[i], e_now, u))
            .collect();
        cyc.push(CycRing { verts, segs, covered });
    }
    let region_of = |cyc: &[CycRing]| -> Option<Region> {
        let polys: Vec<Vec<Point>> = cyc.iter().map(CycRing::polygon).collect();
        bounded_region(&polys, hb)
    };
    // Each run is judged in a window round what it changes: the face's other runs still lie
    // on their cover's edge until their own turn, and would fail the reach of the lower
    // bound anywhere. The whole face is certified once at the end, by the caller.
    let mut changed = false;
    let mut allowance = 0.0;
    let mut current = region_of(&cyc)?;
    for r in 0..cyc.len() {
        loop {
            let Some((a, b)) = next_run(&cyc[r]) else {
                break;
            };
            // Whatever happens, this run has been tried; the options inherit that, so one
            // that keeps some of the run's segments does not hand them back untried.
            mark_tried(&mut cyc[r], a, b);
            let mut options = corner_cuts(&cyc[r], a, b);
            options.extend(replacements(&cyc[r], a, b, e_now));
            options.extend(offset_run(&cyc[r], a, b, e_now).map(|o| (o, 0.0)));
            // The lower bound along this run: the band round its own geometry. The face's
            // other runs keep their own reach to meet, in their own turn.
            let band = run_band(&cyc[r], a, b);
            let need = v.intersect(&band);
            for (opt, extra) in options {
                let mut trial = cyc.clone();
                trial[r] = opt;
                let Some(reg) = region_of(&trial) else {
                    continue;
                };
                let Some((wx0, wy0, wx1, wy1)) = reg.minus(&current).union(&current.minus(&reg)).bbox() else {
                    continue;
                };
                let w = WINDOW;
                let crop = |x: &Region| x.crop(wx0 - w, wy0 - w, wx1 + w, wy1 + w);
                let verdict = check::Interval {
                    lost: need.minus(&reg).widest_line(),
                    spilled: crop(&reg).minus(&crop(h)).widest_line(),
                };
                if trace() {
                    eprintln!(
                        "layers face {f} ring {r} run {a}..={b}: lost {:.3} spilled {:.3}",
                        verdict.lost, verdict.spilled
                    );
                }
                if verdict.holds() {
                    cyc = trial;
                    current = reg;
                    allowance += extra;
                    changed = true;
                    break;
                }
            }
        }
    }
    if !changed {
        return None;
    }
    let mut d = String::new();
    for c in &cyc {
        c.d(inp.decimals, &mut d);
    }
    Some((d, region_of(&cyc)?, allowance))
}

/// The band round run `a ..= b` of a ring: its own geometry widened by the reach and a scan
/// line, square at its ends.
fn run_band(c: &CycRing, a: usize, b: usize) -> Region {
    let n = c.len();
    let mut pts = vec![c.verts[a]];
    let mut i = a;
    loop {
        shape::flatten_segment(c.verts[i], &c.seg(i), &mut pts);
        if i == b {
            break;
        }
        i = (i + 1) % n;
    }
    let path = FittedPath {
        start: pts[0],
        segments: pts[1..].iter().map(|&p| Segment::Line(p)).collect(),
        closed: false,
    };
    shape::stroke_band(&[path], 2.0 * (UNDER + region::DY), EndCap::Butt, JoinKind::Round)
}

/// Run `a ..= b` moved [`UNDER`] under its cover, its ends kept: every interior vertex along
/// its outward normal, every cubic control point along its segment's. The underlap of
/// [`crate::seams`], done inside the completion, for a run nothing cheaper can replace. The
/// same segments, so the same numbers. `None` when the run holds an arc or nothing moves.
fn offset_run(c: &CycRing, a: usize, b: usize, e_now: &Region) -> Option<CycRing> {
    let n = c.len();
    let outward = |p: Point, q: Point| -> Option<Point> {
        let d = unit(Point::new(q.x - p.x, q.y - p.y))?;
        let nrm = Point::new(-d.y, d.x);
        let mid = Point::new(0.5 * (p.x + q.x), 0.5 * (p.y + q.y));
        let probe = Point::new(mid.x + 0.35 * nrm.x, mid.y + 0.35 * nrm.y);
        Some(if e_now.contains(probe) {
            Point::new(-nrm.x, -nrm.y)
        } else {
            nrm
        })
    };
    let mut out = c.clone();
    let mut moved = false;
    let mut i = a;
    loop {
        let (p, q) = (c.verts[i], c.verts[(i + 1) % n]);
        match c.segs[i] {
            Segment::Arc { .. } => return None,
            Segment::Cubic(c1, c2, e) => {
                let nrm = outward(p, q)?;
                let mv = |x: Point| Point::new(x.x + UNDER * nrm.x, x.y + UNDER * nrm.y);
                out.segs[i] = Segment::Cubic(mv(c1), mv(c2), e);
                moved = true;
            }
            Segment::Line(_) => {}
        }
        if i == b {
            break;
        }
        // The vertex between segment i and segment i + 1 is interior to the run.
        let j = (i + 1) % n;
        let (n0, n1) = (outward(p, q), outward(q, c.verts[(j + 1) % n]));
        if let (Some(n0), Some(n1)) = (n0, n1) {
            if let Some(m) = unit(Point::new(n0.x + n1.x, n0.y + n1.y)) {
                out.verts[j] = Point::new(q.x + UNDER * m.x, q.y + UNDER * m.y);
                moved = true;
            }
        }
        i = j;
    }
    moved.then_some(out)
}

/// Whether segment `seg` from `a` lies along a cover: at its quarter points, the point a
/// third of a pixel off it on the side away from the face is under the cover.
fn segment_covered(a: Point, seg: &Segment, e_now: &Region, u: &Region) -> bool {
    let at = |t: f64| -> (Point, Point) {
        let p = point_on(a, seg, t);
        let q = point_on(a, seg, (t + 0.01).min(1.0));
        let o = point_on(a, seg, (t - 0.01).max(0.0));
        (p, Point::new(q.x - o.x, q.y - o.y))
    };
    [0.25, 0.5, 0.75].iter().all(|&t| {
        let (p, d) = at(t);
        let l = d.x.hypot(d.y);
        if l < 1e-12 {
            return false;
        }
        let n = Point::new(-d.y / l * 0.35, d.x / l * 0.35);
        let (p1, p2) = (Point::new(p.x + n.x, p.y + n.y), Point::new(p.x - n.x, p.y - n.y));
        let (in1, in2) = (e_now.contains(p1), e_now.contains(p2));
        match (in1, in2) {
            (true, false) => u.contains(p2),
            (false, true) => u.contains(p1),
            _ => false,
        }
    })
}

/// The point at parameter `t` of segment `seg` starting at `a`.
fn point_on(a: Point, seg: &Segment, t: f64) -> Point {
    match *seg {
        Segment::Line(p) => Point::new(a.x + t * (p.x - a.x), a.y + t * (p.y - a.y)),
        Segment::Cubic(c1, c2, p) => inkvec_fit::curves::eval_cubic([a, c1, c2, p], t),
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            let f = inkvec_fit::curves::arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
            f.at(f.theta1 + f.delta * t)
        }
    }
}

/// The first run of covered, untried segments `a ..= b` (cyclic indices), with an
/// uncovered segment before it. `None` when there is none, or when every segment is
/// covered (a primitive completion's case).
fn next_run(c: &CycRing) -> Option<(usize, usize)> {
    let n = c.len();
    if c.covered.iter().all(|&x| x) {
        return None;
    }
    // Start the scan just after an uncovered segment, so no run wraps unseen.
    let s0 = c.covered.iter().position(|&x| !x)?;
    for o in 1..=n {
        let i = (s0 + o) % n;
        if c.covered[i] {
            let mut b = i;
            while c.covered[(b + 1) % n] && (b + 1) % n != i {
                b = (b + 1) % n;
            }
            return Some((i, b));
        }
    }
    None
}

/// Marks segments `a ..= b` tried.
fn mark_tried(c: &mut CycRing, a: usize, b: usize) {
    let n = c.len();
    let mut i = a;
    loop {
        c.covered[i] = false;
        if i == b {
            break;
        }
        i = (i + 1) % n;
    }
}

/// The rings with run `a ..= b` replaced, best first: the corner of the owned lines on
/// either side, then a chord reaching [`REACH`] and half that under the cover, then a
/// plain chord. Only replacements that write fewer numbers than the run.
fn replacements(c: &CycRing, a: usize, b: usize, e_now: &Region) -> Vec<(CycRing, f64)> {
    let n = c.len();
    let run_len = if b >= a { b - a + 1 } else { b + n - a + 1 };
    if run_len >= n {
        return Vec::new();
    }
    let run_cost: f64 = (0..run_len).map(|o| c.seg((a + o) % n).params()).sum();
    let prev = (a + n - 1) % n; // owned segment ending at vertex a
    let next = (b + 1) % n; // owned segment starting at vertex b + 1
    let va = c.verts[a];
    let vb = c.verts[next];
    let mut out = Vec::new();

    // Removes the run's interior vertices a+1 ..= b and its segments, splicing `insert`
    // (vertices, each followed by a line) after vertex a, which becomes `new_a`.
    let splice = |new_a: Point, mid: &[Point], new_b: Point| -> CycRing {
        let mut verts = Vec::new();
        let mut segs = Vec::new();
        let mut covered = Vec::new();
        // Walk from `next` round to `prev`, then the replacement.
        let mut i = next;
        loop {
            verts.push(if i == next { new_b } else { c.verts[i] });
            segs.push(c.segs[i].clone());
            covered.push(c.covered[i]);
            if i == prev {
                break;
            }
            i = (i + 1) % n;
        }
        // Vertex a (moved) and the replacement's own vertices, joined by lines.
        verts.push(new_a);
        segs.push(Segment::Line(Point::new(0.0, 0.0)));
        covered.push(false);
        for &m in mid {
            verts.push(m);
            segs.push(Segment::Line(Point::new(0.0, 0.0)));
            covered.push(false);
        }
        // The last pushed line runs to `new_b`, the first vertex.
        CycRing {
            verts,
            segs,
            covered,
        }
    };

    let is_line = |i: usize| matches!(c.segs[i], Segment::Line(_));
    // 1. The corner where the owned lines on either side meet.
    if is_line(prev) && is_line(next) && prev != next {
        let p0 = c.verts[prev];
        let q1 = c.verts[(next + 1) % n];
        if let Some((x, s, t)) = intersect(p0, va, vb, q1) {
            // Beyond `va` along the incoming line and before `vb` along the outgoing one.
            if s > 1.0 && t < 0.0 {
                // The corner replaces both va and vb: the incoming line now ends at x and
                // the outgoing one starts there. Encoded as a zero-length line from x to x,
                // dropped below.
                let mut r = splice(x, &[], x);
                // `splice` put x first (as new_b) and last (as new_a), joined by a line of
                // zero length: drop that last vertex.
                r.verts.pop();
                r.segs.pop();
                r.covered.pop();
                if run_cost > 0.0 {
                    out.push((r, 0.0));
                }
            }
        }
    }
    // 2. A chord a margin under the cover, the owned lines extended to reach it; 3. a plain
    //    chord between the run's ends.
    for d in [REACH, 0.5 * REACH, 0.0] {
        // A plain chord over one line is that line.
        if d == 0.0 && run_len == 1 && matches!(c.segs[a], Segment::Line(_)) {
            continue;
        }
        let ta = tangent_at_end(c, prev);
        let tb = tangent_at_start(c, next);
        let (Some(ta), Some(tb)) = (ta, tb) else {
            continue;
        };
        let new_a = Point::new(va.x + d * ta.x, va.y + d * ta.y);
        let new_b = Point::new(vb.x - d * tb.x, vb.y - d * tb.y);
        // A line's end moves with its vertex; a curve keeps its end and gains a line.
        let mut mid = Vec::new();
        let a_vertex = if d > 0.0 && !is_line(prev) {
            mid.push(new_a);
            va
        } else {
            new_a
        };
        let b_vertex = if d > 0.0 && !is_line(next) {
            mid.push(new_b);
            vb
        } else {
            new_b
        };
        let r = splice(a_vertex, &mid, b_vertex);
        let new_cost: f64 = 2.0 * (mid.len() + 1) as f64;
        // No more than the run: a chord that costs what the run did still pays, because the
        // face then reaches under its cover and needs no underlap ([`crate::seams`]), which
        // adds vertices.
        if new_cost <= run_cost {
            out.push((r, 0.0));
        }
    }
    // 4. A straight run that continues its owned neighbours cannot be reached under by a
    //    chord: bulge it under the cover by [`UNDER`], with one vertex at its middle or two
    //    a taper in from its ends -- what the underlap would add, and charged as such (the
    //    second number is the allowance it spends).
    if run_len == 1 && is_line(a) {
        let len = va.dist(vb);
        if let (Some(dir), Some(nrm)) = (unit(Point::new(vb.x - va.x, vb.y - va.y)), outward_normal(va, vb, e_now)) {
            let at = |t: f64| Point::new(va.x + t * dir.x + UNDER * nrm.x, va.y + t * dir.y + UNDER * nrm.y);
            out.push((splice(va, &[at(0.5 * len)], vb), 2.0));
            let t = TAPER.min(len / 3.0);
            if len > 2.0 * t {
                out.push((splice(va, &[at(t), at(len - t)], vb), 4.0));
            }
        }
    }
    out
}

/// Corners a blur cut off, restored under the cover: each segment `k` of run `a ..= b`
/// between two lines is dropped, and the lines extended to meet, when they meet beyond the
/// end of the incoming one and before the start of the outgoing one. At an acute corner a
/// blur (anti-aliasing, a resampling filter, a lossy codec's) removes the tip, and the fit
/// writes the chamfer it leaves as one more segment; under a cover nobody can see the tip,
/// so the face takes the artist's corner back for free. First every such cut at once, then
/// each alone; each writes the numbers of the segments it drops fewer.
fn corner_cuts(c: &CycRing, a: usize, b: usize) -> Vec<(CycRing, f64)> {
    let n = c.len();
    let run_len = if b >= a { b - a + 1 } else { b + n - a + 1 };
    // The run's segments, by the vertex each starts at (indices shift as cuts are made).
    let starts: Vec<Point> = (0..run_len).map(|o| c.verts[(a + o) % n]).collect();
    let mut out = Vec::new();
    let mut all = c.clone();
    let mut cuts = 0;
    for &p in &starts {
        let Some(k) = all.verts.iter().position(|&q| q == p) else {
            continue;
        };
        if let Some(cut) = cut_corner(&all, k) {
            all = cut;
            cuts += 1;
        }
    }
    if cuts > 1 {
        out.push((all, 0.0));
    }
    for o in 0..run_len {
        if let Some(cut) = cut_corner(c, (a + o) % n) {
            out.push((cut, 0.0));
        }
    }
    out
}

/// Ring `c` with segment `k` dropped and the lines either side extended to their meeting
/// point, or `None` when either neighbour is not a line, the ring would fall below three
/// segments, or the lines do not meet ahead of the incoming one and behind the outgoing one.
fn cut_corner(c: &CycRing, k: usize) -> Option<CycRing> {
    let n = c.len();
    if n < 4 {
        return None;
    }
    let prev = (k + n - 1) % n;
    let next = (k + 1) % n;
    if !matches!(c.segs[prev], Segment::Line(_)) || !matches!(c.segs[next], Segment::Line(_)) {
        return None;
    }
    let (p0, p1) = (c.verts[prev], c.verts[k]);
    let (q0, q1) = (c.verts[next], c.verts[(next + 1) % n]);
    let (x, s, t) = intersect(p0, p1, q0, q1)?;
    if !(s > 1.0 && t < 0.0) {
        return None;
    }
    let mut out = c.clone();
    // Vertex k becomes the corner; vertex k + 1 and segment k go. The segment now leaving
    // vertex k is the old segment k + 1, a line re-targeted to its own end by `seg`.
    out.verts[k] = x;
    out.segs[k] = c.segs[next].clone();
    out.covered[k] = c.covered[next];
    out.verts.remove(next);
    out.segs.remove(next);
    out.covered.remove(next);
    Some(out)
}

/// The unit normal of the chord `p → q` pointing away from the face (`e_now`).
fn outward_normal(p: Point, q: Point, e_now: &Region) -> Option<Point> {
    let d = unit(Point::new(q.x - p.x, q.y - p.y))?;
    let nrm = Point::new(-d.y, d.x);
    let mid = Point::new(0.5 * (p.x + q.x), 0.5 * (p.y + q.y));
    let probe = Point::new(mid.x + 0.35 * nrm.x, mid.y + 0.35 * nrm.y);
    Some(if e_now.contains(probe) {
        Point::new(-nrm.x, -nrm.y)
    } else {
        nrm
    })
}

/// The unit direction a segment leaves vertex `i` in.
fn tangent_at_start(c: &CycRing, i: usize) -> Option<Point> {
    let a = c.verts[i];
    let s = c.seg(i);
    let q = match s {
        Segment::Line(p) => p,
        Segment::Cubic(c1, c2, p) => [c1, c2, p].into_iter().find(|q| q.dist(a) > 1e-9)?,
        Segment::Arc { .. } => point_on(a, &s, 0.01),
    };
    unit(Point::new(q.x - a.x, q.y - a.y))
}

/// The unit direction a segment arrives at its end in.
fn tangent_at_end(c: &CycRing, i: usize) -> Option<Point> {
    let a = c.verts[i];
    let s = c.seg(i);
    let e = s.end();
    let q = match s {
        Segment::Line(_) => a,
        Segment::Cubic(c1, c2, _) => [c2, c1, a].into_iter().find(|q| q.dist(e) > 1e-9)?,
        Segment::Arc { .. } => point_on(a, &s, 0.99),
    };
    unit(Point::new(e.x - q.x, e.y - q.y))
}

fn unit(v: Point) -> Option<Point> {
    let l = v.x.hypot(v.y);
    (l > 1e-12).then(|| Point::new(v.x / l, v.y / l))
}

/// The intersection of line `p0 → p1` with line `q0 → q1`: the point, and its parameters
/// `s` on the first (0 at `p0`, 1 at `p1`) and `t` on the second (0 at `q0`, 1 at `q1`).
fn intersect(p0: Point, p1: Point, q0: Point, q1: Point) -> Option<(Point, f64, f64)> {
    let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
    let (ex, ey) = (q1.x - q0.x, q1.y - q0.y);
    let den = dx * ey - dy * ex;
    let scale = (dx.hypot(dy) * ex.hypot(ey)).max(1e-12);
    if den.abs() < 1e-6 * scale {
        return None;
    }
    let (fx, fy) = (q0.x - p0.x, q0.y - p0.y);
    let s = (fx * ey - fy * ex) / den;
    let t = (fx * dy - fy * dx) / den;
    Some((Point::new(p0.x + s * dx, p0.y + s * dy), s, t))
}

#[cfg(test)]
mod tests;
