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

    /// A path in the temp directory, never the user's real preferences.
    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("inkvec-{name}-{}.json", std::process::id()))
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
