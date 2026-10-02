//! Writing the document.
//!
//! Everything here turns fitted geometry into SVG text, and nothing here decides anything
//! about the image — by the time these run, the curves, the fills and the transparency of
//! every face are settled. The colour pipeline ([`crate::pipeline`]) calls [`emit_color`]
//! once per candidate document (flat, and with translucent layers when there are any);
//! the bilevel pipeline calls [`emit_bilevel`]. What comes in is the planar map's faces as
//! rings of shared, fitted edges ([`crate::faces`]); what goes out is a complete `<svg>`
//! in the traced image's pixel coordinates (pixel centres at integers, so the viewBox
//! starts at -0.5), which [`crate::post`] then retargets, margins and minifies.
//!
//! [`emit_color`] runs in stages, each a function below with its own reasons:
//!
//! 1. **Nesting** -- which rings each face paints and which face each sits in
//!    ([`crate::rings::nesting`]).
//! 2. **Stacking** -- which faces are not painted at all (the knocked-out background,
//!    transparent faces) and which faces are punched out of which ([`stack_faces`]).
//! 3. **Harmonizing** -- repeated shapes redrawn from one consensus ([`crate::harmonize`]).
//! 4. **Paint** -- each face's fill, gradient definitions and opacity ([`face_paint`]).
//! 5. **Paint order and names** -- the group tree ([`paint_tree`]) and element ids
//!    ([`face_ids`]).
//! 6. **Strokes** -- rings of one width written as one stroked primitive
//!    ([`annulus_strokes`]).
//! 7. **Writing** -- the element tree, with same-coloured siblings merged into one
//!    compound path ([`Writer`]).
//! 8. **Seams** -- side-by-side faces reach under each other and the tree is written again
//!    ([`seam_overrides`], [`crate::seams`]).
//! 9. **Layers** -- recovered translucent layers painted over everything ([`write_layers`]).
//!
//! Two things are worth knowing before reading:
//!
//! * **Coordinates are written to [`crate::pathdata::EMIT_DECIMALS`], not to
//!   `--precision`.** The geometry arrives good to hundredths of a pixel and rounding it to
//!   tenths threw away 7% of the fidelity on the full corpus, which is more than most of the
//!   fitter earns. The digit count is deliberately the emitter's own and not the fitter's:
//!   pricing coordinates more finely while still rounding coarsely made the corpus *worse*,
//!   so `--precision` sets lambda and `INKVEC_EMIT_DECIMALS` sets the digits. This header
//!   said `--precision` until 2026-09-08.
//! * **Same-coloured siblings become one even-odd path.** An artist draws a letter and its
//!   counter as one path with a hole; emitting them as two costs a shape and leaves a seam
//!   along the shared edge. `fill-rule="evenodd"` says the same thing in fewer marks, and
//!   the parity rules in [`crate::rings`] are what make it correct.

use std::collections::{HashMap, HashSet};

use inkvec_core::Point;
use inkvec_fit::{
    primitives::{PrimitiveFit, PrimitiveKind},
    FittedPath,
};
use inkvec_trace::gradient;

use crate::alpha::{unmatte, AlphaRamp};
use crate::faces::{FaceRings, Layers, Ring};
use crate::harmonize;
use crate::naming::colour_name;
use crate::pathdata::{emit_decimals, fmt_path, fmt_ring, fmt_ring_with};
use crate::primitive::{annulus_stroke, primitive_d, primitive_element, stroke_element};
use crate::rings::{self, point_in_ring, ring_area, Nesting};
use crate::seams;

/// Everything the colour emitter writes from: the settled geometry, fills and transparency
/// of every face. Per-face slices are indexed by face (the planar map's label), per-edge
/// slices by edge; either may be shorter than the face count, and a missing entry reads as
/// "nothing special" (opaque, not clear, no ramp).
#[derive(Clone, Copy)]
pub(crate) struct ColorDoc<'a> {
    /// The rings of each face, as walks over shared edges.
    pub order: &'a [FaceRings],
    /// The fitted curve of each edge.
    pub fitted: &'a [FittedPath],
    /// Per edge, the primitive (circle, ellipse, rounded rectangle) the whole edge fitted,
    /// when it did.
    pub prims: &'a [Option<PrimitiveFit>],
    /// Per face, its fill model: flat, linear or radial gradient.
    pub fill_fits: &'a [gradient::FillFit],
    /// The palette the faces' inks index into.
    pub pal: &'a inkvec_trace::Palette,
    /// Per face, its palette index.
    pub face_color: &'a [usize],
    /// Per face, transparent in the source: a hole, not a colour.
    pub clear: &'a [bool],
    /// What the source drew each face at, 1.0 for opaque. Below it, the face carries
    /// `fill-opacity` and its colour is un-matted.
    pub opacity: &'a [f32],
    /// Faces whose opacity fades across them, as a gradient of `stop-opacity`.
    pub alpha_ramps: &'a [Option<AlphaRamp>],
    /// Faces traced natively as fades: an opacity profile (stops are greys equal to the
    /// opacity) and the colour profile on the same geometry and stops.
    pub fades: &'a [Option<(gradient::FillModel, gradient::FillModel)>],
    /// Translucent layers to paint over the faces once those carry the ground's colour,
    /// with one ring set per layer: its own outline, taken before the merge that removed
    /// its faces from the map. A face can lie under two layers, so these cannot be indexed
    /// by face.
    pub layers: Option<Layers<'a>>,
    /// The colour transparent pixels were composited onto before tracing, sRGB 0..1.
    pub matte: [f32; 3],
    /// Width of the traced raster, px.
    pub w: usize,
    /// Height of the traced raster, px.
    pub h: usize,
}

/// How the colour document is written: the output flags that change it.
#[derive(Clone, Copy)]
pub(crate) struct EmitOptions {
    /// `--cutout`: punch transparent and translucent faces out of the faces above them.
    pub cutout: bool,
    /// Transparency was traced natively: a clear face is the ground, never paint.
    pub native: bool,
    /// `--no-background`: the face covering the canvas is not painted.
    pub no_background: bool,
    /// `--precision`, px; see [`emit_decimals`] for why it does not set the digits.
    pub precision: f64,
    /// Redraw repeated shapes from one consensus ([`crate::harmonize`]).
    pub harmonize: bool,
    /// The clustering threshold harmonizing uses.
    pub harmonize_threshold: f64,
    /// Write harmonized shapes as `<use>` of one shared definition.
    pub use_symbols: bool,
}

/// Emit the map as a **stacked** document: faces painted back to front, each drawing only
/// its outermost rings.
///
/// SVG has no way to share one curve between two fills, so a boundary between two faces
/// must appear twice in the file even though the planar map stores it once. Stacking
/// recovers most of that cost wherever faces nest: a face painted on top *supplies* the
/// hole in the face beneath, so the lower face never draws that hole at all. Concentric
/// rings become discs rather than annuli, and a shape on a background reduces to a
/// canvas-sized rectangle plus the shape.
///
/// The faces still tile exactly, so this changes the file and not the rendering — and
/// because the curve an upper face paints is bit-identical to the one the lower face
/// would have drawn, no seam can appear between them. The exceptions are transparency,
/// where a stacked hole would show the face beneath instead of the page (see
/// [`stack_faces`]), and the anti-aliasing seam between side-by-side faces (see
/// [`seam_overrides`]).
///
/// Returns the whole `<svg>` document, viewBox `-0.5 -0.5 w h`. The stages are listed in
/// the module documentation.
pub(crate) fn emit_color(doc: &ColorDoc, opts: &EmitOptions) -> String {
    let decimals = emit_decimals(opts.precision);

    // Painted rings per face, which contain which, and the faces they nest in: see
    // `crate::rings::nesting`.
    let nest = rings::nesting(doc.order, doc.fitted);
    // Drawing the holes too — making each path self-contained so any face could be
    // dropped freely — was tried and reverted. It fixes interior transparency and costs
    // more than it buys: parameters rose (the arrow 166 to 248) and DISTS regressed on
    // three of six probes (the arrow 0.0054 to 0.0273), because a ring contained inside
    // another of the same face is not reliably a hole, and punching it makes one anyway.
    // So a face draws its outline (`nest.outer`) and nothing else of its own.
    let drawn = &nest.outer;

    let stack = stack_faces(doc, opts, &nest);
    debug_dump(doc, &nest, &stack);

    // Repeated shapes redrawn from one consensus, where their own evidence agrees; see
    // `crate::harmonize`. Its symbols come first among the definitions.
    let mut defs = String::new();
    let harmonized = if opts.harmonize {
        harmonize::harmonize(
            &harmonize::Faces {
                order: doc.order,
                fitted: doc.fitted,
                prims: doc.prims,
                pts: &nest.pts,
                drawn,
                holes: &stack.holes,
                dropped: &stack.dropped,
            },
            opts.harmonize_threshold,
            opts.use_symbols,
            decimals,
        )
    } else {
        harmonize::Harmonized::default()
    };
    defs.push_str(&harmonized.defs);

    let paint = face_paint(doc, &stack, &mut defs);
    let (roots, children) = paint_tree(doc, &nest, &stack, paint.fills.len());
    let ids = face_ids(doc);
    let strokes = if opts.native {
        annulus_strokes(doc, drawn, &stack, &paint, &harmonized, decimals)
    } else {
        HashMap::new()
    };

    let writer = Writer {
        order: doc.order,
        fitted: doc.fitted,
        prims: doc.prims,
        fills: &paint.fills,
        opac: &paint.opac,
        ids: &ids,
        children: &children,
        drawn,
        holes: &stack.holes,
        decimals,
        harmonized_d: &harmonized.d,
        symbol_use: &harmonized.symbols,
        strokes: &strokes,
    };
    let mut painted = Vec::new();
    let mut body = writer.body(&roots, &seams::Overrides::new(), &mut painted);

    // Side-by-side faces: the lower reaches under the upper so no ground shows through
    // their shared edge (see `crate::seams`). The first pass settled the paint order; the
    // second writes the moved outlines, and is skipped when nothing moves.
    if let Some(moved) = seam_overrides(doc, &nest, &stack, &paint, &writer, &painted) {
        body = writer.body(&roots, &moved, &mut Vec::new());
    }

    write_layers(doc, decimals, &mut body);

    let defs_block = if defs.is_empty() {
        String::new()
    } else {
        format!("<defs>{defs}</defs>")
    };
    let (w, h) = (doc.w, doc.h);
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 {w} {h}\" width=\"{w}\" height=\"{h}\">{defs_block}{body}</svg>"
    )
}

/// Which faces are painted, and which are cut out of which: the decisions
/// [`stack_faces`] makes.
struct Stacking {
    /// Per face: not painted at all.
    dropped: Vec<bool>,
    /// Per face: the faces punched out of it, written as extra rings of its even-odd path.
    holes: Vec<Vec<usize>>,
    /// Per face: the opacity it is written at. The source's opacity where the face can be
    /// written translucent honestly, 1.0 where its translucency is baked against the matte.
    thin: Vec<f32>,
}

/// Stage 2: decide which faces are painted and what each has punched out of it.
///
/// A transparent face is a hole, not a colour.
///
/// This emitter is *stacked*: a face's hole is drawn by painting the face inside it on
/// top, so withholding paint from a transparent face shows whatever lies under it.
/// That is why only the outermost transparent faces used to be dropped, and why every
/// interior one — the counter of a letter O, the wedge inside a rounded badge — came
/// out painted in the matte colour. Innocuous while the matte was always white and on
/// a white ground; wrong everywhere else, and glaring now that the matte is chosen for
/// contrast rather than fixed.
///
/// So the face is dropped and its outline is punched, under `evenodd`, out of the faces
/// that would otherwise paint over it: every painted ancestor up to the first ancestor
/// that is itself transparent. Stopping there is what parity requires and not a
/// shortcut — the transparent ancestor is punched out of the faces above in its own
/// right, and its ring already carries this one inside it. Punching the whole chain
/// instead re-paints the inner region, because a second ring inside an even-odd hole
/// flips it back: the counter of a heart inside a page inside a book cover came out
/// solid black that way, dE00 0.19 -> 3.33 on `lucide/book-heart`.
///
/// In order: the canvas background under `--no-background` ([`canvas_background`]) and
/// the faces of its colour that are holes in a glyph ([`punch_background_coloured`]);
/// clear and translucent faces ([`punch_transparent`]); then the hole lists are made
/// parity-safe ([`dedupe_holes`]).
fn stack_faces(doc: &ColorDoc, opts: &EmitOptions, nest: &Nesting) -> Stacking {
    let n = doc.order.len();
    let mut stack = Stacking {
        dropped: vec![false; n],
        holes: vec![Vec::new(); n],
        thin: doc.opacity.to_vec(),
    };
    let canvas_bg = if opts.no_background {
        canvas_background(doc, nest)
    } else {
        None
    };

    // Faces the colour of the canvas are background showing through -- a guess, and the
    // right one for an opaque file. Where the source was transparent under the canvas face
    // its colour is only the matte, and whether a face of that colour is paint is not a
    // guess: `clear` says so face by face, and the pass below punches the clear ones. Taking
    // every face that merely matches the matte deleted white paint: a white ring inside a
    // copper disc on a transparent PNG came out as a hole in the disc.
    if let Some(bg) = canvas_bg {
        stack.dropped[bg] = true;
        if !is_clear(doc, bg) {
            punch_background_coloured(doc, nest, bg, &mut stack);
        }
    }
    punch_transparent(doc, opts, nest, &mut stack);
    dedupe_holes(&nest.parent, &mut stack.holes);
    stack
}

/// Whether face `i` is transparent in the source; a face past the end of `doc.clear` is
/// not.
fn is_clear(doc: &ColorDoc, i: usize) -> bool {
    doc.clear.get(i).copied().unwrap_or(false)
}

/// Whether face `c` lies inside face `p` for certain: some outline ring of `c` has every
/// one of its interior probes inside some outline ring of `p`.
///
/// Containment here has to be certain, not likely. `ring_inside` settles the paint
/// order on a majority of probes, which is right for stacking and wrong for cutting: a
/// wedge that merely runs alongside a shape can win that vote, and punching its ring
/// into a face it is not inside paints the wedge in that face's colour instead of
/// removing it — a black wedge beside the head on `noto-emoji/emoji_u1f3cb_200d_2642`,
/// dE00 0.58 -> 1.78. So a punch needs every probe inside, and a transparent face that
/// cannot prove where it belongs is painted as it always was rather than cut by guess.
fn strictly_inside(nest: &Nesting, c: usize, p: usize) -> bool {
    let (outer, info, pts) = (&nest.outer, &nest.info, &nest.pts);
    outer[c].iter().any(|&kc| {
        let probes = &info[c][kc].probes;
        !probes.is_empty()
            && outer[p].iter().any(|&kp| {
                pts[p][kp].len() >= 3 && probes.iter().all(|&q| point_in_ring(q, &pts[p][kp]))
            })
    })
}

/// The chain of faces that would paint under face `c`: its ancestors, nearest first, up
/// to but not including the first one `stop` accepts.
///
/// Returns the chain and whether `c` is [`strictly_inside`] every face in it. The walk
/// ends early, with `false`, at the first ancestor `c` cannot be proved inside; the chain
/// then holds only the ancestors before it.
fn painted_ancestors(nest: &Nesting, c: usize, stop: impl Fn(usize) -> bool) -> (Vec<usize>, bool) {
    let mut chain = Vec::new();
    let mut k = c;
    let mut sure = true;
    while let Some(p) = nest.parent[k] {
        if stop(p) {
            break;
        }
        if !strictly_inside(nest, c, p) {
            sure = false;
            break;
        }
        chain.push(p);
        k = p;
    }
    (chain, sure)
}

/// A face's colour when it is painted flat: its flat fill model, else its palette ink.
/// `None` for a gradient face with no palette entry to fall back on.
fn flat_colour(doc: &ColorDoc, i: usize) -> Option<[f32; 3]> {
    if let Some(f) = doc.fill_fits.get(i) {
        if let gradient::FillModel::Flat(c) = f.model {
            return Some(c);
        }
    }
    doc.face_color
        .get(i)
        .and_then(|&ci| doc.pal.rgb.get(ci))
        .copied()
}

/// Two sRGB colours within 0.02 of each other on every channel: the same colour for the
/// purpose of knocking out a background.
fn same_rgb(a: [f32; 3], b: [f32; 3]) -> bool {
    (a[0] - b[0]).abs() < 0.02 && (a[1] - b[1]).abs() < 0.02 && (a[2] - b[2]).abs() < 0.02
}

/// The face that is the canvas background under `--no-background`, if one is.
///
/// A top-level face (no parent) qualifies when it covers the whole canvas `-0.5..w-0.5` by
/// `-0.5..h-0.5`, which is recognised two ways: its outline is one rounded-rectangle
/// primitive whose edges are within 0.25 px of the canvas edges, or one of its outline
/// rings passes within 0.5 px (on each axis) of all four canvas corners. The first such
/// face in label order wins.
fn canvas_background(doc: &ColorDoc, nest: &Nesting) -> Option<usize> {
    let (order, outer, pts) = (doc.order, &nest.outer, &nest.pts);
    let drawn = outer;
    let (x0, y0) = (-0.5, -0.5);
    let (x1, y1) = (doc.w as f64 - 0.5, doc.h as f64 - 0.5);
    (0..order.len()).find(|&i| {
        if nest.parent[i].is_some() {
            return false;
        }
        if let Some(pf) = (drawn[i].len() == 1 && order[i][drawn[i][0]].len() == 1)
            .then(|| {
                doc.prims
                    .get(order[i][outer[i][0]][0].0)
                    .and_then(|p| p.as_ref())
            })
            .flatten()
        {
            if let PrimitiveKind::RoundRect {
                x, y, w: rw, h: rh, ..
            } = pf.kind
            {
                if (x + 0.5).abs() <= 0.25
                    && (y + 0.5).abs() <= 0.25
                    && (x + rw - x1).abs() <= 0.25
                    && (y + rh - y1).abs() <= 0.25
                {
                    return true;
                }
            }
        }
        outer[i].iter().any(|&k| {
            let ring = &pts[i][k];
            let has_corner = |cx: f64, cy: f64| {
                ring.iter()
                    .any(|p| (p.x - cx).abs() <= 0.5 && (p.y - cy).abs() <= 0.5)
            };
            has_corner(x0, y0) && has_corner(x1, y0) && has_corner(x1, y1) && has_corner(x0, y1)
        })
    })
}

/// With the canvas face `bg` knocked out, the faces of its colour that are holes in a
/// glyph are knocked out too: dropped, and punched out of the glyph.
///
/// A face `c` of the background's colour (within [`same_rgb`]) qualifies only when it sits
/// directly in one painted face `p` (the chain up to the background or a clear face is
/// exactly one face long, and `c` is certainly inside it), every child of `p` is itself of
/// the background's colour, and one of `c`'s outline rings shares an edge with one of `p`'s
/// non-outline rings -- that is, `c` fills a hole of `p`.
fn punch_background_coloured(doc: &ColorDoc, nest: &Nesting, bg: usize, stack: &mut Stacking) {
    let (order, outer, parent) = (doc.order, &nest.outer, &nest.parent);
    let Some(bg_c) = flat_colour(doc, bg) else {
        return;
    };
    for c in 0..order.len() {
        if c == bg || outer[c].is_empty() {
            continue;
        }
        let Some(cc) = flat_colour(doc, c) else {
            continue;
        };
        if !same_rgb(cc, bg_c) {
            continue;
        }
        let (chain, sure) = painted_ancestors(nest, c, |p| p == bg || is_clear(doc, p));
        if !(sure && chain.len() == 1) {
            continue;
        }
        let p = chain[0];
        // Only punch out holes from a parent `p` if `p` is a glyph / simple container
        // whose children are all background cutouts, NOT a complex illustration figure
        // containing various colored foreground elements (clothes, eyes, teeth, tools).
        let p_has_non_bg_children = (0..order.len()).any(|other| {
            parent[other] == Some(p)
                && other != p
                && flat_colour(doc, other).is_none_or(|oc| !same_rgb(oc, bg_c))
        });
        if p_has_non_bg_children {
            continue;
        }

        let is_hole = outer[c].iter().any(|&kc| {
            let c_ring = &order[c][kc];
            order[p].iter().enumerate().any(|(kp, pr)| {
                !outer[p].contains(&kp)
                    && pr
                        .iter()
                        .any(|&(pe, _)| c_ring.iter().any(|&(ce, _)| ce == pe))
            })
        });
        if is_hole {
            stack.dropped[c] = true;
            if !stack.holes[p].contains(&c) {
                stack.holes[p].push(c);
            }
        }
    }
}

/// Clear and translucent faces: dropped and punched out of what paints under them, or
/// baked against the matte where that cannot be done honestly.
///
/// A translucent face is written as one only if nothing of ours paints under it. In a
/// stacked document its parent is painted across the whole area first, so `fill-opacity`
/// would blend with the parent instead of the page — the film frames on
/// `noto-emoji/emoji_u1f39e` came out over the strip they sit in, dE00 0.46 -> 1.43. It
/// is honest when the face already sits on the page, and when the cutout punches it out
/// of the faces below; otherwise the face keeps the colour it was measured at, baked
/// against the matte, exactly as before.
///
/// "The faces that would paint under it" are [`painted_ancestors`] up to the first clear
/// one. A clear face is punched out of all of them when the cutout (or the knock-out) is
/// on and it is certainly inside them; without either, only a clear face with nothing
/// painted under it is dropped, as it always was. Traced natively, a clear face is the
/// ground itself and is always dropped.
fn punch_transparent(doc: &ColorDoc, opts: &EmitOptions, nest: &Nesting, stack: &mut Stacking) {
    let no_punch = !opts.cutout && !opts.no_background;
    for c in 0..doc.order.len() {
        if stack.dropped[c] {
            continue;
        }
        let clear = is_clear(doc, c);
        let thin = stack.thin.get(c).copied().unwrap_or(1.0) < 1.0;
        if (!clear && !thin) || nest.outer[c].is_empty() {
            continue;
        }
        // The faces that would paint under this one: up the chain to the first ancestor
        // that is itself transparent.
        let (chain, sure) = painted_ancestors(nest, c, |p| is_clear(doc, p));
        if clear {
            if !sure {
                // Traced natively, a clear face is the ground itself, and painting it would
                // put a matte colour where the source had nothing. Drop it and punch it out
                // of the ancestors that do contain it; without native alpha the old rule
                // stands and it is left as it was.
                if opts.native {
                    stack.dropped[c] = true;
                    for p in chain {
                        stack.holes[p].push(c);
                    }
                }
                continue;
            }
            // Without the cutout the old rule stands: drop only what has nothing painted
            // under it, and let the rest paint the matte as it always did.
            if no_punch {
                stack.dropped[c] = chain.is_empty();
                continue;
            }
            stack.dropped[c] = true;
            for p in chain {
                stack.holes[p].push(c);
            }
        } else if no_punch {
            // Translucency is part of what the cutout promises: writing it as
            // `fill-opacity` needs the faces underneath removed, and without the flag they
            // are not. Bake it against the matte, as before.
            stack.thin[c] = 1.0;
        } else if chain.is_empty() {
            continue; // already on the page
        } else if sure {
            for p in chain {
                stack.holes[p].push(c);
            }
        } else {
            stack.thin[c] = 1.0;
        }
    }
}

/// Make every face's hole list correct under even-odd.
///
/// A face is written with its holes under even-odd, and even-odd counts crossings: a
/// hole inside another hole of the same face fills back in, and the same hole twice
/// cancels. Every clear or translucent face is punched out of *all* its ancestors, so a
/// stack of them -- the bands of a fade -- gave the outermost band every inner outline
/// as a hole and painted alternate bands twice over. A hole already inside another of
/// the face's holes is removed with it and needs no hole of its own. So each list keeps
/// the first copy of each hole, and drops a hole with an ancestor (below the face itself)
/// that is also in the list.
fn dedupe_holes(parent: &[Option<usize>], holes: &mut [Vec<usize>]) {
    for p in 0..holes.len() {
        if holes[p].len() < 2 {
            continue;
        }
        let set: HashSet<usize> = holes[p].iter().copied().collect();
        let mut seen = HashSet::new();
        holes[p].retain(|&h| {
            if !seen.insert(h) {
                return false;
            }
            let mut k = h;
            while let Some(q) = parent[k] {
                if q == p {
                    break;
                }
                if set.contains(&q) {
                    return false;
                }
                k = q;
            }
            true
        });
    }
}

/// The nearest ancestor of face `i` that is painted.
///
/// A dropped face must not take its children with it. Walk up past any dropped
/// ancestor so a shape sitting inside a transparent region is still painted, at the
/// level of the nearest ancestor that survives.
fn surviving_parent(parent: &[Option<usize>], dropped: &[bool], mut i: usize) -> Option<usize> {
    while let Some(p) = parent[i] {
        if !dropped[p] {
            return Some(p);
        }
        i = p;
    }
    None
}

/// `INKVEC_ALPHADBG`: one line per face with its rings, areas, parent and stacking
/// decisions, and the edges of any face left with no outline.
fn debug_dump(doc: &ColorDoc, nest: &Nesting, stack: &Stacking) {
    let on = inkvec_core::env::flag("INKVEC_ALPHADBG");
    if !on {
        return;
    }
    let (order, outer) = (doc.order, &nest.outer);
    for i in 0..order.len() {
        crate::diag::debug(on, || {
            format!(
                "  emit {i}: rings {} areas {:?} outer {} clear {:?} parent {:?} drop {} holes {:?}",
                order.get(i).map_or(0, |o| o.len()),
                nest.pts[i]
                    .iter()
                    .map(|r| (r.len(), ring_area(r).round()))
                    .collect::<Vec<_>>(),
                outer.get(i).map_or(0, |o| o.len()),
                doc.clear.get(i),
                nest.parent.get(i).copied().flatten(),
                stack.dropped[i],
                stack.holes.get(i)
            )
        });
        if outer.get(i).is_some_and(|o| o.is_empty()) {
            for r in &order[i] {
                for &(k, rev) in r {
                    crate::diag::debug(on, || {
                        format!(
                            "      edge {k} rev {rev} start {:?} segs {:?}",
                            doc.fitted[k].start, doc.fitted[k].segments
                        )
                    });
                }
            }
        }
    }
}

/// What each face is painted with: the attributes [`face_paint`] settles.
struct Paint {
    /// Per face, the `fill` value: `#rrggbb`, or `url(#…)` for a gradient.
    fills: Vec<String>,
    /// Per face, the attribute written after the fill: ` fill-opacity="…"`, or empty for
    /// an opaque face -- every face of an image that had no alpha channel to begin with.
    opac: Vec<String>,
    /// Per face, the colour of the ground under a recovered layer, for a face that lies
    /// under one; such a face is painted with this and the layer composited on top.
    base_of: Vec<Option<[f32; 3]>>,
}

/// Stage 4: every face's fill and opacity attributes, appending the gradient definitions
/// they refer to to `defs` in face order.
///
/// A face that lies under a recovered layer is painted with what is *underneath* the
/// layer, not with what was observed through it: the layer itself is painted on top,
/// once, at its own opacity. That is what makes the ground continuous — the pieces a
/// layer cut a shape into become one colour again, and the emitter's own sibling
/// merging then writes them as one path.
fn face_paint(doc: &ColorDoc, stack: &Stacking, defs: &mut String) -> Paint {
    let n = doc.order.len();
    let base_of: Vec<Option<[f32; 3]>> = (0..n)
        .map(|i| {
            let (an, _) = doc.layers?;
            an.layers
                .iter()
                .any(|l| l.faces.binary_search(&i).is_ok())
                .then(|| an.base_rgb.get(i).copied())
                .flatten()
        })
        .collect();
    let face_opacity = |i: usize| -> f32 { stack.thin.get(i).copied().unwrap_or(1.0) };
    let fills: Vec<String> = (0..n)
        .map(|i| face_fill(doc, i, face_opacity(i), base_of[i], defs))
        .collect();
    let opac: Vec<String> = (0..n)
        .map(|i| opacity_attr(doc, i, face_opacity(i)))
        .collect();
    Paint {
        fills,
        opac,
        base_of,
    }
}

/// The `fill` value of face `i` written at opacity `a` (0..1), pushing any gradient it
/// needs onto `defs`. In order of precedence:
///
/// 1. under a recovered layer, the ground colour `base`;
/// 2. a fade traced natively: its gradient, id `f{i}`;
/// 3. an alpha ramp: a two-stop `linearGradient` of one colour, id `a{i}`;
/// 4. its fill model: a flat colour, un-matted when translucent, or a gradient, id `g{i}`;
/// 5. with no fill model, its palette ink (un-matted when translucent), else black.
///
/// A translucent face was measured *over the matte*, so its colour is un-matted before
/// it is written. Only flat fills: a gradient's stops would each need the same
/// treatment, and a translucent gradient is rare enough that keeping today's behaviour
/// beats a half-done one.
fn face_fill(
    doc: &ColorDoc,
    i: usize,
    a: f32,
    base: Option<[f32; 3]>,
    defs: &mut String,
) -> String {
    if let Some(base) = base {
        return inkvec_trace::color::to_hex(base);
    }
    // A fade traced natively: linear, radial or elliptical, with the stops the fitter
    // found, all in one colour at their own opacities.
    if let Some((model, color)) = doc.fades.get(i).and_then(|f| f.as_ref()) {
        let (frag, attr) = gradient::fade_to_svg(model, color, &format!("f{i}"));
        defs.push_str(&frag);
        return attr;
    }
    // A fade is written as the gradient an editor would use: one colour, two
    // `stop-opacity` values, along the axis the alpha was measured to run.
    if let Some(r) = doc.alpha_ramps.get(i).copied().flatten() {
        let hex = inkvec_trace::color::to_hex(r.color);
        defs.push_str(&format!(
            "<linearGradient id=\"a{i}\" gradientUnits=\"userSpaceOnUse\" x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\"><stop offset=\"0\" stop-color=\"{hex}\" stop-opacity=\"{:.3}\"/><stop offset=\"1\" stop-color=\"{hex}\" stop-opacity=\"{:.3}\"/></linearGradient>",
            r.p0.x, r.p0.y, r.p1.x, r.p1.y, r.a0, r.a1
        ));
        return format!("url(#a{i})");
    }
    match doc.fill_fits.get(i) {
        Some(f) => match (&f.model, a < 1.0) {
            (gradient::FillModel::Flat(c), true) => {
                inkvec_trace::color::to_hex(unmatte(*c, a, doc.matte))
            }
            _ => {
                let (frag, attr) = gradient::fill_to_svg(&f.model, &format!("g{i}"));
                defs.push_str(&frag);
                attr
            }
        },
        None => doc
            .face_color
            .get(i)
            .and_then(|&ci| doc.pal.rgb.get(ci))
            .map(|c| {
                let c = if a < 1.0 {
                    unmatte(*c, a, doc.matte)
                } else {
                    *c
                };
                inkvec_trace::color::to_hex(c)
            })
            .unwrap_or_else(|| "#000000".into()),
    }
}

/// The attribute face `i` adds after its fill, at opacity `a`: ` fill-opacity="a"` to
/// three decimals, or empty when opaque.
///
/// A fade's opacity is in its gradient's stops; stating it again would apply it
/// twice. A flat profile is the exception: its colour is written plain, so its
/// one opacity goes on the attribute as for any wash.
fn opacity_attr(doc: &ColorDoc, i: usize, a: f32) -> String {
    if let Some((model, _)) = doc.fades.get(i).and_then(|f| f.as_ref()) {
        return match model {
            gradient::FillModel::Flat(c) if c[0] < 1.0 => {
                format!(" fill-opacity=\"{:.3}\"", c[0])
            }
            _ => String::new(),
        };
    }
    if a < 1.0 {
        format!(" fill-opacity=\"{a:.3}\"")
    } else {
        String::new()
    }
}

/// Stage 5a: the paint tree. Returns the top-level faces and, per face, the faces nested
/// directly in it, each list in paint order.
///
/// A face is in the tree when it is painted (not dropped, has an outline, has a fill);
/// its parent is its nearest painted ancestor ([`surviving_parent`]). Within a level,
/// larger first -- by the summed area of its outline rings, px² -- so nothing hides its
/// own children. The sort is stable, so equal areas keep label order.
fn paint_tree(
    doc: &ColorDoc,
    nest: &Nesting,
    stack: &Stacking,
    n_fills: usize,
) -> (Vec<usize>, Vec<Vec<usize>>) {
    let (order, outer, pts) = (doc.order, &nest.outer, &nest.pts);
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); order.len()];
    let mut roots: Vec<usize> = Vec::new();
    for i in 0..order.len() {
        if order[i].is_empty() || i >= n_fills || outer[i].is_empty() || stack.dropped[i] {
            continue;
        }
        match surviving_parent(&nest.parent, &stack.dropped, i) {
            Some(p) => children[p].push(i),
            None => roots.push(i),
        }
    }
    // Paint order within a level: larger first, so nothing hides its own children.
    let by_area = |v: &mut Vec<usize>| {
        v.sort_by(|&a, &b| {
            let fa: f64 = outer[a].iter().map(|&k| ring_area(&pts[a][k])).sum();
            let fb: f64 = outer[b].iter().map(|&k| ring_area(&pts[b][k])).sum();
            fb.partial_cmp(&fa).unwrap_or(std::cmp::Ordering::Equal)
        });
    };
    by_area(&mut roots);
    for c in children.iter_mut() {
        by_area(c);
    }
    (roots, children)
}

/// Stage 5b: an element id per face, `{colour}-{n}`, numbered per colour word in label
/// order (see [`colour_name`]).
///
/// Name a gradient face from its own colours, not the palette: a gradient face has no
/// single palette entry, and falling through to "shape" throws away the most useful half
/// of the name. A gradient is named by the mean of its two end stops.
fn face_ids(doc: &ColorDoc) -> Vec<String> {
    let mut used: HashMap<&'static str, usize> = HashMap::new();
    let mut ids: Vec<String> = Vec::with_capacity(doc.order.len());
    for i in 0..doc.order.len() {
        let base = match doc.fill_fits.get(i).map(|f| &f.model) {
            Some(gradient::FillModel::Flat(c)) => colour_name(*c),
            Some(gradient::FillModel::Linear { c0, c1, .. })
            | Some(gradient::FillModel::Radial { c0, c1, .. }) => colour_name([
                0.5 * (c0[0] + c1[0]),
                0.5 * (c0[1] + c1[1]),
                0.5 * (c0[2] + c1[2]),
            ]),
            None => doc
                .face_color
                .get(i)
                .and_then(|&ci| doc.pal.rgb.get(ci))
                .map(|&c| colour_name(c))
                .unwrap_or("shape"),
        };
        let n = used.entry(base).or_insert(0);
        *n += 1;
        ids.push(format!("{base}-{n}"));
    }
    ids
}

/// Stage 6: rings written as strokes, as a map from face to its finished element.
///
/// A face with exactly one hole punched in it, where the face's outline and the hole are
/// both primitives and one is the other offset by a uniform width, is one stroke: see
/// [`annulus_stroke`]. Only where transparency is traced natively (the caller checks);
/// opaque flat paint only, since a stroke carries no `fill-opacity` of its own; and never
/// for a face harmonizing already redraws.
fn annulus_strokes(
    doc: &ColorDoc,
    drawn: &[Vec<usize>],
    stack: &Stacking,
    paint: &Paint,
    harmonized: &harmonize::Harmonized,
    decimals: usize,
) -> HashMap<usize, String> {
    let (order, holes, fills, opac) = (doc.order, &stack.holes, &paint.fills, &paint.opac);
    let single_prim = |f: usize| -> Option<&PrimitiveKind> {
        let &[k] = drawn[f].as_slice() else {
            return None;
        };
        let &[(e, _)] = order[f][k].as_slice() else {
            return None;
        };
        doc.prims.get(e).and_then(|p| p.as_ref()).map(|p| &p.kind)
    };
    (0..order.len())
        .filter(|&i| {
            !stack.dropped[i]
                && holes[i].len() == 1
                && fills[i].starts_with('#')
                && opac[i].is_empty()
                && !harmonized.d.contains_key(&i)
                && !harmonized.symbols.contains_key(&i)
        })
        .filter_map(|i| {
            let (o, h) = (single_prim(i)?, single_prim(holes[i][0])?);
            let (mid, width) = annulus_stroke(o, h)?;
            Some((i, stroke_element(&mid, width, &fills[i], decimals)?))
        })
        .collect()
}

/// Stage 7: the element writer. Holds every per-face decision made so far and writes the
/// paint tree as SVG elements.
struct Writer<'a> {
    /// The rings of each face.
    order: &'a [FaceRings],
    /// The fitted curve of each edge.
    fitted: &'a [FittedPath],
    /// Per edge, its whole-edge primitive, if any.
    prims: &'a [Option<PrimitiveFit>],
    /// Per face, the `fill` value ([`Paint::fills`]).
    fills: &'a [String],
    /// Per face, the opacity attribute ([`Paint::opac`]).
    opac: &'a [String],
    /// Per face, its element id.
    ids: &'a [String],
    /// Per face, the painted faces nested directly in it, in paint order.
    children: &'a [Vec<usize>],
    /// Per face, the rings it draws: its outline.
    drawn: &'a [Vec<usize>],
    /// Per face, the faces punched out of it.
    holes: &'a [Vec<usize>],
    /// Decimals per coordinate.
    decimals: usize,
    /// Faces whose `d` is a harmonized consensus.
    harmonized_d: &'a HashMap<usize, String>,
    /// Faces written as a `<use>` of a shared symbol: (symbol id, transform).
    symbol_use: &'a HashMap<usize, (String, String)>,
    /// Faces written as one stroked primitive ([`annulus_strokes`]).
    strokes: &'a HashMap<usize, String>,
}

impl Writer<'_> {
    /// The whole paint tree from `roots` down, as element text. `under` moves some edges
    /// of some faces ([`crate::seams`]); `painted` receives every written face with its
    /// paint rank, in paint order.
    fn body(
        &self,
        roots: &[usize],
        under: &seams::Overrides,
        painted: &mut Vec<(usize, usize)>,
    ) -> String {
        let mut body = String::new();
        self.emit_level(roots, under, painted, &mut body);
        body
    }

    /// The `d` of a face: its drawn rings, then the transparent faces punched out of it,
    /// for an even-odd fill. A harmonized face's `d` is its consensus instead.
    fn face_d(&self, i: usize, under: &seams::Overrides) -> String {
        if let Some(h_d) = self.harmonized_d.get(&i) {
            return h_d.clone();
        }
        let (order, fitted, decimals) = (self.order, self.fitted, self.decimals);
        let mut d = String::new();
        // A face's own outline is its primitive too, when it fitted one, and for the same
        // reason as a hole's: it is the geometry every neighbour was drawn against. The
        // fitted ring starts wherever the edge walk did, off the primitive by as much as
        // 0.6 px, and a face only lands here with a primitive outline when a hole was
        // punched in it -- `material-icons/qr_code`'s finder squares came out with a
        // slanted top edge, `simple-icons/phpstorm`'s square with a notch in one corner.
        // Only the face's own outline reaches under its neighbours; a hole punched in it
        // is the punched face's outline exactly.
        let mut ring_d = |ring: &Ring, own: bool| match (ring.len() == 1)
            .then(|| self.prims.get(ring[0].0).and_then(|p| p.as_ref()))
            .flatten()
            .and_then(|pf| primitive_d(&pf.kind, decimals))
        {
            Some(p) => d.push_str(&p),
            None if own && !under.is_empty() => fmt_ring_with(
                ring,
                fitted,
                &|k| under.get(&(i, k)).cloned(),
                decimals,
                &mut d,
            ),
            None => fmt_ring(ring, fitted, decimals, &mut d),
        };
        for &k in &self.drawn[i] {
            ring_d(&order[i][k], true);
        }
        for &c in &self.holes[i] {
            for &k in &self.drawn[c] {
                ring_d(&order[c][k], false);
            }
        }
        d
    }

    /// The element a face is written as on its own, and whether it is a primitive (or
    /// stroke) element rather than a path. In order: a `<use>` of a harmonized symbol, its
    /// annulus stroke, a primitive element when its whole outline is one and nothing is
    /// punched out of it, else a `<path>`. `None` when the path would be empty.
    fn face_element(&self, i: usize, under: &seams::Overrides) -> Option<(String, bool)> {
        let fill = &self.fills[i];
        let alpha = self.opac[i].as_str();
        let id = &self.ids[i];
        if let Some((sym_id, matrix)) = self.symbol_use.get(&i) {
            return Some((
                format!(
                    "<use id=\"{id}\" href=\"#{sym_id}\" transform=\"{matrix}\" fill=\"{fill}\"{alpha} fill-rule=\"evenodd\"/>"
                ),
                false,
            ));
        }
        if let Some(el) = self.strokes.get(&i) {
            let mut el = el.clone();
            if let Some(sp) = el.find(' ') {
                el.insert_str(sp, &format!(" id=\"{id}\""));
            }
            return Some((el, true));
        }
        // A primitive element cannot carry a hole, so a face that has to show one through
        // is written as a path even when its outline would have fitted a circle.
        let drawn = &self.drawn[i];
        if drawn.len() == 1 && self.order[i][drawn[0]].len() == 1 && self.holes[i].is_empty() {
            let (k, _) = self.order[i][drawn[0]][0];
            if let Some(pf) = self.prims.get(k).and_then(|p| p.as_ref()) {
                if let Some(mut el) = primitive_element(&pf.kind, fill, alpha, self.decimals) {
                    if let Some(sp) = el.find(' ') {
                        el.insert_str(sp, &format!(" id=\"{id}\""));
                    }
                    return Some((el, true));
                }
            }
        }
        let d = self.face_d(i, under);
        if d.is_empty() {
            return None;
        }
        Some((
            format!("<path id=\"{id}\" d=\"{d}\" fill=\"{fill}\"{alpha} fill-rule=\"evenodd\"/>"),
            false,
        ))
    }

    /// Emit the faces of one nesting level, and recursively their children.
    ///
    /// Siblings are disjoint by construction -- every pixel belongs to one face -- so
    /// all the siblings of one flat colour can be one compound path under even-odd,
    /// which is how an artist draws them: a whole word is one `<path>`, not one per
    /// letter. Before this the tracer wrote one element per connected face and came
    /// back with 3.5x the artist's path count on a detailed wordmark at the same
    /// colour error. Primitives stay their own element and gradient faces each own a
    /// gradient, so neither merges; nor does a harmonized `<use>`. The merged path takes
    /// the first member's place in the paint order and its id; every member's children
    /// follow it.
    fn emit_level(
        &self,
        members: &[usize],
        under: &seams::Overrides,
        painted: &mut Vec<(usize, usize)>,
        out: &mut String,
    ) {
        let (fills, opac, ids) = (self.fills, self.opac, self.ids);
        let mut done = vec![false; members.len()];
        for a in 0..members.len() {
            if done[a] {
                continue;
            }
            done[a] = true;
            let i = members[a];
            let Some((element, is_prim)) = self.face_element(i, under) else {
                continue;
            };
            let mut group = vec![i];
            if !is_prim && fills[i].starts_with('#') && !self.symbol_use.contains_key(&i) {
                for b in a + 1..members.len() {
                    if done[b]
                        || fills[members[b]] != fills[i]
                        || opac[members[b]] != opac[i]
                        || self.symbol_use.contains_key(&members[b])
                    {
                        continue;
                    }
                    let j = members[b];
                    let Some((_, prim_b)) = self.face_element(j, under) else {
                        done[b] = true;
                        continue;
                    };
                    if prim_b {
                        continue;
                    }
                    done[b] = true;
                    group.push(j);
                }
            }
            let element = if group.len() == 1 {
                element
            } else {
                let mut d = String::new();
                for &j in &group {
                    d.push_str(&self.face_d(j, under));
                }
                format!(
                    "<path id=\"{}\" d=\"{d}\" fill=\"{}\"{} fill-rule=\"evenodd\"/>",
                    ids[i], fills[i], opac[i]
                )
            };
            // One element is one paint: its members share a rank, so none of them reaches
            // under another -- inside one even-odd path an overlap would cancel to a hole,
            // and a shared edge inside one path cannot seam in the first place.
            let rank = painted.last().map_or(0, |&(_, r)| r + 1);
            painted.extend(group.iter().map(|&j| (j, rank)));
            let has_children = group.iter().any(|&j| !self.children[j].is_empty());
            if !has_children {
                out.push_str(&element);
                continue;
            }
            // A face with things inside it becomes a group, so selecting the group
            // selects the shape and its contents together.
            out.push_str(&format!("<g id=\"{}-group\">", ids[i]));
            out.push_str(&element);
            for &j in &group {
                self.emit_level(&self.children[j], under, painted, out);
            }
            out.push_str("</g>");
        }
    }
}

/// The colour a face shows on average: the ground under a recovered layer, its flat fill,
/// the mean of a gradient's two end stops, or its palette ink. sRGB 0..1.
fn mean_ink(doc: &ColorDoc, base_of: &[Option<[f32; 3]>], i: usize) -> Option<[f32; 3]> {
    if let Some(b) = base_of.get(i).copied().flatten() {
        return Some(b);
    }
    match doc.fill_fits.get(i).map(|f| &f.model) {
        Some(gradient::FillModel::Flat(c)) => Some(*c),
        Some(gradient::FillModel::Linear { c0, c1, .. })
        | Some(gradient::FillModel::Radial { c0, c1, .. }) => Some([
            0.5 * (c0[0] + c1[0]),
            0.5 * (c0[1] + c1[1]),
            0.5 * (c0[2] + c1[2]),
        ]),
        None => doc
            .face_color
            .get(i)
            .and_then(|&ci| doc.pal.rgb.get(ci))
            .copied(),
    }
}

/// Stage 8: the edges to move so side-by-side faces reach under each other, or `None`
/// when the underlap is switched off or nothing moves.
///
/// `painted` is the paint rank of every face the first writing pass wrote. A face may
/// reach under a neighbour (be the *lower* face) when its outline is written from its own
/// rings -- not a harmonized consensus, a symbol or a stroke. It may be reached under (be
/// the *upper* face) only when it paints opaque everywhere: no opacity attribute, no fade,
/// no alpha ramp. A pair where one is already painted whole beneath the other (an
/// ancestor in the stack) cannot seam and is skipped.
///
/// The seam between faces `v` and `u` shows a quarter of the ground `g` under the pair in
/// place of their mix, so its strength is judged by the RGB distance
/// `|g - (c_v + c_u)/2|` (Euclidean, channels 0..1), where `g` is the mean ink of their
/// nearest common painted ancestor. Two faces with nothing painted under them seam against
/// the page, which may be anything, and count as full contrast (1.0).
fn seam_overrides(
    doc: &ColorDoc,
    nest: &Nesting,
    stack: &Stacking,
    paint: &Paint,
    writer: &Writer,
    painted: &[(usize, usize)],
) -> Option<seams::Overrides> {
    let reach = seams::underlap_width();
    if reach <= 0.0 {
        return None;
    }
    let n = doc.order.len();
    let (parent, dropped) = (&nest.parent, &stack.dropped);
    let surviving = |i: usize| surviving_parent(parent, dropped, i);
    let mut z = vec![None; n];
    for &(f, rank) in painted {
        z[f] = Some(rank);
    }
    let lower_ok: Vec<bool> = (0..n)
        .map(|i| {
            z[i].is_some()
                && !writer.harmonized_d.contains_key(&i)
                && !writer.symbol_use.contains_key(&i)
                && !writer.strokes.contains_key(&i)
        })
        .collect();
    let upper_ok: Vec<bool> = (0..n)
        .map(|i| {
            z[i].is_some()
                && paint.opac[i].is_empty()
                && doc.fades.get(i).is_none_or(|f| f.is_none())
                && doc.alpha_ramps.get(i).is_none_or(|r| r.is_none())
        })
        .collect();
    // Already painted whole beneath: an ancestor in the stack.
    let under = |v: usize, u: usize| -> bool {
        let mut k = u;
        while let Some(p) = surviving(k) {
            if p == v {
                return true;
            }
            k = p;
        }
        false
    };
    let ink = |i: usize| mean_ink(doc, &paint.base_of, i);
    let contrast = |v: usize, u: usize| -> f32 {
        let mut up = Vec::new();
        let mut k = v;
        while let Some(p) = surviving(k) {
            up.push(p);
            k = p;
        }
        let mut k = u;
        let ground = loop {
            match surviving(k) {
                Some(p) if up.contains(&p) => break Some(p),
                Some(p) => k = p,
                None => break None,
            }
        };
        match (ground.and_then(ink), ink(v), ink(u)) {
            (Some(g), Some(a), Some(b)) => {
                let d: f32 = (0..3)
                    .map(|c| (g[c] - 0.5 * (a[c] + b[c])).powi(2))
                    .sum::<f32>()
                    .sqrt();
                d
            }
            _ => 1.0,
        }
    };
    let moved = seams::underlap(
        &seams::Faces {
            order: doc.order,
            fitted: doc.fitted,
            pts: &nest.pts,
            drawn: &nest.outer,
            outer: &nest.outer,
            z: &z,
            lower_ok: &lower_ok,
            upper_ok: &upper_ok,
            contrast: &contrast,
        },
        &under,
        reach,
    );
    (!moved.is_empty()).then_some(moved)
}

/// Stage 9: the recovered translucent layers, over everything, each as one compound path
/// appended to `body`.
///
/// A layer's path is the union of its faces, from the rings they had before the ground
/// under them was merged. They are disjoint, so even-odd paints their union; and because
/// it is one path rather than one per face, the renderer composites the translucent paint
/// once and no seam appears where two of them met.
///
/// The layers are written **back to front**. `an.layers` is in peel order, frontmost first
/// (`inkvec_trace::alpha::AlphaAnalysis::layers`), because the decomposition can only take a
/// layer off once nothing lies over it. Written in that order the frontmost layer went down
/// first and every layer behind it was composited over it: on `synthetic/stack_overlap`
/// the blue disc came out on top of the green one that covers it, dE00 0.019 -> 2.90 with
/// `--layers`. Over a face both cover, the document now draws
/// `a₀·C₀ + (1 − a₀)·(a₁·C₁ + (1 − a₁)·G)`, the stack the decomposition peeled. The ids
/// keep the peel index, `layer-0` frontmost.
fn write_layers(doc: &ColorDoc, decimals: usize, body: &mut String) {
    let Some((an, shape_order)) = doc.layers else {
        return;
    };
    for (k, l) in an.layers.iter().enumerate().rev() {
        let mut d = String::new();
        if let Some(rings) = shape_order.get(k) {
            for ring in rings {
                fmt_ring(ring, doc.fitted, decimals, &mut d);
            }
        }
        if d.is_empty() {
            continue;
        }
        body.push_str(&format!(
            "<path id=\"layer-{k}\" d=\"{d}\" fill=\"{}\" fill-opacity=\"{:.3}\" fill-rule=\"evenodd\"/>",
            inkvec_trace::color::to_hex(l.color),
            l.alpha
        ));
    }
}

/// Bilevel output: every contour as one path with `evenodd`, so holes fall out of the
/// winding rather than needing to be detected and paired up. `paper` writes the white
/// canvas rectangle under it.
///
/// `paths` are closed polygons in the traced image's pixel coordinates (from
/// `inkvec_fit::adjust_vertices`); polygons of fewer than three points are skipped.
pub(crate) fn emit_bilevel(
    paths: &[Vec<Point>],
    w: usize,
    h: usize,
    precision: f64,
    paper: bool,
) -> String {
    let decimals = emit_decimals(precision);
    let mut d = String::new();
    for pts in paths {
        fmt_path(pts, decimals, &mut d);
    }
    let rect = if paper {
        format!("<rect x=\"-0.5\" y=\"-0.5\" width=\"{w}\" height=\"{h}\" fill=\"#ffffff\"/>")
    } else {
        String::new()
    };
    // See emit_color: pixel-centre coordinates, so the canvas origin is (-0.5, -0.5).
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 {w} {h}\" width=\"{w}\" height=\"{h}\">{rect}<path d=\"{d}\" fill=\"#000000\" fill-rule=\"evenodd\"/></svg>"
    )
}
