//! Fast mode's front end for an opaque image: palette and labels in two passes
//! ([`super::palette`]), speckle removal, faces, then the shared planar-map stages --
//! saddles, the map, symmetry, and the sub-pixel refinement of every boundary point.
//!
//! What it leaves out, next to [`crate::trace_color_full_with_alpha`]: the MDL palette and
//! its noise and ringing measurements, blend absorption (the labelling already sends blends
//! to a neighbour's ink), gradient band merging and residual carving, and, in
//! `finish_color_trace_alpha`, the global boundary solve and order-first decoding.

use crate::{coverage, gradient, regions, ColorOptions, ColorTrace, Rgba, Stopwatch};

/// Trace an image with transparency natively, the fast way: the native palette (it carries
/// opacity, which the histogram palette does not), no gradient or fade recovery, and the
/// fast flag's cuts in the shared stages.
pub(crate) fn trace_color_native(img: &Rgba, opts: &ColorOptions, alpha: &[f32]) -> ColorTrace {
    let opts = ColorOptions {
        gradients: false,
        ..*opts
    };
    crate::native::trace_color(img, &opts, alpha)
}

/// Trace an opaque (or matted) image the fast way.
pub(crate) fn trace_color(
    img: &Rgba,
    opts: &ColorOptions,
    source_alpha: Option<&[f32]>,
) -> ColorTrace {
    let (w, h) = (img.width, img.height);
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let mut sw = Stopwatch::start();
    let (mut pal, mut labels) =
        super::palette::palette_and_labels(&rgb, w, h, opts.merge_distance, opts.max_colors);
    if let Some(a) = source_alpha.filter(|a| opts.alpha_inks && a.len() == labels.len()) {
        crate::color::split_alpha_inks(&mut labels, &mut pal, a);
    }
    sw.mark("palette");
    regions::despeckle(&mut labels, w, h, opts.min_region);
    sw.mark("despeckle");
    let (labels, face_src) = regions::split_components(&labels, w, h);
    sw.mark("split");
    let mut face_fill: Vec<gradient::FillFit> = face_src
        .iter()
        .map(|&l| gradient::FillFit {
            model: gradient::FillModel::Flat(pal.rgb.get(l).copied().unwrap_or([0.0; 3])),
            chi2: 0.0,
            params: gradient::PARAMS_FLAT,
            cost: 0.0,
        })
        .collect();
    let mut face_color = face_src;
    let mut labels = labels;
    if opts.gradients {
        super::bands::merge_ramps(
            &rgb,
            w,
            h,
            &pal,
            &mut labels,
            &mut face_fill,
            &mut face_color,
        );
    }
    sw.mark("ramps");
    let n_faces = face_color.len();
    // No noise measurement: the fast fitter reads no per-point uncertainty, and the
    // refinement only needs a floor under its contrast test.
    crate::finish_color_trace(
        img,
        opts,
        &rgb,
        pal,
        labels,
        face_fill,
        face_color,
        n_faces,
        coverage::NOISE_FLOOR,
        &mut sw,
    )
}
