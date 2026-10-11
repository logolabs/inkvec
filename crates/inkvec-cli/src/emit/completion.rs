//! Stage 7b of the emitter: faces completed behind what is painted over them
//! ([`crate::layers`]), with the writer's paint ranks and the gate's count of what each
//! face is written as now. Split from `emit.rs`, whose private types it reads.

use super::*;

/// Stage 7b: each face completed behind what is painted over it, in the paint ranks the
/// first writing pass recorded in `painted` ([`crate::layers`]).
///
/// What covers: every painted face that paints opaque everywhere (no opacity attribute, no
/// fade, no alpha ramp), and the strokes of [`crate::ribbons`], which go on top of
/// everything; a face written as strokes is not a face here. What may be completed: such a
/// face written from its own rings (not a harmonized consensus, a symbol or an annulus
/// stroke) with nothing punched out of it. Its cost now is the gate's count of the element
/// it is written as.
pub(super) fn complete_layers(
    doc: &ColorDoc,
    nest: &Nesting,
    stack: &Stacking,
    paint: &Paint,
    writer: &Writer,
    painted: &[(usize, usize)],
    decimals: usize,
) -> HashMap<usize, crate::layers::Completion> {
    let n = doc.order.len();
    let mut rank = vec![None; n];
    for &(f, r) in painted {
        if f < n && !doc.ribbons.elements.contains_key(&f) {
            rank[f] = Some(r);
        }
    }
    let covers: Vec<bool> = (0..n)
        .map(|i| {
            rank[i].is_some()
                && paint.opac[i].is_empty()
                && doc.fades.get(i).is_none_or(|f| f.is_none())
                && doc.alpha_ramps.get(i).is_none_or(|r| r.is_none())
        })
        .collect();
    let candidate: Vec<bool> = (0..n)
        .map(|i| {
            covers[i]
                && stack.holes[i].is_empty()
                && !writer.harmonized_d.contains_key(&i)
                && !writer.symbol_use.contains_key(&i)
                && !writer.strokes.contains_key(&i)
        })
        .collect();
    let empty = seams::Overrides::new();
    let cost_now = |i: usize| -> f64 {
        match writer.face_element(i, &empty) {
            Some((Element::Path(d), _)) => crate::layers::gate_count(&d),
            Some((Element::Ready(el), _)) => element_count(&el),
            None => 0.0,
        }
    };
    let strokes: Vec<crate::layers::StrokeShape> = doc.ribbons.shapes.values().cloned().collect();
    let stroke_faces: Vec<usize> = doc.ribbons.shapes.keys().copied().collect();
    let (done, tried) = crate::layers::complete_report(&crate::layers::Input {
        order: doc.order,
        fitted: doc.fitted,
        prims: doc.prims,
        outer: &nest.outer,
        holes: &stack.holes,
        rank: &rank,
        covers: &covers,
        candidate: &candidate,
        cost_now: &cost_now,
        strokes: &strokes,
        stroke_faces: &stroke_faces,
        canvas: Some((-0.5, -0.5, doc.w as f64 - 0.5, doc.h as f64 - 0.5)),
        decimals,
    });
    let debug = inkvec_core::env::number("INKVEC_COMPLETION_DEBUG").unwrap_or(0.0);
    if debug != 0.0 {
        let (b, a): (f64, f64) = done
            .values()
            .map(|c| (c.before, c.after))
            .fold((0.0, 0.0), |s, c| (s.0 + c.0, s.1 + c.1));
        eprintln!(
            "layers: {} face(s) completed of {} tried, {b:.0} -> {a:.0} parameters",
            done.len(),
            tried.len()
        );
    }
    // `3`: one tab-separated line per tried face, for `bench/theory/amodal_eval.py`: the face,
    // completed or refused, the winning candidate, the ink, and the element before and after.
    if debug >= 3.0 {
        let element = |el: Option<(Element, bool)>| -> String {
            match el {
                Some((Element::Path(d), _)) => format!("<path d=\"{d}\"/>"),
                Some((Element::Ready(el), _)) => el,
                None => String::new(),
            }
        };
        for &f in &tried {
            let before = element(writer.face_element(f, &empty));
            let (status, kind, after) = match done.get(&f) {
                Some(c) => {
                    let after = match &c.shape {
                        crate::layers::Shape::Prim(k) => {
                            primitive_element(k, "#000000", "", decimals).unwrap_or_default()
                        }
                        crate::layers::Shape::Path(d) => format!("<path d=\"{d}\"/>"),
                    };
                    ("completed", c.kind.as_str(), after)
                }
                None => ("refused", "-", String::new()),
            };
            eprintln!(
                "completion\t{f}\t{status}\t{kind}\t{}\t{before}\t{after}",
                writer.fills[f]
            );
        }
    }
    done
}

/// The gate's count of one finished element: a primitive's own (circle 3, ellipse 4,
/// rectangle 6), else the path data's.
fn element_count(el: &str) -> f64 {
    let t = el.trim_start();
    if t.starts_with("<circle") {
        3.0
    } else if t.starts_with("<ellipse") {
        4.0
    } else if t.starts_with("<rect") {
        6.0
    } else if let Some(s) = t.find(" d=\"") {
        let rest = &t[s + 4..];
        crate::layers::gate_count(&rest[..rest.find('"').unwrap_or(rest.len())])
    } else {
        0.0
    }
}
