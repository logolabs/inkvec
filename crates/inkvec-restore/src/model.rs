//! The restorer network, in-process, on any Burn backend (Burn 0.22: the backend is part of the
//! [`Device`] the weights are loaded onto, not a type parameter).
//!
//! The network code is generated at build time by burn-onnx from the ONNX export (`build.rs`),
//! which `export_restorer_onnx.py` checks against the PyTorch model before writing. Nothing here
//! re-implements a layer, so there is no hand port to drift from the trained model.
//!
//! The model lives on its own worker thread with a large stack. The generated code builds and
//! runs the network in a handful of very large functions (one per partitioned submodule), which
//! overflow the 1 MB main-thread stack Windows gives a process -- measured, on both CPU backends,
//! before the weights had finished loading. Only plain float buffers cross between threads, so
//! nothing here needs the Burn model to be `Send` or `Sync`.

use std::error::Error;
use std::path::Path;
use std::sync::mpsc;
use std::thread;

use burn::prelude::{Tensor, TensorData};
use burn::tensor::Device;

use crate::planar::{check_input, from_planar_cropped, padded, to_planar_padded};

#[allow(
    missing_docs,
    unreachable_pub,
    unused_qualifications,
    dead_code,
    clippy::all
)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/model/restorer.rs"));
}

/// Where the build wrote the weights it converted from the ONNX export. Valid only on the
/// machine that built the binary; anywhere else, point [`Restorer::load`] at a copy of the file.
pub const BUILD_WEIGHTS: &str = concat!(env!("OUT_DIR"), "/model/restorer.bpk");

/// Stack reserved for the worker thread. Address space only: pages are committed as used.
const WORKER_STACK: usize = 256 << 20;

/// Where the worker sends one job's result: the restored interleaved RGB, or why it failed.
type Reply = mpsc::Sender<Result<Vec<f32>, String>>;
/// One inference request: interleaved RGB, its width and height, and where to answer.
type Job = (Vec<f32>, usize, usize, Reply);

/// The restorer network on one Burn device, running on a dedicated worker thread.
#[derive(Debug)]
pub struct Restorer {
    jobs: Option<mpsc::Sender<Job>>,
    worker: Option<thread::JoinHandle<()>>,
    device: String,
}

impl Restorer {
    /// Load the weights (a `.bpk` file written by the build) onto `device`.
    pub fn load(weights: &Path, device: Device) -> Result<Self, Box<dyn Error>> {
        // The generated loader panics on a missing file; check first so a wrong path is an
        // error the caller can report. A corrupt file still panics, on the worker, and comes
        // back as an error below.
        if !weights.is_file() {
            return Err(format!("restorer weights not found: {}", weights.display()).into());
        }
        let path = weights.to_path_buf();
        let label = format!("{device:?}");
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
        let worker = thread::Builder::new()
            .name("inkvec-restore".into())
            .stack_size(WORKER_STACK)
            .spawn(move || {
                let net = generated::Model::from_file(&path, &device);
                if ready_tx.send(()).is_err() {
                    return;
                }
                for (rgb, width, height, reply) in jobs_rx {
                    // The caller may have given up waiting; nothing to do about that here.
                    let _ = reply.send(forward(&net, &device, &rgb, width, height));
                }
            })?;
        if ready_rx.recv().is_err() {
            let _ = worker.join();
            return Err(
                format!("could not load restorer weights from {}", weights.display()).into(),
            );
        }
        Ok(Self {
            jobs: Some(jobs_tx),
            worker: Some(worker),
            device: label,
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
        let jobs = self.jobs.as_ref().ok_or("restorer has been shut down")?;
        let (reply_tx, reply_rx) = mpsc::channel();
        jobs.send((rgb.to_vec(), width, height, reply_tx))
            .map_err(|_| "restorer worker has stopped")?;
        let out = reply_rx
            .recv()
            .map_err(|_| "restorer worker panicked during inference")?;
        Ok(out?)
    }
}

impl Drop for Restorer {
    fn drop(&mut self) {
        // Closing the job channel ends the worker's loop.
        self.jobs.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl crate::Restore for Restorer {
    fn restore(
        &self,
        rgb: &[f32],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, Box<dyn Error>> {
        Restorer::restore(self, rgb, width, height)
    }

    fn describe(&self) -> String {
        let name = self
            .device
            .split(['(', '{', ' '])
            .next()
            .unwrap_or(&self.device);
        format!("restorer network (Burn {name})")
    }
}

/// One inference, on the worker thread: pad to the network's multiple, run the whole image
/// as a single `1 x 3 x ph x pw` tensor, check the output has the same shape, and crop back to
/// interleaved RGB at the input size. The values are returned as the network produced them;
/// quantisation and snapping happen in the crate root, the same for every backend.
fn forward(
    net: &generated::Model,
    device: &Device,
    rgb: &[f32],
    width: usize,
    height: usize,
) -> Result<Vec<f32>, String> {
    let (pw, ph) = padded(width, height);
    let planar = to_planar_padded(rgb, width, height, pw, ph);
    let x = Tensor::<4>::from_data(TensorData::new(planar, [1, 3, ph, pw]), device);
    let y = net
        .forward(x)
        .into_data()
        .try_into_vec::<f32>()
        .map_err(|e| format!("reading restorer output: {e:?}"))?;
    if y.len() != 3 * ph * pw {
        return Err(format!(
            "restorer output is {} floats, want {}",
            y.len(),
            3 * ph * pw
        ));
    }
    Ok(from_planar_cropped(&y, width, height, pw, ph))
}
