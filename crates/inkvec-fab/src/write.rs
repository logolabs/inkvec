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

/// A whole region as one compound path's `d`, and its segment count.
///
/// Outlines and holes keep the opposite windings the geometry gives them, so the path fills
/// the same under nonzero and even-odd: some cutter software ignores `fill-rule` and uses
/// winding alone, and a hole that relies on even-odd comes back filled there.
pub fn region_d(r: &Region, tolerance: f64) -> (String, usize) {
    region_d_ordered(r, tolerance, false)
}

/// [`region_d`], optionally in cutting order: every hole first, then outlines smallest
/// first, so a part is never cut free before the openings inside it (it can drop or shift
/// once free, which is why laser software cuts inner shapes first).
pub fn region_d_ordered(r: &Region, tolerance: f64, inner_first: bool) -> (String, usize) {
    let mut contours: Vec<&Contour> = r.iter().flatten().collect();
    if inner_first {
        let key = |c: &&Contour| {
            let a = crate::geom::contour_area(c);
            // Holes wind the other way from outlines: negative area first, then by size.
            (a > 0.0, a.abs())
        };
        contours.sort_by(|x, y| {
            let (kx, ky) = (key(x), key(y));
            kx.0.cmp(&ky.0).then(kx.1.total_cmp(&ky.1))
        });
    }
    let mut d = String::new();
    let mut n = 0;
    for c in contours {
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

/// `svg` (one of [`document`]'s) with its size restated in `units`. The viewBox stays in
/// millimetres, so only the stated size changes and with it nothing a reader draws.
pub fn with_units(svg: &str, units: crate::options::FileUnits) -> String {
    let Some(ppi) = units.px_per_inch() else {
        return svg.to_string();
    };
    let mut out = svg.to_string();
    for key in ["width=\"", "height=\""] {
        let Some(at) = out.find(key) else { continue };
        let start = at + key.len();
        let Some(len) = out[start..].find("mm\"") else {
            continue;
        };
        let Ok(mm) = out[start..start + len].parse::<f64>() else {
            continue;
        };
        let px = format!("{:.3}", mm / 25.4 * ppi);
        out.replace_range(start..start + len + 2, &px);
    }
    out
}

/// A filled path element.
pub fn filled(d: &str, hex: &str) -> String {
    format!("<path d=\"{d}\" fill=\"{hex}\"/>")
}

/// A pen or tool line: unfilled, round-ended, `width` wide.
pub fn pen(d: &str, hex: &str, width: f64) -> String {
    format!(
        "<path d=\"{d}\" fill=\"none\" stroke=\"{hex}\" stroke-width=\"{width:.3}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"
    )
}

/// An open or closed polyline's `d`.
pub fn open_d(p: &[crate::geom::Pt], closed: bool) -> String {
    let mut d = String::new();
    for (i, q) in p.iter().enumerate() {
        d.push_str(&format!(
            "{}{:.3} {:.3}",
            if i == 0 { 'M' } else { 'L' },
            q[0],
            q[1]
        ));
    }
    if closed {
        d.push('Z');
    }
    d
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

    #[test]
    fn a_ring_winds_so_both_fill_rules_agree() {
        // An outline and its hole must wind opposite ways; then nonzero and even-odd agree.
        let ring = crate::geom::difference(
            &crate::geom::rect(0.0, 0.0, 20.0, 20.0),
            &crate::geom::rect(5.0, 5.0, 15.0, 15.0),
        );
        let signs: Vec<bool> = ring[0]
            .iter()
            .map(|c| crate::geom::contour_area(c) > 0.0)
            .collect();
        assert_eq!(signs.len(), 2);
        assert_ne!(signs[0], signs[1]);
        let (d, _) = region_d(&ring, 0.05);
        assert!(!filled(&d, "#000000").contains("evenodd"));
    }
}
