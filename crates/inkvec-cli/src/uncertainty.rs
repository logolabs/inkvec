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

use inkvec_core::Point;
use inkvec_trace::planar::Edge;

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

/// The overlay for `edges`, sharing `trace`'s root element (and so its size and viewBox).
pub(crate) fn bands_svg(trace: &str, edges: &[Edge], k: f64) -> String {
    let root = trace
        .find("<svg")
        .and_then(|a| trace[a..].find('>').map(|b| &trace[a..=a + b]))
        .unwrap_or("<svg xmlns=\"http://www.w3.org/2000/svg\">");
    let mut out = String::from(root);
    out.push_str(&format!(
        "<g fill=\"#e5484d\" fill-opacity=\"0.55\" fill-rule=\"evenodd\" data-k=\"{}\">",
        fmt(k)
    ));
    for e in edges.iter().filter(|e| e.points.len() >= 2) {
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
            2.0,
        );
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\">"));
        // Normals point to +y for a boundary running +x: 5 +- 0.2 at the start, +- 0.6 at the end.
        assert!(svg.contains("M0 5.2L10 5.6L10 4.4L0 4.8Z"), "{svg}");
        assert!(svg.contains("data-sigma-mean=\"0.2\"") && svg.contains("data-sigma-max=\"0.3\""));
    }
}
