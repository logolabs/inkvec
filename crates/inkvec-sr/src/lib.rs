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
//!
//! Where it sits: this is an intake stage, before the tracer proper. `inkvec-cli`'s driver
//! calls [`decide`] on a probe trace (in [`Mode::Auto`]) and [`prepass`] to clean, then
//! traces the cleaned raster; `inkvec-restore` borrows [`detect`] and the process helpers in
//! [`external`]. Images are [`Rgba`]: straight (unpremultiplied) sRGB and alpha in `0..1`,
//! row-major, four floats per pixel.
//!
//! The crate borrows `Rgba` and its PNG loader from `inkvec-trace`, which is the only reason
//! it depends on the tracer at all.

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

    /// Parses the command-line spelling: exactly `auto`, `on` or `off` (lower case). The
    /// error names the value and the accepted spellings.
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
///
/// The steps, each justified in [`clean`]'s module doc:
///
/// 1. zero the colour of every fully transparent pixel (alpha `<= 1e-4`), since the network
///    sees RGB and would otherwise be dragged by whatever the encoder stored there;
/// 2. upscale by the upscaler's factor `s` (4 for the packaged network);
/// 3. box-average back down by `s / out_scale`, so the result is `out_scale` times the
///    input's size (2 by default);
/// 4. if [`Options::recolour`], refit each colour channel of the result onto a bicubic
///    upsample of the (zeroed) source over its flat pixels ([`clean::match_flats`]).
///
/// Returns an image `out_scale` times the input in each dimension, or an error when
/// `out_scale` is 0 or does not divide `s`, or when the upscaler fails.
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
///
/// `threshold` is on [`detect::interior_residual`]'s scale (8-bit levels, with that
/// function's calibration factor); [`detect::DEGRADED_RESIDUAL`] is the shipped value. An
/// SVG that does not parse is kept as well.
pub fn decide(img: &Rgba, svg: &str, threshold: f64) -> Decision {
    let Ok(model) = detect::render_svg(svg, img.width, img.height) else {
        return Decision::Keep { residual: None };
    };
    match detect::interior_residual(img, &model) {
        Some(r) if r > threshold => Decision::Clean { residual: Some(r) },
        r => Decision::Keep { residual: r },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A stand-in for the network: nearest-neighbour replication by `k`, remembering the
    /// image it was handed so a test can see what the pre-pass fed it.
    struct Nearest {
        k: usize,
        seen: RefCell<Option<Rgba>>,
    }

    impl Nearest {
        fn new(k: usize) -> Self {
            Self {
                k,
                seen: RefCell::new(None),
            }
        }
    }

    impl Upscaler for Nearest {
        fn scale(&self) -> usize {
            self.k
        }

        fn upscale(&self, img: &Rgba) -> Result<Rgba, Box<dyn std::error::Error>> {
            *self.seen.borrow_mut() = Some(img.clone());
            let (w, h) = (img.width * self.k, img.height * self.k);
            let data = (0..w * h)
                .flat_map(|i| img.pixel((i % w) / self.k, (i / w) / self.k))
                .collect();
            Ok(Rgba {
                width: w,
                height: h,
                data,
            })
        }

        fn describe(&self) -> String {
            format!("nearest x{}", self.k)
        }
    }

    /// A 2-colour image: left half `left`, right half `right`, all opaque.
    fn halves(w: usize, h: usize, left: [f32; 3], right: [f32; 3]) -> Rgba {
        let data = (0..w * h)
            .flat_map(|i| {
                let c = if i % w < w / 2 { left } else { right };
                [c[0], c[1], c[2], 1.0]
            })
            .collect();
        Rgba {
            width: w,
            height: h,
            data,
        }
    }

    #[test]
    fn mode_parses_its_three_spellings_and_names_a_bad_one() {
        assert_eq!("auto".parse::<Mode>(), Ok(Mode::Auto));
        assert_eq!("on".parse::<Mode>(), Ok(Mode::On));
        assert_eq!("off".parse::<Mode>(), Ok(Mode::Off));
        let err = "ON".parse::<Mode>().expect_err("spellings are lower case");
        assert!(
            err.contains("\"ON\"") && err.contains("auto, on or off"),
            "{err}"
        );
        assert_eq!(Mode::default(), Mode::Auto);
    }

    #[test]
    fn prepass_returns_the_requested_scale() {
        let img = halves(6, 4, [0.1, 0.2, 0.3], [0.9, 0.8, 0.7]);
        let up = Nearest::new(4);
        let out = prepass(&up, &img, Options::default()).expect("x4 halves to x2");
        assert_eq!((out.width, out.height), (12, 8));
        let same = prepass(
            &up,
            &img,
            Options {
                out_scale: 4,
                recolour: false,
            },
        )
        .expect("x4 is also allowed");
        assert_eq!((same.width, same.height), (24, 16));
    }

    #[test]
    fn prepass_without_recolour_is_upscale_then_box() {
        // Nearest x4 followed by a 2x2 box is exactly nearest x2, so the pre-pass must
        // reproduce the input's own colours pixel for pixel.
        let img = halves(4, 4, [0.25, 0.5, 0.75], [1.0, 0.0, 0.5]);
        let opt = Options {
            out_scale: 2,
            recolour: false,
        };
        let out = prepass(&Nearest::new(4), &img, opt).expect("runs");
        for y in 0..out.height {
            for x in 0..out.width {
                assert_eq!(out.pixel(x, y), img.pixel(x / 2, y / 2), "at ({x}, {y})");
            }
        }
    }

    #[test]
    fn prepass_rejects_a_scale_the_upscaler_cannot_reach() {
        let img = halves(4, 4, [0.0; 3], [1.0; 3]);
        for out_scale in [0, 3, 8] {
            let opt = Options {
                out_scale,
                recolour: true,
            };
            let err = prepass(&Nearest::new(4), &img, opt).expect_err("not a divisor of 4");
            assert!(err.to_string().contains("x4 upscaler"), "{err}");
        }
    }

    #[test]
    fn prepass_hides_the_colour_under_transparent_pixels() {
        let mut img = halves(4, 4, [0.6, 0.6, 0.6], [0.6, 0.6, 0.6]);
        // Pixel 5 is fully transparent but stores a loud colour.
        img.data[20..24].copy_from_slice(&[1.0, 0.0, 1.0, 0.0]);
        let up = Nearest::new(2);
        let opt = Options {
            out_scale: 2,
            recolour: false,
        };
        prepass(&up, &img, opt).expect("runs");
        let fed = up.seen.borrow().clone().expect("the upscaler was called");
        assert_eq!(fed.pixel(1, 1), [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(
            fed.pixel(0, 0),
            [0.6, 0.6, 0.6, 1.0],
            "opaque pixels untouched"
        );
    }

    /// A 32 x 32 grey square as an SVG, in the same space as a 32 x 32 raster.
    fn grey_svg(level: u8) -> String {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" width="32" height="32"><rect width="32" height="32" fill="#{level:02x}{level:02x}{level:02x}"/></svg>"##
        )
    }

    fn grey(level: u8) -> Rgba {
        let v = level as f32 / 255.0;
        halves(32, 32, [v; 3], [v; 3])
    }

    #[test]
    fn decide_keeps_a_trace_that_fits_and_cleans_one_that_does_not() {
        let img = grey(128);
        match decide(&img, &grey_svg(128), detect::DEGRADED_RESIDUAL) {
            Decision::Keep { residual: Some(r) } => assert!(r < 1e-6, "{r}"),
            d => panic!("a perfect fit is kept, got {d:?}"),
        }
        match decide(&img, &grey_svg(140), detect::DEGRADED_RESIDUAL) {
            // 12 levels in every channel reads 12 / sqrt(3) on this scale.
            Decision::Clean { residual: Some(r) } => assert!((r - 12.0 / 3f64.sqrt()).abs() < 0.1),
            d => panic!("a 12-level miss is cleaned, got {d:?}"),
        }
    }

    #[test]
    fn decide_keeps_what_it_cannot_judge() {
        let img = grey(128);
        assert!(matches!(
            decide(&img, "not an svg", 0.0),
            Decision::Keep { residual: None }
        ));
        // A 4 x 4 image has only 4 interior pixels, far below the 100 needed to judge.
        let tiny = halves(4, 4, [0.5; 3], [0.5; 3]);
        assert!(matches!(
            decide(&tiny, &grey_svg(0), 0.0),
            Decision::Keep { residual: None }
        ));
    }
}
