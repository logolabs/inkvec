/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import Mathlib

/-!
# Counting flat directions

The boundary solve (`crates/inkvec-trace/src/boundary_opt.rs`) gives every boundary point
two unknowns, `x` and `y`. Linearised, its data term is a linear map from `2N` point
coordinates to the `M` band pixels' values; rank–nullity then forces at least `2N - M` flat
directions on it, whatever the image. `polyline_kernel` (in `NullSpace.lean`) shows the
smallest case exactly: `n + 1` vertices moving across an `n`-column run inside one pixel row
leave precisely one flat direction, the zigzag.

The remedy the theory gives is not a heavier prior but fewer unknowns than measurements,
in a family the measurements identify: `polynomial_boundary_determined` (in
`Identifiability.lean`) is the statement that a curve model of dimension at most the number
of columns it spans has no flat direction at all.
-/

namespace Inkvec

/-- **Rank–nullity for the boundary solve.** Any linear map from `N` free points of the plane
to `M` pixel values has a kernel of dimension at least `2N - M`. -/
theorem flat_directions (N M : ℕ) (J : (Fin N → ℝ × ℝ) →ₗ[ℝ] (Fin M → ℝ)) :
    2 * N ≤ M + Module.finrank ℝ (LinearMap.ker J) := by
  have h := LinearMap.finrank_range_add_finrank_ker J
  have hr : Module.finrank ℝ (LinearMap.range J) ≤ M := by
    have := Submodule.finrank_le (LinearMap.range J)
    simpa using this
  have hd : Module.finrank ℝ (Fin N → ℝ × ℝ) = 2 * N := by
    rw [Module.finrank_pi_fintype]; simp [Module.finrank_prod]; ring
  omega

/-- The same count with motion along a fixed normal only (one unknown per point). -/
theorem flat_directions_normal (N M : ℕ) (J : (Fin N → ℝ) →ₗ[ℝ] (Fin M → ℝ)) :
    N ≤ M + Module.finrank ℝ (LinearMap.ker J) := by
  have h := LinearMap.finrank_range_add_finrank_ker J
  have hr : Module.finrank ℝ (LinearMap.range J) ≤ M := by
    have := Submodule.finrank_le (LinearMap.range J)
    simpa using this
  simp at h
  omega

end Inkvec
