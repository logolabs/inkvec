//! Gradient regions: fitting a gradient to the whole region an artist filled with one,
//! rather than to whichever pair of palette bands happens to be adjacent.

/// Whether the region-level gradient recovery is on (`INKVEC_GRAD_REGIONS=1`).
pub(crate) fn enabled() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("INKVEC_GRAD_REGIONS").is_ok_and(|v| v != "0"))
}

/// Largest colour difference (CIEDE2000) between the inks of two *flat* adjacent regions
/// for them to be tried as bands of one ramp. Overridable with `INKVEC_RAMP_STEP`.
pub(crate) static RAMP_STEP_DE00: std::sync::LazyLock<f32> = std::sync::LazyLock::new(|| {
    std::env::var("INKVEC_RAMP_STEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15.0)
});
