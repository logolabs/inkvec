//! A [`Restore`] that shells out to a command.
//!
//! For a machine that would rather run the PyTorch checkpoint, or any other restorer, than a
//! build with an in-process runtime. The contract mirrors `inkvec_sr::external::External`: the
//! command is handed an input PNG and an output path, and is expected to write a same-size
//! restored PNG. `{in}` and `{out}` in the argument list are substituted with the two paths.
//! The per-run temp folder, the command runner and the choice of Python interpreter are the
//! SR pre-pass's, so both external backends clean up and report failures the same way.

use std::path::{Path, PathBuf};

use inkvec_sr::external::{python_program, run_command, RunDir};

use crate::Restore;

/// Restore by running a command that reads one PNG and writes a same-size restored PNG.
#[derive(Debug, Clone)]
pub struct External {
    /// The program to run.
    pub program: String,
    /// Its arguments; `{in}` and `{out}` are replaced with the input and output PNG paths.
    pub args: Vec<String>,
    /// Working directory for the command, if it needs one.
    pub work_dir: Option<PathBuf>,
}

impl External {
    /// The training repository's restorer CLI (`restore_cli.py`), asked to restore one PNG.
    pub fn python(training_repo: &Path, checkpoint: &Path) -> Self {
        Self {
            program: python_program(),
            args: vec![
                "restore_cli.py".to_string(),
                "{in}".to_string(),
                "{out}".to_string(),
                "--ckpt".to_string(),
                checkpoint.to_string_lossy().to_string(),
            ],
            work_dir: Some(training_repo.to_path_buf()),
        }
    }
}

/// Write interleaved RGB (`0..1`) to `path` as an 8-bit RGB PNG, each value clamped and
/// rounded to the nearest level. Fails when the buffer does not hold exactly
/// `3·width·height` values or the file cannot be written.
fn write_png(
    rgb: &[f32],
    width: usize,
    height: usize,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let buf: Vec<u8> = rgb
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let img = image::RgbImage::from_raw(width as u32, height as u32, buf)
        .ok_or("could not build the PNG buffer")?;
    img.save(path)?;
    Ok(())
}

impl Restore for External {
    /// Round-trip through the file system in a fresh temporary folder: `in.png` out, the
    /// command run, `out.png` read back. A result of a different size is an error; any alpha
    /// the command wrote is dropped. The values come back on the 8-bit grid, which is the
    /// recipe the in-process backends reproduce with their own quantisation.
    fn restore(
        &self,
        rgb: &[f32],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        let dir = RunDir::new("inkvec-restore")?;
        let src = dir.path().join("in.png");
        let dst = dir.path().join("out.png");
        write_png(rgb, width, height, &src)?;
        run_command(
            "restorer",
            &self.program,
            &self.args,
            self.work_dir.as_deref(),
            &src,
            &dst,
        )?;
        let restored = inkvec_trace::load_image(&dst)?;

        if (restored.width, restored.height) != (width, height) {
            return Err(format!(
                "external restorer returned {}x{}, expected {}x{}",
                restored.width, restored.height, width, height
            )
            .into());
        }
        let n = width * height;
        let mut rgb_out = vec![0f32; n * 3];
        for i in 0..n {
            for c in 0..3 {
                rgb_out[i * 3 + c] = restored.data[i * 4 + c];
            }
        }
        Ok(rgb_out)
    }

    fn describe(&self) -> String {
        format!("external via `{}`", self.program)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_python_command_runs_restore_cli_in_the_training_repo() {
        let e = External::python(Path::new("repo"), Path::new("ck.pt"));
        assert_eq!(e.args, ["restore_cli.py", "{in}", "{out}", "--ckpt", "ck.pt"]);
        assert_eq!(e.work_dir.as_deref(), Some(Path::new("repo")));
        assert!(e.describe().starts_with("external via"));
    }

    #[test]
    fn a_missing_command_is_an_error() {
        let e = External {
            program: "inkvec-restore-test-no-such-program".into(),
            args: vec!["{in}".into(), "{out}".into()],
            work_dir: None,
        };
        let err = e.restore(&[0.5; 12], 2, 2).expect_err("cannot start");
        assert!(err.to_string().contains("could not run the external restorer"));
    }

    #[test]
    fn a_png_is_written_on_the_eight_bit_grid() {
        let dir = RunDir::new("inkvec-restore-test").expect("temp dir");
        let path = dir.path().join("x.png");
        write_png(&[0.0, 0.5, 1.2, -1.0, 1.0, 0.25], 2, 1, &path).expect("writes");
        let back = inkvec_trace::load_image(&path).expect("reads");
        let bytes: Vec<u8> = back.data.iter().map(|v| (v * 255.0).round() as u8).collect();
        assert_eq!(bytes, [0, 128, 255, 255, 0, 255, 64, 255]);
        assert!(write_png(&[0.0; 5], 2, 1, &path).is_err(), "short buffer");
    }
}
