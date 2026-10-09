//! Core geometry for Inkvec.
//!
//! The type that matters here is [`Polyline`]: a boundary measurement carrying a
//! **per-point positional uncertainty**. That field is not decoration. Everything
//! downstream — the straightness test, the fit tolerance, the node budget — reads its
//! threshold from it rather than from a tuned constant, which is how "adaptive
//! simplification" stops being a separate heuristic and becomes a consequence of the
//! measurement model.
//!
//! A boundary recovered from a high-contrast edge is localized to a small fraction of a
//! pixel and should be fitted tightly. One recovered from a faint edge is barely
//! localized at all and should be simplified aggressively. Both facts are already in
//! `sigma`; no stage below needs to re-derive them.
//!
//! This crate sits below every other engine crate and depends on nothing of theirs. Besides
//! the geometry types it holds four small services the whole pipeline shares:
//!
//! - [`predicates`]: exact orientation and intersection tests (Shewchuk's adaptive
//!   arithmetic), so topology decisions never depend on an epsilon;
//! - [`env`](mod@env): the one place the engine reads environment variables (diagnostics and A/B
//!   switches only; options travel through `inkvec::Options`);
//! - [`progress`]: live progress reports and cooperative cancellation for one trace;
//! - [`clock`]: a monotonic clock that also works on `wasm32-unknown-unknown`.
//!
//! Coordinates throughout are in pixels of the input raster, `x` to the right and `y`
//! down, as `f64`.

pub mod env;
pub mod predicates;
pub mod progress;

/// A point in image space, in pixel units. Sub-pixel positions are the normal case.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal coordinate, in pixels.
    pub x: f64,
    /// Vertical coordinate, in pixels.
    pub y: f64,
}

impl Point {
    /// Builds a point from pixel coordinates.
    #[inline]
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Euclidean distance, in pixels, to another point.
    #[inline]
    pub fn dist(self, o: Point) -> f64 {
        (self - o).norm()
    }
}

/// `a - b` is the displacement vector from `b` to `a`.
impl std::ops::Sub for Point {
    type Output = Vec2;

    #[inline]
    fn sub(self, o: Point) -> Vec2 {
        Vec2 {
            x: self.x - o.x,
            y: self.y - o.y,
        }
    }
}

/// A displacement in image space, in pixel units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec2 {
    /// Horizontal component, in pixels.
    pub x: f64,
    /// Vertical component, in pixels.
    pub y: f64,
}

impl Vec2 {
    /// Vector length (Euclidean norm), in pixels.
    #[inline]
    pub fn norm(self) -> f64 {
        self.x.hypot(self.y)
    }

    /// 2D cross product (the z-component of the 3D cross product).
    #[inline]
    pub fn cross(self, o: Vec2) -> f64 {
        self.x * o.y - self.y * o.x
    }

    /// Dot product.
    #[inline]
    pub fn dot(self, o: Vec2) -> f64 {
        self.x * o.x + self.y * o.y
    }

    /// Direction of the vector, in radians, as returned by `atan2(y, x)`.
    #[inline]
    pub fn angle(self) -> f64 {
        self.y.atan2(self.x)
    }
}

/// A measured boundary: positions plus the uncertainty of each position.
///
/// `sigma[k]` is the standard deviation, in pixels, of point `k`'s position along the
/// boundary normal. The tracer fills it from the sub-pixel boundary stage (S2 in
/// `docs/DESIGN.md`): noise divided by the local edge gradient, combined in quadrature
/// with the method's own resolution limit. [`Polyline::with_uniform_sigma`] supplies a
/// constant instead, which reproduces classical tracing behaviour; tests, examples and
/// A/B experiments use it.
#[derive(Debug, Clone)]
pub struct Polyline {
    /// The measured boundary positions, in order.
    pub points: Vec<Point>,
    /// Per-point positional uncertainty, in pixels, aligned with `points`.
    pub sigma: Vec<f64>,
    /// True when the boundary is a closed loop with no forced junction vertices.
    /// Most edges in a planar map are *open* arcs between junctions, which is why the
    /// optimal-polygon dynamic program can use its simpler open-path form.
    pub closed: bool,
}

/// Smallest uncertainty we will admit, to keep `d / sigma` finite.
pub const MIN_SIGMA: f64 = 1e-6;

impl Polyline {
    /// Builds a polyline from points and their per-point uncertainty, clamping each
    /// `sigma` value to [`MIN_SIGMA`].
    pub fn new(points: Vec<Point>, sigma: Vec<f64>, closed: bool) -> Self {
        assert_eq!(points.len(), sigma.len(), "sigma must be per-point");
        let sigma: Vec<f64> = sigma
            .into_iter()
            .map(|s| {
                if s.is_finite() {
                    s.max(MIN_SIGMA)
                } else {
                    MIN_SIGMA
                }
            })
            .collect();
        #[cfg(debug_assertions)]
        debug_assert!(
            sigma.iter().all(|s| s.is_finite() && *s >= MIN_SIGMA),
            "sigma values must be finite and >= MIN_SIGMA"
        );
        Self {
            points,
            sigma,
            closed,
        }
    }

    /// Constant uncertainty. `sigma = 0.5` approximates a boundary localized only to
    /// the pixel grid, i.e. what a thresholding tracer actually knows.
    pub fn with_uniform_sigma(points: Vec<Point>, sigma: f64, closed: bool) -> Self {
        let n = points.len();
        Self::new(points, vec![sigma; n], closed)
    }

    /// Number of points in the polyline.
    #[inline]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// True when the polyline has no points.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Total arc length of the measured polyline, in pixels: the sum of its segment
    /// lengths. For a closed polyline the closing segment (last point back to the first)
    /// is not included. Zero for fewer than two points.
    pub fn length(&self) -> f64 {
        self.points.windows(2).map(|w| w[0].dist(w[1])).sum()
    }
}

/// A monotonic clock that exists on every target the tracer builds for.
///
/// Every time budget and timing mark in the tracer goes through this so the same code
/// runs natively and in the browser: `std::time::Instant` has no implementation on
/// wasm32-unknown-unknown and panics at the first call.
pub mod clock {
    #[cfg(not(target_arch = "wasm32"))]
    pub use std::time::Instant;
    #[cfg(target_arch = "wasm32")]
    pub use web_time::Instant;
}
