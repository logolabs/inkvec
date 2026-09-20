//! The fewest segments that draw the same picture.
//!
//! An SVG path can be written a thousand ways: a circle as sixteen cubics or as five, a
//! straight edge as a run of collinear lines, one curve as four exactly-subdivided pieces.
//! They all render identically, and every extra segment is description length that says
//! nothing. This rewrites each path as the cheapest description that stays within a
//! tolerance of the original curve, scored the way the tracer scores everything:
//! `0.5·chi² + λ·params`, the minimum-description-length objective of `inkvec-fit`.
//!
//! What makes this different from a tolerance-based simplifier is where corners come
//! from. A raster tracer has to *infer* corners from pixels; here the source is vector, so
//! a corner is a fact: two consecutive segments whose tangents disagree. Those joins are
//! hard breaks that no fit may smooth across, and everything between two of them is
//! refitted freely. Sharp things stay sharp by construction, not by luck.
//!
//! The tolerance is stated at a viewing size: "invisible at 1024 px" is a property of the
//! picture, where "0.01 units" depends on an arbitrary viewBox. Only `d` attributes are
//! touched; paint, ids, groups, gradients and transforms pass through untouched, and a
//! path that would not get cheaper is left exactly as it was.

mod document;
mod driver;
mod fit;
mod geom;
mod path;
mod write;

#[cfg(test)]
use inkvec_core::{Point, Polyline};
#[cfg(test)]
use inkvec_fit::{curves::Segment, primitives::fit_primitive_or_arcs, FitConfig, FittedPath};

/// How the rewrite is judged.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Largest deviation from the original curve, in pixels, when the drawing is viewed at
    /// `judge` pixels on its longer side. 0.1 px at 1024 px is below what a screen shows.
    pub tolerance_px: f64,
    /// The viewing size the tolerance is stated at.
    pub judge: f64,
    /// Turn between two consecutive source segments, in degrees, above which their join
    /// is a corner that must survive exactly.
    pub corner_degrees: f64,
    /// Decimals written per coordinate; `None` derives them from the tolerance.
    pub decimals: Option<usize>,
    /// Also shorten everything that is not path geometry: colours, numeric attributes,
    /// presentation attributes restating a value that already applies, comments,
    /// `<metadata>`, `<desc>`, and whitespace between tags. Nothing that renders or that
    /// a screen reader speaks is removed. See [`document`].
    pub document: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            tolerance_px: 0.1,
            judge: 1024.0,
            corner_degrees: 30.0,
            decimals: None,
            document: true,
        }
    }
}

/// What the rewrite did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// `<path>` elements seen.
    pub paths: usize,
    /// Paths whose `d` was rewritten (the rest got no cheaper and were left alone).
    pub rewritten: usize,
    /// Subpaths seen.
    pub subpaths: usize,
    /// Segments before and after, counting every source segment as one.
    pub segments_before: usize,
    /// Segments after.
    pub segments_after: usize,
    /// Description length before and after, in the fitter's parameter units (a line is
    /// 2, a cubic 6, an arc fewer).
    pub params_before: f64,
    /// Description length after.
    pub params_after: f64,
    /// Runs whose fit strayed past the tolerance and were replaced by their source.
    pub guarded: usize,
    /// Paths written as a `<circle>`, `<ellipse>` or `<rect>` element instead of a path.
    pub primitives: usize,
    /// The tolerance actually used, in the document's units.
    pub tolerance_units: f64,
}

/// Rewrite every `<path d>` in `svg` as its cheapest description within the tolerance.
pub fn minify(svg: &str, opts: &Options) -> Result<(String, Report), String> {
    driver::run(svg, opts, true)
}

/// Rewrite every `<path d>` in the fewest bytes, leaving the drawing alone.
///
/// The same writer as [`minify`] without the fitter: no segment is removed, moved or
/// re-chosen, and — unless `Options::decimals` says otherwise — no coordinate is rounded
/// either, so the picture that comes out is the picture that went in, pixel for pixel.
/// Only the spelling changes.
///
/// This is what an emitter that already knows its own geometry wants: it can hand over
/// the precision it chose (`decimals: Some(2)` for the tracer, which is what it already
/// rounds to) and get the bytes back without giving up anything.
pub fn compact(svg: &str, opts: &Options) -> Result<(String, Report), String> {
    driver::run(svg, opts, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::node_scale;
    use crate::fit::ls_cubic_from;
    use crate::geom::{cubic_at, dist_to_segment, Curve};
    use crate::path::{parse_d, sample, Src};

    fn circle_as_cubics(cx: f64, cy: f64, r: f64, n: usize) -> String {
        // A circle as `n` cubic arcs, each exact to the usual 4/3·tan(θ/4) handle.
        let mut d = String::new();
        let step = std::f64::consts::TAU / n as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan() * r;
        for i in 0..n {
            let (a0, a1) = (i as f64 * step, (i + 1) as f64 * step);
            let p0 = (cx + r * a0.cos(), cy + r * a0.sin());
            let p3 = (cx + r * a1.cos(), cy + r * a1.sin());
            let c1 = (p0.0 - k * a0.sin(), p0.1 + k * a0.cos());
            let c2 = (p3.0 + k * a1.sin(), p3.1 - k * a1.cos());
            if i == 0 {
                d.push_str(&format!("M{},{}", p0.0, p0.1));
            }
            d.push_str(&format!(
                "C{},{} {},{} {},{}",
                c1.0, c1.1, c2.0, c2.1, p3.0, p3.1
            ));
        }
        d.push('Z');
        d
    }

    fn doc(d: &str) -> String {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 128 128\">\
             <path id=\"keep\" fill=\"#c9754a\" d=\"{d}\"/></svg>"
        )
    }

    #[test]
    fn an_oversegmented_circle_gets_cheaper_and_stays_a_circle() {
        let (out, rep) = minify(
            &doc(&circle_as_cubics(64.0, 64.0, 40.0, 16)),
            &Options::default(),
        )
        .expect("minifies");
        assert_eq!(rep.paths, 1);
        assert_eq!(rep.rewritten, 1);
        assert_eq!(rep.primitives, 1, "{rep:?}");
        assert_eq!(rep.params_after, 3.0, "{rep:?}");
        // The element that says so, with the path's other attributes carried over.
        let doc = roxmltree::Document::parse(&out).unwrap();
        let el = doc
            .descendants()
            .find(|n| n.tag_name().name() == "circle")
            .unwrap_or_else(|| panic!("no <circle> in {out}"));
        assert_eq!(el.attribute("fill"), Some("#c9754a"));
        assert_eq!(el.attribute("id"), Some("keep"));
        assert!(el.attribute("d").is_none());
        let num = |a: &str| el.attribute(a).unwrap().parse::<f64>().unwrap();
        let eps = rep.tolerance_units;
        assert!(
            (num("cx") - 64.0).abs() < eps && (num("cy") - 64.0).abs() < eps,
            "{out}"
        );
        assert!((num("r") - 40.0).abs() < eps, "{out}");
    }

    fn the_nearest_segment_lookup_matches_brute_force() {
        let subs = parse_d(&circle_as_cubics(64.0, 64.0, 40.0, 16)).unwrap();
        let eps = 0.0125;
        let cfg = FitConfig::from_precision(128.0, eps, 2.0);
        let pts = sample(&subs[0].segs, 2.0 * eps, true);
        let poly = Polyline::new(pts.clone(), vec![eps; pts.len()], true);
        let (segs, _, _) =
            fit_primitive_or_arcs(&poly.points, &poly.sigma, true, &cfg).expect("arcs");
        let fitted = FittedPath {
            start: pts[0],
            segments: segs,
            closed: true,
        };
        let curve = Curve::new(&fitted);
        let mut starts = Vec::new();
        let mut cur = fitted.start;
        for s in &fitted.segments {
            starts.push(cur);
            cur = s.end();
        }
        for p in sample(&subs[0].segs, 0.5, true) {
            let brute = fitted
                .segments
                .iter()
                .zip(&starts)
                .map(|(s, &st)| dist_to_segment(p, s, st))
                .fold(f64::INFINITY, f64::min);
            let (k, fast) = curve.nearest(p);
            assert!(
                (fast - brute).abs() < 1e-6,
                "point {p:?}: fast {fast} via segment {k}, brute {brute}; segments {}",
                fitted.segments.len()
            );
        }
    }

    /// A thin sliver of long segments: the far side's table sample is nearer to a point
    /// than the near side's, and a lookup that trusts the table reads the sliver's width
    /// (0.2) as the distance to a path the point lies on.

    fn the_nearest_lookup_is_not_fooled_by_the_far_side_of_a_thin_shape() {
        let fitted = FittedPath {
            start: Point::new(0.0, 0.0),
            segments: vec![
                Segment::Line(Point::new(100.0, 0.0)),
                Segment::Line(Point::new(100.0, 0.2)),
                // The far side's samples are offset by half a gap from the near side's.
                Segment::Line(Point::new(-6.25, 0.2)),
                Segment::Line(Point::new(0.0, 0.0)),
            ],
            closed: true,
        };
        let curve = Curve::new(&fitted);
        for i in 0..=1000 {
            let p = Point::new(f64::from(i) * 0.1, 0.0);
            let (k, d) = curve.nearest(p);
            assert!(
                d < 1e-9,
                "point {p:?} is on the path; lookup says {d} via segment {k}"
            );
        }
    }

    #[test]
    fn a_sharp_rectangle_becomes_a_rect_element() {
        let (out, rep) =
            minify(&doc("M10,20L100,20L100,80L10,80Z"), &Options::default()).expect("minifies");
        assert_eq!(rep.primitives, 1, "{rep:?}\n{out}");
        assert_eq!(rep.params_after, 4.0);
        assert!(out.contains("<rect"), "{out}");
        assert!(
            out.contains("x=\"10\"") && out.contains("y=\"20\""),
            "{out}"
        );
        assert!(
            out.contains("width=\"90\"") && out.contains("height=\"60\""),
            "{out}"
        );
        assert!(
            out.contains("fill=\"#c9754a\"") && out.contains("id=\"keep\""),
            "{out}"
        );
        // A rectangle that is not axis-aligned stays a path.
        let (_, rep) =
            minify(&doc("M10,20L100,30L90,80L0,70Z"), &Options::default()).expect("minifies");
        assert_eq!(rep.primitives, 0, "{rep:?}");
    }

    #[test]
    fn a_square_keeps_its_four_corners_exactly() {
        // Each edge drawn as three collinear pieces: twelve lines that should become four.
        let mut d = String::from("M10,10");
        for (x, y) in [
            (40.0, 10.0),
            (70.0, 10.0),
            (100.0, 10.0),
            (100.0, 40.0),
            (100.0, 70.0),
            (100.0, 100.0),
            (70.0, 100.0),
            (40.0, 100.0),
            (10.0, 100.0),
            (10.0, 70.0),
            (10.0, 40.0),
        ] {
            d.push_str(&format!("L{x},{y}"));
        }
        d.push('Z');
        let (out, rep) = minify(&doc(&d), &Options::default()).expect("minifies");
        // Three lines and a `Z`: the fourth edge is the one `Z` draws on its way home.
        assert_eq!(rep.segments_after, 3, "{rep:?}\n{out}");
        // Read the corners back out of the text: the writer is free to say `H100` or
        // `h90`, so what has to survive is the geometry, not the spelling.
        let drawn = parse_d(&attr_d(&out)).expect("our own output parses");
        let corners: Vec<Point> = drawn
            .iter()
            .flat_map(|sp| sp.segs.iter())
            .flat_map(|s| [s.start(), s.segment().end()])
            .collect();
        for (x, y) in [(10.0, 10.0), (100.0, 10.0), (100.0, 100.0), (10.0, 100.0)] {
            let corner = Point::new(x, y);
            assert!(
                corners.iter().any(|p| p.dist(corner) < 1e-9),
                "corner {corner:?} lost in {out}"
            );
        }
    }

    /// The `d` attribute of the first path in a document. Split on the space before it:
    /// `id="` ends in `d="` as well, and matching that reads the wrong attribute.
    fn attr_d(svg: &str) -> String {
        let after = svg.split(" d=\"").nth(1).expect("a path with a d");
        after
            .split('"')
            .next()
            .expect("a closing quote")
            .to_string()
    }

    #[test]
    fn a_subdivided_cubic_collapses_to_one() {
        // One cubic split exactly in two by de Casteljau at t = 0.5.
        let (p0, c1, c2, p3) = ((10.0, 100.0), (30.0, 10.0), (90.0, 10.0), (110.0, 100.0));
        let mid = |a: (f64, f64), b: (f64, f64)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let (q1, q2, q3) = (mid(p0, c1), mid(c1, c2), mid(c2, p3));
        let (r1, r2) = (mid(q1, q2), mid(q2, q3));
        let s = mid(r1, r2);
        let d = format!(
            "M{},{}C{},{} {},{} {},{}C{},{} {},{} {},{}",
            p0.0, p0.1, q1.0, q1.1, r1.0, r1.1, s.0, s.1, r2.0, r2.1, q3.0, q3.1, p3.0, p3.1
        );
        let (_, rep) = minify(&doc(&d), &Options::default()).expect("minifies");
        assert_eq!(rep.segments_after, 1, "{rep:?}");
    }

    fn least_squares_recovers_a_piece_of_an_exact_cubic() {
        // The same subdivided cubic the minifier sees, sampled as it samples it, with a
        // span that ends part-way through the second half.
        let (p0, c1, c2, p3) = ((10.0, 100.0), (30.0, 10.0), (90.0, 10.0), (110.0, 100.0));
        let mid = |a: (f64, f64), b: (f64, f64)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let (q1, q2, q3) = (mid(p0, c1), mid(c1, c2), mid(c2, p3));
        let (r1, r2) = (mid(q1, q2), mid(q2, q3));
        let s = mid(r1, r2);
        let pt = |a: (f64, f64)| Point::new(a.0, a.1);
        let run = [
            Src::Cubic(pt(p0), pt(q1), pt(r1), pt(s)),
            Src::Cubic(pt(s), pt(r2), pt(q3), pt(p3)),
        ];
        let pts = sample(&run, 0.025, false);
        let span = &pts[..=38];
        let (a, b) = (span[0], span[span.len() - 1]);
        let along: Vec<f64> = (0..span.len()).map(|i| i as f64 / 38.0).collect();
        let (n1, n2) = ls_cubic_from(span, a, b, along).expect("fits");
        let seg = Segment::Cubic(n1, n2, b);
        let dev = span
            .iter()
            .map(|&p| dist_to_segment(p, &seg, a))
            .fold(0.0, f64::max);
        assert!(dev < 1e-3, "a piece of a cubic is a cubic; missed by {dev}");
    }

    fn least_squares_recovers_an_exact_cubic() {
        let (p0, c1, c2, p3) = (
            Point::new(10.0, 100.0),
            Point::new(30.0, 10.0),
            Point::new(90.0, 10.0),
            Point::new(110.0, 100.0),
        );
        let pts: Vec<Point> = (0..=96)
            .map(|i| cubic_at(p0, c1, c2, p3, i as f64 / 96.0))
            .collect();
        let along: Vec<f64> = (0..=96).map(|i| f64::from(i) / 96.0).collect();
        let (n1, n2) = ls_cubic_from(&pts, p0, p3, along).expect("solvable");
        let worst = pts
            .iter()
            .map(|&p| dist_to_segment(p, &Segment::Cubic(n1, n2, p3), p0))
            .fold(0.0, f64::max);
        assert!(worst < 1e-3, "c1 {n1:?} c2 {n2:?} worst {worst}");
    }

    #[test]
    fn a_path_already_at_its_shortest_is_left_byte_for_byte() {
        // The two lines a triangle needs, written the way the writer would write them —
        // relative, because `h90` is shorter than `H100`. There is nothing left to take
        // out, so the bytes must come back untouched. The document pass is off, because it
        // would rightly take the unreferenced `id` with it.
        let src = doc("M10 10h90v90Z");
        let opts = Options {
            document: false,
            ..Default::default()
        };
        let (out, rep) = minify(&src, &opts).expect("minifies");
        assert_eq!(rep.rewritten, 0, "{rep:?}\n{out}");
        assert_eq!(out, src);
    }

    #[test]
    fn a_verbose_path_is_rewritten_shorter_without_moving() {
        let src = doc("M10,10L100,10L100,100Z");
        let (out, rep) = minify(&src, &Options::default()).expect("minifies");
        assert_eq!(rep.rewritten, 1, "{rep:?}\n{out}");
        assert!(out.len() < src.len(), "{out}");
        let drawn = parse_d(&attr_d(&out)).expect("our own output parses");
        let corners: Vec<Point> = drawn
            .iter()
            .flat_map(|sp| sp.segs.iter())
            .map(Src::start)
            .collect();
        for (x, y) in [(10.0, 10.0), (100.0, 10.0), (100.0, 100.0)] {
            let c = Point::new(x, y);
            assert!(
                corners.iter().any(|p| p.dist(c) < 1e-9),
                "corner {c:?} lost in {out}"
            );
        }
    }

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
