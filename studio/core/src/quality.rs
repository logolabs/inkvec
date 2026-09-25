//! The quality report: what the trace actually cost, measured rather than asserted.
//!
//! No other free tracer tells the user whether the trace was any good, and the whole
//! panel is worthless if the headline number is a guess. So the colour difference is a
//! measurement: the SVG that was just written is rendered back to pixels at the size it
//! was traced at, and compared with the source raster the tracer saw, pixel for pixel, in
//! CIEDE2000.
//!
//! Three things that keep the comparison honest:
//!
//! * **The same raster.** `inkvec_cli::intake` may cap or resample the input; the tracer
//!   works on the result, so that is what the render is compared against, not the file on
//!   disk.
//! * **The same ground.** Both sides are composited onto one neutral matte before the
//!   conversion, so a transparency the trace got wrong shows up as colour error instead
//!   of being skipped.
//! * **Mean and median.** A trace can be excellent everywhere and wrong along one seam;
//!   the mean says so and the median does not, and the panel shows both.

use resvg::tiny_skia;
use resvg::usvg;
use serde::Serialize;

/// The mid-grey both sides are composited onto before the comparison. Neutral, and far
/// from both ends of the range, so neither a white nor a black artwork is flattered.
const MATTE: [f32; 3] = [0.5, 0.5, 0.5];

/// What one trace produced, as numbers.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// Mean colour difference, dE00, against the source. `None` when the SVG could not be
    /// rendered back — the panel then says so rather than showing a made-up number.
    pub mean_de00: Option<f64>,
    /// Median colour difference, dE00.
    pub median_de00: Option<f64>,
    /// The 99th percentile: where the trace is at its worst.
    pub worst_de00: Option<f64>,
    /// Coordinates in the path data.
    pub coordinates: usize,
    /// Drawn elements: paths, circles, ellipses and rects.
    pub paths: usize,
    /// Segments (move, line, cubic, arc, close) in the path data.
    pub segments: usize,
    /// Distinct inks.
    pub colours: usize,
    /// Bytes of the SVG as written.
    pub bytes: usize,
    /// Bytes after the minifier, for the same drawing.
    pub minified_bytes: Option<usize>,
    /// How editable the drawing is: what its nodes and handles do, counted.
    pub structure: Structure,
    /// Seconds the trace took.
    pub seconds: f64,
    /// The longer side the trace ran at, in pixels.
    pub traced_px: u32,
}

/// The counts behind the editability card, as `inkvec_svgmin::structure` measures them.
///
/// The engine's own type is plain data with no serializer, so this mirrors it field for
/// field under the names the interface uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Structure {
    /// On-curve points.
    pub nodes: usize,
    /// Cubic segments.
    pub cubics: usize,
    /// Handles with any length.
    pub handles: usize,
    /// Of those, the ones on an axis.
    pub axis_handles: usize,
    /// Joins between two cubics.
    pub joins: usize,
    /// Of those, the smooth ones.
    pub smooth_joins: usize,
    /// Nodes sharing an exact x or y with another.
    pub aligned_nodes: usize,
}

impl From<inkvec_svgmin::Structure> for Structure {
    fn from(s: inkvec_svgmin::Structure) -> Self {
        Self {
            nodes: s.nodes,
            cubics: s.cubics,
            handles: s.handles,
            axis_handles: s.axis_handles,
            joins: s.joins,
            smooth_joins: s.smooth_joins,
            aligned_nodes: s.aligned_nodes,
        }
    }
}

/// One ink in the palette.
///
/// An ink is a flat colour or a gradient. The tracer writes a gradient element per face,
/// so one gradient ink can be painted through many `url(#id)` references; `keys` lists
/// every paint value that paints with it, which is what the viewer highlights on hover.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ink {
    /// The colour the tracer measured, `#rrggbb`. For a gradient, its first stop.
    pub traced: String,
    /// What it is painted as now: the traced value, or a colour the user snapped it to.
    /// For a gradient, its first stop.
    pub hex: String,
    /// Share of the canvas it covers, 0..1.
    pub share: f64,
    /// Colour difference between `traced` and `hex`, when the user has snapped it.
    pub snapped_de00: Option<f64>,
    /// Flat colour or gradient.
    pub kind: InkKind,
    /// The exact `fill` / `stroke` attribute values that paint with this ink, as written:
    /// `#aabbcc`, or `url(#g12)` for each gradient element that has these stops.
    pub keys: Vec<String>,
    /// The stop colours in offset order, `#rrggbb`; `[hex]` for a flat ink.
    pub stops: Vec<String>,
    /// Linear or radial, for a gradient; absent for a flat ink.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gradient: Option<GradientKind>,
}

/// Whether an ink is one colour or a ramp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InkKind {
    /// One colour.
    #[default]
    Flat,
    /// A `<linearGradient>` or `<radialGradient>`.
    Gradient,
}

/// Which kind of gradient element an ink was written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GradientKind {
    /// `<linearGradient>`.
    Linear,
    /// `<radialGradient>`.
    Radial,
}

/// Where the trace and the source disagree most: the region the viewer can jump to.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorstCorner {
    /// Centre of the window, in traced-raster pixels.
    pub x: u32,
    /// Centre of the window, in traced-raster pixels.
    pub y: u32,
    /// Mean dE00 inside the window.
    pub de00: f64,
}

/// Render an SVG document to straight RGBA8 at `w` x `h`.
///
/// The SVG's own viewBox is mapped onto the whole pixmap, so the render lines up with the
/// raster whatever `width`/`height` the document declares.
pub fn render(svg: &str, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let tree = parse_tree(svg)?;
    let size = tree.size();
    let scale = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    render_tree(&tree, w, h, scale)
}

/// Render an SVG document at `w` x `h` without distorting it: scaled uniformly to fit, and
/// centred, with the rest of the pixmap left transparent. What an export wants where the
/// size is fixed and the drawing's proportions are not (a square favicon of a wide logo).
pub fn render_contained(svg: &str, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let tree = parse_tree(svg)?;
    let size = tree.size();
    let k = (w as f32 / size.width()).min(h as f32 / size.height());
    let (dx, dy) = (
        (w as f32 - size.width() * k) / 2.0,
        (h as f32 - size.height() * k) / 2.0,
    );
    render_tree(
        &tree,
        w,
        h,
        tiny_skia::Transform::from_row(k, 0.0, 0.0, k, dx, dy),
    )
}

/// The document's canvas, width by height, in its own user units.
pub fn canvas_size(svg: &str) -> Result<(f32, f32), String> {
    let size = parse_tree(svg)?.size();
    Ok((size.width(), size.height()))
}

fn parse_tree(svg: &str) -> Result<usvg::Tree, String> {
    // No font database is loaded on purpose: the tracer never writes <text>, and a render
    // that silently substituted a font would be comparing against something the SVG does
    // not actually say.
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(svg, &opt).map_err(|e| format!("cannot parse the SVG: {e}"))?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return Err("the SVG declares an empty canvas".into());
    }
    Ok(tree)
}

fn render_tree(
    tree: &usvg::Tree,
    w: u32,
    h: u32,
    transform: tiny_skia::Transform,
) -> Result<Vec<u8>, String> {
    if w == 0 || h == 0 {
        return Err("cannot render to a zero-sized pixmap".into());
    }
    let mut pixmap =
        tiny_skia::Pixmap::new(w, h).ok_or_else(|| format!("{w}x{h} is too large to render"))?;
    resvg::render(tree, transform, &mut pixmap.as_mut());

    // tiny-skia stores premultiplied RGBA; the comparison wants straight.
    let mut out = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for px in pixmap.pixels() {
        let a = px.alpha();
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            out.extend_from_slice(&[un(px.red()), un(px.green()), un(px.blue()), a]);
        }
    }
    Ok(out)
}

/// The pixel-for-pixel comparison, kept whole.
///
/// The per-pixel differences are the expensive part and more than one panel reads them:
/// the report wants three summary statistics, the viewer wants the worst window, and the
/// honesty panel wants to know *where* the disagreement is and what the source looks like
/// there. So the map is computed once and handed around.
pub struct Analysis {
    /// dE00 per pixel, row-major, `width * height` long.
    pub deltas: Vec<f32>,
    /// The SVG rendered back to straight RGBA8, the same size as the source.
    pub rendered: Vec<u8>,
    /// Width of both, in pixels.
    pub width: u32,
    /// Height of both, in pixels.
    pub height: u32,
    /// Mean dE00 over the canvas.
    pub mean: f64,
    /// Median dE00.
    pub median: f64,
    /// 99th percentile dE00.
    pub worst: f64,
    /// The window the two disagree in most, if any is worth going to.
    pub corner: Option<WorstCorner>,
}

/// Compare the raster the tracer saw with a render of the SVG it wrote.
pub fn analyse(source: &inkvec_trace::Rgba, svg: &str) -> Result<Analysis, String> {
    let (w, h) = (source.width as u32, source.height as u32);
    let rendered = render(svg, w, h)?;

    let n = (w as usize) * (h as usize);
    if source.data.len() < n * 4 {
        return Err("the source raster is shorter than its own dimensions".into());
    }
    let deltas = pixel_deltas(&source.data, &rendered, w as usize, n);

    // The mean is summed in order, one pixel after another, so it is the same number
    // however the map above was split across threads.
    let ((mean, (median, worst)), corner) = rayon::join(
        || {
            let mean = deltas.iter().map(|d| *d as f64).sum::<f64>() / n as f64;
            (mean, median_and_p99(&deltas))
        },
        || worst_corner(&deltas, w, h),
    );

    Ok(Analysis {
        deltas,
        rendered,
        width: w,
        height: h,
        mean,
        median,
        worst,
        corner,
    })
}

/// dE00 for every pixel, row by row across the cores.
///
/// Each pixel's number is computed exactly as it would be alone: a row is one task, and
/// inside a row a pixel whose source and render are both bit-for-bit the pixel before it
/// takes that pixel's number rather than computing the same one again. Flat artwork is
/// mostly such runs, which is where most of the saving comes from. A noisy source (a
/// JPEG) breaks the runs, but its render still runs flat, so the render's own colour
/// conversion is carried along a run of the render alone. The answer is the same either
/// way.
fn pixel_deltas(source: &[f32], rendered: &[u8], w: usize, n: usize) -> Vec<f32> {
    use rayon::prelude::*;
    let mut deltas = vec![0.0f32; n];
    deltas
        .par_chunks_mut(w.max(1))
        .enumerate()
        .for_each(|(y, row)| {
            let mut last: Option<([u32; 4], [u8; 4], f32)> = None;
            let mut last_render: Option<([u8; 4], [f64; 3])> = None;
            for (x, delta) in row.iter_mut().enumerate() {
                let i = (y * w + x) * 4;
                let s = &source[i..i + 4];
                let r = [
                    rendered[i],
                    rendered[i + 1],
                    rendered[i + 2],
                    rendered[i + 3],
                ];
                let key = [
                    s[0].to_bits(),
                    s[1].to_bits(),
                    s[2].to_bits(),
                    s[3].to_bits(),
                ];
                if let Some((ks, kr, d)) = last {
                    if ks == key && kr == r {
                        *delta = d;
                        continue;
                    }
                }
                let lab_r = match last_render {
                    Some((kr, l)) if kr == r => l,
                    _ => {
                        let l = rendered_lab(r);
                        last_render = Some((r, l));
                        l
                    }
                };
                let d = ciede2000(source_lab(s), lab_r) as f32;
                *delta = d;
                last = Some((key, r, d));
            }
        });
    deltas
}

/// dE00 between one source pixel and one rendered pixel, both over the matte.
#[cfg(test)]
fn pixel_delta(s: &[f32], r: [u8; 4]) -> f32 {
    ciede2000(source_lab(s), rendered_lab(r)) as f32
}

/// A source pixel over the matte, in L*a*b*.
fn source_lab(s: &[f32]) -> [f64; 3] {
    lab(over_matte([s[0], s[1], s[2]], s[3]))
}

/// A rendered pixel over the matte, in L*a*b*.
fn rendered_lab(r: [u8; 4]) -> [f64; 3] {
    lab(over_matte(
        [
            r[0] as f32 / 255.0,
            r[1] as f32 / 255.0,
            r[2] as f32 / 255.0,
        ],
        r[3] as f32 / 255.0,
    ))
}

/// The median and the 99th percentile: the values a full sort would put at `n / 2` and
/// at `n * 0.99`.
///
/// Two selections instead of a sort. The 99th percentile goes first, which leaves every
/// smaller value in front of it, and the median is then selected inside that front part.
/// Same comparator, same indices, same two values.
fn median_and_p99(deltas: &[f32]) -> (f64, f64) {
    use std::cmp::Ordering;
    let n = deltas.len();
    let cmp = |a: &f32, b: &f32| a.partial_cmp(b).unwrap_or(Ordering::Equal);
    let mid = n / 2;
    let k99 = ((n as f64 * 0.99) as usize).min(n - 1);
    let mut v = deltas.to_vec();
    let worst = *v.select_nth_unstable_by(k99, cmp).1;
    let median = match mid.cmp(&k99) {
        Ordering::Less => *v[..k99].select_nth_unstable_by(mid, cmp).1,
        Ordering::Equal => worst,
        Ordering::Greater => *v[k99 + 1..].select_nth_unstable_by(mid - k99 - 1, cmp).1,
    };
    (median as f64, worst as f64)
}

/// The window, on a coarse grid, whose mean dE00 is highest.
///
/// This is what "find the worst corner" jumps to. A grid rather than a search: the point
/// is to land somewhere the disagreement is visible at 12x, not to find a global optimum.
fn worst_corner(deltas: &[f32], w: u32, h: u32) -> Option<WorstCorner> {
    let win = (w.min(h) / 12).clamp(8, 96);
    if w < win || h < win {
        return None;
    }
    use rayon::prelude::*;
    let step = (win / 2).max(1);
    let ys: Vec<u32> = (0..)
        .map(|i| i * step)
        .take_while(|y| y + win <= h)
        .collect();
    let xs: Vec<u32> = (0..)
        .map(|i| i * step)
        .take_while(|x| x + win <= w)
        .collect();
    // Every window's mean, each summed exactly as before, one row of windows per task...
    let means: Vec<Vec<f64>> = ys
        .par_iter()
        .map(|&y| {
            xs.iter()
                .map(|&x| {
                    let mut sum = 0.0f64;
                    for yy in y..y + win {
                        let row = (yy as usize) * (w as usize);
                        for xx in x..x + win {
                            sum += deltas[row + xx as usize] as f64;
                        }
                    }
                    sum / (win as f64 * win as f64)
                })
                .collect()
        })
        .collect();
    // ...and the choice made in scan order, so a tie goes to the same window it always did.
    let mut best: Option<WorstCorner> = None;
    for (&y, row) in ys.iter().zip(&means) {
        for (&x, &mean) in xs.iter().zip(row) {
            if best.is_none_or(|b| mean > b.de00) {
                best = Some(WorstCorner {
                    x: x + win / 2,
                    y: y + win / 2,
                    de00: mean,
                });
            }
        }
    }
    // A perfectly clean trace has no worst corner worth going to.
    best.filter(|b| b.de00 > 0.05)
}

/// Composite a straight RGBA colour onto the shared matte.
fn over_matte(rgb: [f32; 3], a: f32) -> [f32; 3] {
    let a = a.clamp(0.0, 1.0);
    [
        rgb[0] * a + MATTE[0] * (1.0 - a),
        rgb[1] * a + MATTE[1] * (1.0 - a),
        rgb[2] * a + MATTE[2] * (1.0 - a),
    ]
}

/// sRGB (0..1) to CIE L*a*b* under D65, the space dE00 is defined in.
pub fn lab(rgb: [f32; 3]) -> [f64; 3] {
    let lin = |c: f32| -> f64 {
        let c = c.clamp(0.0, 1.0) as f64;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    // sRGB -> XYZ (D65), then normalised by the D65 white point.
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = (0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 between two L*a*b* colours.
///
/// The standard formulation (Sharma, Wu & Dalal 2005), including the hue-rotation term
/// that the simplified versions drop. Around 1.0 is where a trained eye starts to see a
/// difference; a good trace lands near 0.1.
pub fn ciede2000(l1: [f64; 3], l2: [f64; 3]) -> f64 {
    const K_L: f64 = 1.0;
    const K_C: f64 = 1.0;
    const K_H: f64 = 1.0;

    let (l_1, a_1, b_1) = (l1[0], l1[1], l1[2]);
    let (l_2, a_2, b_2) = (l2[0], l2[1], l2[2]);

    let c1 = (a_1 * a_1 + b_1 * b_1).sqrt();
    let c2 = (a_2 * a_2 + b_2 * b_2).sqrt();
    let c_bar = (c1 + c2) / 2.0;

    let c_bar7 = c_bar.powi(7);
    let g = 0.5 * (1.0 - (c_bar7 / (c_bar7 + 25f64.powi(7))).sqrt());

    let a1p = (1.0 + g) * a_1;
    let a2p = (1.0 + g) * a_2;
    let c1p = (a1p * a1p + b_1 * b_1).sqrt();
    let c2p = (a2p * a2p + b_2 * b_2).sqrt();

    let hp = |ap: f64, b: f64| -> f64 {
        if ap == 0.0 && b == 0.0 {
            0.0
        } else {
            let h = b.atan2(ap).to_degrees();
            if h < 0.0 {
                h + 360.0
            } else {
                h
            }
        }
    };
    let h1p = hp(a1p, b_1);
    let h2p = hp(a2p, b_2);

    let dlp = l_2 - l_1;
    let dcp = c2p - c1p;

    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let dhp_big = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();

    let lp_bar = (l_1 + l_2) / 2.0;
    let cp_bar = (c1p + c2p) / 2.0;

    let hp_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else {
        let d = (h1p - h2p).abs();
        if d <= 180.0 {
            (h1p + h2p) / 2.0
        } else if h1p + h2p < 360.0 {
            (h1p + h2p + 360.0) / 2.0
        } else {
            (h1p + h2p - 360.0) / 2.0
        }
    };

    let t = 1.0 - 0.17 * (hp_bar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hp_bar).to_radians().cos()
        + 0.32 * (3.0 * hp_bar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hp_bar - 63.0).to_radians().cos();

    let d_theta = 30.0 * (-(((hp_bar - 275.0) / 25.0).powi(2))).exp();
    let cp_bar7 = cp_bar.powi(7);
    let r_c = 2.0 * (cp_bar7 / (cp_bar7 + 25f64.powi(7))).sqrt();
    let s_l = 1.0 + (0.015 * (lp_bar - 50.0).powi(2)) / (20.0 + (lp_bar - 50.0).powi(2)).sqrt();
    let s_c = 1.0 + 0.045 * cp_bar;
    let s_h = 1.0 + 0.015 * cp_bar * t;
    let r_t = -(2.0 * d_theta.to_radians()).sin() * r_c;

    let term_l = dlp / (K_L * s_l);
    let term_c = dcp / (K_C * s_c);
    let term_h = dhp_big / (K_H * s_h);

    (term_l * term_l + term_c * term_c + term_h * term_h + r_t * term_c * term_h).sqrt()
}

/// Count what the SVG holds: coordinates, segments, drawn elements, distinct inks.
pub fn count(svg: &str) -> (usize, usize, usize, usize) {
    let mut coordinates = 0usize;
    let mut segments = 0usize;
    let mut elements = 0usize;
    let mut inks: Vec<String> = Vec::new();

    for tag in ["<path", "<circle", "<ellipse", "<rect", "<polygon", "<line"] {
        elements += svg.matches(tag).count();
    }
    // <svg ...> itself is not a drawn element, and neither is a <rect> inside <defs>;
    // the first is worth subtracting because it is always there.
    #[allow(clippy::unnecessary_min_or_max)]
    {
        elements = elements.saturating_sub(svg.matches("<rect").count().min(0));
    }

    for d in attribute_values(svg, "d") {
        let (c, s) = count_path(d);
        coordinates += c;
        segments += s;
    }
    for key in ["fill", "stroke"] {
        for v in attribute_values(svg, key) {
            let v = v.trim();
            if v.starts_with('#') && !inks.iter().any(|i| i == v) {
                inks.push(v.to_ascii_lowercase());
            }
        }
    }
    (coordinates, segments, elements, inks.len())
}

/// Numbers and commands in one `d` attribute.
fn count_path(d: &str) -> (usize, usize) {
    let mut numbers = 0usize;
    let mut commands = 0usize;
    let mut in_number = false;
    for ch in d.chars() {
        if ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+' || ch == 'e' || ch == 'E' {
            if !in_number {
                in_number = true;
                numbers += 1;
            }
        } else {
            in_number = false;
            if ch.is_ascii_alphabetic() {
                commands += 1;
            }
        }
    }
    (numbers, commands)
}

/// Every value of `name="..."` in the document, in order.
///
/// A scan rather than a parse: this runs on the tracer's own output, which is generated,
/// flat and free of namespaces, and the numbers it feeds are counts for a readout.
fn attribute_values<'a>(svg: &'a str, name: &str) -> Vec<&'a str> {
    let needle = format!(" {name}=\"");
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find(&needle) {
        let after = &rest[i + needle.len()..];
        match after.find('"') {
            Some(j) => {
                out.push(&after[..j]);
                rest = &after[j + 1..];
            }
            None => break,
        }
    }
    out
}

/// The palette: every ink the SVG paints with, flat or gradient, and how much of the
/// canvas it covers.
///
/// The share is measured from a render rather than inferred from the geometry, because
/// what the user is being shown is the picture, and overlap and holes move the answer.
/// It is measured exactly rather than guessed from colours: the document is rendered once
/// with every ink's paint replaced by a colour that stands for that ink alone, without
/// antialiasing, so each pixel names the ink on top of it. Reading inks back from an
/// ordinary render by nearest colour could not do this for gradients, whose pixels run
/// through colours no flat ink has and were credited to whichever flat ink was nearest.
pub fn palette(svg: &str, w: u32, h: u32) -> Result<Vec<Ink>, String> {
    let mut inks = declared_inks(svg);
    if inks.is_empty() {
        return Ok(Vec::new());
    }

    // Render small: the share is a percentage, and a 256-px render settles it to well
    // inside the 1% the readout shows while costing nothing.
    let scale = 256.0 / w.max(h).max(1) as f64;
    let (rw, rh) = (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    );
    let code = IdColours::new(inks.len());
    let px = render(&id_document(svg, &inks, &code), rw, rh)?;

    let mut counts = vec![0usize; inks.len()];
    let mut covered = 0usize;
    for chunk in px.as_chunks::<4>().0 {
        if chunk[3] < 128 {
            continue;
        }
        covered += 1;
        if let Some(i) = code.decode([chunk[0], chunk[1], chunk[2]]) {
            counts[i] += 1;
        }
    }

    let total = covered.max(1) as f64;
    for (ink, n) in inks.iter_mut().zip(&counts) {
        ink.share = *n as f64 / total;
    }
    // Stable: inks of equal share keep the order the document declares them in.
    inks.sort_by(|a, b| {
        b.share
            .partial_cmp(&a.share)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(inks)
}

/// How close two gradients' stops must be, in dE00 stop for stop, to be one ink. The tracer
/// writes one gradient element per face, and faces cut from one ramp repeat its stops to
/// the last digit or within a rounding of it; 1.0 is the difference a trained eye starts
/// to see, so two gradients inside it are the same ramp to anyone looking.
const SAME_GRADIENT_DE00: f64 = 1.0;

/// Every ink the document paints with, in the order it first appears, with no shares yet.
fn declared_inks(svg: &str) -> Vec<Ink> {
    use std::collections::HashMap;
    let defs: HashMap<String, GradientDef> = gradient_defs(svg)
        .into_iter()
        .map(|d| (d.id.clone(), d))
        .collect();
    let mut inks: Vec<Ink> = Vec::new();
    // Paint values already placed, and inks by their exact stop list: a drawing repeats
    // both thousands of times, and only a gradient seen for the first time is compared
    // stop by stop against the gradients before it.
    let mut by_key: HashMap<&str, usize> = HashMap::new();
    let mut by_stops: HashMap<(bool, Vec<String>), usize> = HashMap::new();
    // The gradient inks' stops in L*a*b*, for that comparison.
    let mut gradient_labs: Vec<(usize, Vec<[f64; 3]>)> = Vec::new();
    for value in paint_values(svg) {
        let key = value.trim();
        if by_key.contains_key(key) {
            continue;
        }
        let (stops, kind) = if let Some(c) = parse_hex(key).filter(|_| key.starts_with('#')) {
            (vec![c], None)
        } else if let Some(def) = url_id(key).and_then(|id| defs.get(id)) {
            (def.stops.clone(), Some(def.kind))
        } else {
            continue;
        };
        let hexes: Vec<String> = stops.iter().map(|c| to_hex(*c)).collect();
        let exact = by_stops.get(&(kind.is_some(), hexes.clone())).copied();
        let labs: Vec<[f64; 3]> = stops.iter().map(|c| lab(*c)).collect();
        let near = || {
            gradient_labs
                .iter()
                .find(|(_, have)| {
                    have.len() == labs.len()
                        && have
                            .iter()
                            .zip(&labs)
                            .all(|(a, b)| ciede2000(*a, *b) <= SAME_GRADIENT_DE00)
                })
                .map(|(i, _)| *i)
        };
        let found = exact.or_else(|| if kind.is_some() { near() } else { None });
        let i = match found {
            Some(i) => {
                inks[i].keys.push(key.to_string());
                i
            }
            None => {
                let i = inks.len();
                if kind.is_some() {
                    gradient_labs.push((i, labs));
                }
                by_stops.insert((kind.is_some(), hexes.clone()), i);
                inks.push(Ink {
                    traced: hexes[0].clone(),
                    hex: hexes[0].clone(),
                    share: 0.0,
                    snapped_de00: None,
                    kind: if kind.is_some() {
                        InkKind::Gradient
                    } else {
                        InkKind::Flat
                    },
                    keys: vec![key.to_string()],
                    stops: hexes,
                    gradient: kind,
                });
                i
            }
        };
        by_key.insert(key, i);
    }
    inks
}

/// Every `fill` and `stroke` value, in document order.
fn paint_values(svg: &str) -> Vec<&str> {
    let mut found: Vec<(usize, &str)> = Vec::new();
    for name in ["fill", "stroke"] {
        let needle = format!(" {name}=\"");
        let mut from = 0;
        while let Some(i) = svg[from..].find(&needle) {
            let start = from + i + needle.len();
            let Some(len) = svg[start..].find('"') else {
                break;
            };
            found.push((start, &svg[start..start + len]));
            from = start + len + 1;
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, v)| v).collect()
}

/// `url(#id)` (quotes allowed) to `id`.
fn url_id(value: &str) -> Option<&str> {
    let inner = value.strip_prefix("url(")?.strip_suffix(')')?.trim();
    let inner = inner.trim_matches(|c| c == '\'' || c == '"');
    inner.strip_prefix('#').filter(|id| !id.is_empty())
}

/// One gradient element as the document declares it.
struct GradientDef {
    id: String,
    kind: GradientKind,
    stops: Vec<[f32; 3]>,
}

/// Every linear and radial gradient with an id and at least one readable stop.
///
/// A scan, like the rest of this module: the tracer writes each gradient as one element
/// with its stops inside, `stop-color` as a hex attribute. A gradient that borrows its
/// stops from another through `href` is given them; stops in any colour syntax other than
/// hex are skipped.
fn gradient_defs(svg: &str) -> Vec<GradientDef> {
    let mut defs: Vec<(GradientDef, Option<String>)> = Vec::new();
    for (tag, close, kind) in [
        ("<linearGradient", "</linearGradient>", GradientKind::Linear),
        ("<radialGradient", "</radialGradient>", GradientKind::Radial),
    ] {
        let mut from = 0;
        while let Some(i) = svg[from..].find(tag) {
            let at = from + i;
            let after = &svg[at + tag.len()..];
            from = at + tag.len();
            if !after.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
                continue;
            }
            let Some(end) = after.find('>') else {
                break;
            };
            let head = &after[..end];
            let body = if head.ends_with('/') {
                ""
            } else {
                let rest = &after[end + 1..];
                &rest[..rest.find(close).unwrap_or(rest.len())]
            };
            let Some(id) = tag_attr(head, "id") else {
                continue;
            };
            let href = tag_attr(head, "href")
                .or_else(|| tag_attr(head, "xlink:href"))
                .and_then(|h| h.strip_prefix('#'))
                .map(str::to_string);
            let mut stops = Vec::new();
            let mut rest = body;
            while let Some(j) = rest.find("<stop") {
                let s = &rest[j + 5..];
                let Some(e) = s.find('>') else {
                    break;
                };
                let stop = &s[..e];
                let colour = tag_attr(stop, "stop-color").or_else(|| {
                    tag_attr(stop, "style").and_then(|st| {
                        st.split(';')
                            .find_map(|d| d.trim().strip_prefix("stop-color:").map(str::trim))
                    })
                });
                if let Some(c) = colour.filter(|c| c.starts_with('#')).and_then(parse_hex) {
                    stops.push(c);
                }
                rest = &s[e..];
            }
            defs.push((
                GradientDef {
                    id: id.to_string(),
                    kind,
                    stops,
                },
                href,
            ));
        }
    }
    // Borrowed stops, one step deep, which is as deep as any writer goes.
    let borrowed: Vec<Option<Vec<[f32; 3]>>> = defs
        .iter()
        .map(|(d, href)| {
            let href = href.as_deref().filter(|_| d.stops.is_empty())?;
            defs.iter()
                .find(|(o, _)| o.id == href)
                .map(|(o, _)| o.stops.clone())
        })
        .collect();
    defs.into_iter()
        .zip(borrowed)
        .map(|((mut d, _), b)| {
            if let Some(stops) = b {
                d.stops = stops;
            }
            d
        })
        .filter(|d| !d.stops.is_empty())
        .collect()
}

/// The value of `name="..."` inside one tag's attribute text.
fn tag_attr<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let mut from = 0;
    while let Some(i) = head[from..].find(&needle) {
        let at = from + i;
        let start = at + needle.len();
        // Whole attribute names only: `id` must not match inside `gradient-id`.
        if at == 0 || head[..at].ends_with(char::is_whitespace) {
            let len = head[start..].find('"')?;
            return Some(&head[start..start + len]);
        }
        from = start;
    }
    None
}

/// A distinct colour for each ink, on a grid far enough apart that a pixel's colour names
/// its ink without doubt. Index 0 (black) is kept for paint that is no ink of the palette.
struct IdColours {
    /// Levels per channel.
    levels: u32,
}

impl IdColours {
    fn new(inks: usize) -> Self {
        let mut levels = 2u32;
        while (levels.pow(3) as usize) <= inks {
            levels += 1;
        }
        Self { levels }
    }

    fn step(&self) -> f32 {
        255.0 / (self.levels - 1) as f32
    }

    /// The colour standing for ink `i`.
    fn encode(&self, i: usize) -> String {
        let n = i as u32 + 1;
        let k = self.levels;
        let c = |d: u32| (d as f32 * self.step()).round() as u8;
        format!(
            "#{:02x}{:02x}{:02x}",
            c(n % k),
            c(n / k % k),
            c(n / (k * k) % k)
        )
    }

    /// The ink a rendered colour stands for, if it is one of the grid's.
    fn decode(&self, rgb: [u8; 3]) -> Option<usize> {
        let k = self.levels;
        let mut n = 0u32;
        for (place, v) in [(1, rgb[0]), (k, rgb[1]), (k * k, rgb[2])] {
            let d = (v as f32 / self.step()).round();
            if (d * self.step() - v as f32).abs() > 2.0 {
                return None;
            }
            n += d as u32 * place;
        }
        (n as usize).checked_sub(1)
    }
}

/// The document with every ink's paint replaced by its identifying colour, drawn without
/// antialiasing, so each pixel of a render is exactly one ink.
///
/// Paint that is no ink of the palette (a pattern, a named colour) becomes black, which
/// stands for no ink, rather than disappearing: it still covers what is under it. Opacity
/// is rounded to all or nothing, so a translucent ink owns the pixels it is at least half
/// of and leaves the rest to what shows through, as the eye would split them.
fn id_document(svg: &str, inks: &[Ink], code: &IdColours) -> String {
    let owners: std::collections::HashMap<&str, usize> = inks
        .iter()
        .enumerate()
        .flat_map(|(i, ink)| ink.keys.iter().map(move |k| (k.as_str(), i)))
        .collect();
    let paint = |value: &str| -> Option<String> {
        let v = value.trim();
        if v == "none" || v == "transparent" {
            return None;
        }
        Some(match owners.get(v).copied() {
            Some(i) => code.encode(i),
            None => "#000000".to_string(),
        })
    };
    let out = rewrite_attr(svg, "fill", paint);
    let out = rewrite_attr(&out, "stroke", paint);
    let solid = |v: &str| {
        let a = v.trim().trim_end_matches('%');
        let f = a.parse::<f32>().ok().map(|f| {
            if v.trim().ends_with('%') {
                f / 100.0
            } else {
                f
            }
        });
        Some(if f.unwrap_or(1.0) >= 0.5 { "1" } else { "0" }.to_string())
    };
    let mut out = out;
    for name in ["opacity", "fill-opacity", "stroke-opacity"] {
        out = rewrite_attr(&out, name, solid);
    }
    let crisp = |_: &str| Some("crispEdges".to_string());
    let mut out = rewrite_attr(&out, "shape-rendering", crisp);
    // On the root too, where it is inherited by everything that did not name its own.
    if let Some(i) = out.find("<svg") {
        let head_end = out[i..].find('>').map_or(out.len(), |e| i + e);
        if !out[i..head_end].contains("shape-rendering=") {
            out.insert_str(i + 4, " shape-rendering=\"crispEdges\"");
        }
    }
    out
}

/// The document with every ` name="value"` attribute's value passed through `f`; `None`
/// keeps it as it was.
pub(crate) fn rewrite_attr(svg: &str, name: &str, f: impl Fn(&str) -> Option<String>) -> String {
    let needle = format!(" {name}=\"");
    let mut out = String::with_capacity(svg.len());
    let mut from = 0;
    while let Some(i) = svg[from..].find(&needle) {
        let start = from + i + needle.len();
        let Some(len) = svg[start..].find('"') else {
            break;
        };
        out.push_str(&svg[from..start]);
        let value = &svg[start..start + len];
        match f(value) {
            Some(v) => out.push_str(&v),
            None => out.push_str(value),
        }
        from = start + len;
    }
    out.push_str(&svg[from..]);
    out
}

/// `#rgb` or `#rrggbb` to linear 0..1 sRGB components.
pub fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let s = s.trim().trim_start_matches('#');
    let v = |a: &str| u8::from_str_radix(a, 16).ok().map(|n| n as f32 / 255.0);
    match s.len() {
        3 => {
            let d: Vec<char> = s.chars().collect();
            Some([
                v(&format!("{}{}", d[0], d[0]))?,
                v(&format!("{}{}", d[1], d[1]))?,
                v(&format!("{}{}", d[2], d[2]))?,
            ])
        }
        6 => Some([v(&s[0..2])?, v(&s[2..4])?, v(&s[4..6])?]),
        _ => None,
    }
}

/// 0..1 sRGB components back to `#rrggbb`.
pub fn to_hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// The colour difference between two hex strings, for the "you are overriding a
/// measurement" line beside a snapped swatch.
pub fn hex_distance(a: &str, b: &str) -> Option<f64> {
    Some(ciede2000(lab(parse_hex(a)?), lab(parse_hex(b)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><rect width="64" height="64" fill="#ffffff"/><path d="M8 8L56 8L56 56L8 56Z" fill="#14453f"/></svg>"##;

    #[test]
    fn identical_colours_are_zero_apart() {
        assert!(ciede2000(lab([0.2, 0.4, 0.6]), lab([0.2, 0.4, 0.6])).abs() < 1e-9);
    }

    #[test]
    fn ciede2000_matches_the_sharma_reference_pairs() {
        // Rows from the Sharma, Wu & Dalal (2005) test data, Lab in, dE00 out.
        let cases = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 3.1571, -77.2803], [50.0, 0.0, -82.7485], 2.8615),
            ([50.0, 2.8361, -74.0200], [50.0, 0.0, -82.7485], 3.4412),
            ([50.0, -1.3802, -84.2814], [50.0, 0.0, -82.7485], 1.0000),
            ([50.0, 2.5, 0.0], [50.0, 0.0, -2.5], 4.3065),
            (
                [60.2574, -34.0099, 36.2677],
                [60.4626, -34.1751, 39.4387],
                1.2644,
            ),
            (
                [22.7233, 20.0904, -46.6940],
                [23.0331, 14.9730, -42.5619],
                2.0373,
            ),
            (
                [2.0776, 0.0795, -1.1350],
                [0.9033, -0.0636, -0.5514],
                0.9082,
            ),
        ];
        for (a, b, want) in cases {
            let got = ciede2000(a, b);
            assert!(
                (got - want).abs() < 1e-3,
                "dE00({a:?}, {b:?}) = {got}, want {want}"
            );
        }
    }

    #[test]
    fn white_and_black_are_a_hundred_apart() {
        let d = ciede2000(lab([1.0, 1.0, 1.0]), lab([0.0, 0.0, 0.0]));
        assert!((d - 100.0).abs() < 0.5, "{d}");
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(to_hex(parse_hex("#14453F").unwrap()), "#14453f");
        assert_eq!(to_hex(parse_hex("#abc").unwrap()), "#aabbcc");
        assert!(parse_hex("#12345").is_none());
    }

    #[test]
    fn counting_reads_the_path_data() {
        let (coords, segments, elements, inks) = count(SVG);
        assert_eq!(coords, 8, "four points, two numbers each");
        assert_eq!(segments, 5, "M L L L Z");
        assert_eq!(elements, 2);
        assert_eq!(inks, 2);
    }

    #[test]
    fn a_render_of_the_svg_has_the_colours_the_svg_declares() {
        let px = render(SVG, 64, 64).unwrap();
        // Middle pixel is inside the dark square.
        let i = ((32 * 64) + 32) * 4;
        assert_eq!(&px[i..i + 3], &[0x14, 0x45, 0x3f]);
        // A corner is the white ground.
        assert_eq!(&px[0..3], &[0xff, 0xff, 0xff]);
    }

    #[test]
    fn the_palette_sums_to_the_whole_canvas() {
        let inks = palette(SVG, 64, 64).unwrap();
        assert_eq!(inks.len(), 2);
        let total: f64 = inks.iter().map(|i| i.share).sum();
        assert!((total - 1.0).abs() < 1e-6, "{total}");
        // Sorted largest first: the inner square is 48/64 squared, which is more than
        // half the canvas, so it leads and the white ground follows.
        assert!(inks[0].share > inks[1].share);
        assert_eq!(inks[0].traced, "#14453f");
        assert!(
            (inks[0].share - (48.0 * 48.0) / (64.0 * 64.0)).abs() < 0.02,
            "{:?}",
            inks[0]
        );
    }

    /// A flat ground, a linear gradient over the left half and, in the right half, two
    /// gradient elements with the same stops (one a rounding apart) and a radial one.
    const GRADIENTS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><defs><linearGradient id="g1" x1="0" y1="0" x2="32" y2="0" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient><linearGradient id="g2" x1="0" y1="0" x2="0" y2="64" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#fe0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient><radialGradient id="g3" cx="48" cy="48" r="16" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#ffffff"/><stop offset="0.5" stop-color="#808080"/><stop offset="1" stop-color="#000000"/></radialGradient></defs><rect width="64" height="64" fill="#14453F"/><path d="M0 0H32V64H0Z" fill="url(#g1)"/><path d="M32 0H64V16H32Z" fill="url(#g2)"/><path d="M32 32H64V64H32Z" fill="url(#g3)"/></svg>"##;

    #[test]
    fn gradients_are_inks_with_their_stops() {
        let inks = palette(GRADIENTS, 64, 64).unwrap();
        let ramp = inks
            .iter()
            .find(|i| i.keys.iter().any(|k| k == "url(#g1)"))
            .expect("the linear gradient is an ink");
        assert_eq!(ramp.kind, InkKind::Gradient);
        assert_eq!(ramp.gradient, Some(GradientKind::Linear));
        assert_eq!(ramp.stops, ["#ff0000", "#0000ff"]);
        assert_eq!(ramp.traced, "#ff0000", "a gradient's hex is its first stop");
        let radial = inks
            .iter()
            .find(|i| i.keys == ["url(#g3)"])
            .expect("the radial gradient is an ink of its own");
        assert_eq!(radial.gradient, Some(GradientKind::Radial));
        assert_eq!(radial.stops.len(), 3);
        let ground = inks.iter().find(|i| i.kind == InkKind::Flat).unwrap();
        assert_eq!(
            ground.keys,
            ["#14453F"],
            "keys are the attribute values as written"
        );
        assert_eq!(ground.stops, ["#14453f"]);
        let json = serde_json::to_value(ramp).unwrap();
        assert_eq!(json["kind"], "gradient");
        assert_eq!(json["gradient"], "linear");
        assert!(serde_json::to_value(ground)
            .unwrap()
            .get("gradient")
            .is_none());
    }

    #[test]
    fn gradients_with_the_same_stops_are_one_ink() {
        let inks = palette(GRADIENTS, 64, 64).unwrap();
        assert_eq!(inks.len(), 3, "{inks:?}");
        let ramp = inks
            .iter()
            .find(|i| i.keys.iter().any(|k| k == "url(#g1)"))
            .unwrap();
        assert_eq!(ramp.keys, ["url(#g1)", "url(#g2)"]);
    }

    #[test]
    fn gradient_shares_are_measured_and_sum_to_the_coverage() {
        let inks = palette(GRADIENTS, 64, 64).unwrap();
        let total: f64 = inks.iter().map(|i| i.share).sum();
        assert!((total - 1.0).abs() < 1e-6, "{total}");
        let share = |key: &str| {
            inks.iter()
                .find(|i| i.keys.iter().any(|k| k == key))
                .unwrap()
                .share
        };
        // Left half plus the top quarter of the right half.
        assert!((share("url(#g1)") - (0.5 + 0.125)).abs() < 0.01, "{inks:?}");
        assert!((share("url(#g3)") - 0.25).abs() < 0.01, "{inks:?}");
        assert!((share("#14453F") - 0.125).abs() < 0.01, "{inks:?}");
        // Sorted largest first.
        assert!(inks.windows(2).all(|w| w[0].share >= w[1].share));
    }

    #[test]
    fn a_transparent_ground_is_not_part_of_any_share() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><path d="M0 0H32V32H0Z" fill="#ff0000"/><path d="M32 32H64V64H32Z" fill="#0000ff" fill-opacity="0.8"/><path d="M0 32H32V64H0Z" fill="none" stroke="#00ff00" stroke-width="0"/></svg>"##;
        let inks = palette(svg, 64, 64).unwrap();
        assert_eq!(inks.len(), 3);
        assert!(
            (inks[0].share - 0.5).abs() < 0.01 && (inks[1].share - 0.5).abs() < 0.01,
            "{inks:?}"
        );
        assert_eq!(inks[2].share, 0.0);
    }

    #[test]
    fn the_id_colours_round_trip() {
        for n in [1usize, 7, 8, 26, 27, 300, 5000] {
            let code = IdColours::new(n);
            for i in 0..n {
                let c = parse_hex(&code.encode(i)).unwrap();
                let rgb = c.map(|v| (v * 255.0).round() as u8);
                assert_eq!(code.decode(rgb), Some(i), "{n} inks, ink {i}");
            }
            assert_eq!(code.decode([0, 0, 0]), None, "black is no ink");
        }
    }

    #[test]
    fn comparing_an_svg_with_itself_is_zero() {
        let rendered = render(SVG, 64, 64).unwrap();
        let source = inkvec_trace::rgba8_capped(&rendered, 64, 64, 0);
        let a = analyse(&source, SVG).unwrap();
        assert!(a.mean < 0.01, "{}", a.mean);
        assert!(a.median < 0.01, "{}", a.median);
        assert!(a.corner.is_none(), "a perfect trace has no worst corner");
    }

    #[test]
    fn comparing_against_a_different_drawing_is_not_zero() {
        let other = SVG.replace("#14453f", "#8a2f2f");
        let rendered = render(SVG, 64, 64).unwrap();
        let source = inkvec_trace::rgba8_capped(&rendered, 64, 64, 0);
        let a = analyse(&source, &other).unwrap();
        assert!(a.mean > 1.0, "{}", a.mean);
        assert!(a.corner.is_some());
    }

    /// The two selections must land on exactly the values a full sort puts at the two
    /// indices, for every length, including the short ones where the indices meet.
    #[test]
    fn the_percentiles_are_the_ones_a_sort_would_give() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for n in (1..70).chain([199, 200, 201, 1000, 4096, 65_537]) {
            // Plenty of ties, as in a real delta map: most pixels are exactly zero.
            let deltas: Vec<f32> = (0..n)
                .map(|_| match next() % 4 {
                    0 | 1 => 0.0,
                    2 => (next() % 16) as f32 * 0.125,
                    _ => (next() % 100_000) as f32 / 997.0,
                })
                .collect();
            let mut sorted = deltas.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let want = (
                sorted[n / 2] as f64,
                sorted[((n as f64 * 0.99) as usize).min(n - 1)] as f64,
            );
            let got = median_and_p99(&deltas);
            assert_eq!(
                (got.0.to_bits(), got.1.to_bits()),
                (want.0.to_bits(), want.1.to_bits()),
                "n = {n}"
            );
        }
    }

    /// The parallel map is the sequential one, bit for bit, runs and all.
    #[test]
    fn the_delta_map_is_the_pixel_by_pixel_one() {
        let (w, h) = (37usize, 23usize);
        let mut source = Vec::with_capacity(w * h * 4);
        let mut rendered = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                // Runs of identical pixels broken by a few that differ in one channel only.
                let v = if x % 7 == 3 {
                    0.25
                } else {
                    (y % 3) as f32 * 0.4
                };
                source.extend_from_slice(&[v, 0.5, 1.0 - v, if x == 5 { 0.5 } else { 1.0 }]);
                let r = if x % 5 == 1 { 200 } else { 30 + y as u8 };
                rendered.extend_from_slice(&[r, 128, 64, if x == 9 { 90 } else { 255 }]);
            }
        }
        let got = pixel_deltas(&source, &rendered, w, w * h);
        for i in 0..w * h {
            let r = [
                rendered[i * 4],
                rendered[i * 4 + 1],
                rendered[i * 4 + 2],
                rendered[i * 4 + 3],
            ];
            let want = pixel_delta(&source[i * 4..i * 4 + 4], r);
            assert_eq!(got[i].to_bits(), want.to_bits(), "pixel {i}");
        }
    }

    #[test]
    fn an_unparseable_document_is_an_error_not_a_number() {
        assert!(render("not an svg at all", 8, 8).is_err());
    }
}
