//! Preferences: where they are kept on this computer.
//!
//! One JSON file in the platform's own config directory. It holds preferences and a list
//! of recently opened paths -- nothing about the images themselves, no thumbnails and no
//! contents, because a file that survives the session should not be a record of what
//! somebody traced. The model (fields, defaults, clamps) is shared with the web build and
//! lives in `inkvec_studio_core::prefs`; this module is only the file.
//!
//! Every field has a default, and an unreadable or half-written file falls back to those
//! defaults rather than refusing to start: a corrupt preferences file is not a reason to
//! lose the app.

use std::path::PathBuf;

pub use inkvec_studio_core::prefs::*;

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

/// The Advanced group's "Reset settings": every preference back to its default and the
/// recent list cleared, the saved presets kept ([`Prefs::after_reset`]).
pub fn reset() -> Result<Prefs, String> {
    reset_at(path())
}

/// [`reset`] against an explicit file, so a test can exercise it on a temporary one. The
/// test suite must never call `reset()` itself: that rewrites the real preferences of
/// whoever runs it.
///
/// With no saved presets to keep, the file is removed, as a reset always did; otherwise it
/// is rewritten with the defaults and the presets.
fn reset_at(file: Option<PathBuf>) -> Result<Prefs, String> {
    let kept = load_at(file.clone()).after_reset();
    if kept.saved.is_empty() {
        if let Some(p) = file {
            match std::fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot remove {}: {e}", p.display())),
            }
        }
        return Ok(kept);
    }
    save_at(file, &kept)?;
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// "Reset settings" used to delete the preferences file whole, and the saved presets
    /// with it, though its confirmation names only the preferences and the recent list.
    #[test]
    fn a_reset_keeps_the_saved_presets() {
        let file = scratch("prefs-with-presets");
        let saved = vec![SavedPreset {
            id: "mine".into(),
            name: "Our house style".into(),
            settings: inkvec_studio_core::options::Settings::default(),
        }];
        let used = Prefs {
            draft_px: 256,
            recent: vec![PathBuf::from("logo.png")],
            saved: saved.clone(),
            ..Prefs::default()
        };
        save_at(Some(file.clone()), &used).unwrap();
        let reset = reset_at(Some(file.clone())).unwrap();
        assert_eq!(reset.saved, saved, "the presets are kept");
        assert_eq!(
            reset.draft_px,
            Prefs::default().draft_px,
            "the preferences are not"
        );
        assert!(reset.recent.is_empty());
        assert_eq!(
            load_at(Some(file.clone())),
            reset,
            "and that is what the next start reads"
        );
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn a_reset_removes_the_file_it_is_given_and_returns_the_defaults() {
        let file = scratch("prefs-to-reset");
        std::fs::write(&file, b"{}").unwrap();
        assert_eq!(reset_at(Some(file.clone())).unwrap(), Prefs::default());
        assert!(!file.exists(), "the file should be gone");
    }
}
