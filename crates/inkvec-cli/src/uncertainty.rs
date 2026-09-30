//! Confidence bands: where each traced boundary could be, drawn as an overlay.
//!
//! Every boundary point the tracer places comes with its own positional uncertainty,
//! measured from the pixels: how sharply the coverage crosses its level there, against
//! the noise (see `inkvec_trace::planar::Edge::sigma`). It decides everything downstream,
//! from how many curve parameters a boundary may spend to when a corner is real, and it is
//! otherwise thrown away at the end. This writes it out: each boundary as a band `k` sigma
//! either side of it, a closed boundary as a ring, in a document that shares the trace's
//! own root element so the two overlay exactly. At the default `k = 2` the band holds the
//! true edge about 95% of the time where the noise model holds.
//!
//! Where a band is thin the trace is certain; where it swells (a soft or low-contrast edge,
//! a blurred or compressed source) the curve there is a best guess, and the band says by
//! how much. Each band carries its mean and largest sigma, in pixels, as data attributes.
//!
//! Where it sits: a side output of the colour pipeline, not a stage of the trace. When
//! `--uncertainty <file>` is given, `pipeline.rs` calls [`bands_svg`] right after the
//! emitter with the emitted document and the planar map's edges, and writes the result to
//! that file; nothing here feeds back into the trace. In: the map's edges, whose points
//! are in pixel coordinates with pixel centres at integers (the canvas spans
//! `-0.5 .. w - 0.5`, the same frame as the emitter's `viewBox="-0.5 -0.5 w h"`), and a
//! per-point sigma in pixels. Out: a standalone SVG document.

use inkvec_core::Point;
use inkvec_trace::planar::Edge;

/// A number as short SVG text: three decimals, trailing zeros and a bare point dropped, and
/// `-0` written as `0`. Three decimals is a thousandth of a pixel, well below anything the
/// band's own width can resolve.
fn fmt(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// Unit normal at point `i` of a polyline, from its neighbours.
///
/// The tangent is the central difference `d = p[i+1] − p[i−1]` (wrapping round on a closed
/// polyline; one-sided at the two ends of an open one, where the missing neighbour is
/// replaced by `p[i]` itself), and the normal is that turned a quarter turn:
/// `n = (−d_y, d_x) / |d|`. In image coordinates, where y points down, a boundary running
/// towards +x gets a normal towards +y. Using the neighbours rather than one adjacent
/// segment keeps the normal symmetric about the point, so the band does not lean at a
/// bend. When the two neighbours coincide (`|d| < 1e-12`, e.g. a duplicated point or a
/// one-point polyline) there is no direction to take and the normal is `(0, 0)`: the band
/// pinches to the curve there rather than pointing anywhere arbitrary.
///
/// `p` must not be empty.
fn normal(p: &[Point], i: usize, closed: bool) -> (f64, f64) {
    let n = p.len();
    let (a, b) = if closed {
        (p[(i + n - 1) % n], p[(i + 1) % n])
    } else {
        (p[i.saturating_sub(1)], p[(i + 1).min(n - 1)])
    };
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = dx.hypot(dy);
    if len < 1e-12 {
        (0.0, 0.0)
    } else {
        (-dy / len, dx / len)
    }
}

/// One closed polygon as SVG path data: `M` to the first point, `L` to each of the rest,
/// then `Z`. An empty iterator gives a bare `Z`.
fn ring(pts: impl Iterator<Item = (f64, f64)>) -> String {
    let mut d = String::new();
    for (k, (x, y)) in pts.enumerate() {
        d.push(if k == 0 { 'M' } else { 'L' });
        d.push_str(&fmt(x));
        d.push(' ');
        d.push_str(&fmt(y));
    }
    d.push('Z');
    d
}

/// The band of one boundary, `k` sigma either side, as path data.
///
/// Each traced point is pushed out along its normal ([`normal`]) by `k` times its own
/// sigma, once to each side: `p_i ± k σ_i n_i`, where `p_i` is the point (px), `σ_i` its
/// positional standard deviation (px), `n_i` the unit normal and `k` the band's half-width
/// in sigmas. The two offset polylines are the band's edges. On an open boundary they are
/// joined into one polygon, out along the `+` side and back along the `−` side. On a closed
/// one they are two separate rings, and the even-odd fill of the enclosing group paints only
/// the annulus between them.
///
/// A point with no sigma (a `sigma` shorter than `points`) is treated as certain, width 0.
/// Where the band is wider than the boundary's radius of curvature, the inner offset curve
/// crosses itself, as offset curves do; under even-odd fill the doubly covered bits then
/// read as gaps. That only happens where the band is already telling the reader the edge is
/// very uncertain. `e.points` must not be empty.
fn band(e: &Edge, k: f64) -> String {
    let p = &e.points;
    let side = |i: usize, s: f64| {
        let (nx, ny) = normal(p, i, e.closed);
        let w = s * k * e.sigma.get(i).copied().unwrap_or(0.0);
        (p[i].x + nx * w, p[i].y + ny * w)
    };
    if e.closed {
        // Two rings, filled even-odd: the band between them.
        format!(
            "{}{}",
            ring((0..p.len()).map(|i| side(i, 1.0))),
            ring((0..p.len()).rev().map(|i| side(i, -1.0)))
        )
    } else {
        ring(
            (0..p.len())
                .map(|i| side(i, 1.0))
                .chain((0..p.len()).rev().map(|i| side(i, -1.0))),
        )
    }
}

/// Whether every point of `e` lies on the border of a `w` by `h` raster: the canvas's own
/// edge, which the map carries but nobody drew.
///
/// The canvas edges are the lines `x = −0.5`, `y = −0.5`, `x = w − 0.5` and `y = h − 0.5`
/// (pixel centres at integers). A point counts as on the border when it is within 0.51 px
/// of any of the four, so a boundary that runs along one side and turns a corner onto the
/// next still qualifies; a single point anywhere inside the canvas disqualifies it.
fn on_border(e: &Edge, w: usize, h: usize) -> bool {
    let (x1, y1) = (w as f64 - 0.5, h as f64 - 0.5);
    let near = |a: f64, b: f64| (a - b).abs() < 0.51;
    e.points
        .iter()
        .all(|p| near(p.x, -0.5) || near(p.y, -0.5) || near(p.x, x1) || near(p.y, y1))
}

/// The overlay for the edges of a `w` by `h` map, sharing `trace`'s root element (and so
/// its size and viewBox).
///
/// The root is the first `<svg ...>` tag of `trace`, copied byte for byte; a `trace` with no
/// such tag falls back to a bare root with no size. Inside it is one translucent red group,
/// filled even-odd and tagged with `data-k`, holding one `<path>` per edge: every edge with
/// at least two points except those that only trace the canvas border ([`on_border`]). Each
/// path carries its edge's mean and largest sigma, in px, as `data-sigma-mean` and
/// `data-sigma-max`; an edge with an empty `sigma` reads 0 for both.
///
/// `trace` is the document as the emitter wrote it, before `lib.rs` retargets its size or
/// `post_process` adds a margin, so the overlay matches the trace's coordinate frame; with
/// `--margin` or a resized presentation the written trace's viewBox or size can differ
/// from the overlay's. `k` is the band half-width in sigmas (`--uncertainty-k`).
pub(crate) fn bands_svg(trace: &str, edges: &[Edge], w: usize, h: usize, k: f64) -> String {
    let root = trace
        .find("<svg")
        .and_then(|a| trace[a..].find('>').map(|b| &trace[a..=a + b]))
        .unwrap_or("<svg xmlns=\"http://www.w3.org/2000/svg\">");
    let mut out = String::from(root);
    out.push_str(&format!(
        "<g fill=\"#e5484d\" fill-opacity=\"0.55\" fill-rule=\"evenodd\" data-k=\"{}\">",
        fmt(k)
    ));
    for e in edges
        .iter()
        .filter(|e| e.points.len() >= 2 && !on_border(e, w, h))
    {
        let n = e.sigma.len().max(1) as f64;
        let mean = e.sigma.iter().sum::<f64>() / n;
        let max = e.sigma.iter().copied().fold(0.0, f64::max);
        out.push_str(&format!(
            "<path d=\"{}\" data-sigma-mean=\"{}\" data-sigma-max=\"{}\"/>",
            band(e, k),
            fmt(mean),
            fmt(max)
        ));
    }
    out.push_str("</g></svg>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(points: Vec<Point>, sigma: Vec<f64>, closed: bool) -> Edge {
        Edge {
            points,
            sigma,
            left: 0,
            right: 1,
            start_node: 0,
            end_node: 0,
            closed,
            lambda_scale: 1.0,
        }
    }

    #[test]
    fn a_straight_boundary_gets_a_band_k_sigma_either_side() {
        let e = edge(
            vec![Point::new(0.0, 5.0), Point::new(10.0, 5.0)],
            vec![0.1, 0.3],
            false,
        );
        let svg = bands_svg(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\"><path/></svg>",
            &[e],
            100,
            100,
            2.0,
        );
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\">"));
        // Normals point to +y for a boundary running +x: 5 +- 0.2 at the start, +- 0.6 at the end.
        assert!(svg.contains("M0 5.2L10 5.6L10 4.4L0 4.8Z"), "{svg}");
        assert!(svg.contains("data-sigma-mean=\"0.2\"") && svg.contains("data-sigma-max=\"0.3\""));
    }
}
