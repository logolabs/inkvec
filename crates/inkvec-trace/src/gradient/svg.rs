//! SVG representation of fill models (linear and radial gradients, flat colours).

use super::{FillModel, Interp};
use crate::color::to_hex;

fn interp_attr(interp: Interp) -> &'static str {
    match interp {
        Interp::Srgb => "",
        Interp::LinearRgb => " color-interpolation=\"linearRGB\"",
    }
}

/// Render a fade -- the opacity profile `alpha`, whose stops are greys equal to the
/// opacity, and the colour profile `color` on the same geometry and stop offsets -- as one
/// gradient carrying `stop-color` and `stop-opacity` at each stop: the geometry
/// [`fill_to_svg`] writes. Returns the `<defs>` fragment and the `fill` value; a flat
/// opacity profile is a plain colour, and the caller writes its `fill-opacity`.
pub fn fade_to_svg(alpha: &FillModel, color: &FillModel, id: &str) -> (String, String) {
    let stops = |m: &FillModel| -> Vec<[f32; 3]> {
        match m {
            FillModel::Flat(c) => vec![*c],
            FillModel::Linear { c0, c1, mids, .. } | FillModel::Radial { c0, c1, mids, .. } => {
                let mut v = vec![*c0];
                v.extend(mids.iter().map(|m| m.1));
                v.push(*c1);
                v
            }
        }
    };
    let (alphas, colours) = (stops(alpha), stops(color));
    if !alpha.is_gradient() {
        return (String::new(), to_hex(colours[0]));
    }
    let (defs, attr) = fill_to_svg(alpha, id);
    // `fill_to_svg` wrote one `stop-color="#rrggbb"` per stop, in stop order: the grey that
    // stood for the opacity. Replace each with that stop's colour and opacity.
    let mut out = String::with_capacity(defs.len() + 32 * alphas.len());
    let mut rest = defs.as_str();
    let mut k = 0usize;
    while let Some(i) = rest.find("stop-color=\"#") {
        out.push_str(&rest[..i]);
        let a = alphas.get(k).map_or(1.0, |g| g[0]).clamp(0.0, 1.0);
        let c = colours
            .get(k)
            .or(colours.last())
            .copied()
            .unwrap_or([1.0; 3]);
        out.push_str(&format!(
            "stop-color=\"{}\" stop-opacity=\"{a:.3}\"",
            to_hex(c)
        ));
        rest = &rest[i + "stop-color=\"#rrggbb\"".len()..];
        k += 1;
    }
    out.push_str(rest);
    (out, attr)
}

/// Render a fill as SVG: the `<defs>` fragment it needs (empty for a flat colour) and the
/// value of the `fill` attribute that references it.
///
/// Gradient coordinates are emitted in user space (pixel-centre units), so the fragment
/// belongs in a document whose user space is the tracer's: `viewBox="-0.5 -0.5 w h"`.
/// A linear-light gradient carries `color-interpolation="linearRGB"`; note that several
/// renderers ignore that property on gradients and interpolate in sRGB regardless.
pub fn fill_to_svg(fit: &FillModel, id: &str) -> (String, String) {
    fn stops_svg(c0: [f32; 3], mids: &[(f64, [f32; 3])], c1: [f32; 3]) -> String {
        let mut s = format!("<stop offset=\"0\" stop-color=\"{}\"/>", to_hex(c0));
        for &(off, col) in mids {
            s.push_str(&format!(
                "<stop offset=\"{off:.3}\" stop-color=\"{}\"/>",
                to_hex(col)
            ));
        }
        s.push_str(&format!(
            "<stop offset=\"1\" stop-color=\"{}\"/>",
            to_hex(c1)
        ));
        s
    }
    match *fit {
        FillModel::Flat(c) => (String::new(), to_hex(c)),
        FillModel::Linear {
            p0,
            p1,
            c0,
            c1,
            interp,
            ref mids,
        } => (
            format!(
                "<linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\"{} x1=\"{:.3}\" y1=\"{:.3}\" x2=\"{:.3}\" y2=\"{:.3}\">{}</linearGradient>",
                interp_attr(interp),
                p0.0,
                p0.1,
                p1.0,
                p1.1,
                stops_svg(c0, mids, c1)
            ),
            format!("url(#{id})"),
        ),
        FillModel::Radial {
            c,
            r,
            c0,
            c1,
            interp,
            aspect,
            angle,
            ref mids,
        } => {
            // An ellipse is a circle in a frame that is rotated by `angle` and squashed
            // across the major axis by `1/aspect`. The transform maps the gradient's
            // own (circular) frame to user space, so it reads right to left: move the
            // centre to the origin, squash, rotate, move back.
            let transform = if aspect == 1.0 {
                String::new()
            } else {
                format!(
                    " gradientTransform=\"translate({:.3} {:.3}) rotate({:.2}) scale(1 {:.4}) translate({:.3} {:.3})\"",
                    c.0,
                    c.1,
                    angle.to_degrees(),
                    1.0 / aspect,
                    -c.0,
                    -c.1
                )
            };
            (
                format!(
                    "<radialGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\"{}{transform} cx=\"{:.3}\" cy=\"{:.3}\" r=\"{:.3}\">{}</radialGradient>",
                    interp_attr(interp),
                    c.0,
                    c.1,
                    r,
                    stops_svg(c0, mids, c1)
                ),
                format!("url(#{id})"),
            )
        }
    }
}
