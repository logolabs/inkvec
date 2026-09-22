//! The optional denoiser: what it is, where it goes, and how it gets there.
//!
//! One download, explained honestly. The model repairs JPEG and screenshot damage before
//! tracing, which is why the Photo-or-scan preset wants it, and it is about 80 MB, which
//! is why it is not bundled. Everything about it still runs on this machine: the model
//! comes down, the image never goes up.
//!
//! Nothing here invents its own facts. The repository, the URL, the expected SHA-256 and
//! the cache location all come from `inkvec_restore`, which is the same set the command
//! line and the browser build check against — so a model this app installs is a model the
//! `inkvec` binary beside it will also accept, and vice versa.

use std::io::{Read, Write};
use std::path::PathBuf;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Where the denoiser stands right now.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Whether this build can run the model at all. A build without the `denoiser`
    /// feature has no ONNX Runtime linked in, and says so rather than offering a download
    /// that would achieve nothing.
    pub supported: bool,
    /// Whether the weights are on disk and hash to the published value.
    pub installed: bool,
    /// Where they are, or where they would go.
    pub path: Option<PathBuf>,
    /// Their size on disk, in bytes, if present.
    pub bytes: Option<u64>,
    /// The Hugging Face repository they come from.
    pub repo: &'static str,
    /// The SHA-256 the download is checked against.
    pub sha256: &'static str,
}

/// Whether this build has the restorer compiled in.
pub const SUPPORTED: bool = cfg!(feature = "denoiser");

/// Where the weights live for this user.
pub fn weights_path() -> Option<PathBuf> {
    inkvec_restore::user_cache_model_path()
}

/// What to tell the Settings screen and the "denoiser missing" state.
pub fn status() -> Status {
    let path = weights_path();
    let meta = path.as_ref().and_then(|p| std::fs::metadata(p).ok());
    let installed = match (&path, &meta) {
        (Some(p), Some(m)) if m.is_file() => verify(p).is_ok(),
        _ => false,
    };
    Status {
        supported: SUPPORTED,
        installed,
        bytes: meta.as_ref().filter(|_| installed).map(|m| m.len()),
        path,
        repo: inkvec_restore::HF_DENOISER_REPO,
        sha256: inkvec_restore::WEIGHTS_SHA256,
    }
}

/// Check a file against the published digest.
pub fn verify(path: &std::path::Path) -> Result<(), String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual == inkvec_restore::WEIGHTS_SHA256 {
        Ok(())
    } else {
        Err(format!(
            "the file that arrived is not the published model: SHA-256 {actual}, expected {}",
            inkvec_restore::WEIGHTS_SHA256
        ))
    }
}

/// Download the weights, reporting progress, and install them only once they verify.
///
/// Written to a `.part` beside the destination and renamed at the end, so an interrupted
/// download leaves nothing that looks installed. `should_stop` is polled between chunks:
/// the modal's Cancel is a real cancel here, unlike a trace's, because a download has
/// somewhere to stop.
pub fn download(
    on_progress: impl Fn(u64, Option<u64>),
    should_stop: impl Fn() -> bool,
) -> Result<PathBuf, String> {
    let dest = weights_path().ok_or_else(|| {
        "there is no cache directory to install the denoiser into on this system".to_string()
    })?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let part = dest.with_extension("part");
    let _ = std::fs::remove_file(&part);

    let response = ureq::get(inkvec_restore::HF_DENOISER_URL)
        .call()
        .map_err(|e| format!("cannot reach {}: {e}", inkvec_restore::HF_DENOISER_REPO))?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(&part)
        .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    let mut buf = vec![0u8; 1 << 18];
    let mut got = 0u64;
    loop {
        if should_stop() {
            drop(file);
            let _ = std::fs::remove_file(&part);
            return Err("cancelled".into());
        }
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("the download stopped early: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
        got += n as u64;
        on_progress(got, total);
    }
    file.flush()
        .map_err(|e| format!("cannot finish writing: {e}"))?;
    drop(file);

    if let Err(why) = verify(&part) {
        let _ = std::fs::remove_file(&part);
        return Err(why);
    }
    std::fs::rename(&part, &dest).map_err(|e| format!("cannot move the model into place: {e}"))?;
    Ok(dest)
}

/// Remove the installed weights.
pub fn remove() -> Result<(), String> {
    remove_at(weights_path())
}

/// [`remove`] against an explicit file, so a test can exercise it on a temporary one. The
/// test suite must never call `remove()` itself: that deletes the model installed for
/// whoever runs it.
fn remove_at(file: Option<PathBuf>) -> Result<(), String> {
    let Some(path) = file else {
        return Ok(());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove {}: {e}", path.display())),
    }
}

/// Point the engine at the installed weights for the rest of this process.
///
/// `inkvec_restore` reads `INKVEC_RESTORE_ONNX` when no explicit path is given, which is
/// the seam the command line uses too. Setting it here means a trace started from the app
/// finds the same model a trace started from the terminal would.
pub fn announce_to_engine() {
    if let Some(path) = weights_path() {
        if path.is_file() && std::env::var_os("INKVEC_RESTORE_ONNX").is_none() {
            // Safety: called once during setup, before any worker thread exists.
            unsafe { std::env::set_var("INKVEC_RESTORE_ONNX", &path) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_quotes_the_engines_own_repository_and_digest() {
        let s = status();
        assert_eq!(s.repo, inkvec_restore::HF_DENOISER_REPO);
        assert_eq!(s.sha256, inkvec_restore::WEIGHTS_SHA256);
        assert_eq!(s.sha256.len(), 64, "a SHA-256 is 64 hex characters");
        assert_eq!(s.supported, cfg!(feature = "denoiser"));
    }

    #[test]
    fn a_file_that_is_not_the_model_is_refused_by_name() {
        let path = std::env::temp_dir().join(format!("inkvec-fake-model-{}", std::process::id()));
        std::fs::write(&path, b"not the model").unwrap();
        let err = verify(&path).unwrap_err();
        assert!(err.contains("not the published model"), "{err}");
        assert!(err.contains(inkvec_restore::WEIGHTS_SHA256), "{err}");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn removing_something_that_is_not_there_is_not_an_error() {
        let missing =
            std::env::temp_dir().join(format!("inkvec-no-such-model-{}", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        assert!(remove_at(Some(missing)).is_ok());
    }

    #[test]
    fn removing_deletes_the_file_it_is_given() {
        let file = std::env::temp_dir().join(format!("inkvec-model-to-remove-{}", std::process::id()));
        std::fs::write(&file, b"weights").unwrap();
        assert!(remove_at(Some(file.clone())).is_ok());
        assert!(!file.exists(), "the file should be gone");
    }

    #[test]
    fn nothing_is_reported_as_installed_without_a_matching_file() {
        let s = status();
        if !s.installed {
            assert!(
                s.bytes.is_none(),
                "a size was reported for a model that is not there"
            );
        }
    }
}
