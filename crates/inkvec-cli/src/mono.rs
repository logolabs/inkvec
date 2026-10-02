//! Monochrome output (`--monochrome`): the drawing in two tones, black artwork on a white
//! or transparent ground.
//!
//! This runs on the finished colour trace, after the fit, so both engines get it and the
//! geometry is the colour trace's own: every boundary is where the colour tracer placed it,
//! to a fraction of a pixel, and the black shape is drawn with those exact curves. Nothing is
//! thresholded on lightness. A lightness threshold (the `--bilevel` mode) has to choose which
//! inks are dark, and a light ink -- yellow on white -- falls on the wrong side of it; here a
//! colour is artwork because it differs from what it is drawn on, whatever its lightness.
//!
//! Each face of the planar map becomes **ground** (white, or transparent) or **ink** (black):
//!
//! * **The ground** is found on the image border. Where most of the border is transparent,
//!   the ground is transparency itself, and anything the source drew at least half opaque on
//!   it is artwork: white lettering on a transparent PNG comes back black. Otherwise the
//!   ground is the colour covering most of the border (faces of nearly one colour counted
//!   together), so a white-on-black image has a black ground and its white lettering is the
//!   artwork.
//! * **Every shape takes the opposite tone of the shape it sits in, unless it is the same
//!   colour** (within [`SAME_COLOUR`] in OKLab). A red mark on white is black; the white
//!   counter inside a letter is ground again, so it is a hole in the black, not a patch on
//!   it; black lettering on a yellow panel is knocked out of the black panel, where painting
//!   every non-white colour black would have lost it in one silhouette. This is what a
//!   one-colour version of a logo does, and it keeps every nested boundary the drawing has.
//! * **Hairline faces** -- under [`THIN_WIDTH`] wide, the anti-aliased rims and slivers a
//!   colour trace sometimes keeps -- are not trusted as containers: a shape inside one is
//!   judged against the face around the hairline. A hairline that is (a blend of) the colour
//!   of a neighbour joins that neighbour's tone, so an anti-aliased rim neither fattens the
//!   black nor cracks it; one that is none of its neighbours' colours is a real line and
//!   follows the rule above.
//!
//! The ink faces are then merged into one face the way [`crate::pipeline`] merges a
//! translucent layer's pieces: an edge with ink on both sides becomes interior and leaves
//! every ring, so what is left is the outline of the union, drawn once as one even-odd
//! path. Only pure black is written -- no grey, no pale fringe -- so an anti-aliased edge is
//! the renderer's coverage of a black shape: over transparency its colour channels are black
//! and only its alpha varies.
//!
//! Called from [`crate::pipeline`] after the colour fit, in place of the colour emitter:
//! [`classify`] reads the finished trace and returns each face's tone, [`emit`] writes the
//! document and [`report`] its report line. Colour distances are Euclidean in OKLab.

use inkvec_core::Point;
use inkvec_fit::{primitives::PrimitiveFit, FittedPath};
use inkvec_trace::{
    color::{rgb_to_oklab, Oklab},
    gradient::{FillFit, FillModel},
    planar::{self, PlanarMap},
    Palette,
};

use crate::faces::FaceRings;
use crate::pathdata::{emit_decimals, fmt_ring};
use crate::primitive::primitive_d;
use crate::rings::{ring_area, ring_points, MIN_RING_AREA};

/// OKLab distance within which two faces are one colour: a shape this close to what it sits
/// in takes the same tone, and a face this close to the border's colour is the ground.
///
/// About three times the palette's own merge distance (0.035): a face this close is the
/// same colour drawn a shade off -- paper tone, a compression artefact, a light rim the
/// palette kept -- and not a mark of its own. Against white: #e6e6e6 is 0.075 and pale pink
/// #ffe0e0 0.076, the same colour; #c8c8c8 is 0.167 and yellow #ffff00 0.213, marks.
pub(crate) const SAME_COLOUR: f32 = 0.1;

/// Below this opacity a face is part of a transparent ground, not artwork.
const INK_OPACITY: f32 = 0.5;

/// Faces narrower than this, in pixels (twice the area over the perimeter), are hairlines:
/// anti-aliased rims and slivers, not containers.
const THIN_WIDTH: f64 = 1.2;

/// What [`classify`] decided.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ground {
    /// Per face: painted black.
    pub ink: Vec<bool>,
    /// The ground's colour, or `None` when the ground is transparent.
    pub colour: Option<[f32; 3]>,
}

/// What the tone of each face is decided from, per face.
#[derive(Debug, Clone, Default)]
pub(crate) struct Evidence {
    /// The colours a face shows: one for a flat fill, every stop of a gradient.
    pub stops: Vec<Vec<[f32; 3]>>,
    /// Transparent or under half opaque in the source: always ground.
    pub see_through: Vec<bool>,
    /// Pixels of the image border the face covers.
    pub border: Vec<usize>,
    /// Twice the area over the perimeter, in pixels: a strip's width.
    pub width: Vec<f64>,
    /// The smallest face containing this one.
    pub parent: Vec<Option<usize>>,
    /// Faces sharing a boundary with this one.
    pub neighbours: Vec<Vec<usize>>,
}

/// The mean of some colours, in OKLab.
fn mean(stops: &[[f32; 3]]) -> Oklab {
    let n = stops.len().max(1) as f32;
    let (mut l, mut a, mut b) = (0.0, 0.0, 0.0);
    for &c in stops {
        let o = rgb_to_oklab(c);
        l += o.l;
        a += o.a;
        b += o.b;
    }
    Oklab {
        l: l / n,
        a: a / n,
        b: b / n,
    }
}

/// Distance from `p` to the segment `a`-`b` in OKLab.
///
/// The closest point is `a + t·(b - a)` with `t = ((p - a)·(b - a)) / |b - a|²` clamped to
/// `0..1` (the projection onto the line, kept on the segment); `t = 0` when `a = b`.
fn to_segment(p: Oklab, a: Oklab, b: Oklab) -> f32 {
    let d = [b.l - a.l, b.a - a.a, b.b - a.b];
    let q = [p.l - a.l, p.a - a.a, p.b - a.b];
    let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    let t = if dd > 0.0 {
        ((q[0] * d[0] + q[1] * d[1] + q[2] * d[2]) / dd).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let e = [q[0] - t * d[0], q[1] - t * d[1], q[2] - t * d[2]];
    (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt()
}

/// Distance from `p` to the colours `stops` runs through: to a flat colour the plain
/// distance, to a gradient the distance to the nearest colour along it.
fn to_stops(p: Oklab, stops: &[[f32; 3]]) -> f32 {
    let pts: Vec<Oklab> = stops.iter().map(|&c| rgb_to_oklab(c)).collect();
    match pts.as_slice() {
        [] => f32::INFINITY,
        [one] => p.dist(*one),
        _ => pts
            .windows(2)
            .map(|w| to_segment(p, w[0], w[1]))
            .fold(f32::INFINITY, f32::min),
    }
}

/// The ground: `None` for transparency (most of the border clear, or no border at all),
/// else the face whose colour covers the most border.
fn ground_face(ev: &Evidence, means: &[Oklab]) -> Option<usize> {
    let n = ev.stops.len();
    let clear = |f: usize| ev.see_through.get(f).copied().unwrap_or(false);
    let border = |f: usize| ev.border.get(f).copied().unwrap_or(0);
    let total: usize = (0..n).map(border).sum();
    let clear_border: usize = (0..n).filter(|&f| clear(f)).map(border).sum();
    if total == 0 || 2 * clear_border >= total {
        return None;
    }
    let on_border: Vec<usize> = (0..n).filter(|&f| border(f) > 0 && !clear(f)).collect();
    let share = |g: usize| -> usize {
        on_border
            .iter()
            .filter(|&&f| to_stops(means[f], &ev.stops[g]) <= SAME_COLOUR)
            .map(|&f| border(f))
            .sum()
    };
    on_border
        .iter()
        .copied()
        .max_by_key(|&g| (share(g), border(g), std::cmp::Reverse(g)))
}

/// The tone of every face. See the module documentation for the rules.
pub(crate) fn decide(ev: &Evidence) -> Ground {
    let n = ev.stops.len();
    let clear = |f: usize| ev.see_through.get(f).copied().unwrap_or(false);
    let means: Vec<Oklab> = ev.stops.iter().map(|s| mean(s)).collect();
    let ground = ground_face(ev, &means);

    let thin = |f: usize| ev.width.get(f).is_some_and(|&w| w < THIN_WIDTH);
    let parent = |f: usize| ev.parent.get(f).copied().flatten().filter(|&p| p < n);
    // The face a shape is judged against: its container, past any hairline.
    let judge = |f: usize| -> Option<usize> {
        let mut p = parent(f);
        let mut steps = 0;
        while let Some(q) = p {
            if !thin(q) || steps > n {
                break;
            }
            p = parent(q);
            steps += 1;
        }
        p
    };
    let depth = |f: usize| -> usize {
        let (mut d, mut p) = (0, parent(f));
        while let Some(q) = p {
            d += 1;
            if d > n {
                break;
            }
            p = parent(q);
        }
        d
    };

    let mut ink: Vec<Option<bool>> = vec![None; n];
    // The opposite of what the face sits in, unless it is the same colour.
    let by_container = |f: usize, ink: &[Option<bool>]| -> bool {
        if clear(f) {
            return false;
        }
        match judge(f) {
            // Opaque paint on the transparent ground, or on a clear hole, is artwork.
            Some(p) if clear(p) => true,
            Some(p) => {
                let p_ink = ink[p].unwrap_or(false);
                if to_stops(means[f], &ev.stops[p]) <= SAME_COLOUR {
                    p_ink
                } else {
                    !p_ink
                }
            }
            None => match ground {
                Some(g) => to_stops(means[f], &ev.stops[g]) > SAME_COLOUR,
                None => true,
            },
        }
    };

    // Containers first, outermost first, so every face's container is settled before it.
    let mut by_depth: Vec<usize> = (0..n).collect();
    by_depth.sort_by_key(|&f| (depth(f), f));
    for &f in by_depth.iter().filter(|&&f| !thin(f)) {
        ink[f] = Some(by_container(f, &ink));
    }
    // Hairlines: a rim of a neighbour's colour, or a blend of two neighbours, joins the
    // nearer; anything else is a line of its own.
    for &f in by_depth.iter().filter(|&&f| thin(f)) {
        if clear(f) {
            ink[f] = Some(false);
            continue;
        }
        let nbrs: Vec<usize> = ev
            .neighbours
            .get(f)
            .map(|v| {
                v.iter()
                    .copied()
                    .filter(|&q| q < n && !thin(q) && !clear(q) && ink[q].is_some())
                    .collect()
            })
            .unwrap_or_default();
        let dist = |q: usize| to_stops(means[f], &ev.stops[q]);
        let nearest = nbrs
            .iter()
            .copied()
            .min_by(|&a, &b| dist(a).total_cmp(&dist(b)));
        let between = nbrs.iter().enumerate().any(|(i, &a)| {
            nbrs[i + 1..]
                .iter()
                .any(|&b| to_segment(means[f], means[a], means[b]) <= SAME_COLOUR)
        });
        ink[f] = Some(match nearest {
            Some(q) if dist(q) <= SAME_COLOUR || between => ink[q].unwrap_or(false),
            _ => by_container(f, &ink),
        });
    }

    let colour = ground.map(|g| match ev.stops[g].as_slice() {
        [c] => *c,
        s => inkvec_trace::color::oklab_to_rgb(mean(s)),
    });
    Ground {
        ink: ink.into_iter().map(|i| i.unwrap_or(false)).collect(),
        colour,
    }
}

/// The colours a face shows: one for a flat fill, every stop of a gradient.
fn face_stops(f: usize, fills: &[FillFit], face_color: &[usize], pal: &Palette) -> Vec<[f32; 3]> {
    match fills.get(f).map(|x| &x.model) {
        Some(FillModel::Flat(c)) => vec![*c],
        Some(FillModel::Linear { c0, c1, mids, .. } | FillModel::Radial { c0, c1, mids, .. }) => {
            let mut v = vec![*c0];
            v.extend(mids.iter().map(|m| m.1));
            v.push(*c1);
            v
        }
        None => face_color
            .get(f)
            .and_then(|&ci| pal.rgb.get(ci))
            .map(|&c| vec![c])
            .unwrap_or_else(|| vec![[0.0; 3]]),
    }
}

/// The finished colour trace, as [`classify`] reads it.
pub(crate) struct Trace<'a> {
    /// The planar map: faces and the shared edges between them.
    pub map: &'a PlanarMap,
    /// The rings of each face.
    pub order: &'a [FaceRings],
    /// The fitted curve of each edge.
    pub fitted: &'a [FittedPath],
    /// Per-pixel face id.
    pub labels: &'a [u16],
    /// Per face, its fill model.
    pub fills: &'a [FillFit],
    /// Per face, its palette index.
    pub face_color: &'a [usize],
    /// The palette.
    pub pal: &'a Palette,
    /// Per face, transparent in the source.
    pub clear: &'a [bool],
    /// Per face, the opacity the source drew it at.
    pub opacity: &'a [f32],
}

/// Per face, the face it sits on: of the faces across its outline that enclose more area
/// than it does, the one it shares the most boundary with; `None` on the canvas edge or for
/// the largest face around.
///
/// Not the smallest face that contains it geometrically, though that is the same face
/// whenever a shape is an island in another. Drawings overlap: on a Noto emoji the tears
/// run over the edge of the face, so no single face encloses the eyes -- the yellow is a
/// C-shape -- and by containment the eyes, the mouth and the yellow all sat on the
/// transparent ground and came out as one black silhouette. What a shape is drawn *on* is
/// what is around it, and that is what this asks. Requiring the container to enclose more
/// area makes the relation a tree, so every chain ends at the ground.
fn sits_on(t: &Trace, length: &[f64], n: usize) -> Vec<Option<usize>> {
    let nest = crate::rings::nesting(t.order, t.fitted);
    let enclosed: Vec<f64> = (0..n)
        .map(|f| {
            nest.outer
                .get(f)
                .map(|o| o.iter().map(|&k| nest.info[f][k].area).sum())
                .unwrap_or(0.0)
        })
        .collect();
    let larger = |q: usize, f: usize| (enclosed[q], q) > (enclosed[f], f);
    (0..n)
        .map(|f| {
            let mut shared: Vec<(usize, f64)> = Vec::new();
            let mut outside = 0.0;
            for &k in nest.outer.get(f).into_iter().flatten() {
                for &(e, rev) in &t.order[f][k] {
                    let edge = &t.map.edges[e];
                    let other = if rev { edge.left } else { edge.right } as usize;
                    let len = length.get(e).copied().unwrap_or(0.0);
                    if other >= n {
                        outside += len;
                    } else if other != f && larger(other, f) {
                        match shared.iter_mut().find(|s| s.0 == other) {
                            Some(s) => s.1 += len,
                            None => shared.push((other, len)),
                        }
                    }
                }
            }
            match shared.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
                Some(&(q, len)) if len >= outside => Some(q),
                Some(_) => None,
                // A face with no outline of its own to walk: the one containing it.
                None if outside == 0.0 => nest.parent.get(f).copied().flatten(),
                None => None,
            }
        })
        .collect()
}

/// The tone of every face of a finished colour trace.
///
/// Gathers the [`Evidence`] [`decide`] needs: each face's colours, whether it is see-through
/// (clear, or under [`INK_OPACITY`]), the image-border pixels it covers (from `labels`), its
/// width `2·area/perimeter` in px (area in pixels, perimeter the summed length of its
/// edges' measured polylines), its neighbours across each edge, and the face it sits on
/// ([`sits_on`]).
pub(crate) fn classify(t: &Trace) -> Ground {
    let (w, h) = (t.map.width, t.map.height);
    let n = t.face_color.len().max(t.fills.len()).max(t.order.len());
    let mut ev = Evidence {
        stops: (0..n)
            .map(|f| face_stops(f, t.fills, t.face_color, t.pal))
            .collect(),
        see_through: (0..n)
            .map(|f| {
                t.clear.get(f).copied().unwrap_or(false)
                    || t.opacity.get(f).copied().unwrap_or(1.0) < INK_OPACITY
            })
            .collect(),
        border: vec![0; n],
        width: vec![f64::INFINITY; n],
        parent: vec![None; n],
        neighbours: vec![Vec::new(); n],
    };

    let mut area = vec![0usize; n];
    for &l in t.labels {
        if let Some(a) = area.get_mut(l as usize) {
            *a += 1;
        }
    }
    let mut count = |x: usize, y: usize| {
        if let Some(b) = t
            .labels
            .get(y * w + x)
            .and_then(|&f| ev.border.get_mut(f as usize))
        {
            *b += 1;
        }
    };
    if w > 0 && h > 0 && t.labels.len() == w * h {
        for x in 0..w {
            count(x, 0);
            count(x, h - 1);
        }
        for y in 0..h {
            count(0, y);
            count(w - 1, y);
        }
    }

    let length: Vec<f64> = t
        .map
        .edges
        .iter()
        .map(|e| {
            e.points
                .windows(2)
                .map(|p| ((p[1].x - p[0].x).powi(2) + (p[1].y - p[0].y).powi(2)).sqrt())
                .sum::<f64>()
                + if e.closed && e.points.len() > 2 {
                    let (a, b) = (e.points[0], e.points[e.points.len() - 1]);
                    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
                } else {
                    0.0
                }
        })
        .collect();
    let mut perimeter = vec![0.0f64; n];
    for (e, &len) in t.map.edges.iter().zip(&length) {
        let (l, r) = (e.left as usize, e.right as usize);
        for (f, g) in [(l, r), (r, l)] {
            if f < n {
                perimeter[f] += len;
                if g < n && g != f && !ev.neighbours[f].contains(&g) {
                    ev.neighbours[f].push(g);
                }
            }
        }
    }
    for f in 0..n {
        if perimeter[f] > 0.0 {
            ev.width[f] = 2.0 * area[f] as f64 / perimeter[f];
        }
    }
    ev.parent = sits_on(t, &length, n);

    decide(&ev)
}

/// The document: the union of the ink faces as one black even-odd path, on a white canvas
/// rectangle unless `no_background` asks for the ground to be left transparent.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit(
    map: &PlanarMap,
    ink: &[bool],
    fitted: &[FittedPath],
    prims: &[Option<PrimitiveFit>],
    no_background: bool,
    w: usize,
    h: usize,
    precision: f64,
) -> String {
    let decimals = emit_decimals(precision);
    // Two faces: 0 the ground, 1 the ink. An edge with the same tone on both sides is
    // interior and belongs to no ring; the canvas outside (`u16::MAX`) stays outside.
    const GROUND: u16 = 0;
    const INK: u16 = 1;
    let tone = |l: u16| -> u16 {
        match ink.get(l as usize) {
            Some(true) => INK,
            Some(false) => GROUND,
            None => l,
        }
    };
    let mut merged = PlanarMap {
        edges: Vec::with_capacity(map.edges.len()),
        width: map.width,
        height: map.height,
        n_labels: 2,
    };
    for e in &map.edges {
        let (l, r) = (tone(e.left), tone(e.right));
        let (l, r) = if l == r { (u16::MAX, u16::MAX) } else { (l, r) };
        // The ring walk reads only the labels, the nodes and `closed`; the points stay behind.
        merged.edges.push(planar::Edge {
            points: Vec::new(),
            sigma: Vec::new(),
            left: l,
            right: r,
            start_node: e.start_node,
            end_node: e.end_node,
            closed: e.closed,
            lambda_scale: e.lambda_scale,
        });
    }
    let rings = planar::face_edge_order(&merged)
        .into_iter()
        .nth(INK as usize)
        .unwrap_or_default();

    let mut d = String::new();
    for ring in &rings {
        let pts: Vec<Point> = ring_points(ring, fitted);
        if pts.len() < 3 || ring_area(&pts) <= MIN_RING_AREA {
            continue;
        }
        // A boundary that is one closed primitive is written as that primitive, exactly as
        // the colour emitter writes it.
        let prim = (ring.len() == 1)
            .then(|| prims.get(ring[0].0).and_then(|p| p.as_ref()))
            .flatten()
            .and_then(|pf| primitive_d(&pf.kind, decimals));
        match prim {
            Some(p) => d.push_str(&p),
            None => fmt_ring(ring, fitted, decimals, &mut d),
        }
    }

    let mut body = String::new();
    if !no_background {
        body.push_str(&format!(
            "<rect id=\"white-1\" x=\"-0.5\" y=\"-0.5\" width=\"{w}\" height=\"{h}\" fill=\"#ffffff\"/>"
        ));
    }
    if !d.is_empty() {
        // Every ring of the ink, outlines, holes and islands in holes alike, wound by its
        // nesting depth so the default fill rule paints what even-odd did.
        let (d, rule) = crate::emit::for_nonzero(&d);
        body.push_str(&format!(
            "<path id=\"black-1\" d=\"{d}\" fill=\"#000000\"{rule}/>"
        ));
    }
    // The header every emitter writes, so retargeting and the margin find it.
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 {w} {h}\" width=\"{w}\" height=\"{h}\">{body}</svg>"
    )
}

/// The report line.
pub(crate) fn report(g: &Ground) -> String {
    let black = g.ink.iter().filter(|&&i| i).count();
    let ground = match g.colour {
        Some(c) => format!("ground {}", inkvec_trace::color::to_hex(c)),
        None => "ground transparent".to_string(),
    };
    format!(
        "monochrome    {ground}; {black} of {} faces painted black",
        g.ink.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
    const BLACK: [f32; 3] = [0.0, 0.0, 0.0];
    const YELLOW: [f32; 3] = [1.0, 0.85, 0.0];

    /// Faces given as (colour, parent, border pixels, width), neighbours from the parents.
    fn evidence(faces: &[([f32; 3], Option<usize>, usize, f64)]) -> Evidence {
        let n = faces.len();
        let mut neighbours = vec![Vec::new(); n];
        for (f, &(_, p, _, _)) in faces.iter().enumerate() {
            if let Some(p) = p {
                neighbours[f].push(p);
                neighbours[p].push(f);
            }
        }
        Evidence {
            stops: faces.iter().map(|f| vec![f.0]).collect(),
            see_through: vec![false; n],
            border: faces.iter().map(|f| f.2).collect(),
            width: faces.iter().map(|f| f.3).collect(),
            parent: faces.iter().map(|f| f.1).collect(),
            neighbours,
        }
    }

    #[test]
    fn a_letter_is_black_and_its_counter_a_hole() {
        // White ground, a black O, the white counter inside it, and a near-white patch.
        let g = decide(&evidence(&[
            (WHITE, None, 100, 50.0),
            (BLACK, Some(0), 0, 6.0),
            ([0.97, 0.97, 0.97], Some(1), 0, 8.0),
        ]));
        assert_eq!(g.ink, [false, true, false]);
        assert_eq!(g.colour, Some(WHITE));
    }

    #[test]
    fn a_light_ink_on_white_is_still_ink() {
        let g = decide(&evidence(&[
            (WHITE, None, 100, 50.0),
            (YELLOW, Some(0), 0, 9.0),
        ]));
        assert_eq!(g.ink, [false, true]);
    }

    #[test]
    fn lettering_on_a_coloured_panel_is_knocked_out_of_it() {
        // White ground, a yellow panel, black lettering on it, a counter inside a letter.
        let g = decide(&evidence(&[
            (WHITE, None, 100, 50.0),
            (YELLOW, Some(0), 0, 40.0),
            (BLACK, Some(1), 0, 5.0),
            (YELLOW, Some(2), 0, 3.0),
        ]));
        assert_eq!(g.ink, [false, true, false, true]);
    }

    #[test]
    fn white_on_black_paints_the_white_black() {
        let g = decide(&evidence(&[
            (BLACK, None, 100, 50.0),
            (WHITE, Some(0), 0, 6.0),
            (BLACK, Some(1), 0, 4.0),
        ]));
        assert_eq!(g.ink, [false, true, false]);
        assert_eq!(g.colour, Some(BLACK));
    }

    #[test]
    fn a_coloured_ground_is_the_ground() {
        // Black lettering straight on a red poster.
        let red = [0.9, 0.1, 0.1];
        let g = decide(&evidence(&[
            (red, None, 100, 50.0),
            (BLACK, Some(0), 0, 5.0),
        ]));
        assert_eq!(g.ink, [false, true]);
    }

    #[test]
    fn on_a_transparent_ground_everything_opaque_is_ink() {
        // White lettering (1) on a clear ground (0), a clear counter (2), a faint wash (3).
        let mut ev = evidence(&[
            (WHITE, None, 100, 50.0),
            (WHITE, Some(0), 0, 6.0),
            (WHITE, Some(1), 0, 4.0),
            (WHITE, Some(0), 0, 4.0),
        ]);
        ev.see_through = vec![true, false, true, true];
        let g = decide(&ev);
        assert_eq!(g.ink, [false, true, false, false]);
        assert_eq!(g.colour, None);
    }

    #[test]
    fn an_antialiased_rim_neither_cracks_nor_fattens_the_black() {
        // A black O (1) with a grey hairline rim (2) inside it around the white counter (3),
        // and a grey rim (4) outside it on the white ground.
        let grey = [0.5, 0.5, 0.5];
        let mut ev = evidence(&[
            (WHITE, None, 100, 50.0),
            (BLACK, Some(0), 0, 6.0),
            (grey, Some(1), 0, 0.8),
            (WHITE, Some(2), 0, 8.0),
            (grey, Some(0), 0, 0.8),
        ]);
        ev.neighbours[4].push(1);
        ev.neighbours[1].push(4);
        ev.neighbours[2].push(3);
        let g = decide(&ev);
        assert!(
            !g.ink[3],
            "the counter is judged against the O, past its rim"
        );
        assert!(g.ink[1]);
    }

    #[test]
    fn a_hairline_that_is_its_own_colour_is_a_line() {
        // A one-pixel black rule on white is drawn, not absorbed into the ground.
        let g = decide(&evidence(&[
            (WHITE, None, 100, 50.0),
            (BLACK, Some(0), 0, 1.0),
        ]));
        assert_eq!(g.ink, [false, true]);
    }

    #[test]
    fn a_gradient_ground_takes_in_every_colour_along_it() {
        let mut ev = evidence(&[
            (WHITE, None, 100, 50.0),
            (BLACK, Some(0), 0, 6.0),
            ([0.8, 0.9, 1.0], Some(0), 0, 6.0),
        ]);
        ev.stops[0] = vec![WHITE, [0.6, 0.8, 1.0]];
        let g = decide(&ev);
        assert_eq!(g.ink, [false, true, false]);
    }
}
