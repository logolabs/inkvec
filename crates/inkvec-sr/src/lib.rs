//! An optional super-resolution pre-pass for inkvec.
//!
//! For input that has been through something -- JPEG, a screenshot, a resize, a
//! chat app -- upscaling by four and halving back removes about two thirds of
//! the damage, which turns this tracer's worst input condition into a win. On
//! JPEG q50 it takes dE00 from 1.0010 to 0.4381 and the parameter count from
//! 19.81x the artist's to 6.78x, beating Potrace on every column.
//!
//! On **clean** input the same pre-pass is three times worse than tracing
//! directly, which is why it is a mode and not a default, and why [`Mode::Auto`]
//! exists: it traces once, asks the trace how well it fits the input, and only
//! reaches for the network when the answer is "badly".
//!
//! Everything here except the upscaler itself is plain arithmetic and ships in
//! the binary unconditionally. The upscaler is an [`Upscaler`] implementation;
//! today that is [`external::External`], which runs the packaged Python network
//! or any command, and the mode logic does not know the difference.
//! Measurements: see `docs/algorithm/01-intake.md`.

pub mod clean;
pub mod detect;
pub mod external;

use inkvec_trace::Rgba;

/// When to clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Trace, measure the fit, clean and retrace only if the fit is bad.
    #[default]
    Auto,
    /// Always clean first.
    On,
    /// Never clean. Identical to not having the mode.
    Off,
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Mode::Auto),
            "on" => Ok(Mode::On),
            "off" => Ok(Mode::Off),
            other => Err(format!("unknown SR mode {other:?}; want auto, on or off")),
        }
    }
}

/// Something that can upscale an image by a fixed integer factor.
pub trait Upscaler {
    /// The fixed integer factor this upscaler multiplies each dimension by.
    fn scale(&self) -> usize;

    /// Upscale straight-alpha RGBA in [0, 1] by [`Upscaler::scale`].
    fn upscale(&self, img: &Rgba) -> Result<Rgba, Box<dyn std::error::Error>>;

    /// A one-line description for logging.
    fn describe(&self) -> String;
}

/// Options for [`prepass`].
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Output scale relative to the input.
    pub out_scale: usize,
    /// Put flat colours back on the source's values.
    pub recolour: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            out_scale: 2,
            recolour: true,
        }
    }
}

/// Upscale, halve, and put the colours back.
pub fn prepass(
    up: &dyn Upscaler,
    img: &Rgba,
    opt: Options,
) -> Result<Rgba, Box<dyn std::error::Error>> {
    let scale = up.scale();
    if opt.out_scale == 0 || !scale.is_multiple_of(opt.out_scale) {
        return Err(format!(
            "cannot produce x{} from an x{scale} upscaler",
            opt.out_scale
        )
        .into());
    }

    // The network is RGB-only, and a fully transparent pixel's colour channels
    // hold whatever the encoder left there. Zero them so they cannot drag their
    // neighbours, and fit the recolour map against the same zeroed source rather
    // than the raw file -- otherwise the map is fitted on pixels that never
    // entered the upscale.
    let mut src = img.clone();
    for i in 0..src.width * src.height {
        if src.data[i * 4 + 3] <= 1e-4 {
            for c in 0..3 {
                src.data[i * 4 + c] = 0.0;
            }
        }
    }

    let mut hi = up.upscale(&src)?;
    let factor = scale / opt.out_scale;
    if factor > 1 {
        hi = clean::box_downsample(&hi, factor);
    }
    if opt.recolour {
        clean::match_flats(&mut hi, &src);
    }
    Ok(hi)
}

/// What [`decide`] concluded, and why.
#[derive(Debug, Clone, Copy)]
pub enum Decision {
    /// Clean it.
    Clean {
        /// The interior residual that triggered cleaning.
        residual: Option<f64>,
    },
    /// Leave it alone, and keep the trace already produced.
    Keep {
        /// The interior residual measured, or `None` when the trace could not be rendered or
        /// had too little flat area to judge.
        residual: Option<f64>,
    },
}

/// Decide whether a traced result is good enough to keep.
///
/// `svg` is the probe trace of `img`. Returns [`Decision::Keep`] when the model
/// fits the input where it claims to be flat, and [`Decision::Clean`] when it
/// does not. A trace with too little flat area to judge is kept: "cannot tell"
/// must not become "clean it", because cleaning is the expensive mistake.
pub fn decide(img: &Rgba, svg: &str, threshold: f64) -> Decision {
    let Ok(model) = detect::render_svg(svg, img.width, img.height) else {
        return Decision::Keep { residual: None };
    };
    match detect::interior_residual(img, &model) {
        Some(r) if r > threshold => Decision::Clean { residual: Some(r) },
        r => Decision::Keep { residual: r },
    }
}
