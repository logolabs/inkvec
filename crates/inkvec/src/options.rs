//! The options contract: the one definition every language binding is generated from.
//!
//! [`Options`] is the single source of truth. Its field names, types, defaults, doc comments
//! and ranges become `bindings/options.schema.json` (see [`options_schema_json`]), and the C,
//! Python and future bindings read that schema instead of restating the fields. Validation
//! reads the same schema, so a range written once on a field is both documented and enforced.
//!
//! Adding an option touches this file only: the field (with its doc comment and, for a number,
//! its range), its default in [`Default`], and its line in [`Options::to_args`]. The last two are
//! compile-checked -- `Default` names every field, and `to_args` destructures the struct
//! without `..` -- and `cargo test -p inkvec` then regenerates the schema and fails once so the
//! change is reviewed. `docs/BINDINGS.md` has the whole procedure.

use crate::Error;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;

/// Options for one trace.
///
/// [`Options::default`] is exactly the command line's defaults (it is built from
/// `inkvec_cli::Args::default()`), so a trace with default options is the trace
/// `inkvec logo.png` writes.
///
/// The struct is `#[non_exhaustive]`: fields will be added, and code outside this crate
/// cannot build it with a struct literal. Start from the defaults and assign the fields you
/// want, or parse JSON:
///
/// ```
/// let mut opts = inkvec::Options::default();
/// opts.colors = 16;
/// opts.no_background = true;
/// assert!(opts.validate().is_ok());
///
/// let same = inkvec::Options::from_json(r#"{"colors": 16, "no_background": true}"#).unwrap();
/// assert_eq!(opts, same);
/// ```
///
/// Every number is validated by [`Options::validate`] (called by [`crate::trace`] and
/// [`crate::trace_rgba`] too) against the range recorded in the schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[schemars(
    title = "InkvecOptions",
    description = "Options for one Inkvec trace. Every field is optional and takes its default when missing; an unknown field is an error."
)]
#[non_exhaustive]
pub struct Options {
    /// Sets the description-length cost of a coordinate, in pixels: lambda = ln(extent / precision). Smaller values buy more detail with more coordinates. It does not set the digits written; coordinates are always written at 2 decimals.
    #[schemars(extend("exclusiveMinimum" = 0))]
    pub precision: f64,

    /// Discard features smaller than this area, in square pixels.
    #[schemars(extend("exclusiveMinimum" = 0))]
    pub min_area: f64,

    /// Maximum palette size.
    #[schemars(range(min = 1, max = 4096))]
    pub colors: u32,

    /// OKLab distance below which two colours are treated as one ink.
    #[schemars(range(min = 0))]
    pub merge: f64,

    /// Inputs larger than this on their longer side, in pixels, are traced at this size and the SVG is written at the original size. Trace time grows with the pixel count. 0 means no cap.
    pub max_dim: u32,

    /// Advisory wall-clock budget, in seconds; 0 means none. Gradient-band merging stops at 60% of it and the boundary solve gets 25%; the output is still a correct trace, with more fills or a less polished outline. A nonzero budget makes the output depend on machine speed and load, so it is no longer reproducible.
    #[schemars(range(min = 0))]
    pub time_budget: f64,

    /// Transparent margin around the output, as a fraction of the larger side. The viewBox grows; the geometry does not move.
    #[schemars(range(min = 0))]
    pub margin: f64,

    /// Knock the background out: the face that covers the whole canvas is not painted, so the artwork sits on transparency.
    pub no_background: bool,

    /// No ids or groups, no trailing zeros. Same geometry, typically about a tenth smaller.
    pub minify: bool,

    /// Spend parameters on structure an artist can edit: joins between curves made G1-smooth, handles snapped to the axes and to 45 degrees, handles of one curve made equal in length, nodes that nearly share a coordinate made to share it, and rings that are their own mirror image locked into exact mirrors. Every change is guarded to the fit's own tolerance -- 3 sigma of the source point plus half a pixel, or 1.5 px for a mirror lock -- so the picture stays within a fraction of a pixel of the default trace; the price measured on 25 icons is about 0.04 dE00. Off by default.
    pub editability: bool,

    /// Trace transparency natively: each ink is a colour and an opacity, and the transparent ground is an ink of its own, instead of the image being composited onto a matte first. Holes stay holes, white artwork on a transparent ground traces, glows and shadows stay translucent, and a fade is one gradient of colour and opacity. An opaque input traces the same either way. On by default, as on the command line (where the environment variable INKVEC_NATIVE_ALPHA=0 turns the default off); false composites onto a matte first, as releases up to 0.1.3 did.
    pub native_alpha: bool,

    /// With native_alpha off, carry the input's transparency into the SVG: a face the source drew transparent becomes a hole, one drawn at a single opacity keeps it as fill-opacity, and white artwork on a transparent ground survives. Changes nothing for an opaque input, and nothing with native_alpha on (the default), which already carries the transparency out.
    pub cutout: bool,

    /// Scale the fit tolerances with the raster, so a large, simple drawing gets the parameter count of a small one. Trades fidelity for parsimony: small squares can come back as circles and thin rings broken.
    pub content_units: bool,

    /// Shape harmonization (on by default): marks that repeat across the drawing are redrawn from one consensus geometry per cluster, which saves parameters. A mark takes the consensus only where that stays within 0.1 px of the boundary traced for it and costs fewer parameters; a face another face is drawn against, and a fitted circle or rounded rectangle, is never moved. Set it to false to skip the pass.
    pub harmonize: bool,

    /// Shape-equivalence threshold for harmonization: the outline similarity (IoU after affine normalisation) above which two marks count as the same shape.
    #[schemars(range(min = 0, max = 1))]
    pub harmonize_threshold: f64,

    /// Which engine traces the image. "quality" (the default) is the full engine: the best fidelity and the fewest parameters, at about half a second for a 512 px logo. "fast" is a Potrace-class fit on the same palette, planar map and emitter, with a one-pass gradient check in place of gradient recovery: several times faster (tens of milliseconds at 512 px), a little less faithful, with somewhat more parameters. Options that only steer quality stages (precision, content_units, harmonize, harmonize_threshold, time_budget) are ignored in fast mode.
    #[schemars(extend("enum" = ["quality", "fast"]))]
    pub mode: String,

    /// Colour groups: fills to draw as one, so the shapes between them join rather than being recoloured. Empty (the default) changes nothing. Groups are separated by ';' and members by ','; a member is a colour '#rrggbb' as it appears in a trace of the same image, or a gradient written as its stop colours joined by '>'. An optional '=' says what the group becomes: '=#rrggbb' a flat colour, '=@n' its n-th member (1-based; a gradient there is refitted over the whole group); without it, the member covering the most of the image. Example: '#c0392b,#e74c3c;#f00>#00f,#0a0=@1'. A group costs one extra trace.
    pub merge_colors: String,
}

impl Default for Options {
    /// The command line's defaults, read from `inkvec_cli::Args::default()` so the two can
    /// never disagree.
    fn default() -> Self {
        let a = inkvec_cli::Args::default();
        Self {
            precision: a.precision,
            min_area: a.min_area,
            colors: a.max_colors.min(u32::MAX as usize) as u32,
            merge: shortest_f64(a.merge_distance),
            max_dim: a.max_dim.min(u32::MAX as usize) as u32,
            time_budget: a.time_budget,
            margin: a.margin,
            no_background: a.no_background,
            minify: a.minify,
            editability: a.editability,
            native_alpha: a.native_alpha,
            cutout: a.cutout,
            content_units: a.content_units,
            harmonize: a.harmonize,
            harmonize_threshold: a.harmonize_threshold,
            // The command line has no groups by default; a spec that parses to none.
            merge_colors: String::new(),
            mode: a.mode.name().to_string(),
        }
    }
}

impl Options {
    /// Parse options from a JSON object, then [`validate`](Options::validate) them.
    ///
    /// Missing fields take their defaults; an empty or all-whitespace string means "all
    /// defaults". Unknown fields, wrong types and out-of-range values are
    /// [`Error::InvalidOptions`] naming the field.
    pub fn from_json(json: &str) -> Result<Self, Error> {
        let json = json.trim();
        if json.is_empty() {
            return Ok(Self::default());
        }
        let invalid = |e: serde_json::Error| Error::InvalidOptions(e.to_string());
        let value: Value = serde_json::from_str(json).map_err(invalid)?;
        // serde would also read a struct from an array, by position; options are named.
        if !value.is_object() {
            return Err(Error::InvalidOptions(
                "options must be a JSON object".into(),
            ));
        }
        let opts: Self = serde_json::from_value(value).map_err(invalid)?;
        opts.validate()?;
        Ok(opts)
    }

    /// These options as a compact JSON object, every field present.
    pub fn to_json(&self) -> String {
        // Straight to text rather than through `serde_json::Value`: the text serializer
        // writes the shortest form of each number.
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Check every number against the range recorded for it in the schema, and that it is
    /// finite.
    ///
    /// The ranges are declared once, as attributes on the fields, and read back here from
    /// the generated schema, so what the schema documents and what this enforces cannot
    /// drift apart.
    pub fn validate(&self) -> Result<(), Error> {
        let value = serde_json::to_value(self).map_err(|e| Error::InvalidOptions(e.to_string()))?;
        let props = schema()
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(|| Error::Internal("the options schema has no properties".into()))?;
        for (name, prop) in props {
            check_number(name, value.get(name), prop)?;
        }
        for (name, prop) in props {
            check_choice(name, value.get(name), prop)?;
        }
        inkvec_cli::parse_color_groups(&self.merge_colors)
            .map_err(|e| Error::InvalidOptions(format!("merge_colors: {e}")))?;
        Ok(())
    }

    /// The pipeline's own settings for these options, for the crates of this workspace that
    /// drive the pipeline in two halves (the WebAssembly build hands the decoded raster to a
    /// denoiser between them). Everything else should call [`crate::trace`].
    #[doc(hidden)]
    pub fn pipeline_args(&self) -> inkvec_cli::Args {
        self.to_args()
    }

    /// The pipeline's own settings for these options.
    ///
    /// The struct is destructured without `..`, so a field added to [`Options`] and not
    /// mapped here is a compile error rather than an option that silently does nothing.
    pub(crate) fn to_args(&self) -> inkvec_cli::Args {
        let Self {
            precision,
            min_area,
            colors,
            merge,
            max_dim,
            time_budget,
            margin,
            no_background,
            minify,
            editability,
            native_alpha,
            cutout,
            content_units,
            harmonize,
            harmonize_threshold,
            ref merge_colors,
            ref mode,
        } = *self;
        inkvec_cli::Args {
            // Validated before any trace; an invalid spec that got this far merges nothing.
            merge_colors: inkvec_cli::parse_color_groups(merge_colors).unwrap_or_default(),
            precision,
            min_area,
            max_colors: colors as usize,
            merge_distance: merge as f32,
            max_dim: max_dim as usize,
            time_budget,
            margin,
            no_background,
            minify,
            editability,
            native_alpha,
            cutout,
            content_units,
            harmonize,
            harmonize_threshold,
            // Validated before any trace against the schema's `enum`.
            mode: mode.parse().unwrap_or_default(),
            // A library never writes to the terminal.
            quiet: true,
            ..inkvec_cli::Args::default()
        }
    }
}

/// An `f32` as the `f64` with the same shortest decimal form, so `0.035f32` becomes `0.035`
/// in the schema and in every generated binding rather than `0.03500000014901161`.
fn shortest_f64(x: f32) -> f64 {
    x.to_string().parse().unwrap_or(x as f64)
}

/// One property's value against its schema: finite, and inside any `minimum`, `maximum`,
/// `exclusiveMinimum` or `exclusiveMaximum` the schema records. Non-numbers pass.
fn check_number(name: &str, value: Option<&Value>, prop: &Value) -> Result<(), Error> {
    let numeric = matches!(
        prop.get("type").and_then(Value::as_str),
        Some("number" | "integer")
    );
    if !numeric {
        return Ok(());
    }
    // `serde_json` turns NaN and the infinities into `null`, so a non-finite field arrives
    // here as a missing number.
    let x = match value.and_then(Value::as_f64) {
        Some(x) if x.is_finite() => x,
        _ => {
            return Err(Error::InvalidOptions(format!(
                "`{name}` must be a finite number"
            )))
        }
    };
    let bound = |key: &str| prop.get(key).and_then(Value::as_f64);
    let fail = |rule: &str, b: f64| {
        Err(Error::InvalidOptions(format!(
            "`{name}` must be {rule} {b}, got {x}"
        )))
    };
    if let Some(b) = bound("minimum").filter(|&b| x < b) {
        return fail(">=", b);
    }
    if let Some(b) = bound("exclusiveMinimum").filter(|&b| x <= b) {
        return fail(">", b);
    }
    if let Some(b) = bound("maximum").filter(|&b| x > b) {
        return fail("<=", b);
    }
    if let Some(b) = bound("exclusiveMaximum").filter(|&b| x >= b) {
        return fail("<", b);
    }
    Ok(())
}

/// A string property against the `enum` the schema records for it, if any.
fn check_choice(name: &str, value: Option<&Value>, prop: &Value) -> Result<(), Error> {
    let (Some(choices), Some(v)) = (
        prop.get("enum").and_then(Value::as_array),
        value.and_then(Value::as_str),
    ) else {
        return Ok(());
    };
    if choices.iter().any(|c| c.as_str() == Some(v)) {
        return Ok(());
    }
    let names: Vec<&str> = choices.iter().filter_map(Value::as_str).collect();
    Err(Error::InvalidOptions(format!(
        "`{name}` must be one of {}, got {v:?}",
        names.join(", ")
    )))
}

/// The JSON Schema of [`Options`], generated once.
fn schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
        let schema = generator.into_root_schema_for::<Options>();
        serde_json::to_value(schema).unwrap_or(Value::Null)
    })
}

/// The JSON Schema (draft 2020-12) of [`Options`], pretty-printed, with a trailing newline.
///
/// This is byte for byte the committed `bindings/options.schema.json` that the language
/// bindings are generated from; a test fails when the two differ. Each property carries its
/// `type`, `default`, `description` and, for numbers, its range.
pub fn options_schema_json() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| {
        let mut s = serde_json::to_string_pretty(schema()).unwrap_or_else(|_| "{}".into());
        s.push('\n');
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_command_line_defaults() {
        let expected = inkvec_cli::Args {
            quiet: true,
            ..inkvec_cli::Args::default()
        };
        assert_eq!(
            format!("{:?}", Options::default().to_args()),
            format!("{expected:?}")
        );
    }

    #[test]
    fn every_option_reaches_the_pipeline() {
        // Change each field in turn, through JSON so the test needs no list of fields, and
        // check the pipeline's settings move. An option mapped to nothing fails here.
        let base = format!("{:?}", Options::default().to_args());
        let defaults = serde_json::to_value(Options::default()).unwrap();
        let props = &schema()["properties"];
        for (name, v) in defaults.as_object().unwrap() {
            // A string with a fixed set of values changes to another of them.
            let other_choice = props[name]["enum"]
                .as_array()
                .and_then(|c| c.iter().find(|c| *c != v).cloned());
            let changed = match v {
                Value::String(_) if other_choice.is_some() => other_choice.unwrap(),
                Value::Bool(b) => Value::Bool(!b),
                Value::Number(n) if n.is_u64() => Value::from(n.as_u64().unwrap() + 1),
                Value::Number(n) => Value::from(n.as_f64().unwrap() * 0.5 + 0.25),
                // Otherwise the string is `merge_colors`, a group spec.
                Value::String(_) => Value::from("#c0392b,#e74c3c"),
                other => panic!("no test value for {name} = {other}"),
            };
            let json = serde_json::json!({ name.as_str(): changed }).to_string();
            let opts = Options::from_json(&json).unwrap_or_else(|e| panic!("{json}: {e}"));
            assert_ne!(
                format!("{:?}", opts.to_args()),
                base,
                "{name} changes nothing"
            );
        }
    }

    #[test]
    fn json_round_trips_and_defaults_fill_gaps() {
        let o = Options::from_json(r#"{"colors": 8}"#).unwrap();
        assert_eq!(o.colors, 8);
        assert_eq!(o.precision, Options::default().precision);
        assert_eq!(Options::from_json(&o.to_json()).unwrap(), o);
        assert_eq!(Options::from_json("").unwrap(), Options::default());
        assert_eq!(Options::from_json(" {} ").unwrap(), Options::default());
        assert!(Options::default().to_json().contains("\"merge\":0.035"));
    }

    #[test]
    fn bad_options_name_the_field() {
        let err = |j: &str| match Options::from_json(j) {
            Err(Error::InvalidOptions(m)) => m,
            other => panic!("{j}: expected InvalidOptions, got {other:?}"),
        };
        assert!(err(r#"{"colours": 8}"#).contains("colours"));
        assert!(err(r##"{"merge_colors": "#f00,#nope"}"##).contains("merge_colors"));
        assert!(err(r##"{"merge_colors": "#f00,#00f=@9"}"##).contains("merge_colors"));
        assert!(err(r#"{"colors": 0}"#).contains("colors"));
        assert!(err(r#"{"colors": 5000}"#).contains("colors"));
        assert!(err(r#"{"colors": -1}"#).contains("u32"));
        assert!(err(r#"{"precision": 0}"#).contains("precision"));
        assert!(err(r#"{"harmonize_threshold": 1.5}"#).contains("harmonize_threshold"));
        assert!(err(r#"{"cutout": 1}"#).contains("bool"));
        assert!(err(r#"{"mode": "slow"}"#).contains("quality, fast"));
        assert_eq!(
            Options::from_json(r#"{"mode": "fast"}"#).unwrap().mode,
            "fast"
        );
        assert!(err("[1, 2]").contains("JSON object"));
        assert!(err("null").contains("JSON object"));
        assert!(err("{nope").contains("key"));
        let o = Options {
            margin: f64::NAN,
            ..Options::default()
        };
        assert!(matches!(o.validate(), Err(Error::InvalidOptions(m)) if m.contains("margin")));
    }

    #[test]
    fn the_schema_records_types_defaults_and_ranges() {
        let s = schema();
        assert_eq!(s["additionalProperties"], Value::Bool(false));
        let p = &s["properties"];
        assert_eq!(p["colors"]["type"], "integer");
        assert_eq!(p["colors"]["default"], 64);
        assert_eq!(p["colors"]["minimum"], 1);
        assert_eq!(p["merge"]["default"], 0.035);
        assert_eq!(p["precision"]["exclusiveMinimum"], 0);
        assert_eq!(p["harmonize"]["default"], true);
        assert_eq!(p["mode"]["default"], "quality");
        assert_eq!(p["mode"]["enum"], serde_json::json!(["quality", "fast"]));
        assert_eq!(
            p["native_alpha"]["default"],
            inkvec_cli::Args::default().native_alpha
        );
        for (name, prop) in p.as_object().unwrap() {
            assert!(
                prop["description"].as_str().is_some_and(|d| !d.is_empty()),
                "{name} has no description"
            );
            assert!(prop.get("default").is_some(), "{name} has no default");
        }
    }
}
