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
