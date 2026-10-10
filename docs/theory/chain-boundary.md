# Chain B: from pixels to a likelihood over descriptions

> The boundary chain of [`chains.md`](chains.md), stated and argued informally: what the
> pixels measure, the exact likelihood of any candidate description against them, where
> that likelihood is flat, and how precisely it fixes every coordinate the representation
> might write. Each claim says whether it is proved here informally, proved in
> [`formal/InkvecTheory`](../../formal/InkvecTheory/README.md), measured, or open. The
> section [Interface proposal](#interface-proposal) is what this chain hands to the
> representation chain; [Phase 2](#phase-2-formal-then-code) is how it becomes Lean and Rust.

## Summary

* **One identity carries the chain.** Summed over any set of whole pixels, a face's unmixed
  weights are the area of that face inside those pixels, for any shape whatever (B2.1). So
  any candidate description (a curve, a spline, an arc, a primitive, a stroke, the visible
  partition of a layered drawing) can be scored exactly against the pixels by computing
  areas, with no boundary points extracted first. The column-sum theorem
  (`column_sum_eq_average`) is the special case of a graph and one column.
* **The likelihood** is a sum of squared area residuals over a partition of the boundary's
  pixels into windows, each with a calibrated variance (B2.2-B2.5). Column windows keep at
  least 3/4 of the information that per-pixel terms carry when 8-bit quantisation dominates,
  and all of it when supersampling dominates (argued here; measured to three digits).
  Evaluating it takes time linear in the windows a candidate touches, and for curves linear
  in their parameters it is a quadratic with prefix-sum coefficients.
* **The variances are known.** 8-bit quantisation gives each window `n_partial / (12·255²)`
  on a grey axis (measured 0.00114 per partial pixel against 0.00113); `n × n` point
  supersampling adds `1/(12n³)` per window at generic slopes (measured 0.0129 against 0.0128
  at `n = 8`) and a bias of up to `1/(2n)` correlated along edges near 0° and 45°. The
  corpus's own renderer has to be measured: the fourth difference of consecutive column
  means estimates its floor per image, and it is the statistic `strip.rs`'s corner test
  already computes.
* **Corners, junctions and thin strokes are not special to the likelihood** (B3): their
  pixels are windows like any other, scored per pixel. What is special is identifiability
  and precision. A vertex is fixed by its arms to `√(2V(1 + 12 s_m²/L²)/L) / sin α`
  (measured within Monte Carlo error at 8-bit, within a factor of 1.4 under 8×8 point
  supersampling, whose errors are not independent near 0° and 45°); a fillet can be told
  from a sharp corner only above `r ≈ √(3σ / (tan(τ/2) − τ/2))` (0.13-0.42 px for a right
  angle, 0.7-2.5 px for a 30° turn); a sub-pixel stroke along a pixel border has a
  likelihood that is exactly flat over an interval. So B3 proposes and reports; it does not
  decide.
* **Precision belongs to the parametrisation** (B4). The data never decide whether a
  likelihood is flat: the parametrisation does (the zigzag of `polyline_kernel` is a
  parametrisation's null space). The Fisher information of any parametrisation is
  `∫ ρ(s) N(s) N(s)ᵀ ds`, with an information density `ρ` B2 supplies per unit of boundary
  length. At the corpus's resolutions every position is known to well under a hundredth of
  a design unit, so whether a coordinate lies on the design grid is never in doubt; what the
  pixels leave in doubt is structure.
* **Proposed shortening**: corners, junction continuation, stroke-or-fill and
  gradient-or-flat become the representation's decisions, tested against this likelihood;
  B3 shrinks to proposals and per-pixel terms; stage 08's point solve, its kink and anchor
  priors, and `DEFAULT_SIGMA_MODEL` are no longer needed once the fitter scores curves
  against calibrated windows.

| link | claim | status |
|---|---|---|
| B1.1 | a face holding an axis-aligned 2 × 2 square has a pure pixel, which reads a flat ink exactly | informal (elementary) |
| B1.2 | two-ink unmixing is unbiased, variance `dᵀΣd/‖d‖⁴`, at most `(Δ²/12)(‖d‖₁/‖d‖₂²)²` (grey axis) | informal; measured |
| B1.3 | three or more inks: weights need affine independence, geometry needs neighbours; the colour residual needs neither | informal |
| B1.4 | affine inks: exact centre values, first-moment correction, bias at most `|ΔG·ν|/(8‖d‖)` if ignored | informal |
| B1.5 | inks without pure pixels: hidden strokes fix width × contrast; border-straddling strokes fix neither width nor centre without the ink | Lean (first), informal (second) |
| B2.1 | window sum = area of the face in the window, any region (Cavalieri, Green) | Lean for graphs; general form informal |
| B2.2 | window sums lose information iff sensitivities are not proportional to variances; column sums keep `(1 − t/2)/(1 − t/3) ≥ 3/4` (8-bit), all (supersampled) | informal; measured |
| B2.3 | window variances under 8-bit and `n × n` point supersampling; per-image floor estimator | informal (bound in Lean); measured |
| B2.4 | calibration `E χ² = M`; cost linear; prefix sums for linear families | informal; measured |
| B2.5 | first variation `∂A_W = ∫_{γ∩W} δ ds`; information density `ρ = ℓ_W / V_W` | informal (classical) |
| B3.1 | wedge identifiable from about `2 + 3/sin α` px; vertex precision formula | informal; measured |
| B3.2 | junction covariance `(Σ νν/σ²)⁻¹`; conditioning `tan²(α/2)` | informal |
| B3.3 | fillet area `r²(tan(τ/2) − τ/2)`; smallest distinguishable fillet; corner proposals by the fourth difference | informal; measured |
| B3.4 | thin strokes: straddle exact, hidden flat, slanted fraction hidden `max(0, 1 − w/cos θ − |t|)` | Lean (first two); informal |
| B4.1 | the log-likelihood is exactly a sum of edge, vertex and strip terms; couplings are shared windows | informal |
| B4.2 | identifiability ⟺ `rank J` full; null space is the parametrisation's | informal; Lean for the examples |
| B4.3 | covariance `F⁻¹`; closed forms for lines, circles, vertices; the supersampling floor | informal (Gauss-Markov); measured |
| B4.4 | nested and Wald tests, consistency, boundary-of-parameter-space mixture | Lean for nested linear; informal |

## 0. The model, stated once

* **Pixels.** Pixel `(i, j)` is the open square `(i, i+1) × (j, j+1)`, as in the Lean
  development; inkvec's code puts pixel centres on integers, so its coordinates are these
  minus ½. `λ` is area (Lebesgue measure).
* **Descriptions.** A candidate description `𝓓` (what the representation proposes)
  determines a partition of the plane into face regions `Ω_f` (for a layered drawing, its
  visible partition) and an ink per face: a flat colour `c_f`, an affine colour field
  `c_f(x) = c_f(x₀) + G_f (x − x₀)` (a linear gradient between two stops, in the space the
  renderer interpolates in), or a radial field.
* **Rendering.** Exact area: `P_p = Σ_f ∫_{Ω_f ∩ p} c_f(x) dx`, composited linearly in the
  encoded values (resvg does, and so does the corpus builder's box filter,
  `bench/build_corpus_v2.py`); with transparency, in premultiplied channels
  `(colour × α, α)`, where mixtures are linear (Porter & Duff 1984, doi:10.1145/800031.808606)
  and straight colour is not.
* **Observation.** `O_p = Q(P_p + e_p)`, with `Q` the 8-bit quantiser and `e_p` the
  renderer's departure from exact area (point supersampling, curve flattening, per-layer
  compositing): the *renderer floor*.
* **The statistical surrogate.** Window errors are independent and zero-mean with known
  variances `V_W` (B2.3), Gaussian where a density is needed. Every precision statement
  below is also the exact covariance of the weighted least-squares estimate for families
  linear in their parameters (the Gauss-Markov theorem needs means and variances only), so
  it does not rest on Gaussianity. The surrogate is conservative: under exact uniform
  quantisation the consistent set of a boundary shrinks faster than least squares
  (Havelock's locales), but the renderer floor violates the hard bounds that would exploit
  it, so it is not used.

Nothing is assumed about the curve.

## B1. Inks and mixtures

**In:** the raster. **Out:** per face, candidate ink models (flat, linear, radial) with
their evidence; per pixel, either its unmixed weight against its two inks with its variance
(two flat inks), or its raw colour with its covariance (three or more inks, or a gradient
on either side).

### B1.1 Pure pixels fix flat inks

*Claim.* A face that contains an axis-aligned square of side 2 contains a whole pixel. A
pure pixel of a flat ink reads the ink exactly when the ink is on the 8-bit lattice, as an
artist's hex colour is; otherwise every pure pixel reads it to within half a level per
channel, and their mean converges.

*Proof.* The square `[u, u+2] × [v, v+2]` contains `[⌈u⌉, ⌈u⌉+1] × [⌈v⌉, ⌈v⌉+1]`,
since `⌈u⌉ + 1 ≤ u + 2`. That pixel's coverage by the face is 1, so `P_p = c_f` and
`Q(c_f) = c_f`. ∎ (A disc of radius √2 contains such a square, so faces with that inradius
qualify; a diagonal stroke narrower than √2 px need not.)

### B1.2 Two-ink unmixing

*Claim.* For `P = B + a·d + e`, `d = F − B ≠ 0`, the projection
`â = (P − B)·d / ‖d‖²` equals `a + e·d/‖d‖²`. It is unbiased whenever `e` has zero mean,
and must not be clamped to `[0, 1]` (a clamp biases every sum it enters, which is why
`strip.rs` does not). Its variance is `dᵀ Σ_e d / ‖d‖⁴`: `Δ² / (12‖d‖²)` when the three
channels round independently (`Δ = 1/255`), and at most `(Δ²/12)(‖d‖₁/‖d‖₂²)²` whatever
their correlation. The bound is attained on a grey axis, where the three channels hold the
same value and round identically: black on white has variance `Δ²/12`, three times the
independent value. Deterministically, `|â − a| ≤ (Δ/2)·‖d‖₁ / ‖d‖₂²`. The residual
orthogonal to the axis, `P − B − â·d`, is noise for a two-ink pixel; its size against `Σ_e`
(two degrees of freedom) is the test for a third ink.

*Proof.* Linearity of the projection. With `|Σ_e,cc'| ≤ σ²` for any correlation,
`dᵀΣ_e d ≤ σ² (Σ_c |d_c|)²`, with equality when the channel errors are perfectly
correlated with the signs of `d`'s components, as on a grey axis. ∎

*Measured* (`bench/theory/evidence_eval.py calib`, discs, exact area, 8-bit, black on
white): window residuals have rms 0.00114 per √(partial pixel), against `Δ/√12 = 0.00113`,
with no lag-1 correlation along the boundary (−0.003).

### B1.3 Three or more inks

*Claim.* (i) A pixel's weights over `k` inks are determined by its colour exactly when the
inks are affinely independent (so `k ≤ 4` in RGB, `k ≤ 5` with alpha). (ii) Whatever the
inks, the pixel's term of the likelihood is its colour residual
`(O_p − μ_p(𝓓))ᵀ Σ_p⁻¹ (O_p − μ_p(𝓓))` with `μ_p(𝓓) = Σ_f a_f(𝓓) c_f`, so scoring a
candidate needs no unmixing at all. (iii) One pixel never fixes a junction: a junction of
`k` straight rays has `k + 2` parameters and its pixel at most `k − 1` independent weights,
so the geometry comes from the neighbours (B3.2).

*Proof.* (i) `Σ_f a_f = 1` and `Σ_f a_f c_f = P` form a linear system whose matrix
`[c_f; 1]` has full column rank exactly under affine independence. (ii) is the model.
(iii) counts. ∎ So the earlier formulation, "a junction pixel's weights are identifiable
only with geometry from its neighbours", splits in two: the weights need affine
independence, the geometry always needs neighbours, and the likelihood needs neither.

### B1.4 Affine inks

*Claim.* For an affine ink `c(x) = c(x_p) + G (x − x_p)`, `x_p` the pixel centre:
(i) a pure pixel reads `c(x_p)` exactly (an affine function's mean over a square is its
value at the centre), so three pure pixels with non-collinear centres fix the field.
(ii) A pixel shared by faces `f` and `g` reads

```text
P = a·c_f(x_p) + (1 − a)·c_g(x_p) + (G_f − G_g)·m_f,     m_f = ∫_{Ω_f ∩ p} (x − x_p) dx,
```

the first moment of `f`'s part about the centre, a geometric quantity of the candidate
computable by the same line integrals as its area (B2.1). (iii) Unmixing against the centre
values and ignoring the moment biases the weight by at most `|(G_f − G_g)·ν| / (8‖d‖)` for a
straight edge with normal `ν` (the moment is `a(1 − a)/2` along the normal on an
axis-aligned edge, at most 1/8; √2/12 on a diagonal). (iv) Radial fields and gradients with
interior stops are affine only piecewise: on a pixel the second-order term is `tr(H)/24`
(the midpoint rule's error on a square), negligible unless the field curves within a few
pixels, and a stop inside a pixel is an ink boundary with no geometric edge.

*Proof.* Linearity of the integral, and `m_f + m_g = 0` because the pixel's own first
moment about its centre vanishes. For (iii), on an axis-aligned edge `f` holds a strip of
height `a` whose centroid sits `(1 − a)/2` from the centre. ∎

The term matters: a gradient spanning half the colour range over 50 px (`|G| = 0.01/px`)
next to a flat face at contrast 0.3 biases a weight by up to 0.004, several times the
quantisation noise at that contrast. So windows next to a gradient face are kept per pixel,
with the moment in the model.

### B1.5 Inks without pure pixels

*Claim.* (i) A stroke inside one pixel row fixes only width × contrast
(`width_contrast_ambiguity`, proved). (ii) A sub-pixel stroke that straddles one pixel border
along its whole length fixes neither its width nor its centre without its ink: any contrast
`kK` with both edges scaled towards the border by `1/k` gives the same pixels. (iii) A stroke
that straddles different borders along its length (any slanted stroke) fixes its ink, with
information that grows with the number of distinct offsets.

*Proof.* (ii) From `straddle_inversion`, `a₋ = j+1 − lo`, `a₊ = hi − (j+1)` and
`P± = B + a±(F − B)`; replacing `F − B` by `k(F − B)` and `a±` by `a±/k` leaves both pixels
unchanged and is the stroke `lo' = j+1 − a₋/k`, `hi' = j+1 + a₊/k`. (iii) Two columns that
straddle different borders scale about different points, which agree only for `k = 1`. ∎

B1 therefore outputs, per face, the candidate ink models with their parameter counts,
interior `χ²` and covariance, and leaves the choice to the representation (proposal S3).

## B2. The likelihood of any description

**In:** the raster, B1's inks, a starting map (topology from the label map, geometry from
stage 07). **Out:** a list of window observations and an evaluator of `χ²(𝓓)` for any
candidate description `𝓓`.

### B2.1 The window identity

*Claim.* For every measurable region `Ω` and every finite set `W` of pixels,

```text
Σ_{p ∈ W} coverage(Ω, p) = λ(Ω ∩ ⋃W).
```

For a column window `R = (i, i+1) × (j₀, j₀+m)` this is
`∫_i^{i+1} λ₁{ y ∈ (j₀, j₀+m) : (x, y) ∈ Ω } dx`, the column integral of the length of the
region's vertical cross-section inside the window. If the window's top side lies outside
`Ω` (its top pixel is pure in the other face) and `∂Ω` is piecewise smooth, it is the line
integral `∫_{∂Ω ∩ R} (y − j₀) dx` along the part of the boundary inside the window, oriented
so that a graph contributes positively. For the region below a continuous graph `f` it is
`∫_i^{i+1} f − j₀`: `column_sum_eq_average`.

*Proof.* Pixels are disjoint, so their areas add; the grid lines between them are null;
Tonelli's theorem slices the window into vertical segments; Green's theorem with the form
`(y − j₀) dx` gets no contribution from the window's vertical sides (`dx = 0`), none from its
bottom (`y = j₀`), and none from its top by hypothesis. ∎

*Consequences.* No slope, normal, curvature, graph property or threshold enters. The model
value of any candidate in any window is a sum of line integrals along its boundary inside
the window, in closed form: trapezoids for line segments, polynomials in the parameter for
Bézier segments (after solving `x(t) = i` and `x(t) = i + 1`), sector-and-triangle terms for
circular arcs. A candidate that leaves the window vertically is handled by clamping the
cross-section, which adds its crossings of the window's top and bottom. A layered candidate
is scored through its visible partition (but see B3.2 on per-layer compositing).

*Status.* Lean for graphs (`column_sum_eq_average`); the general form is phase 2 (finite
additivity and Tonelli). The identity is volume-of-fluid's (Puckett 2010, eq. 11) and the
partial-area effect's (Trujillo-Pino et al. 2013, doi:10.1016/j.imavis.2012.10.005). Not
from the literature: using it as the likelihood interface between boundary recovery and
representation.

### B2.2 The likelihood, and what windows cost

*Definition.* Partition the band (the pixels any candidate within reach can touch) into
windows: on runs, column or row windows from a pure pixel of one face to a pure pixel of the
other, crossed by exactly one boundary piece (columns where the boundary is within 45° of
horizontal, rows elsewhere); single pixels where two or more boundary pieces share pixels
(vertex neighbourhoods, thin features) and next to gradient faces. A two-flat-ink window
observes `S_W = Σ_{p ∈ W} â_T(p)`, the weight of its top face `T`, with variance `V_W`;
its model is `A_W(𝓓) = λ(Ω_T(𝓓) ∩ W)`. A single pixel with more inks, or a gradient
neighbour, observes its colour `O_p` with covariance `Σ_p`; its model is the area- and
moment-weighted mixture of B1.3 and B1.4. Then

```text
χ²(𝓓) = Σ_W (S_W − A_W(𝓓))² / V_W  +  Σ_p (O_p − μ_p(𝓓))ᵀ Σ_p⁻¹ (O_p − μ_p(𝓓)),
log-likelihood = −χ²/2 + const.
```

*Claim (what a window sum keeps).* With independent per-pixel errors of variances `σ_p²`
and sensitivities `g_p = ∂a_p/∂θ`, the window sum carries the information
`F_W = (Σ g_p)(Σ g_p)ᵀ / Σ σ_p²`, never more than the per-pixel
`F_pix = Σ g_p g_pᵀ / σ_p²`, and equal to it exactly when `g_p` is proportional to `σ_p²`
across the window's pixels.

*Proof.* For any direction `u`, Cauchy-Schwarz gives
`(Σ uᵀg_p)² = (Σ (uᵀg_p/σ_p)·σ_p)² ≤ (Σ (uᵀg_p)²/σ_p²)(Σ σ_p²)`, with equality when
`uᵀg_p/σ_p ∝ σ_p`. ∎

*Corollary (8-bit regime).* For the normal offset of a straight edge, with equal noise on
every partial pixel and none on pure ones, column sums keep `(1 − t/2)/(1 − t/3)` of the
per-pixel information, `t = |tan θ| ≤ 1` the slope in the window's frame: never less than
3/4, reached at 45°.

*Proof.* The sensitivity of a pixel to the normal offset is its chord `ℓ_p`. A column is
crossed with chord `1/cos θ`; with probability `1 − t` (over the edge's position) it has one
partial pixel, with probability `t` two, split `u : 1 − u` with `u` uniform. Per column the
window keeps `(Σℓ)²/n` against `Σℓ²`, in expectation `ℓ²(1 − t + t/2)` against
`ℓ²(1 − t + 2t/3)`. ∎

*Corollary (supersampled regime).* Under `n × n` point supersampling a pixel's error is the
sum of the rounding errors of the sample columns whose boundary falls in its row
(`ss_column_sum` writes the column sum that way), so its variance is proportional to its
share of the column's sample columns, that is to its chord: the equality condition holds,
and column sums keep all the information about the offset.

*Measured* (`evidence_eval.py info`): 1.000, 0.969, 0.932, 0.880, 0.807 at 0°, 10°, 20°,
30°, 40°, against 1.000, 0.969, 0.931, 0.881, 0.805 predicted. For polynomial curves the sums
also lose no identifiability: a polynomial of degree `< n` is fixed by its means on `n`
columns (`poly_eq_of_averages`).

So: column windows on runs, where they are smooth in the parameters, cheap, and keep at
least 3/4 of the information; per-pixel terms wherever two boundary pieces share a window,
since there a window sum measures only their difference (a thin stroke's width) and loses
both positions. Not from the literature: the information ratio.

### B2.3 The variances

*Claim.* (a) **Quantisation.** `V_W = Σ_{p ∈ W partial} var(â_p)` (B1.2): pure pixels
contribute nothing when the inks are on the 8-bit lattice. (b) **Point supersampling.** A
window's error is the mean of `n` per-sample-column rounding errors, each within `±1/(2n)`
(`subCount_close`), so it is within `1/(2n)` (`ss_column_sum_close`). The sub-row phase of
consecutive sample columns differs by the slope `t` (in sub-rows per sample column); away
from `t ≈ 0` and `t ≈ 1` the `n` errors behave as independent uniform variables
(equidistribution) and `V = 1/(12n³)`; near 0° and 45° they coincide and the error is a bias
of up to `1/(2n)` (rms about `1/(n√12)`), constant along an exactly axis-aligned or diagonal
edge and periodic along slopes with small denominators. (c) The two add. (d) **The renderer
floor** of a real renderer must be measured. The corpus is rendered by resvg at 8× and box
filtered, and resvg's own anti-aliasing inside each of the 64 sub-pixels makes its sampling
finer than `8 × 8` points, so (b) at `n = 8` is an upper bound. Estimator: along a run, the
fourth difference `D = h₋₂ − 4h₋₁ + 6h₀ − 4h₁ + h₂` of five consecutive windows' mean
heights annihilates cubics and has variance `70 V` under independent errors, so
`V̂ = (median |D| / 0.6745)² / 70` over an image's runs estimates the floor robustly against
the few corners (the difference-based variance estimators of Rice 1984; Gasser, Sroka &
Jennen-Steinmetz 1986; Hall, Kay & Titterington 1990). Its quartic bias is `f''''`
(`quartic_defect`), negligible unless the curvature radius is a few pixels.

*Proof.* (a) B1.2 and independence; (b) as stated, the per-sample-column count reads the
boundary height rounded to the sub-row lattice (`subCount_close`) and the column sum is
their mean (`ss_column_sum`); independence away from integer phase steps is a model, checked
below. (d) The weights `1, −4, 6, −4, 1` sum in squares to 70 and annihilate cubics. ∎

*Measured* (`evidence_eval.py calib`, discs of radius 6-26 px at 64 px, every window):

| render | window residual std | predicted | within 0.02 rad of the window's axis | lag-1 correlation |
|---|---|---|---|---|
| exact area, float | 0 | 0 | 0 | — |
| exact area, 8-bit | 0.00135 | 0.00135 | — | −0.003 |
| 8 × 8 points, 8-bit | 0.0129 | 0.0128 | 0.035 (bias, bound 0.0625) | −0.029 |
| 16 × 16 points, 8-bit | 0.0044 | 0.0047 | 0.014 (bound 0.031) | +0.028 |
| 32 × 32 points, 8-bit | 0.0021 | 0.0021 | 0.0065 (bound 0.016) | +0.077 |

The first row is B2.1 checked on every window: the window sums of an exact render equal the
exact areas, curvature and slope notwithstanding. The 45° regime shows in the spread by
angle: under 8 × 8 points, 0.0123 between 23° and 45° against 0.0073 between 7° and 23°.
Not from the literature: the per-window variance model under point supersampling and its
near-axis bias regime.

### B2.4 Calibration and cost

*Claim.* With the variances of B2.3, `E χ²(truth) = M`, the number of windows. For a
candidate family of `p` parameters that contains the truth and is linear in its parameters
on the windows, the minimum of `χ²` has mean `M − p` (`noise_absorbed`: each fitted
dimension absorbs one unit of noise). So "explains the pixels within their noise" is a test
on numbers the evaluator returns: `χ² ≤ M − p + z√(2(M − p))`.

*Cost.* One pass over the windows a candidate touches, with closed-form integrals; Bézier
segments need the roots of `x(t) = i` per grid line, a bounded computation. For families
linear in their parameters as graphs over the window axis (lines, polynomials),
`S_W − A_W(θ)` is affine in `θ`, so the `χ²` of a fit over any contiguous range of windows
is a quadratic whose coefficients are prefix sums: `O(p²)` per dynamic-programming
candidate after `O(M p²)` preparation per edge.

*Measured.* At the truth, `χ²/M` is 1.00 for exact area at 8-bit and 1.01 for 8 × 8 points
(the same table). The near-axis windows are the exception (their bias is several times the
generic spread), which is why the floor is reported separately (B4.3).

### B2.5 First variation and information density

*Claim.* If a candidate's boundary moves along its normal by `δ(s)`, the window area moves by
`∫_{γ ∩ W} δ ds` to first order. So for parameters `θ` with normal velocity
`N_θ(s) = ∂δ/∂θ`, the Jacobian is `J_W = ∫_{γ∩W} N_θ ds` and the Fisher information

```text
F = Σ_W J_W J_Wᵀ / V_W  ≈  ∫_γ ρ(s) N_θ(s) N_θ(s)ᵀ ds,      ρ = ℓ_W / V_W,
```

with `ℓ_W` the boundary's length inside the window: the information density per unit of
boundary length. For a column window at angle `θ` to its axis, `ρ = 1/(V_W cos θ)`.

*Proof.* The first variation of area (moving a boundary by `δν` adds or removes a layer of
thickness `δ`); the continuum form replaces `N_θ` by its value in the window. ∎

## B3. Vertices and thin features

**In:** the places where a single run's window stops being the whole story: a tangent
discontinuity, three or more inks, two edges of a feature under two pixels wide. **Out:**
(1) per-pixel observations there (B2.2), each owned by its vertex or stretch; (2) proposals:
junction nodes from the topology, corner candidates along runs with their test statistic,
thin stretches; (3) for each, the identifiability and precision the likelihood offers,
including its flat directions; (4) starting geometry. **No decision**: whether a candidate
corner is a corner, where a hidden stroke's centre lies, which arms continue through a
junction, are the representation's (proposal S1).

### B3.1 Corners (wedges)

*Claim (identifiability).* A corner of apex angle `α` whose arms are straight within
radius `r` of the vertex is fixed by the pixels in that disc when each arm has two windows
of its own (pure flanks, no other boundary). That holds beyond about `3/sin α` px from the
vertex for `α ≤ 90°` (3 px for obtuse corners), so `r ≈ 2 + 3/sin α` suffices.

*Proof.* The other arm lies `d sin α` from a point at distance `d` along this one (`≥ d`
when `α > 90°`), and a run window reaches at most about 3 px across its crossing. Two
windows of one arm give its line exactly (`line_from_two_columns`); two non-parallel lines
meet once. ∎ (Sufficient, not sharp: a very acute corner is a thin stroke near its apex,
B3.4.)

*Claim (precision).* An arm fitted over arclength `[d₀, d₀ + L]` from the vertex, with
information density `ρ`, has normal error at the vertex
`σ_n² = (1/(ρL))·(1 + 12 s_m²/L²)`, `s_m = d₀ + L/2`; two arms with independent errors place
the vertex within `E|Δv|² = (σ_n1² + σ_n2²) / sin² α`.

*Proof.* Least squares on a line: offset variance `1/(ρL)` at the midpoint, slope variance
`12/(ρL³)`, uncorrelated; extrapolate to `s = 0`. The vertex error solves `ν_k·Δv = ε_k`,
whose inverse has `|Δv|² = (ε₁² + ε₂² − 2ε₁ε₂ cos α)/sin² α`. ∎

*Measured* (`evidence_eval.py vertex`, 30 random wedges each, arms fitted by column-sum
least squares, `d₀` as above; RMS vertex error against the prediction, px):

| α | L | exact area, 8-bit | predicted | 8 × 8 points, 8-bit | predicted |
|---|---|---|---|---|---|
| 30° | 6 | 0.0073 | 0.0070 | 0.053 | 0.066 |
| 30° | 12 | 0.0033 | 0.0035 | 0.022 | 0.033 |
| 60° | 12 | 0.0018 | 0.0017 | 0.013 | 0.017 |
| 90° | 6 | 0.0030 | 0.0027 | 0.023 | 0.025 |
| 90° | 12 | 0.0014 | 0.0015 | 0.008 | 0.014 |
| 120° | 12 | 0.0020 | 0.0017 | 0.014 | 0.016 |
| 150° | 6 | 0.0042 | 0.0053 | 0.065 | 0.051 |
| 150° | 12 | 0.0033 | 0.0029 | 0.026 | 0.028 |

At 8-bit the formula holds within the Monte Carlo error of 30 trials (about 13 % on an rms).
Under 8 × 8 points it holds within a factor of 1.4 either way: arms near 0° or 45° carry the
correlated bias of B2.3, which a line fit partly absorbs and partly does not.

The vertex's own pixels add the information of a few pixels; the arms dominate as soon as
they are a few pixels long. So the vertex is best fixed by fitting the arms the
representation chooses (lines, arcs, cubics) jointly with the per-pixel terms around it,
not by a local wedge model: straight local arms are biased by about `κd²/(2 sin α)` on a
curved arm, the representation's own arms are not. That is the case for moving corners
across the interface (S1).

### B3.2 Junctions

*Claim.* `k` arms whose normal errors at the vertex are `σ_k` give
`Cov(v) = (Σ_k ν_k ν_kᵀ / σ_k²)⁻¹`. The vertex is identified exactly when two arms are not
parallel; for two arms the eigenvalues of `Σ ν_k ν_kᵀ` are `1 ± cos α`, a ratio of
`tan²(α/2)`, which is what `junctions.rs`'s `JUNCTION_MIN_CONDITION = 0.02` thresholds
(about 16°). A tangential junction (a taper) is flat along the common tangent: B3 reports
that direction and its extent, and the representation's prior (a tangency, a taper
constraint) decides. Three-ink pixels enter through their colour residual (B1.3).

*Proof.* Weighted least squares for the intersection of lines (the normal equations
`junctions.rs`'s `solve_junction` already forms), with the arms' variances from B3.1 instead
of stage 07's per-point sigmas. ∎

*Open.* A layered description is rendered by compositing each layer with its own coverage,
which differs from the exact area of the visible partition in pixels that edges of two
layers both cross (the conflation of coverage compositing). Those are junction pixels of
the visible map. At 8× supersampling with a box filter the difference is confined to the
sub-pixels both edges cross; its size on the corpus has not been measured, and the vertex
term should use per-layer compositing when the candidate is layered.

### B3.3 Corner or rounded corner

*Claim.* A fillet of radius `r` on a corner of turning angle `τ` removes
`r²(tan(τ/2) − τ/2)` of area. While that lies in one pixel, the sharp corner and the fillet
differ by `Δχ² = (r²(tan(τ/2) − τ/2))² / σ_p²`, so they are distinguishable at
`Δχ² ≥ 9` only for

```text
r ≥ √(3σ_p / (tan(τ/2) − τ/2)).
```

*Proof.* The region between the corner and the arc is the kite of the centre, the two
tangent points and the corner (area `r² tan(τ/2)`) less the sector (`r²τ/2`). ∎

*Measured* (`evidence_eval.py fillet`, per-pixel `χ²` with equal noise per pixel, at
`σ_p = 0.00113` for 8-bit and `1/√(12·8³) = 0.0128` for 8 × 8 points): the bound is 0.13 and
0.42 px for a right angle, where `Δχ²` measures 3.4 / 55 at `r` = 0.1 / 0.2 px and 5.8 / 97
at 0.4 / 0.8 px; 0.25 and 0.84 px for 60°; 0.74 and 2.5 px for 30°. Below those radii the
pixels cannot tell a fillet from a sharp corner and the prior must (icons' corners are sharp
or rounded by at least a design unit, several pixels).

A fillet against a sharp corner is a test on the boundary of the parameter space
(`r ≥ 0`), whose null distribution is `½χ²₀ + ½χ²₁` (Chernoff 1954; Self & Liang 1987):
`Δχ² > 5.41` gives a 1 % false-alarm rate. Not from the literature: the resolution formula.

*Corner proposals.* `strip.rs`'s corner test compares the cubic through four column means
with the one-sided cubics at the vertex's pixel border. The one-sided difference is exactly
`D/12` with `D` the fourth difference of B2.3 (weights `(1, −4, 6, −4, 1)/12`, checked in
rationals), which is why every cubic passes (`side_stencil_cubic`) and why `strip.rs`
reports "0.7 times the noise of one column mean" (`√70/12 = 0.70`). A kink with slope
change `Δs` at the stencil's centre gives `|D| = 1.25 Δs` at most (the second difference of
the quadratic B-spline at its centre). So corners should be proposed where
`|D| > z√(70 V̂)`, `z ≈ 3`, which finds kinks down to `Δs = z√(70V)/1.25`: about 1.6° at
8-bit black on white, 14° at 8 × 8 points. The fixed `SIDE_TOL = 0.05` px is `|D| > 0.6`,
`Δs ≈ 0.48`, about 26°: the "gentle corner under about 30°" that `README.md` §7 says the
strip reading cannot tell from noise. Proposals should favour recall: a false proposal
costs the representation one `Δχ²` test, a missed one a corner.

### B3.4 Thin features

*Claim.* (i) A stroke straddling a pixel border along a column is read exactly
(`straddle_inversion`): centre `j + 1 + (a₊ − a₋)/2` with variance `(σ₋² + σ₊²)/4`, width
`a₋ + a₊` with variance `σ₋² + σ₊²`. (ii) Inside one pixel row its likelihood is exactly flat
in the centre over an interval of length `1 − w` (`hidden_stroke_invisible`): flat-bottomed,
not Gaussian. (iii) A straight stroke of width `w` at slope `t` is hidden in a column with
probability `max(0, 1 − w/cos θ − |t|)` over its position; when `w/cos θ + |t| ≥ 1` every
column informs the centreline, and otherwise a parametric centreline with at least two
non-hidden columns per segment is still fixed. (iv) The ink: B1.5.

*Proof.* (i) and (ii) are proved in Lean; the variances are `weighted_variance` with
weights `(−½, ½)` and `(1, 1)`. (iii) The stroke's vertical extent in a column is
`w/cos θ + |t|`, and it is hidden when that extent lies within one row. ∎

What B3 reports for a thin stretch: its per-pixel windows, and for every part where the
likelihood is flat, the interval. The representation's prior places the centre (on the
design grid, or at the pixel centre, the minimax choice).

### B3.5 A refusal is a report

Where the likelihood is flat or elongated (hidden strokes, tangential junctions, a gentle
corner's position along its line, arms shorter than two windows, parts of a face hidden
behind others, which have no windows at all), B3 reports the flat direction and its extent
instead of a point. Nothing in this chain guesses; the prior decides, in the representation.

## B4. What the pixels can confirm

**In:** B2's observations and evaluator; a parametrisation `θ ↦ 𝓓(θ)` from the
representation. **Out:** the decomposition of the log-likelihood into local terms and their
couplings; whether `θ` is identifiable; the precision of every coordinate and constraint;
the test statistics R2 needs.

### B4.1 The decomposition

*Claim.* Exactly, with no cross terms,

```text
χ²(𝓓) = Σ_edges χ²_e(γ_e) + Σ_vertices χ²_v(γ_e : e ∋ v) + Σ_strips χ²_s(γ_e, γ_e'),
```

where `χ²_e` sums the windows only edge `e` crosses, `χ²_v` the per-pixel terms around
vertex `v`, and `χ²_s` those of a thin stretch both its sides cross. Two edges interact only
through windows they both cross, so the Fisher matrix is block-sparse along that graph, and
edges can be fitted independently (in parallel, as now) except for the vertex and strip
terms, which a joint polish of the end segments settles.

*Proof.* Each window is assigned to one term, and `λ(Ω ∩ W)` changes only when the boundary
inside `W` changes (B2.1). ∎

*Validity.* The assignment is made on the starting map. It stays exact for every candidate
whose boundaries stay inside the band, that is within about a pixel of where they started
(the representation's trust region). Beyond it the whole-description evaluator is still
exact, but the edges it moves must be scored together. This replaces the earlier
formulation, "exact up to stated cross terms": there are none, only couplings.

### B4.2 The null space belongs to the parametrisation

*Claim.* The likelihood depends on a description only through its regions (B2.1), and on
its regions only through the window areas. A parametrisation is locally identifiable exactly
when `J = ∂A/∂θ` has full column rank; its flat directions are `ker J`.

*Examples.* A polynomial graph of degree `< n` on `n` windows: `J` is the histopolation
matrix, invertible (`poly_eq_of_averages`). A polyline with a vertex on every pixel border:
`ker J` is the zigzag (`polyline_kernel`). Free 2-D points: at least `2N − M` flat directions
(`flat_directions`). A segment with more parameters than the windows it spans. Any change of
description that draws the same regions (a stroke or two fills, one arc or three: the
program ambiguity of `optimal.md` §1) is exactly flat. So the representation checks
`rank J` (cheap, per segment) and never fits a segment over fewer windows than it has
parameters (`README.md` §6.4).

### B4.3 Precision

*Claim.* For an identifiable parametrisation, the weighted least-squares estimate has
covariance `F⁻¹ = (Jᵀ V⁻¹ J)⁻¹` to first order, exactly for families linear in their
parameters (Gauss-Markov, whatever the error distribution), and this is the Cramér-Rao bound
under Gaussian errors. Any coordinate or constraint `g(θ)` has `σ_g² = ∇gᵀ F⁻¹ ∇g`. In the
continuum form of B2.5:

* a line segment of length `L`: offset at its midpoint `1/√(ρL)`, angle `√(12/(ρL³))`;
* a circle of radius `R`: radius `1/√(2πRρ)`, centre `1/√(πRρ)` per axis;
* a vertex: B3.1; a junction: B3.2;
* absolute positions of edges near 0° or 45° under `n × n` point supersampling add the bias
  floor of B2.3 (up to `1/(2n)`, rms about `1/(n√12)`), which does not average down with
  length; angles are barely affected (the bias is constant along an exactly aligned edge
  and varies slowly along a nearly aligned one).

*Proof.* Gauss-Markov for linear families; linearisation otherwise; the closed forms are
`∫ρ N Nᵀ ds` for `N = (1, s)` on a segment and `N = (1, cos φ, sin φ)` on a circle. ∎

Black ink on white, `cos θ ≈ 0.9` on average (other contrasts: `ρ` scales with the squared
contrast where quantisation dominates):

| regime | `V` per window | `ρ` (px⁻³) | line, 10 px: offset / angle | line, 50 px | circle `R = 10`: radius | vertex, 90°, 12 px arms |
|---|---|---|---|---|---|---|
| exact area, 8-bit | 1.8e-6 | 6.1e5 | 0.0004 px / 0.008° | 0.0002 px / 0.0007° | 0.00016 px | 0.0015 px |
| 8 × 8 points, 8-bit | 1.6e-4 | 6.8e3 | 0.0038 px / 0.076° | 0.0017 px / 0.0068° | 0.0015 px | 0.014 px |
| 32 × 32 points, 8-bit | 4.4e-6 | 2.6e5 | 0.0006 px / 0.012° | 0.0003 px / 0.0011° | 0.00025 px | 0.0023 px |

What this means for the representation. At 128 px a 24-unit icon has 5.33 px per unit, so
even the worst case here (an axis-aligned edge's absolute position under 8 × 8 points,
about 0.04 px) is under 0.008 units, against a half-unit grid spacing of 2.67 px. Whether a
coordinate is on the design grid, whether a line is horizontal, whether two radii are equal
to a hundredth of a unit: the pixels settle all of these at every tier of the gate. What
they leave open is structure: fillets below the radius of B3.3, gentle corners below the
proposal threshold, hidden strokes, tangential junctions, and occluded parts. The same
sharpness has a cost. The likelihood separates curves that differ by a few hundredths of a
pixel, for instance a true circular arc of radius 200 px from its common four-cubic
approximation (radial error about `2.7·10⁻⁴ R`, 0.05 px, against a radius known to about
0.001 px). So the evaluator must draw a candidate the way the renderer draws it (renderers
approximate arcs by Bézier or conic pieces, each its own way), and the representation must
offer the encodings artists use.

*Digits.* Writing a coordinate with step `q` adds `q²/12` to its variance; the
representation can choose the coarsest `q` that keeps that small against `σ_g²`, and B
supplies `σ_g`.

### B4.4 Tests

*Claim.* (a) A true linear constraint of rank `k` raises the minimum `χ²` by a `χ²_k`
variable (`merge_chi2_increase`: the increase is the noise in the removed directions).
(b) The Wald statistic `W = g(θ̂)ᵀ (G F⁻¹ Gᵀ)⁻¹ g(θ̂)` equals that increase for linear
models and constraints, without refitting. (c) A false constraint whose best constrained fit
leaves a normal deviation `δ(s)` raises `χ²` by about `∫ ρ δ² ds`, which grows linearly
with the boundary length, while a true one does not: the test is consistent. (d) On the
boundary of the parameter space (a fillet radius, a stroke width) the null distribution is
`½χ²₀ + ½χ²₁`. (e) All of this holds only with calibrated variances: the self-calibration of
B2.3 is part of the interface, not an option.

*Proof.* (a) `Naturality.lean`; (b) the standard equivalence (Wald 1943) for linear
Gaussian models; (c) the expected residual of a misspecified projection; (d) Chernoff
(1954). ∎

## Interface proposal

### What the boundary chain delivers

One `Evidence` per image, built once (in about stage 07's time: `strip.rs`'s `line_mean`
already computes each window's sum):

1. **`map`** — the planar map as now: topology exact from the label map, starting geometry
   from stage 07 (and stage 08 while it remains).
2. **`inks`** — per face, the candidate ink models (flat, linear, radial), each with its
   parameter count, interior `χ²` and covariance, and B1's default.
3. **`runs`** — per edge, the ordered observations along it. A run window is
   `{axis: column | row, line, lo, hi, s, top_face, sum, var}`: the window spans pixels
   `lo..=hi` of grid line `line`, `s` is its arclength position along the starting edge, and
   `h = lo + sum` is the boundary's mean height over the line (Lean convention; minus ½ in
   inkvec's), known to `±√var`. Interleaved in order: per-pixel observations at corner
   candidates (two flat inks: `{x, y, face, weight, var}`; otherwise `{x, y, colour, cov,
   faces}`).
4. **`vertices`** — per junction node (degree ≥ 3, from the topology) and per corner
   candidate: incident edge ends, the per-pixel observations within about 2 px, a starting
   position with covariance (B3.1, B3.2), the proposal statistic `z = |D| / √(70V̂)` for
   corners, and any flat direction with its extent.
5. **`strips`** — thin stretches (edge pairs sharing windows), their per-pixel observations,
   and their flat intervals (B3.4).
6. **`noise`** — `σ_q`, the per-image floor `V̂` (B2.3), the bias bound for near-axis
   edges, and the calibration check (`χ²/M` of the starting map's smooth runs).

### The evaluator

A trait in `inkvec-core`, so that `inkvec-fit` can use it without depending on
`inkvec-trace`, implemented in `inkvec-trace`:

```rust
pub enum Piece {
    Line([Point; 2]),
    Quad([Point; 3]),
    Cubic([Point; 4]),
    Arc { c: Point, r: f64, a0: f64, a1: f64 },
}

pub trait BoundaryLikelihood {
    /// χ² of edge `e`'s observations `range` (run windows and the per-pixel terms between
    /// them) against a candidate curve, given as pieces in order, in px. Returns (χ², M).
    fn chi2_run(&self, e: EdgeId, range: Range<usize>, curve: &[Piece]) -> (f64, usize);
    /// The residuals `(S_W − A_W)/√V_W`, and their Jacobian by the pieces' coordinates.
    fn residuals_run(&self, e: EdgeId, range: Range<usize>, curve: &[Piece],
                     jac: Option<&mut Matrix>) -> Vec<f64>;
    /// Prefix moments for curves linear in their parameters as graphs over the window axis
    /// (lines, polynomials): the χ² of a least-squares fit over any range in O(p²).
    fn run_moments(&self, e: EdgeId, degree: usize) -> RunMoments;
    /// χ² of a vertex neighbourhood or a thin stretch, given every incident edge's candidate
    /// near it.
    fn chi2_local(&self, owner: Owner, curves: &[(EdgeId, &[Piece])]) -> (f64, usize);
    /// The whole description: visible partition and inks.
    fn chi2(&self, d: &Description) -> (f64, usize);
    /// Information density along an edge, and the floor, for precision without a Jacobian.
    fn density(&self, e: EdgeId) -> &[(f64, f64)];
    fn floor(&self) -> Floor;
}
```

| call | cost |
|---|---|
| building `Evidence` | one pass over the band, `O(boundary pixels)` |
| `chi2_run`, `residuals_run` | `O(windows in range + pieces)`, closed-form integrals |
| `run_moments` | `O(M p²)` per edge, then `O(p²)` per candidate range |
| `chi2_local` | `O(pixels × pieces near)`, a few dozen pixels |
| `chi2` | `O(all windows + all pieces)` |

**Semantics.** `χ²` is `−2 log L` up to a constant, calibrated so that the truth has
`E χ² = M`; every call returns `M` so the representation can apply "explains the pixels
within their noise" (B2.4) and the tests of B4.4 directly. Precision for any coordinate or
constraint the representation writes is `∇gᵀ F⁻¹ ∇g` with `F = JᵀV⁻¹J` from
`residuals_run`, or `∫ρ N Nᵀ ds` from `density` (B4.3).

**Integration, in steps.** (1) The fitter's residual per measured point, a perpendicular
distance over `σ`, becomes a residual per window, the measured mean height minus the
candidate's mean height over the window, over `√V`; its dynamic program indexes windows
instead of points, with breakpoints at window borders (which keeps the cost additive) and a
continuous polish of the breakpoints afterwards. (2) The vertex and strip terms enter a joint
polish of each vertex's incident end segments, where corners, continuation and tangency are
tested (B4.4). (3) Inks become candidates, and strokes a primitive the evaluator renders as a
band.

### Shortening the chains

* **S1. Corners and junctions become R2 decisions.** B3 supplies proposals at high recall,
  per-pixel terms and precision; the representation's own arms, fitted jointly with the
  vertex terms, place the vertex (B3.1 shows that is where the precision is), and a corner,
  a fillet or a smooth join is a nested test (B3.3, B4.4). This removes B3's wedge models,
  `SIDE_TOL` as a decision, and `refine_junctions`' tangent extrapolation as a final estimate.
* **S2. Continuation through a junction** (the stem of a T, two arms that are one curve) and
  tangency at a junction are representation constraints tested against the vertex terms;
  `optimal.md` §4 lists continuation as a missing move.
* **S3. Gradient or flat, and which gradient,** is the representation's choice among B1's
  candidates, priced by its prior.
* **S4. Stroke or two edges, and the ink of a sub-pixel stroke,** are the representation's,
  on B3.4's strip terms and flat intervals.
* **S5. Points stop being the interface.** Stage 08's point solve becomes initialisation at
  most: its kink and anchor priors stand in for data the point parametrisation cannot see
  (B4.2), and a curve parametrisation does not need them. `DEFAULT_SIGMA_MODEL` and the
  curvature inflation of per-point sigmas give way to calibrated window variances, which
  means the price of a parameter must be re-derived on the representation's side.
* **S6. The links are re-cut.** `chains.md`'s B2 ("a likelihood for any curve") and B4 ("the
  planar map as one likelihood") are one identity (B2.1); here B2 is the likelihood of any
  description, decomposition included, and B4 is what the pixels can confirm: precision and
  tests, the part of the interface the representation consumes most.

### Questions for the representation chain

1. **Residual form.** Can the fitter score a candidate segment by its window residuals
   (mean height over a column or row) instead of perpendicular point distances? Which
   segment kinds must the evaluator integrate exactly (line, arc, quadratic, cubic,
   primitives, stroke bands)?
2. **Additivity.** Is a dynamic program with breakpoints at window borders, followed by a
   continuous polish, acceptable, or do you need straddling windows handled inside the
   program (a residual that depends on two adjacent segments)?
3. **The price of a parameter.** With calibrated variances, a window's standard deviation
   is 0.002-0.013 px where `DEFAULT_SIGMA_MODEL` says 0.05 px, so `χ²` differences grow by a
   factor of 15 to 600 and `λ = ln(extent/precision)` no longer balances them. Will R2/R4 use
   the fidelity constraint `χ² ≤ M − p + z√(2(M − p))` with a prior over descriptions, or a
   refined description length with `½ log det F` per segment (B can return `log det F`)?
4. **Tests.** Wald from `F` (no refit, cheap) or `Δχ²` by refit? Which constraints span
   several edges' windows (concurrency, continuation, equal radii, symmetry), so that the
   evaluator must score them jointly?
5. **Corners.** Will R own corner, fillet and smooth-join decisions (S1)? What recall and
   false-proposal rate should the proposals target, and what starting tangents do you need?
6. **Strokes and hidden geometry.** Should the evaluator accept a stroke (centreline, width,
   caps, joins) directly? For a flat interval (a hidden stroke, an occluded part), do you want
   it as an interval, or the flat-bottomed likelihood itself?
7. **Inks.** Will R take gradient-or-flat (S3) on B1's candidates?
8. **Layers.** For a layered candidate, will you hand the evaluator its visible partition,
   or should it composite per layer as the renderer does (B3.2)?
9. **Units and the floor.** B reports precision in px; is the separate near-axis floor
   (B4.3) useful to you as a term in grid-snapping tests, or should it be folded into the
   variances?
10. **Primitives the likelihood can separate.** At 512 px a true arc and its four-cubic
    approximation differ by about fifty standard deviations (B4.3), and renderers draw arcs
    by approximations of their own. Should the evaluator draw each primitive exactly as the
    gate's renderer does, and should "circle" be offered in both encodings, or does the
    prior decide which the artist meant?

## Phase 2: formal, then code

### Lean (`formal/InkvecTheory`)

| file | lemma | for | difficulty |
|---|---|---|---|
| `Windows.lean` | `window_sum_eq_area` (finite sets of pixels, any measurable region) | B2.1 | easy |
| `Windows.lean` | `column_area_cavalieri` (cross-section form, any measurable region) | B2.1 | medium |
| `Information.lean` | `window_information_le` (Cauchy-Schwarz, equality condition) | B2.2 | easy |
| `Information.lean` | `column_information_ratio`: `(1 − t/2)/(1 − t/3) ≥ 3/4` from the chord split | B2.2 | easy |
| `Information.lean` | `ss_window_variance`: `1/(12n³)` under independent uniform sample-column errors | B2.3 | easy |
| `Unmixing.lean` | `unmix_unbiased`, `unmix_variance_le` (`(Δ²/12)(‖d‖₁/‖d‖₂²)²`) | B1.2 | easy |
| `AffineInk.lean` | centre value of an affine ink; the first-moment correction; `a(1 − a)/2` on an axis edge | B1.4 | easy |
| `ThinStroke.lean` | `straddle_noise`, `straddle_contrast_ambiguity` | B1.5, B3.4 | easy |
| `Vertex.lean` | `line_fit_variance` (offset `1/N`, slope `12/(N(N²−1))` on `N` columns), `intersection_variance`, `wedge_from_four_columns` | B3.1 | medium |
| `Vertex.lean` | `fillet_area_deficit` from Lebesgue measure | B3.3 | medium |
| `Corner.lean` | `one_sided_is_fourth_difference` (the rational identity), `fourth_difference_cubic`, `kink_response` (`5/4`) | B3.3 | easy |
| `Vectors.lean` | rational twins of the constants above, with `#eval` printing the test vectors the Rust tests read | ties | easy |

The general first variation (B2.5) and the Gauss-Markov theorem are stated and used, not
formalised in this round: Mathlib lacks the latter, and the former needs differentiation
under the integral of a Lipschitz, not smooth, integrand.

### Rust

* `crates/inkvec-core/src/likelihood.rs`: the `BoundaryLikelihood` trait, `Piece`,
  `Description`, `Floor`.
* `crates/inkvec-trace/src/evidence.rs` with `evidence/windows.rs` (the partition, built from
  `strip.rs`'s `line_mean`, which already finds each window), `evidence/area.rs` (exact
  window integrals of lines, Béziers and arcs, with clamping), `evidence/noise.rs` (the
  variance model and the fourth-difference self-calibration), `evidence/vertex.rs`
  (proposals, per-pixel neighbourhoods, flat directions), `evidence/fisher.rs` (Jacobians,
  `F`, density).
* `strip.rs`'s `SIDE_TOL` becomes the same test with a calibrated threshold,
  `z√(70V̂)/12`, and the vertices it declines become corner proposals.
* The fitter's side (the window residual in `inkvec-fit`'s dynamic program) is the
  representation chain's; B provides the evaluator and test fixtures.

**How Lean constrains Rust.** (1) Every rational constant (histopolation, the fourth
difference, `70`, `12`, `5/4`, `3/4`, the straddle weights) is checked by a `cargo test`
against `Vectors.lean`'s output, committed as a file. (2) Debug assertions of the
inequalities the proofs rely on: for every window and candidate, the faces' areas sum to the
window's size and each lies in `[0, |W|]` (B2.1); a window's information never exceeds its
pixels' (B2.2); the unmixing residual of a two-ink window is within its bound (B1.2); `F` is
positive definite where `rank J` is claimed full (B4.2). (3) Tests on exact renders: window
sums of a known polygon equal its exact areas to `1e-12` (the first row of B2.3's table).

### Measuring success

* **Calibration** (`bench/theory`): `χ²/M` at the truth within `1 ± 0.1` on exact, 8 × 8
  and 32 × 32 renders, and on the corpus's own intake (resvg) once its floor is estimated.
* **Efficiency:** the maximum-likelihood estimates of lines, circles, ellipses and polygons
  against `F⁻¹`: ratio of predicted to measured variance at least 0.8.
* **Vertices and corners:** vertex error against B3.1; corner precision and recall of the
  proposals and of the `Δχ²` decision against turning angle and fillet radius, compared with
  `SIDE_TOL`.
* **Thin strokes:** centre and width against B3.4's variances, and flat intervals reported
  where the stroke is hidden.
* **Stage 07** (`strip_eval.py`, `inkvec_compare.py`): the starting geometry, which only
  changes at gentle corners.
* **The gate:** next to no change in dE00 from the boundary chain alone (the starting map
  changes only where the corner test's threshold does); then, with the fitter scoring windows, the geometric match to the artist's file
  (edge displacement by symmetric-difference area; corner precision and recall) and the
  parameter ratio, against the base. Corners and junctions are where the gain should show:
  they hold about 5 % of the colour error (`attribution.py`) but most of the visible
  geometric disagreement, and they are where today's point readings decline.

## Phase 2, milestone 1: the floor measured, the evaluator built

What landed, against the amendments of `chain-representation.md` (A1–A5) and the
measurements the coordinator asked for. Engine output is unchanged: nothing in the
pipeline calls the evaluator yet.

### The renderer floor of the corpus intake (measured)

`bench/theory/renderer_floor.py` scores the intake against the artists' own geometry,
drawn exactly, on six flat-paint icons per family (24 to 30 k windows per tier). The unit is
a window's error in area, which is the error of the boundary's mean position across the
window, px. Every partial pixel lies in exactly one window, including at 45°.

| tier | family | quantisation | as drawn (`as8`) | per-edge share | straight fills, tiny-skia's lattice | curves, lattice | strokes, lattice | stale levels |
|---|---|---|---|---|---|---|---|---|
| 128 | lucide | 0.0011 | 0.0107 | 0.58 | 0.0017 | – | 0.0106 | 0 |
| 128 | material | 0.0008 | 0.0099 | 0.85 | 0.0016 | 0.0042 | – | 0 |
| 128 | noto | 0.0070 | 0.0083 | 0.23 | 0.0044 | 0.0076 | 0.0073 | 0 |
| 128 | openmoji | 0.0020 | 0.0077 | 0.46 | 0.0025 | 0.0094 | 0.0093 | 0 |
| 128 | simple-icons | 0.0014 | 0.0045 | 0.53 | 0.0015 | 0.0048 | – | 0 |
| 128 | twemoji | 0.0025 | 0.0053 | 0.48 | 0.0026 | 0.0048 | – | 0 |
| 512 | lucide | 0.0011 | 0.0240 (fresh 0.0170) | 0.44 | 0.0019 | – | 0.0207 | 24 |
| 512 | material | 0.0008 | 0.0188 (0.0109) | 0.86 | 0.0018 | 0.0094 | – | 11 |
| 512 | noto | 0.0063 | 0.0140 (0.0081) | 0.53 | 0.0048 | 0.0080 | 0.0076 | 16 |
| 512 | openmoji | 0.0022 | 0.0167 (0.0087) | 0.47 | 0.0050 | 0.0073 | 0.0099 | 22 |
| 512 | simple-icons | 0.0013 | 0.0122 (0.0092) | 0.58 | 0.0017 | 0.0113 | – | 9 |
| 512 | twemoji | 0.0024 | 0.0113 (0.0067) | 0.56 | 0.0024 | 0.0059 | – | 18 |

The columns:

* **quantisation:** the 8-bit rounding alone.
* **as drawn:** the intake against usvg's geometry composited per element at 8× with exact
  coverage, in brackets against a render made now.
* **per-edge share:** the part of the floor's variance that is a constant per edge.
* **lattice:** a fresh render against the same geometry sampled on tiny-skia's 4 × 4
  lattice.
* **stale levels:** the largest difference between the committed intake and a fresh
  resvg 0.48.1 render.

What this says:

1. **A1, arcs.** Drawing arcs as usvg does (kurbo cubics at 0.1 user units, ported to
   `EllipticalArc::to_cubics`) matters where icons are arcs: on simple-icons the window error
   falls from 0.0204 to 0.0122 at 512 (0.0062 to 0.0045 at 128). Elsewhere it is below the
   floor.
2. **A1, the lattice.** With tiny-skia's sample lattice replicated, straight fill edges reach
   the quantisation level (0.0015 to 0.0026 px at 128, 0.0017 to 0.0050 at 512; material 12mp
   per pixel 0.0106 to 0.0014). The rest is tiny-skia's own curve flattening (0.004 to 0.011
   px) and its stroker (0.008 to 0.021 px; round caps up to 0.06 per pixel). These are
   deterministic: a forward model that calls tiny-skia removes them, while a floor can only
   absorb them.
3. **A3, the correlation.** Most of the floor is a constant per edge (0.44 to 0.86 of its
   variance). The fourth-difference self-calibration sees 0 to 22 % of it, as it should,
   since a per-edge offset cancels in `D`. On material at 128 the per-edge sd is
   0.009 ≈ 1/(32√12), which is `Floor::lattice(32)`'s `edge_var`. The default floor is
   therefore right for straight edges and too small for curves and strokes until (2) is done.
4. **The 512ss tier is stale.** The committed 512ss PNGs differ from a fresh render by up to
   24 levels: an older resvg, or older SVGs. The 128ss tier reproduces byte for byte.
   Calibrating at 512 needs that tier re-rendered (the coordinator's call: it moves every
   512 baseline).
5. **A2, conflation.** Compositing each element at 8× and then box-filtering is not the
   visible partition. At junction pixels the difference is mean 0.004 to 0.006, p99 0.017 to
   0.040 (colour units), which is many times the quantisation, so junction terms need
   per-layer compositing. At two-ink pixels it is negligible, except where shapes abut
   (twemoji: 1.3 % of boundary pixels over one level; the synthetic set: 60 %, window error
   0.067 px along such seams).

### What the evaluator is (`inkvec_core::likelihood`, `inkvec_trace::evidence`)

* **The trait `BoundaryLikelihood`** (in `inkvec-core`, so `inkvec-fit` needs no
  `inkvec-trace`), with these methods:
  * `runs(e)` gives the `RunObs {window, s, sum, var, left_low}` of edge `e` in order.
  * `chi2_run(e, range, pieces)` gives `Chi2 {chi2, m, chi2_floor}`. Its `chi2_floor` is the
    Sherman–Morrison form with the per-edge offset, on edges `edge_on_lattice` says share it.
  * `residuals_run`.
  * `run_moments(e, degree)` gives prefix sums for least-squares polynomial graphs over any
    range in `O(p³)`.
  * `chi2_local(Owner::Junction | Owner::Corner, curves)` is the per-pixel term.
  * `junctions()` gives arms in angular order with direction sd, and continuations (A4).
  * `corners()` and `density(e)`.

  Candidates are `Piece::{Line, Quad, Cubic, Arc}`. The `RenderModel` converts arcs exactly
  as usvg does. `left_area` integrates each piece exactly over the strip, clamped, and counts
  every pass of the curve through the strip, so a curve that folds or crosses a window twice
  is scored correctly (B2.1's identity for any curve, not only graphs).
* **`Evidence::build(map, rgb, faces, σ, opts)`.** For each edge between two flat inks with
  enough contrast, it unmixes the pixels onto the two-ink axis. Quantisation variance is
  grouped by the channels that round together. Pixels holding a third ink are set aside.
  The partial pixels are then grouped into windows. A window the starting geometry does not
  cross exactly once becomes a **corner term** (per-pixel areas against the candidate,
  closed along a box on its left), with a proposal there. Corners along runs come from the
  fourth difference at `z ≥ 3`. Junction neighbourhoods (2.5 px) are kept per pixel, in
  colour. Pixels two edges reach away from a junction are listed, not yet scored (B3.4).

Decisions taken while building it:

* **No pure flanks.** A pixel that reads as one ink's pure colour can hold up to half a
  level of the other's area and reads none of it. That error has one sign, so flanks put
  `χ²/M` at 1.85. A window is its partial pixels only; the clamp in `left_area` makes that
  exact.
* **Inks on the 8-bit lattice.** The face colours unmixing uses must be the pure pixels'
  8-bit values. Inks a fraction of a level off bias every window by up to 3σ.
* **Calibration.** At the truth, on exact renders rounded to 8 bits, `χ²/M` is 0.97 over 442
  windows (discs and squares at several angles, 45° included). Junction and corner terms
  stay below 2. A vertex moved by 1 px, or a 1 px chamfer, costs more than 100.

### Verified by checkers generated from Lean

`formal/InkvecTheory/InkvecTheory/Windows.lean` proves the window identity for any finite
set of pixels and any measurable region (`window_sum_eq_area`), and the area of a straight
piece (`trapezoid_integral`). `Gen/Evidence.lean` defines four kernels, each with its
theorem; `#print axioms` gives `propext, Classical.choice, Quot.sound` only:

* `trapezoidK`: `trapezoidK_eq`, `trapezoidK_integral`.
* `windowTermK`: `(S − A)²/V` and `S − A`.
* `fourthDiffK`: `fourthDiff_cubic` says it vanishes on cubics; `fourthDiff_side` relates
  it to `strip.rs`'s one-sided test.
* `cornerExcessK`: `D² − z² Σ wᵢ² Vᵢ`.

The generated `inkvec_verified::generated::evidence` supplies `_f64`, `_iv` and `_q` forms
of each kernel. Two results are accepted only through them:

* **Runs.** Every run window must be certified crossed by the starting geometry, with its
  area enclosed (`checks::certified_left_area`, outward-rounded intervals). A window the
  checker refuses becomes a corner term.
* **Corner proposals.** Each must have `corner_excess_iv ≥ 0`.

`checks::certify_run_chi2_polyline` certifies a solver's `χ²` of a polyline candidate on any
set of windows.

### Next (as decided after milestone 1)

The engine's forward model stays renderer-agnostic: exact geometry and a floor calibrated per
image (`RenderModel::default()` draws arcs exactly; `RenderModel::usvg()` keeps usvg's cubics
for measuring a known renderer). tiny-skia is replicated only in bench tooling. The 512ss
tier stays as committed: its renderer offsets are realistic variation too.

## Phase 2, milestone 2: noise from the edges

The engine's intake model was 8-bit rounding and one noise level per image, read from flat
regions. On a resized JPEG (the gate's `web` tier, `docs/theory/noise.md`) the error sits at
the edges, 5 to 9 times the interiors', and has heavy tails. The truth (each icon's exact
geometry at 400 px) scored on the engine's own windows read `χ²/M` = 150 there. This
milestone measures the noise per image from the edges, gives the windows a forward model with
the blur in it, and hands the result to the representation chain as one object
(`inkvec_core::noise::NoiseModel`).

### What a window's error is made of (measured)

`bench/theory/floor_selfcal.py` runs the engine on five renders of 21 icons (3 per family)
and on the `web` tier: an exact render, 8 × 8 point samples (`ss8`), tiny-skia's 32-sample
lattice with exact flattening (`lat32`), resvg 0.48.1 now and the committed tier (identical
at 128 px), and `web`. Each run window is scored against the truth's sum on the same pixels
(`crates/inkvec-trace/examples/evidence_calibrate.rs`). A window's error is a function of the
edge's local sub-pixel phase and of what lies near it, and splits three ways:

* **Replicas.** Neighbouring windows whose mean positions differ by a whole pixel see the
  edge at the same phase and repeat one measurement with its error. The correlation between
  such neighbours is 0.84 to 0.95 on every input. On an axis-aligned edge every window is a
  replica: on `web` 81 % of the fourth differences of one material icon are exactly zero.
  Every source is replicated, including the JPEG's: a block holding a horizontal edge has
  only vertical frequencies.
* **The rough part.** Errors at different phases are independent on exact, lat32 and ss8
  renders. They anticorrelate at phase steps under 0.05 px (rounding's sawtooth).
* **The smooth part.** On resvg and `web` renders a correlation of about 0.6 remains between
  neighbours at every phase step. A smooth error along the run (curve flattening, arc
  conversion, a resampler's phase ripple) is indistinguishable from moving the edge.

### The estimator (`evidence/noise.rs`)

* **Rounding alone as the base variance.** It is exact for clean renders (`χ²/M` 0.97 at the
  truth). It is alpha-aware: where the source drew an ink over the clear ground, the intake
  rounds the alpha, and the weight's variance is `Q²/12` whatever the ink's contrast.
* **Replica stretches count once.** A stretch of `k` replicas gets `k` times the variance and
  a share `1/k` (`RunObs::share`; `Chi2::dof = Σ share`).
* **The rough part from the windows themselves.** It is solved per class (straight, curved)
  from the fourth differences of replica-free stencils. A winsorised second moment
  (`E min(z², 6.25)` = 0.9776 for a normal) is exact for normal errors and within 2 % for
  rounding's bounded ones. A median is 1.65 times off there: that is what first put exact
  renders at 0.55.
* **Geometry kept out of the estimate.** A stencil counts only if its local bend (its largest
  second difference of positions) is under the larger of 0.08 px and six noise standard
  deviations, iterated from all stencils. Below 0.08 the `D`s of the measured positions and
  of the truth's errors agree on every input; above it a tight curve's own fourth
  difference leaks in (6 times the noise on "straight" and 60 on curved windows of exact
  renders at 128 px).
* **A significance gate.** A class's extra is kept only when it beats a normal's moment by
  three standard errors.
* **The smooth part as one offset per edge.** It has the variance of one of the edge's
  windows (`edge_offset_var`, read by `chi2_floor`). An axis-aligned edge's constant error
  *is* one window's error, replicated. A fitted candidate absorbs it in its own position.
* **The tail.** It is read from the `q90/q50` ratio of the standardised `D`s and matched to
  Student-t's. It is then diluted back to one window's: a `D` keeps 0.37 of its windows'
  excess kurtosis.
* **The robust cost.** Huber's at `κ(ν) = min(3, √ν)` (agreed with the representation chain:
  where Student-t's influence peaks), on the Kalman-standardised innovations (`Chi2::cost`).
  It equals `χ²` while every innovation is within `κ`: always on a clean intake's truth.
* **A point-sampling renderer's lattice.** It is read from the image. On an axis-aligned edge
  such a renderer's coverage is a multiple of `1/n`, so the smallest `n` that 90 % of the
  replica stretches' partial pixels sit on (and a random value would not) is the lattice.
  Its sawtooth `1/(12n²)` is added to replica windows. Coverages of one half are left out:
  drawings put edges on half pixels by design.

### Lossy and soft intakes: windows on the observed ramp

* **Luma unmixing.** With 4:2:0 chroma, weights are read from luma
  (`α = (Y − Y_b)/(Y_a − Y_b)`, BT.601), the channel kept at full resolution. This applies
  where the inks' lumas differ by at least 0.1 (three times the edge noise); a nearly
  isoluminant pair falls back to full colour. The third-ink test allows 26 levels of chroma
  bleed.
* **Plateau-median inks.** Each edge's inks are the per-channel medians of the pixels just
  beyond its reach on either side, not the palette's means. On a clean render these are the
  exact inks.
* **The reach.** It widens to cover the observed ramp: `0.5·w + 1.1` px for the
  box-equivalent edge width `w` of `softness::ramp_evidence`.
* **Band windows.** A window takes every pixel of the edge's band on its line, not only
  those reading as mixtures: noise splits a blurred ramp's run of partial pixels, and each
  fragment would lose mass to the next.
* **Column or row is decided along the edge.** The choice uses the direction smoothed over
  ±2 px of arclength with a hysteresis of 0.15 rad about 45°. Decided per pixel from a noisy
  polyline's local tangent, diagonals were cut into alternating one- and two-pixel windows
  that erred in opposite directions (+0.085 and −0.12 px side by side).

### Blur: the forward model

For a blur or resampling `w(y, s)` from input pixels `s` to output pixels `y`, a column
window `W` over the blurred transition sums to

    Σ_{y∈W} (f ∗ k)(x, y) = (A ∗ k_x)(x) + Δ · m_y,

with three conditions and limits:

* **Hypothesis: `Σ_{y∈W} w(y, s) = 1` for every input `s` it draws on.** `A` is the column-area
  profile, `k_x` the horizontal marginal, `Δ` the plateau step and `m_y` the vertical first
  moment (`docs/theory/noise.md`). The mass-preservation step is `window_sum_resampled`
  (`Windows.lean`).
* **Box filters and general resamplers.** An integer-ratio box filter satisfies the
  hypothesis. A general resampler (Pillow normalises per output pixel) satisfies it only on
  average over phase, and its phase ripple, 0.04 px at a factor of 0.78, is the measured
  residual. On `web` that ripple is part of the windows' variance.
* **A centred kernel (`m_y = 0`).** It leaves a straight edge's profile alone
  (`blur_affine_profile`) and moves a curved one by exactly `½ μ₂ A''`
  (`blur_quadratic_profile`). That is the forward model, not noise.
* **The translation.** The first moment, a global sub-pixel translation, cannot be identified
  from one image: translating the drawing reproduces it exactly. The coordinator measured it
  at 0.004 px, so it is left out.

`μ₂` comes from `ramp_evidence`'s width: its spread is `(3w² − 1)/12` with a native mean of
1/6, so `μ₂ = (w² − 1)/4`, not `/12`. `RenderModel::window_area` adds `½ μ₂ A''`, with `A''`
the second difference of the window's area profile (the candidate moved a pixel either way
across the strip), only when `μ₂ > 0`.

On damaged input the target is the artist's clean original: the boundary reported is the
sharp one before the blur, and a corner lost to blur comes back when its arms support it.
This model makes that possible. The per-pixel corner and junction terms still compare
unblurred candidates. Blurring them is the next step, and the soft-intake reduce step could
then be replaced by this forward model (to be gated before acting).

### In the engine

The colour path already decides whether an intake is soft: its edges wider than a native
render's, ringing (`coverage::ringing_score`), or a lossy container. That is evidence from
the pixels, not the flag alone. Only then does `evidence::measure_noise` run, which costs 13
to 49 ms at 400 px, after the crossing test was indexed by arclength.

Its measurement goes to `ColorTrace::noise` whenever the edge noise exceeds `FOLD_LEVEL`
(3 levels). Clean renders read at most about 1.3 levels and `web` 4 to 27. Under it, and on
every clean intake, the model is `NoiseModel::clean()` and nothing changes: `quality-128ss`
is identical on the gate.

`NoiseModel` (agreed with the representation chain) holds:
`{sigma_flat, sigma_edge[4 bands], psf_radius, psf_mu2, nu, lossy, window_scale}`. It also
provides `huber_kappa` and `explained_by_psf` (a band narrower than the blur, coloured on the
line through its two inks or overshooting them).

Folding the noise into the planar map's per-point `σ` is opt-in (`INKVEC_NOISE_FOLD=1`). Each
point's `σ` then grows by the windows' measured error in quadrature, which is what the fitter
divides by. With today's fitter, honest uncertainties buy simpler fits at a cost in fidelity
(`quality-web`, a 62-icon sample: turning −5.0 %, parameters −1.4 %, dE00 +2.8 %, geom
+1.7 %). So the fold waits for the representation chain's fitter to read it with its robust
loss.

### `strip.rs`: a statistical side test

The one-sided corner test is the fourth difference of six column means over 12
(`fourthDiff_side`), so its noise is `√(70 V)/12`. A vertex is declined only beyond
`max(SIDE_TOL, 3·√(70 V)/12)`, with `V` from the trace's own per-channel noise and the widest
window. On a clean render that term is about 0.007 px, below `SIDE_TOL` (0.05), and the
reading is unchanged bit for bit. On a noisy intake the noise sets the threshold. The
certificate (`CERT_TOL`, the shift lands on the cubic) is unchanged.

### The adapter (`EdgeScorer`, agreed with the representation chain)

* **The partition.** `windows_between_points(i, j)` gives the run windows whose centres lie
  in the span `[i, j)` of the edge's point indices, as two ranges through the seam of a
  closed edge. Every window belongs to exactly one span whatever the segmentation, so the
  number scored is constant. Corner and junction terms are fixed per edge.
* **Scoring.** `chi2(r, pieces)` scores a span's piece, extended past its ends, on all of its
  windows.
* **The O(1) queries.** `best_graph(r, degree)` and `weight_moments(r)` run in `O(1)` from
  prefix sums, for the dynamic program's inner loop and the Fisher matrix of a graph of
  degree at most 3.
* **Arcs.** `Piece::from_svg_arc` converts the fitter's endpoint arcs to centre form.

### A2: stroke bands and per-layer compositing

* **`chi2_band(pair, band, inks)`.** It scores a `StrokeBand`, a centreline and a width, on
  the thin-feature pixels its two edges both reach. These pixels were only listed until now.
  Each pixel is a mix of the ink left of the band, the band's and the ink right of it, by
  their exact areas. On a 1.4 px band at 20° the true band is calibrated, while a 0.3 px
  shift or a band 0.3 px too wide costs more than 100.
* **`chi2_local_layers(node, layers, ground)`.** It scores a junction's pixels by compositing
  each layer's whole shape over what lies beneath, in paint order, as a renderer does. That
  is the term the visible partition misses at junctions (mean 0.004 to 0.006, p99 up to
  0.04, `renderer_floor.py`). On two overlapping squares drawn that way the true shapes are
  calibrated, and the top one moved by half a pixel costs more than 100.
* **Arcs in local terms.** The local terms' flattening follows arcs through fine cubics. With
  the default render model drawing arcs exactly, it had been dropping them.

### Validation

The engine's windows, scored against the truth with the evaluator's own `score` (replicas,
the per-edge offset, Huber at `κ(ν)`): `χ²_floor` per independent measurement (Huber in
brackets). 21 icons, 3 per family.

| input | rounding alone | per-image noise model |
|---|---|---|
| exact, 128 px | 0.97 | 0.72 (0.72) |
| 8 × 8 point samples, 128 px | 334 | 3.02 (2.07) |
| tiny-skia lattice, 128 px | 14 | 1.79 (1.57) |
| resvg (fresh = committed), 128 px | 32 | 4.33 (3.34) |
| resvg fresh, 512 px | 119 | 13.9 (7.4) |
| committed 512ss | 206 | 9.9 (6.5) |
| `web` (400 px JPEG) | 150 | 3.12 (1.79) |

On `web` the median standardised residual is 1.02. Against a local cubic fitted over 9
windows (what a fitter sees) it is 1.37, and 1.05 under Huber. Most families sit at 1.2 to
1.7 under Huber; openmoji is the outlier at 3.4.

What remains:

* **resvg's smooth curve error.** Flattening chords and usvg's arc cubics leave about 0.01 px
  along curves, smooth enough that no difference sees it (simple-icons, all arcs, is the
  worst). It is the price of an exact-geometry model that does not replicate a renderer, and
  a fitted candidate absorbs it in its own geometry.
* **A point-sampled render's sawtooth.** On near-axis edges that are not exact replicas it is
  only partly modelled.
* **Exact renders are slightly over-calibrated** (0.72): the extra variance is accepted on
  significance, and a small geometry leak gets through.


```bash
python3 bench/theory/evidence_eval.py all       # B1.2, B2.2, B2.3, B3.1, B3.3 (about 10 s)
python3 bench/theory/exact_raster.py            # the renderer's self-check
python3 bench/theory/renderer_floor.py --fresh  # the intake's floor (about 25 min)
cargo build --release -p inkvec-trace --example evidence_calibrate
python3 bench/theory/floor_selfcal.py --sizes 128 --per-family 3              # five renders
python3 bench/theory/floor_selfcal.py --sizes 400 --conditions web --per-family 3
cargo test -p inkvec-core likelihood noise
cargo test -p inkvec-trace --lib evidence
```

## References

* Chernoff, H. (1954). On the distribution of the likelihood ratio. *Annals of Mathematical
  Statistics* 25(3).
* Gasser, T., Sroka, L., Jennen-Steinmetz, C. (1986). Residual variance and residual pattern
  in nonlinear regression. *Biometrika* 73(3).
* Hall, P., Kay, J. W., Titterington, D. M. (1990). Asymptotically optimal difference-based
  estimation of variance in nonparametric regression. *Biometrika* 77(3).
* Havelock, D. I. (1989). Geometric precision in noise-free digital images. *IEEE TPAMI*
  11(10).
* Kay, S. M. (1993). *Fundamentals of Statistical Signal Processing: Estimation Theory.*
  Prentice Hall. (Cramér-Rao bound, least squares.)
* Porter, T., Duff, T. (1984). Compositing digital images. *SIGGRAPH '84*.
  doi:10.1145/800031.808606.
* Puckett, E. G. (2010). A volume-of-fluid interface reconstruction algorithm that is
  second-order accurate in the max norm. *CAMCoS* 5(1).
* Rice, J. (1984). Bandwidth choice for nonparametric regression. *Annals of Statistics*
  12(4).
* Self, S. G., Liang, K.-Y. (1987). Asymptotic properties of maximum likelihood estimators
  and likelihood ratio tests under nonstandard conditions. *JASA* 82(398).
* Trujillo-Pino, A., Krissian, K., Alemán-Flores, M., Santana-Cedrés, D. (2013). Accurate
  subpixel edge location based on partial area effect. *Image and Vision Computing* 31(1).
  doi:10.1016/j.imavis.2012.10.005.
* Wald, A. (1943). Tests of statistical hypotheses concerning several parameters when the
  number of observations is large. *Transactions of the AMS* 54(3).
