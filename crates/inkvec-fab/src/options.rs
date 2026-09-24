//! What a caller asks for, and what comes back. Serialised in camelCase for the Studio.

use serde::{Deserialize, Serialize};

/// What is being made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// One sheet of one vinyl: everything chosen, joined, cut as one layer.
    #[default]
    SingleColour,
    /// One sheet per colour, stacked: each lower layer runs under the ones above by
    /// the bleed, so the stack has no gaps where the colours meet.
    Layered,
    /// Side by side, no stacking: each colour cut exactly to its visible shape, to be
    /// laid edge to edge (inlay). Thinner result, unforgiving alignment.
    Inlay,
    /// A die-cut sticker: the artwork plus one contour around it, offset by the margin.
    Sticker,
    /// A stencil: a sheet with the artwork cut out, every loose counter held by a bridge.
    Stencil,
    /// For a pen, a scoring blade or a laser line: every line-shaped part drawn once along
    /// its centre, everything else round its outline.
    Lines,
}

/// How a saved file states its size.
///
/// A millimetre size is exact in the SVG specification, but the commonest complaint about
/// cutter files is that they import at the wrong size: programs that read a bare pixel
/// size turn it into inches at their own rate, 72 per inch in Cricut Design Space and
/// Silhouette Studio, 96 in Inkscape, LightBurn and browsers. A file whose size is written
/// in the pixels its program expects imports at the size it was drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FileUnits {
    /// `width="100mm"`: exact for every reader that follows the specification.
    #[default]
    Mm,
    /// Bare pixels at 96 per inch (the CSS pixel).
    Px96,
    /// Bare pixels at 72 per inch.
    Px72,
}

impl FileUnits {
    /// Pixels per inch, when the size is written in pixels.
    pub fn px_per_inch(self) -> Option<f64> {
        match self {
            FileUnits::Mm => None,
            FileUnits::Px96 => Some(96.0),
            FileUnits::Px72 => Some(72.0),
        }
    }
}

/// How a cut line is drawn in the output file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CutStyle {
    /// Filled shapes in the layer's colour: what Cricut Design Space and Silhouette
    /// Studio expect, and they cut the outline of every shape.
    #[default]
    Filled,
    /// Unfilled red hairlines (0.025 mm): the laser and sign-cutter convention, where
    /// only a hairline stroke is a cut.
    Hairline,
}

/// A fabrication request. Lengths in millimetres.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Options {
    /// What is being made.
    pub mode: Mode,
    /// Width of the artwork's canvas on the material.
    pub width_mm: f64,
    /// Colours (`#rrggbb`) to cut. Empty: every colour except the background.
    pub include: Vec<String>,
    /// Layer order for [`Mode::Layered`], bottom first. Empty: by area, largest at the
    /// bottom, which is how the colours of a traced logo stack in nearly every case.
    pub order: Vec<String>,
    /// How far a lower layer runs under the layers above it.
    pub bleed_mm: f64,
    /// Narrowest part that can be weeded and will stay stuck.
    pub min_feature_mm: f64,
    /// Drop parts and necks narrower than `min_feature_mm` from the output.
    pub remove_thin: bool,
    /// Sticker contour distance from the artwork.
    pub sticker_margin_mm: f64,
    /// Width of a stencil's bridges.
    pub bridge_mm: f64,
    /// Width of the stencil sheet's frame around the artwork.
    pub stencil_margin_mm: f64,
    /// Widest part [`Mode::Lines`] draws as a single line rather than an outline; 0 takes
    /// every part the analysis reads as a drawn line, whatever its width.
    pub max_line_mm: f64,
    /// Width of the pen or tool line in [`Mode::Lines`] output.
    pub pen_mm: f64,
    /// Registration marks on every layer, outside the artwork.
    pub registration: bool,
    /// A weeding border this far outside the artwork; zero for none.
    pub weed_border_mm: f64,
    /// Mirror everything: heat-transfer vinyl is cut from the back.
    pub mirror: bool,
    /// How cut lines are drawn.
    pub cut_style: CutStyle,
    /// Radius of a router bit: every inside corner gets a dogbone of it, so parts cut with
    /// that bit seat in each other; 0 for none.
    pub dogbone_mm: f64,
    /// G-code cutting speed, millimetres per minute.
    pub gcode_feed_mm_min: f64,
    /// G-code power, in the controller's S units.
    pub gcode_power: f64,
    /// Times the G-code cuts each path.
    pub gcode_passes: u32,
    /// Side of a square cut outside the design on every sheet, to measure after cutting
    /// and so catch a program that changed the size on import; 0 for none.
    pub size_check_mm: f64,
    /// How the saved files state their size.
    pub file_units: FileUnits,
    /// Width of material the cut itself removes (a laser's kerf, a knife's does not
    /// count). Every kept piece grows by half of it, which is the inside/outside rule:
    /// outlines move out, holes move in, so the parts come out the drawn size.
    pub kerf_mm: f64,
    /// Largest distance a cut may stray from the artwork's own curves.
    pub tolerance_mm: f64,
    /// Colours closer than this (CIE76) are one vinyl.
    pub merge_delta_e: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::SingleColour,
            width_mm: 100.0,
            include: Vec::new(),
            order: Vec::new(),
            bleed_mm: 0.8,
            min_feature_mm: 0.8,
            remove_thin: false,
            sticker_margin_mm: 3.0,
            bridge_mm: 1.5,
            stencil_margin_mm: 10.0,
            max_line_mm: 0.0,
            pen_mm: 0.4,
            registration: true,
            weed_border_mm: 0.0,
            mirror: false,
            cut_style: CutStyle::Filled,
            dogbone_mm: 0.0,
            gcode_feed_mm_min: 1000.0,
            gcode_power: 1000.0,
            gcode_passes: 1,
            size_check_mm: 0.0,
            file_units: FileUnits::Mm,
            kerf_mm: 0.0,
            tolerance_mm: 0.05,
            merge_delta_e: 3.0,
        }
    }
}

/// One colour found in the artwork.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColourInfo {
    /// `#rrggbb`.
    pub hex: String,
    /// Share of the canvas it covers.
    pub coverage: f64,
    /// True when it is the page rather than the artwork.
    pub background: bool,
    /// True when some of it was a gradient.
    pub gradient: bool,
    /// True when some of it was translucent.
    pub translucent: bool,
}

/// What is in a file, before anything is asked of it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    /// Canvas size in the SVG's own units.
    pub size_px: [f64; 2],
    /// Height over width.
    pub aspect: f64,
    /// Colours, largest first.
    pub colours: Vec<ColourInfo>,
    /// Painted items.
    pub items: usize,
    /// Path segments as written.
    pub nodes: usize,
    /// Content that cannot be cut at all.
    pub unsupported: Vec<String>,
}

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// Worth knowing.
    Info,
    /// Will probably cause trouble.
    Warn,
    /// Will not work as it stands.
    Error,
}

/// One preflight finding.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// How serious.
    pub level: Level,
    /// A stable identifier, for the interface to key on.
    pub code: &'static str,
    /// What was found, in a sentence.
    pub message: String,
}

/// One sheet to cut.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    /// Name, for the file and the list.
    pub name: String,
    /// Colour, `#rrggbb`.
    pub hex: String,
    /// The file to cut, sized in millimetres.
    pub svg: String,
    /// Path segments in it.
    pub nodes: usize,
    /// Separate pieces.
    pub parts: usize,
    /// Material area, square millimetres.
    pub area_mm2: f64,
    /// The sheet's own artwork extent, width and height in millimetres, without the
    /// registration marks and border every sheet shares.
    pub material_mm: [f64; 2],
}

/// The result of a request.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// The sheets, bottom first.
    pub layers: Vec<Layer>,
    /// Every layer stacked in its colour, for looking at.
    pub preview_svg: String,
    /// The preflight's problems where they are, drawn to lay over the preview: parts too
    /// thin to weed filled, specks ringed. Empty of shapes when there are none.
    pub problems_svg: String,
    /// Every sheet in one file, one group per colour, for cutter software that splits a
    /// file into layers itself.
    pub combined_svg: String,
    /// Every sheet in one DXF, a layer each, for CAD and CAM programs.
    pub dxf: String,
    /// Every sheet as GRBL G-code, lines and arcs, for lasers and plotters.
    pub gcode: String,
    /// Output size, millimetres, including marks and borders.
    pub size_mm: [f64; 2],
    /// The preflight.
    pub checks: Vec<Check>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_studio_camel_case_request_reads_back_and_fills_the_rest() {
        // The Studio sends camelCase and may leave fields out; the engine's defaults fill them.
        let o: Options = serde_json::from_str(
            r##"{"mode":"stencil","widthMm":120,"bridgeMm":2,"cutStyle":"hairline","kerfMm":0.15}"##,
        )
        .unwrap();
        assert_eq!(o.mode, Mode::Stencil);
        assert_eq!(o.cut_style, CutStyle::Hairline);
        assert_eq!((o.width_mm, o.bridge_mm, o.kerf_mm), (120.0, 2.0, 0.15));
        assert_eq!(o.min_feature_mm, Options::default().min_feature_mm);
    }
}
