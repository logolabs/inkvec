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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the operating system.
    #[default]
    System,
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
    /// Whether the first-run screen has been seen.
    pub seen_first_run: bool,
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
        }
    }
}

/// How many recent files are kept. Enough to be useful, few enough that the menu is a
/// menu and not a history.
const RECENT_LIMIT: usize = 8;

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
        self.trace = self.trace.sanitised();
        self.recent.truncate(RECENT_LIMIT);
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

/// The file preferences are kept in.
pub fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("inkvec-studio").join("preferences.json"))
}

/// Read preferences, falling back to the defaults for anything missing or unreadable.
pub fn load() -> Prefs {
    let Some(p) = path() else {
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
    let p = path().ok_or_else(|| "there is no config directory on this system".to_string())?;
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
    if let Some(p) = path() {
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
        assert_eq!(parsed.theme, Theme::System);
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
    fn preferences_round_trip_through_json() {
        let mut p = Prefs::default();
        p.theme = Theme::Light;
        p.trace.precision = 0.05;
        p.remember(std::path::Path::new("/tmp/a.png"));
        let text = serde_json::to_string(&p).unwrap();
        let back: Prefs = serde_json::from_str(&text).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn a_missing_file_is_not_a_reason_to_fail_a_reset() {
        assert!(reset().is_ok());
    }
}
