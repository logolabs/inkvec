//! What will go wrong on the machine, said before it does.
//!
//! Every check is about a physical failure someone has had: a gradient cut as one flat
//! colour, a sliver too thin to weed, a speck that lifts with the transfer tape, a file
//! the cutter's software refuses. The findings are measurements, in millimetres, not
//! badges: the same honesty the Studio's quality report is built on.

use crate::geom::{self, Region};
use crate::options::{Check, Level};
use crate::regions::{hex, ColourRegion};

/// Cricut Design Space reports a file with more paths than this as too large. Cricut has
/// not published its limits; this is the only one attributed to it (research, 2026-09-24),
/// so the finding says "reported", not "will".
pub const DESIGN_SPACE_MAX_PATHS: usize = 5_000;
/// Above this many segments in one sheet, cutter software becomes slow to edit.
pub const COMFORTABLE_NODES: usize = 2_000;

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
