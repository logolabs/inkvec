//! G-code for GRBL lasers and plotters: the cut paths as G1 lines and G2/G3 arcs.
//!
//! Most hobby diode lasers, and pen plotters driven by a servo on the spindle output, run
//! GRBL. Its laser mode (`$32=1`) turns the beam off during G0 rapids by itself and, under
//! M4, scales power with speed so corners do not burn. The arcs are the DXF's (see
//! [`crate::biarc`]): one G2/G3 per arc instead of a stutter of short G1 moves, which is
//! what keeps a controller's look-ahead full and the edge clean.
//!
//! The job's frame has its origin at the lower left of the material the plan measures
//! (marks and borders included), millimetres, y up: jog the head to the lower left of the
//! stock and zero it there. Within a sheet, contours go smallest first, so holes and inner
//! parts are cut before the piece around them can shift.

use crate::dxf::CamSheet;
use crate::geom::Pt;

/// What the machine is told.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Cutting speed, millimetres per minute.
    pub feed_mm_min: f64,
    /// Power, in the controller's S units (GRBL's default maximum `$30` is 1000).
    pub power: f64,
    /// Times each path is cut.
    pub passes: u32,
    /// Subtracted from every x so the frame's left edge is x = 0.
    pub x0: f64,
}

/// Largest sagitta written as a straight G1, millimetres.
const SAGITTA_AS_LINE_MM: f64 = 0.002;

fn shoelace(v: &[(Pt, f64)]) -> f64 {
    let n = v.len();
    (0..n)
        .map(|i| {
            let (a, b) = (v[i].0, v[(i + 1) % n].0);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        .abs()
        / 2.0
}

fn xy(p: Pt, x0: f64) -> String {
    format!("X{:.3} Y{:.3}", p[0] - x0, p[1])
}

/// The move from `a` to `b` along the edge `bulge` describes.
fn edge(a: Pt, b: Pt, bulge: f64, x0: f64) -> String {
    let (cx, cy) = (b[0] - a[0], b[1] - a[1]);
    let len = cx.hypot(cy);
    // An arc whose sagitta is below what the rounding can say is a line; sent as an arc,
    // its huge radius only invites the controller's radius check to disagree.
    if (bulge * len / 2.0).abs() < SAGITTA_AS_LINE_MM {
        return format!("G1 {}", xy(b, x0));
    }
    let theta = 4.0 * bulge.atan();
    let r = len / (2.0 * (theta / 2.0).sin());
    let h = r * (theta / 2.0).cos();
    let centre = [
        (a[0] + b[0]) / 2.0 - cy / len * h,
        (a[1] + b[1]) / 2.0 + cx / len * h,
    ];
    format!(
        "{} {} I{:.3} J{:.3}",
        if bulge > 0.0 { "G3" } else { "G2" },
        xy(b, x0),
        centre[0] - a[0],
        centre[1] - a[1]
    )
}

/// The G-code for already-fitted sheets, one after another.
pub fn write(cam: &[CamSheet], s: &Settings) -> String {
    let mut out = String::new();
    let mut line = |t: &str| {
        out.push_str(t);
        out.push('\n');
    };
    line("(Inkvec fabrication: millimetres, origin at the lower left of the job)");
    line("(GRBL laser mode $32=1 expected: the beam is off during G0 moves)");
    line("G21 G90 G17");
    line("M5");
    line(&format!("M4 S0 F{:.0}", s.feed_mm_min));
    for sheet in cam {
        line(&format!("(Sheet {})", sheet.name.replace(['(', ')'], "")));
        let mut closed: Vec<&Vec<(Pt, f64)>> = sheet.closed.iter().collect();
        closed.sort_by(|a, b| shoelace(a).total_cmp(&shoelace(b)));
        for _ in 0..s.passes.max(1) {
            for v in &closed {
                line(&format!("G0 {}", xy(v[0].0, s.x0)));
                line(&format!("S{:.0}", s.power));
                for (k, &(a, bulge)) in v.iter().enumerate() {
                    let b = v[(k + 1) % v.len()].0;
                    line(&edge(a, b, bulge, s.x0));
                }
                line("S0");
            }
            for l in &sheet.open {
                line(&format!("G0 {}", xy(l[0], s.x0)));
                line(&format!("S{:.0}", s.power));
                for &p in &l[1..] {
                    line(&format!("G1 {}", xy(p, s.x0)));
                }
                line("S0");
            }
        }
    }
    line("M5");
    line("G0 X0 Y0");
    line("M2");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dxf, geom};

    #[test]
    fn a_disc_is_cut_with_arcs_that_return_to_the_start() {
        let disc: geom::Region = vec![vec![(0..360)
            .map(|i| {
                let t = (i as f64).to_radians();
                [30.0 + 5.0 * t.cos(), 5.0 + 5.0 * t.sin()]
            })
            .collect()]];
        let cam = dxf::cam_sheets(&[("Cut".into(), &disc)], &[], 0.05, 10.0);
        let g = write(
            &cam,
            &Settings {
                feed_mm_min: 1200.0,
                power: 800.0,
                passes: 2,
                x0: 0.0,
            },
        );
        let is_arc = |l: &&str| l.starts_with("G2 ") || l.starts_with("G3 ");
        let arcs = g.lines().filter(is_arc).count();
        assert!(
            (8..=48).contains(&arcs),
            "{arcs} arcs over two passes
{g}"
        );
        assert_eq!(
            g.matches(
                "
S800
"
            )
            .count(),
            2,
            "one path, two passes"
        );
        // Every arc is one GRBL accepts, and ends on the disc (y flipped about 10: still 5).
        let mut at = [0.0, 0.0];
        for l in g.lines().filter(|l| l.starts_with('G')) {
            let v = |k: char| -> Option<f64> {
                l.split_whitespace()
                    .find(|w| w.starts_with(k))
                    .and_then(|w| w[1..].parse().ok())
            };
            if is_arc(&l) {
                // GRBL refuses an arc whose end is not on its circle (error 33): the
                // start and end radii must agree to 0.005 mm after rounding.
                let c = [at[0] + v('I').unwrap(), at[1] + v('J').unwrap()];
                let end = [v('X').unwrap(), v('Y').unwrap()];
                let (r0, r1) = (
                    (at[0] - c[0]).hypot(at[1] - c[1]),
                    (end[0] - c[0]).hypot(end[1] - c[1]),
                );
                assert!((r0 - r1).abs() < 0.005, "{l}: radii {r0} {r1}");
                assert!(
                    ((end[0] - 30.0).hypot(end[1] - 5.0) - 5.0).abs() < 0.06,
                    "{l}"
                );
            }
            if let (Some(x), Some(y)) = (v('X'), v('Y')) {
                at = [x, y];
            }
        }
        assert!(g.trim_end().ends_with("M2"));
    }
}
