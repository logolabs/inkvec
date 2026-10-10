//! What an intake's noise is, measured once per image by the boundary chain and read by
//! everything that weighs evidence against it (`docs/theory/chain-boundary.md`, "Noise").
//!
//! A clean render is exact to 8-bit rounding, and the window sums of
//! [`crate::likelihood`] are calibrated on it with nothing measured. A resampled or lossy
//! intake (a logo found on the web: resized, saved as JPEG) is not. Its error sits within a
//! few pixels of every edge, where flat-region estimates cannot see it, and it has heavy
//! tails. The boundary chain therefore estimates the noise *from the edges*: from the fourth
//! differences of consecutive window sums, which a smooth boundary does not move. It
//! summarises that estimate here so that the representation chain weighs its fits the same
//! way. [`NoiseModel::clean`] is today's model, and a clean intake gets exactly it.

/// The noise of one intake, as the boundary chain measured it.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseModel {
    /// Per-channel noise of interior pixels, sRGB units (`coverage::estimate_noise`).
    pub sigma_flat: f64,
    /// Per-channel noise by distance from the nearest edge, sRGB units: bands `[0, 1)`,
    /// `[1, 2)`, `[2, 3)` and `≥ 3` px.
    pub sigma_edge: Vec<f64>,
    /// How far beyond a native edge's the intake's edge ramp reaches on each side, px: half of
    /// `width − 1`, `width` the box-equivalent edge width of `softness::ramp_evidence`. 0 on a
    /// native render.
    pub psf_radius: f64,
    /// The blur's second moment along a window's strip, px²: `(width² − 1)/4`. A window's
    /// sum on a blurred intake is the face's area profile convolved with the blur, which on a
    /// curve moves it by `½ μ₂ A''` (the forward model, not noise). 0 on a native render.
    pub psf_mu2: f64,
    /// Tail of the standardised window residuals, as Student-t degrees of freedom;
    /// `f64::INFINITY` is Gaussian.
    pub nu: f64,
    /// The intake was lossy (a JPEG's blocks and subsampled chroma).
    pub lossy: bool,
    /// Measured window-sum standard deviation over the 8-bit rounding model's; 1 on a clean
    /// intake.
    pub window_scale: f64,
}

/// Distance bands of [`NoiseModel::sigma_edge`], px: the upper end of each but the last.
pub const EDGE_BANDS: [f64; 3] = [1.0, 2.0, 3.0];

/// One 8-bit level.
const Q: f64 = 1.0 / 255.0;

impl NoiseModel {
    /// A clean render: 8-bit rounding and nothing else, exactly today's model.
    pub fn clean() -> Self {
        let s = 0.5 * Q;
        NoiseModel {
            sigma_flat: s,
            sigma_edge: vec![s; EDGE_BANDS.len() + 1],
            psf_radius: 0.0,
            psf_mu2: 0.0,
            nu: f64::INFINITY,
            lossy: false,
            window_scale: 1.0,
        }
    }

    /// Whether this is [`NoiseModel::clean`]'s model: nothing was measured beyond rounding.
    pub fn is_clean(&self) -> bool {
        *self == NoiseModel::clean()
    }

    /// Per-channel noise at distance `d_px` from the nearest edge, sRGB units.
    pub fn sigma_edge_at(&self, d_px: f64) -> f64 {
        let k = EDGE_BANDS.iter().take_while(|&&b| d_px >= b).count();
        self.sigma_edge
            .get(k)
            .or(self.sigma_edge.last())
            .copied()
            .unwrap_or(self.sigma_flat)
            .max(self.sigma_flat)
    }

    /// Whether a band `band_width` px wide along an edge between inks `ink_a` and `ink_b`,
    /// coloured `colour`, is what the intake's blur makes of that edge rather than a face of
    /// its own: narrower than the blur's reach on both sides (`2·psf_radius + 1`), and its
    /// colour on the line through the two inks, between them or overshooting one of them
    /// (ringing), to within three times the noise within a pixel of an edge.
    pub fn explained_by_psf(
        &self,
        band_width: f64,
        colour: [f32; 3],
        ink_a: [f32; 3],
        ink_b: [f32; 3],
    ) -> bool {
        if self.psf_radius <= 0.0 || band_width > 2.0 * self.psf_radius + 1.0 {
            return false;
        }
        let d: [f64; 3] = std::array::from_fn(|c| ink_b[c] as f64 - ink_a[c] as f64);
        let q: [f64; 3] = std::array::from_fn(|c| colour[c] as f64 - ink_a[c] as f64);
        let dd: f64 = d.iter().map(|v| v * v).sum();
        if dd < 1e-12 {
            return false;
        }
        let t = (q[0] * d[0] + q[1] * d[1] + q[2] * d[2]) / dd;
        let off: f64 = (0..3)
            .map(|c| (q[c] - t * d[c]).powi(2))
            .sum::<f64>()
            .sqrt();
        // Ringing overshoots by a fraction of the step, not by more than the step itself.
        (-0.5..=1.5).contains(&t) && off <= 3.0 * self.sigma_edge_at(0.0) * 3f64.sqrt()
    }
}

/// Huber's threshold for residuals whose tail is Student-t with `nu` degrees of freedom:
/// `min(3, √ν)`, where the t's influence function `(ν + 1) u/(ν + u²)` peaks and begins to
/// redescend, so the two losses agree on which residuals are outliers. 3 for a Gaussian
/// (`nu = ∞`), so that clean residuals stay in the quadratic part.
pub fn huber_kappa(nu: f64) -> f64 {
    if nu.is_finite() {
        nu.max(0.0).sqrt().min(3.0)
    } else {
        3.0
    }
}

/// Huber's cost of a standardised residual `z` at threshold `kappa`: `z²` within it, then
/// growing linearly with the same slope (`2κ|z| − κ²`). Equal to `χ²` for small residuals.
pub fn huber(z: f64, kappa: f64) -> f64 {
    let a = z.abs();
    if a <= kappa {
        z * z
    } else {
        2.0 * kappa * a - kappa * kappa
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_is_todays_model() {
        let n = NoiseModel::clean();
        assert!(n.is_clean());
        assert_eq!(n.window_scale, 1.0);
        assert_eq!(n.nu, f64::INFINITY);
        assert_eq!(n.sigma_edge_at(0.3), 0.5 / 255.0);
        assert_eq!(n.sigma_edge_at(9.0), 0.5 / 255.0);
        assert!(!n.explained_by_psf(0.5, [0.5; 3], [0.0; 3], [1.0; 3]));
    }

    #[test]
    fn bands_and_ringing() {
        let n = NoiseModel {
            sigma_flat: 0.001,
            sigma_edge: vec![0.04, 0.012, 0.009, 0.006],
            psf_radius: 1.0,
            psf_mu2: 0.2,
            nu: 4.0,
            lossy: true,
            window_scale: 13.0,
        };
        assert!(!n.is_clean());
        assert_eq!(n.sigma_edge_at(0.5), 0.04);
        assert_eq!(n.sigma_edge_at(1.0), 0.012);
        assert_eq!(n.sigma_edge_at(2.5), 0.009);
        assert_eq!(n.sigma_edge_at(7.0), 0.006);
        // A band of overshoot beyond black, between black and white: blur.
        let (black, white) = ([0.0f32; 3], [1.0f32; 3]);
        assert!(n.explained_by_psf(1.5, [-0.05, -0.05, -0.05], black, white));
        assert!(n.explained_by_psf(2.0, [0.4, 0.4, 0.4], black, white));
        // Too wide, or a third colour: a face.
        assert!(!n.explained_by_psf(3.5, [0.4, 0.4, 0.4], black, white));
        assert!(!n.explained_by_psf(1.5, [0.9, 0.1, 0.1], black, white));
    }

    #[test]
    fn huber_is_chi2_inside_and_linear_outside() {
        assert_eq!(huber_kappa(f64::INFINITY), 3.0);
        assert_eq!(huber_kappa(4.0), 2.0);
        assert!((huber_kappa(3.0) - 3f64.sqrt()).abs() < 1e-15);
        assert_eq!(huber_kappa(100.0), 3.0);
        assert_eq!(huber(1.5, 2.0), 2.25);
        assert_eq!(huber(-3.0, 2.0), 8.0);
        // Continuous with a continuous slope at the threshold.
        let k = 2.0;
        assert!((huber(k + 1e-9, k) - huber(k - 1e-9, k)).abs() < 1e-7);
    }
}
