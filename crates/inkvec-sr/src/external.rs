//! An [`Upscaler`] that shells out to a command.
//!
//! The upscaler network runs out of process: the packaged Python pre-pass under
//! `tools/inkvec_sr`, or any command that honours the same contract. The command is handed an
//! input PNG and an output path, and is expected to write an image `scale` times larger.
//!
//! `{in}` and `{out}` in the argument list are replaced with the two paths. The helpers here
//! ([`RunDir`], [`run_command`], [`python_program`]) are shared with the restorer's external
//! backend, so both pre-passes clean up and report failures the same way.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use inkvec_trace::Rgba;

use crate::Upscaler;

/// An [`Upscaler`] that shells out to a command, substituting `{in}` and `{out}` in
/// its argument list with the input and output PNG paths.
#[derive(Debug, Clone)]
pub struct External {
    /// The command to run.
    pub program: String,
    /// Its arguments, with `{in}` and `{out}` substituted for the temporary PNG paths.
    pub args: Vec<String>,
    /// The integer factor the command is expected to upscale by.
    pub scale: usize,
    /// Working directory the command runs in, if it needs one.
    pub work_dir: Option<PathBuf>,
}

impl External {
    /// The packaged Python pre-pass, asked for the raster only.
    pub fn python(tools_dir: &Path, scale: usize) -> Self {
        Self {
            program: python_program(),
            args: [
                "-m",
                "inkvec_sr",
                "{in}",
                "-o",
                "{out}",
                "--png-only",
                "--mode",
                "on",
                "--scale",
            ]
            .iter()
            .map(|s| s.to_string())
            .chain(std::iter::once(scale.to_string()))
            .collect(),
            scale,
            work_dir: Some(tools_dir.to_path_buf()),
        }
    }
}

/// The Python interpreter packaged tools run with: `INKVEC_PYTHON` when set, else `python` on
/// Windows and `python3` elsewhere, where many systems install no bare `python`.
pub fn python_program() -> String {
    std::env::var("INKVEC_PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "python".to_string()
        } else {
            "python3".to_string()
        }
    })
}

/// A working directory for one external run, removed when dropped.
///
/// Named after a prefix, the process id and a per-process counter, so two runs never share
/// files and a failed run leaves nothing behind in the temp folder.
#[derive(Debug)]
pub struct RunDir(PathBuf);

impl RunDir {
    /// Create a fresh, empty directory under the system temp folder.
    pub fn new(prefix: &str) -> std::io::Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `program` with `{in}` and `{out}` substituted by `src` and `dst`, and explain a failure:
/// the program could not start, it exited with an error (its own stderr is quoted), or it
/// exited successfully without writing `dst`. `what` names the pre-pass in the messages.
pub fn run_command(
    what: &str,
    program: &str,
    args: &[String],
    work_dir: Option<&Path>,
    src: &Path,
    dst: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let subst = |s: &String| -> String {
        s.replace("{in}", &src.to_string_lossy())
            .replace("{out}", &dst.to_string_lossy())
    };
    let mut cmd = Command::new(program);
    cmd.args(args.iter().map(subst));
    if let Some(d) = work_dir {
        cmd.current_dir(d);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("could not run the external {what} `{program}`: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "external {what} `{program}` failed ({}): {}",
            out.status,
            err.trim().chars().take(400).collect::<String>()
        )
        .into());
    }
    if !dst.is_file() {
        return Err(format!(
            "external {what} `{program}` exited successfully but did not write {}",
            dst.display()
        )
        .into());
    }
    Ok(())
}

/// Write `img` to `path` as an 8-bit straight-alpha PNG, each channel clamped to `0..1` and
/// rounded to the nearest level. Fails if the data length does not match the dimensions or
/// the file cannot be written.
fn write_png(img: &Rgba, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let buf: Vec<u8> = img
        .data
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let rgba = image::RgbaImage::from_raw(img.width as u32, img.height as u32, buf)
        .ok_or("could not build the PNG buffer")?;
    rgba.save(path)?;
    Ok(())
}

impl Upscaler for External {
    fn scale(&self) -> usize {
        self.scale
    }

    /// Round-trip through the file system: write `img` as `in.png` in a fresh [`RunDir`], run
    /// the command, read `out.png` back, and check it is exactly `scale` times the input in
    /// each dimension. The run directory is removed on every way out.
    fn upscale(&self, img: &Rgba) -> Result<Rgba, Box<dyn std::error::Error>> {
        let dir = RunDir::new("inkvec-sr")?;
        let src = dir.path().join("in.png");
        let dst = dir.path().join("out.png");
        write_png(img, &src)?;
        run_command(
            "upscaler",
            &self.program,
            &self.args,
            self.work_dir.as_deref(),
            &src,
            &dst,
        )?;
        let up = inkvec_trace::load_image(&dst)?;

        let want = (img.width * self.scale, img.height * self.scale);
        if (up.width, up.height) != want {
            return Err(format!(
                "external upscaler returned {}x{}, expected {}x{}",
                up.width, up.height, want.0, want.1
            )
            .into());
        }
        Ok(up)
    }

    fn describe(&self) -> String {
        format!("external x{} via `{}`", self.scale, self.program)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program name no system has on its path.
    const MISSING: &str = "inkvec-sr-test-no-such-program";

    #[test]
    fn the_python_command_asks_for_the_raster_at_the_given_scale() {
        let e = External::python(Path::new("tools"), 4);
        assert_eq!(e.scale, 4);
        assert_eq!(e.work_dir.as_deref(), Some(Path::new("tools")));
        assert_eq!(e.args[..2], ["-m", "inkvec_sr"]);
        assert!(e.args.contains(&"{in}".to_string()) && e.args.contains(&"{out}".to_string()));
        assert!(e.args.contains(&"--png-only".to_string()));
        assert_eq!(e.args.last().map(String::as_str), Some("4"));
        assert!(e.describe().starts_with("external x4 via"));
    }

    #[test]
    fn a_run_dir_is_fresh_and_removed_on_drop() {
        let a = RunDir::new("inkvec-sr-test").expect("temp dir");
        let b = RunDir::new("inkvec-sr-test").expect("temp dir");
        assert_ne!(a.path(), b.path(), "two runs never share a directory");
        assert!(a.path().is_dir());
        let kept = a.path().to_path_buf();
        drop(a);
        assert!(!kept.exists(), "dropping the run removes its files");
    }

    #[test]
    fn a_program_that_cannot_start_is_reported_by_name() {
        let dir = RunDir::new("inkvec-sr-test").expect("temp dir");
        let (src, dst) = (dir.path().join("in.png"), dir.path().join("out.png"));
        let err = run_command("upscaler", MISSING, &[], None, &src, &dst)
            .expect_err("the program does not exist");
        let msg = err.to_string();
        assert!(msg.contains("could not run the external upscaler") && msg.contains(MISSING));
    }

    #[test]
    fn upscale_fails_cleanly_when_the_command_is_missing() {
        let e = External {
            program: MISSING.to_string(),
            args: vec!["{in}".into(), "{out}".into()],
            scale: 2,
            work_dir: None,
        };
        let img = Rgba {
            width: 2,
            height: 2,
            data: vec![0.5; 16],
        };
        assert!(e.upscale(&img).is_err());
    }

    #[test]
    fn a_png_round_trips_through_the_file_the_command_is_handed() {
        let dir = RunDir::new("inkvec-sr-test").expect("temp dir");
        let path = dir.path().join("in.png");
        // Out-of-range values are clamped before quantising.
        let img = Rgba {
            width: 2,
            height: 1,
            data: vec![0.0, 1.0, 1.5, 1.0, -0.5, 0.5, 128.0 / 255.0, 0.25],
        };
        write_png(&img, &path).expect("writes");
        let back = inkvec_trace::load_image(&path).expect("reads");
        assert_eq!((back.width, back.height), (2, 1));
        let bytes: Vec<u8> = back
            .data
            .iter()
            .map(|v| (v * 255.0).round() as u8)
            .collect();
        assert_eq!(bytes, [0, 255, 255, 255, 0, 128, 128, 64]);
        let short = Rgba {
            width: 4,
            height: 4,
            data: vec![0.0; 4],
        };
        assert!(
            write_png(&short, &path).is_err(),
            "a short buffer is an error"
        );
    }
}
