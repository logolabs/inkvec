//! What will go wrong on the machine, said before it does.
//!
//! Every check is about a physical failure someone has had: a gradient cut as one flat
//! colour, a sliver too thin to weed, a speck that lifts with the transfer tape, a file
//! the cutter's software refuses. The findings are measurements, in millimetres, not
//! badges: the same honesty the Studio's quality report is built on.

use crate::geom::{self, Region};
use crate::options::{Check, Level};
use crate::regions::{hex, ColourRegion};

/// Cricut Design Space refuses a single path with more points than this.
pub const DESIGN_SPACE_MAX_PATH_POINTS: usize = 20_000;
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

/// Findings about one finished sheet.
pub fn layer_checks(name: &str, r: &Region, min_feature: f64, removed: bool) -> Vec<Check> {
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

/// Findings about the size of what was written.
pub fn node_checks(name: &str, nodes: usize) -> Vec<Check> {
    let mut out = Vec::new();
    if nodes > DESIGN_SPACE_MAX_PATH_POINTS {
        out.push(check(
            Level::Error,
            "nodes",
            format!(
                "{name} has {nodes} path segments, more than the {DESIGN_SPACE_MAX_PATH_POINTS} Cricut Design Space accepts. Raise the tolerance."
            ),
        ));
    } else if nodes > COMFORTABLE_NODES {
        out.push(check(
            Level::Warn,
            "nodes",
            format!(
                "{name} has {nodes} path segments; cutter software will be slow to edit it. A larger tolerance lowers this."
            ),
        ));
    }
    out
}
