//! Raster analysis: from pixels to a planar map of coloured faces with fitted fills.
//!
//! Everything that decides *what is in the image* lives here. The crate above turns the
//! answer into SVG; the crate beside it fits curves to the boundaries this one finds.
//!
//! # The Quality pipeline, stage by stage
//!
//! [`trace_color_full_with_alpha`] runs these in order; the names are the stage names the
//! [`Stopwatch`] and the progress log report. Pixels are row-major, `p = y·w + x`, colours
//! sRGB `[0, 1]` composited onto white unless a stage says otherwise.
//!
//! 1. **noise, intake** ([`coverage`]): the pixel noise `σ` from the luminance, and
//!    whether the intake is soft (wide edges, a lossy container, JPEG ringing), which
//!    switches the palette's noise guard.
//! 2. **palette** ([`color::extract_palette_mdl`]): how many inks, and which.
//! 3. **labels** ([`color::label_image`]): the nearest ink per pixel; on a soft intake `σ`
//!    is then re-measured against the labels ([`regularize`]); with alpha inks,
//!    [`color::split_alpha_inks`].
//! 4. **despeckle** ([`regions::despeckle`]): components under `min_region` pixels go.
//! 5. **blend_absorb** ([`regions::absorb_blend_slivers`],
//!    [`regions::reassign_blend_pixels`]): anti-aliasing slivers and pixels go back to the
//!    inks they are blends of.
//! 6. **merge_bands** ([`gradient::bands`]): adjacent palette bands that one gradient
//!    explains become one label, and every label gets its fitted fill (flat, linear or
//!    radial, whichever pays).
//! 7. **carve** ([`gradient::carve`]): clusters of pixels no fill explains become labels of
//!    their own.
//! 8. **fades** (native-alpha path only, `native/fade.rs`): translucent bands become one
//!    opacity gradient.
//! 9. **split** ([`regions::split_components`]): labels become faces, one per 4-connected
//!    component, each with its label's fill and ink.
//!
//! From here `finish_color_trace_alpha` takes over, shared by every entry point:
//!
//! 10. **saddles** (research only, `regions::merge_saddle_faces`);
//! 11. **build_map** ([`planar::build`]): faces, and the edges and junctions they share;
//! 12. **symmetry_detect** ([`symmetry::detect`]): mirrors of the label map;
//! 13. **refine_subpix** ([`planar::refine_subpixel_alpha`]): each boundary point moved to
//!     its sub-pixel position by unmixing the two faces' fills. Stages 12 and 13 both only
//!     read the lattice map, so they run side by side and are timed together as this one;
//! 14. **refine_junc** ([`planar::refine_junctions`]): where three or more faces meet;
//! 15. **boundary_opt** ([`boundary_opt`]): all boundary points solved at once against an
//!     exact coverage render of the image (Quality only);
//! 16. **decode** (research only);
//! 17. **symmetry** ([`symmetry::enforce`]): mirrored boundaries made exactly symmetric.
//!
//! The result, a [`ColorTrace`], goes to the curve fitter (`inkvec-fit`, stage `fit_dp`)
//! and the SVG emitter in `inkvec-cli`. Fast mode ([`fast`]) and the native-alpha path
//! ([`native`]) replace stages 1–9 and share 10–17 (Fast skips 15); [`trace_color_from_labels`] replaces
//! 2–5 with a caller's label map.
//!
//! Two things run beside that path rather than in it: [`alpha`] recovers a translucent
//! layer seen against two grounds, and [`centerline`] recovers strokes rather than
//! regions. `decode` is a research stage: compiled only with the `research` feature, and
//! off even then unless `INKVEC_DECODE` is set.
//!
//! One rule governs every stage: a model is kept only when it lowers squared residual
//! against the image by more than `lambda` times the parameters it adds.

// Pixel and channel loops here index several parallel arrays at once — coverage, labels,
// sigma and the image, all addressed by the same `p` — and the zip that would replace one
// index reads worse than the index does. Where a loop really does touch a single slice it
// is written as an iterator; this turns off the blanket suggestion, not the practice.
#![allow(clippy::needless_range_loop)]
// The doc comments explain the maths by pointing at the private helper that does each step.
// Those links resolve under `--document-private-items` and read as plain names in the public
// docs; broken links still fail the build.
#![allow(rustdoc::private_intra_doc_links)]

pub mod alpha;
pub mod boundary_opt;
pub mod centerline;
// Exact polygon clipping and pixel coverage: the stroke solve's, and in a `research` build
// also `decode`'s (the parts only `decode` calls are unused without it).
#[cfg_attr(not(feature = "research"), allow(dead_code))]
mod clip;
pub mod color;
pub mod contour;
pub mod coverage;
#[cfg(feature = "research")]
pub mod decode;
pub mod diag;
pub mod fast;
pub mod gradient;
#[cfg(feature = "research")]
pub mod ink_ideas;
mod load;
pub mod native;
pub mod occlusion;
pub mod planar;
pub mod regions;
pub mod regroup;
pub mod regularize;
pub mod symmetry;
pub mod taper;

use inkvec_core::progress;
use inkvec_core::Polyline;

pub use color::{Oklab, Palette};
pub use coverage::{CoverageField, Rgba};
pub use load::{
    decode_image, decode_image_capped, load_image, load_image_capped, lossy_container,
    rgba8_capped, TraceError,
};
pub use planar::PlanarMap;
pub use regions::{
    absorb_blend_slivers, despeckle, dump_labels, reassign_blend_pixels, split_components,
};
#[cfg(feature = "research")]
pub use regions::{merge_saddle_faces, SADDLE_SIGMAS};

/// The clock behind [`ColorOptions::deadline`] and [`Stopwatch`], re-exported so a caller
/// can build a deadline with the same `Instant` type on every target.
pub use inkvec_core::clock;

/// Options for the bilevel path.
#[derive(Debug, Clone, Copy)]
pub struct TraceOptions {
    /// Discard contours enclosing less than this area, in square pixels.
    ///
    /// Potrace's `turdsize`, in continuous units. Note that this is *not* how sub-pixel
    /// features are meant to be preserved or removed — a 0.3px-wide stroke spanning the
    /// image has a large area and survives regardless. Speckle removal and feature
    /// preservation are different questions, and conflating them is how thin content
    /// gets destroyed.
    pub min_area: f64,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self { min_area: 1.0 }
    }
}

/// Full bilevel front end: image -> coverage -> sub-pixel contours with uncertainty.
pub fn trace_bilevel(img: &Rgba, opts: &TraceOptions) -> (Vec<Polyline>, CoverageField) {
    let field = coverage::bilevel_coverage(img);
    let mut polys = contour::trace(&field);
    polys.retain(|p| contour::signed_area(p).abs() >= opts.min_area);
    (polys, field)
}

/// Options for the colour path.
#[derive(Debug, Clone, Copy)]
pub struct ColorOptions {
    /// OKLab distance below which two colours are one ink.
    pub merge_distance: f32,
    /// Maximum number of palette entries to recover.
    pub max_colors: usize,
    /// Drop regions smaller than this many pixels before building the map.
    pub min_region: usize,
    /// Fit gradient fills at all.
    ///
    /// Turning this off is a genuine fast path, not a cosmetic one. Gradient fitting is
    /// the dominant cost in the tracer: with every optional stage disabled a slow emoji
    /// still took 10.9s, and capping the palette — which is what feeds the per-region
    /// gradient search — took it to 6.3s. The CLI's `--no-gradients` used to replace the
    /// fitted model with a flat one *after* paying for the fit, so it changed the output
    /// and saved nothing.
    pub gradients: bool,
    /// Stop merging gradient bands once this instant has passed. The merge is the
    /// one stage whose cost grows with the square of the region count, and it is
    /// safe to stop early: an unmerged band is a correct fill, just a separate one.
    pub deadline: Option<clock::Instant>,
    /// Separate a region the source drew at one opacity into an ink of its own, instead of
    /// letting it merge with whatever it resembles once the image is matted opaque. See
    /// `color::split_alpha_inks`.
    pub alpha_inks: bool,
    /// Spend fewer coordinates on boundaries that are barely visible: the positional
    /// uncertainty handed to the fitter is inflated where the two inks meeting at an edge
    /// are close in colour. See `planar::refine_subpixel`.
    pub simplify_faint: bool,
    /// Wall-clock budget for the boundary solve. `None` (the
    /// default, and what a zero `--time-budget` gives) means no clock: the solve stops on
    /// its iteration count alone, so the result does not depend on how fast the machine is.
    pub boundary_ms: Option<u64>,
    /// The intake came out of a lossy codec, so the palette's noise guard must run even
    /// though the edges are sharp. See [`lossy_container`].
    pub lossy_intake: bool,
    /// Carry transparency natively: inks with opacity, the clear ground as an ink, and
    /// alpha as a fourth channel wherever the tracer unmixes. See [`native`]. Only an image
    /// with transparency takes this path; an opaque one traces as it always has.
    pub native_alpha: bool,
    /// Fast mode's front end: a histogram palette and labels, then the same planar map
    /// with every boundary point refined to its sub-pixel position, but no MDL palette,
    /// blend absorption, global boundary solve or order-first decoding (see [`fast`], whose
    /// fitter follows). A transparent image traced natively keeps the native palette and
    /// skips the rest. False is quality mode, exactly as before the flag existed.
    pub fast: bool,
}

impl Default for ColorOptions {
    fn default() -> Self {
        Self {
            merge_distance: color::DEFAULT_MERGE_DISTANCE,
            max_colors: 64,
            gradients: true,
            native_alpha: false,
            alpha_inks: false,
            simplify_faint: false,
            deadline: None,
            boundary_ms: None,
            lossy_intake: false,
            min_region: 4,
            fast: false,
        }
    }
}

/// Full colour front end: palette -> labels -> planar map -> sub-pixel refinement.
pub fn trace_color(img: &Rgba, opts: &ColorOptions) -> (PlanarMap, Palette) {
    let r = trace_color_full(img, opts);
    (r.map, r.palette)
}

/// Everything the colour front end produced, including the intermediates a caller needs
/// to fit per-region fills: the label image and the estimated pixel noise.
pub struct ColorTrace {
    /// The planar map of faces and boundaries.
    pub map: PlanarMap,
    /// What the global boundary solve did, when it ran and gained anything.
    pub boundary_opt: Option<boundary_opt::Report>,
    /// What order-first decoding did, when it ran and changed anything.
    #[cfg(feature = "research")]
    pub decode: Option<decode::Report>,
    /// Mirrors of the label map, and how the boundaries pair off under them.
    pub symmetry: symmetry::Symmetry,
    /// Boundary points the symmetry pass moved.
    pub symmetrised: usize,
    /// The recovered colour palette.
    pub palette: Palette,
    /// Per-pixel **face** id — a connected component, not a palette index.
    pub labels: Vec<u16>,
    /// Palette index for each face id.
    pub face_color: Vec<usize>,
    /// Fill model chosen for each face id.
    pub face_fill: Vec<gradient::FillFit>,
    /// One representative colour per face, as used for boundary unmixing.
    pub face_rgb: Vec<[f32; 3]>,
    /// Estimated per-channel pixel noise, in sRGB units.
    pub sigma_noise: f64,
    /// Per face, when transparency was traced natively and the face is a fade: one colour
    /// at an opacity that varies across it. Empty on the classic path.
    pub face_fade: Vec<Option<native::Fade>>,
}

/// Full colour front end, with alpha assumed opaque. See [`trace_color_full_with_alpha`].
pub fn trace_color_full(img: &Rgba, opts: &ColorOptions) -> ColorTrace {
    trace_color_full_with_alpha(img, opts, None)
}

/// The colour path, told separately what the source's alpha was.
///
/// The caller may hand this an image that has already been matted — composited onto one
/// opaque colour, alpha set to 1 — because unmixing a boundary needs two opaque colours.
/// When it does, the stages that ask "was anything drawn here" would otherwise see a solid
/// alpha channel and answer wrongly: sliver absorption and blend reassignment both read it,
/// and it is what tells an anti-aliased rim from a face. So the true alphas come alongside.
///
/// # Dispatch
///
/// * `opts.native_alpha` and a `source_alpha` of the right length with some pixel below
///   [`native::OPAQUE`]: the native-alpha path, [`native::trace_color`] (or its Fast
///   counterpart when `opts.fast`);
/// * otherwise `opts.fast`: the Fast engine's front end ([`fast`]);
/// * otherwise the Quality pipeline below, stages 1–9 of the crate overview, then
///   `finish_color_trace` for the geometry.
///
/// # Inputs and outputs
///
/// `img` is straight RGBA in `[0, 1]`; every stage here reads it composited onto white.
/// `source_alpha`, when given, must have one value per pixel to be used; otherwise the
/// image's own alpha channel stands in where alpha is read. The returned [`ColorTrace`]
/// has one face per 4-connected region, each with its palette ink, its fitted fill and
/// its boundaries at sub-pixel positions.
///
/// # Noise
///
/// `σ` (`sigma_noise`, sRGB units per channel) is estimated once from the luminance
/// before the palette exists and is then only ever raised: on a soft intake, by
/// `regularize::residual_sigma` measured against the labels (scaled by
/// [`color::MEASURED_SIGMA_SCALE`] and capped at [`color::MEASURED_SIGMA_CAP`] levels). The
/// value the palette saw is kept as `detail_sigma` for the research carve variant.
pub fn trace_color_full_with_alpha(
    img: &Rgba,
    opts: &ColorOptions,
    source_alpha: Option<&[f32]>,
) -> ColorTrace {
    if opts.native_alpha {
        if let Some(a) = source_alpha
            .filter(|a| a.len() == img.width * img.height && a.iter().any(|&v| v < native::OPAQUE))
        {
            if opts.fast {
                return fast::trace_color_native(img, opts, a);
            }
            return native::trace_color(img, opts, a);
        }
    }
    if opts.fast {
        return fast::trace_color(img, opts, source_alpha);
    }
    let rgb = img.composited([1.0, 1.0, 1.0]);

    // Noise first: the palette needs it to judge whether two nearby modes are two inks or
    // one ink measured twice.
    let lum: Vec<f32> = rgb
        .iter()
        .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
        .collect();
    let mut sigma_noise = coverage::estimate_noise(&lum, img.width, img.height);
    let detail_sigma = sigma_noise;
    if (sigma_noise - coverage::NOISE_FLOOR).abs() < 1e-12 {
        // Pinned at the floor means the estimator measured nothing and the constant is
        // speaking instead. Everything that divides by sigma is then dividing by a
        // constant, which is how a wrong noise gain survived every benchmark in the corpus.
        diag::saturated(
            "noise",
            "sigma_noise",
            sigma_noise,
            coverage::NOISE_FLOOR,
            diag::Stop::Floor,
        );
    } else {
        crate::diag!(
            "noise",
            "sigma_noise={sigma_noise:.5} levels={:.2}",
            sigma_noise * 255.0
        );
    }

    let min_region = opts.min_region;
    let mut sw = Stopwatch::start();
    progress::begin("palette");

    // The noise guard is free on a clean render and costs 2.8 % on the screen set when
    // it is on, so it is switched by what the raster is: the measured edge width. A
    // native render has one-pixel edges and the guard stays off, bit-identical to
    // before; a resampled, blurred or upscaled intake has wider ones, and there the
    // anti-aliasing ramps carry colours the palette must not take for inks.
    // ...and by what the file is. Compression damages flat regions without widening an
    // edge, so `intake_scale` alone leaves the guard off on exactly the input that needs
    // it most; `lossy_container` supplies the other half.
    let edge_width = coverage::intake_scale(&rgb, img.width, img.height);
    // ...and by what the pixels say, which is the only one of the three a re-encode cannot
    // launder. `lossy_intake` reads the container, so a JPEG re-saved as PNG defeats it, and
    // `intake_scale` reads edge width, which compression leaves at 1.00. See
    // `coverage::ringing_score`.
    let ringing = coverage::ringing_score(&rgb, img.width, img.height);
    let ringing_gate = if img.width.min(img.height) >= color::RINGING_MIN_DIM {
        color::SOFT_RINGING_LARGE
    } else {
        color::SOFT_RINGING
    };
    let soft_intake =
        edge_width > color::SOFT_INTAKE_EDGE || opts.lossy_intake || ringing > ringing_gate;
    let noise_sigmas = if soft_intake {
        color::SOFT_NOISE_SIGMAS
    } else {
        color::NOISE_SIGMAS
    };
    // The ramp between two inks on a soft intake is a family of colours that are not
    // inks; see `color::SOFT_SAME_INK_DE00`.
    let same_ink_de00 = if soft_intake {
        color::SOFT_SAME_INK_DE00
    } else {
        color::SAME_INK_DE00
    };
    crate::diag!(
        "intake",
        "w={} h={} edge_width={edge_width:.3} ringing={ringing:.4} lossy={} noise_guard={noise_sigmas} same_ink={same_ink_de00:.2}",
        img.width,
        img.height,
        opts.lossy_intake
    );
    if !soft_intake {
        // The palette's noise guard is switched by this and by nothing else, so a reader
        // asking why an upscaled image kept 13 inks needs to see that the gate was shut.
        diag::saturated(
            "intake",
            "edge_width",
            edge_width,
            color::SOFT_INTAKE_EDGE,
            diag::Stop::GateNeverFired,
        );
    }
    if inkvec_core::env::flag("INKVEC_PALDBG") {
        eprintln!(
            "  intake edge width {edge_width:.2} px, ringing {ringing:.4} (gate {ringing_gate}), lossy {} -> noise guard {noise_sigmas} sigma",
            opts.lossy_intake
        );
    }
    // The image's distinct colours, numbered once for the palette and the labels.
    let ids = color::distinct::ColourIds::of_rgb(&rgb);
    let pal = color::extract_palette_mdl_ids(
        &rgb,
        &ids,
        img.width,
        img.height,
        opts.merge_distance,
        opts.max_colors,
        color::PaletteEvidence {
            sigma_noise,
            lambda: gradient::bic_lambda(img.width * img.height),
            noise_sigmas,
            same_ink_de00,
        },
    );
    sw.mark("palette");
    progress::note(|| format!("{} inks", pal.colors.len()));
    progress::begin("labels");
    let mut pal = pal;
    let mut labels = color::label_image_ids(&rgb, &ids, &pal);
    drop(ids);
    if opts.lossy_intake && research_lossy_regularize() {
        let sigma_lossy = sigma_noise.max(regularize::residual_sigma(
            &rgb, &labels, img.width, img.height, &pal,
        ));
        let changed =
            regularize::labels(&rgb, &mut labels, img.width, img.height, &pal, sigma_lossy);
        if !inkvec_core::env::flag("INKVEC_LOSSY_KEEP_NOISE") {
            sigma_noise = sigma_lossy;
        }
        eprintln!(
            "  lossy regularization: {changed} label updates, noise {:.2}/255",
            sigma_lossy * 255.0
        );
    }
    // MEASURE the noise now that there are labels to measure it against, and only when the
    // intake has already given positive evidence of damage.
    //
    // `coverage::estimate_noise` cannot do this. It runs before the palette exists, takes a
    // median over every pixel, and vector art is 90% exactly flat, so it returns NOISE_FLOOR
    // for a clean render and a quality-35 JPEG alike (measured: the identical 0.00196 in both
    // cases, while the true deviation rises to 0.0115). Everything downstream that divides by
    // sigma was therefore dividing by a constant on exactly the input that needed it most.
    //
    // `regularize::residual_sigma` has none of those problems: it measures how far each
    // interior pixel sits from its OWN ink, and at a boundary it projects out the direction
    // the anti-aliasing ramp is allowed to move along, so only compression damage is counted.
    // It already existed, and was reachable only behind a CLI flag AND an environment
    // variable, so in practice it never ran. It is one pass over the pixels.
    //
    // Gated on `soft_intake`, which is the same positive evidence that opens the palette
    // guard: a wide edge, a lossy container, or measured ringing. On a clean intake this
    // branch is not taken and the trace is bit-identical to before. It can only ever RAISE
    // the estimate, never lower it, and `residual_sigma` clamps itself to 8 display levels.
    // With the diagnostic on, MEASURE even when the gate is shut, so it can say what the gate
    // is turning down. `coverage::ringing_score` is built around a JPEG artefact and fires on
    // only 21% of VAE output against 88% of JPEG, so the measurement is the more general
    // signal and this is how we find out whether it can gate itself. Without the diagnostic,
    // a shut gate and a clean build read neither number, so neither is computed (on a 2048 px
    // render the two passes cost 50 ms), and the gated branch reuses this measurement.
    let measure = soft_intake || diag::on() || cfg!(feature = "research");
    let measured_always = if measure {
        regularize::residual_sigma(&rgb, &labels, img.width, img.height, &pal)
    } else {
        coverage::NOISE_FLOOR
    };
    if diag::on() {
        let incoherence =
            regularize::residual_incoherence(&rgb, &labels, img.width, img.height, &pal);
        crate::diag!(
            "noise",
            "residual_sigma={measured_always:.5} levels={:.2} incoherence={incoherence:.3} gate={}",
            measured_always * 255.0,
            soft_intake
        );
    }
    if soft_intake {
        let scale = color::MEASURED_SIGMA_SCALE;
        // `residual_sigma` clamps itself to 8 display levels, which is far too generous for
        // thin-feature content: a diagram is mostly text, and an assumed noise of 8 levels
        // merges a glyph stroke into its background. Measured per class at quality 60, the
        // median escalation is 1.6 levels on diagrams against 3.9 to 4.9 elsewhere, but the
        // diagram TAIL reaches the 8-level ceiling, and those are exactly the files whose
        // colour error doubled. The cap is therefore set from the tail, not the median.
        let cap = color::MEASURED_SIGMA_CAP / 255.0;
        let measured = (measured_always * scale).min(cap);
        if measured > sigma_noise {
            crate::diag!(
                "noise",
                "sigma raised by measurement {:.5} -> {:.5} ({:.2} -> {:.2} levels)",
                sigma_noise,
                measured,
                sigma_noise * 255.0,
                measured * 255.0
            );
            sigma_noise = measured;
        }
    }

    // EXPERIMENT: five ink-grouping rules (`ink_ideas`). Compiled only with the `research`
    // feature, and inert even then unless INKVEC_INK_IDEA is set. Placed after the
    // measured-sigma step so every rule sees the same labels and the same noise level the
    // shipped trace goes on to use.
    #[cfg(feature = "research")]
    if let Some(idea) = inkvec_core::env::text("INKVEC_INK_IDEA") {
        let distinct = |ls: &[u16]| ls.iter().collect::<std::collections::HashSet<_>>().len();
        let inks_before = distinct(&labels);
        let t0 = std::time::Instant::now();
        let changed = ink_ideas::apply(
            &idea,
            &rgb,
            &mut labels,
            &pal,
            &ink_ideas::Ctx {
                w: img.width,
                h: img.height,
                sigma: sigma_noise,
                measured_sigma: measured_always,
                edge_width,
            },
        );
        crate::diag!(
            "inks",
            "idea={idea} relabelled={changed} inks={inks_before}->{} ms={}",
            distinct(&labels),
            t0.elapsed().as_millis()
        );
    }

    if let Some(a) = source_alpha {
        if opts.alpha_inks && a.len() == labels.len() {
            color::split_alpha_inks(&mut labels, &mut pal, a);
        }
    }
    sw.mark("labels");
    progress::begin("despeckle");

    despeckle(&mut labels, img.width, img.height, min_region);
    sw.mark("despeckle");
    progress::begin("blend_absorb");

    // Anti-aliased pixels between two inks are blends of those inks, but `label_image`
    // can only hand them the *nearest palette entry* - often a third colour entirely.
    // Those pixels then form thin sliver faces along every boundary, and each sliver
    // mints two junctions per couple of pixels: the boundary the fitter finally sees is
    // confetti before any curve model gets a say. Measured on `mosaic_grid6` (36 flat
    // cells): 128 faces and 368 edges, where the truth has 37 regions.
    if !inkvec_core::env::flag("INKVEC_NO_ABSORB") {
        let alpha: Vec<f32> = match source_alpha {
            Some(a) if a.len() == img.width * img.height => a.to_vec(),
            _ => (0..img.width * img.height)
                .map(|p| img.data[p * 4 + 3])
                .collect(),
        };
        let absorbed = absorb_blend_slivers(
            &mut labels,
            &rgb,
            &alpha,
            img.width,
            img.height,
            &pal,
            sigma_noise,
        );
        let moved = reassign_blend_pixels(
            &mut labels,
            &rgb,
            &alpha,
            img.width,
            img.height,
            &pal,
            sigma_noise,
        );
        if absorbed > 0 || moved > 0 {
            despeckle(&mut labels, img.width, img.height, min_region);
        }
    }
    sw.mark("blend_absorb");
    progress::begin("merge_bands");
    if let Some(path) = inkvec_core::env::path("INKVEC_DUMP_LABELS") {
        dump_labels(path.as_os_str(), &labels, &pal, img.width, img.height);
    }

    // Merge palette bands that are really one gradient, before anything downstream sees
    // them as separate regions.
    //
    // A smooth gradient has no flat regions to find, so palette extraction quantises it
    // into bands. Left alone, each band becomes its own face with its own fill — a single
    // radial gradient came back as four concentric radial gradients sharing a centre and
    // an inner stop, costing 102 parameters where the source spent 9. Merging asks
    // whether one gradient explains two adjacent bands better than two fills do, under
    // the same MDL objective used everywhere else.
    let (mut fills_by_label, mut label_ink) = if opts.gradients {
        gradient::merge_gradient_bands_with_ink(
            &mut labels,
            &rgb,
            img.width,
            img.height,
            &pal,
            sigma_noise,
            gradient::bic_lambda(img.width * img.height),
            opts.deadline,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    sw.mark("merge_bands");
    progress::note(|| {
        let g = fills_by_label
            .iter()
            .filter(|f| f.model.is_gradient())
            .count();
        format!("{g} gradient fill{}", if g == 1 { "" } else { "s" })
    });
    progress::begin("carve");

    // The residual against the FITTED fill, which is the one damage signal with no codec
    // bias and no gradient confound.
    //
    // Three earlier candidates each failed on one of those two axes, measured on 218 images
    // under clean / JPEG 75 / JPEG 40 / WebP 70 / VAE:
    //   * `coverage::ringing_score` looks for Gibbs ringing, a JPEG artefact. Zero false
    //     positives on clean, but it catches 86% of JPEG, 3% of WebP and 16% of VAE.
    //   * `regularize::residual_sigma` compares pixels to a FLAT ink, so clean gradient art
    //     inflates it: a threshold catching 98% of VAE fires on 36% of clean images.
    //   * the Laplacian of that residual separated nothing at all (clean 1.12 levels, VAE
    //     0.86): at pixel scale it measures anti-aliasing, not damage.
    // What is left is to compare pixels against the model the tracer actually intends to
    // emit. A clean gradient is then explained and contributes nothing; damage of any origin
    // is not, and contributes. `FillFit.chi2` already holds exactly that, per label, and was
    // never summed. It is normalised back to display levels here so a threshold can be
    // written in units a designer would recognise.
    let fitted_residual = {
        let mut counts = vec![0usize; fills_by_label.len()];
        for &l in labels.iter() {
            if let Some(c) = counts.get_mut(l as usize) {
                *c += 1;
            }
        }
        let (mut chi2, mut n) = (0f64, 0usize);
        for (f, &c) in fills_by_label.iter().zip(counts.iter()) {
            if c > 0 && f.chi2.is_finite() {
                chi2 += f.chi2;
                n += c * 3;
            }
        }
        if n > 0 {
            (chi2 / n as f64).sqrt() * sigma_noise * 255.0
        } else {
            0.0
        }
    };
    crate::diag!("noise", "fitted_residual={fitted_residual:.3} levels");

    // A feature the palette quantised into its surroundings — a seam, a dot, a stroke
    // end — is a cluster of pixels no fill explains. Carve it out as its own region
    // before the fitter is left to explain it with a gradient. See
    // `gradient::carve_residual_features`.
    if opts.gradients && !inkvec_core::env::flag("INKVEC_NO_CARVE") {
        let carved = gradient::carve_residual_features_with_detail_noise(
            &mut labels,
            &rgb,
            img.width,
            img.height,
            &pal,
            &mut fills_by_label,
            &mut label_ink,
            sigma_noise,
            gradient::bic_lambda(img.width * img.height),
            min_region.max(2),
            (opts.lossy_intake && research_lossy_regularize()).then_some(detail_sigma),
        );
        if carved > 0 && inkvec_core::env::flag("INKVEC_TIMING") {
            eprintln!("  [t] carved {carved} residual feature(s) into regions");
        }
    }
    sw.mark("carve");
    progress::begin("split");

    // A face is a *connected region*, not "everywhere this colour appears".
    //
    // Assigning faces by palette index alone merges every disconnected shape of the same
    // colour into one face. That is wrong structurally — two separate shapes are two
    // objects, and an editor should be able to move one — and it silently corrupts any
    // per-face measurement. Concentric rings quantised to six palette entries put several
    // non-adjacent annuli of slightly different colour into a single "region", whose flat
    // residual was then a thousand times what noise could explain, so the fill fitter
    // dutifully explained the difference as a radial gradient. It was reporting this bug,
    // not committing one.
    let (labels, face_src) = split_components(&labels, img.width, img.height);
    sw.mark("split");
    let n_faces = face_src.len();

    // Each face inherits the fill of the label it came from, and a representative colour
    // for boundary unmixing: a gradient's midpoint stands in for the whole region there,
    // which is enough to locate an edge against its neighbour.
    let face_fill: Vec<gradient::FillFit> = face_src
        .iter()
        .map(|&l| {
            fills_by_label
                .get(l)
                .cloned()
                .unwrap_or_else(|| gradient::FillFit {
                    model: gradient::FillModel::Flat(
                        pal.rgb.get(l).copied().unwrap_or([0.0, 0.0, 0.0]),
                    ),
                    chi2: 0.0,
                    params: gradient::PARAMS_FLAT,
                    cost: 0.0,
                })
        })
        .collect();
    // A label the merger minted (a gradient, or a flat region given its own colour) is
    // not a palette index; map it back to the entry it was cut from.
    let face_color: Vec<usize> = face_src
        .iter()
        .map(|&l| label_ink.get(l).copied().unwrap_or(l))
        .collect();

    finish_color_trace(
        img,
        opts,
        &rgb,
        pal,
        labels,
        face_fill,
        face_color,
        n_faces,
        sigma_noise,
        &mut sw,
    )
}

/// Research entry: run the colour tracer from a caller-supplied label map.
///
/// `labels[y*w+x]` is a region id in `0..n_labels` (any value >= `n_labels` is treated as
/// unlabelled and reassigned to the nearest labelled 4-neighbour by repeated dilation; a
/// `labels` shorter than the image leaves the rest unlabelled). `img` is straight RGBA in
/// `[0, 1]`, read composited onto white. Returns a [`ColorTrace`] exactly as
/// [`trace_color_full`] does, with the palette standing for the supplied labels.
///
/// From there this is the classical path, stage for stage, differing only in where the
/// labels came from:
///
/// 1. the same noise estimate, because everything below divides by it;
/// 2. unlabelled pixels closed by dilation, then `despeckle`, as on the classical path;
/// 3. a [`Palette`] built from the supplied partition — one entry per label id, its
///    colour the per-channel median of the label's pixels, which is the statistic
///    `fit_flat` uses and carries the classical palette's meaning, "representative ink
///    colour". A label with no pixels gets a black entry of weight zero;
/// 4. when `opts.gradients`, [`gradient::merge_gradient_bands_with_ink`] and (unless
///    `INKVEC_NO_CARVE` is set) [`gradient::carve_residual_features_with_detail_noise`],
///    with the same lambda and sigma the classical path passes. These are why the fills
///    are *not* fitted per face here: the merge stage returns one fill per label, and a
///    per-face fit would re-shatter a gradient the merge stage had just reassembled;
/// 5. `split_components`, then `face_fill` / `face_color` read off `fills_by_label` /
///    `label_ink` exactly as above;
/// 6. `finish_color_trace`: saddles, planar map, sub-pixel and junction refinement, the
///    global boundary solve, decoding and symmetry.
///
/// What is *not* run: palette extraction (`extract_palette_mdl`), `label_image`, alpha-ink
/// splitting and blend absorption. Those stages exist to turn an image into a partition,
/// and a caller supplying a label map is claiming to have done that job already.
///
/// With `opts.gradients` false the fills are flat palette colours, which is what the
/// classical path's `else` branch leaves behind.
pub fn trace_color_from_labels(
    img: &Rgba,
    opts: &ColorOptions,
    labels: &[u16],
    n_labels: usize,
) -> ColorTrace {
    let (w, h) = (img.width, img.height);
    let n = w * h;
    let rgb = img.composited([1.0, 1.0, 1.0]);

    // The same noise estimate the classical path makes, for the same reason: everything
    // below that divides by sigma is measuring evidence against it.
    let lum: Vec<f32> = rgb
        .iter()
        .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
        .collect();
    let sigma_noise = coverage::estimate_noise(&lum, w, h);
    if (sigma_noise - coverage::NOISE_FLOOR).abs() < 1e-12 {
        diag::saturated(
            "noise",
            "sigma_noise",
            sigma_noise,
            coverage::NOISE_FLOOR,
            diag::Stop::Floor,
        );
    } else {
        crate::diag!(
            "noise",
            "sigma_noise={sigma_noise:.5} levels={:.2}",
            sigma_noise * 255.0
        );
    }
    let lambda = gradient::bic_lambda(n);

    let mut sw = Stopwatch::start();
    progress::begin("labels_in");

    // Anything the caller did not label is filled in from its labelled 4-neighbours, one
    // ring at a time, so an unlabelled band along a boundary closes from both sides at
    // once rather than being swept in from one.
    let mut labels: Vec<u16> = (0..n)
        .map(|p| match labels.get(p) {
            Some(&l) if (l as usize) < n_labels => l,
            _ => u16::MAX,
        })
        .collect();
    if labels.iter().all(|&l| l == u16::MAX) {
        // Nothing was labelled at all; one face is the only honest answer.
        labels.iter_mut().for_each(|l| *l = 0);
    } else {
        while labels.contains(&u16::MAX) {
            let prev = labels.clone();
            for y in 0..h {
                for x in 0..w {
                    let p = y * w + x;
                    if prev[p] != u16::MAX {
                        continue;
                    }
                    let mut take = |q: usize| {
                        if labels[p] == u16::MAX && prev[q] != u16::MAX {
                            labels[p] = prev[q];
                        }
                    };
                    if x > 0 {
                        take(p - 1);
                    }
                    if x + 1 < w {
                        take(p + 1);
                    }
                    if y > 0 {
                        take(p - w);
                    }
                    if y + 1 < h {
                        take(p + w);
                    }
                }
            }
        }
    }
    sw.mark("labels_in");
    progress::begin("despeckle");

    despeckle(&mut labels, w, h, opts.min_region);
    sw.mark("despeckle");
    progress::begin("palette");

    // One palette entry per supplied label, standing in for the one `extract_palette_mdl`
    // would have produced. The stages below read `pal.rgb[l]` as "the ink this region is
    // drawn in": band merging uses it to decide whether a pixel is a blend of two inks,
    // carving uses it to name the region a carved feature was cut from, and boundary
    // unmixing uses it for any label the merge stage left without a fill of its own. The
    // per-channel median is the statistic `fit_flat` uses, and it is the one that
    // survives a label whose region swallowed a feature: a mean would drag the ink
    // colour towards whatever the labelling got wrong.
    let mut px_by_label: Vec<Vec<usize>> = vec![Vec::new(); n_labels.max(1)];
    for (p, &l) in labels.iter().enumerate() {
        if let Some(v) = px_by_label.get_mut(l as usize) {
            v.push(p);
        }
    }
    let denom = n.max(1) as f32;
    let pal_rgb: Vec<[f32; 3]> = px_by_label
        .iter()
        .map(|px| {
            if px.is_empty() {
                return [0.0, 0.0, 0.0];
            }
            let mut c = [0.0f32; 3];
            for (k, ck) in c.iter_mut().enumerate() {
                let mut v: Vec<f32> = px.iter().map(|&p| rgb[p][k]).collect();
                let m = v.len() / 2;
                let (_, med, _) = v.select_nth_unstable_by(m, |a, b| a.total_cmp(b));
                *ck = *med;
            }
            c
        })
        .collect();
    let pal = Palette {
        colors: pal_rgb.iter().map(|&c| color::rgb_to_oklab(c)).collect(),
        rgb: pal_rgb,
        weight: px_by_label.iter().map(|v| v.len() as f32 / denom).collect(),
        alpha: vec![1.0; px_by_label.len()],
    };
    drop(px_by_label);
    sw.mark("palette");
    progress::begin("merge_bands");

    // From here to `split_components` this is the classical path verbatim. A label map is
    // a statement about *where* regions are; it is not a statement that each region has
    // one flat colour, and the two MDL stages that turn quantised bands back into the
    // gradient that made them are as necessary here as they are there. Skipping them cost
    // colour accuracy on gradient-heavy emoji while inflating the path count, which is the
    // exact disease they were written for.
    let (mut fills_by_label, mut label_ink) = if opts.gradients {
        gradient::merge_gradient_bands_with_ink(
            &mut labels,
            &rgb,
            w,
            h,
            &pal,
            sigma_noise,
            lambda,
            opts.deadline,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    sw.mark("merge_bands");
    progress::begin("carve");

    // A feature the labelling quantised into its surroundings — a seam, a dot, a stroke
    // end — is a cluster of pixels no fill explains. A supplied label map swallows those
    // the same way a palette does, and more of them. No separate interior noise estimate:
    // that exists on the classical path to undo what lossy regularisation did to
    // `sigma_noise`, and nothing here raises it.
    if opts.gradients && !inkvec_core::env::flag("INKVEC_NO_CARVE") {
        let carved = gradient::carve_residual_features_with_detail_noise(
            &mut labels,
            &rgb,
            w,
            h,
            &pal,
            &mut fills_by_label,
            &mut label_ink,
            sigma_noise,
            lambda,
            opts.min_region.max(2),
            None,
        );
        if carved > 0 && inkvec_core::env::flag("INKVEC_TIMING") {
            eprintln!("  [t] carved {carved} residual feature(s) into regions");
        }
    }
    sw.mark("carve");
    progress::begin("split");

    // A face is a connected region, exactly as on the classical path: two disconnected
    // blobs the network gave the same id are two shapes, and an editor should be able to
    // move one of them.
    let (labels, face_src) = split_components(&labels, w, h);
    let n_faces = face_src.len();
    sw.mark("split");

    // Each face inherits the fill of the label it came from, and the palette entry that
    // label was cut from — the same derivation as on the classical path, which is what
    // lets `merge_saddle_faces` and symmetry compare faces by ink.
    let face_fill: Vec<gradient::FillFit> = face_src
        .iter()
        .map(|&l| {
            fills_by_label
                .get(l)
                .cloned()
                .unwrap_or_else(|| gradient::FillFit {
                    model: gradient::FillModel::Flat(
                        pal.rgb.get(l).copied().unwrap_or([0.0, 0.0, 0.0]),
                    ),
                    chi2: 0.0,
                    params: gradient::PARAMS_FLAT,
                    cost: 0.0,
                })
        })
        .collect();
    let face_color: Vec<usize> = face_src
        .iter()
        .map(|&l| label_ink.get(l).copied().unwrap_or(l))
        .collect();

    finish_color_trace(
        img,
        opts,
        &rgb,
        pal,
        labels,
        face_fill,
        face_color,
        n_faces,
        sigma_noise,
        &mut sw,
    )
}

/// Everything from saddle disambiguation onwards: the geometry stages, which do not care
/// how the labels were arrived at.
///
/// Shared by [`trace_color_full_with_alpha`] and [`trace_color_from_labels`] so the
/// research entry cannot drift away from the shipped one. The opaque form of
/// [`finish_color_trace_alpha`], which documents the arguments.
#[allow(clippy::too_many_arguments)]
fn finish_color_trace(
    img: &Rgba,
    opts: &ColorOptions,
    rgb: &[[f32; 3]],
    pal: Palette,
    labels: Vec<u16>,
    face_fill: Vec<gradient::FillFit>,
    face_color: Vec<usize>,
    n_faces: usize,
    sigma_noise: f64,
    sw: &mut Stopwatch,
) -> ColorTrace {
    finish_color_trace_alpha(
        img,
        opts,
        rgb,
        pal,
        labels,
        face_fill,
        face_color,
        n_faces,
        sigma_noise,
        sw,
        None,
        None,
    )
}

/// [`finish_color_trace`], told the source's alpha. The sub-pixel refinement and the
/// boundary solve then unmix in four channels, each face at its palette entry's opacity, so
/// an edge between white paint and the clear ground is found although over white it has no
/// contrast at all. With `None` this is exactly the classic function.
///
/// # Stages
///
/// saddles (research only) → `build_map` ([`planar::build`]) → `symmetry_detect` →
/// `refine_subpix` ([`planar::refine_subpixel_alpha`]) → `refine_junc` → `boundary_opt`
/// (skipped in Fast mode or with `INKVEC_BOPT=0`) → decode (research only) → `symmetry`
/// ([`symmetry::enforce`]). Stages 10–17 of the crate overview.
///
/// # Arguments
///
/// * `rgb`: the image composited onto white, sRGB `[0, 1]`, row-major;
/// * `pal`: the palette, moved into the result;
/// * `labels`: the **face** map (one id per 4-connected component, from
///   [`regions::split_components`]), with `n_faces` ids;
/// * `face_fill`, `face_color`: per face, its fitted fill and its palette index;
/// * `sigma_noise`: per-channel noise in sRGB units, which sets how far the sub-pixel
///   refinement trusts each pixel;
/// * `sw`: the caller's stopwatch, so stage timings continue in one sequence;
/// * `source_alpha`: the source's alpha per pixel, or `None` on the opaque path;
/// * `face_alpha_override`: per face, the opacity at which it meets the ground (the
///   native path's fades and washes); used only with `source_alpha` and when its length
///   matches `face_color`, otherwise each face takes its ink's `pal.alpha`.
///
/// Returns the finished [`ColorTrace`], with `face_fade` empty (the native path fills it).
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_color_trace_alpha(
    img: &Rgba,
    opts: &ColorOptions,
    rgb: &[[f32; 3]],
    pal: Palette,
    labels: Vec<u16>,
    face_fill: Vec<gradient::FillFit>,
    face_color: Vec<usize>,
    n_faces: usize,
    sigma_noise: f64,
    sw: &mut Stopwatch,
    source_alpha: Option<&[f32]>,
    face_alpha_override: Option<Vec<f32>>,
) -> ColorTrace {
    // Four pixels meeting at one corner are the one thing the labels cannot settle on
    // their own. Ask the image, and record the answer where the map can read it. An
    // experiment (`INKVEC_SADDLE`), compiled only in a `research` build.
    #[cfg(feature = "research")]
    let (labels, mut face_fill, face_color, n_faces) = merge_saddle_faces(
        labels,
        img.width,
        img.height,
        rgb,
        &pal,
        face_fill,
        face_color,
        n_faces,
        sigma_noise,
    );
    sw.mark("saddles");
    progress::begin("build_map");

    let mut map = planar::build(&labels, img.width, img.height, n_faces);
    sw.mark("build_map");
    let (edges, faces) = (map.edges.len(), map.n_labels);
    progress::note(|| format!("{edges} boundaries between {faces} faces"));
    let face_model: Vec<gradient::FillModel> = face_fill.iter().map(|f| f.model.clone()).collect();
    let face_alpha: Option<Vec<f32>> = source_alpha.map(|_| {
        face_alpha_override
            .filter(|o| o.len() == face_color.len())
            .unwrap_or_else(|| {
                face_color
                    .iter()
                    .map(|&c| pal.alpha.get(c).copied().unwrap_or(1.0))
                    .collect()
            })
    });
    let alpha_pair = source_alpha.zip(face_alpha.as_deref());
    sw.mark("symmetry_detect");
    progress::begin("refine_subpix");
    // Symmetry is found here, on the lattice the extractor produced, where the comparison
    // is exact, and applied further down, once every stage that can break a tie has had
    // its turn. The sub-pixel refinement's measuring phase reads the same lattice map and
    // writes nothing, so the two run side by side (`rayon::join`) and the refinement is
    // written back afterwards; each computes exactly what it computed alone, since
    // neither sees the other's output (see `planar::measure_subpixel`). Their time is
    // reported together as `refine_subpix`; `symmetry_detect` now only marks the setup
    // above. Where rayon has one thread (the WebAssembly build) `join` runs the two in
    // turn, with no work added.
    //
    // Inspired by: Ragan-Kelley et al. 2013, "Halide: a language and compiler for
    // optimizing parallelism, locality, and recomputation in image processing pipelines",
    // PLDI 2013, 519–530, <https://doi.org/10.1145/2491956.2462176>: independent pipeline
    // stages scheduled to run concurrently. Here the schedule is written by hand for one
    // pair of stages, not derived by a compiler.
    let measure = || {
        planar::measure_subpixel(
            &map,
            rgb,
            &face_model,
            sigma_noise,
            opts.simplify_faint,
            alpha_pair,
        )
    };
    // A small map is refined on this thread, after detection, as before: handing it to the
    // pool costs more than it saves (see `planar::refine_in_parallel`).
    let (sym, refined) = if planar::refine_in_parallel(&map) {
        rayon::join(|| symmetry::detect(&map, &labels, &face_color), measure)
    } else {
        (symmetry::detect(&map, &labels, &face_color), measure())
    };
    refined.apply(&mut map);
    sw.mark("refine_subpix");
    progress::begin("refine_junc");
    planar::refine_junctions(&mut map);
    sw.mark("refine_junc");
    progress::begin("boundary_opt");

    // Then solve the whole boundary against the image at once: every point above was
    // placed by a one-dimensional argument of its own, and a pixel's value is the area
    // each face covers in it, so neighbouring points share evidence and have to be moved
    // together. See `boundary_opt`.
    let boundary_opt = if !opts.fast && inkvec_core::env::switch("INKVEC_BOPT", true) {
        boundary_opt::optimise_alpha(&mut map, rgb, &face_model, opts.boundary_ms, alpha_pair)
    } else {
        None
    };
    sw.mark("boundary_opt");
    if let Some(r) = boundary_opt.as_ref() {
        let (from, to, n) = (r.before, r.after, r.iters);
        progress::note(|| format!("energy {from:.0} to {to:.0} in {n} iterations"));
    }

    // A face too thin to own a fully covered pixel never had its colour read off the
    // image: the palette saw only blends. Its boundary was then fitted against that
    // biased colour. Fix the model order first and solve the two together. Research only.
    #[cfg(feature = "research")]
    let decode = if !opts.fast && inkvec_core::env::flag("INKVEC_DECODE") {
        decode::decode_faces(
            &mut map,
            rgb,
            &labels,
            &mut face_fill,
            gradient::bic_lambda(img.width * img.height),
            opts.deadline.map(|_| decode::BUDGETED_MS),
        )
    } else {
        None
    };
    sw.mark("decode");
    let face_rgb: Vec<[f32; 3]> = face_fill.iter().map(|f| f.model.representative()).collect();

    // The label map is exactly symmetric whenever the artist's drawing was, and every
    // stage above breaks that symmetry a little by breaking ties. Put it back.
    let symmetrised = if sym.is_empty() {
        0
    } else {
        symmetry::enforce(&mut map, &sym)
    };
    sw.mark("symmetry");
    ColorTrace {
        map,
        symmetry: sym,
        symmetrised,
        boundary_opt,
        #[cfg(feature = "research")]
        decode,
        palette: pal,
        labels,
        face_color,
        face_fill,
        face_rgb,
        sigma_noise,
        face_fade: Vec::new(),
    }
}

/// `INKVEC_LOSSY_REGULARIZE`: re-label a lossy intake with `regularize::labels`, an
/// experiment never switched on by default. Research builds only.
fn research_lossy_regularize() -> bool {
    cfg!(feature = "research") && inkvec_core::env::flag("INKVEC_LOSSY_REGULARIZE")
}

/// Stopwatch for logging wall-clock timing across tracing stages.
///
/// Each [`Stopwatch::mark`] closes one stage: it reports the milliseconds since the
/// previous mark to stderr when `INKVEC_TIMING` is set, to the thread's stage sink
/// ([`with_stage_sink`]) and to the progress log, and then restarts the clock.
pub struct Stopwatch {
    on: bool,
    t: clock::Instant,
}

/// A sink for stage boundaries: the stage's name, and how long it took in milliseconds.
type StageSink = std::rc::Rc<dyn Fn(&str, f64)>;

thread_local! {
    /// The sink installed on this thread by [`with_stage_sink`], if any.
    static STAGE_SINK: std::cell::RefCell<Option<StageSink>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` with `sink` receiving every stage boundary the pipeline passes on this thread,
/// as a stage name and the milliseconds that stage took.
///
/// This is the seam an embedder needs to show a trace *as it happens* rather than after
/// it. A trace of a real logo takes about a second, and an interface has to spend that
/// second saying something true; without this the alternatives are a bare spinner or a
/// fabricated sequence of stages, and neither is worth offering.
///
/// The names are the pipeline's own internal stage names (`palette`, `carve`,
/// `boundary_opt`, `fit_dp`, ...), which is what makes them worth reporting: they are the
/// real boundaries rather than a display schedule. They are **not** a stable interface —
/// stages get added, split and renamed as the pipeline changes — so a caller must treat an
/// unrecognised name as "some stage" and must not assume a fixed set, count or order.
///
/// Thread-local by design. The pipeline marks stages on the thread that drives it and does
/// its parallel work below that level, so a caller tracing several images at once (a batch
/// queue on a rayon pool) gets each image's stages on its own thread with no interleaving.
/// Nothing is reported from rayon's workers.
///
/// The sink runs on the pipeline's thread, in the middle of a trace. It should hand the
/// value off and return, and it must not call back into the tracer.
///
/// ```
/// use std::{cell::RefCell, rc::Rc};
///
/// let seen = Rc::new(RefCell::new(Vec::new()));
/// let sink = Rc::clone(&seen);
/// inkvec_trace::with_stage_sink(
///     move |name, _ms| sink.borrow_mut().push(name.to_string()),
///     || inkvec_trace::Stopwatch::start().mark("palette"),
/// );
/// assert_eq!(*seen.borrow(), ["palette"]);
/// ```
pub fn with_stage_sink<R>(sink: impl Fn(&str, f64) + 'static, f: impl FnOnce() -> R) -> R {
    /// Puts back whatever sink was installed before, on every path out of the call —
    /// including a panic inside `f`, which would otherwise leave a stale sink behind on a
    /// thread that goes on to be reused.
    struct Restore(Option<StageSink>);
    impl Drop for Restore {
        fn drop(&mut self) {
            STAGE_SINK.with(|s| *s.borrow_mut() = self.0.take());
        }
    }

    let installed: StageSink = std::rc::Rc::new(sink);
    let _guard = Restore(STAGE_SINK.with(|s| s.borrow_mut().replace(installed)));
    f()
}

impl Stopwatch {
    /// Start a stopwatch, enabled if the `INKVEC_TIMING` environment variable is set.
    pub fn start() -> Self {
        Self {
            on: inkvec_core::env::flag("INKVEC_TIMING"),
            t: clock::Instant::now(),
        }
    }

    /// Log elapsed time since previous mark and reset the baseline.
    ///
    /// Also reports the boundary to the sink [`with_stage_sink`] installed on this
    /// thread, if there is one. With no sink and no `INKVEC_TIMING` this is a thread-local
    /// read and a clock sample, which is what it already was.
    pub fn mark(&mut self, name: &str) {
        let ms = self.t.elapsed().as_secs_f64() * 1e3;
        if self.on {
            eprintln!("  [t] {name:<16} {ms:>9.1} ms");
        }
        // Cloned out of the cell before being called, so a sink that somehow reached back
        // into this thread would not find the RefCell already borrowed.
        let sink = STAGE_SINK.with(|s| s.borrow().clone());
        if let Some(sink) = sink {
            sink(name, ms);
        }
        // The live log's copy of the same boundary, and a cancellation point: a stage
        // boundary is always a safe place to stop. See `inkvec_core::progress`.
        progress::end(name, ms);
        self.t = clock::Instant::now();
    }
}

#[cfg(test)]
mod stage_sink_tests {
    use super::{with_stage_sink, Stopwatch};
    use std::{cell::RefCell, rc::Rc};

    fn recorder() -> (Rc<RefCell<Vec<String>>>, impl Fn(&str, f64) + 'static) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        (seen, move |n: &str, _ms: f64| {
            sink.borrow_mut().push(n.to_string())
        })
    }

    #[test]
    fn marks_reach_the_installed_sink() {
        let (seen, sink) = recorder();
        with_stage_sink(sink, || {
            let mut sw = Stopwatch::start();
            sw.mark("palette");
            sw.mark("carve");
        });
        assert_eq!(*seen.borrow(), ["palette", "carve"]);
    }

    #[test]
    fn a_stage_is_timed_from_the_previous_mark() {
        let elapsed = Rc::new(RefCell::new(0.0f64));
        let sink = Rc::clone(&elapsed);
        with_stage_sink(
            move |_, ms| *sink.borrow_mut() = ms,
            || {
                let mut sw = Stopwatch::start();
                std::thread::sleep(std::time::Duration::from_millis(12));
                sw.mark("slow");
            },
        );
        assert!(*elapsed.borrow() >= 10.0, "{}", elapsed.borrow());
    }

    #[test]
    fn no_sink_installed_is_a_no_op() {
        Stopwatch::start().mark("palette");
    }

    #[test]
    fn the_previous_sink_is_restored_afterwards() {
        let (outer_seen, outer_sink) = recorder();
        with_stage_sink(outer_sink, || {
            let (inner_seen, inner_sink) = recorder();
            with_stage_sink(inner_sink, || Stopwatch::start().mark("nested"));
            assert_eq!(*inner_seen.borrow(), ["nested"]);
            Stopwatch::start().mark("after");
        });
        assert_eq!(*outer_seen.borrow(), ["after"]);
    }

    #[test]
    fn a_panic_does_not_leave_a_sink_behind() {
        let (seen, sink) = recorder();
        let boom = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_stage_sink(sink, || panic!("inside the trace"))
        }));
        assert!(boom.is_err());
        Stopwatch::start().mark("afterwards");
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn a_sink_on_one_thread_does_not_see_another_threads_stages() {
        let (seen, sink) = recorder();
        with_stage_sink(sink, || {
            std::thread::scope(|s| {
                s.spawn(|| Stopwatch::start().mark("elsewhere"));
            });
            Stopwatch::start().mark("here");
        });
        assert_eq!(*seen.borrow(), ["here"]);
    }
}

#[cfg(test)]
mod from_labels_tests {
    use super::*;

    /// A black square on white, labelled by hand, must come back as two faces whose fills
    /// were read off the image -- the caller supplies the partition, never the colours.
    #[test]
    fn a_supplied_label_map_traces_two_faces_and_fits_their_fills() {
        let (w, h) = (32usize, 32usize);
        let mut img = Rgba {
            width: w,
            height: h,
            data: vec![1.0; w * h * 4],
        };
        let mut labels = vec![0u16; w * h];
        for y in 10..22 {
            for x in 10..22 {
                for c in 0..3 {
                    img.data[(y * w + x) * 4 + c] = 0.0;
                }
                labels[y * w + x] = 1;
            }
        }

        let t = trace_color_from_labels(&img, &ColorOptions::default(), &labels, 2);

        assert_eq!(t.face_fill.len(), 2, "one fill per connected face");
        assert_eq!(t.palette.len(), 2);
        assert_eq!(t.face_color, vec![0, 1]);
        assert!(!t.map.edges.is_empty(), "the square has a boundary");

        let mut reps: Vec<[f32; 3]> = t
            .face_fill
            .iter()
            .map(|f| f.model.representative())
            .collect();
        reps.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
        let (dark, light) = (reps[0], reps[1]);
        for c in 0..3 {
            assert!(dark[c] < 0.05, "the square's fill is black, got {dark:?}");
            assert!(
                (light[c] - 1.0).abs() < 0.05,
                "the ground's fill is white, got {light:?}"
            );
        }
    }

    /// Unlabelled pixels are filled in from their neighbours rather than becoming a face
    /// of their own, and an entirely unlabelled image is one face.
    ///
    /// The image is flat, so nothing is drawn where the hole was: what comes back measures
    /// the dilation and only the dilation. A hole punched over artwork is the next test.
    #[test]
    fn unlabelled_pixels_are_dilated_away() {
        let (w, h) = (24usize, 24usize);
        let img = Rgba {
            width: w,
            height: h,
            data: vec![1.0; w * h * 4],
        };
        // Everything outside the square is labelled; the square itself is left blank.
        let mut labels = vec![0u16; w * h];
        for y in 8..16 {
            for x in 8..16 {
                labels[y * w + x] = u16::MAX;
            }
        }
        let t = trace_color_from_labels(&img, &ColorOptions::default(), &labels, 1);
        assert_eq!(t.face_fill.len(), 1, "the hole closed into the ground");
        assert!(t.labels.iter().all(|&l| l == 0));

        let all_blank = vec![u16::MAX; w * h];
        let t = trace_color_from_labels(&img, &ColorOptions::default(), &all_blank, 4);
        assert_eq!(t.face_fill.len(), 1);
    }

    /// A feature the supplied labelling swallowed is carved back out, exactly as on the
    /// classical path: `carve_residual_features` runs here too, and a black square inside
    /// a region called white is the cluster of pixels no fill explains that it looks for.
    #[test]
    fn a_swallowed_feature_is_carved_back_out() {
        let (w, h) = (24usize, 24usize);
        let mut img = Rgba {
            width: w,
            height: h,
            data: vec![1.0; w * h * 4],
        };
        for y in 8..16 {
            for x in 8..16 {
                for c in 0..3 {
                    img.data[(y * w + x) * 4 + c] = 0.0;
                }
            }
        }
        // One label for the whole image: the caller claims there is nothing here.
        let labels = vec![0u16; w * h];
        let t = trace_color_from_labels(&img, &ColorOptions::default(), &labels, 1);
        assert_eq!(t.face_fill.len(), 2, "the square is a region of its own");
        let reps: Vec<[f32; 3]> = t
            .face_fill
            .iter()
            .map(|f| f.model.representative())
            .collect();
        assert!(
            reps.iter().any(|c| c.iter().all(|&v| v < 0.05)),
            "one of the fills is the square's black, got {reps:?}"
        );
    }
}
