//! Reading an SVG into painted regions, in paint order and in millimetres.
//!
//! Any SVG `usvg` understands is accepted, not only the tracer's own: fabrication is as
//! useful on a logo someone drew as on one Inkvec traced. Every painted fill and stroke
//! becomes an [`Item`]: its region (strokes are outlined), its colour, and what kind of
//! paint it was, because a gradient or a translucent fill has no physical equivalent in a
//! sheet of vinyl and the preflight has to say so.

use kurbo::{BezPath, PathEl, Point};

use crate::geom::{self, Contour, Region};

/// What a painted item was drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PaintKind {
    /// One solid colour.
    Solid,
    /// A linear or radial gradient; its colour here is the mean of its stops.
    Gradient,
    /// A pattern; treated as grey.
    Pattern,
}

/// One painted fill or stroke.
#[derive(Clone, Debug)]
pub struct Item {
    /// Its area, in millimetres, normalised.
    pub region: Region,
    /// Its colour, sRGB bytes.
    pub rgb: [u8; 3],
    /// What it was painted with.
    pub kind: PaintKind,
    /// Combined opacity of the paint and its groups.
    pub opacity: f32,
    /// True when it came from a stroke rather than a fill.
    pub from_stroke: bool,
    /// Path segments as written in the source, before flattening.
    pub nodes: usize,
}

/// The drawing, in paint order (first is bottom).
#[derive(Clone, Debug)]
pub struct Artwork {
    /// Painted items, bottom first.
    pub items: Vec<Item>,
    /// The canvas size, in millimetres.
    pub size_mm: [f64; 2],
    /// The canvas size in the SVG's own units (CSS pixels).
    pub size_px: [f64; 2],
    /// Things that were in the file and cannot be cut: raster images, text left as text.
    pub unsupported: Vec<String>,
}

/// Why a file could not be read.
#[derive(Debug)]
pub struct LoadError(pub String);

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoadError {}

/// Parse `svg` and scale it so its canvas is `width_mm` wide (keeping its aspect ratio).
/// `tolerance_mm` is the largest distance a flattened curve may stray from the original.
pub fn load(svg: &str, width_mm: f64, tolerance_mm: f64) -> Result<Artwork, LoadError> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default())
        .map_err(|e| LoadError(format!("not a readable SVG: {e}")))?;
    let size = tree.size();
    let (w, h) = (size.width() as f64, size.height() as f64);
    if w.is_nan() || h.is_nan() || w <= 0.0 || h <= 0.0 {
        return Err(LoadError("the SVG has no size".into()));
    }
    let scale = width_mm / w;
    let mut art = Artwork {
        items: Vec::new(),
        size_mm: [width_mm, h * scale],
        size_px: [w, h],
        unsupported: Vec::new(),
    };
    walk(tree.root(), 1.0, scale, tolerance_mm, &mut art);
    Ok(art)
}

fn walk(group: &usvg::Group, opacity: f32, scale: f64, tol: f64, art: &mut Artwork) {
    let opacity = opacity * group.opacity().get();
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => walk(g, opacity, scale, tol, art),
            usvg::Node::Path(p) if p.is_visible() => add_path(p, opacity, scale, tol, art),
            usvg::Node::Path(_) => {}
            usvg::Node::Image(_) => art.unsupported.push("a raster image".into()),
            usvg::Node::Text(_) => art
                .unsupported
                .push("text that is not converted to paths".into()),
        }
    }
}

fn add_path(p: &usvg::Path, opacity: f32, scale: f64, tol: f64, art: &mut Artwork) {
    let ts = p.abs_transform();
    let map = |x: f32, y: f32| -> Point {
        let (x, y) = (x as f64, y as f64);
        let tx = ts.sx as f64 * x + ts.kx as f64 * y + ts.tx as f64;
        let ty = ts.ky as f64 * x + ts.sy as f64 * y + ts.ty as f64;
        Point::new(tx * scale, ty * scale)
    };
    let mut bez = BezPath::new();
    let mut nodes = 0usize;
    for seg in p.data().segments() {
        nodes += 1;
        match seg {
            usvg::tiny_skia_path::PathSegment::MoveTo(a) => bez.move_to(map(a.x, a.y)),
            usvg::tiny_skia_path::PathSegment::LineTo(a) => bez.line_to(map(a.x, a.y)),
            usvg::tiny_skia_path::PathSegment::QuadTo(a, b) => {
                bez.quad_to(map(a.x, a.y), map(b.x, b.y))
            }
            usvg::tiny_skia_path::PathSegment::CubicTo(a, b, c) => {
                bez.curve_to(map(a.x, a.y), map(b.x, b.y), map(c.x, c.y))
            }
            usvg::tiny_skia_path::PathSegment::Close => bez.close_path(),
        }
    }
    let subpaths = flatten(&bez, tol);
    if let Some(fill) = p.fill() {
        let closed: Vec<Contour> = subpaths.iter().map(|(c, _)| c.clone()).collect();
        let even_odd = fill.rule() == usvg::FillRule::EvenOdd;
        let region = geom::normalise(&closed, even_odd);
        if !region.is_empty() {
            let (rgb, kind) = paint_colour(fill.paint());
            art.items.push(Item {
                region,
                rgb,
                kind,
                opacity: opacity * fill.opacity().get(),
                from_stroke: false,
                nodes,
            });
        }
    }
    if let Some(stroke) = p.stroke() {
        // A stroke's width scales with the transform; the mean scale is exact for the
        // similarity transforms real files use.
        let k = ((ts.sx * ts.sy - ts.kx * ts.ky).abs() as f64).sqrt() * scale;
        let width = stroke.width().get() as f64 * k;
        let parts: Vec<Region> = subpaths
            .iter()
            .map(|(c, closed)| geom::stroke(c, width, *closed))
            .collect();
        let region = geom::union_all(parts.iter());
        if !region.is_empty() {
            let (rgb, kind) = paint_colour(stroke.paint());
            art.items.push(Item {
                region,
                rgb,
                kind,
                opacity: opacity * stroke.opacity().get(),
                from_stroke: true,
                nodes,
            });
        }
    }
}

/// Subpaths as polylines, with whether each was closed.
fn flatten(bez: &BezPath, tol: f64) -> Vec<(Contour, bool)> {
    let mut out: Vec<(Contour, bool)> = Vec::new();
    let mut cur: Contour = Vec::new();
    let finish = |cur: &mut Contour, closed: bool, out: &mut Vec<(Contour, bool)>| {
        if cur.len() >= 2 {
            out.push((std::mem::take(cur), closed));
        } else {
            cur.clear();
        }
    };
    kurbo::flatten(bez.iter(), tol, |el| match el {
        PathEl::MoveTo(p) => {
            finish(&mut cur, false, &mut out);
            cur.push([p.x, p.y]);
        }
        PathEl::LineTo(p) => cur.push([p.x, p.y]),
        PathEl::ClosePath => finish(&mut cur, true, &mut out),
        _ => {}
    });
    finish(&mut cur, false, &mut out);
    out
}

fn paint_colour(paint: &usvg::Paint) -> ([u8; 3], PaintKind) {
    let stops_mean = |stops: &[usvg::Stop]| -> [u8; 3] {
        if stops.is_empty() {
            return [128; 3];
        }
        let n = stops.len() as f64;
        let m = |f: fn(&usvg::Color) -> u8| {
            (stops.iter().map(|s| f(&s.color()) as f64).sum::<f64>() / n).round() as u8
        };
        [m(|c| c.red), m(|c| c.green), m(|c| c.blue)]
    };
    match paint {
        usvg::Paint::Color(c) => ([c.red, c.green, c.blue], PaintKind::Solid),
        usvg::Paint::LinearGradient(g) => (stops_mean(g.stops()), PaintKind::Gradient),
        usvg::Paint::RadialGradient(g) => (stops_mean(g.stops()), PaintKind::Gradient),
        usvg::Paint::Pattern(_) => ([128; 3], PaintKind::Pattern),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_loads_at_the_asked_size() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50" viewBox="0 0 100 50">
            <rect x="10" y="10" width="20" height="20" fill="#ff0000"/>
            <circle cx="70" cy="25" r="10" fill="none" stroke="#0000ff" stroke-width="2"/></svg>"##;
        let art = load(svg, 200.0, 0.01).unwrap();
        assert_eq!(art.size_mm, [200.0, 100.0]);
        assert_eq!(art.items.len(), 2);
        assert_eq!(art.items[0].rgb, [255, 0, 0]);
        // 20 px at 2 mm/px is 40 mm a side.
        assert!((geom::area(&art.items[0].region) - 1600.0).abs() < 1.0);
        assert!(art.items[1].from_stroke);
        // A 4 mm wide ring of radius 20 mm: 2 pi r w.
        let ring = geom::area(&art.items[1].region);
        assert!(
            (ring - 2.0 * std::f64::consts::PI * 20.0 * 4.0).abs() < 5.0,
            "{ring}"
        );
    }
}
