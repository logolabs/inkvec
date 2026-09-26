//! The interface as it was left: the part of the preferences that is choices made in passing
//! rather than settings chosen on purpose. Which tab was showing, which half of the rail,
//! the viewer's arrangement and layers, the export formats ticked, the Minify and Fabricate
//! tabs' own controls, and (on the desktop) where the window was.
//!
//! Kept by both shells in the same file as the rest of the preferences ([`crate::prefs`]),
//! written as the choices change and read before the first paint, so the app opens as it
//! was closed. Nothing here is about an image: colour groups, snaps, zoom and pan belong to
//! the image on screen and start fresh with the next one.
//!
//! Read leniently. A value this version does not recognise (an older or newer build wrote
//! it, or a hand edit) falls back to its own default and takes nothing else with it: a
//! field is never a reason to lose the rest of the file.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::export::Formats;
use crate::minify::MinifySettings;

/// Deserialize `T`, or its default when the value is not one `T` accepts.
pub fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

/// Deserialize `T`, or `fallback` when the value is not one `T` accepts: for a field whose
/// default is not its type's.
pub fn lenient_or<'de, D, T>(d: D, fallback: T) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or(fallback))
}

/// A switch that is on unless the file says otherwise, readably.
pub fn lenient_true<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    lenient_or(d, true)
}

fn lenient_half<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    lenient_or(d, 0.5)
}

/// Deserialize a list, keeping the entries `T` accepts and dropping the rest.
pub fn lenient_list<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|x| serde_json::from_value(x).ok())
            .collect(),
        _ => Vec::new(),
    })
}

/// Which of the viewer's layers are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Layers {
    pub fill: bool,
    pub wireframe: bool,
    pub anchors: bool,
    pub handles: bool,
    pub certainty: bool,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            fill: true,
            wireframe: false,
            anchors: false,
            handles: false,
            certainty: false,
        }
    }
}

/// The interface's remembered choices. Names are the frontend's (`State` in
/// `studio/src/lib/state.ts`); the enumerations are strings here, checked in
/// [`Interface::sanitised`], so an unknown one resets that field alone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Interface {
    /// The preset last selected: a built-in id or a saved preset's. `None` once a control
    /// was moved away from it; `logo` in a new file.
    #[serde(deserialize_with = "lenient")]
    pub preset: Option<String>,
    /// The top-level tab: `vectorize`, `minify`, `fabricate` or `batch`.
    #[serde(deserialize_with = "lenient")]
    pub tab: String,
    /// The rail's half: `result` or `tune`.
    #[serde(deserialize_with = "lenient")]
    pub rail_tab: String,
    /// Which of the Tune tab's groups are folded open, by name.
    #[serde(deserialize_with = "lenient")]
    pub groups_open: BTreeMap<String, bool>,
    /// The viewer's arrangement: `side`, `wipe` or `ab`.
    #[serde(deserialize_with = "lenient")]
    pub view: String,
    /// Where the wipe divides the panes, 0 to 1.
    #[serde(deserialize_with = "lenient_half")]
    pub wipe: f64,
    /// The viewer's layers.
    #[serde(deserialize_with = "lenient")]
    pub show: Layers,
    /// The Detail loupe.
    #[serde(deserialize_with = "lenient")]
    pub detail: bool,
    /// The export sheet's formats and PNG sizes; `None` for the defaults.
    #[serde(deserialize_with = "lenient")]
    pub export: Option<Formats>,
    /// The Minify tab's controls; `None` for the defaults.
    #[serde(deserialize_with = "lenient")]
    pub minify: Option<MinifySettings>,
    /// What the Minify tab shows drawings on: `auto`, `light` or `dark`.
    #[serde(deserialize_with = "lenient")]
    pub minify_backdrop: String,
    /// The Minify tab's arrangement: `side`, `wipe` or `ab`.
    #[serde(deserialize_with = "lenient")]
    pub minify_view: String,
    /// The Fabricate tab's request, less what belongs to one drawing (its colours and
    /// their order); `None` for the defaults. Checked against `inkvec_fab::Options`.
    #[serde(deserialize_with = "lenient")]
    pub fab: Option<serde_json::Value>,
    /// Lengths in `mm` or `in`.
    #[serde(deserialize_with = "lenient")]
    pub fab_unit: String,
    /// Whether the preflight's problems are drawn over the sheets.
    #[serde(deserialize_with = "lenient_true")]
    pub fab_show_problems: bool,
    /// The material preset last chosen.
    #[serde(deserialize_with = "lenient")]
    pub fab_preset: Option<String>,
    /// Whether saving also writes a DXF.
    #[serde(deserialize_with = "lenient")]
    pub fab_dxf: bool,
    /// Whether saving also writes G-code.
    #[serde(deserialize_with = "lenient")]
    pub fab_gcode: bool,
    /// The batch queue's preset (the desktop's Batch tab).
    #[serde(deserialize_with = "lenient")]
    pub batch_preset: Option<String>,
    /// Whether a batch skips images whose SVG already exists.
    #[serde(deserialize_with = "lenient_true")]
    pub batch_skip_existing: bool,
    /// Whether a batch's table sorts failures first.
    #[serde(deserialize_with = "lenient")]
    pub batch_failures_first: bool,
}

impl Default for Interface {
    fn default() -> Self {
        Self {
            // The interface opens on the Logo preset, the default; a file that remembers
            // "no preset" (a control was moved) says so with an explicit null.
            preset: Some("logo".into()),
            tab: "vectorize".into(),
            rail_tab: "result".into(),
            groups_open: BTreeMap::new(),
            view: "side".into(),
            wipe: 0.5,
            show: Layers::default(),
            detail: false,
            export: None,
            minify: None,
            minify_backdrop: "auto".into(),
            minify_view: "side".into(),
            fab: None,
            fab_unit: "mm".into(),
            fab_show_problems: true,
            fab_preset: None,
            fab_dxf: false,
            fab_gcode: false,
            batch_preset: None,
            batch_skip_existing: true,
            batch_failures_first: false,
        }
    }
}

/// How many Tune groups the fold state is kept for, and how long a name may be.
const GROUPS_LIMIT: usize = 16;
const NAME_LIMIT: usize = 64;
/// How many PNG sizes an export remembers.
const PNG_SIZES_LIMIT: usize = 8;

/// `value` if it is one of `allowed`, else the first of them.
fn one_of(value: &mut String, allowed: &[&str]) {
    if !allowed.contains(&value.as_str()) {
        *value = allowed[0].to_string();
    }
}

/// An id or name worth keeping: trimmed, not empty, not absurdly long.
fn short(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().count() <= NAME_LIMIT)
}

fn finite_or(v: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}

impl Interface {
    /// Every field in a range and a vocabulary the interface can act on.
    pub fn sanitised(mut self) -> Self {
        self.preset = short(self.preset);
        one_of(
            &mut self.tab,
            &["vectorize", "minify", "fabricate", "batch"],
        );
        one_of(&mut self.rail_tab, &["result", "tune"]);
        self.groups_open = std::mem::take(&mut self.groups_open)
            .into_iter()
            .filter(|(k, _)| !k.is_empty() && k.chars().count() <= NAME_LIMIT)
            .take(GROUPS_LIMIT)
            .collect();
        one_of(&mut self.view, &["side", "wipe", "ab"]);
        self.wipe = finite_or(self.wipe, 0.0, 1.0, 0.5);
        if let Some(f) = &mut self.export {
            f.png_sizes.retain(|s| (16..=8192).contains(s));
            f.png_sizes.sort_unstable();
            f.png_sizes.dedup();
            f.png_sizes.truncate(PNG_SIZES_LIMIT);
        }
        if let Some(m) = &mut self.minify {
            let d = MinifySettings::default();
            m.tolerance_px = finite_or(m.tolerance_px, 0.0, 5.0, d.tolerance_px);
            m.judge_px = finite_or(m.judge_px, 16.0, 16384.0, d.judge_px);
            m.corner_degrees = finite_or(m.corner_degrees, 1.0, 179.0, d.corner_degrees);
        }
        one_of(&mut self.minify_backdrop, &["auto", "light", "dark"]);
        one_of(&mut self.minify_view, &["side", "wipe", "ab"]);
        self.fab = self.fab.take().and_then(fab_options);
        one_of(&mut self.fab_unit, &["mm", "in"]);
        self.fab_preset = short(self.fab_preset);
        self.batch_preset = short(self.batch_preset);
        self
    }
}

/// The Fabricate request as the engine reads it, without the two fields that name one
/// drawing's colours; `None` when it is not one the engine reads at all.
fn fab_options(v: serde_json::Value) -> Option<serde_json::Value> {
    let parsed: inkvec_fab::Options = serde_json::from_value(v).ok()?;
    let mut out = serde_json::to_value(parsed).ok()?;
    let map = out.as_object_mut()?;
    map.remove("include");
    map.remove("order");
    Some(out)
}

/// Where the desktop window was, in the window system's own (physical) pixels, the one
/// coordinate space all monitors share, and whether it was maximised. The rectangle is the
/// last one it had while neither maximised nor minimised, so un-maximising after a restart
/// goes back to it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPlace {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub maximized: bool,
}

/// The window's own minimum (`tauri.conf.json`), and a ceiling well past any real screen.
const MIN_W: f64 = 1024.0;
const MIN_H: f64 = 640.0;
const MAX_SIDE: f64 = 16384.0;
/// How much of the window's top edge must land on a screen for it to be put back there:
/// enough of the title bar to grab.
const GRAB_W: f64 = 160.0;
const GRAB_H: f64 = 40.0;

impl WindowPlace {
    /// A rectangle the window can take, or `None` for one it cannot (not finite).
    pub fn sanitised(self) -> Option<Self> {
        let all = [self.x, self.y, self.width, self.height];
        if !all.iter().all(|v| v.is_finite()) {
            return None;
        }
        Some(Self {
            x: self.x.clamp(-MAX_SIDE, MAX_SIDE),
            y: self.y.clamp(-MAX_SIDE, MAX_SIDE),
            width: self.width.clamp(MIN_W, MAX_SIDE),
            height: self.height.clamp(MIN_H, MAX_SIDE),
            maximized: self.maximized,
        })
    }

    /// Whether enough of the window's top edge is on one of `screens` (each `x, y, width,
    /// height`, in the same pixels) to grab it. A window last seen on a monitor that has since
    /// been unplugged is not put back off-screen; it opens centred instead.
    pub fn reachable(&self, screens: &[(f64, f64, f64, f64)]) -> bool {
        screens.iter().any(|&(sx, sy, sw, sh)| {
            let left = self.x.max(sx);
            let right = (self.x + self.width).min(sx + sw);
            let top = self.y.max(sy);
            let bottom = (self.y + GRAB_H).min(sy + sh);
            right - left >= GRAB_W && bottom - top >= GRAB_H * 0.5
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_value_resets_its_own_field_and_nothing_else() {
        let ui: Interface = serde_json::from_str(
            r#"{"tab": "fabricate", "view": 7, "railTab": "sideways", "show": {"wireframe": true},
                "wipe": "half", "somethingNew": [1, 2]}"#,
        )
        .expect("a bad field is not a parse failure");
        let ui = ui.sanitised();
        assert_eq!(ui.tab, "fabricate");
        assert_eq!(
            ui.view, "side",
            "a number where a name was expected: the default"
        );
        assert_eq!(ui.rail_tab, "result", "an unknown name: the default");
        assert_eq!(ui.wipe, 0.5);
        assert!(
            ui.show.wireframe && ui.show.fill,
            "a partial object keeps the rest default"
        );
    }

    #[test]
    fn numbers_and_names_are_clamped() {
        let ui = Interface {
            wipe: 7.0,
            preset: Some("   ".into()),
            fab_preset: Some("x".repeat(200)),
            export: Some(Formats {
                png_sizes: vec![4096, 3, 512, 512, 99_999],
                ..Formats::default()
            }),
            minify: Some(MinifySettings {
                tolerance_px: f64::NAN,
                judge_px: 1.0,
                corner_degrees: 500.0,
                document_cleanup: false,
            }),
            ..Interface::default()
        }
        .sanitised();
        assert_eq!(ui.wipe, 1.0);
        assert_eq!(ui.preset, None);
        assert_eq!(ui.fab_preset, None);
        assert_eq!(ui.export.unwrap().png_sizes, vec![512, 4096]);
        let m = ui.minify.unwrap();
        assert_eq!(m.tolerance_px, MinifySettings::default().tolerance_px);
        assert_eq!((m.judge_px, m.corner_degrees), (16.0, 179.0));
        assert!(!m.document_cleanup);
    }

    #[test]
    fn the_fabricate_request_keeps_its_settings_but_not_one_drawings_colours() {
        let ui = Interface {
            fab: Some(serde_json::json!({
                "mode": "stencil", "widthMm": 250.0, "include": ["#ff0000"], "order": ["#ff0000"],
                "unknownKnob": 3
            })),
            ..Interface::default()
        }
        .sanitised();
        let fab = ui.fab.expect("a request the engine reads is kept");
        assert_eq!(fab["widthMm"], 250.0);
        assert_eq!(fab["mode"], "stencil");
        assert!(fab.get("include").is_none() && fab.get("order").is_none());
        assert!(fab.get("unknownKnob").is_none());
        let junk = Interface {
            fab: Some(serde_json::json!("not a request")),
            ..Interface::default()
        };
        assert_eq!(junk.sanitised().fab, None);
    }

    #[test]
    fn a_list_keeps_the_entries_it_can_read() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(deserialize_with = "lenient_list")]
            items: Vec<u32>,
        }
        let h: Holder = serde_json::from_str(r#"{"items": [1, "two", 3]}"#).unwrap();
        assert_eq!(h.items, vec![1, 3]);
        let h: Holder = serde_json::from_str(r#"{"items": "none"}"#).unwrap();
        assert!(h.items.is_empty());
    }

    #[test]
    fn a_window_is_put_back_only_where_it_can_be_reached() {
        let screen = [(0.0, 0.0, 1920.0, 1080.0)];
        let on = WindowPlace {
            x: 100.0,
            y: 50.0,
            width: 1440.0,
            height: 900.0,
            maximized: false,
        };
        assert!(on.reachable(&screen));
        // On a second monitor that is no longer there.
        let gone = WindowPlace { x: 2200.0, ..on };
        assert!(!gone.reachable(&screen));
        // Mostly off the left edge, but a grabbable strip of title bar still on it.
        let edge = WindowPlace { x: -1200.0, ..on };
        assert!(edge.reachable(&screen));
        // Title bar above the top of the screen.
        let above = WindowPlace { y: -500.0, ..on };
        assert!(!above.reachable(&screen));
        // Two monitors side by side: the right-hand one.
        let two = [(0.0, 0.0, 1920.0, 1080.0), (1920.0, 0.0, 2560.0, 1440.0)];
        assert!(gone.reachable(&two));
    }

    #[test]
    fn a_window_too_small_or_not_finite_is_corrected_or_dropped() {
        let tiny = WindowPlace {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            maximized: true,
        };
        let fixed = tiny.sanitised().unwrap();
        assert_eq!(
            (fixed.width, fixed.height, fixed.maximized),
            (MIN_W, MIN_H, true)
        );
        assert_eq!(
            WindowPlace {
                x: f64::NAN,
                ..tiny
            }
            .sanitised(),
            None
        );
    }
}
