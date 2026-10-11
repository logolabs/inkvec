# Theory: inverting box-filter rasterisation

> What an anti-aliased raster does and does not determine about the shapes drawn into it,
> proved in Lean, compared against the literature, and turned into a measured improvement of
> inkvec's sub-pixel stage.

**Formal development:** [`formal/InkvecTheory`](../../formal/InkvecTheory/README.md)
(Lean 4.34.1, Mathlib; no `sorry`, no axiom of its own, 42 headline theorems audited).
**Experiments:** [`bench/theory`](../../bench/theory/README.md).
**Next:** [the optimal vectorizer, informally](optimal.md): what the best possible tracer
computes, how far inkvec is from it, and which approximations to make.
**Noisy inputs:** [what resampling and JPEG do to the evidence](noise.md): where the noise
sits, why area windows stay unbiased under it, and what the engine must change.
**Code it changed:** `crates/inkvec-trace/src/planar/strip.rs` (new) and `probe_chord`
(moved to `crates/inkvec-trace/src/planar/chord.rs`).

## Summary

Inkvec's founding claim is that an anti-aliased pixel is a *measurement*, the area of the
shape inside the pixel, not noise to threshold (`coverage.rs`). Reversing rasterisation is
therefore inverting a box filter. This page:

1. surveys the mathematics behind that inversion and its formalisation status (none existed);
2. states what this repository now proves about it, in Lean, from Lebesgue measure up;
3. uses the theorems to diagnose inkvec's sub-pixel stage (stage 07) and to replace its
   per-vertex reading, wherever it applies, by an exact one;
4. reports the measured effect: on the 246-icon regression gate, Fast mode's colour error
   falls **8.7–11.0 %** in every condition and Quality's **1.4–4.5 %**, with no axis worse
   than its margin.

The theory is the box-filter model. It does not, and cannot, prove inkvec is "the best
tracer": that is an empirical claim about real inputs and a chosen metric. It proves which
readings of the pixels are exact, which are biased and by exactly how much, what no
estimator can recover, and that the code built on these theorems computes what they say.

## 1. The problem

A pixel `(i, j)` covers the unit square `[i, i+1] × [j, j+1]`. An exact-area renderer
(libart, FreeType's smooth rasteriser, font-rs, Skia's analytic AA, and inkvec's own forward
model in `boundary_opt/band.rs`) writes into it

```text
P = B + a·(F − B),     a = area(shape ∩ pixel)
```

for a shape of ink `F` on ground `B`. Inkvec unmixes `a` from `P` (`coverage.rs`), so the
question is what the field of `a` values determines about the shape's boundary, and how to
read it. Three classical answers dominate practice:

* **threshold** `a ≥ ½` and trace the bitmap (Potrace; colour quantisers such as VTracer);
* read the boundary as the **½-level set of the interpolated coverage** (marching squares
  on `a`; PolyFit's "isoline at the mean colour"; inkvec's `contour.rs` and its
  `root_find_half` fallback);
* **invert the coverage of a straight edge per pixel** along an estimated normal (inkvec's
  `planar::edge_offset`, stage 07).

## 2. The literature

A survey of the mathematics behind reversing rasterisation, with what each line of work
proves. Citations were checked against the papers or their authors' code; where only
metadata or a secondary source could be reached, that is said.

**Exact-area (box-filter) rasterisation, the forward model.** Catmull (SIGGRAPH 1978)
computes exact area-weighted pixel colour by per-pixel polygon clipping. Duff (RIDT 1989,
*Polygon scan conversion by exact convolution*) and the signed-area accumulation of libart,
FreeType's `ftgrays.c` and Levien's font-rs make it a scan converter. Manson & Schaefer
(*Wavelet Rasterization*, CGF 30(2), 2011) compute box-filtered coverage of polygons and
quadratic/cubic Béziers exactly through boundary integrals. Auzinger, Guthe & Jeschke
(CGF 31(2), 2012) do the same for radially symmetric polynomial filters. DiffVG (Li et al.,
TOG 39(6), 2020) samples a box filter by Monte Carlo and differentiates it with Reynolds
transport. None of these inverts the filter.

**Volume-of-fluid interface reconstruction: the closest rigorous prior work.** In VOF the
cell volume fractions *are* box-filter coverage. Pilliod & Puckett (*JCP* 199(2), 2004)
build reconstructions (LVIRA, ELVIRA) from 3x3 column sums that reproduce every straight
interface exactly. Puckett (*CAMCoS* 5(1), 2010, eq. 11) states the identity this page calls
the column-sum theorem: when an interface `y = g(x)` enters and leaves a column through its
vertical sides, the column's volume fractions sum to `∫ (g − y₀) dx`. He proves second-order
max-norm convergence of a piecewise-linear reconstruction for `C²` interfaces under a
curvature condition.

**Sub-pixel edge location by the partial area effect.** Trujillo-Pino, Krissian,
Alemán-Flores & Santana-Cedrés (*Image and Vision Computing* 31(1), 2013) use the same
identity in a 3x5 (up to 3x9) window to recover a quadratic edge `y = a + bx + cx²` from
three column sums: `c = (S_L + S_R − 2S_M)/(2(A−B))`, `b = (S_R − S_L)/(2(A−B))`, and
`a = (2S_M − 5(A+B))/(2(A−B)) − c/12` (confirmed in the authors' slides and MATLAB code;
the paper itself was behind a paywall). The `−c/12` term is the finite-volume cell-average
correction below with `h = 1`.

**Finite-volume reconstruction.** A polynomial of degree `< n` is determined by its means on
`n` cells (Shu, ICASE 97-65, 1997, eq. 2.9; histopolation goes back to Schoenberg, *Splines
and histograms*, 1973). The point value from cell averages,
`u(xᵢ) = ūᵢ − (ūᵢ₊₁ − 2ūᵢ + ūᵢ₋₁)/24 + O(h⁴)`, is McCorquodale & Colella (*CAMCoS* 6(1),
2011, eqs. 12–13). Harten's subcell resolution (*JCP* 83(1), 1989) and Siddiqi, Kimia & Shu's
GENO (*CVGIP:GMIP* 59(5), 1997) locate a discontinuity inside a cell by preserving its
average: 1-D box-coverage inversion.

**Finite rate of innovation and shapes from samples.** Dragotti, Vetterli & Blu (*IEEE TSP*
55(5), 2007) recover Diracs and piecewise polynomials from samples taken with kernels that
reproduce polynomials (the box kernel is the degree-0 case). Shukla & Dragotti (*IEEE TSP*
55(7), 2007) recover polygons with rational edge slopes from B-spline (including box)
samples. Pan, Blu & Dragotti (*IEEE TSP* 62(2), 2014) recover curves that are zero sets of
trigonometric polynomials from low-pass samples; a box kernel is not covered by their
exactness theory. Milanfar, Verghese, Karl & Willsky (*IEEE TSP* 43(2), 1995) and Golub,
Milanfar & Varah (*SISC* 21(4), 1999) recover polygon vertices from complex moments. Fatemi,
Amini, Baboulaz & Vetterli (*Shapes from Pixels*, *IEEE TIP* 25(3), 2016) recover the
minimum-perimeter shape consistent with pixel samples via a TV relaxation, exact under a
"reducibility" condition that is combinatorial to check. They show the recovered shape is
the *minimum-perimeter* consistent one, not that it is the true shape.

**Precision limits.** Kiryati & Bruckstein (*CVGIP:GMIP* 53(1), 1991) prove error-free
reconstruction of straight-edged silhouettes from unquantised grey levels (for a disc-shaped
sensor, not a box) and a lower bound for bilevel sampling. Havelock (*IEEE TPAMI* 11(10),
1989) defines *locales*, the sets of object positions giving the same digital image, and
bounds the best possible position error of any estimator. Dorst & Smeulders (*IEEE TPAMI*
6(4), 1984) characterise the continuous segments consistent with a digital straight line.
Kakarala & Hero (*IEEE TPAMI* 14(7), 1992) give Cramér–Rao bounds for edge localisation in
the continuous domain, before sampling. Classic moment operators (Tabatabai & Mitchell 1984;
Lyvers et al. 1989; Ghosal & Mehrotra 1993) reach about 1/20 px with lookup-table bias
corrections.

**Vectorisers.** Yang et al. (*TVCG* 22(2), 2016) optimise Bézigons against a box-filtered
(wavelet) rasterisation: analysis by synthesis, as in inkvec's boundary solve, not a
closed-form inverse. Hoshyari et al. (TOG 2018) and Kopf & Lischinski (TOG 2011) do not model
anti-aliasing; PolyFit (Dominici et al., TOG 2020) reads anti-aliased input at the isoline of
the mean colour, the ½-crossing.

**Formalisation status.** No formalisation of rasterisation coverage, anti-aliasing,
area-sampling inversion or sub-pixel edge recovery was found in Lean, Coq/Rocq, Isabelle or
HOL Light. The nearest formal work is Pick's theorem (HOL Light: Harrison 2011; Isabelle AFP:
Binder & Kosaian 2024), Green's theorem (Isabelle AFP: Abdulaziz & Paulson 2018), a Why3
proof of Bresenham's line algorithm (Filliâtre), and μSkia (Kulkarni, Whiting & Panchekha,
arXiv 2603.23696, 2026), a Lean semantics of Skia that assumes away anti-aliasing. Mathlib has
a trapezoidal-rule error bound but no midpoint rule, shoelace formula or polygon area.

## 3. What is proved here

All of it in [`formal/InkvecTheory`](../../formal/InkvecTheory/README.md), each theorem
named in parentheses. Coverage is *defined* as the Lebesgue area of `Ω ∩ pixel` in `ℝ²`.
Nothing about it is assumed.

### 3.1 What a column of pixels measures

**Column sums are boundary means** (`column_sum_eq_average`). If a boundary `y = f(x)`,
`f` continuous, stays within rows `j₀ … j₀+m` over column `i`, then

```text
Σ_{r<m} coverage(i, j₀+r) = ∫ᵢ^{i+1} f(x) dx − j₀.
```

Puckett's identity, here derived from 2-D Lebesgue measure for every continuous graph and
any number of rows. No slope, curvature, normal or threshold enters.

**The exact fibre** (`coverage_layer_cake`, `column_fiber`). Each pixel reads
`L(j) − L(j+1)` with `L(t) = ∫ (f − t)₊`, so a column's coverages determine, and are
determined by, `L` at the integer heights. Two boundaries that rearrange the same heights
inside a column are indistinguishable. This is the exact null space of the box filter on
graph boundaries; the literature has its binary analogues (locales, Dorst–Smeulders domains)
and the zero-mean special case implied by Puckett's identity.

### 3.2 What cannot be recovered

**Free-form boundaries are not determined** (`invisible_perturbation`,
`sine_ripple_invisible`, `rasterization_not_injective`). A ripple `ε sin(2πkx)` of any whole
frequency on an edge inside one pixel row changes no pixel anywhere. The invisible
perturbations span an infinite-dimensional space.

**The sawtooth, exactly** (`polyline_zigzag_invisible`, `polyline_kernel`). For a polyline
with one vertex on every pixel border, the parametrisation `planar::build` produces and the
boundary solve moves, the vertex zigzag `δ(−1)ᵏ` changes no pixel. Inside one pixel row it is
the **only** invisible deformation of the vertex heights: the null space is exactly
one-dimensional. This is the "row of triangular teeth that render almost as well as a
straight edge" of [08-boundary-solve.md](../algorithm/08-boundary-solve.md); they render
*exactly* as well. With two coordinates per point, rank–nullity leaves at least `2N − M` flat
directions for `N` points and `M` pixels (`flat_directions`).

**Quantising first costs a fixed price** (`binarize_minimax`, `quantize_minimax`). Any
estimator of an edge's position that reads only thresholded pixels is off by at least
`½ − η` px on some edge, for every `η > 0`. Through *any* `L`-level per-pixel quantiser it is
off by `δ/2` for every `δ < 1/L`. The bound binds every tracer that binarises or
colour-quantises before it finds boundaries, whatever it does afterwards.

**Sub-pixel strokes** (`hidden_stroke_invisible`, `width_contrast_ambiguity`). A stroke of
width `w < 1` inside one pixel row has every position in a `1 − w` interval give the same
pixels, and its pixels fix only width × contrast (`coverage.rs`: "a 0.3px black stroke and a
1px grey stroke produce the same pixels").

### 3.3 What can be recovered, exactly

**Polynomial boundaries are determined** (`zeros_of_zero_averages`, `poly_eq_of_averages`,
`polynomial_boundary_determined`). A boundary of degree `< n` is fixed by its pixels on `n`
columns. A line needs two (`line_from_two_columns` gives the closed form), a cubic graph four.
This is the other side of §3.2: the null space vanishes once the boundary is drawn from a
model with no more parameters than the columns it covers. That is what a model-selecting
fitter exploits, and what free-point optimisation lacks.

**Points from pixels, exact for cubics** (`cubic_point_from_means`, `quartic_defect`,
`boundary_point_from_pixels`). From three adjacent column sums `S₋, S, S₊`, the boundary
passes through `(i + ½, j₀ + S − (S₊ − 2S + S₋)/24)` exactly whenever it is a cubic; the
first missed term is `−(3/640)f''''`. This is the finite-volume correction, whose fourth-order
accuracy already implies exactness on cubics, and it is Trujillo-Pino's centre coefficient
(their model is a quadratic; their slope coefficient, unlike the centre, is biased on a
cubic). What is added here is the machine-checked chain from pixel areas to the point.

**Thin strokes that straddle a border** (`straddle_inversion`). A stroke thinner than a pixel
that crosses a pixel border along a whole column is read exactly from its two pixels:
centre line `j + 1 + (a₊ − a₋)/2`, width `a₋ + a₊`.

**Under noise** (`strip_unbiased`, `strip_variance`, `face_value_noise`,
`point_from_means_noise`). Zero-mean, pairwise independent pixel noise leaves a column sum
unbiased with variance `k·σ²`. The face value `(7(S₁ + S₂) − (S₀ + S₃))/12` has noise gain
`25/36`; the point correction `113/96`.

**Under supersampling renderers** (`subCount_close`, `ss_column_sum_close`). With `n × n`
point supersampling instead of exact area, a column sum is the `n`-point midpoint rule of the
boundary height to within `1/(2n)` px, `1/16` px on the 8x8 renders inkvec's regression
corpus uses. That is an upper bound on the column sum's error. Informally it is also a floor
for any reading: a boundary aligned with the sample grid moves no sample until it crosses a
sample row, so its pixels cannot place it more finely.

### 3.4 How biased the common readings are

**The ½-crossing** (`levelSet_reading`, `levelSetBias_le`, `levelSetBias_max`). On a
straight, axis-aligned, noiseless step edge (the easiest input there is), reading the edge
where the linear interpolation of pixel-centre coverages crosses ½ puts an edge `δ` above its
pixel's centre at `δ/(½+δ)`. The error is at most `3/2 − √2 ≈ 0.0858` px, and that maximum
is attained. The literature has empirical measurements of this bias (Rockett 1999; Lyvers et
al. 1989) but, as far as the survey found, not the constant. The column sum reads the same
edges with error 0 (`column_sum_reading`). `coverage.rs`'s `DEFAULT_SIGMA_MODEL = 0.05 px`,
"measured on analytic circles" for level-set extraction, is consistent with this bias.

**The coverage centroid on a thin stroke** (`centroid_bias`, `centroid_bias_worst`). Reading
a straddling sub-pixel stroke's centre as the coverage centroid of its two pixels, as
`planar::ridge_offset` does, is off by `(1 − w)(a₊/w − ½)`, up to `(1 − w)/2`: as large as
the error of not knowing the position at all, in the one case where it is known exactly.

### 3.5 The code's own constants

`strip.rs`'s rational constants are proved exact (`histopolate_cubic`, `face_value_cubic`,
`side_stencil_cubic`). Fed the four cell means of any cubic, its `histopolate` returns that
cubic's coefficients, and the one-sided stencils of its corner test agree with the central
one on every cubic. End to end (`strip_reading_exact`): from the raw Lebesgue pixel areas of
the four columns round a pixel border, `histopolate` returns the boundary cubic's own
coefficients, so the vertex `strip.rs` slides onto it lies on the boundary.

## 4. What the theorems say about inkvec

Measured on shapes rendered with an exact-area rasteriser (`bench/theory/exact_raster.py`, a
float64 port of font-rs's accumulation, checked against hand computations), 64 px, 8-bit,
by distance from each boundary point to the true boundary (`bench/theory/inkvec_compare.py`):

| reading | mean px | p99 px | max px |
|---|---|---|---|
| ½-crossing of interpolated coverage | 0.0505 | 0.087 | 0.094 |
| inkvec stage 07 (`refine_subpixel`), before this change | 0.0720 | 0.315 | 0.613 |
| inkvec stage 07, after it (§5) | 0.0034 | 0.073 | 0.349 |
| column sum + cubic correction (Python, `strip_eval.py`) | 0.0011 | 0.003 | 0.004 |

The per-vertex reading was worse than the plain ½-crossing on average, for three reasons the
theory names:

1. **It needs a normal and does not have one.** `edge_offset` inverts a straight edge's
   coverage exactly *given its normal*. The normal comes from the lattice staircase and is
   quantised to multiples of 45°; where two partial pixels bracket ½ the reading falls back to
   linear interpolation between them, which is the biased ½-crossing of §3.4. The column sum
   needs no normal at all (§3.1).
2. **The seam vertex of every closed ring was never refined.** `probe_chord` clamped
   indices at the ends of an edge, so a closed ring's first vertex took its chord from one
   neighbour only. On a ring whose seam lies on a horizontal run that is a horizontal normal
   on a horizontal boundary: the probe runs along the boundary and the vertex stays on the
   lattice, 0.30 px off on an exact disc. Quality's boundary solve repairs it; Fast mode kept
   it in every closed ring.
3. **It reads one vertex at a time**, where the evidence about a smooth boundary is spread
   over several columns. The cubic through four column means uses all of it.

The boundary solve (stage 08) recovers most of this in Quality mode: it fits the exact
rendered coverage. On 30 exact circles Quality's emitted `<circle>` is within 0.0065 px of
the true radius on average, against 0.027 px for Fast. But §3.2 says the solve's data term
is flat along the zigzag and, with free 2-D points, along at least `2N − M` directions, so it
needs priors (`K_KINK`, `K_ANCHOR`) to choose for it. A start already on the boundary leaves
the priors less to decide.

## 5. What changed

**The strip reading** (`crates/inkvec-trace/src/planar/strip.rs`). For a vertex between two
flat fills with enough contrast, before the per-vertex probes:

1. Read six adjacent pixel columns around the vertex (rows when the boundary is steeper than
   45°, and the other axis when the first declines). In each, find the window from a pixel
   saturated in one face to the nearest pixel saturated in the other, and sum its unmixed
   coverages, flanks included. By `column_sum_eq_average` that is the boundary's mean height
   over the column, and summing the flanks' own values keeps it unbiased under noise
   (`strip_unbiased`). Each column's window is searched around its inner neighbour's mean, so
   steep boundaries are followed.
2. Histopolate the cubic through the four central means (`histopolate`: exact constants,
   `histopolate_cubic`). It *is* the boundary whenever the boundary is a cubic
   (`poly_eq_of_averages`).
3. Refuse a corner. The cubics through the left four and the right four columns must agree
   with the central one at the vertex's pixel border to within `SIDE_TOL = 0.05` px. Every
   cubic passes (`side_stencil_cubic`); a corner within reach, or a third colour in a window,
   does not.
4. Slide the vertex along its lattice normal onto the cubic (Newton). The edge keeps one
   point per vertex, so nothing downstream sees a different structure.

A vertex where any step declines takes the per-vertex probes as before.

**The seam** (`probe_chord`). A closed edge's chord wraps round its seam.

### Measured

Stage-07 points, `bench/theory` shapes (circles, ellipses, straight edges at random angles,
rotated squares, regular polygons), distance to the true boundary in px:

| renderer | before: mean / p99 / max | after: mean / p99 / max |
|---|---|---|
| exact area | 0.0734 / 0.319 / 0.516 | **0.0106 / 0.180 / 0.368** |
| 8x8 supersampled | 0.0732 / 0.318 / 0.443 | **0.0159 / 0.194 / 0.351** |

The remaining tail is vertices where the strip reading declines (corners, junctions,
windows a third colour enters) and the probes still decide.

The regression gate (`bench/ci_gate.py`, 246 icons, judged at 1024 px against the artists'
files, paired family-stratified bootstrap) against this branch's `main` with both changes
off:

| condition | dE00 (95 % CI) | icons better / worse | turning | parameters vs artist |
|---|---|---|---|---|
| fast-128ss | **−10.96 %** [−12.28, −9.71] | 216 / 30 | −0.69 % (better) | −2.72 % (better) |
| fast-512ss | **−8.70 %** [−10.43, −7.18] | 220 / 22 | −1.13 % (better) | −0.02 % |
| fast-512ssop | **−9.83 %** [−11.71, −8.15] | 216 / 21 | −1.22 % (better) | −0.68 % |
| quality-128ss | **−4.51 %** [−6.61, −2.54] | 144 / 79 | +0.01 % | +1.93 % (margin 3 %) |
| quality-512ss | **−1.89 %** [−3.52, −0.26] | 124 / 99 | −0.04 % | +0.72 % |
| quality-512ssop | −1.36 % [−2.87, +0.19] | 131 / 93 | +0.36 % | +0.50 % |

The gate passed: every dE00 and turning change is a demonstrable gain or non-inferior, and
every parameter-ratio change is inside its 3 % margin. Quality spends slightly more
parameters at 128 px (+1.9 %), because sharper boundary points give the MDL fitter more
evidence for detail, while its colour error falls 4.5 %. Ablated, the strip reading accounts
for the whole gain (fast-128ss −10.92 %, quality-128ss −4.32 % on its own). The seam fix is
within noise on every condition: the corpus's closed rings mostly start at a junction or a
corner. The hard-case ratchet passes 42/42 in Quality and 28/42 in Fast, one more than before
(`edge_axis`).

Time, whole process, 40 corpus icons at 512 px (minimum of three runs each for Fast):
Fast median 22.3 ms before, 22.7 ms after; Quality median 538 ms before, 529 ms after. The
strip reading reads pixels directly at their centres and compares squared residuals, which
is what keeps it out of Fast's budget; reading them through the probes' bilinear sampler
had cost Fast 14 %.

## 6. What else the theory suggests, ranked

1. **Read thin strokes from the straddle formula** (`straddle_inversion`, `centroid_bias`).
   `ridge_offset` reads a sub-pixel stroke from its coverage centroid, biased up to
   `(1 − w)/2`. Where the stroke straddles a border the exact centre is
   `j + 1 + (a₊ − a₋)/2`; where it does not, the pixel centre is the minimax choice. Small,
   local, testable on `bench/cases` `ribbon_*`.
2. **Remove the null space from the boundary solve instead of pricing it.** The data term
   is flat along exactly the directions `polyline_kernel` and `flat_directions` name. Moving
   points only across the boundary removes the tangential half. Parametrising each run by a
   curve with no more coefficients than the columns it covers removes the rest
   (`polynomial_boundary_determined`). Either leaves the kink prior to express a preference,
   not to stand in for missing data, and should cut the iterations a quarter of the parts now
   spend at the cap.
3. **Retune `DEFAULT_SIGMA_MODEL`.** It encodes the ½-crossing's resolution limit (0.05 px,
   §3.4). Strip-read vertices are an order of magnitude better located. Because sigma scales
   with `lambda`, this is a fidelity/compactness move to be swept, not a free gain.
4. **A minimum span for the fitter's segments.** A segment with `p` shape coefficients
   needs at least `p` columns of evidence (`polynomial_boundary_determined`); shorter
   segments are fitting the null space.
5. **Corners from one-sided stencils.** Where the corner test refuses, the one-sided cubics
   (or lines) on either side are each exact on their side; intersecting them would place the
   corner, the role `refine_junctions` plays for junctions.
6. **Estimate the compositing curve (DESIGN.md S0).** The column-sum theorem holds in the
   space where the renderer blends. A straight edge's column sums are affine in the column
   index only under the right transfer curve, which makes the curve measurable from the image.

## 7. Limits

* The model is exact area coverage, composited linearly in the encoded values, with two
  colours meeting at each boundary. Renderers that point-supersample are covered with the
  `1/(2n)` floor of §3.3; linear-light compositing and colour management are not modelled.
* The theorems are about the coverage `a`. Unmixing `a` from colours (`unmix_pair`) is taken
  as given; it is exact for two flat fills and approximate at gradients, which is why the
  strip reading is used only between flat fills.
* The strip reading is exact on cubic boundaries in the noiseless exact-area model. Elsewhere
  its error is bounded by the theorems' error terms (`quartic_defect`, `ss_column_sum_close`)
  or it declines. Its corner test cannot tell a gentle corner (under about 30°) from noise and
  may round it by up to about 0.1 px.
* The measured gains are on the committed corpus and the synthetic shapes above. Other
  inputs (photographs, heavily compressed JPEG, hand-drawn art) are untested here.

## Reproduce

```bash
cargo build --release -p inkvec-cli
python3 bench/theory/exact_raster.py                      # renderer self-check
python3 bench/theory/strip_eval.py                        # estimators on exact renders
python3 bench/theory/inkvec_compare.py --exe target/release/inkvec
python3 bench/ci_gate.py --exe target/release/inkvec      # the regression gate
cd formal/InkvecTheory && lake exe cache get && lake build && lake build InkvecTheory.Audit
```
