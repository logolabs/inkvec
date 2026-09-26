//! Hoisted evaluation of gradient fill models over many pixel coordinates.

use super::{eval_stops, linear_t, to_lin, to_srgb, FillModel, Interp};

/// [`radial_t`] with the rotation's `angle.sin_cos()` given (unused when `aspect` is 1).
#[inline]
pub(super) fn radial_t_rot(
    x: f64,
    y: f64,
    c: (f64, f64),
    r: f64,
    aspect: f64,
    sin_cos: (f64, f64),
) -> f64 {
    if r <= 0.0 {
        return 0.0;
    }
    let (dx, dy) = (x - c.0, y - c.1);
    let rho = if aspect == 1.0 {
        (dx * dx + dy * dy).sqrt()
    } else {
        let (sn, cs) = sin_cos;
        let u = dx * cs + dy * sn;
        let v = (-dx * sn + dy * cs) * aspect;
        (u * u + v * v).sqrt()
    };
    (rho / r).clamp(0.0, 1.0)
}

/// Interpolate two stops already in linear light, returning sRGB.
#[inline]
pub(super) fn lerp_lin(a: [f64; 3], b: [f64; 3], t: f64) -> [f32; 3] {
    to_srgb([
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ])
}

/// Which piece of a multi-stop profile `t` falls in, as the index of its first stop
/// (`c0` is 0, `mids[i]` is `i + 1`), and where along that piece, 0 to 1.
#[inline]
pub(super) fn segment(mids: &[(f64, [f32; 3])], t: f64) -> (usize, f64) {
    if mids.is_empty() {
        return (0, t);
    }
    let mut lo_t = 0.0;
    for (i, &(off, _)) in mids.iter().enumerate() {
        if t <= off {
            let u = if off > lo_t {
                (t - lo_t) / (off - lo_t)
            } else {
                0.0
            };
            return (i, u);
        }
        lo_t = off;
    }
    let u = if lo_t < 1.0 {
        (t - lo_t) / (1.0 - lo_t)
    } else {
        1.0
    };
    (mids.len(), u)
}

/// [`FillModel::color_at`] for many positions of one model.
///
/// What does not depend on the position is worked out once here: the stops' conversion
/// to linear light and an ellipse's rotation. Those were 6 of the 9 `powf` of every
/// evaluated pixel, and the gradient fits evaluate millions of pixels per image. The
/// result is `color_at`'s bit for bit: the same expressions on the same values, only
/// hoisted out of the pixel loop.
pub(crate) struct FillEval<'a> {
    model: &'a FillModel,
    /// Every stop, first to last, in linear light; empty unless the model interpolates
    /// in linear RGB.
    lin: Vec<[f64; 3]>,
    /// `angle.sin_cos()` of an elliptical radial model.
    sin_cos: (f64, f64),
}

impl FillModel {
    /// A [`FillEval`] of this model, for evaluating it at many positions.
    pub(crate) fn eval(&self) -> FillEval<'_> {
        let (mut lin, mut sin_cos) = (Vec::new(), (0.0, 1.0));
        match self {
            FillModel::Flat(_) => {}
            FillModel::Linear {
                c0,
                c1,
                interp,
                mids,
                ..
            }
            | FillModel::Radial {
                c0,
                c1,
                interp,
                mids,
                ..
            } => {
                if *interp == Interp::LinearRgb {
                    lin.push(to_lin(*c0));
                    lin.extend(mids.iter().map(|&(_, c)| to_lin(c)));
                    lin.push(to_lin(*c1));
                }
            }
        }
        if let FillModel::Radial { aspect, angle, .. } = *self {
            if aspect != 1.0 {
                sin_cos = angle.sin_cos();
            }
        }
        FillEval {
            model: self,
            lin,
            sin_cos,
        }
    }
}

impl FillEval<'_> {
    /// The fill colour (sRGB) at a pixel-centre position; see [`FillModel::color_at`].
    #[inline]
    pub(crate) fn color_at(&self, x: f64, y: f64) -> [f32; 3] {
        let (t, c0, mids, c1, interp) = match *self.model {
            FillModel::Flat(c) => return c,
            FillModel::Linear {
                p0,
                p1,
                c0,
                c1,
                interp,
                ref mids,
            } => (linear_t(x, y, p0, p1), c0, mids, c1, interp),
            FillModel::Radial {
                c,
                r,
                c0,
                c1,
                interp,
                aspect,
                ref mids,
                ..
            } => (
                radial_t_rot(x, y, c, r, aspect, self.sin_cos),
                c0,
                mids,
                c1,
                interp,
            ),
        };
        match interp {
            Interp::LinearRgb => {
                let (k, u) = segment(mids, t);
                lerp_lin(self.lin[k], self.lin[k + 1], u)
            }
            Interp::Srgb => eval_stops(c0, mids, c1, t, interp),
        }
    }
}
