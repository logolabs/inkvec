//! The document plumbing: paths read out of the SVG, tolerance scaled out of
//! the page, each rewrite weighed byte against byte, the results spliced back.

use std::ops::Range;

use crate::document;
use crate::fit::minify_subpath;
use crate::geom::dist_to_segment;
use crate::path::{parse_d, Src, Subpath};
use crate::write::{decimals_for, primitive_element, Writer, LOSSLESS_DECIMALS};
use crate::{Options, Report};
use inkvec_core::Point;
use inkvec_fit::curves::Segment;
use inkvec_fit::primitives::PrimitiveKind;
use inkvec_fit::FitConfig;

/// The longer side of the drawing in its own units: from `viewBox`, else `width`/`height`.
fn extent(root: roxmltree::Node) -> Option<f64> {
    if let Some(vb) = root.attribute("viewBox") {
        let v: Vec<f64> = vb
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        if v.len() == 4 && v[2] > 0.0 && v[3] > 0.0 {
            return Some(v[2].max(v[3]));
        }
    }
    let num = |a: &str| -> Option<f64> {
        root.attribute(a)?
            .trim_end_matches(|c: char| c.is_alphabetic() || c == '%')
            .parse()
            .ok()
    };
    match (num("width"), num("height")) {
        (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some(w.max(h)),
        _ => None,
    }
}
/// The uniform scale a node's accumulated `transform` applies: `sqrt(|det|)`. A tolerance
/// stated on the page has to be divided by this to hold in the path's own coordinates.
pub(crate) fn node_scale(node: roxmltree::Node) -> f64 {
    let mut m = [1.0, 0.0, 0.0, 1.0];
    for anc in node.ancestors() {
        let Some(t) = anc.attribute("transform") else {
            continue;
        };
        for tok in svgtypes::TransformListParser::from(t).flatten() {
            use svgtypes::TransformListToken as T;
            let (a, b, c, d) = match tok {
                T::Matrix { a, b, c, d, .. } => (a, b, c, d),
                T::Translate { .. } => continue,
                T::Scale { sx, sy } => (sx, 0.0, 0.0, sy),
                T::Rotate { angle } => {
                    let (s, co) = angle.to_radians().sin_cos();
                    (co, s, -s, co)
                }
                T::SkewX { angle } => (1.0, 0.0, angle.to_radians().tan(), 1.0),
                T::SkewY { angle } => (1.0, angle.to_radians().tan(), 0.0, 1.0),
            };
            m = [
                m[0] * a + m[2] * b,
                m[1] * a + m[3] * b,
                m[0] * c + m[2] * d,
                m[1] * c + m[3] * d,
            ];
        }
    }
    let det = (m[0] * m[3] - m[1] * m[2]).abs();
    if det > 1e-12 {
        det.sqrt()
    } else {
        1.0
    }
}
/// What a `d` attribute costs as written: segments, and numbers the reader has to store.
/// `S` and `T` are cheaper than the cubic or quadratic they restate, `H` and `V` cheaper
/// than a line, and an arc's two flags are not coordinates. This, not the fitter's
/// alphabet, is the description length a rewrite has to beat.
fn text_cost(d: &str) -> (usize, f64) {
    use svgtypes::PathSegment as P;
    let (mut segs, mut params) = (0usize, 0.0);
    for seg in svgtypes::PathParser::from(d).flatten() {
        let cost = match seg {
            P::MoveTo { .. } => {
                params += 2.0;
                continue;
            }
            P::ClosePath { .. } => continue,
            P::LineTo { .. } | P::SmoothQuadratic { .. } => 2.0,
            P::HorizontalLineTo { .. } | P::VerticalLineTo { .. } => 1.0,
            P::CurveTo { .. } => 6.0,
            P::SmoothCurveTo { .. } | P::Quadratic { .. } => 4.0,
            P::EllipticalArc { .. } => 5.0,
        };
        segs += 1;
        params += cost;
    }
    (segs, params)
}
/// One `<path>` element, read out of the document so it can be fitted on any thread.
struct Job {
    d: String,
    d_range: Range<usize>,
    quote: char,
    node_range: Range<usize>,
    /// The element's other attributes, as written, when it can be replaced whole.
    other_attrs: Option<Vec<String>>,
    scale: f64,
}
/// What became of one path: the text to splice in, if any, and its share of the report.
struct Outcome {
    edit: Option<(Range<usize>, String)>,
    delta: Report,
}
/// Fit one path and decide, byte against byte, whether the rewrite is kept.
fn rewrite_path(job: &Job, eps_units: f64, ext: f64, opts: &Options, fit: bool) -> Outcome {
    let mut delta = Report {
        paths: 1,
        ..Default::default()
    };
    let (segs0, params0) = text_cost(&job.d);
    delta.segments_before = segs0;
    delta.params_before = params0;
    let keep = |mut delta: Report| {
        delta.segments_after = segs0;
        delta.params_after = params0;
        Outcome { edit: None, delta }
    };
    let Ok(subpaths) = parse_d(&job.d) else {
        return keep(delta); // leave what we cannot read exactly as it is
    };
    let eps = eps_units / job.scale;
    let cfg = FitConfig::from_precision(ext / job.scale, eps, 2.0);
    // Asked for a number of decimals, round to exactly that. Otherwise: a rewrite that
    // refits the drawing may round within the tolerance it is already spending, but one
    // that only respells it may not round at all. Rounding is moving the drawing -- a
    // quarter of a tolerance at a hard edge is a fifth of a pixel of coverage, which is
    // visible in a difference image even though no fit has changed.
    let (most, quantum) = match (opts.decimals, fit) {
        (Some(n), _) => (n, 0.5 * 10f64.powi(-(i32::try_from(n).unwrap_or(6)))),
        (None, true) => (decimals_for(eps), 0.25 * eps),
        // The shortest spelling that is still the same number.
        (None, false) => (LOSSLESS_DECIMALS, 0.0),
    };

    let mut w = Writer::new(quantum, most);
    let mut guarded = 0;
    let mut whole: Option<PrimitiveKind> = None;
    for sp in &subpaths {
        delta.subpaths += 1;
        let first = w.out.is_empty();
        match fit
            .then(|| minify_subpath(sp, eps, &cfg, opts.corner_degrees))
            .flatten()
        {
            Some((start, segs, g, prim)) => {
                guarded += g;
                if subpaths.len() == 1 {
                    whole = prim;
                }
                w.subpath(start, &segs, sp.closed, first);
            }
            None => {
                // The geometry stays exactly as it was; the bytes need not.
                let segs: Vec<Segment> = sp.segs.iter().map(Src::segment).collect();
                w.subpath(sp.segs[0].start(), &segs, sp.closed, first);
            }
        }
    }
    let d = w.out;
    // A path that is one whole primitive becomes the element that says so -- a
    // `<circle>` is three numbers where its path form is seventeen -- with every
    // other attribute carried over byte for byte.
    if let (Some((tag, attrs, cost)), Some(others)) = (
        whole.and_then(|k| primitive_element(&k, quantum, most)),
        &job.other_attrs,
    ) {
        if cost < params0 {
            let mut el = format!("<{tag}");
            for a in others {
                el.push(' ');
                el.push_str(a);
            }
            el.push(' ');
            el.push_str(&attrs);
            el.push_str("/>");
            delta.rewritten = 1;
            delta.primitives = 1;
            delta.guarded = guarded;
            delta.segments_after = 1;
            delta.params_after = cost;
            return Outcome {
                edit: Some((job.node_range.clone(), el)),
                delta,
            };
        }
    }
    // What a minifier owes the reader is a smaller file, so the two are weighed in bytes,
    // and a path that would not get smaller keeps its original text exactly. Fewer numbers
    // is not the same thing: a hand-tightened source writes them in a form we might not
    // beat, and then the honest answer is to leave it alone.
    let (segs1, params1) = text_cost(&d);
    if d.len() < job.d.len() && parses_back(&d, &subpaths, eps) {
        delta.rewritten = 1;
        delta.guarded = guarded;
        delta.segments_after = segs1;
        delta.params_after = params1;
        let q = job.quote;
        Outcome {
            edit: Some((job.d_range.clone(), format!("d={q}{d}{q}"))),
            delta,
        }
    } else {
        keep(delta)
    }
}
/// Does the text we are about to write read back as the drawing we meant?
///
/// The writer drops separators, letters and leading zeros wherever the SVG grammar says a
/// parser does not need them, and reconstructs relative commands from what it wrote. That
/// reasoning is worth checking against an actual parser once per path -- it costs one
/// parse, and it is the difference between a bug that shortens a file and a bug that
/// silently redraws it.
fn parses_back(d: &str, source: &[Subpath], eps: f64) -> bool {
    let Ok(read) = parse_d(d) else {
        return false;
    };
    let ours: Vec<Point> = read
        .iter()
        .flat_map(|sp| sp.segs.iter().map(Src::start))
        .collect();
    if ours.is_empty() {
        return false;
    }
    // Every point of the source must lie on what we wrote. The rewrite moves points by
    // design, so this asks the looser question the guard already asked: nothing is far
    // from the curve it belongs to.
    let limit = 4.0 * eps;
    source.iter().all(|sp| {
        sp.segs.iter().all(|s| {
            let p = s.start();
            ours.iter()
                .map(|&q| p.dist(q))
                .fold(f64::INFINITY, f64::min)
                < limit
                || read.iter().any(|r| {
                    let mut cur = r.segs[0].start();
                    r.segs.iter().any(|seg| {
                        let d = dist_to_segment(p, &seg.segment(), cur);
                        cur = seg.segment().end();
                        d < limit
                    })
                })
        })
    })
}
pub(crate) fn run(svg: &str, opts: &Options, fit: bool) -> Result<(String, Report), String> {
    let doc = roxmltree::Document::parse(svg).map_err(|e| format!("not an SVG document: {e}"))?;
    let root = doc.root_element();
    let ext = extent(root).ok_or("the SVG has no usable viewBox or width/height")?;
    let eps_units = opts.tolerance_px * ext / opts.judge;
    let mut rep = Report {
        tolerance_units: eps_units,
        ..Default::default()
    };

    // Every path is independent, so they are fitted on every core, as the tracer fits its
    // boundaries: the work per path varies by orders of magnitude, which is the shape of
    // problem rayon's work stealing handles. The document is read once, up front, into
    // plain jobs; the results are merged in document order afterwards.
    let jobs: Vec<Job> = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "path")
        .filter_map(|node| {
            let attr = node.attributes().find(|a| a.name() == "d")?;
            let text = &svg[node.range()];
            let replaceable = !node.has_children() && text.trim_end().ends_with("/>");
            Some(Job {
                d: attr.value().to_string(),
                d_range: attr.range(),
                quote: svg[attr.range()].chars().next_back().unwrap_or('"'),
                node_range: node.range(),
                other_attrs: replaceable.then(|| {
                    node.attributes()
                        .filter(|a| a.name() != "d")
                        .map(|a| svg[a.range()].to_string())
                        .collect()
                }),
                scale: node_scale(node),
            })
        })
        .collect();

    use rayon::prelude::*;
    let outcomes: Vec<Outcome> = jobs
        .par_iter()
        .map(|job| rewrite_path(job, eps_units, ext, opts, fit))
        .collect();

    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for o in outcomes {
        let r = &o.delta;
        rep.paths += r.paths;
        rep.rewritten += r.rewritten;
        rep.subpaths += r.subpaths;
        rep.segments_before += r.segments_before;
        rep.segments_after += r.segments_after;
        rep.params_before += r.params_before;
        rep.params_after += r.params_after;
        rep.guarded += r.guarded;
        rep.primitives += r.primitives;
        edits.extend(o.edit);
    }

    // Everything that is not path geometry: colours, numbers, restated defaults, the
    // elements that draw nothing. Ranges a path rewrite has already claimed are left to it.
    if opts.document {
        let taken: Vec<Range<usize>> = edits.iter().map(|(r, _)| r.clone()).collect();
        edits.extend(document::edits(svg, &doc, &taken));
    }

    let mut out = svg.to_string();
    edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
    for (range, text) in edits {
        out.replace_range(range, &text);
    }
    if opts.document {
        out = document::squeeze_whitespace(&out);
    }
    Ok((out, rep))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(svg: &str) -> roxmltree::Document {
        roxmltree::Document::parse(svg).expect("parses")
    }

    #[test]
    fn extent_prefers_the_viewbox_and_falls_back_to_width_and_height() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 256"/>"#;
        assert_eq!(extent(parsed(svg).root_element()), Some(256.0));
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="96"/>"#;
        assert_eq!(extent(parsed(svg).root_element()), Some(96.0));
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#;
        assert_eq!(extent(parsed(svg).root_element()), None);
    }

    /// The description length a rewrite has to beat, as the file writes it: an `H` is one
    /// number, an arc's flags are not coordinates, and the `M` itself is not counted.
    #[test]
    fn text_cost_counts_what_a_reader_stores() {
        // The `M` is not a segment but its two numbers are stored, so they count.
        let (segs, params) = text_cost("M0,0L10,0L10,10Z");
        assert_eq!((segs, params), (2, 6.0));
        let (segs, params) = text_cost("M0,0H10V10Z");
        assert_eq!((segs, params), (2, 4.0));
        let (segs, params) = text_cost("M0,0A5,5 0 1,1 10,0");
        assert_eq!((segs, params), (1, 7.0));
    }

    #[test]
    fn a_transform_scales_the_tolerance() {
        // Under a 10x scale, 0.1 px on the page is 0.01 units in the path.
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1280 1280\">\
                   <g transform=\"scale(10)\"><path d=\"M0,0L10,0L10,10Z\"/></g></svg>";
        let doc = roxmltree::Document::parse(svg).unwrap();
        let node = doc
            .descendants()
            .find(|n| n.tag_name().name() == "path")
            .unwrap();
        assert!((node_scale(node) - 10.0).abs() < 1e-12);
    }
}
