//! The two desktop integrations the Advanced settings offer: the `inkvec` command on the
//! path, and "Vectorize with Inkvec" in the file manager's context menu.
//!
//! Both are **per-user**, always. Nothing here asks for an administrator, writes outside
//! the user's own directories, or touches a machine-wide registry hive — an app that
//! demands elevation to add a convenience has misjudged the trade, and a free tool that
//! asks for it twice will not be installed a third time. Both are reversible from the
//! same screen that offered them, and removing one leaves nothing behind.
//!
//! The command that gets linked is the `inkvec` binary shipped beside the app, built from
//! this same tree by the same release. A trace started from the terminal and a trace
//! started from the window are then the same engine at the same version, which is the
//! only version of this feature worth having.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Where a desktop integration stands.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Whether this build and platform can offer it at all.
    pub available: bool,
    /// Whether it is in place now.
    pub installed: bool,
    /// Where it lives, or would.
    pub path: Option<PathBuf>,
    /// What to tell the user when `available` is false.
    pub note: Option<String>,
}

impl Status {
    fn unavailable(note: &str) -> Self {
        Self {
            available: false,
            installed: false,
            path: None,
            note: Some(note.to_string()),
        }
    }
}

// --------------------------------------------------------------- the command ---

/// The name the command has on this platform.
pub fn cli_name() -> &'static str {
    if cfg!(windows) {
        "inkvec.exe"
    } else {
        "inkvec"
    }
}

/// The `inkvec` binary shipped beside the app.
///
/// Tauri puts an external binary next to the app's own executable and strips the target
/// triple from its name, so this is where it lands on every platform. A build without the
/// sidecar returns `None` and the Settings row says so rather than offering something
/// that would fail.
pub fn bundled_cli() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok()?.parent()?.join(cli_name());
    beside.is_file().then_some(beside)
}

/// Where the command is put so a shell will find it.
///
/// `~/.local/bin` on Unix — on the PATH of every current distribution and of macOS once
/// a user has ever installed anything with pipx or Homebrew, and harmless if not. On
/// Windows, the per-user `WindowsApps` folder, which is on the PATH of a default install
/// and needs no registry edit.
pub fn cli_target() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        dirs::data_local_dir().map(|d| d.join("Microsoft").join("WindowsApps").join(cli_name()))
    }
    #[cfg(not(windows))]
    {
        dirs::home_dir().map(|d| d.join(".local").join("bin").join(cli_name()))
    }
}

/// Whether the command is on the path, and where it would go.
pub fn cli_status() -> Status {
    let Some(target) = cli_target() else {
        return Status::unavailable("there is nowhere on this system to put it");
    };
    match bundled_cli() {
        None => Status {
            available: false,
            installed: target.exists(),
            path: Some(target),
            note: Some("this build does not ship the inkvec command".into()),
        },
        Some(_) => Status {
            available: true,
            installed: target.exists(),
            path: Some(target),
            note: None,
        },
    }
}

/// Put `inkvec` where the shell will find it.
///
/// A symlink on Unix, so an app update updates the command with it; a copy on Windows,
/// which has no dependable unprivileged symlink.
pub fn install_cli() -> Result<Status, String> {
    let source =
        bundled_cli().ok_or_else(|| "this build does not ship the inkvec command".to_string())?;
    let target = cli_target().ok_or_else(|| "there is nowhere to put it".to_string())?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let _ = std::fs::remove_file(&target);

    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, &target)
        .map_err(|e| format!("cannot link {}: {e}", target.display()))?;
    #[cfg(not(unix))]
    std::fs::copy(&source, &target)
        .map(|_| ())
        .map_err(|e| format!("cannot copy to {}: {e}", target.display()))?;

    Ok(cli_status())
}

/// Take it off the path again.
pub fn remove_cli() -> Result<Status, String> {
    if let Some(target) = cli_target() {
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot remove {}: {e}", target.display())),
        }
    }
    Ok(cli_status())
}

// ------------------------------------------------------------- context menu ---

/// The image types the context-menu entry is offered on.
const IMAGE_TYPES: [&str; 6] = [".png", ".jpg", ".jpeg", ".webp", ".bmp", ".tif"];

/// Whether "Vectorize with Inkvec" is in the file manager's menu.
pub fn context_menu_status() -> Status {
    #[cfg(windows)]
    {
        windows_menu::status()
    }
    #[cfg(target_os = "linux")]
    {
        let path = linux_menu_path();
        Status {
            available: path.is_some(),
            installed: path.as_ref().is_some_and(|p| p.exists()),
            path,
            note: None,
        }
    }
    #[cfg(target_os = "macos")]
    {
        // macOS puts "Open With" in the Finder's menu from the app's declared document
        // types, which this app already has — there is nothing to add and nothing that
        // could be added without a Services bundle the user never asked for.
        Status::unavailable("macOS already offers Inkvec Studio under Finder's Open With")
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Status::unavailable("not available on this platform")
    }
}

/// Add the entry.
pub fn install_context_menu(app_exe: &Path) -> Result<Status, String> {
    #[cfg(windows)]
    {
        windows_menu::install(app_exe)?;
    }
    #[cfg(target_os = "linux")]
    {
        linux_menu_install(app_exe)?;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app_exe;
        return Err("macOS offers this through Finder's Open With already".into());
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = app_exe;
        return Err("not available on this platform".into());
    }
    #[allow(unreachable_code)]
    Ok(context_menu_status())
}

/// Take it out again, leaving nothing behind.
pub fn remove_context_menu() -> Result<Status, String> {
    #[cfg(windows)]
    {
        windows_menu::remove()?;
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(path) = linux_menu_path() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot remove {}: {e}", path.display())),
            }
        }
    }
    Ok(context_menu_status())
}

// -- Linux: a desktop entry, which is what every file manager reads --------------

#[cfg(target_os = "linux")]
fn linux_menu_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| {
        d.join("applications")
            .join("inkvec-studio-vectorize.desktop")
    })
}

#[cfg(target_os = "linux")]
fn linux_menu_install(app_exe: &Path) -> Result<(), String> {
    let path = linux_menu_path().ok_or_else(|| "there is no data directory".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mime: Vec<String> = IMAGE_TYPES
        .iter()
        .map(|e| match *e {
            ".jpg" | ".jpeg" => "image/jpeg".to_string(),
            ".tif" => "image/tiff".to_string(),
            other => format!("image/{}", other.trim_start_matches('.')),
        })
        .collect();
    // `NoDisplay` keeps it out of the applications menu: it is an action offered on a
    // file, not a second copy of the app in the launcher.
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Vectorize with Inkvec\n\
         Comment=Trace this image to SVG with Inkvec Studio\n\
         Exec={} %f\n\
         Icon=inkvec-studio\n\
         Terminal=false\n\
         NoDisplay=true\n\
         MimeType={};\n\
         Categories=Graphics;\n",
        app_exe.display(),
        mime.join(";")
    );
    std::fs::write(&path, entry).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

// -- Windows: per-user file associations, HKCU only ------------------------------

#[cfg(windows)]
mod windows_menu {
    use super::{Status, IMAGE_TYPES};
    use std::path::{Path, PathBuf};
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    /// Where the entry lives for one extension. `SystemFileAssociations` is the hive that
    /// adds a verb without taking over the file type, which is what we want: this offers
    /// an action, it does not claim to be the default handler for PNG.
    fn verb_path(ext: &str) -> String {
        format!("Software\\Classes\\SystemFileAssociations\\{ext}\\shell\\InkvecStudio")
    }

    pub(super) fn status() -> Status {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let installed = hkcu
            .open_subkey_with_flags(verb_path(IMAGE_TYPES[0]), KEY_READ)
            .is_ok();
        Status {
            available: true,
            installed,
            path: Some(PathBuf::from(format!(
                "HKCU\\{}",
                verb_path(IMAGE_TYPES[0])
            ))),
            note: None,
        }
    }

    pub(super) fn install(app_exe: &Path) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for ext in IMAGE_TYPES {
            let (verb, _) = hkcu
                .create_subkey_with_flags(verb_path(ext), KEY_WRITE)
                .map_err(|e| format!("cannot write the registry for {ext}: {e}"))?;
            verb.set_value("", &"Vectorize with Inkvec")
                .map_err(|e| format!("cannot name the entry: {e}"))?;
            verb.set_value("Icon", &format!("{},0", app_exe.display()))
                .map_err(|e| format!("cannot set the entry's icon: {e}"))?;
            let (command, _) = verb
                .create_subkey_with_flags("command", KEY_WRITE)
                .map_err(|e| format!("cannot write the command: {e}"))?;
            command
                .set_value("", &format!("\"{}\" \"%1\"", app_exe.display()))
                .map_err(|e| format!("cannot write the command: {e}"))?;
        }
        Ok(())
    }

    pub(super) fn remove() -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for ext in IMAGE_TYPES {
            match hkcu.delete_subkey_all(verb_path(ext)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot remove the registry entry for {ext}: {e}")),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_is_named_for_its_platform() {
        assert_eq!(
            cli_name(),
            if cfg!(windows) {
                "inkvec.exe"
            } else {
                "inkvec"
            }
        );
    }

    #[test]
    fn the_command_goes_somewhere_inside_the_users_own_directories() {
        let Some(target) = cli_target() else { return };
        let home = dirs::home_dir().expect("a home directory");
        assert!(
            target.starts_with(&home),
            "{} is outside {}",
            target.display(),
            home.display()
        );
    }

    #[test]
    fn a_build_without_the_sidecar_says_so_rather_than_offering_it() {
        let s = cli_status();
        if bundled_cli().is_none() {
            assert!(!s.available);
            assert!(s.note.is_some());
        }
    }

    #[test]
    fn removing_something_that_is_not_there_is_not_an_error() {
        // Nothing is installed in a test environment; both must still succeed.
        assert!(remove_cli().is_ok());
        assert!(remove_context_menu().is_ok());
    }

    #[test]
    fn the_context_menu_reports_a_platform_it_cannot_serve() {
        let s = context_menu_status();
        if !s.available {
            assert!(s.note.is_some(), "an unavailable integration must say why");
            assert!(!s.installed);
        }
    }

    #[test]
    fn every_offered_extension_is_one_the_tracer_reads() {
        for ext in IMAGE_TYPES {
            assert!(ext.starts_with('.'), "{ext}");
            assert!(
                matches!(ext, ".png" | ".jpg" | ".jpeg" | ".webp" | ".bmp" | ".tif"),
                "{ext} is not a format the decoder accepts"
            );
        }
    }
}
