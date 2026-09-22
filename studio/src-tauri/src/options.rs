//! The nineteen controls and the eight presets, and how both become `inkvec_cli::Args`.
//!
//! The interface never shows an engine flag. Every control carries the user-facing name
//! from the terminology table (`min_area` is "Speckle floor", `max_dim` is "Trace size",
//! `content_units` is "Fewer paths") and its tooltip is the engine's own documentation,
//! unedited — the descriptions in [`CONTROLS`] are copied from the doc comments on
//! `inkvec_cli::Args` and `inkvec::Options`, not rewritten.
//!
//! One direction only: the frontend owns a [`Settings`] value, sends it with every trace,
//! and this module turns it into `Args`. Nothing here reads a global.

use serde::{Deserialize, Serialize};

/// Which of the three "clean up damage" positions a control is in.
///
/// The same three the engine has (`Auto` traces, measures the fit, and only cleans if the
/// trace disagrees with the input where it claims to be flat).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Cleanup {
    /// Never clean.
    #[default]
    Off,
    /// Clean only if the trace disagrees with the input.
    Auto,
    /// Always clean first.
    On,
}

impl Cleanup {
    fn restore(self) -> inkvec_restore::Mode {
        match self {
            Cleanup::Off => inkvec_restore::Mode::Off,
            Cleanup::Auto => inkvec_restore::Mode::Auto,
            Cleanup::On => inkvec_restore::Mode::On,
        }
    }
}

/// The nineteen controls, exactly as the Tune tab shows them.
///
/// Serialised with the names the frontend uses. Defaults are the command line's, read
/// through `Args::default()` so the app and `inkvec logo.png` cannot drift apart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    // --- Detail ---
    /// Precision (px).
    pub precision: f64,
    /// Speckle floor (px²).
    pub speckle_floor: f64,
    /// Trace size (px).
    pub trace_size: u32,
    /// Time limit (s); 0 means none.
    pub time_limit: f64,

    // --- Colour ---
    /// Max colours.
    pub max_colours: u32,
    /// Colour merging.
    pub colour_merging: f64,
    /// Flat fills instead of gradients.
    pub flat_fills: bool,
    /// Black & white.
    pub black_and_white: bool,
    /// Clean up damage.
    pub clean_up_damage: Cleanup,

    // --- Shape ---
    /// Match repeated shapes.
    pub match_repeated_shapes: bool,
    /// Match threshold.
    pub match_threshold: f64,
    /// Fewer paths.
    pub fewer_paths: bool,
    /// Line art.
    pub line_art: bool,
    /// Repair crossing rings.
    pub repair_rings: bool,
    /// Editable structure.
    pub editability: bool,
    /// What one Bézier curve costs the fit, in parameters (a line costs 2).
    pub bezier_cost: f64,
    /// The turn, in degrees, at a join that is charged as a full corner.
    pub corner_angle: f64,

    // --- Output ---
    /// Minify.
    pub minify: bool,
    /// Transparent background.
    pub transparent_background: bool,
    /// Margin (px, as a fraction of the larger side — see `margin_fraction`).
    pub margin: f64,
    /// Holes as cutouts.
    pub holes_as_cutouts: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let a = inkvec_cli::Args::default();
        Self {
            precision: a.precision,
            speckle_floor: a.min_area,
            trace_size: a.max_dim as u32,
            time_limit: a.time_budget,
            max_colours: a.max_colors as u32,
            colour_merging: a.merge_distance as f64,
            flat_fills: a.no_gradients,
            black_and_white: a.bilevel,
            clean_up_damage: Cleanup::Off,
            match_repeated_shapes: a.harmonize,
            match_threshold: a.harmonize_threshold,
            fewer_paths: a.content_units,
            line_art: a.strokes,
            repair_rings: !a.no_repair,
            editability: a.editability,
            // The engine's own prices, so an untouched control asks for nothing.
            bezier_cost: inkvec_fit::cost::CostModel::standard().cubic_params,
            corner_angle: inkvec_fit::cost::CostModel::standard().g1_break_degrees,
            minify: a.minify,
            transparent_background: a.no_background,
            margin: a.margin,
            holes_as_cutouts: a.cutout,
        }
    }
}

impl Settings {
    /// These settings as the engine's own argument struct.
    ///
    /// Destructured without `..` so a control added above and not mapped here is a compile
    /// error rather than a knob that silently does nothing — the same discipline
    /// `inkvec::Options::to_args` keeps.
    pub fn to_args(&self) -> inkvec_cli::Args {
        let Self {
            precision,
            speckle_floor,
            trace_size,
            time_limit,
            max_colours,
            colour_merging,
            flat_fills,
            black_and_white,
            clean_up_damage,
            match_repeated_shapes,
            match_threshold,
            fewer_paths,
            line_art,
            repair_rings,
            editability,
            bezier_cost,
            corner_angle,
            minify,
            transparent_background,
            margin,
            holes_as_cutouts,
        } = *self;

        // A price that equals the engine's own is passed as no request at all, so a trace that
        // has not touched these two controls takes exactly the path it always did.
        let standard = inkvec_fit::cost::CostModel::standard();
        let asked = |v: f64, base: f64| ((v - base).abs() > 1e-9).then_some(v);

        inkvec_cli::Args {
            precision,
            min_area: speckle_floor,
            max_dim: trace_size as usize,
            time_budget: time_limit,
            max_colors: max_colours as usize,
            merge_distance: colour_merging as f32,
            no_gradients: flat_fills,
            bilevel: black_and_white,
            restore: clean_up_damage.restore(),
            harmonize: match_repeated_shapes,
            harmonize_threshold: match_threshold,
            content_units: fewer_paths,
            strokes: line_art,
            no_repair: !repair_rings,
            editability,
            bezier_cost: asked(bezier_cost, standard.cubic_params),
            corner_angle: asked(corner_angle, standard.g1_break_degrees),
            minify,
            no_background: transparent_background,
            margin,
            cutout: holes_as_cutouts,
            // Not exposed: the app is not a research harness. `quiet` only suppresses
            // printing, which a library caller gets none of anyway.
            quiet: true,
            ..inkvec_cli::Args::default()
        }
    }

    /// The same settings, but traced small and briefly: the draft tier.
    ///
    /// A draft is a different *size*, not a different drawing. Only the two knobs that
    /// buy time move — the trace size drops to `draft_px` and a hard time limit goes on —
    /// so a draft is recognisably the final trace, just coarser. Changing precision or
    /// the palette here would make the draft lie about what the final will look like.
    pub fn draft(&self, draft_px: u32, draft_seconds: f64) -> Self {
        Self {
            trace_size: draft_px.min(self.trace_size.max(1)),
            time_limit: draft_seconds,
            // The restorer costs about 0.6 s, which is more than the draft's whole
            // budget. It runs for the final trace.
            clean_up_damage: Cleanup::Off,
            ..self.clone()
        }
    }

    /// Clamp every number into the range the engine will accept, so a hand-edited
    /// settings file or a slider glitch cannot reach the tracer as a panic.
    pub fn sanitised(mut self) -> Self {
        let clamp = |v: f64, lo: f64, hi: f64| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                lo
            }
        };
        self.precision = clamp(self.precision, 0.01, 2.0);
        self.speckle_floor = clamp(self.speckle_floor, 0.0, 4096.0);
        self.trace_size = self.trace_size.clamp(64, 16384);
        self.time_limit = clamp(self.time_limit, 0.0, 600.0);
        self.max_colours = self.max_colours.clamp(1, 4096);
        self.colour_merging = clamp(self.colour_merging, 0.0, 1.0);
        self.match_threshold = clamp(self.match_threshold, 0.0, 1.0);
        self.margin = clamp(self.margin, 0.0, 1.0);
        let (lo, hi) = inkvec_fit::cost::CostModel::CUBIC_RANGE;
        self.bezier_cost = clamp(self.bezier_cost, lo, hi);
        let (lo, hi) = inkvec_fit::cost::CostModel::G1_RANGE;
        self.corner_angle = clamp(self.corner_angle, lo, hi);
        self
    }
}

/// One of the seven presets.
///
/// The names are plain language and the flags never appear beside them; the mapping is
/// the one in the design brief's preset table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    Logo,
    Icon,
    FineDetail,
    FewerPaths,
    PhotoOrScan,
    BlackAndWhite,
    LineArt,
    Editable,
}

impl Preset {
    /// Every preset, in the order the tray shows them.
    pub const ALL: [Preset; 8] = [
        Preset::Logo,
        Preset::Icon,
        Preset::FineDetail,
        Preset::FewerPaths,
        Preset::PhotoOrScan,
        Preset::BlackAndWhite,
        Preset::LineArt,
        Preset::Editable,
    ];

    /// The name and the one-line subtitle. The names alone are not self-explanatory,
    /// which is why every one of them carries a subtitle wherever it is shown.
    pub fn labels(self) -> (&'static str, &'static str) {
        match self {
            Preset::Logo => ("Logo", "The default"),
            Preset::Icon => ("Icon", "Small flat marks"),
            Preset::FineDetail => ("Fine detail", "Filigree, crests"),
            Preset::FewerPaths => ("Fewer paths", "Smallest file"),
            Preset::PhotoOrScan => ("Photo or scan", "Photographed or screenshotted"),
            Preset::BlackAndWhite => ("Black & white", "Stamps, signatures"),
            Preset::LineArt => ("Line art", "Uniform-stroke drawings"),
            Preset::Editable => ("Editable", "Tidy nodes for an artist to edit"),
        }
    }

    /// Whether this preset needs the denoiser to do what it says.
    ///
    /// Photo-or-scan traces without it — the "Denoiser missing" state says exactly that —
    /// but the compression damage stays in the colours.
    pub fn wants_denoiser(self) -> bool {
        matches!(self, Preset::PhotoOrScan)
    }

    /// The settings this preset means.
    pub fn settings(self) -> Settings {
        let base = Settings::default();
        match self {
            Preset::Logo => base,
            Preset::Icon => Settings {
                speckle_floor: 1.0,
                max_colours: 16,
                ..base
            },
            Preset::FineDetail => Settings {
                precision: 0.05,
                trace_size: 2048,
                ..base
            },
            Preset::FewerPaths => Settings {
                fewer_paths: true,
                colour_merging: (base.colour_merging * 2.0).min(1.0),
                ..base
            },
            Preset::PhotoOrScan => Settings {
                clean_up_damage: Cleanup::Auto,
                ..base
            },
            Preset::BlackAndWhite => Settings {
                black_and_white: true,
                ..base
            },
            Preset::LineArt => Settings {
                line_art: true,
                ..base
            },
            Preset::Editable => Settings {
                editability: true,
                ..base
            },
        }
    }
}

/// What kind of control a row in the advanced drawer is.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A number with a slider on a perceptual scale.
    Range,
    /// An on/off switch.
    Switch,
    /// Off / Auto / On.
    Tri,
}

/// A labelled stop on a slider's scale.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Stop {
    /// Where on the track, 0..1.
    pub at: f64,
    /// What to call the position.
    pub label: &'static str,
}

/// One row of the advanced drawer.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Control {
    /// The group heading it sits under.
    pub group: &'static str,
    /// The field in [`Settings`] it drives, as the frontend spells it.
    pub key: &'static str,
    /// Its plain-words label.
    pub label: &'static str,
    /// Its unit, or "" where it has none.
    pub unit: &'static str,
    /// Range, switch or tri-state.
    pub kind: Kind,
    /// Lowest accepted value (ranges only).
    pub min: f64,
    /// Highest accepted value (ranges only).
    pub max: f64,
    /// The exponent of the slider's power scale. 1.0 is a linear track; precision runs
    /// 0.02–0.5 and is perceptually nowhere near linear, so it gets a curve.
    pub curve: f64,
    /// Decimals to show. 0 means an integer.
    pub decimals: u8,
    /// Named stops along the track, rather than a bare numeric axis.
    pub stops: &'static [Stop],
    /// The tooltip: the engine's own documentation for this option, unedited.
    pub help: &'static str,
}

/// The Tune tab's controls, in order. Four groups, nineteen rows.
pub const CONTROLS: &[Control] = &[
    // ----------------------------------------------------------------- Detail ---
    Control {
        group: "Detail",
        key: "precision",
        label: "Precision",
        unit: "px",
        kind: Kind::Range,
        min: 0.02,
        max: 0.5,
        curve: 2.2,
        decimals: 2,
        stops: &[
            Stop { at: 0.0, label: "exact" },
            Stop { at: 0.35, label: "default" },
            Stop { at: 1.0, label: "coarse" },
        ],
        help: "Sets the description-length cost of a coordinate, in pixels: lambda = ln(extent / precision). Smaller values buy more detail with more coordinates. It does not set the digits written; coordinates are always written at 2 decimals.",
    },
    Control {
        group: "Detail",
        key: "speckleFloor",
        label: "Speckle floor",
        unit: "px²",
        kind: Kind::Range,
        min: 0.0,
        max: 64.0,
        curve: 2.0,
        decimals: 1,
        stops: &[
            Stop { at: 0.0, label: "keep all" },
            Stop { at: 1.0, label: "drop dust" },
        ],
        help: "Discard features smaller than this area, in square pixels. Raise it to drop scanner dust; lower it to keep small serifs.",
    },
    Control {
        group: "Detail",
        key: "traceSize",
        label: "Trace size",
        unit: "px",
        kind: Kind::Range,
        min: 256.0,
        max: 4096.0,
        curve: 1.0,
        decimals: 0,
        stops: &[
            Stop { at: 0.0, label: "256" },
            Stop { at: 0.47, label: "2048" },
            Stop { at: 1.0, label: "4096" },
        ],
        help: "Inputs larger than this on their longer side, in pixels, are traced at this size and the SVG is written at the original size. Trace time grows with the pixel count.",
    },
    Control {
        group: "Detail",
        key: "timeLimit",
        label: "Time limit",
        unit: "s",
        kind: Kind::Range,
        min: 0.0,
        max: 60.0,
        curve: 1.6,
        decimals: 1,
        stops: &[
            Stop { at: 0.0, label: "none" },
            Stop { at: 1.0, label: "60" },
        ],
        help: "Advisory wall-clock budget, in seconds; 0 means none. Gradient-band merging stops at 60% of it and the boundary solve gets 25%; the output is still a correct trace, with more fills or a less polished outline. A nonzero budget makes the output depend on machine speed and load, so it is no longer reproducible.",
    },
    // ----------------------------------------------------------------- Colour ---
    Control {
        group: "Colour",
        key: "maxColours",
        label: "Max colours",
        unit: "",
        kind: Kind::Range,
        min: 2.0,
        max: 2048.0,
        curve: 2.4,
        decimals: 0,
        stops: &[
            Stop { at: 0.0, label: "2" },
            Stop { at: 0.4, label: "64" },
            Stop { at: 1.0, label: "2048" },
        ],
        help: "Maximum palette size.",
    },
    Control {
        group: "Colour",
        key: "colourMerging",
        label: "Colour merging",
        unit: "",
        kind: Kind::Range,
        min: 0.0,
        max: 0.2,
        curve: 1.4,
        decimals: 3,
        stops: &[
            Stop { at: 0.0, label: "none" },
            Stop { at: 1.0, label: "heavy" },
        ],
        help: "OKLab distance below which two colours are treated as one ink.",
    },
    Control {
        group: "Colour",
        key: "flatFills",
        label: "Flat fills instead of gradients",
        unit: "",
        kind: Kind::Switch,
        min: 0.0,
        max: 1.0,
        curve: 1.0,
        decimals: 0,
        stops: &[],
        help: "Skip gradient fitting entirely and fill flat.",
    },
    Control {
        group: "Colour",
        key: "blackAndWhite",
        label: "Black & white",
        unit: "",
        kind: Kind::Switch,
        min: 0.0,
        max: 1.0,
        curve: 1.0,
        decimals: 0,
        stops: &[],
        help: "Two-tone output (the Potrace-comparable mode).",
    },
    Control {
        group: "Colour",
        key: "cleanUpDamage",
        label: "Clean up damage",
        unit: "",
        kind: Kind::Tri,
        min: 0.0,
        max: 2.0,
        curve: 1.0,
        decimals: 0,
        stops: &[],
        help: "The trained restorer: removes JPEG, WebP and decoder damage at the input's own size before tracing. Off by default, because on clean input it costs a little colour accuracy. Auto traces, measures the fit, and restores only if the trace disagrees with the input where it claims to be flat.",
    },
    // ------------------------------------------------------------------ Shape ---
    Control {
        group: "Shape",
        key: "matchRepeatedShapes",
        label: "Match repeated shapes",
        unit: "",
        kind: Kind::Switch,
        min: 0.0,
        max: 1.0,
        curve: 1.0,
        decimals: 0,
        stops: &[],
        help: "Marks that repeat across the drawing are redrawn from one consensus geometry per cluster, which saves parameters. A mark takes the consensus only where that stays within 0.1 px of the boundary traced for it and costs fewer parameters; a face another face is drawn against, and a fitted circle or rounded rectangle, is never moved.",
    },
    Control {
        group: "Shape",
        key: "matchThreshold",
        label: "Match threshold",
        unit: "",
        kind: Kind::Range,
        min: 0.5,
        max: 1.0,
        curve: 1.0,
        decimals: 2,
        stops: &[
            Stop { at: 0.0, label: "loose" },
            Stop { at: 1.0, label: "identical" },
        ],
        help: "Shape-equivalence threshold for harmonization: the outline similarity (IoU after affine normalisation) above which two marks count as the same shape.",
    },
    Control {
        group: "Shape",
        key: "fewerPaths",
        label: "Fewer paths",
        unit: "",
        kind: Kind::Switch,
        min: 0.0,
        max: 1.0,
        curve: 1.0,
        decimals: 0,
        stops: &[],
        help: "Scale the fit tolerances with the raster, so a large, simple drawing gets the parameter count of a small one. Trades fidelity for parsimony: small squares can come back as circles and thin rings broken.",
    },
    Control {
        group: "Shape",
        key: "lineArt",
        label: "Line art",
        unit: "",
        kind: Kind::Switch,
        min: 0.0,
        max: 1.0,
        curve: 1.0,
        decimals: 0,
        stops: &[],
        help: "Emit line art as strokes — one path and one width — instead of as filled outlines. Declines silently on anything else.",
    },
    Control {
        group: "Shape",
        key: "repairRings",
        label: "Repair crossing rings",
        unit: "",
        kind: Kind::Switch,
        min: 0.0,
        max: 1.0,
        curve: 1.0,
        decimals: 0,
        stops: &[],
        help: "The self-crossing ring repair pass that runs after fitting. Off leaves a fitted boundary exactly as the fitter wrote it, crossings and all.",
    },
    Control {
        group: "Shape",
        key: "editability",
        label: "Editable structure",
        unit: "",
        kind: Kind::Switch,
        min: 0.0,
        max: 1.0,
        curve: 1.0,
        decimals: 0,
        stops: &[],
        help: "Post-fit passes that trade parameters for structure an artist can edit: G1-smooth joins, axis-aligned and equal-length handles, aligned nodes, and self-symmetric rings locked into exact mirrors. Fidelity stays within the same tolerance; only structure and parameters move.",
    },
    Control {
        group: "Shape",
        key: "bezierCost",
        label: "Curve cost",
        unit: "params",
        kind: Kind::Range,
        min: 2.0,
        max: 12.0,
        curve: 1.0,
        decimals: 1,
        stops: &[
            Stop { at: 0.0, label: "more curves" },
            Stop { at: 0.4, label: "default" },
            Stop { at: 1.0, label: "more lines" },
        ],
        help: "What one Bézier curve costs the fit, in parameters; a straight line costs 2. At the default, 6, a chain of short lines is cheaper than the one curve that describes it, which is why traces come out less curved than hand-drawn artwork. Lower it and the tracer draws more curves and fewer lines, at some cost in file size.",
    },
    Control {
        group: "Shape",
        key: "cornerAngle",
        label: "Smooth-join angle",
        unit: "°",
        kind: Kind::Range,
        min: 1.0,
        max: 60.0,
        curve: 1.0,
        decimals: 0,
        stops: &[
            Stop { at: 0.0, label: "sharper" },
            Stop { at: 0.15, label: "default" },
            Stop { at: 1.0, label: "smoother" },
        ],
        help: "The turn, in degrees, at a join that is charged as a full corner; below it the charge ramps up gradually. Raising it lets gentler bends stay smooth, which tends to give more curves and a little more detail, at a somewhat larger file. The default is 10°.",
    },
    // ----------------------------------------------------------------- Output ---
    Control {
        group: "Output",
        key: "minify",
        label: "Minify",
        unit: "",
        kind: Kind::Switch,
        min: 0.0,
        max: 1.0,
        curve: 1.0,
        decimals: 0,
        stops: &[],
        help: "No ids or groups, no trailing zeros. Same geometry, typically about a tenth smaller.",
    },
    Control {
        group: "Output",
        key: "transparentBackground",
        label: "Transparent background",
        unit: "",
        kind: Kind::Switch,
        min: 0.0,
        max: 1.0,
        curve: 1.0,
        decimals: 0,
        stops: &[],
        help: "Knock the background out: the face that covers the whole canvas is not painted, so the artwork sits on transparency.",
    },
    Control {
        group: "Output",
        key: "margin",
        label: "Margin",
        unit: "%",
        kind: Kind::Range,
        min: 0.0,
        max: 0.25,
        curve: 1.0,
        decimals: 3,
        stops: &[
            Stop { at: 0.0, label: "none" },
            Stop { at: 1.0, label: "25%" },
        ],
        help: "Transparent margin around the output, as a fraction of the larger side. The viewBox grows; the geometry does not move.",
    },
    Control {
        group: "Output",
        key: "holesAsCutouts",
        label: "Holes as cutouts",
        unit: "",
        kind: Kind::Switch,
        min: 0.0,
        max: 1.0,
        curve: 1.0,
        decimals: 0,
        stops: &[],
        help: "Carry the input's transparency into the SVG: a face the source drew transparent becomes a hole, one drawn at a single opacity keeps it as fill-opacity, and white artwork on a transparent ground survives. Changes nothing for an opaque input.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_command_lines_defaults() {
        let a = Settings::default().to_args();
        let d = inkvec_cli::Args::default();
        assert_eq!(a.precision, d.precision);
        assert_eq!(a.min_area, d.min_area);
        assert_eq!(a.max_dim, d.max_dim);
        assert_eq!(a.max_colors, d.max_colors);
        assert_eq!(a.harmonize, d.harmonize);
        assert_eq!(a.no_repair, d.no_repair);
    }

    #[test]
    fn there_are_twenty_one_controls_in_four_groups() {
        assert_eq!(CONTROLS.len(), 21);
        let mut groups: Vec<&str> = CONTROLS.iter().map(|c| c.group).collect();
        groups.dedup();
        assert_eq!(groups, ["Detail", "Colour", "Shape", "Output"]);
    }

    #[test]
    fn untouched_curve_prices_ask_the_engine_for_nothing() {
        let a = Settings::default().to_args();
        assert_eq!(a.bezier_cost, None, "the default cost is no request");
        assert_eq!(a.corner_angle, None, "the default angle is no request");
    }

    #[test]
    fn changed_curve_prices_reach_the_engine() {
        let s = Settings {
            bezier_cost: 3.0,
            corner_angle: 30.0,
            ..Settings::default()
        };
        let a = s.to_args();
        assert_eq!(a.bezier_cost, Some(3.0));
        assert_eq!(a.corner_angle, Some(30.0));
    }

    #[test]
    fn curve_prices_are_held_to_the_range_the_engine_accepts() {
        let wild = Settings {
            bezier_cost: -5.0,
            corner_angle: 9999.0,
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(wild.bezier_cost, inkvec_fit::cost::CostModel::CUBIC_RANGE.0);
        assert_eq!(wild.corner_angle, inkvec_fit::cost::CostModel::G1_RANGE.1);
    }

    #[test]
    fn every_control_names_a_field_that_serialises() {
        let json = serde_json::to_value(Settings::default()).unwrap();
        for c in CONTROLS {
            assert!(json.get(c.key).is_some(), "no settings field for {}", c.key);
        }
        // And nothing in Settings is missing a control.
        let keys: Vec<&str> = CONTROLS.iter().map(|c| c.key).collect();
        for k in json.as_object().unwrap().keys() {
            assert!(keys.contains(&k.as_str()), "{k} has no control");
        }
    }

    #[test]
    fn a_draft_only_moves_the_two_knobs_that_buy_time() {
        let full = Settings::default();
        let draft = full.draft(512, 0.4);
        assert_eq!(draft.trace_size, 512);
        assert_eq!(draft.time_limit, 0.4);
        assert_eq!(draft.precision, full.precision);
        assert_eq!(draft.max_colours, full.max_colours);
        assert_eq!(draft.minify, full.minify);
    }

    #[test]
    fn a_draft_never_traces_larger_than_the_full_trace() {
        let small = Settings {
            trace_size: 300,
            ..Settings::default()
        };
        assert_eq!(small.draft(512, 0.4).trace_size, 300);
    }

    #[test]
    fn sanitising_survives_nonsense() {
        let wild = Settings {
            precision: f64::NAN,
            trace_size: 0,
            max_colours: 99_999,
            margin: -3.0,
            ..Settings::default()
        }
        .sanitised();
        assert!(wild.precision.is_finite());
        assert_eq!(wild.trace_size, 64);
        assert_eq!(wild.max_colours, 4096);
        assert_eq!(wild.margin, 0.0);
    }

    #[test]
    fn editable_structure_is_off_until_asked_for_and_reaches_the_engine() {
        assert!(!Settings::default().to_args().editability);
        assert!(!inkvec_cli::Args::default().editability);
        let on = Settings {
            editability: true,
            ..Settings::default()
        };
        assert!(on.to_args().editability);
        assert!(Preset::Editable.settings().to_args().editability);
        // Only the Editable preset asks for it.
        for p in Preset::ALL.iter().filter(|p| **p != Preset::Editable) {
            assert!(!p.settings().editability, "{p:?} turns editability on");
        }
    }

    #[test]
    fn a_settings_file_from_before_editability_still_loads() {
        // Prefs written by an older build have no such key; the struct-level default fills it.
        let old = serde_json::json!({ "precision": 0.2, "maxColours": 12 });
        let s: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(s.max_colours, 12);
        assert!(!s.editability);
    }

    #[test]
    fn each_preset_differs_from_the_default_except_logo() {
        assert_eq!(Preset::Logo.settings(), Settings::default());
        for p in Preset::ALL.iter().filter(|p| **p != Preset::Logo) {
            assert_ne!(p.settings(), Settings::default(), "{p:?} changes nothing");
        }
    }
}
