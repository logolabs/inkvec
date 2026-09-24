//! DXF for CAD and CAM: every sheet a layer, every contour a closed polyline, millimetres.
//!
//! R12 (AC1009) because it is the one version every laser, router and CAD program still
//! reads; later versions need handles and object tables that small CAM tools get wrong.
//! Contours are the refitted curves (see [`crate::fitcurve`]) subdivided into short chords:
//! CAM programs handle SPLINE entities badly, and R12 has none. DXF's y axis points up, so
//! the drawing is flipped to stand the right way round.
//!
//! Arcs (LWPOLYLINE bulges from a biarc fit, which CAM turns into G2/G3 moves) are the
//! better encoding and the next step; a dense polyline is correct everywhere today.

use crate::fitcurve::{self, Seg};
use crate::geom::{Pt, Region};

/// Longest chord a cubic is cut into, millimetres. On a 5 mm radius that strays 0.006 mm.
const CHORD_MM: f64 = 0.5;

fn cubic_points(p0: Pt, c1: Pt, c2: Pt, p3: Pt, out: &mut Vec<Pt>) {
    let poly = |a: Pt, b: Pt| (b[0] - a[0]).hypot(b[1] - a[1]);
    let len = poly(p0, c1) + poly(c1, c2) + poly(c2, p3);
    let n = ((len / CHORD_MM).ceil() as usize).clamp(2, 256);
    for k in 1..=n {
        let t = k as f64 / n as f64;
        let u = 1.0 - t;
        let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push([
            b0 * p0[0] + b1 * c1[0] + b2 * c2[0] + b3 * p3[0],
            b0 * p0[1] + b1 * c1[1] + b2 * c2[1] + b3 * p3[1],
        ]);
    }
}

/// A contour refitted and walked into points.
fn contour_points(c: &[Pt], tolerance: f64) -> Vec<Pt> {
    let (start, segs) = fitcurve::fit_closed(c, tolerance.max(1e-4));
    if segs.is_empty() {
        return c.to_vec();
    }
    let mut out = vec![start];
    let mut at = start;
    for s in &segs {
        match *s {
            Seg::Line(e) => out.push(e),
            Seg::Cubic(a, b, e) => cubic_points(at, a, b, e, &mut out),
        }
        at = *out.last().unwrap_or(&start);
    }
    // Closed by the flag; the repeated start is dropped.
    if out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

fn layer_name(name: &str, i: usize) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("SHEET-{}-{}", i + 1, clean.trim_matches('-'))
}

/// One DXF holding `sheets` (name and region each) as layers, with y flipped about `top`.
pub fn document(sheets: &[(String, &Region)], tolerance: f64, top: f64) -> String {
    document_with_lines(sheets, &[], tolerance, top)
}

/// [`document`], with open polylines too: `lines[i]` are drawn on sheet `i`'s layer.
pub fn document_with_lines(
    sheets: &[(String, &Region)],
    lines: &[Vec<Vec<Pt>>],
    tolerance: f64,
    top: f64,
) -> String {
    let mut s = String::new();
    let mut put = |code: i32, value: &str| {
        s.push_str(&format!("{code}\n{value}\n"));
    };
    put(0, "SECTION");
    put(2, "HEADER");
    put(9, "$ACADVER");
    put(1, "AC1009");
    // Millimetres, for the readers that look; the rest are told in the file name and docs.
    put(9, "$INSUNITS");
    put(70, "4");
    put(0, "ENDSEC");
    put(0, "SECTION");
    put(2, "TABLES");
    put(0, "TABLE");
    put(2, "LAYER");
    put(70, &sheets.len().to_string());
    for (i, (name, _)) in sheets.iter().enumerate() {
        put(0, "LAYER");
        put(2, &layer_name(name, i));
        put(70, "0");
        // AutoCAD colour index 1..7 (red, yellow, green, cyan, blue, magenta, white).
        put(62, &((i % 7) + 1).to_string());
        put(6, "CONTINUOUS");
    }
    put(0, "ENDTAB");
    put(0, "ENDSEC");
    put(0, "SECTION");
    put(2, "ENTITIES");
    for (i, (name, region)) in sheets.iter().enumerate() {
        let layer = layer_name(name, i);
        for c in region.iter().flatten() {
            let pts = contour_points(c, tolerance);
            if pts.len() < 2 {
                continue;
            }
            put(0, "POLYLINE");
            put(8, &layer);
            put(66, "1");
            put(70, "1");
            put(10, "0.0");
            put(20, "0.0");
            put(30, "0.0");
            for p in pts {
                put(0, "VERTEX");
                put(8, &layer);
                put(10, &format!("{:.4}", p[0]));
                put(20, &format!("{:.4}", top - p[1]));
                put(30, "0.0");
            }
            put(0, "SEQEND");
            put(8, &layer);
        }
        for line in lines.get(i).map(Vec::as_slice).unwrap_or(&[]) {
            if line.len() < 2 {
                continue;
            }
            put(0, "POLYLINE");
            put(8, &layer);
            put(66, "1");
            put(70, "0");
            put(10, "0.0");
            put(20, "0.0");
            put(30, "0.0");
            for p in line {
                put(0, "VERTEX");
                put(8, &layer);
                put(10, &format!("{:.4}", p[0]));
                put(20, &format!("{:.4}", top - p[1]));
                put(30, "0.0");
            }
            put(0, "SEQEND");
            put(8, &layer);
        }
    }
    put(0, "ENDSEC");
    put(0, "EOF");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom;

    #[test]
    fn a_square_and_a_disc_are_closed_polylines_on_their_layers() {
        let square = geom::rect(0.0, 0.0, 10.0, 10.0);
        let disc: Region = vec![vec![(0..360)
            .map(|i| {
                let t = (i as f64).to_radians();
                [30.0 + 5.0 * t.cos(), 5.0 + 5.0 * t.sin()]
            })
            .collect()]];
        let d = document(
            &[("Cut".into(), &square), ("#ff0000".into(), &disc)],
            0.05,
            10.0,
        );
        assert!(d.starts_with("0\nSECTION\n") && d.ends_with("0\nEOF\n"));
        assert_eq!(d.matches("\nPOLYLINE\n").count(), 2);
        assert!(d.contains("SHEET-1-Cut") && d.contains("SHEET-2-ff0000"));
        // The square is four vertices; the disc is walked into chords no longer than 0.5 mm.
        let vertices = d.matches("\nVERTEX\n").count();
        assert!(vertices > 4 + 40 && vertices < 4 + 200, "{vertices}");
    }
}
