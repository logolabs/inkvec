//! Preferences: what is remembered between sessions, and where.
//!
//! One JSON file in the platform's own config directory. It holds preferences and a list
//! of recently opened paths — nothing about the images themselves, no thumbnails and no
//! contents, because a file that survives the session should not be a record of what
//! somebody traced.
//!
//! Every field has a default, and an unreadable or half-written file falls back to those
//! defaults rather than refusing to start: a corrupt preferences file is not a reason to
//! lose the app.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::options::Settings as TraceSettings;

/// Which theme the window follows.
///
/// Dark is the default rather than `System`, and that is a product decision rather than a
/// preference: this is a viewer, the stage is a dark ground so artwork reads against it,
/// and a first run that opened light on a machine set to light would show the app at its
/// least convincing. Light is fully supported and one click away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the operating system.
    System,
    #[default]
    Dark,
    Light,
}

/// Which builds the update check looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Stable,
    Prerelease,
}

/// What opening an image does besides the automatic trace, which always starts at once.
///
/// `Ask` shows a small card over the stage offering Auto or the Custom wizard; ignoring it
/// is Auto. The other two remember an answer, and are changed back in Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnOpen {
    #[default]
    Ask,
    /// Trace automatically and show nothing else.
    Auto,
    /// Trace automatically and open the wizard over it.
    Custom,
}

/// A preset the user saved from the advanced controls.
///
/// The built-in eight are each a small set of deliberate differences from the defaults,
/// which is why a batch row's preset only overrides what that preset speaks to. A saved
/// one is the opposite: a whole snapshot, because whoever pressed Save had already moved
/// exactly what they wanted moved, and "everything else follows the queue" would quietly
/// undo half of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPreset {
    /// Stable across a rename, so the rail's selection survives one.
    pub id: String,
    pub name: String,
    pub settings: TraceSettings,
}

/// Everything remembered between sessions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    // ---- General ----
    /// Where Export opens first.
    pub output_folder: Option<PathBuf>,
    /// Dark, light, or whatever the system says.
    pub theme: Theme,

    // ---- Performance ----
    /// Worker threads the tracer may use. `None` means every core.
    pub threads: Option<usize>,
    /// The longer side a draft is traced at.
    pub draft_px: u32,
    /// The time limit a draft is given, in seconds.
    pub draft_seconds: f64,
    /// How long the controls must be still before the full trace is queued, in
    /// milliseconds.
    pub settle_ms: u64,

    // ---- Updates ----
    /// Whether to look for a new version at startup.
    pub check_updates: bool,
    /// Which builds to look at.
    pub channel: Channel,

    // ---- Working state worth keeping ----
    /// The last trace settings, so the app opens where it was left.
    pub trace: TraceSettings,
    /// Recently opened files, newest first. Paths only.
    pub recent: Vec<PathBuf>,
    /// Whether the first-run introduction to the wizard has been seen.
    pub seen_first_run: bool,
    /// What opening an image offers besides the automatic trace.
    pub on_open: OnOpen,
    /// Presets the user saved, in the order the tray shows them.
    pub saved: Vec<SavedPreset>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            output_folder: None,
            theme: Theme::default(),
            threads: None,
            // 512 px and 0.4 s: the draft has to feel live while a slider is moving, and
            // a draft that overruns is abandoned rather than shown late.
            draft_px: 512,
            draft_seconds: 0.4,
            settle_ms: 800,
            check_updates: true,
            channel: Channel::default(),
            trace: TraceSettings::default(),
            recent: Vec::new(),
            seen_first_run: false,
            on_open: OnOpen::default(),
            saved: Vec::new(),
        }
    }
}

/// How many recent files are kept. Enough to be useful, few enough that the menu is a
/// menu and not a history.
const RECENT_LIMIT: usize = 8;

/// How many saved presets are kept. The tray is a tray, not a filing cabinet; past this
/// many the eight built-ins stop being findable, which is the thing they are for.
const SAVED_LIMIT: usize = 12;

/// The longest a saved preset's name may be. Long enough for a client and a job, short
/// enough to read on a chip.
const NAME_LIMIT: usize = 40;

impl Prefs {
    /// Clamp anything that came out of the file into a range the app can act on.
    pub fn sanitised(mut self) -> Self {
        self.draft_px = self.draft_px.clamp(128, 2048);
        self.draft_seconds = if self.draft_seconds.is_finite() {
            self.draft_seconds.clamp(0.05, 10.0)
        } else {
            0.4
        };
        self.settle_ms = self.settle_ms.clamp(100, 5_000);
        self.threads = self.threads.map(|t| t.clamp(1, 256));
        self.trace = self.trace.sanitised().for_keeping();
        self.recent.truncate(RECENT_LIMIT);

        // A saved preset with no name or a duplicate id would show as a blank tile or as
        // two tiles that both answer to one click, so neither survives the file.
        let mut seen = std::collections::HashSet::new();
        self.saved.retain_mut(|p| {
            p.name = p.name.trim().chars().take(NAME_LIMIT).collect();
            p.settings = p.settings.clone().sanitised().for_keeping();
            !p.id.is_empty() && !p.name.is_empty() && seen.insert(p.id.clone())
        });
        self.saved.truncate(SAVED_LIMIT);
        self
    }

    /// Put `path` at the top of the recent list, without duplicating it.
    pub fn remember(&mut self, path: &std::path::Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(RECENT_LIMIT);
    }

    /// Drop recent entries whose file is no longer there, so the menu does not offer
    /// something that cannot be opened.
    pub fn prune_recent(&mut self) {
        self.recent.retain(|p| p.is_file());
    }
}

/// An environment variable naming the directory preferences are kept in instead of the
/// system's config directory (`preferences.json` goes straight inside it). `dirs` does not
/// read `APPDATA` or `XDG_CONFIG_HOME` overrides on every platform, so without this a
/// smoke test of a built app would read and write the real preferences of whoever runs it.
pub const CONFIG_DIR_ENV: &str = "INKVEC_STUDIO_CONFIG_DIR";

/// The file preferences are kept in.
pub fn path() -> Option<PathBuf> {
    path_from(std::env::var_os(CONFIG_DIR_ENV), dirs::config_dir())
}

/// [`path`] from its two inputs: the override, if set and not empty, else the system's
/// config directory.
fn path_from(dir: Option<std::ffi::OsString>, config_dir: Option<PathBuf>) -> Option<PathBuf> {
    match dir.filter(|d| !d.is_empty()) {
        Some(d) => Some(PathBuf::from(d).join("preferences.json")),
        None => config_dir.map(|d| d.join("inkvec-studio").join("preferences.json")),
    }
}

/// Read preferences, falling back to the defaults for anything missing or unreadable.
pub fn load() -> Prefs {
    load_at(path())
}

/// [`load`] from an explicit file.
fn load_at(file: Option<PathBuf>) -> Prefs {
    let Some(p) = file else {
        return Prefs::default();
    };
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Prefs::default();
    };
    serde_json::from_str::<Prefs>(&text)
        .unwrap_or_default()
        .sanitised()
}

/// Write preferences, creating the directory if it is not there.
pub fn save(prefs: &Prefs) -> Result<(), String> {
    save_at(path(), prefs)
}

/// [`save`] to an explicit file. Like [`reset_at`], this is what tests call: `save()`
/// itself would overwrite the preferences of whoever runs the suite.
fn save_at(file: Option<PathBuf>, prefs: &Prefs) -> Result<(), String> {
    let p = file.ok_or_else(|| "there is no config directory on this system".to_string())?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(prefs)
        .map_err(|e| format!("cannot write preferences: {e}"))?;
    // Written beside and renamed, so an interrupted write cannot leave a truncated file
    // that the next start would throw away.
    let tmp = p.with_extension("json.part");
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("cannot replace {}: {e}", p.display()))
}

/// Forget everything. The Advanced group's "Reset settings".
pub fn reset() -> Result<Prefs, String> {
    reset_at(path())
}

/// [`reset`] against an explicit file, so a test can exercise it on a temporary one. The
/// test suite must never call `reset()` itself: that deletes the real preferences of
/// whoever runs it.
fn reset_at(file: Option<PathBuf>) -> Result<Prefs, String> {
    if let Some(p) = file {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot remove {}: {e}", p.display())),
        }
    }
    Ok(Prefs::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_corrupt_file_falls_back_to_the_defaults() {
        let parsed: Prefs = serde_json::from_str("{ this is not json").unwrap_or_default();
        assert_eq!(parsed, Prefs::default());
    }

    #[test]
    fn a_file_from_an_older_version_keeps_what_it_recognises() {
        // One field this version still has, one it no longer does, and everything else
        // absent: the first survives, the second is ignored, the rest take their defaults.
        let parsed: Prefs = serde_json::from_str(r#"{"draftPx": 256, "somethingRemoved": true}"#)
            .expect("an unknown field is not a parse failure");
        assert_eq!(parsed.draft_px, 256);
        assert_eq!(parsed.theme, Theme::Dark, "dark is the product default");
        assert!(parsed.recent.is_empty());
    }

    #[test]
    fn nonsense_numbers_are_clamped_rather_than_obeyed() {
        let p = Prefs {
            draft_px: 99_999,
            draft_seconds: f64::INFINITY,
            settle_ms: 0,
            threads: Some(0),
            ..Prefs::default()
        }
        .sanitised();
        assert_eq!(p.draft_px, 2048);
        assert_eq!(p.draft_seconds, 0.4);
        assert_eq!(p.settle_ms, 100);
        assert_eq!(p.threads, Some(1));
    }

    #[test]
    fn the_recent_list_has_no_duplicates_and_a_ceiling() {
        let mut p = Prefs::default();
        for i in 0..12 {
            p.remember(std::path::Path::new(&format!("/tmp/logo-{i}.png")));
        }
        p.remember(std::path::Path::new("/tmp/logo-3.png"));
        assert_eq!(p.recent.len(), RECENT_LIMIT);
        assert_eq!(p.recent[0], PathBuf::from("/tmp/logo-3.png"));
        assert_eq!(
            p.recent
                .iter()
                .filter(|x| x.ends_with("logo-3.png"))
                .count(),
            1
        );
    }

    #[test]
    fn a_saved_preset_that_cannot_be_shown_does_not_survive_the_file() {
        let one = |id: &str, name: &str| SavedPreset {
            id: id.into(),
            name: name.into(),
            settings: TraceSettings::default(),
        };
        let p = Prefs {
            saved: vec![
                one("a", "  Northwind  "),
                one("b", "   "),
                one("", "No id"),
                one("a", "A second tile answering to the same click"),
                one("c", &"x".repeat(NAME_LIMIT + 20)),
            ],
            ..Prefs::default()
        }
        .sanitised();

        let ids: Vec<&str> = p.saved.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(
            ids,
            ["a", "c"],
            "blank, id-less and duplicate tiles are dropped"
        );
        assert_eq!(p.saved[0].name, "Northwind", "names are trimmed");
        assert_eq!(p.saved[1].name.chars().count(), NAME_LIMIT);
    }

    #[test]
    fn saved_presets_have_a_ceiling_and_go_through_the_same_clamps() {
        let p = Prefs {
            saved: (0..SAVED_LIMIT + 5)
                .map(|i| SavedPreset {
                    id: format!("p{i}"),
                    name: format!("Preset {i}"),
                    settings: TraceSettings {
                        trace_size: 99_999,
                        ..TraceSettings::default()
                    },
                })
                .collect(),
            ..Prefs::default()
        }
        .sanitised();
        assert_eq!(p.saved.len(), SAVED_LIMIT);
        assert_eq!(
            p.saved[0].settings.trace_size, 16_384,
            "a saved preset's numbers are clamped like any live ones"
        );
    }

    #[test]
    fn colour_groups_are_never_written_to_preferences_or_presets() {
        let groups = vec![crate::options::ColourGroup {
            members: vec!["#111111".into(), "#222222".into()],
            target: None,
        }];
        let with_groups = TraceSettings {
            colour_groups: groups,
            ..TraceSettings::default()
        };
        let p = Prefs {
            trace: with_groups.clone(),
            saved: vec![SavedPreset {
                id: "a".into(),
                name: "Northwind".into(),
                settings: with_groups,
            }],
            ..Prefs::default()
        }
        .sanitised();
        assert!(p.trace.colour_groups.is_empty());
        assert!(p.saved[0].settings.colour_groups.is_empty());
        assert!(!serde_json::to_string(&p).unwrap().contains("colourGroups"));
    }

    #[test]
    fn opening_an_image_asks_until_told_otherwise() {
        assert_eq!(Prefs::default().on_open, OnOpen::Ask);
        // A file written before the choice existed asks too.
        let old: Prefs = serde_json::from_str(r#"{"seenFirstRun": true}"#).unwrap();
        assert_eq!(old.on_open, OnOpen::Ask);
        assert!(old.seen_first_run);
        let custom: Prefs = serde_json::from_str(r#"{"onOpen": "custom"}"#).unwrap();
        assert_eq!(custom.on_open, OnOpen::Custom);
        assert!(serde_json::to_string(&Prefs::default())
            .unwrap()
            .contains(r#""onOpen":"ask""#));
    }

    #[test]
    fn preferences_round_trip_through_json() {
        let mut p = Prefs {
            theme: Theme::Light,
            ..Prefs::default()
        };
        p.trace.precision = 0.05;
        p.on_open = OnOpen::Auto;
        p.seen_first_run = true;
        p.remember(std::path::Path::new("/tmp/a.png"));
        let text = serde_json::to_string(&p).unwrap();
        let back: Prefs = serde_json::from_str(&text).unwrap();
        assert_eq!(back, p);
    }

    /// A path in the temp directory, never the user's real preferences.
    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("inkvec-{name}-{}.json", std::process::id()))
    }

    #[test]
    fn the_config_dir_override_holds_the_preferences_file_directly() {
        let system = Some(PathBuf::from("system-config"));
        assert_eq!(
            path_from(Some("smoke-prefs".into()), system.clone()),
            Some(PathBuf::from("smoke-prefs").join("preferences.json"))
        );
        // Unset or empty: the system's directory, as before.
        let default = Some(
            PathBuf::from("system-config")
                .join("inkvec-studio")
                .join("preferences.json"),
        );
        assert_eq!(path_from(None, system.clone()), default);
        assert_eq!(path_from(Some("".into()), system), default);
        assert_eq!(path_from(None, None), None);
    }

    #[test]
    fn preferences_round_trip_through_an_overridden_directory() {
        let dir = std::env::temp_dir().join(format!("inkvec-config-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = path_from(Some(dir.clone().into_os_string()), None);
        let prefs = Prefs {
            draft_px: 256,
            ..Prefs::default()
        };
        save_at(file.clone(), &prefs).expect("the directory is created and the file written");
        assert_eq!(load_at(file), prefs.clone().sanitised());
        assert!(dir.join("preferences.json").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_not_a_reason_to_fail_a_reset() {
        let missing = scratch("no-such-prefs");
        let _ = std::fs::remove_file(&missing);
        assert!(reset_at(Some(missing)).is_ok());
    }

    #[test]
    fn a_reset_removes_the_file_it_is_given_and_returns_the_defaults() {
        let file = scratch("prefs-to-reset");
        std::fs::write(&file, b"{}").unwrap();
        assert_eq!(reset_at(Some(file.clone())).unwrap(), Prefs::default());
        assert!(!file.exists(), "the file should be gone");
    }
}
