//! DXF for CAD and CAM: every sheet a layer, every contour a closed polyline, millimetres.
//!
//! R12 (AC1009) because it is the one version every laser, router and CAD program still
//! reads; later versions need handles and object tables that small CAM tools get wrong.
//! Contours are the refitted curves (see [`crate::fitcurve`]) written as lines and
//! circular arcs: each cubic becomes a biarc within the tolerance ([`crate::biarc`]), carried
//! as the bulge (group 42) of the vertex it starts at, which CAM programs turn into G2/G3
//! moves. CAM programs handle SPLINE entities badly, and R12 has none. DXF's y axis points
//! up, so the drawing is flipped to stand the right way round before it is fitted.

use crate::biarc;
use crate::fitcurve::{self, Seg};
use crate::geom::{Pt, Region};

/// A closed contour, already in the file's y-up frame, refitted and written as vertices
/// with the bulge of the edge each one starts.
fn contour_vertices(c: &[Pt], tolerance: f64) -> Vec<(Pt, f64)> {
    let tol = tolerance.max(1e-4);
    let (start, segs) = fitcurve::fit_closed(c, tol);
    if segs.is_empty() {
        return c.iter().map(|&p| (p, 0.0)).collect();
    }
    let mut out = Vec::new();
    let mut at = start;
    for s in &segs {
        match *s {
            Seg::Line(e) => {
                out.push((at, 0.0));
                at = e;
            }
            Seg::Cubic(a, b, e) => {
                // Half the tolerance for the arcs, half already spent by the refit.
                out.extend(biarc::cubic(at, a, b, e, tol / 2.0));
                at = e;
            }
        }
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
            let flipped: Vec<Pt> = c.iter().map(|p| [p[0], top - p[1]]).collect();
            let pts = contour_vertices(&flipped, tolerance);
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
            for (p, bulge) in pts {
                put(0, "VERTEX");
                put(8, &layer);
                put(10, &format!("{:.4}", p[0]));
                put(20, &format!("{:.4}", p[1]));
                put(30, "0.0");
                if bulge != 0.0 {
                    put(42, &format!("{bulge:.6}"));
                }
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
        // The square is four straight vertices; the disc is a handful of arcs.
        let vertices = d.matches("\nVERTEX\n").count();
        assert!((4 + 4..=4 + 24).contains(&vertices), "{vertices}");
        // Read the disc back: every arc, sampled, lies on the circle.
        let lines: Vec<&str> = d.lines().collect();
        let mut verts: Vec<(Pt, f64)> = Vec::new();
        let (mut layer, mut in_vertex) = ("", false);
        for pair in lines.chunks_exact(2) {
            let (code, value) = (pair[0].trim(), pair[1]);
            let disc = layer.contains("ff0000") && in_vertex;
            match code {
                "0" => {
                    in_vertex = value == "VERTEX";
                    if in_vertex {
                        verts.push(([0.0, 0.0], 0.0));
                    }
                }
                "8" => {
                    layer = value;
                    if !layer.contains("ff0000") && in_vertex {
                        verts.pop();
                        in_vertex = false;
                    }
                }
                "10" if disc => verts.last_mut().unwrap().0[0] = value.parse().unwrap(),
                "20" if disc => verts.last_mut().unwrap().0[1] = value.parse().unwrap(),
                "42" if disc => verts.last_mut().unwrap().1 = value.parse().unwrap(),
                _ => {}
            }
        }
        assert!(
            verts.len() >= 4 && verts.iter().all(|v| v.1 != 0.0),
            "{verts:?}"
        );
        for i in 0..verts.len() {
            let (a, b) = (verts[i], verts[(i + 1) % verts.len()].0);
            for s in [0.25, 0.5, 0.75] {
                let p = crate::biarc::arc_at(a.0, b, a.1, s);
                let r = (p[0] - 30.0).hypot(p[1] - 5.0);
                assert!((r - 5.0).abs() < 0.06, "{p:?} is {r} from the centre");
            }
        }
    }
}
