//! What will go wrong on the machine, said before it does.
//!
//! Every check is about a physical failure someone has had: a gradient cut as one flat
//! colour, a sliver too thin to weed, a speck that lifts with the transfer tape, a file
//! the cutter's software refuses. The findings are measurements, in millimetres, not
//! badges: the same honesty the Studio's quality report is built on.
//!
//! Called from [`crate::plan`]: colour findings once, layer findings per sheet, and node
//! findings per written sheet. Thin parts and narrow gaps are found by morphology: the
//! opening at the minimum feature width removes exactly the parts narrower than it, and the
//! closing fills exactly the gaps narrower than it, so the difference with the sheet is the
//! problem area.

use crate::geom::{self, Region};
use crate::options::{Check, Level};
use crate::regions::{hex, ColourRegion};

/// Cricut Design Space reports a file with more paths than this as too large. Cricut has
/// not published its limits; this is the only one attributed to it (research, 2026-09-24),
/// so the finding says "reported", not "will".
pub const DESIGN_SPACE_MAX_PATHS: usize = 5_000;
/// Above this many segments in one sheet, cutter software becomes slow to edit.
pub const COMFORTABLE_NODES: usize = 2_000;

/// A finding, built in one line.
fn check(level: Level, code: &'static str, message: String) -> Check {
    Check {
        level,
        code,
        message,
    }
}

/// Findings about the colours chosen, before any layer is built.
pub fn colour_checks(chosen: &[&ColourRegion], unsupported: &[String]) -> Vec<Check> {
    let mut out = Vec::new();
    for u in unsupported {
        out.push(check(
            Level::Error,
            "unsupported",
            format!("The file contains {u}, which has no cut line and is left out."),
        ));
    }
    if chosen.is_empty() {
        out.push(check(
            Level::Error,
            "empty",
            "No colour is selected, so there is nothing to cut.".into(),
        ));
    }
    for c in chosen {
        if c.gradient {
            out.push(check(
                Level::Warn,
                "gradient",
                format!(
                    "{} is a gradient. Vinyl is one colour, so it is cut as its average colour.",
                    hex(c.rgb)
                ),
            ));
        }
        if c.translucent {
            out.push(check(
                Level::Warn,
                "translucent",
                format!(
                    "{} is partly transparent. Vinyl is opaque, so it is cut at full strength.",
                    hex(c.rgb)
                ),
            ));
        }
    }
    out
}

/// Findings about one finished sheet: thin parts and specks judged on `r`, what shows of
/// it; waste gaps on `sheet`, what is actually cut.
pub fn layer_checks(
    name: &str,
    r: &Region,
    sheet: &Region,
    min_feature: f64,
    removed: bool,
) -> Vec<Check> {
    let mut out = Vec::new();
    let total = geom::area(r);
    if total <= 0.0 {
        return out;
    }
    // Narrow parts: whatever the morphological opening at the minimum feature removes.
    if !removed {
        let thin = geom::difference(r, &geom::opening(r, min_feature));
        let thin_area = geom::area(&thin);
        let pieces = thin
            .iter()
            .filter(|s| geom::shape_area(s) > 0.02 * min_feature * min_feature)
            .count();
        if thin_area > (0.002 * total).max(0.05) && pieces > 0 {
            out.push(check(
                Level::Warn,
                "thin",
                format!(
                    "{name}: {thin_area:.1} mm² in {pieces} place(s) is narrower than {min_feature} mm and may tear or not stay stuck. Make the design larger or turn on thin-part removal."
                ),
            ));
        }
    }
    // Narrow gaps: waste between two cuts closer than the minimum feature, which lifts with
    // the parts when weeding. What the morphological closing fills in.
    let gaps = geom::difference(&geom::closing(sheet, min_feature), sheet);
    let gap_area = geom::area(&gaps);
    let gap_pieces = gaps
        .iter()
        .filter(|s| geom::shape_area(s) > 0.02 * min_feature * min_feature)
        .count();
    if gap_area > (0.002 * total).max(0.05) && gap_pieces > 0 {
        out.push(check(
            Level::Warn,
            "gaps",
            format!(
                "{name}: {gap_area:.1} mm² of waste in {gap_pieces} place(s) is narrower than {min_feature} mm between cuts and will lift with the design when weeding."
            ),
        ));
    }
    // Specks: pieces too small to weed or to stay on the transfer tape.
    let speck = 4.0 * min_feature * min_feature;
    let specks = r.iter().filter(|s| geom::shape_area(s) < speck).count();
    if specks > 0 {
        out.push(check(
            Level::Warn,
            "specks",
            format!(
                "{name}: {specks} piece(s) are smaller than {speck:.1} mm² and are easily lost when weeding."
            ),
        ));
    }
    out
}

/// Findings about the size of what was written: `paths` closed contours in `nodes` segments.
pub fn node_checks(name: &str, paths: usize, nodes: usize) -> Vec<Check> {
    let mut out = Vec::new();
    if paths > DESIGN_SPACE_MAX_PATHS {
        out.push(check(
            Level::Error,
            "paths",
            format!(
                "{name} has {paths} separate cut paths. Cricut Design Space is reported to refuse files with more than {DESIGN_SPACE_MAX_PATHS}; turn on thin-part removal or raise the minimum feature."
            ),
        ));
    }
    if nodes > COMFORTABLE_NODES {
        out.push(check(
            Level::Warn,
            "nodes",
            format!(
                "{name} has {nodes} path segments; cutter software gets slow to edit files this detailed. A larger tolerance lowers it."
            ),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_colour_checks() {
        let unsupported = vec!["clipPath".to_string()];
        let checks = colour_checks(&[], &unsupported);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].code, "unsupported");
        assert_eq!(checks[1].code, "empty");

        let cr1 = ColourRegion {
            rgb: [255, 0, 0],
            region: Region::new(),
            area_mm2: 100.0,
            gradient: true,
            translucent: true,
            background: false,
            strokes: vec![],
        };
        let checks2 = colour_checks(&[&cr1], &[]);
        assert_eq!(checks2.len(), 2);
        assert_eq!(checks2[0].code, "gradient");
        assert_eq!(checks2[1].code, "translucent");
    }

    #[test]
    fn test_node_checks() {
        let checks = node_checks("test", DESIGN_SPACE_MAX_PATHS + 10, COMFORTABLE_NODES + 10);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].code, "paths");
        assert_eq!(checks[1].code, "nodes");

        let ok_checks = node_checks("test", 10, 100);
        assert!(ok_checks.is_empty());
    }

    #[test]
    fn test_layer_checks() {
        let empty_reg = Region::new();
        assert!(layer_checks("test", &empty_reg, &empty_reg, 1.0, false).is_empty());

        let poly = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let shape = vec![poly];
        let r = vec![shape];
        let checks = layer_checks("layer1", &r, &r, 0.5, false);
        assert!(checks.is_empty());

        let tiny_poly = vec![[0.0, 0.0], [0.1, 0.0], [0.1, 0.1], [0.0, 0.1]];
        let tiny_r = vec![vec![tiny_poly]];
        let speck_checks = layer_checks("specks", &tiny_r, &tiny_r, 2.0, false);
        assert!(speck_checks.iter().any(|c| c.code == "specks"));
    }
}
