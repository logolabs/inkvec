/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).

# Axiom audit

`lake build InkvecTheory.Audit` prints, for every headline theorem, the axioms its proof
depends on. Every line must read `[propext, Classical.choice, Quot.sound]`: Lean's three
standard axioms, on which all of Mathlib's real analysis rests. No `sorry`, and no axiom of
this library's own, appears anywhere.
-/
import InkvecTheory

open Inkvec

#print axioms coverage_subgraph
#print axioms column_sum_eq_average
#print axioms invisible_perturbation
#print axioms sine_ripple_invisible
#print axioms rasterization_not_injective
#print axioms polyline_zigzag_invisible
#print axioms polyline_kernel
#print axioms column_fiber
#print axioms zeros_of_zero_averages
#print axioms poly_eq_of_averages
#print axioms polynomial_boundary_determined
#print axioms line_from_two_columns
#print axioms cubic_point_from_means
#print axioms quartic_defect
#print axioms boundary_point_from_pixels
#print axioms levelSet_reading
#print axioms levelSetBias_le
#print axioms levelSetBias_max
#print axioms column_sum_reading
#print axioms binarize_minimax
#print axioms quantize_minimax
#print axioms coverage_band
#print axioms straddle_inversion
#print axioms hidden_stroke_invisible
#print axioms centroid_bias
#print axioms centroid_bias_worst
#print axioms width_contrast_ambiguity
#print axioms flat_directions
#print axioms histopolate_cubic
#print axioms face_value_cubic
#print axioms side_stencil_cubic
#print axioms strip_reading_exact
#print axioms strip_unbiased
#print axioms strip_variance
#print axioms face_value_noise
#print axioms point_from_means_noise
#print axioms subCount_close
#print axioms ss_column_sum_close
#print axioms natural_model_closer
#print axioms natural_model_gain
#print axioms merge_chi2_increase
#print axioms noise_absorbed
#print axioms Inkvec.Gen.Expr.evalQ_sound
#print axioms Inkvec.Gen.Expr.evalI_sound
#print axioms Inkvec.Gen.Expr.nonneg_of_evalI
#print axioms Inkvec.Gen.histopolateK_eq
#print axioms Inkvec.Gen.stripResidualK_eq
#print axioms Inkvec.Gen.stripResidual_on_cubic
#print axioms Inkvec.Design.above_eq_of_interval
#print axioms Inkvec.Design.painter_interval_iff
#print axioms Inkvec.Design.replace_in_interval
#print axioms Inkvec.Design.seam_of_exact_outline
#print axioms Inkvec.Design.no_seam_of_completion
#print axioms Inkvec.Design.junction_residue
#print axioms Inkvec.Design.accept_monotone
#print axioms Inkvec.Design.huber_eq_sq
#print axioms Inkvec.Design.inlier_of_le
#print axioms Inkvec.Design.huber_le_sq
#print axioms Inkvec.Gen.spanWithinK_eq
#print axioms Inkvec.Gen.spanWithin_meaning
#print axioms Inkvec.Gen.spanLinkK_eq
#print axioms Inkvec.Gen.chain_cover
#print axioms Inkvec.Gen.line_cover
#print axioms Inkvec.window_sum_eq_area
#print axioms Inkvec.trapezoid_integral
#print axioms Inkvec.window_sum_resampled
#print axioms Inkvec.blur_affine_profile
#print axioms Inkvec.blur_quadratic_profile
#print axioms Inkvec.Gen.trapezoidK_integral
#print axioms Inkvec.Gen.windowTermK_eq
#print axioms Inkvec.Gen.fourthDiff_cubic
#print axioms Inkvec.Gen.fourthDiff_side
#print axioms Inkvec.Gen.cornerExcessK_eq
