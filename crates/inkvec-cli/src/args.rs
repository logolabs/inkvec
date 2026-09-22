//! The command line: what the tool accepts, and what it means.
//!
//! One struct of settled options, one help text, one parser. Every default lives in
//! [`Args::default`] so that the library entry points can be driven without a command
//! line at all — the Gradio desk app and the WASM build both do exactly that.

use std::path::PathBuf;

/// Settled command line options, driving the library whether or not there was an
/// actual command line.
#[derive(Clone, Debug)]
pub struct Args {
    /// Path to the input raster.
    pub input: PathBuf,
    /// Output SVG path. Defaults to alongside the input.
    pub output: Option<PathBuf>,
    /// Chord tolerance, in standard deviations of the measurement uncertainty.
    pub tau: f64,
    /// Output coordinate precision, in pixels. Also sets the MDL cost of a
    /// coordinate: `lambda = ln(extent / precision)`.
    pub precision: f64,
    /// Discard features below this area, in px².
    pub min_area: f64,
    /// Transparent margin around the output, as a fraction of the larger side; the
    /// viewBox grows, the geometry does not move.
    pub margin: f64,
    /// Scale the fit tolerances (sigma, precision, lambda) with the raster so a
    /// 512-px logo gets the parameter count of a 128-px one.
    pub content_units: bool,
    /// Inputs larger than this on their longer side are traced at this size and the
    /// SVG is written at the original size, in pixels.
    pub max_dim: usize,
    /// Advisory wall-clock budget, in seconds. 0 means no budget.
    pub time_budget: f64,
    /// Exit with status 2 on input that has nothing to trace (a single flat
    /// colour), instead of writing the flat SVG and printing a warning.
    pub strict: bool,
    /// Knock the background out: the face that covers the whole canvas is not
    /// painted, so the artwork sits on transparency.
    pub no_background: bool,
    /// Keep a nearest-neighbour upscale as it is instead of undoing it.
    pub no_unblock: bool,
    /// Spend fewer coordinates on boundaries that are barely visible.
    pub simplify_faint: bool,
    /// Recover translucent layers: one shape at one opacity seen against several grounds.
    pub layers: bool,
    /// Punch the faces the source drew transparent out of the faces above them, so the
    /// SVG carries the input's holes instead of painting them.
    pub cutout: bool,
    /// Trace transparency natively: inks carry an opacity, the clear ground is an ink, and
    /// alpha is a fourth channel wherever the tracer unmixes. Implies `cutout`'s output.
    /// An opaque input traces exactly as without it. On by default; `--no-native-alpha` (or
    /// `INKVEC_NATIVE_ALPHA=0`) goes back to compositing onto a matte first.
    pub native_alpha: bool,
    /// No ids or groups, no trailing zeros. Same geometry, typically about a tenth
    /// smaller.
    pub minify: bool,
    /// Post-fit passes that trade parameters for structure an artist can edit:
    /// G1-smooth joins, axis-aligned and equal-length handles, aligned nodes, and
    /// self-symmetric rings locked into exact mirrors. Fidelity stays within the
    /// same tolerance; only structure and parameters move.
    pub editability: bool,
    /// Two-tone output (the Potrace-comparable mode).
    pub bilevel: bool,
    /// Skip gradient fitting entirely and fill flat.
    pub no_gradients: bool,
    /// Skip the self-crossing ring repair pass that runs after fitting.
    pub no_repair: bool,
    /// OKLab distance below which two colours are treated as one ink.
    pub merge_distance: f32,
    /// Maximum palette size.
    pub max_colors: usize,
    /// Only write the file; suppress other output.
    pub quiet: bool,
    /// Treat the intake as lossily compressed, so the palette's noise guard runs even
    /// though the edges are sharp. `Auto` reads the container: JPEG and lossy WebP are
    /// lossy, PNG/GIF/BMP/TIFF are not. Set it `On` for a PNG that was once a JPEG --
    /// a re-saved screenshot, an export from a chat app -- where the file no longer
    /// admits what was done to it.
    pub lossy: inkvec_sr::Mode,
    /// Whether to run the super-resolution pre-pass before tracing: never, always,
    /// or `Auto` to trace, measure the fit, and clean and retrace only if the trace
    /// disagrees with the input where it claims to be flat.
    pub sr: inkvec_sr::Mode,
    /// Interior residual above which `Auto` cleans.
    pub sr_threshold: f64,
    /// Output scale before tracing, relative to the input.
    pub sr_scale: usize,
    /// Skip putting flat colours back on the source's values after upscaling.
    pub sr_no_recolour: bool,
    /// Upscale with an external command instead of the built-in model. `{in}` and
    /// `{out}` are replaced with PNG paths; the command must write an image
    /// `sr_scale` times larger.
    pub sr_command: Option<String>,
    /// The trained restorer: removes JPEG, WebP and decoder damage at the input's own size
    /// before tracing. Off by default for the same reason SR is: on clean input it costs a
    /// little colour accuracy.
    pub restore: inkvec_restore::Mode,
    /// Interior residual above which `Auto` restores.
    pub restore_threshold: f64,
    /// The built-in restorer's network: an `.onnx` export (ONNX Runtime) or Burn `.bpk`
    /// weights. Falls back to `INKVEC_RESTORE_ONNX` / `INKVEC_RESTORE_WEIGHTS`.
    pub restore_weights: Option<PathBuf>,
    /// Restore with an external command instead of the built-in network. `{in}`
    /// and `{out}` are replaced with PNG paths; the command must write a
    /// same-size image.
    pub restore_command: Option<String>,
    /// Resample an oversampled input before tracing.
    pub intake_scale: bool,
    /// Emit line art as strokes -- one path and one width -- instead of as filled
    /// outlines. Declines silently on anything else.
    pub strokes: bool,
    /// Least share of the input's ink the strokes must actually draw, in [0, 1],
    /// or the drawing falls back to outlines.
    pub stroke_balance: f64,
    /// Iterations of centreline refinement against the measured coverage, after
    /// stroke recovery. 0 skips it.
    pub stroke_refine: usize,
    /// Residual, per stroke against its own region, above which a stroke falls
    /// back to an outline.
    pub stroke_residual: f64,
    /// Research: a global multiplier on the MDL cost of a parameter. Above 1.0 the fit is
    /// plainer and cheaper, below it more detailed. 1.0 leaves every fit exactly as it is.
    pub lambda_scale: f64,
    /// What one Bézier segment costs the MDL objective, in parameters. A line costs two.
    /// The default, 6, is why traced output is far less curved than hand-drawn artwork; a
    /// lower price buys more curves and fewer straight segments, at some cost in file size.
    /// `None` leaves the fit's own price (6, or `INKVEC_PARAMS_CUBIC`) exactly as it is.
    pub bezier_cost: Option<f64>,
    /// The turn at a join, in degrees, that is charged as a full corner. `None` leaves the
    /// fit's own angle (10, or `INKVEC_G1_BREAK`) exactly as it is.
    pub corner_angle: Option<f64>,
    /// Detect and regularize repeating glyphs/shapes via affine moment normalization.
    pub harmonize: bool,
    /// Threshold IoU for shape equivalence [default: 0.92].
    pub harmonize_threshold: f64,
    /// Emit harmonized shapes as SVG `<defs>` and `<use>` instances.
    pub use_symbols: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            input: PathBuf::new(),
            output: None,
            tau: 2.0,
            precision: 0.1,
            min_area: 2.0,
            margin: 0.0,
            content_units: false,
            max_dim: 2048,
            time_budget: 0.0,
            strict: false,
            no_background: false,
            no_unblock: false,
            simplify_faint: false,
            layers: false,
            cutout: false,
            native_alpha: std::env::var_os("INKVEC_NATIVE_ALPHA").is_none_or(|v| v != "0"),
            minify: false,
            editability: false,
            bilevel: false,
            no_gradients: false,
            no_repair: false,
            merge_distance: inkvec_trace::color::DEFAULT_MERGE_DISTANCE,
            max_colors: 64,
            quiet: true,
            lossy: inkvec_sr::Mode::Auto,
            sr: inkvec_sr::Mode::Off,
            sr_threshold: inkvec_sr::detect::DEGRADED_RESIDUAL,
            sr_scale: 2,
            sr_no_recolour: false,
            sr_command: None,
            restore: inkvec_restore::Mode::Off,
            restore_threshold: inkvec_sr::detect::DEGRADED_RESIDUAL,
            restore_weights: None,
            restore_command: None,
            intake_scale: false,
            strokes: false,
            stroke_balance: 0.90,
            stroke_refine: 0,
            // Per stroke against its own region, which is where it discriminates. Pooled
            // over a drawing the cap-and-join mismatch every stroke pays swamped it.
            // Conservative: only strokes that fit their own region closely. Swept on
            // lucide, no value wins -- 0.06 keeps two of sixteen at dE00 0.161 against
            // filled's 0.128, and looser values are worse still.
            stroke_residual: 0.06,
            lambda_scale: 1.0,
            bezier_cost: None,
            corner_angle: None,
            harmonize: true,
            harmonize_threshold: 0.92,
            use_symbols: false,
        }
    }
}

/// The help text.
///
/// Returns an owned `String`, not a literal, so a default it quotes can be filled in from
/// the constant that actually supplies it. `--merge` is why this changed: the help said
/// `0.055` while the compiled default was `DEFAULT_MERGE_DISTANCE = 0.035`, and nothing
/// could notice the two had drifted apart. A number the reader will act on should be
/// printed from the thing it describes rather than copied next to it.
///
/// A plain `replace` rather than `format!`: the text is full of literal braces that
/// `format!` would demand be doubled, and doubling a hundred of them to interpolate one
/// value is a worse trade than a single substitution.
pub(crate) fn usage() -> String {
    USAGE_TEMPLATE.replace(
        "{merge_default}",
        &inkvec_trace::color::DEFAULT_MERGE_DISTANCE.to_string(),
    )
}

/// The text [`usage`] prints, with `{merge_default}` still to fill in.
const USAGE_TEMPLATE: &str = "\
inkvec — raster to vector (bilevel)

USAGE:
    inkvec <input.(png|jpg|webp|gif|bmp|tif)> [-o <output.svg>] [OPTIONS]

OPTIONS:
    -o, --output <path>     Output SVG (default: alongside the input)
        --tau <f>           Chord tolerance in standard deviations   [default: 2.0]
        --precision <f>     Sets the MDL cost of a coordinate: lambda = ln(extent/precision).
                            It does not set the digits the emitter writes; output
                            coordinates are fixed at 2 decimals        [default: 0.1]
        --min-area <f>      Discard features below this area, in px^2  [default: 2.0]
        --margin <f>        Transparent margin around the output, as a fraction of the
                            larger side; the viewBox grows, the geometry does not move
        --max-dim <px>      Inputs larger than this on their longer side are traced at
                            this size and the SVG is written at the original size.
                            Trace time grows with the pixel count; 2048 keeps a
                            typical logo under a few seconds. 0 means no cap
                                                                   [default: 2048]
        --time-budget <s>   Advisory wall-clock budget. Gradient-band merging stops at
                            60% of it and the boundary solve gets 25%; the output is
                            still a correct trace, with more fills or a less polished
                            outline. 0 means no budget                 [default: 0]
        --strict            Exit with status 2 on input that has nothing to trace
                            (a single flat colour). Without it the flat SVG is
                            written and a warning is printed
        --no-background     Knock the background out: the face that covers the whole
                            canvas is not painted, so the artwork sits on transparency
        --layers            Recover a translucent shape seen against several grounds --
                            three overlapping circles at 85% rather than five flat patches.
                            The faces under it are repainted with the ground and the layer
                            is composited over them, so the image is unchanged and the
                            document says what was drawn. Off by default: it adds a path
                            rather than removing any until the ground pieces it reunites
                            are merged, and it fires on about one real icon in twenty
        --simplify-faint    Spend fewer coordinates where the two inks meeting at a
                            boundary are close in colour. The error a viewer sees is the
                            position error times the contrast across it, so at a tenth of
                            the contrast a coordinate buys a tenth of the visible accuracy.
                            Off by default: on the 246-icon screen set, which is
                            high-contrast art, it saves 0.3% of the parameters and costs
                            0.2% of the colour error
        --no-unblock        Trace a nearest-neighbour upscale as it arrived. By default
                            an input whose pixels are exact k x k blocks is averaged back
                            down to the original grid first -- the inverse is exact -- and
                            the SVG is still written at the size that came in. Without it a
                            96-px logo blown up to 768 traces its pixel boundaries: 13 inks
                            instead of 3, and 1568 straight lines walking round the corners
        --cutout            Carry the input's transparency into the output. A face the
                            source drew transparent becomes a hole in the faces above it
                            rather than a patch of white; one drawn at a single opacity
                            comes back with `fill-opacity` and its own colour; and the
                            matte the image is traced against is chosen so that white
                            artwork on a transparent ground survives at all. Only matters
                            with --no-native-alpha: native tracing (the default) already
                            carries the transparency out
        --no-native-alpha   Composite a transparent input onto a matte before tracing, as
                            releases up to 0.1.3 did, instead of tracing its transparency
                            natively. Native (the default) gives every ink an opacity and
                            the transparent ground an ink of its own: holes stay holes,
                            glows and shadows stay translucent, and a fade is one gradient
                            of colour and opacity. An opaque input traces the same either
                            way
        --minify            No ids or groups, and the path data written in the fewest
                            bytes: relative commands where they are shorter, repeated
                            letters and needless separators dropped, H/V/S where they say
                            the same thing. Nothing is rounded and nothing moves -- the
                            same picture, pixel for pixel, about a twelfth smaller
        --editability       Post-fit passes for artists: G1-smooth joins, axis-aligned
                            and equal-length handles, aligned nodes, and self-symmetric
                            rings locked into exact mirrors. Every pass is guarded to the
                            same fidelity tolerance; parameters move so structure can too
        --content-units     Scale the fit tolerances (sigma, precision, lambda) with the
                            raster so a 512-px logo gets the parameter count of a 128-px
                            one. Off by default: it trades fidelity for parsimony
                            (5-px squares fitted as circles, thin rings broken).
        --colors <n>        Maximum palette size                     [default: 64]
        --merge <f>         OKLab distance below which two colours are one ink [default: {merge_default}]
        --bilevel           Two-tone output (the Potrace-comparable mode)
        --no-gradients      Skip gradient fitting entirely and fill flat. A genuine
                            fast path: gradient fitting dominates runtime
        --no-repair         Skip the self-crossing ring repair pass that runs after
                            fitting
        --strokes           Emit line art as strokes -- one path and one width --
                            instead of as filled outlines. 28% of the corpus is
                            drawn this way and costs 3.3x the artist's parameters
                            as outlines. Declines silently on anything else.
        --stroke-balance <f>   Least share of the input's ink the strokes must
                            actually draw, or the drawing falls back to outlines
                            [default: 0.90]
        --stroke-refine <n> Iterations of centreline refinement against the measured
                            coverage, after stroke recovery. 0 skips it  [default: 0]
        --stroke-residual <f>  Residual, per stroke against its own region, above
                            which a stroke falls back to an outline [default: 0.06]
        --lambda-scale <f>  Research: multiply the MDL cost of every parameter. Above
                            1.0 buys a plainer, cheaper description, below 1.0 a more
                            detailed one                              [default: 1.0]
        --bezier-cost <f>   What one Bézier segment costs the MDL objective, in parameters
                            (a line costs 2). Lower draws more curves and fewer straight
                            segments, at some cost in file size. 2 to 12  [default: 6]
        --corner-angle <f>  The turn at a join, in degrees, charged as a full corner.
                            Higher keeps gentler bends smooth. 1 to 60    [default: 10]
        --no-harmonize      Disable repeating shape harmonization (on by default)
        --harmonize         Explicitly enable repeating shape harmonization
        --harmonize-threshold <f> Shape equivalence IoU threshold     [default: 0.92]
        --use-symbols       Emit harmonized shapes as SVG <defs> and <use> instances
    -q, --quiet             Only write the file
    -V, --version           Print the version
    -h, --help

SUPER-RESOLUTION PRE-PASS:
    For input that has been through something -- JPEG, a screenshot, a resize.
    Upscaling and halving back removes about two thirds of the damage: on JPEG
    q50 it takes colour error from 1.00 to 0.44 and the path count from 20x the
    artist's to 6.8x. On CLEAN input it is three times worse than not using it,
    which is why `auto` measures before deciding.

        --lossy <mode>      auto  read the container: JPEG/lossy WebP -> on (default)
                            on    the source was compressed, whatever the file says
                            off   trust the pixels
        --sr <mode>         off   never (default)
                            on    always clean first
                            auto  trace, measure the fit, clean and retrace only
                                  if the trace disagrees with the input where it
                                  claims to be flat
        --sr-threshold <f>  Interior residual above which auto cleans [default: 0.5]
                            Clean input reads at most 0.376; JPEG q50 at least 0.650
        --sr-scale <n>      Output scale before tracing              [default: 2]
        --sr-no-recolour    Skip putting flat colours back on the source's values
        --sr-command <cmd>  Upscale with an external command instead of the packaged
                            Python tool. `{in}` and `{out}` are replaced with PNG
                            paths; the command must write an image x4 larger.
                            Quote a program path that contains spaces.

RESTORER PRE-PASS:
    A trained network that removes JPEG, WebP and AI-decoder damage at the input's
    own size. On damaged input it cuts colour error by 28-45% and parameters by
    about 30%; on clean input it costs a few percent of colour accuracy, which is
    why it is off by default and `auto` measures first. Restored input is traced
    with lossy intake on.

        --restore <mode>    off   never (default)
                            on    always restore first
                            auto  trace, measure the fit, restore and retrace only
                                  if the trace disagrees with the input where it
                                  claims to be flat
        --restore-threshold <f>   Interior residual above which auto restores
                            [default: 0.5]
        --restore-weights <file>  The built-in restorer's network: an .onnx export,
                            or Burn .bpk weights; else INKVEC_RESTORE_ONNX /
                            INKVEC_RESTORE_WEIGHTS
        --restore-command <cmd>   Restore with an external command instead. `{in}`
                            and `{out}` are replaced with PNG paths; the command
                            must write a same-size image. Quote a program path
                            that contains spaces.

INTAKE SCALE (opt-in; trades structural accuracy for speed):
    Thresholds here are in pixels and were tuned where one pixel was one unit of
    real detail. An upsampled, blurred or photographed image has more pixels than
    detail, and the surplus is not information -- it is what shatters the palette.
    This measures the width of an edge and resamples the input back to one pixel
    per unit of detail. A native render reads exactly 1.00 and is never touched.

    OFF by default, because it is not free. On 8x-upsampled input at 1024 it is
    24x faster with a twelfth of the parameters and half the colour error -- but
    DINO falls 0.954 to 0.916, and DINO is the measure of structure. Turn it on
    when an oversampled input is too slow to trace at all, not to make a good
    trace faster.

        --intake-scale      Resample an oversampled input before tracing
";

pub(crate) fn parse_args() -> Result<Args, String> {
    parse_args_from(std::env::args().skip(1))
}

/// Parse a command line, without the program name, into [`Args`]. Every option starts at
/// its [`Args::default`] value except `quiet`: a library caller wants silence, a person at
/// a terminal wants the log.
fn parse_args_from(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut a = Args {
        quiet: false,
        ..Args::default()
    };
    let mut input = None;
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "-h" | "--help" => {
                print!("{}", usage());
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("inkvec {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-q" | "--quiet" => a.quiet = true,
            "-o" | "--output" => {
                a.output = Some(PathBuf::from(it.next().ok_or("--output needs a path")?))
            }
            "--tau" => a.tau = parse_value(&mut it, "--tau")?,
            "--precision" => a.precision = parse_value(&mut it, "--precision")?,
            "--min-area" => a.min_area = parse_value(&mut it, "--min-area")?,
            "--margin" => a.margin = parse_value(&mut it, "--margin")?,
            "--content-units" => a.content_units = true,
            "--max-dim" => a.max_dim = parse_value(&mut it, "--max-dim")?,
            "--time-budget" => a.time_budget = parse_value(&mut it, "--time-budget")?,
            "--strict" => a.strict = true,
            "--no-background" => a.no_background = true,
            "--no-unblock" => a.no_unblock = true,
            "--simplify-faint" => a.simplify_faint = true,
            "--layers" => a.layers = true,
            "--cutout" => a.cutout = true,
            "--native-alpha" => a.native_alpha = true,
            "--no-native-alpha" => a.native_alpha = false,
            "--minify" => a.minify = true,
            "--editability" => a.editability = true,
            "--bilevel" => a.bilevel = true,
            "--lossy" => {
                let v = it.next().ok_or("--lossy needs auto, on or off")?;
                a.lossy = v
                    .parse()
                    .map_err(|_| format!("unknown lossy mode {v:?}; want auto, on or off"))?
            }
            "--sr" => a.sr = it.next().ok_or("--sr needs auto, on or off")?.parse()?,
            "--sr-threshold" => a.sr_threshold = parse_value(&mut it, "--sr-threshold")?,
            "--sr-scale" => a.sr_scale = parse_value(&mut it, "--sr-scale")?,
            "--sr-no-recolour" => a.sr_no_recolour = true,
            "--intake-scale" => a.intake_scale = true,
            "--strokes" => a.strokes = true,
            "--stroke-residual" => a.stroke_residual = parse_value(&mut it, "--stroke-residual")?,
            "--stroke-refine" => a.stroke_refine = parse_value(&mut it, "--stroke-refine")?,
            "--lambda-scale" => a.lambda_scale = parse_value(&mut it, "--lambda-scale")?,
            "--bezier-cost" => a.bezier_cost = Some(parse_value(&mut it, "--bezier-cost")?),
            "--corner-angle" => a.corner_angle = Some(parse_value(&mut it, "--corner-angle")?),
            "--stroke-balance" => a.stroke_balance = parse_value(&mut it, "--stroke-balance")?,
            "--harmonize" => a.harmonize = true,
            "--no-harmonize" => a.harmonize = false,
            "--harmonize-threshold" => {
                a.harmonize_threshold = parse_value(&mut it, "--harmonize-threshold")?
            }
            "--use-symbols" => a.use_symbols = true,
            "--sr-command" => {
                a.sr_command = Some(it.next().ok_or("--sr-command needs a command line")?)
            }
            "--restore" => {
                a.restore = it
                    .next()
                    .ok_or("--restore needs auto, on or off")?
                    .parse()?
            }
            "--restore-threshold" => {
                a.restore_threshold = parse_value(&mut it, "--restore-threshold")?
            }
            "--restore-weights" => {
                a.restore_weights = Some(PathBuf::from(
                    it.next().ok_or("--restore-weights needs a file")?,
                ))
            }
            "--restore-command" => {
                a.restore_command = Some(it.next().ok_or("--restore-command needs a command line")?)
            }
            "--no-gradients" => a.no_gradients = true,
            "--no-repair" => a.no_repair = true,
            "--colors" => a.max_colors = parse_value(&mut it, "--colors")?,
            "--merge" => a.merge_distance = parse_value(&mut it, "--merge")?,
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other => input = Some(PathBuf::from(other)),
        }
    }
    a.input = input.ok_or("no input file")?;
    Ok(a)
}

/// Take the next argument as the value of `flag` and parse it. On failure the message names
/// both the flag and the value that was rejected.
fn parse_value<T: std::str::FromStr>(
    it: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<T, String> {
    let v = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
    v.parse().map_err(|_| format!("bad {flag} value {v:?}"))
}

#[cfg(test)]
mod tests {
    use super::{parse_args_from, Args};
    use std::path::PathBuf;

    fn parse(line: &str) -> Result<Args, String> {
        parse_args_from(line.split_whitespace().map(String::from))
    }

    #[test]
    fn a_bare_input_gets_every_library_default_except_quiet() {
        let parsed = parse("logo.png").expect("parses");
        let expected = Args {
            input: "logo.png".into(),
            quiet: false,
            ..Args::default()
        };
        assert_eq!(format!("{parsed:?}"), format!("{expected:?}"));
    }

    #[test]
    fn flags_set_their_fields() {
        let a = parse("logo.png --tau 3 --cutout -o out.svg --restore on --colors 8 -q")
            .expect("parses");
        assert!((a.tau - 3.0).abs() < 1e-12);
        assert!(a.cutout && a.quiet);
        assert!(a.harmonize);
        assert_eq!(a.output, Some(PathBuf::from("out.svg")));
        assert!(matches!(a.restore, inkvec_restore::Mode::On));
        assert_eq!(a.max_colors, 8);

        let a_opt_out = parse("logo.png --no-harmonize").expect("parses");
        assert!(!a_opt_out.harmonize);

        if std::env::var_os("INKVEC_NATIVE_ALPHA").is_none() {
            assert!(parse("logo.png").expect("parses").native_alpha);
        }
        assert!(
            !parse("logo.png --no-native-alpha")
                .expect("parses")
                .native_alpha
        );
    }

    #[test]
    fn bad_input_is_an_error_naming_the_problem() {
        assert_eq!(
            parse("logo.png --tau abc").unwrap_err(),
            "bad --tau value \"abc\""
        );
        assert_eq!(parse("logo.png --tau").unwrap_err(), "--tau needs a value");
        assert_eq!(parse("--cutout").unwrap_err(), "no input file");
        assert!(parse("logo.png --bogus")
            .unwrap_err()
            .contains("unknown option --bogus"));
    }
}
