//! The emitter writing a drawing the way its artist would: holes cut out of a glyph rather
//! than painted over it (stage 2b), and square rectangles written into their siblings'
//! compound path (stage 7). Split from `emit.rs`, whose private types it reads.

use super::*;

/// The faces that fill a hole of a glyph on the page in the page's colour: dropped, and
/// punched out of the glyph.
///
/// The stacked emitter draws a hole by painting the face inside it on top, so the counter
/// of an opaque letter O came out as a page-coloured disc over a solid one. The picture is
/// the same; the drawing is not the artist's. They cut the counter out of the letter, one
/// path with a hole in it: on the screen set's opaque tiers no trace had a single nested
/// subpath where material-icons' artists have 42 % of theirs, and 47 % of the trace's
/// elements were painted over another where the artists' are 0 %.
///
/// The page is the face that covers the image's frame ([`canvas_background`]), painted one
/// flat colour opaque (no fade, alpha ramp or recovered layer); a transparent ground is
/// clear, and its holes are cut already ([`punch_transparent`]). Face `c`, painted flat and
/// opaque in the page's colour (written the same), is cut out of the face `p` it sits in
/// when:
///
/// * `p` sits on the page, is painted (not dropped, clear or written as strokes) and
///   opaque, and is not punched out of the page, so the page paints under all of `p`;
/// * `c` is [`strictly_inside`] `p` and one of its outline rings shares an edge with a hole
///   ring of `p`: the hole `p` is written with is `c`'s own outline, and the picture is the
///   one painting `c` made (`Design.Layers.cut_hole`);
/// * neither outline is a primitive a path writes in more numbers (any but a
///   [`plain_rect`]);
/// * as in [`punch_background_coloured`], every child of `p` is the page's colour: a glyph
///   and its counters, not a figure with eyes and buttons, which its artist stacks.
///
/// Only the page: a ground the artist painted is the artist's to stack on, and the emoji
/// families do (the letters of `openmoji/1F195` are strokes over a white tile, and cut out
/// of it they moved its dE00 at 512 px in Fast mode by the renderer's rounding, +0.26 %).
/// Nor is a face
/// of the page's colour sitting on the page dropped, though the page paints there too: its
/// neighbours' outlines are not its own, and without it they showed (`openmoji/1F517` on
/// `web`, dE00 +1.2 % in Fast mode).
/// `INKVEC_CUT_HOLES=0` paints the counters over, as before.
pub(super) fn cut_ground_holes(doc: &ColorDoc, nest: &Nesting, stack: &mut Stacking) {
    if !inkvec_core::env::switch("INKVEC_CUT_HOLES", true) {
        return;
    }
    let (order, outer, parent) = (doc.order, &nest.outer, &nest.parent);
    let in_layer = |i: usize| {
        doc.layers
            .is_some_and(|(an, _)| an.layers.iter().any(|l| l.faces.binary_search(&i).is_ok()))
    };
    let painted = |i: usize, stack: &Stacking| {
        !stack.dropped[i]
            && !is_clear(doc, i)
            && !outer[i].is_empty()
            && !doc.ribbons.elements.contains_key(&i)
            && !doc.ribbons.absorbed.contains(&i)
    };
    let opaque = |i: usize, stack: &Stacking| stack.thin.get(i).copied().unwrap_or(1.0) >= 1.0;
    // A face painted one flat colour, opaque: that colour.
    let solid = |i: usize, stack: &Stacking| -> Option<[f32; 3]> {
        let flat = doc
            .fill_fits
            .get(i)
            .is_none_or(|f| matches!(f.model, gradient::FillModel::Flat(_)));
        (painted(i, stack)
            && flat
            && opaque(i, stack)
            && doc.fades.get(i).is_none_or(|f| f.is_none())
            && doc.alpha_ramps.get(i).is_none_or(|r| r.is_none())
            && !in_layer(i))
        .then(|| flat_colour(doc, i))
        .flatten()
    };
    let Some((page, ink)) = canvas_background(doc, nest).and_then(|b| Some((b, solid(b, stack)?)))
    else {
        return;
    };
    // The page's colour exactly as it is written. Nearly is not enough: the bottle cap of
    // `noto-emoji/emoji_u1f9d1_1f3fc_200d_1f37c` is #fdfefa on a #ffffff page, one palette
    // ink and within `same_rgb`, and taken for page it went white, geom 0.065 -> 0.346.
    let hex = inkvec_trace::color::to_hex;
    let page_ink = |o: usize| flat_colour(doc, o).is_some_and(|oc| hex(oc) == hex(ink));
    for c in 0..order.len() {
        if c == page || solid(c, stack).is_none() || !page_ink(c) {
            continue;
        }
        let Some(p) = parent[c].filter(|&p| p != page && painted(p, stack)) else {
            continue;
        };
        if parent[p] != Some(page) || !opaque(p, stack) || stack.holes[page].contains(&p) {
            continue;
        }
        if (0..order.len()).any(|o| parent[o] == Some(p) && !stack.dropped[o] && !page_ink(o))
            || !strictly_inside(nest, c, p)
        {
            continue;
        }
        // Written as a `<circle>`, an ellipse or a rounded rectangle, an outline costs a
        // fraction of the arcs a path needs for it: cut only where that is not given up.
        // A ring of two such outlines would be one stroke ([`annulus_strokes`]), but a
        // stroke's outline is the renderer's offset of the centre line, not the fitted
        // circles: `material-icons/group_remove` read dE00 +20 % at 512 px, and
        // `lucide/app-window-mac` on `web` +3 % and its turning gap +37 %.
        let costly = |f: usize| lone_prim(doc, nest, f).is_some_and(|k| !plain_rect(k));
        if costly(p) || costly(c) {
            continue;
        }
        let in_hole = outer[c].iter().any(|&kc| {
            order[p].iter().enumerate().any(|(kp, pr)| {
                !outer[p].contains(&kp)
                    && pr
                        .iter()
                        .any(|&(pe, _)| order[c][kc].iter().any(|&(ce, _)| ce == pe))
            })
        });
        if in_hole {
            stack.dropped[c] = true;
            if !stack.holes[p].contains(&c) {
                stack.holes[p].push(c);
            }
        }
    }
}

/// The primitive face `f`'s outline is, when it is one ring of one edge that fitted one.
fn lone_prim<'a>(doc: &ColorDoc<'a>, nest: &Nesting, f: usize) -> Option<&'a PrimitiveKind> {
    let &[k] = nest.outer[f].as_slice() else {
        return None;
    };
    let &[(e, _)] = doc.order[f][k].as_slice() else {
        return None;
    };
    doc.prims.get(e).and_then(|p| p.as_ref()).map(|p| &p.kind)
}

/// A rectangle with square corners: in a compound path it is three lines, the six numbers
/// of a `<rect>`, so it joins the path of its same-coloured siblings rather than standing
/// alone. The artist draws the bars of `material-icons/border_left` as one path of
/// seventeen rectangles; written one element each, the trace had seventeen elements.
pub(super) fn plain_rect(kind: &PrimitiveKind) -> bool {
    matches!(*kind, PrimitiveKind::RoundRect { rx, .. } if rx.abs() < 1e-4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkvec_fit::curves::Segment;
    use inkvec_fit::primitives::PrimitiveFit;

    /// A closed square edge from `(a, a)` to `(b, b)`.
    fn square(a: f64, b: f64) -> FittedPath {
        FittedPath {
            start: Point::new(a, a),
            segments: vec![
                Segment::Line(Point::new(b, a)),
                Segment::Line(Point::new(b, b)),
                Segment::Line(Point::new(a, b)),
                Segment::Line(Point::new(a, a)),
            ],
            closed: true,
        }
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Option<PrimitiveFit> {
        Some(PrimitiveFit {
            kind: PrimitiveKind::RoundRect {
                x,
                y,
                w,
                h,
                rx: 0.0,
            },
            chi2: 0.0,
            params: 6.0,
        })
    }

    /// The document of faces `order` in palette inks `ink`: white, black, red, grey.
    fn emit(
        order: &[Vec<Ring>],
        fitted: &[FittedPath],
        prims: &[Option<PrimitiveFit>],
        ink: &[usize],
    ) -> String {
        let pal = inkvec_trace::Palette {
            colors: vec![],
            rgb: vec![
                [1.0, 1.0, 1.0],
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.5, 0.5, 0.5],
            ],
            weight: vec![1.0; 4],
            alpha: vec![1.0; 4],
        };
        let n = order.len();
        let doc = ColorDoc {
            order,
            fitted,
            prims,
            fill_fits: &[],
            pal: &pal,
            face_color: ink,
            clear: &vec![false; n],
            opacity: &vec![1.0; n],
            alpha_ramps: &vec![None; n],
            fades: &vec![None; n],
            layers: None,
            matte: [1.0, 1.0, 1.0],
            ribbons: &Default::default(),
            w: 40,
            h: 40,
        };
        let opts = EmitOptions {
            precision: 0.1,
            native: true,
            harmonize: false,
            harmonize_threshold: 0.05,
            use_symbols: false,
            layers: false,
            cutout: false,
            no_background: false,
        };
        emit_color(&doc, &opts)
    }

    fn elements(svg: &str) -> usize {
        ["<path", "<rect", "<circle"]
            .iter()
            .map(|t| svg.matches(t).count())
            .sum()
    }

    /// A page (edge 0), a glyph on it (edge 1) and the glyph's counter (edge 2), each face's
    /// rings sharing the edge between them, in inks `inks`.
    fn glyph(inks: [usize; 3]) -> String {
        let order = vec![
            vec![vec![(0, false)], vec![(1, true)]],
            vec![vec![(1, false)], vec![(2, true)]],
            vec![vec![(2, false)]],
        ];
        let fitted = vec![square(-0.5, 39.5), square(8.0, 30.0), square(14.0, 24.0)];
        emit(&order, &fitted, &[None, None, None], &inks)
    }

    #[test]
    fn counter_in_the_page_colour_is_cut_out_of_its_glyph() {
        let svg = glyph([0, 1, 0]);
        // The page and the glyph, the counter a second ring of the glyph's path.
        assert_eq!(elements(&svg), 2, "{svg}");
        let glyph_d = svg
            .split("<path")
            .find(|p| p.contains("fill=\"#000000\""))
            .expect("the glyph is a path");
        assert_eq!(glyph_d.matches('M').count(), 2, "{svg}");
    }

    #[test]
    fn face_of_the_page_colour_on_the_page_is_kept() {
        // A white face straight on the white page is no hole of anything: it is painted.
        let order = vec![
            vec![vec![(0, false)], vec![(1, true)]],
            vec![vec![(1, false)]],
        ];
        let fitted = vec![square(-0.5, 39.5), square(8.0, 30.0)];
        assert_eq!(elements(&emit(&order, &fitted, &[None, None], &[0, 0])), 2);
    }

    #[test]
    fn round_counter_is_painted_over() {
        let circle = |r: f64| {
            Some(PrimitiveFit {
                kind: PrimitiveKind::Circle {
                    c: Point::new(19.0, 19.0),
                    r,
                },
                chi2: 0.0,
                params: 3.0,
            })
        };
        let order = vec![
            vec![vec![(0, false)], vec![(1, true)]],
            vec![vec![(1, false)], vec![(2, true)]],
            vec![vec![(2, false)]],
        ];
        let fitted = vec![square(-0.5, 39.5), square(8.0, 30.0), square(14.0, 24.0)];
        // A round counter in a square glyph: its `<circle>` is cheaper than the arcs a hole
        // would need, so it is painted over.
        let svg = emit(&order, &fitted, &[None, None, circle(5.0)], &[0, 1, 0]);
        assert_eq!(svg.matches("<circle").count(), 1, "{svg}");
        // Nor is a ring of two circles cut, which would be a stroke.
        let svg = emit(
            &order,
            &fitted,
            &[None, circle(11.0), circle(5.0)],
            &[0, 1, 0],
        );
        assert_eq!(svg.matches("<circle").count(), 2, "{svg}");
        assert!(!svg.contains("stroke-width"), "{svg}");
    }

    #[test]
    fn counter_of_another_colour_is_painted_over() {
        // Grey on a white page: the hole would show white, not grey.
        assert_eq!(elements(&glyph([0, 1, 3])), 3);
    }

    #[test]
    fn figure_with_coloured_contents_stays_stacked() {
        // The glyph also holds a red face: a figure with its eyes and buttons, not a glyph.
        let order = vec![
            vec![vec![(0, false)], vec![(1, true)]],
            vec![vec![(1, false)], vec![(2, true)], vec![(3, true)]],
            vec![vec![(2, false)]],
            vec![vec![(3, false)]],
        ];
        let fitted = vec![
            square(-0.5, 39.5),
            square(4.0, 36.0),
            square(8.0, 14.0),
            square(20.0, 30.0),
        ];
        let svg = emit(&order, &fitted, &[None, None, None, None], &[0, 1, 0, 2]);
        assert_eq!(elements(&svg), 4, "{svg}");
    }

    #[test]
    fn square_rectangles_of_one_colour_are_one_path() {
        // Two black bars on a white page: one compound path, as the artist writes them.
        let order = vec![
            vec![vec![(0, false)], vec![(1, true)], vec![(2, true)]],
            vec![vec![(1, false)]],
            vec![vec![(2, false)]],
        ];
        let fitted = vec![square(-0.5, 39.5), square(4.0, 10.0), square(20.0, 26.0)];
        let prims = vec![None, rect(4.0, 4.0, 6.0, 6.0), rect(20.0, 20.0, 6.0, 6.0)];
        let svg = emit(&order, &fitted, &prims, &[0, 1, 1]);
        assert_eq!(elements(&svg), 2, "{svg}");
        assert_eq!(svg.matches("<rect").count(), 0, "{svg}");
        // Of two colours, each bar is still a `<rect>`.
        let svg = emit(&order, &fitted, &prims, &[0, 1, 2]);
        assert_eq!(svg.matches("<rect").count(), 2, "{svg}");
    }
}
