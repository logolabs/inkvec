//! The restorer network through ONNX Runtime: the fast CPU path.
//!
//! Reads the same `.onnx` export Burn generates its code from, so the two runtimes cannot
//! disagree about the network; only the kernels differ. ONNX Runtime's are multithreaded
//! throughout (element-wise ops included) and fuse convolution layouts for the CPU, which is
//! the gap to Burn's CPU backends on this network.

use std::error::Error;
use std::path::Path;
use std::sync::Mutex;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;

use crate::planar::{check_input, from_planar_cropped, padded, to_planar_padded};

/// Name of the network's input in the export (`export_restorer_onnx.py`).
const INPUT: &str = "image";
/// Name of the network's output in the export.
const OUTPUT: &str = "restored";

/// The restorer network in an ONNX Runtime session.
pub struct OnnxRestorer {
    // `Session::run` takes `&mut self`; `Restore::restore` takes `&self`.
    session: Mutex<Session>,
}

impl std::fmt::Debug for OnnxRestorer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnnxRestorer").finish_non_exhaustive()
    }
}

impl OnnxRestorer {
    /// Load the ONNX export and build a CPU session for it.
    pub fn load(onnx: &Path) -> Result<Self, Box<dyn Error>> {
        if !onnx.is_file() {
            return Err(format!("restorer ONNX file not found: {}", onnx.display()).into());
        }
        let mut builder = Session::builder()
            .map_err(|e| e.to_string())?
            .with_optimization_level(GraphOptimizationLevel::All)
            .map_err(|e| e.to_string())?;
        let session = builder
            .commit_from_file(onnx)
            .map_err(|e| format!("loading {}: {e}", onnx.display()))?;
        Ok(Self {
            session: Mutex::new(session),
        })
    }

    /// Restore straight RGB in `[0, 1]`, row-major, 3 floats per pixel. Same size out.
    pub fn restore(
        &self,
        rgb: &[f32],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, Box<dyn Error>> {
        check_input(rgb, width, height)?;
        let (pw, ph) = padded(width, height);
        let planar = to_planar_padded(rgb, width, height, pw, ph);
        let input = TensorRef::from_array_view(([1usize, 3, ph, pw], planar.as_slice()))
            .map_err(|e| e.to_string())?;
        let mut session = self
            .session
            .lock()
            .map_err(|_| "restorer session lock poisoned")?;
        let outputs = session
            .run(ort::inputs![INPUT => input])
            .map_err(|e| format!("restorer inference: {e}"))?;
        let (_, y) = outputs[OUTPUT]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("reading restorer output: {e}"))?;
        if y.len() != 3 * ph * pw {
            return Err(format!(
                "restorer output is {} floats, want {}",
                y.len(),
                3 * ph * pw
            )
            .into());
        }
        Ok(from_planar_cropped(y, width, height, pw, ph))
    }
}

impl crate::Restore for OnnxRestorer {
    fn restore(
        &self,
        rgb: &[f32],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, Box<dyn Error>> {
        OnnxRestorer::restore(self, rgb, width, height)
    }

    fn describe(&self) -> String {
        "restorer network (ONNX Runtime, CPU)".into()
    }
}
