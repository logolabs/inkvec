# Inkvec — Design Document

**Scope:** logos, icons, flat/vector-style art. Not photographs, not centerline/sketch tracing.
**Delivery:** Rust core (geometry, topology, fitting) + Python bindings for ML modules.
**Date:** 2026-08-31 · **Status:** M0 built and measured; M1 designed.

> **Governing directive:** use the best available algorithm at every stage, even where it
> is substantially slower. §5 was rewritten against this. The three changes that matter:
> S2 becomes analysis-by-synthesis rather than per-pixel coverage inversion; S3 uses exact
> geometric predicates rather than epsilon comparison; and S4 becomes one global
> discrete–continuous optimization whose segment alphabet **includes primitives**, which
> dissolves the old M2 into M1. Baseline to beat: [`M0-BASELINE.md`](M0-BASELINE.md).

---

## 1. The target: what Vectorizer.AI actually does

Vectorizer.AI (Cedar Lake Ventures — James Diebel & Jacob Norda, out of a Stanford AI
project; the same team behind Vector Magic since 2007) is the commercial bar. Its own
feature disclosures are unusually specific, and they confirm what the output quality
implies. Decomposed:

| Capability | What it means technically | OSS has it? |
|---|---|---|
| **Sub-pixel precision** — "feature extraction at sub-pixel widths using anti-aliasing pixel values" | The AA pixel value *is* a coverage measurement. Inverting it recovers edge position to a fraction of a pixel. | **No** |
| **"Vector Graph"** — a proprietary geometry framework "enabling localized optimizations impossible with conventional representations" | A planar subdivision (shared edges between adjacent faces), not a list of independent paths. | **No** |
| **Full shape fitting** — parameterized circles, ellipses, rounded rectangles, stars, with rotation | Primitive detection and *snapping* before curve fitting. | **No** |
| **Rich curve alphabet** — lines, circular arcs, elliptical arcs, quadratic and cubic Béziers | An arc is exact and one segment; four cubics approximating a circle is neither. | **No** |
| **Symmetry modeling** — mirror and rotational | Detect, then *enforce*, symmetry. Cleans geometry and enables `<use>` reuse. | **No** |
| **Clean corners** | Explicit corner model rather than a smoothing threshold. | Partial (Potrace `alphamax`) |
| **Adaptive simplification** | Node budget allocated by boundary salience — aggressive on faint edges, conservative on sharp ones. | **No** |
| **32-bit ARGB, alpha as a first-class concept** | Partial transparency handled in the region model, not post-hoc. | **No** |
| **Stacked vs. cutout output, grouping controls** | Export policy decoupled from internal representation. | Partial (VTracer `--hierarchical cutout`) |

They also claim custom in-house deep learning models on a proprietary dataset. Note the
ordering: **the deep learning sits on top of 15 years of computational geometry**, not
in place of it. That ordering is the single most important lesson in this document.

Marketing claims of "5–10× fewer anchor points, up to 85% smaller files" are vendor
numbers and should be treated as unvalidated until we measure them ourselves (§7).

---

## 2. Literature review

### 2.1 Classical tracing

**Potrace** (Selinger, 2003) — still the best-understood tracing algorithm, and the one
worth stealing from. Four phases:

1. **Path decomposition.** Walk the boundary graph between black and white pixels.
   Vertices are *integer pixel corners*. Turn policy (`minority` by default) resolves
   diagonal ambiguity. Despeckle by path interior area (`turdsize`).
2. **Optimal polygon.** Defines a "straight" subpath via a triplewise criterion, computes
   all straight subpaths in O(n²), then finds the polygon minimizing
   **(segment count, penalty)** lexicographically — as a shortest cycle in a directed
   graph, O(nm). Penalty is `|v_j − v_i| · stdev(distance of path points from the chord)`,
   computable in O(1) per pair via prefix-sum tables. **This is the only non-local step**,
   and it is why Potrace straightens long shallow staircases that local methods leave bumpy.
3. **Vertex adjustment + corner analysis.** Fit a best-fit line to each polygon segment,
   place each vertex near the intersection of adjacent lines (constrained to a unit
   square around the original). Then a one-parameter Bézier family (α = β, justified by an
   area-equivalence argument) gives a corner test: α ≤ α_max → smooth curve, α > α_max →
   corner. α is clamped to [0.55, 1]; 0.55 ≈ 4/3(√2−1), the optimal quarter-circle
   approximation. Note the resulting bias: corners are favoured both by sharp angles and
   by long segments.
4. **Curve optimization.** Join adjacent Bézier segments where convexity agrees and total
   turn < 179°.

**The ceiling:** step 1 operates on *integer* coordinates. Every downstream refinement is
polishing information that was destroyed before the algorithm started. Potrace is also
1-bit only, and GPL — **do not vendor it**.

**VTracer** (visioncortex, MIT, Rust) — the de-facto OSS colour tracer. Explicitly
"skips Potrace's expensive optimal-polygon search in favour of a fast, linear pipeline."
Clustering modes `color-cluster` / `bw` / `watershed`; `--hierarchical stacked|cutout`
(cutout does produce a seam-free mosaic); curve modes `pixel|polygon|spline`; plus
`filter_speckle`, `color_precision`, `gradient_step`, `layer_difference`,
`corner_threshold`, `splice_threshold`, `path_precision`.

Give it credit: cutout mode and watershed clustering are more than it is usually
credited with. But: boundaries remain pixel-quantized, gradients are approximated by
*banding into discrete layers* (`gradient_step`), there is no node budget (dropping the
optimal-polygon DP is exactly what costs it anchor efficiency), no primitives, no arcs,
no symmetry, no alpha model. AnchorFlow's robustness experiment is damning: under
boundary perturbation, VTracer's parameter count grew **+106.7%**, because
boundary-following inherits contour noise directly as redundant anchors.

### 2.2 Optimization-based (differentiable rendering)

**DiffVG** (Li et al. 2020) established differentiable rasterization. **LIVE** (CVPR 2022)
added layer-wise path insertion with a self-intersection (Xing) loss. **O&R** (2023)
made it top-down and faster. **SuperSVG** (CVPR 2024, code released) uses superpixel
decomposition with a coarse-then-refine pair of models and a *dynamic path warping* loss
so the refinement stage inherits knowledge from the coarse stage — currently the best
speed/quality point in this family.

**Verdict for our scope:** this family optimizes *pixels*, not *structure*. Topology is
entangled with curve parameters; node counts are uncontrolled; paths self-intersect;
output is not something a designer can open and edit. Recent work states this bluntly —
VectorFusion and SVGDreamer are described as producing "uninterpretable and uneditable
shapes." **We use differentiable rendering only as a final polish with topology and node
count frozen** (§5, S5). That inverts the usual use, and the inversion is the point.

### 2.3 Learned structure — the important recent line

**AdaVec** (Zhao et al., 2025) — "Less is more: efficient image vectorization with
adaptive parameterization." Decomposes into path-like foreground components, then
adapts parameterization per component.

**AnchorFlow** (arXiv 2605.19551, May 2026) — the closest published work to our goal, and
the most important paper here. Thesis: *the fidelity↔editability tradeoff should be
attacked at the level of anchor placement*, because anchors on Bézier curves define local
path structure and drive both accuracy and editability. Pipeline:

1. **AFNet**, a lightweight encoder–decoder, predicts an image-conditioned **sparse
   anchor field** — sharp peaks at likely anchor locations plus weaker contour support
   for ordering and connectivity. Crucially this is a *learned structural intermediate*,
   not the output.
2. **Deterministic resolver**: local maxima → anchor candidates → projected onto the
   contour for stable ordering → tangent-based Bézier initialization → SDF-guided
   refinement that preserves anchor structure.
3. **Rendering-guided field refinement**: iterative, updates only the per-sample latent
   code (not network weights), re-resolves the field, and accepts only if a
   tolerance-based stroke score improves.

Results: 61.2 parameters vs VTracer's 206.4 on a 2.5K-path benchmark. Under boundary
noise, +2.9% parameters vs AdaVec's +20.7% and VTracer's +106.7%. Human study (n=15)
preferred it over LIVE and StarVector on visual similarity, and over AutoTrace, LIVE and
AdaVec on task-based editability. Datasets: Noto Emoji, Fluent Emoji, ColorSVG-100K.
Limitations: train/inference gap (fixed anchor targets at training, rendering feedback at
inference); inherits upstream component-extraction errors; slower than single-pass
tracing. **No code or weights announced.**

**What we take:** the anchor-field concept is exactly the right division of labour — the
network decides *where structure is*, deterministic geometry decides *what the
coordinates are*. We adopt it as our S4 corner/anchor prior. We do **not** adopt their
upstream component decomposition; our planar map is a better substrate and removes the
error source they name as their own limitation.

### 2.4 Semantic layering

**LayerPeeler** (arXiv 2505.23740) — a VLM builds a layer graph of occlusion
relationships; a finetuned diffusion model removes the frontmost layer using its caption
as an edit instruction, with localized attention control; repeat. Ships a large-scale
layer-peeling dataset. **AmodalSVG** (arXiv 2604.10940) — Qwen3-VL picks the frontmost
object, GroundedSAM segments it, LaMa+FLUX inpaint what was behind, repeat; then
per-layer vectorization with occlusion-aware primitive pruning (an "effective visual
contribution score" measuring what survives alpha compositing) and an error-budget
mechanism that extrapolates historical improvement rate to predict primitive counts.
PSNR 32.91 dB vs 30.52 dB for the best baseline, with object-level editing (recolour,
reposition, resize, remove, replace).

**Verdict:** genuinely valuable for editability, and directly applicable to layered logo
art. But heavy (VLM + SAM + diffusion inpainting), and generative inpainting *invents*
content behind occlusions — a correctness hazard for a logo. Schedule for M4, opt-in,
never on by default.

### 2.5 Sub-pixel recovery

**Subpixel Deblurring of Anti-Aliased Raster Clip-Art** (Yang et al., CGF 2023, UBC) —
the most directly useful paper for our front end. Observes that anti-aliasing blurs
region boundaries and obscures the artist's intended region topology and colour palette,
*but simultaneously preserves sub-pixel detail*. Two stages: a network predicts a
low-blur double-resolution approximation, then a perception-driven **discrete**
partitioning procedure — guided by clip-art priors (few regions, uniform colours, clear
boundaries), by properties of the anti-aliasing process, and by how humans actually
perceive anti-aliased art — produces a blur-free output with dramatically reduced palette
and region counts. User study preferred it 75 to 8.5 over the best alternative (compared
against ESRGAN, Lightroom, VectorMagic, Kuwahara, MMPX, XBR). The paper explicitly
proposes itself as the first step of a vectorization pipeline. Code listed as "coming
soon" — **verify current status.**

### 2.6 Curve fitting — and the piece already built in Rust

**Levien, "Fitting cubic Bézier curves" (2021)** — under G1 constraints (matching
endpoints and tangents), the only free parameters are the two control-arm lengths.
Matching **signed area** (via Green's theorem) reduces the 2-D space to 1-D; adding the
**first x-moment** reduces the simultaneous constraints to a single **quartic**, solvable
analytically. This exposes the full solution structure — including that C-shaped cubics
admit three near-identical-looking parameter sets, i.e. three local minima, which is
precisely why Schneider's Graphics Gems algorithm gets stuck. A fourth, looped solution
with balanced signed area can fit cusped sources better.

**"Simplifying Bézier paths" (2023)** and **`kurbo`** — the `ParamCurveFit` trait
abstracts a source curve via position/derivative samples with cusp detection; prefix sums
give O(1) area/moment range queries. Error is approximate **Fréchet distance** (preserves
orientation, unlike Hausdorff), with an arc-length reparameterization fallback for
"spicy" high-curvature-variation curves (~10× cost). Naïve adaptive halving produces
~1.5× the optimal segment count; finding subdivision points that equalize error at the
threshold is ~50× slower but near-minimal. Bumpy cubics — where endpoint-to-control
distance approaches the chord length — are penalized or excluded above δ ≈ 0.85.
`fit_to_bezpath` targets a Fréchet accuracy; **`fit_to_bezpath_opt` computes optimal
subdivision points and is "expected to be very close to the optimum possible Bézier path
— minimal number of segments and minimal error over all paths with that number of
segments."** Levien's own assessment: the best implementation in either the academic
literature or shipping products.

Known gaps: Fréchet optimizes distance only, ignoring curvature/angle error; not
validated on noisy scanned input; low-pass prefiltering unexplored.

**This is decisive for the stack choice.** The hardest classical component of our S4 —
optimal Bézier fitting with minimal segment count — already exists, in Rust, permissively
licensed, written by the person who understands the problem best. We are not rebuilding it.

### 2.7 Gradients

**Du et al., SIGGRAPH 2023** — "Image vectorization and editing via linear gradient layer
decomposition": decomposes regions into opaque and semi-transparent linear-gradient fills.
**Chakraborty et al., CGF 2025** — "Image Vectorization via Gradient Reconstruction":
discontinuity-aware segmentation plus gradient reconstruction into compact Béziers.
**Hierarchical Diffusion Curves** (TOG 2014) and **Diffusion Curves** (2008) are the
expressive ceiling but are not SVG-native — a diffusion curve does not round-trip into
Illustrator. For our scope, `linearGradient`/`radialGradient` with 2–4 stops covers the
overwhelming majority of real logo art and *does* round-trip.

### 2.8 Primitives and symmetry

Standard computer vision, under-applied to vectorization. Ellipse/circle detection via
geometric symmetry (locate centre candidates by centre-symmetry, link edge points into arc
segments by connectivity and curvature conditions, group arcs belonging to the same
ellipse, then RANSAC-fit) is mature and fast. Optimized least-squares polygon and ellipse
fitting is likewise well covered. Nobody in open-source vectorization wires it in. Cheap,
high-value, differentiating work.

### 2.9 Benchmarks

**VectorGym** (arXiv 2603.29852) — multitask benchmark from in-the-wild GitHub SVGs,
preserving original structure and higher-order primitives (circle, text, gradients,
animation logic), with expert-human targets. **VectorEdits**, **SVGenius**,
**SVGEditBench** target LLM editing rather than vectorization fidelity. **ColorSVG-100K**,
Noto Emoji and Fluent Emoji are the de-facto reconstruction sets (used by AnchorFlow).

**Gap:** no benchmark measures the *editability* of a vectorizer's output with structural
metrics. We define one in §7. That is itself a publishable contribution, and it is what
will make a SOTA claim defensible.

### 2.10 Competitive note

Adobe Illustrator's Image Trace added linear-gradient detection with a strength slider,
reduced anchor counts, and an Auto Grouping option (Oct 2024), with Concept to Vector in
30.5 (May 2026). "Fewer points, grouped layers, editable gradients" is now the industry's
stated definition of quality. We are aiming at a moving target, and the target's direction
of travel is **editability**, not fidelity. Our thesis is aligned with where the field is
going, not merely with where it is.

---

## 3. Gap analysis

Four structural deficits, in order of impact for flat art. **None require a neural network.**

1. **Information destroyed at step 1.** Thresholding and colour quantization discard the
   AA coverage signal. Every OSS tracer starts from pixel-quantized boundaries.
   Vectorizer.AI does not.
2. **Wrong output data structure.** SVG's native model is a list of independent paths.
   Adjacent regions therefore duplicate their shared boundary — producing seams or
   overdraw, doubling geometry, and making "move this edge" a two-path operation.
3. **Impoverished curve alphabet.** Cubics only. A circle becomes four approximate
   segments carrying 12 numbers instead of `<circle cx cy r/>` carrying 3. A rounded
   rectangle becomes 8+ segments. This is simultaneously the fidelity loss *and* the
   editability loss.
4. **No node budget.** VTracer explicitly dropped Potrace's optimal-polygon DP for speed.
   Nothing replaced it. Anchor count is an emergent accident rather than a controlled
   quantity.

**Thesis.** Fix these four with classical geometry and we approach the commercial bar
without a GPU. Then apply learned priors where they genuinely outperform hand-written
rules — deciding *where anchors belong* (AnchorFlow) and *what a region means* (semantic
grouping) — rather than asking a network to emit coordinates, which is the thing networks
are worst at.

---

## 4. Core data structure: the region graph

Everything hinges on this. We do **not** carry a list of paths internally.

```
Planar subdivision (half-edge / DCEL)

  Vertex    — a junction where >=3 regions meet; sub-pixel position; confidence

  Edge      — a boundary arc between EXACTLY TWO faces
              · sub-pixel polyline (the measurement)
              · fitted geometry: Line | Arc | Cubic* | owned-by-Primitive
              · salience: contrast across the edge; drives the node budget
              · uncertainty: from S2 conditioning; drives fit tolerance

  Face      — a region with a fill model:
              Flat(colour) | Linear(axis, stops) | Radial(c, r, stops) | Raster(fallback)
              · alpha, z-hint, optional semantic label

  Primitive — a constraint asserting that a cycle of edges IS a
              circle / ellipse / rect / rounded-rect / star, owning its parameters;
              member edges become views onto it

  Symmetry  — a constraint relating edge groups under reflection or k-fold rotation
```

Consequences, all of which are the point:

- A shared boundary exists **once**. Seams and overdraw become structurally impossible,
  not something to tune away.
- Local edits stay local: simplify one edge, snap one junction, merge two faces — without
  touching anything else. This is what "localized optimizations impossible with
  conventional representations" means.
- Export becomes a *policy* over the same structure: stacked (with knockouts), cutout
  mosaic, or grouped-by-semantics. Chosen at write time, not baked in at trace time.
- Constraints (primitive, symmetry) are first-class and can be enforced *during*
  optimization rather than pattern-matched afterwards.

Rendering-order inference: build a containment/adjacency DAG over faces; topological order
gives z. Ambiguity resolved by area and by alpha-compositing consistency.

---

## 5. Pipeline

**Governing directive (2026-08-31):** *use the best algorithm at every stage, even where
it is substantially slower.* This section was revised against that directive. Where a
cheaper method was previously chosen for tractability, the accurate method is now the
specified one, and the cheap method survives only as an initializer or a development
affordance.

### 5.0 One objective, not two knobs

Fidelity and editability were previously separate axes traded against each other by a
λ. That framing is what forces the usual compromise. Both are description-length terms,
so state the whole problem as **minimum description length under an explicit image
formation model**:

```
  D* = argmin_D   -log P( I | render(D, θ) )   +   λ · L_desc(D)
                  \_______ fidelity _______/       \_ editability _/
```

* `I` — the observed raster.
* `θ` — the *estimated* image formation parameters (anti-aliasing kernel, gamma), not
  assumed ones (§5.1).
* `render(D, θ)` — analytic coverage rasterization of the vector document.
* `L_desc(D)` — description length in **editable** units, not bytes. A `<circle>` costs
  3 numbers; the same shape as four cubics costs 24. A shared edge is stored once. A
  `<use>` costs a reference, not a copy. So the term that rewards compactness is the
  same term that rewards editability — they stop being opposed.

Every stage below is a tractable approximation to this single objective, and the final
stage (§5.6) optimizes it directly over structured moves. This is also what makes the
benchmark's Pareto sweep principled: λ *is* the quality knob.

### S0 — Estimate the image formation model, do not assume it

The coverage inversion in S2 is only as good as the forward model it inverts, and the
two parameters that matter are routinely guessed wrong:

* **Compositing gamma.** If the source composited in sRGB and we invert assuming linear
  light (or vice versa), every coverage estimate carries a systematic bias — worst
  exactly at mid-coverage, which is where boundary position is most sensitive.
* **The anti-aliasing kernel.** Analytic box coverage, a downsampled supersample, a
  Lanczos resize and a JPEG round-trip produce measurably different edge profiles.

So estimate both, per image, by fitting the edge-spread function on high-contrast
straight boundaries where the answer is over-determined. This is a small parametric
inverse problem and it is cheap relative to what follows. Assume nothing.

Also here: JPEG artifact removal, and optional 2–4x super-resolution **fed only to S2**,
never to S1's colour model — SR sharpens the geometry signal but hallucinates colour.
For <=128px anti-aliased clip art, route through a Subpixel-Deblurring-style module
(§2.5).

### S1 — Region proposal

Perceptual clustering in **OKLab** with ΔE00 merge thresholds — not RGB bit-truncation.
(`color_precision` is a blunt instrument: it splits colours a viewer cannot distinguish
and merges ones they can, because RGB distance is not perceptual distance.)

Per-region **model selection** — flat / linear / radial / texture — chosen by MDL on the
residual under the S0 image model, so we neither spend a gradient on a flat region nor
band a gradient into layers. Alpha estimated jointly with colour, not post-hoc.

Region proposals are *hypotheses*, not decisions: S6 can still merge or split faces if
the objective improves. Nothing here is final.

### S2 — Sub-pixel boundaries by analysis-by-synthesis

**Revised.** The cheap method — invert `P = a·F + (1-a)·B` per pixel for coverage `a`,
convert each to a local edge position, then least-squares a polyline through them —
is now only the *initializer*.

The accurate method never forms that intermediate polyline. Parametrize the boundary
curve directly, forward-render it through the S0 model to predict pixel values, and
minimize the residual against the observed image by non-linear least squares,
solved with Fletcher–Reeves nonlinear conjugate gradient:

```
  minimise  Σ_pixels  || I_observed  -  render(boundary curve, θ) ||²
```

Why this is strictly better, not merely slower:

* **Every pixel's evidence is used jointly.** Per-pixel inversion treats each coverage
  estimate as independent and equally reliable; they are neither. The joint solve
  weights each by its actual conditioning.
* **Low-contrast boundaries stop being a special case.** Where `|F-B|` is small the
  per-pixel estimate is ill-conditioned and noisy; in the joint solve it simply carries
  little weight, and the boundary is pinned by its better-conditioned neighbours.
* **Triple points stop being a special case.** Three regions meeting inside one pixel
  is just a three-way mixture in the forward model. The cheap method needs an explicit
  detector and a separate solver; here it falls out.
* **Sub-pixel-wide features stop being a special case.** A stroke narrower than a pixel
  never reaches full coverage, so thresholding destroys it — measured in
  `M0-BASELINE.md` §3, where VTracer recovered 2 of 11 elements and *no setting*
  recovered more. A forward model that can render a 0.3px stroke can fit one.

Output carries a per-edge **posterior covariance**, which is the honest uncertainty and
becomes the fit tolerance and node budget salience downstream. Nothing later has to
invent a tolerance parameter.

v2 may add a learned SDF predictor as a better initializer, self-supervised on rendered
random SVGs where ground truth is free and infinite. It initializes; it does not decide.

### S3 — Planar map with exact predicates

**Revised.** Topology decided by **Shewchuk adaptive-precision predicates** (the
`robust` crate), not by float comparison against an epsilon.

Orientation and incircle tests are evaluated exactly; coordinates remain `f64`. The
consequence is categorical rather than incremental: the planar subdivision is **valid by
construction**. No epsilon to tune, no degenerate configuration that silently yields a
self-intersecting face or an inverted winding, no class of input that quietly produces
a broken document. Given that the whole architecture rests on this structure being
sound, its correctness should not be a matter of tolerance selection.

Then: snap junctions (T-junctions, triple points), resolve topology conflicts, despeckle
by face area under the MDL objective rather than a fixed pixel count.

### S4 — Global discrete–continuous fit

**Substantially revised.** Previously a pipeline of local decisions: corner detection,
then curve fitting, then primitive pattern-matching afterwards. That ordering cannot
work well — by the time cubics have been fitted they have already absorbed the error a
primitive would have explained.

Instead, **one global optimization** deciding segmentation and segment type together, by
dynamic programming over each boundary with a multi-model alphabet:

```
  alphabet:  line | circular arc | elliptical arc | cubic Bézier
  DP state:  (position, incoming tangent constraint)
  edge cost: orthogonal-distance residual under S2's covariance  +  λ · description length
```

This is Potrace's optimal-polygon dynamic program — the lexicographic
(segment-count, penalty) shortest cycle — generalized from one model to four, with cost
measured in MDL rather than in a straightness penalty. It is established technique, not
invention: the multi-model MDL approximation of digital curves, and optimal polyline
compression with segments and arcs, are both published lines of work. Prefix sums keep
each candidate segment's cost O(1), giving O(n²) per boundary. That is exactly the step
VTracer dropped for speed — and `M0-BASELINE.md` §5 measures what dropping it costs:
Potrace's parameter count grows **0.0% (median)** under boundary perturbation where
VTracer's grows **+18.0%**. The non-local optimization is what buys noise robustness.

Continuous fitting inside each candidate segment uses **orthogonal (geometric) distance
fitting**, not algebraic least squares. Algebraic fitting carries a documented
**high-curvature bias**, and high curvature is precisely where logo geometry lives —
corners, tight fillets, small counters. Recipe: Taubin algebraic fit for initialization,
then Ahn's orthogonal-distance refinement. Where the G1 cubic case applies, Levien's
reduction to a single quartic gives the optimal control-arm lengths in closed form,
which both removes the three-local-minima trap that defeats Schneider's algorithm and
is faster than iterating.

**Primitives and symmetry are inside this optimization, not bolted on after it.** This
is a direct architectural consequence of the directive and it dissolves the old M2:

* A circle is not pattern-matched from fitted cubics; `circle` is a member of the
  segment alphabet, and the DP selects it when it minimizes description length. Same
  for ellipse, rounded rectangle, star, regular polygon.
* Candidate primitive parameters are refined **against the image**, not against the
  extracted polyline — analysis-by-synthesis again. The polyline is an intermediate; the
  pixels are the evidence.
* Symmetry (mirror axes, k-fold rotation) is searched over the region graph and, once
  accepted, **enforced** as a constraint during re-optimization rather than approximated
  afterwards. Enforcement is what produces the visibly clean result, and it lets the
  document emit `<use>` + transform, which pays for itself twice: fewer parameters and a
  structure a designer can actually edit once instead of k times.
* Acceptance is by MDL against S2's posterior covariance, so a primitive is adopted only
  when the evidence genuinely supports it. This is the guard against the failure mode
  in §9.5 — enforcing a symmetry the designer never intended.

Where an exact node budget is requested rather than a quality level, carry the budget as
a DP state dimension (knapsack-style) instead of sweeping λ. The λ sweep only reaches
the convex hull of the frontier; the budget-indexed DP reaches the frontier itself.

### S5 — Structured refinement (topology **not** frozen)

**Revised — this reverses an earlier decision.** The previous design froze topology and
node count during differentiable polish, to avoid inheriting the LIVE/O&R family's
collapse into unstructured, uneditable geometry. With the time budget relaxed, freezing
is no longer the right trade: it protects editability by refusing to search at all.

Instead, search over a **structured move set** — every move preserves or improves
editability by construction:

```
  merge two adjacent faces          promote a cubic run -> arc -> primitive
  split an over-fitted edge         snap a junction
  adopt a candidate symmetry        share a boundary that was duplicated
  re-order z / promote to <use>     demote a primitive back to curves
```

Each move is accepted only if the §5.0 objective improves. This is hill-climbing over
documents rather than gradient descent over control points, which is why it cannot
degrade into the blob failure: there is no move in the set that produces one. The
distinction from LIVE/O&R is not that we optimize less, it is that we optimize over a
different space.

Continuous polish then refines coordinates, colours and gradient stops with L-BFGS to
convergence (not a fixed iteration count), from multiple restarts, against
`DISTS + multiscale gradient loss` rendered at **1x and 4x** — an SVG must be correct
when zoomed, and single-resolution losses systematically over-fit to source resolution.

#### Amendment — how much continuous polish is worth (measured)

Far less than this section assumed, and the reason generalises, so it is recorded here
rather than left in the code.

The premise above is that the fit gives back sub-pixel accuracy which consulting the
image can recover. That premise was tested directly
(`crates/inkvec-trace/tests/polish_headroom.rs`): sample the coverage evidence along the
curves the fitter actually produces, over real emoji, and ask what displacement it claims.

```
  mean offset   +0.0169 px      <- bias; what any polish can remove
  std dev        0.3897 px      <- noise; what it cannot
  bias / noise    0.043
```

The fitted curves are already **centred** on the boundary. There is no systematic
displacement left to remove, because S2 snapped every contour point onto the `0.5`
coverage level *before* fitting, and the fit's least squares then averaged them — which is
a better estimate of the boundary than any single probe of the image can be. The 0.39px
spread is part real modelling error at corners and part extraction noise, and a local
probe cannot separate them.

The consequence is a rule for this stage: **it pays only where it averages.** Solving a
cubic's two control points against ~24 samples yields a small consistent gain. Anything
with less averaging loses. A stage that moved the joins between segments was built twice
— once from a single probe at the join, once as a weighted least squares over both
adjacent segments — and lost on real and synthetic corpora both times; the least-squares
version being the better of the two confirms the diagnosis rather than rescuing the idea.
Neither is in the tree.

Measured over 200 emoji, paired against no polish at all:

| | better | worse | sign test |
|---|---|---|---|
| DISTS | 104 | 79 | p = 0.076 |
| ΔE00 | 153 | 27 | p ≈ 1e-20 |

So the stage ships, justified by **re-rendered colour accuracy**, not by DISTS, which is
a wash. The wider lesson for the roadmap: the remaining error is not a misplaced
boundary, so accuracy work should go to the objective's exchange rate between fidelity
and parameters (§5.0), not to refining coordinates against the image.

### S6 — Structure and emit

Semantic grouping and layer naming (VLM over faces). `<defs>` + `<use>` for symmetry and
repeated elements. Optional text detection to real `<text>`, or at minimum a labelled
group. Coordinate precision chosen by **rendered error budget**, not a fixed decimal
count. Export policy — stacked | cutout | grouped — chosen at write time from the same
region graph. SVGO-equivalent cleanup last.

### 5.7 The quality ladder

One concession, and it is a development affordance rather than a compromise of the
directive:

| level | S2 | S4 | S5 | use |
|---|---|---|---|---|
| `draft` | per-pixel inversion | greedy | none | **benchmark iteration only** |
| `good` | joint LM, coarse | DP, cubic+line | continuous only | interactive preview |
| `best` | joint LM | full DP, all models | structured search | **default** |
| `exhaustive` | joint LM + SR | budget-indexed DP | search to convergence, multi-restart | publication / hero assets |

`best` is the default and the thing we claim. `draft` exists because a benchmark sweep
that takes a day per revision stops informing development — M0 already runs 720 cells,
and M1 will run more. It is never the shipped default, and the harness records the
level with every row so the two can never be silently compared.

---

## 6. Stack

**Rust core** — geometry, planar map, fitting, export. Compiles to WASM, which gives a
free, private, offline, in-browser product that Vectorizer.AI structurally cannot match.

| Crate | Role | License |
|---|---|---|
| `kurbo` | Bézier fitting (`fit_to_bezpath_opt`), path algebra | MIT / Apache-2.0 |
| `robust` | Shewchuk adaptive-precision predicates — **exact** topology decisions in S3 | MIT / Apache-2.0 |
| `nalgebra` + `argmin` (or `levenberg-marquardt`) | non-linear least squares for S2 analysis-by-synthesis and S4 orthogonal-distance fitting | permissive (verify) |
| `tiny-skia` / `vello` | rasterization for round-trip validation | permissive |
| `resvg` / `usvg` | SVG parse + render for the eval harness | MPL-2.0 (verify) |
| `palette` | OKLab / ΔE00 | permissive |
| `rayon` | parallelism | MIT / Apache-2.0 |
| `ort` | ONNX Runtime — in-process inference; keeps the single-binary and WASM story | MIT |
| `pyo3` + `maturin` | Python bindings | permissive |

**Python side** — training only (PyTorch): the S2 SDF predictor, the S4 anchor field, S5
losses. Ship as ONNX; the Rust core never depends on a Python runtime at inference time.

**License hygiene:** Potrace is **GPL** — study the paper, do not vendor the code. VTracer
is MIT and may be vendored or referenced. Verify `resvg`'s MPL terms and diffvg's license
before taking a dependency.

**A note on the `robust` caveat:** Shewchuk's predicates do not handle exponent overflow,
so inputs outside roughly 1e-142 .. 1e201 are not guaranteed. Image-plane coordinates are
nowhere near those bounds, so this does not constrain us — but normalize coordinates on
input rather than relying on it implicitly.

**Crate layout**

```
inkvec-core     region graph, DCEL, constraints
inkvec-trace    S0-S3
inkvec-fit      S4   (kurbo, primitives, symmetry)
inkvec-render   rasterization, differentiable-polish hooks
inkvec-io       SVG / EPS / PDF emit, export policies
inkvec-cli      binary
inkvec-py       PyO3 bindings
inkvec-wasm     wasm-bindgen
bench/          the evaluation harness (§7) — Python, see note below
```

**Amendment (M0, built):** the harness is Python, not a Rust crate as originally
sketched. Two reasons, both decisive. The perceptual metrics that define fidelity here
(LPIPS, DISTS) exist only as PyTorch models, and the baselines we must measure against
ship as Python packages — `vtracer`, `potracer`, and, already present in this
environment, `starvector`. Forcing the harness into Rust would have meant
reimplementing or subprocessing all of it for no benefit.

The split still respects the project's stack philosophy, just at a different seam:
Python does perception and orchestration, Rust will do geometry. The structural and
editability metrics (`svgmodel.py`, `metrics/editability.py`) are pure geometry and
**should be ported into `inkvec-core` once it exists** — at which point the engine can
enforce them as internal invariants (a planar map that produces a seam is a bug, not a
low score) rather than only being graded on them from outside.

---

## 7. Evaluation harness — build this first

A SOTA claim is only as good as the benchmark behind it, and no existing benchmark
measures editability. This is a contribution in its own right.

**Corpora**
- *Ground-truth-paired*: render Twemoji / Noto Emoji / Fluent Emoji / Material / FluentUI
  SVGs to raster at 32 / 64 / 128 / 256 / 512 px. Exact GT paths, anchor counts and
  primitives — so we can measure *structural* error, not just pixel error. Include a
  deliberately anti-aliased low-res tier (≤128px): the hardest and most commercially
  common case.
- *In-the-wild*: real logos (PNG/JPEG, including compressed and rescanned), plus
  VectorGym's GitHub-sourced SVGs for higher-order-primitive coverage.
- *Adversarial*: boundary-perturbed masks, replicating AnchorFlow's robustness protocol.

**Fidelity metrics** — DISTS and LPIPS at 1× / 4× / 16×; ΔE00 mean and p95; SSIM and PSNR
for comparability with published numbers.

**Structural metrics** (against GT paths, where available) — anchor count ratio; primitive
recall (did we recover the circle that really was a `<circle>`?); path count; segment-type
distribution.

**Editability score** — new; define rigorously and publish the definition:
- anchors per unit boundary length
- **primitive fraction**: geometry expressed as circle/ellipse/rect/arc/`<use>` vs. raw cubic
- **seam area**: rendered pixels where an adjacent-region gap exposes background
- **overdraw ratio**: Σ path areas ÷ covered canvas area
- self-intersection count
- layer-tree depth, group count, named-group fraction
- **edit locality**: perturb one anchor — how much of the render changes, and does a shared
  boundary move coherently or tear?

**Robustness** — Δ(anchor count) under boundary noise. AnchorFlow's +2.9% is the bar;
VTracer's +106.7% is the floor.

**Protocol** — report **Pareto curves** (quality vs. anchor budget), never single points.
Every method has a quality knob, and comparing one operating point each is how this field
currently misleads itself. Baselines: VTracer (several settings), Potrace (bilevel
control), Illustrator Image Trace, SuperSVG, and **Vectorizer.AI via its paid API** as the
ceiling. A human preference study for the final claim — pixel metrics do not capture
"looks right".

---

## 8. Roadmap

| Milestone | Content | Outcome |
|---|---|---|
| **M0** ✅ **done** | `bench/`: synthetic GT corpus, fidelity + structural + editability metrics, VTracer/Potrace/Vectorizer.AI runners, Pareto + robustness protocol | Built and run. Baseline measured — see [`M0-BASELINE.md`](M0-BASELINE.md). |
| **M1** (~8 wks) | S0 image-model estimation, S1 region proposal, S2 analysis-by-synthesis boundaries, S3 exact planar map, S4 multi-model MDL dynamic program **including primitives and symmetry** | **Should beat VTracer decisively on flat art.** No GPU. Longer than the pre-directive estimate because the DP alphabet and the joint solve replace three cheaper stages. |
| ~~M2~~ | **folded into M1.** Primitives and symmetry are members of the S4 segment alphabet, selected by the global optimization. They cannot be a later pass: once cubics are fitted they have already absorbed the error a primitive would have explained. | This was the single largest structural consequence of the quality-first directive. |
| **M3** (6 wks) | Learned S2 SDF + S4 anchor field (synthetic self-supervision); S5 polish | Robustness to noisy and compressed input; the AnchorFlow lesson. |
| **M4** | Semantic layering and naming (VLM), text detection; opt-in amodal peeling | Beyond Vectorizer.AI, not just parity. |
| **M5** | WASM build, browser demo, API | Free / private / offline — the structural advantage. |

M1 is the go/no-go, and M0 has made it falsifiable with numbers rather than impressions.
The targets, against measured VTracer baselines, are in
[`M0-BASELINE.md`](M0-BASELINE.md) §"What M1 must beat": parameter blow-up 5.9× → <2×,
primitive recall 0.00 → >0.80, mosaic overdraw 1.64 → ~1.00, seam@4x 0.216% → 0.00%,
sub-pixel strokes recovered 2/11 → 11/11, parameter growth under perturbation
+18.0% → <5%.

Note M1 does **not** need to win on DISTS. VTracer's fidelity is already decent here
(median DISTS@4x ≈ 0.05); it buys that with 6x the parameters, zero primitives and 64%
redundant geometry. The claim to prove is *equal or better fidelity at a fraction of the
structural cost*.

---

## 9. Risks and open questions

*Revised under the quality-first directive. Three risks shrank, two grew, one is new.*

**Reduced by the revision**

1. ~~Sub-pixel inversion is ill-conditioned at low contrast.~~ **Largely dissolved.** In a
   joint analysis-by-synthesis solve (S2), a poorly-conditioned pixel simply carries little
   weight and the boundary is pinned by better-conditioned neighbours. What remains is the
   need to propagate the posterior covariance honestly into S4's tolerance.
2. ~~Triple points break the two-colour mixture model.~~ **Dissolved.** Three regions meeting
   in one pixel is an ordinary three-way mixture in the forward model. It needed a special
   detector only because the cheap per-pixel method could not express it.
3. ~~Symmetry enforcement can be wrong.~~ **Reduced, not gone.** Acceptance is now by MDL
   against S2's posterior covariance rather than a hand-set threshold, so a symmetry is
   adopted only where the evidence supports it. Still needs a user override: a logo can be
   near-symmetric by intent *or* by accident, and no amount of evidence distinguishes those.

**Grown**

4. **The multi-model DP is now the critical path, and it is genuinely hard.** Potrace's
   O(n²)/O(nm) analysis assumes integer lattice paths; the straightness criterion must be
   redefined for continuous coordinates and the complexity argument re-derived. The
   directive makes this *worse*, not better: the alphabet grows from one model to four and
   the DP state now carries a tangent constraint, so the state space multiplies. M0 raised
   this item's priority, though **not for the reason first given**: a controlled experiment
   (M1-PROGRESS §4) shows non-locality does *not* buy the noise robustness — a greedy
   segmenter under the same `tau*sigma` admissibility envelope is equally stable. The DP is
   justified instead by **fit quality at a fixed parameter budget** (8.5% less residual on a
   circle, 39.8% on a star, at matched segment count) and by being the structure the
   multi-model alphabet plugs into without redesign. The risk is real research risk.
   **Mitigation:**
   build it single-model first, verify it reproduces Potrace's robustness number on the
   bilevel corpus, then widen the alphabet one model at a time with the benchmark watching.
5. **Runtime.** The honest statement of the directive's cost: joint LM per boundary, O(n²)
   DP with four models, orthogonal-distance fitting inside each candidate segment, and a
   structured search at the end. Seconds to minutes per image, not milliseconds. This is
   accepted, and §5.7's `draft` level exists so benchmark iteration does not stall — but if
   `best` reaches minutes on a typical logo, the WASM/browser story in M5 weakens and
   should be re-scoped to `good` rather than quietly shipped slow.

**New**

6. **The MDL λ is now load-bearing.** Collapsing fidelity and editability into one objective
   is the right move, but it concentrates a great deal into one constant. λ sets the
   exchange rate between a unit of residual and a unit of description length, and there is
   no principled prior value. It must be calibrated empirically against the benchmark's
   Pareto sweep and against human preference — not guessed. A mis-set λ will look like a
   *pipeline* failure while actually being a units problem.

**Unchanged**

7. **Unknown AA kernel.** Reduced by estimating θ in S0 rather than assuming it, but blind
   estimation can still fail on heavily-processed input. The Subpixel Deblurring paper is
   the strongest prior art — check whether its code has shipped.
8. **Fréchet ignores curvature error** — Levien's own caveat. Orthogonal-distance fitting
   addresses positional accuracy but not tangent/curvature continuity across joins;
   smoothness may need a supplementary term in the DP cost.
9. **Amodal inpainting invents content.** Never default-on for logos.
10. **Moving target.** Illustrator shipped gradient detection, anchor reduction and auto
    grouping in Oct 2024, and Concept to Vector in May 2026. Parity with 2026 Vectorizer.AI
    is not the goal; semantic layering and local execution are where we can lead.

## 9.5 What actually limits quality — measured, 2026-08-31

Everything below came from scoring the full 180-image real corpus and cross-tabulating
against ground-truth SVG features, rather than from reading outputs and forming
impressions. Three of the conclusions reverse something previously believed.

### The binding constraint is translucent gradients, and it is an interaction

Gradient count predicts DISTS far better than complexity does, and the two are
independent:

```
  correlation DISTS vs gradient count      0.427
  correlation DISTS vs ground-truth params 0.139
  correlation gradient count vs params     0.084   <- independent
```

The penalty survives stratification by complexity in every bin (worst in 600-1200 params:
0.0666 -> 0.1494). But it is not gradients as such, and not stop count either:

```
  grad=0 opacity=0 : 0.0648  n=121
  grad=0 opacity=1 : 0.0664  n=11    opacity alone costs nothing
  grad=1 opacity=0 : 0.0545  n=5     gradients alone cost nothing
  grad=1 opacity=1 : 0.1392  n=43    together, 2.1x worse
```

The construct behind that cell is Illustrator's soft-shadow idiom — a single-colour
`stop-opacity` ramp from 0 to 1, on a *unit circle* (`r="1"`) positioned entirely by a
`gradientTransform` matrix. 44 of the 48 gradient-bearing images carry such a transform;
only 4 do not.

Why it hurts is structural, and it is a document-model limit rather than a fitting one.
We emit an **opaque partition**: every pixel belongs to exactly one face with one fill. A
translucent overlay lying across several differently-coloured regions composites to a
*different* ramp over each one, so a partition must represent it as one gradient per
(base region x overlay) intersection. That is simultaneously more parameters and worse
colour — which is exactly the measured signature. Representing it once, as a translucent
shape over a base, is what `alpha.rs` was written for and has never been wired to.

Priority order by measured coverage: `gradientTransform` (44/48), `stop-opacity` layering
(43/180, the 2.1x cell), then multi-stop offsets (31/48, worth about 7% beyond the other
two — among gradient images, 2-stop-only medians 0.1235 against 0.1323 for multi-stop).

### Upscaling as a pre-process: no

Tested three arms at 4x — direct, Lanczos, and *ground truth rendered at 512*, which is a
perfect upscaler that no model can beat. Lambda was held constant across arms.

```
  real emoji, paired vs direct
    lanczos  DISTS -0.0114 (6/1)   dE00 +0.077 (2 better/5 worse)   params +2.22x
    oracle   DISTS -0.0359 (7/0)   dE00 -0.186 (7/0)                params +0.81x

  synthetic
    lanczos  DISTS +0.1157 (0 better / 4 worse)   params ratio +94.5
      gradient_linear  0.0074 -> 0.2239    511 ->  15504 bytes
      gradient_radial  0.0082 -> 0.2258   1422 -> 100720 bytes
```

Naive interpolation is decisively harmful: ringing overshoots *past* both inks, so those
pixels are not between two colours and `is_blend` cannot reject them. The palette
explodes, and with it the face count and the O(n^2) contour DP — one Lanczos trace ran for
eleven minutes.

A perfect upscaler does help, but the gain is largely a parameter-budget effect and the
arithmetic says so: at 4x resolution chi-squared has four times the sample points while
`lambda * params` is unchanged, so

```
  0.5*(4*chi2) + lambda*p  =  4*( 0.5*chi2 + (lambda/4)*p )
```

**Tracing at 4x is arithmetically the same as dividing lambda by 4.** Upscaling does not
add information; it manufactures correlated samples and thereby inflates the fidelity term
until the objective buys more parameters. The one genuine exception is clean synthetic
gradients, where the oracle scored 5x better DISTS at unchanged size (511 -> 512 bytes) —
there the 128px sampling of a smooth ramp really is the limit.

### Lambda cannot buy fidelity, and that is correct

Sweeping precision from 0.02 to 45 moves parameters 683 -> 1062, and DISTS gets *worse*
past about 0.6 (0.0951 -> 0.1297). `precision` sets both lambda and the emitted decimal
count, and that coupling is not an oversight: a coordinate quantised to `d` genuinely
costs `ln(extent/d)` nats. Buying more parameters requires cheapening coordinates, and the
two effects cancel. The objective is self-limiting in the right way — which is also why
the upscaling gain is not reproducible by lowering lambda.

An earlier sweep of precision 0.02 to 0.6 found every arm within 2% on parameters and
looked flat. That range is lambda 8.76 to 5.36, a factor of 1.6, and far too narrow to say
anything.

### Where we stand against VTracer

Pareto over 40 emoji, both engines swept, both scored identically at 128px:

```
  arm                        params    DISTS    dE00
  inkvec p=0.6                  730   0.0951   0.858   <- frontier
  inkvec p=0.05                 693   0.0967   0.915   <- frontier
  vtracer spline-ct60-lt8      1200   0.1178   2.136
  vtracer cp8-ld8-fs2          2622   0.1108   2.018   <- vtracer's best fidelity
```

Our 730-parameter output beats VTracer's best-fidelity 2622-parameter output on both
fidelity and colour: 3.6x fewer parameters, better DISTS, 2.4x better dE00. Every VTracer
configuration is dominated except its cheapest, which survives only by being smaller while
scoring 2.4x worse. Reporting single operating points, as section 7 warns, would have
hidden this in both directions.

---

## 10. Bibliography

**Classical**
- Selinger, *Potrace: a polygon-based tracing algorithm*, 2003 — https://potrace.sourceforge.net/potrace.pdf
- VTracer — https://github.com/visioncortex/vtracer

**Curve fitting and optimal segmentation**
- Levien, *Fitting cubic Bézier curves*, 2021 — https://raphlinus.github.io/curves/2021/03/11/bezier-fitting.html
- Levien, *Simplifying Bézier paths*, 2023 — https://raphlinus.github.io/curves/2023/04/18/bezpath-simplify.html
- `kurbo::fit_to_bezpath_opt` — https://docs.rs/kurbo/latest/kurbo/fn.fit_to_bezpath_opt.html
- *Optimal Compression of a Polyline with Segments and Arcs*, arXiv:1604.07476 — the
  line+arc DP that S4 generalizes
- *Segmentation and multi-model approximation of digital curves*, Pattern Recognition
  Letters — MDL-driven multi-model DP
- *Minimum Description Length approximation of digital curves* — the MDL cost formulation

**Geometric fitting (S4 continuous inner solve)**
- Ahn, *Least Squares Orthogonal Distance Fitting of Curves and Surfaces in Space* —
  orthogonal vs. algebraic fitting; algebraic carries a high-curvature bias
- Ahn et al., *Least-squares orthogonal distances fitting of circle, sphere, ellipse,
  hyperbola, and parabola*, Pattern Recognition 2001
- Taubin — algebraic fit, used only to initialize the orthogonal-distance refinement

**Exact computational geometry (S3)**
- Shewchuk, *Adaptive Precision Floating-Point Arithmetic and Fast Robust Geometric
  Predicates* — https://github.com/georust/robust

**Sub-pixel**
- Yang et al., *Subpixel Deblurring of Anti-Aliased Raster Clip-Art*, CGF 2023 — https://www.cs.ubc.ca/labs/imager/tr/2022/SubpixelDeblurring/

**Learned structure**
- *AnchorFlow: Editable SVG Reconstruction via Sparse Anchor Point Fields*, arXiv:2605.19551
- Zhao et al., *AdaVec: Less is more — efficient image vectorization with adaptive parameterization*, 2025
- *SuperSVG*, CVPR 2024 — arXiv:2406.09794
- *Optimize and Reduce*, arXiv:2312.11334

**Semantic layering**
- *LayerPeeler*, arXiv:2505.23740 — https://layerpeeler.github.io
- *AmodalSVG*, arXiv:2604.10940

**Gradients**
- Du et al., *Image vectorization and editing via linear gradient layer decomposition*, SIGGRAPH 2023
- Chakraborty et al., *Image Vectorization via Gradient Reconstruction*, CGF 2025
- *Hierarchical Diffusion Curves*, TOG 2014

**Benchmarks**
- *VectorGym*, arXiv:2603.29852
- *VectorEdits*, arXiv:2506.15903 · *SVGenius*, arXiv:2506.03139

**Commercial**
- Vectorizer.AI — https://vectorizer.ai/ · https://vectorizer.ai/about
