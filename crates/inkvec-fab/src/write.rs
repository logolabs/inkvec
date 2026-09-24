//! Writing regions as SVG sized in millimetres.
//!
//! Boolean operations and offsets return polygons, and a polygon with a vertex every
//! twentieth of a millimetre is exactly the "too many nodes" file cutter software refuses
//! (Cricut Design Space rejects a path over 20 000 points and slows to a crawl well
//! before). So every contour is refitted — lines where it is straight, cubics where it
//! curves, within the tolerance — before it is written.

use crate::fitcurve::{self, Seg};
use crate::geom::{Contour, Region};

/// A contour refitted as lines and cubics within `tolerance` (see [`crate::fitcurve`]):
/// appends its `d` data and returns its segment count.
pub fn contour_d(c: &Contour, tolerance: f64, d: &mut String) -> usize {
    if c.len() < 3 {
        return 0;
    }
    let (start, segs) = fitcurve::fit_closed(c, tolerance.max(1e-4));
    if segs.is_empty() {
        return polyline_d(c, tolerance.max(1e-4), d);
    }
    let f = |v: f64| {
        let s = format!("{v:.3}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" {
            "0".to_string()
        } else {
            s.to_string()
        }
    };
    d.push_str(&format!("M{} {}", f(start[0]), f(start[1])));
    for s in &segs {
        match *s {
            Seg::Line(e) => d.push_str(&format!("L{} {}", f(e[0]), f(e[1]))),
            Seg::Cubic(a, b, e) => d.push_str(&format!(
                "C{} {} {} {} {} {}",
                f(a[0]),
                f(a[1]),
                f(b[0]),
                f(b[1]),
                f(e[0]),
                f(e[1])
            )),
        }
    }
    d.push('Z');
    segs.len()
}

/// A contour as a polyline, Douglas-Peucker simplified to `tol`.
fn polyline_d(c: &Contour, tol: f64, d: &mut String) -> usize {
    fn dp(c: &[[f64; 2]], tol: f64, keep: &mut Vec<bool>, lo: usize, hi: usize) {
        if hi <= lo + 1 {
            return;
        }
        let (a, b) = (c[lo], c[hi]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt().max(1e-12);
        let (mut worst, mut at) = (0.0, lo);
        for (i, p) in c.iter().enumerate().take(hi).skip(lo + 1) {
            let e = ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / len;
            if e > worst {
                worst = e;
                at = i;
            }
        }
        if worst > tol {
            keep[at] = true;
            dp(c, tol, keep, lo, at);
            dp(c, tol, keep, at, hi);
        }
    }
    let mut ring = c.clone();
    ring.push(c[0]);
    let mut keep = vec![false; ring.len()];
    let last = ring.len() - 1;
    keep[0] = true;
    keep[last] = true;
    dp(&ring, tol, &mut keep, 0, last);
    let mut n = 0;
    for (i, q) in ring.iter().enumerate().take(last) {
        if keep[i] {
            d.push_str(&format!(
                "{}{:.3} {:.3}",
                if n == 0 { 'M' } else { 'L' },
                q[0],
                q[1]
            ));
            n += 1;
        }
    }
    d.push('Z');
    n
}

/// A whole region as one even-odd path's `d`, and its segment count.
pub fn region_d(r: &Region, tolerance: f64) -> (String, usize) {
    let mut d = String::new();
    let mut n = 0;
    for c in r.iter().flatten() {
        n += contour_d(c, tolerance, &mut d);
    }
    (d, n)
}

/// An SVG document `size` millimetres, with its origin at `origin` in artwork millimetres.
pub fn document(size: [f64; 2], origin: [f64; 2], body: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.3}mm\" height=\"{h:.3}mm\" viewBox=\"{x:.3} {y:.3} {w:.3} {h:.3}\">{body}</svg>",
        w = size[0],
        h = size[1],
        x = origin[0],
        y = origin[1]
    )
}

/// A filled path element.
pub fn filled(d: &str, hex: &str) -> String {
    format!("<path d=\"{d}\" fill=\"{hex}\" fill-rule=\"evenodd\"/>")
}

/// A hairline cut element: unfilled, red, 0.025 mm (a thousandth of an inch).
pub fn hairline(d: &str) -> String {
    hairline_in(d, "#ff0000")
}

/// A hairline in a given colour, for programs where colour picks the operation.
pub fn hairline_in(d: &str, hex: &str) -> String {
    format!("<path d=\"{d}\" fill=\"none\" stroke=\"{hex}\" stroke-width=\"0.025\"/>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_densely_flattened_circle_comes_back_as_a_few_curves() {
        let c: Contour = (0..720)
            .map(|i| {
                let t = i as f64 / 720.0 * std::f64::consts::TAU;
                [50.0 + 20.0 * t.cos(), 50.0 + 20.0 * t.sin()]
            })
            .collect();
        let mut d = String::new();
        let n = contour_d(&c, 0.05, &mut d);
        assert!(n > 0 && n <= 8, "{n} segments: {d}");
    }
}
