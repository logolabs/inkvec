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
