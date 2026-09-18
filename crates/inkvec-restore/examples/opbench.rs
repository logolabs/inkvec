//! Per-op CPU timing of the layer types the restorer runs, at the full-resolution sizes it runs
//! them for a 512 px input (the most expensive stage), to find where a backend loses time.
//!
//!     cargo run --release -p inkvec-restore --features flex,ndarray --example opbench -- <flex|ndarray>
//!
//! Counts per forward pass at full resolution, for turning these into a total: 4 CDC blocks
//! (each: 7x7 depthwise on 48, 1x1 48->144, SiLU on 144, SPRN on 144, 1x1 144->48, RMSNorm
//! on 48), one 3x3 fuse 144->48, one bilinear 2x resize of 96 channels, stem and head convs.

use std::time::Instant;

use burn::nn::conv::Conv2dConfig;
use burn::nn::interpolate::{Interpolate2dConfig, InterpolateMode};
use burn::nn::PaddingConfig2d;
use burn::prelude::{Backend, Tensor};
use burn::tensor::Distribution;

const S: usize = 512;

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "flex".into());
    match which.as_str() {
        #[cfg(feature = "flex")]
        "flex" => run::<burn::backend::Flex>(),
        #[cfg(feature = "ndarray")]
        "ndarray" => run::<burn::backend::NdArray>(),
        other => eprintln!("backend {other:?} is not compiled in"),
    }
}

fn time<B: Backend>(name: &str, f: impl Fn() -> Tensor<B, 4>) {
    drop(f());
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        let y = f();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        drop(y);
        best = best.min(ms);
    }
    println!("{name:44} {best:8.1} ms");
}

fn run<B: Backend>() {
    let d = B::Device::default();
    let u = || Distribution::Uniform(-1.0, 1.0);
    let x3 = Tensor::<B, 4>::random([1, 3, S, S], u(), &d);
    let x48 = Tensor::<B, 4>::random([1, 48, S, S], u(), &d);
    let x144 = Tensor::<B, 4>::random([1, 144, S, S], u(), &d);
    let y144 = Tensor::<B, 4>::random([1, 144, S, S], u(), &d);
    let per_c = Tensor::<B, 4>::random([1, 144, 1, 1], u(), &d);
    let per_px = Tensor::<B, 4>::random([1, 1, S, S], Distribution::Uniform(0.5, 1.5), &d);
    let x96h = Tensor::<B, 4>::random([1, 96, S / 2, S / 2], u(), &d);

    let pad = |k: usize| PaddingConfig2d::Explicit(k, k, k, k);
    let stem = Conv2dConfig::new([3, 48], [3, 3])
        .with_padding(pad(1))
        .init::<B>(&d);
    let fuse = Conv2dConfig::new([144, 48], [3, 3])
        .with_padding(pad(1))
        .init::<B>(&d);
    let dw7 = Conv2dConfig::new([48, 48], [7, 7])
        .with_padding(pad(3))
        .with_groups(48)
        .with_bias(false)
        .init::<B>(&d);
    let pw_up = Conv2dConfig::new([48, 144], [1, 1]).init::<B>(&d);
    let pw_down = Conv2dConfig::new([144, 48], [1, 1]).init::<B>(&d);
    let sprn = Conv2dConfig::new([144, 144], [3, 3])
        .with_padding(pad(1))
        .with_groups(144)
        .with_bias(false)
        .init::<B>(&d);
    let down = Conv2dConfig::new([48, 96], [2, 2])
        .with_stride([2, 2])
        .init::<B>(&d);
    let resize = Interpolate2dConfig::new()
        .with_scale_factor(Some([2.0, 2.0]))
        .with_mode(InterpolateMode::Linear)
        .with_align_corners(false)
        .init();

    println!("backend {}", std::any::type_name::<B>());
    time("conv 3x3 3->48 (stem)", || stem.forward(x3.clone()));
    time("conv 3x3 144->48 (fuse)", || fuse.forward(x144.clone()));
    time("depthwise 7x7 48", || dw7.forward(x48.clone()));
    time("conv 1x1 48->144", || pw_up.forward(x48.clone()));
    time("conv 1x1 144->48", || pw_down.forward(x144.clone()));
    time("grouped 3x3 144 (SPRN)", || sprn.forward(x144.clone()));
    time("conv 2x2 s2 48->96 (down)", || down.forward(x48.clone()));
    time("resize bilinear 2x, 96 ch", || resize.forward(x96h.clone()));
    time("cat 48 + 96 ch", || {
        Tensor::cat(vec![x48.clone(), x48.clone().repeat_dim(1, 2)], 1)
    });
    time("mul 144 ch, same shape", || x144.clone().mul(y144.clone()));
    time("mul 144 ch by per-channel", || {
        x144.clone().mul(per_c.clone())
    });
    time("div 144 ch by per-pixel", || {
        x144.clone().div(per_px.clone())
    });
    time("add 144 ch, same shape", || x144.clone().add(y144.clone()));
    time("sqrt 144 ch", || x144.clone().abs().sqrt());
    time("sigmoid 144 ch", || {
        burn::tensor::activation::sigmoid(x144.clone())
    });
    time("mean over channels, 144 ch", || x144.clone().mean_dim(1));
}
