# Chain R: representation, informally

> From the boundary chain's likelihood to the fewest coordinates a person would write. What
> the objective should be and why (R0); constraints and the free coordinates they leave (R1);
> which constraints hold (R2); the human prior, counted from the artists' own files (R3);
> structure and encoding (R4). Each link states its problem, its claims and an informal
> proof of each, or a gap marked as one with what would close it. The numbers the prior needs
> are measured on the corpus (`bench/theory/design_prior.py`). The interface this chain
> needs from the boundary chain is proposed at the end, with the plan for the formal
> development and the code.

Companion pages: [`chains.md`](chains.md) (the split into two chains),
[`chain-boundary.md`](chain-boundary.md) (the boundary chain, whose likelihood this chain
consumes), [`optimal.md`](optimal.md) (the optimum as the most probable SVG; the
search/model diagnostic), [`README.md`](README.md) (what the pixels determine, proved in
[`formal/InkvecTheory`](../../formal/InkvecTheory/README.md)).

## Summary

* **The objective is the posterior probability of the design, with its free coordinates
  integrated out.** A design has a discrete part (elements, segment kinds, paint order,
  which constraints hold, which numbers are tied or on the design grid) and the continuous
  coordinates the constraints leave free. Under a loss that counts structural mistakes
  (a missed corner, a wrong segment kind, a broken constraint: what the gate's geometric
  match counts) the best decision is the most probable discrete part, and its probability
  is the prior times the *evidence*, the likelihood integrated over the free coordinates
  (R0.1). The evidence prices each free coordinate at `ln(W/(√(2π)·σ))`, set by how
  precisely the pixels fix it, and each constraint at the log prior odds the corpus gives
  it (R0.2). Today's `½χ² + λ·P` is the special case with one precision for every
  coordinate and a flat prior (R0.3).
* **The lexicographic rule is the high-resolution limit of that objective, proved
  informally (R0.5).** As the resolution grows, a design that cannot draw the truth loses
  by a misfit that grows at least as the square of the resolution; among designs that draw
  it, each extra free coordinate costs a term growing as its logarithm; the prior stays
  bounded. So the order "fit within the noise, then fewest free coordinates, then the most
  human" of [`chains.md`](chains.md) is what the posterior converges to. It is kept as the
  search order and as a check; at finite resolution, and decisively at 128 px where
  [`optimal.md`](optimal.md) §6 finds the objective preferring the trace on 117 of 246
  icons, the posterior's own trade decides.
* **Imposing a constraint that is slightly false costs at most what the test let through
  (R0.4).** The constrained fit's error is, for every noise, the noise kept in the smaller
  model plus the constraint's violation. A constraint lowers the expected error exactly
  when its violation is under one standard deviation per removed coordinate. This extends
  `natural_model_closer` from true constraints to false ones and bounds the fidelity cost
  of every decision of R2 by that decision's own threshold.
* **Free coordinates are counted by rank, and most icon constraints are linear (R1).**
  Ties, axis alignment, mirror pairs and translated copies are linear equations with
  coefficients 0 and ±1, so their rank is exact (a tie graph with `c` components leaves
  `c` values); a finite symmetry group leaves the average of its characters; nonlinear
  constraints (tangency, incidence on a curve) are counted at the fitted point with a
  rank decision certified by a singular-value gap (Weyl). The gate's count is not the
  free-coordinate count (it charges an arc 7 numbers for 3 free ones and a subpath's start
  nothing), so the objective uses free coordinates and the encoder minimises the gate's
  count separately (R4.6).
* **Which constraints hold is a Bayes-factor test with stated error rates (R2).** Accept
  when `½Δχ²` is under the removed coordinates' price plus the log prior odds. A true
  constraint is then rejected with probability `P(χ²₁ > 2τ)` (5·10⁻⁴ at a threshold of 6
  nats), a false one accepted only when its violation is within `√(2τ)` standard
  deviations (3.5σ at 6 nats), and both rates go to zero as the resolution grows.
  Accepted in order of evidence, each tested against the fit under the ones already
  accepted, the set is never contradictory. Ties are found exactly by a one-dimensional
  dynamic program (equal noise makes the optimal tie classes contiguous), the grid on the
  tie classes rather than on the coordinates.
* **The prior is a few hundred numbers counted from 1,408 artist files, and its main
  finding is structural (R3).** The design grid governs *construction* points: 95.5 % of
  lucide's line–line corners and 100 % of its primitive numbers lie on the half-unit grid,
  but only 27 % of the points where its arcs meet, which are intersections the artist
  computed and rounded. Artists' constraints are exact or clearly absent: 66.8 % of
  lucide's lines and 72.3 % of material's are exactly axis-aligned against 0.0 % and 0.5 %
  within half a degree of it; joins between curves are smooth (within 2°) 69 % of the time
  and corners (over 30°) 18 %, with 7 % between. Under this prior a coordinate of a grid
  family costs 5.3 nats instead of the flat 8.5. In simulation at the boundary chain's
  calibrated precision (σ = 0.005 px), the hierarchical prior (ties, then the grid by role)
  returns 73–75 % of lucide's and material's numbers *exactly*, which is all of their
  on-grid numbers, snaps 0–0.3 % wrong, and cuts the positional error by 77–78 %; ties are
  recovered with pairwise precision 0.98–1.00 and recall 1.00.
* **Structure decomposes exactly once the paint order is fixed (R4).** With a back-to-front
  order, a set of shapes paints the traced picture if and only if each shape lies between
  its visible part and its visible part together with everything painted above it; these
  conditions involve no other shape, so each face's shape is chosen alone, and inside that
  interval the pixels are indifferent, so the prior alone chooses (amodal completion as
  the simplest shape in an interval, invisible by construction). Completion also removes
  the compositing seam that `seams.rs`'s underlap works around. In the emoji families
  37–56 % of the elements are partly covered by a differently painted element, and they
  hold 44–47 % of the artist's parameters: the largest parameter share the trace cannot
  reach today.
* **The interface is a measurement model, not a fitted map, and the two chains agree on
  it.** The boundary chain's `Evidence` and `BoundaryLikelihood`
  ([`chain-boundary.md`](chain-boundary.md)) score any description exactly by window areas
  with calibrated variances; chain R adopts
  them, asks that the evaluator draw every candidate exactly as the corpus's renderer
  does (the likelihood separates an arc from its four-cubic approximation by about fifty
  standard deviations at 512 px), and takes over corners, continuations, gradients and
  strokes as its own decisions. R computes the precision of its own parametrisation from
  the same terms, so the boundary chain need not.

## 0. The problem, and notation

**In.** The boundary chain's output (B4): a planar map (faces, edges between two faces,
vertices) and a likelihood: for any picture `S` drawn by a candidate description, a
negative log-likelihood `ℓ(S) = −ln P(R | S)` of the raster `R`, as a sum of local terms,
one per graph-like run of an edge and one per vertex window (see the interface proposal).
`χ²(S) = 2ℓ(S) + const`.

**Out.** An SVG document.

**A design** `D = (T, θ)`:

* `T`, the **discrete part**: the elements and their kinds (path, circle, rect, stroke or
  fill), their paint order, each boundary's segment kinds (line, arc, cubic) and joins, the
  set `C` of design constraints that hold (R1's vocabulary), and the discrete labels of
  coordinates (which grid point a coordinate sits on, which coordinates are tied to one
  value);
* `θ ∈ ℝ^k(T)`, the **free coordinates**: a chart of the set of raw coordinates
  satisfying `C`, of dimension `k(T)`, the number of free coordinates.

It draws a picture `S_T(θ)` and is written as an SVG `enc(T, θ)`. Three counts of one design
differ and must not be confused: the raw coordinates `n(T)` of its segments and primitives;
the free coordinates `k(T) = n(T) − rank J_C` (R1); and the **gate's count** `w(enc)`, the
numbers `bench/inkvec_bench/svgmodel.py` charges: line 2 (also when written `H`/`V`), quadratic
4, cubic 6 (also `S`), arc 7, `<circle>` 3, `<ellipse>` 4, `<rect>` 6, `<line>` 4, `<use>` 6,
geometry in `<defs>` once, and nothing for a subpath's start point, a transform, a stroke
width or a colour.

**The prior** `P(T)·h_T(θ)`: a probability for each discrete part and a density on its
free coordinates. **The posterior** of the discrete part:

```text
P(T | R)  ∝  P(T) · ∫ exp(−ℓ(S_T(θ))) · h_T(θ) dθ          (the evidence of T)
```

Coordinates in this page are in design units unless said otherwise; `s` is the resolution
in pixels per design unit (128 px for a 24-unit icon: `s = 5.33`).

## R0. The objective

### R0.0 The candidates

Seven ways to choose among descriptions of the same pixels, and what each gets wrong or
right.

1. **Flat description length** (inkvec today): `½χ² + λ·P`, `λ = ln(extent/precision)`
   (`FitConfig::from_precision`, `cost.rs`), the two-part code of Rissanen (1978). Right in
   form. It prices every coordinate the same, however precisely the pixels fix it, and it
   has no prior: a horizontal line costs what a skew one costs, a repeated radius what a new
   one costs.
2. **Lexicographic**: fit within a `χ²` tolerance, then fewest free coordinates, then the
   prior. Potrace orders (segments, penalty) this way. It states the goal directly, but its
   tolerance is a hypothesis-test threshold chosen without the prior, it cannot trade a
   slightly worse fit for a much more probable structure, and it is discontinuous in the
   data. R0.5 shows it is the limit of the posterior at high resolution, which is where its
   virtues come from.
3. **Bayesian model selection with learned priors** (Bayes factors, the Laplace
   approximation; Kass & Raftery 1995; MacKay 1992), or its coding twin, minimum message
   length with the Wallace–Freeman approximation (Wallace & Freeman 1987): the evidence of
   each discrete structure, the free numbers integrated out. This is the choice below.
4. **Rate–distortion**: fewest numbers subject to a perceptual error bound. A good loss for
   colour error alone, but it recovers the artist's design only by accident: among
   descriptions under the bound it has no reason to prefer the one the artist drew.
5. **Constraint inference with fixed tolerances**, as in drawing beautification (Pavlidis &
   Van Wyk 1985; Igarashi, Matsuoka, Kawachiya & Tanaka 1997) and relation discovery in
   primitive fitting (GlobFit: Li, Wu, Chrysanthou, Sharf, Cohen-Or & Mitra 2011): detect
   near-relations, impose them, re-solve. The right moves, with uncalibrated tolerances: the
   tolerance should depend on how precisely the pixels fix the quantity and on how often
   people use the relation, which is what R2's test gives it. `editable.rs` is a
   beautifier of this kind (`3σ` plus a slack).
6. **A learned generative prior** (DeepSVG, Carlier et al. 2020; StarVector): excluded by
   the requirement of a small model, and not needed: R3 shows the statistics that decide
   are a few hundred counts.
7. **Program synthesis over a grammar** (Ellis, Ritchie, Solar-Lezama & Tenenbaum 2018;
   Szalinski, Nandi et al. 2020, which recovers loops and symmetry in CAD by equality
   saturation): the right search space for repetition and symmetry (R4.5); its objective is
   still a choice among 1–5.

### R0.1 Claim: the Bayes decision is the most probable discrete part

**Claim.** Under the loss "1 if the discrete part differs from the artist's, else 0", the
estimate minimising the expected loss is `argmax_T P(T | R)`. Under the Hamming loss that
counts mismatched components of `T` (each corner, each segment kind, each constraint), it is
the component-wise posterior mode, which coincides with the joint mode whenever the joint
mode has posterior probability above ½.

**Proof.** The expected 0–1 loss of `T̂` is `1 − P(T̂ | R)`. For the Hamming loss the
expected loss is a sum over components of `1 − P(T_i = T̂_i | R)`, minimised component by
component. If `P(T* | R) > ½` then every component has `P(T_i = T*_i | R) ≥ P(T* | R) > ½`, so
`T*_i` is that component's mode. ∎

The gate's geometric match (corner precision and recall, segment-type agreement) is a
Hamming-type loss on `T`, so this is the objective the gate's newest axis rewards, and the
condition says when the joint mode, which a search can find, is also the component-wise
optimum.

### R0.2 Claim: the evidence prices each free coordinate by its precision

**Claim.** Let the picture be affine in `θ` near the best fit (or linearised there), the
noise Gaussian with covariance `Σ`, so `χ²(θ) = χ²_min + (θ − θ̂)ᵀ F (θ − θ̂)` with Fisher
information `F = AᵀΣ⁻¹A`, and the prior a density `h` that is nearly constant where the
likelihood is not negligible. Then

```text
−ln P(R | T) = ½ χ²_min(T) + ½ ln det(F_T / 2π) − ln h_T(θ̂) + const(R)
```

and in coordinates in which `F_T` is diagonal, `diag(1/σ_i²)`, with `h` uniform of width `W_i`
along each, the cost of the design is

```text
L(T) = ½ χ²_min(T) + Σ_i ln( W_i / (√(2π)·σ_i) ) − ln P(T).
```

The constrained case uses the same formula with `F_T = Nᵀ F_raw N`, `N` an orthonormal basis
of the constraint set's tangent space (the null space of the constraint Jacobian `J_C`):
the information the pixels carry about the directions the constraints leave free.

**Proof.** `∫ exp(−½(θ − θ̂)ᵀF(θ − θ̂)) dθ = (2π)^{k/2} det(F)^{−1/2}`, and the rest is the
logarithm. On a constraint manifold, a chart `θ ↦ x(θ)` with orthonormal tangent vectors at
`x̂` gives `∂x/∂θ = N` and `F_T = Nᵀ F_raw N`. For a nonlinear model the formula is
Laplace's method, with relative error `O(1/m)` in the number of measurements (Tierney &
Kadane 1986). ∎

Two consequences. A coordinate the pixels pin down more precisely costs *more* to state,
because more of its digits are information: from 6.2 nats (σ = 0.1 px) to 8.5 nats (σ = 0.01
px) at 128 px, 7.6 to 9.9 at 512 px, for `W` the canvas; at the boundary chain's calibrated
precisions (0.0004–0.04 px, `chain-boundary.md` B4.3) about 7–13 nats. And a coordinate
that is not free costs nothing here; what it costs instead is the prior of the constraint
that fixed it, in `−ln P(T)`.

### R0.3 Claim: inkvec's objective is the special case with one precision and no prior

**Claim.** `E = ½χ² + λ·P` with `λ = ln(extent/precision)` is R0.2 with every `W_i` the
canvas, every `σ_i = precision/√(2π)`, `P(T)` uniform, and `P` the free-coordinate count.

**Proof.** Substitute. ∎

So the present objective is right in form and wrong in two terms. Its precision is global
(0.1 px), where R0.2 asks for each coordinate's own (B4's), so it over-prices coordinates
the pixels fix loosely and under-prices those they fix tightly by about ±1.5 nats. And its
prior is flat, where the corpus says a coordinate of a grid family is worth about 3 nats
less than a free one (R3, Table R3.3). Both errors move the choice at 128 px, where
[`optimal.md`](optimal.md) §6 measured the objective preferring the trace over the artist's
file on 117 of 246 icons.

### R0.4 Claim: a slightly false constraint costs at most what the test admitted

**Setting.** As in `Naturality.lean`: whitened measurements `y = s + ε`, the larger model's
tangent space `L`, the constrained model's `K ≤ L`, fits by orthogonal projection. Unlike
there, the truth `s` is only assumed to lie in `L`, not in `K`.

**Claim.** For every noise `ε`,

```text
‖P_K y − s‖² = ‖P_K ε‖² + ‖s − P_K s‖²,        ‖P_L y − s‖² = ‖P_L ε‖²,
```

so with isotropic unit noise the expected squared errors are `dim K + b²` and `dim L`, where
`b = ‖s − P_K s‖` is the constraint's violation in standard deviations. Imposing the
constraint lowers the expected error if and only if `b² < dim L − dim K`.

**Proof.** `P_K y − s = P_K ε − (s − P_K s)`; the first term lies in `K` and the second is
orthogonal to `K`, so their squared norms add. `P_L y − s = P_L ε` because `s ∈ L`. The
expectations follow from `noise_absorbed` (`E‖P_K ε‖² = dim K`). ∎

**Consequence for R2.** The constraint test accepts only when `Δχ² < 2τ` (R2.1), and
`E Δχ² = Δk + b²`, so an accepted false constraint has `b²` of the order of `2τ` or less: its
fidelity cost is bounded by the test's own threshold, a few standard deviations of the
removed coordinate. At 512 px that is a few thousandths of a pixel for a line's direction.
With `b = 0` this is `natural_model_closer` and `natural_model_gain`. The statement is the
mean-square criterion for restricted least squares (Toro-Vizcarrondo & Wallace 1968),
here pointwise in the noise.

### R0.5 Claim: the lexicographic rule is the high-resolution limit

**Setting.** A fixed artist's design `T_A` drawn at resolution `s`, per-pixel noise fixed,
B4's likelihood calibrated (the truth's `χ²` has its nominal distribution). Candidates are
compared by `L(T)` of R0.2.

**Claim.** As `s → ∞`, for every candidate `T`:

1. if `T` cannot draw the artist's picture (its closest picture differs by a positive area
   in design units), `L(T) − L(T_A) → +∞` at least as fast as `s²`;
2. if `T` can draw it and has `k(T) > k(T_A)` free coordinates, `L(T) − L(T_A) =
   (k(T) − k(T_A))·β·ln s + O_p(1)` with `β > 0`;
3. if `T` can draw it and `k(T) = k(T_A)`, `L(T) − L(T_A) = −ln(P(T)/P(T_A)) +
   ½ ln(det F_T / det F_{T_A}) + O_p(1)`, bounded.

So with probability tending to one, the ranking by `L` is the lexicographic ranking by
(draws the picture, fewest free coordinates, prior and an Occam ratio).

**Proof.** (1) A boundary displaced by `δ` design units is displaced by `sδ` px, which
changes each crossed column sum by about `sδ` against a column variance that does not grow,
over a number of columns proportional to `s`: `χ²` grows as `s³δ²` for independent column
noise, and at least as `s²δ²` when part of the noise is a floor correlated along the edge
(the supersampling floor of `ss_column_sum_close` is `1/(2n)` px, which shrinks as `1/s` in
design units). Every other term of `L` grows at most logarithmically. (2) Both designs draw
the truth; compare each with a common larger design `T₃` that contains both (for example
the free polyline): by `merge_chi2_increase` the `χ²` excess of each over `T₃` is the noise
in the removed directions, `χ²` with `k(T₃) − k(T)` degrees of freedom, so the difference of
fits is `O_p(1)`. Each free coordinate's Fisher information grows as a power `s^{2β}` (`β =
3/2` for a position read by `∝ s` independent columns), so its term `ln(W/(√(2π)σ))` grows as
`β ln s`. (3) The `det F` of two designs with the same number of coordinates scale by the
same power of `s`, so their ratio is bounded; the rest is (2). ∎

This is Schwarz's (1978) consistency of the evidence, read for vector graphics: the order
the chains page asks for ("fidelity is a constraint, not a currency; among those that fit,
the fewest free coordinates; among equals, the most human") is the posterior's own order in
the limit. *Not from the literature* as a statement about vectorisation. What it does not
say: at a finite resolution the terms of (2) and (3) are a few nats each and comparable with
the prior's log odds, and that is exactly where the 128 px model errors of
[`optimal.md`](optimal.md) §6 live. There the posterior decides, not the limit. The
lexicographic order remains useful as the search order (R4.3 prunes by free-coordinate
count) and as an assertion: a decision that raises `k` *and* worsens the fit is a bug.

If the artist's design is not in the vocabulary at all (a spiral; a hand-drawn wobble), no
candidate draws the truth and (1) applies to all: the limit then picks, at each resolution,
the cheapest description whose misfit is within the noise, and it grows with the resolution,
as today's fits do.

**The hypothesis that matters in practice is calibration at the truth, and it includes the
renderer.** The picture `S_T(θ)` must be drawn as the renderer that made the input draws it.
If the evaluator draws the artist's own design differently from the renderer by `Δ` px along
a boundary (an arc converted to Béziers another way, a different flattening of curves or
offsets of strokes), (1) applies to the artist's design itself: its misfit grows like
`s·(Δ/σ)²` per unit length, and the limit prefers descriptions that fit the renderer's
approximation with extra segments. The boundary chain measures how sharp this is: a true arc
and its four-cubic approximation differ by about fifty standard deviations at 512 px
(`chain-boundary.md` B4.3). Hence amendment A1 of the interface.

### R0.6 Claim: the gate ratio is at most one where the design is recovered

**Claim.** If the posterior mode recovers the artist's discrete part `T_A`, and the encoder
writes every design with the fewest gate-counted numbers among the SVG encodings of that
design (R4.6), the trace's gate count is at most the artist's.

**Proof.** The artist's file is one encoding of `T_A`; ours is the cheapest. ∎

**Gap.** Where `T_A` is not identifiable at the given resolution (a 0.3 px feature at 128
px), the mode is another design. Its free-coordinate count is no larger than that of any
equally fitting alternative (R0.5 in the large; the evidence at finite resolution), but its
gate count may exceed the artist's because the gate's count is not monotone in free
coordinates (R1.6). Closing this needs the encoder's count inside the search's tie-breaks
(R4.6), and a measurement: the per-icon gap between the posterior mode's gate count and the
artist's on icons where the artist's design is the mode.

### R0.7 The decision

**The objective of chain R is `L(T)` of R0.2: the negative log posterior of the discrete
design, its free coordinates integrated out by the Laplace (Wallace–Freeman) approximation,
with B4's likelihood and R3's prior.** Why this and not the alternatives: it is the
optimum for the loss the user's goal describes (R0.1); it contains today's objective as a
special case and corrects its two errors (R0.3); its decisions have computable error rates
and a bounded fidelity cost (R0.4, R2.2); and it reduces to the lexicographic rule exactly
where that rule is right (R0.5). It is still a description length, in nats, so every
existing comparison in the code (`ribbons.rs`'s decision, `mirror_fit.rs`'s choice, the
merges) keeps its form; what changes is the price of each coordinate and the presence of
the prior.

### R0.8 Claim: within the Laplace regime, more noise never un-accepts a constraint

Real inputs are resized and compressed ([`noise.md`](noise.md)). On damaged input the target
is still the artist's clean file. Noise should therefore make the description lean more on
the prior, never less. This claim states exactly how far that holds.

**Setting.** `M₀ ⊂ M₁` are nested descriptions. `M₁` has `k` more free coordinates (one
dropped tie, one unsnapped number: `k = 1`). `Δ ≥ 0` is the drop in the residual sum of
squares that `M₁` buys, in px². It is a property of the geometry and is held fixed while
the noise scale `σ` varies (each window's sd is `σ·g_w`). By R0.2 the log posterior odds of
`M₀` are

```text
B(σ) = c − Δ/(2σ²) + k·ln(a/σ),        a = W/(√(2π)·g),
```

where `c` is the log prior odds of the constraint (R3) and `k·ln(a/σ)` is the Occam factor of
the `k` coordinates.

**Claim.** If `B(σ₁) ≥ 0`, `σ₂ ≥ σ₁`, and at `σ₂` the coordinates still cost at least half a
nat each beyond the prior odds (`k/2 ≤ c + k·ln(a/σ₂)`), then `B(σ₂) ≥ 0`.

**Proof.** Put `t = σ₂/σ₁ ≥ 1`.

1. If `Δ ≥ kσ₂²`, the misfit is still significant at `σ₂`. Then
   `B(σ₂) − B(σ₁) = Δ(σ₂² − σ₁²)/(2σ₁²σ₂²) − k·ln t ≥ k(t² − 1)/2 − k·ln t ≥ 0`, using
   `ln t² ≤ t² − 1`.
2. If `Δ < kσ₂²`, then `B(σ₂) > c − k/2 + k·ln(a/σ₂) ≥ 0` by the regime.

∎ Lean: `Inkvec.Design.accept_monotone` (`Design/Noise.lean`).

The claim is about the acceptance set in `σ`, not about `B`'s slope:

* **`B` is not monotone.** `dB/dσ = (Δ/σ² − k)/σ`, so `B` rises while the misfit is
  significant and falls once it is within noise. Where it falls, the Occam factor is
  shrinking. Past the regime, the `k` coordinates' posterior is as wide as their prior and
  Laplace's approximation fails; the exact evidence ratio there tends to the prior odds `c`,
  not to `−∞`.
* **The literal reading is false.** "The Bayes factor for `M₀` rises with `σ` when the extra
  fit is within noise" does not hold. What holds is that the constraint, once accepted, stays
  accepted.
* **What noise does to a true constraint.** The standardised gain `Δ/σ²` has the `χ²_k`
  distribution at every `σ`, and the threshold `2k·ln(a/σ) + 2c` falls only logarithmically.
  So a true tie is refused a little more often on noisier input. At `k = 1` and 7 nats,
  `P(χ²₁ > 14) ≈ 2·10⁻⁴`.
* **What noise does to a slightly false constraint.** It is accepted while its violation `δ`
  stays under `σ_post·√(2k·ln(a/σ) + 2c)`. That acceptance radius grows with `σ`. This is the
  sense in which a noisy input snaps more.

**Heavy tails.** JPEG block steps and ringing lobes are outliers. Under a Gaussian, a
0.05 px wobble at a block edge counts as many-σ evidence against a tie, so the likelihood
needs heavy tails.

* **Huber with threshold `κ` keeps the claim on inliers, exactly.** On residuals within `κ`
  standard units the Huber loss equals the Gaussian `u²/2` (`huber_eq_sq`). An inlier at `σ₁`
  is still an inlier at every `σ₂ ≥ σ₁` (`inlier_of_le`). So whenever both fits' residuals are
  inliers at `σ₁`, the proof above applies verbatim at every larger `σ`. Outliers always cost
  less than under the Gaussian (`huber_le_sq`).
* **Huber keeps clean inputs unchanged.** The boundary chain's `huber_kappa(ν) = min(3, √ν)`
  is 3 on a clean intake (`ν = ∞`), where every residual is an inlier, so clean traces are
  untouched. `√ν` is where the Student-t influence function `(ν+1)u/(ν+u²)` peaks.
* **Student-t loses the claim.** `σ²·ρ_ν(r/σ)` with `ρ_ν(u) = ((ν+1)/2)·ln(1 + u²/ν)` increases
  with `σ`, towards the Gaussian's `(ν+1)r²/(2ν)`. The fit gain is a difference of two such
  terms, so it can grow with `σ`. This happens when `M₁`'s extra coordinate explains an
  outlier.
* **A counterexample for the t.** Take `ν = 4`, `k = 1`, `c = 0`, `a = 20`. `M₀` leaves one
  residual of 5 px; `M₁` removes it at the price of two residuals of 1 px. The t-evidence
  accepts the constraint at `σ = 0.10` (`B = +5.5`) and refuses it at `σ = 0.38`
  (`B = −0.5`). The Gaussian and Huber refuse it at both.
* **Huber shares the failure, but only on outliers.** For an outlier,
  `d/dσ[σ²ρ_H(r/σ)] = κ(|r| − κσ) > 0`, so Huber can fail the same way. Both failures need
  `M₁` to fit an outlier, and that is the case the heavy tail exists to refuse.
* **The choice.** Chain R uses Huber, with `κ` from the boundary chain's noise model.

**Consequence: a `web` trace should need no more parameters than the clean one.** Five
steps give it, with their limits:

1. Each constraint is decided by its own evidence.
2. On the same geometry, `accept_monotone` makes the accepted set at the clean noise level a
   subset of the accepted set on `web`.
3. The free-coordinate count is `n − rank` of the accepted constraints (R1.1), so it can only
   fall.
4. Blur changes the geometry itself: detail below the PSF (a corner's tip, a hairline) is
   lost. The honest description drops it, which lowers the count further. Restoring what
   blur removed is the prior's job: corners under a cover (milestone 1 below) and the
   design grid.
5. The argument is per constraint. It does not cover a greedy or joint search, it holds only
   inside the regime, and it **needs an honest `σ`**.

The last condition is the one that fails today. The engine reads one noise level per image,
while the codec's error at an edge is twenty times the interior's ([`noise.md`](noise.md)).
So the `χ²` term on `web` is inflated about 170×, constraints are refused for the wrong
reason, and the count rises. The measured web ratios below show exactly that.

## R1. Constraints and free coordinates

**Problem.** *In:* B4's map and a candidate structure (element kinds, segment kinds). *Out:*
a vocabulary of design constraints; for any set of them, the number of free coordinates,
a minimal parametrisation, and which constraints are redundant or contradictory.

**The vocabulary**, with the number of coordinates each removes (its codimension) when
independent:

| constraint | equation | removes |
|---|---|---|
| tie | `x_i = x_j` (also `y`, radii, widths, rx) | 1 |
| axis alignment | `y_a = y_b` or `x_a = x_b` for a line | 1 |
| 45° | `x_b − x_a = ±(y_b − y_a)` | 1 |
| on the design grid | `x_i ∈ o + gℤ` | 1 continuous, replaced by a discrete label |
| incidence | point on a curve; point = point is the map's topology | 1 |
| G1 join | tangents parallel at a join | 1 |
| parallel, perpendicular | direction cross or dot product zero | 1 |
| equal radius, concentric | `r_i = r_j`; `c_i = c_j` | 1; 2 |
| mirror symmetry | points paired across an axis (axis: 2 numbers) | R1.5 |
| repetition | a copy is the original translated by `t` (2 numbers) | R1.5 |
| one stroke width | ties among widths | 1 each |
| primitive | a closed boundary is a circle, ellipse, rounded rectangle | its outline's count minus 3, 4 (5 rotated), 4 (5 rounded) |

### R1.1 Claim: linear constraints are counted exactly, and ties by components

**Claim.** For constraints `Ax = b` (ties, axis alignment, 45° with a known sign, mirror
pairs about a known axis, translated copies with a known offset, concentricity), the
solutions form an affine subspace of dimension `n − rank A`. For ties alone, `rank A =
n − c` where `c` is the number of connected components of the graph whose edges are the ties,
so the free coordinates are the tie classes. A grid label fixes one class to a grid value.

**Proof.** Rank–nullity. The incidence matrix of a graph with `n` vertices and `c`
components has rank `n − c` (its null space is spanned by the components' indicator
vectors). ∎

Because the coefficients are 0 and ±1, the rank is computed exactly over the rationals:
no tolerance decides it. Most of what the corpus shows artists doing (R3) is in this class.

### R1.2 Claim: nonlinear constraints are counted at the fitted point, with a certified margin

**Claim.** (a) If the constraint map `g` has Jacobian of constant rank `r` near a solution
`x*`, the solutions near `x*` form a manifold of dimension `n − r`; a minimal
parametrisation is `n − r` of the raw coordinates, chosen by column pivoting, with the
others implicit functions of them (the *derived* coordinates). (b) Computed at the fitted
point `x̂` instead of `x*`, with `‖Dg(x̂) − Dg(x*)‖ ≤ η`, the number of singular values of
`Dg(x̂)` above `η` equals `r` whenever the smallest nonzero singular value of `Dg(x*)`
exceeds `2η`.

**Proof.** (a) The constant rank theorem (e.g. Lee, *Introduction to Smooth Manifolds*,
Thm. 4.12); the implicit function theorem gives the derived coordinates. (b) Weyl's
inequality for singular values: `|σ_i(Dg(x̂)) − σ_i(Dg(x*))| ≤ η`. ∎

`η` comes from B4's precision of `x̂` times the Lipschitz constant of `Dg`. Designs are not
generic: tangent circles, concurrent lines and symmetric tangencies sit exactly where ranks
drop. **Gap:** at such a point the gap condition of (b) fails by construction; the count is
then taken at nearby regular points (the generic rank) and the point is flagged, which is
right for counting but leaves the local solution set's shape unproved.

The derived coordinates of (a) are what the corpus calls derived points (R3.2): where two of
lucide's arcs meet, the corner is the intersection of two circles whose centres and radii
are on the grid; the artist wrote it rounded to two decimals, and only 27 % of such numbers
are on the grid. They are written, but they are not free.

### R1.3 Claim: the joins of an open boundary are independent; a closed one loses at most three

**Claim.** Order a boundary's segments `s₁ … s_m` and let each join constraint (G1,
incidence of the next segment's start) involve a parameter of the outgoing segment that no
earlier constraint involves (its start tangent for G1). Then the join constraints of an
open boundary have full row rank. On a closed boundary, constraints invariant under rigid
motions (angles, lengths, tangency) can lose at most `dim SE(2) = 3` ranks to the closure.

**Proof.** Ordered by segment, the Jacobian is block lower-triangular with a nonzero entry
on each constraint's own new parameter, so it has full row rank. A closed chain imposes
that the composition of its segments' relative rigid motions is the identity, three
equations; a set of motion-invariant constraints that determines all relative motions but
one is dependent through these three at most. ∎ The bound is stated, not sharp: the
rectangle's four right angles lose one rank (total turning), not three; R1.2 computes the
actual rank on the ring's small block.

### R1.4 Claim: redundancy and equivalence

**Claim.** Two constraint sets define the same design near `x*` exactly when their
Jacobians have the same row space there (for linear constraints: the same affine subspace).
A constraint whose row is in the span of the accepted ones removes no coordinate (`Δk =
0`): it earns no Occam credit in R0.2 and is recorded only for the encoder (an `H` instead of
an `L` whose two `y` are already tied).

**Proof.** For linear constraints, the solution set is determined by the row space and one
solution. For the nonlinear regular case, the local solution sets are manifolds whose
tangent spaces are the null spaces of the Jacobians (R1.2a). ∎

### R1.5 Claim: under a symmetry group the free coordinates are the average character

**Claim.** Let a finite group `G` (a mirror, a half turn, a rotation of order `m`) act on the
coordinate space `V` by permuting points and applying its isometry to each. The
configurations it fixes form a subspace of dimension

```text
dim V^G = (1/|G|) · Σ_{g ∈ G} tr ρ(g),
```

plus the group's own parameters (a mirror axis 2, a centre 2). For a mirror: `dim = n/2`
whatever the number of points on the axis (each pair keeps one point's two coordinates; a
point on the axis keeps one). For a half turn: `n/2 − 1` if a point sits at the centre. A
copy translated by `t` keeps only `t`.

**Proof.** The averaging operator `P = (1/|G|) Σ_g ρ(g)` is a projection onto `V^G` (it fixes
`V^G` and `P² = P`), and the trace of a projection is its rank. For a mirror, `ρ(σ)` permutes
points and negates one coordinate of each image, so its trace is the sum over fixed points
of `tr diag(−1, 1) = 0`, and `dim V^G = (n + 0)/2`. ∎

Classical representation theory; its use to count an icon's free coordinates is *not from
the literature*. `mirror_fit.rs` already fits a self-mirrored boundary on its fundamental
domain, which is this count made into a parametrisation.

### R1.6 The gate's count is not the free-coordinate count

| element | free coordinates (start given) | gate's count | numbers typed |
|---|---|---|---|
| line | 2 | 2 | 2 |
| line, axis-aligned | 1 | 2 | 1 (`H`/`V`) |
| cubic | 6 | 6 | 6 |
| cubic, G1 at its start | 5 | 6 | 6 (4 as `S` when the handle is also mirrored) |
| circular arc | 3 (end, radius; flags discrete) | 7 | 7 |
| elliptical arc | 5 | 7 | 7 |
| subpath start | 2 | 0 | 2 |
| `<circle>` | 3 | 3 | 3 |
| `<rect>` (axis-aligned; rounded) | 4; 5 | 6 | 4–6 |
| `<ellipse>` (axis-aligned; rotated) | 4; 5 | 4 | 4; 7 with `rotate(a cx cy)` |
| `<use>` of a translated copy | 2 | 6 | 2–3 |
| stroke width | 1 | 0 | 1 |

The gate over-counts arcs, rectangles and copies and under-counts starts and widths.
Measured on the artists' files, the numbers typed are 0.90 of the gate's count for material
(its `H`, `V` and 582 `S` commands) and 1.07 for lucide (its subpath starts). So the
objective (R0) counts free coordinates, which is what "the same design" means and what the
evidence prices, and the encoder (R4.6) separately writes each design with the fewest
gate-counted numbers; R0.6 connects the two where the design is recovered.

## R2. Which constraints hold

**Problem.** *In:* candidate constraints from cheap detectors (near-equal values, near-axis
directions, near-tangent joins, near-mirror pairs, repeated shapes), B4's likelihood (as an
evaluator of any constrained design), R3's prior. *Out:* the accepted constraints, the
tie classes, the grid and each coordinate's grid label, and the constrained fit.

### R2.1 Claim: the test

**Claim.** Imposing a constraint `c` that removes `Δk` free coordinates raises the posterior
of the design exactly when

```text
½ Δχ²_c  <  Σ_{removed j} ln( W_j / (√(2π)·σ_j) )  +  ln( π_c / (1 − π_c) )  =:  τ_c,
```

where `Δχ²_c` is the rise in the best fit's `χ²` when `c` is imposed (the rest unchanged),
`σ_j` the posterior standard deviations along the removed directions, `W_j` their prior
ranges and `π_c` the prior probability of `c` in its context (R3). Two discrete special cases:

* **grid label**, coordinate at offset `z·σ` from the nearest point of a grid of pitch `g`:
  `½ z² < ln( g / (√(2π)·σ) ) + ln( π_on / (1 − π_on) )`;
* **tie classes**, from the Chinese-restaurant prior with concentration `α` (R2.5): merging
  two classes of sizes `m₁, m₂` is worth `ln(W/(√(2π)σ)) − ln α + ln(Γ(m₁+m₂)/(Γ(m₁)Γ(m₂)))
  − ½ ln((m₁+m₂)/(m₁m₂))` against its `½Δχ²`.

**Proof.** R0.2 applied to the two nested designs: the constrained one lacks the removed
coordinates' Occam terms and carries the constraint's prior `π_c` instead of `1 − π_c`. For a
grid label, the prior of a grid point is `π_on·g/W` per unit length against `(1 − π_on)/W`
for a free value, and the free value's Occam term is `ln(W/(√(2π)σ))`; for the tie classes,
the Gaussian integral of R2.5's cluster cost. ∎

### R2.2 Claim: error rates at a given resolution

**Claim.** In the linear-Gaussian case, a true constraint's `Δχ²_c` is distributed as `χ²`
with `Δk` degrees of freedom, and a false one's as noncentral `χ²` with noncentrality `b²`,
`b` the violation in standard deviations of the removed direction. So `P(reject true) =
P(χ²_Δk > 2τ)` and `P(accept false) = P(χ²_Δk(b²) < 2τ)`:

| τ (nats) | P(reject a true constraint) | violation accepted half the time | accepted 5 % of the time |
|---|---|---|---|
| 2 | 4.6·10⁻² | 2.0 σ | 3.6 σ |
| 4 | 4.7·10⁻³ | 2.8 σ | 4.5 σ |
| 6 | 5.3·10⁻⁴ | 3.5 σ | 5.1 σ |
| 8 | 6.3·10⁻⁵ | 4.0 σ | 5.6 σ |
| 10 | 7.7·10⁻⁶ | 4.5 σ | 6.1 σ |

(`Δk = 1`.) For a coordinate the price alone is 6.2–9.9 nats over 128–512 px and σ = 0.1–0.01
px, and 7–13 nats at the boundary chain's calibrated precisions; the prior odds move `τ` by
±2 nats (R3).

**Proof.** `merge_chi2_increase`: the rise is the squared norm of the noise in the removed
directions (true constraint), plus the violation's projection (false one); Gaussian noise
makes these `χ²` and noncentral `χ²` variables. ∎

### R2.3 Claim: the test is consistent

**Claim.** As the resolution grows, both error rates go to zero.

**Proof.** `τ_c` grows as `β ln s` (R0.5), so `P(χ²_Δk > 2τ)` falls as a power of `s`. A false
constraint's violation in standard deviations grows as a power of `s` (its `σ` shrinks while
its violation in design units is fixed), faster than `√τ`, so `P(χ²_Δk(b²) < 2τ) → 0`. ∎

The corpus helps the test at every resolution: artists' constraints are exact or clearly
absent (R3.1), so few true violations sit near the threshold.

### R2.4 Claim: accepting in order of evidence never produces a contradiction

**Claim.** Test candidates in decreasing order of `τ_c − ½Δχ²_c`, each against the fit under
all constraints already accepted, and refit after each acceptance. Then (a) the accepted set
always has a solution; (b) a candidate contradicting the accepted set is rejected; (c) a
candidate implied by them is recorded but earns nothing; (d) the final `χ²` exceeds the free
fit's by the sum of the accepted rises, each below its threshold, so the design's cost `L`
never rises; (e) after a final pass that tries removing each accepted constraint, the set is
a local optimum of `L`: no single addition or removal lowers it.

**Proof.** (a) The current constrained fit is a solution. (b) A contradiction leaves no
solution or a singular one with the observed data; the constrained `χ²` is then infinite or
the rise exceeds every threshold. (c) R1.4: `Δk = 0`. (d) The rises telescope:
`χ²_final − χ²_free = Σ Δχ²_c` and each accepted `½Δχ²_c < τ_c`. (e) By construction. ∎

**Gap.** Not a global optimum: choosing the best subset of constraints contains best-subset
selection, which is NP-hard (Natarajan 1995). When the removed directions are orthogonal in
the Fisher metric the rises are independent and the order does not matter; when they are
not (two ties sharing a coordinate) the tie dynamic program of R2.5 decides them jointly.

### R2.5 Claim: ties are found exactly by a one-dimensional dynamic program

**Setting.** Measured values `v₁ … v_n` of one kind (the `x` of every point, or every
radius), equal noise `σ`, a Chinese-restaurant prior on their partition into tie classes
(concentration `α`; Ewens 1972, Aldous 1985) with any base measure for the class values.

**Claim.** The most probable partition together with class values has classes that are
contiguous in sorted order, so a dynamic program over the sorted values,
`best[b] = min_a best[a] + cost(a..b)`, finds it exactly in `O(n²)`, with

```text
cost(m values, sum of squares S about their mean) = −ln α − ln Γ(m) + ln W
     + ((m − 1)/2)·ln(2πσ²) + S/(2σ²) + ½ ln m.
```

**Proof.** The prior depends on the partition only through the class sizes. Take any
partition with class values `c₁ < c₂` and values `a < b` with `a` in the class of `c₂`, `b`
in the class of `c₁`. Swapping them keeps every size and changes the squared residual by
`(a − c₁)² + (b − c₂)² − (a − c₂)² − (b − c₁)² = −2(b − a)(c₂ − c₁) < 0`. So an optimum has no
such pair, which is contiguity. The cost is the Gaussian marginal of `m` values about a class
value uniform on `[0, W]`. ∎

Fisher (1958) proved contiguity for least-squares grouping with a fixed number of groups;
the extension to the Chinese-restaurant prior, where the number is chosen by the evidence,
is *not from the literature*. **Gap:** with unequal `σ_i` the exchange argument fails (it
needs the weights equal); the program then returns the best contiguous partition, an upper
bound, and the coupled correction is R2.4's.

**Measured** (simulation on the artists' numbers, `--part ties_sim`): pairwise precision of
recovered equalities 0.98–1.00 and recall 1.00 in lucide and material at σ = 0.005 px
(0.94–1.00 and 0.99–1.00 at σ = 0.02–0.1 px), 128 and 512 px; over all families 0.83 and
0.99 at 128 px, 0.95 and 1.00 at 512 px. Where precision is lower (the emoji families at
128 px and at the larger σ, down to 0.25), the merged values are closer than about `2σ`:
the pixels cannot tell them apart, and merging them costs no fidelity (the positional error
does not rise, as R0.4 predicts for violations under a standard deviation).

### R2.6 Claim: the grid is inferred from tie classes, not from coordinates

**Claim.** In the hierarchical model (values → tie classes under the Chinese-restaurant
prior → class values drawn from a base measure that mixes a grid with a continuous
background), the grid's likelihood is a product over *classes*, each with noise
`σ/√m`, not over coordinates.

**Proof.** A class's value is one draw from the base measure; its `m` members are
conditionally independent given it. ∎ Counting coordinates instead treats a value repeated
twenty times as twenty independent agreements with the grid. Measured over the eight
conditions of Table R2.8 (two resolutions, four noise levels): in the families without a
grid, the share of icons where a grid is found falls from 49–74 % to 31–65 % (simple-icons)
and from 32–75 % to 11–63 % (noto); wrong snaps fall to 23–77 % of their per-coordinate
rate over all families (to 5–40 % in noto).

### R2.7 Claim: harmonics resolve themselves

**Claim.** If all class values lie on a grid of pitch `g`, the posterior prefers `g` to `g/2`
by `n_on·ln 2` nats minus the prior difference (`n_on` the number of on-grid classes), and
prefers `g` to `2g` unless half of the classes happen to lie on `2g`.

**Proof.** A class's likelihood ratio against the background is `π·g·φ(z) + (1 − π)`; halving
the pitch leaves `φ` unchanged for on-grid classes and halves the factor `g`. ∎ The grid that
explains the numbers with the coarsest pitch wins: a 24-unit icon is read on its unit or half
unit, not on a 1/8.

### R2.8 Empirical Bayes without double counting, and the measurements

**Claim.** Each coordinate's grid label, decided with the grid posterior computed from the
*other* classes (leave one out), is the exact Bayesian conditional; the plug-in decision
(grid estimated from all classes) differs from it only when the grid's posterior margin is
smaller than the largest single class's log likelihood ratio.

**Proof.** The grid posterior is a product over classes; removing one class divides by its
factor, at most `ln(g/(√(2π)σ/√m)) + |ln π|` nats; if the margin between the best grid
hypothesis and the next exceeds that, the same hypothesis wins without the class. ∎

**Simulation.** `bench/theory/design_prior.py --part grid_sim` and `--part ties_sim`. The
artist's positions (every on-curve point and primitive centre or side, 1,408 files) at
128 and 512 px, Gaussian noise of σ px per position standing in for B4's precision (0.005 px
is the boundary chain's calibrated regime under 8×8 supersampling, `chain-boundary.md` B4.3;
0.05 px is `DEFAULT_SIGMA_MODEL`), hypotheses of pitch `s/V·2^{−k}` for 11 viewBox sizes `V`
and `k = 0…3`, prior halving per subdivision and ½ on "no grid". "Exact" counts positions
returned equal to the artist's number; "wrong" counts positions snapped to another value.

| family | px | σ px | exact (per coordinate, one weight) | wrong | exact (hierarchical) | wrong | error after / before (hier.) |
|---|---|---|---|---|---|---|---|
| lucide | 128 | 0.005 | 0.727 | 0.006 | 0.728 | 0.003 | 0.232 |
| lucide | 512 | 0.005 | 0.728 | 0.001 | 0.729 | 0.000 | 0.227 |
| lucide | 128 | 0.05 | 0.716 | 0.045 | 0.719 | 0.036 | 0.235 |
| material | 128 | 0.005 | 0.732 | 0.000 | 0.739 | 0.000 | 0.220 |
| material | 512 | 0.005 | 0.744 | 0.000 | 0.752 | 0.000 | 0.221 |
| material | 128 | 0.05 | 0.722 | 0.020 | 0.722 | 0.025 | 0.273 |
| twemoji | 128 | 0.005 | 0.251 | 0.022 | 0.253 | 0.021 | 0.611 |
| noto-emoji | 128 | 0.005 | 0.057 | 0.004 | 0.052 | 0.002 | 0.921 |
| simple-icons | 128 | 0.005 | 0.048 | 0.020 | 0.043 | 0.014 | 0.834 |
| all | 128 | 0.005 | 0.164 | 0.010 | 0.162 | 0.008 | 0.584 |
| all | 512 | 0.005 | 0.178 | 0.004 | 0.179 | 0.003 | 0.567 |
| all | 128 | 0.05 | 0.132 | 0.030 | 0.127 | 0.016 | 0.639 |
| all | 512 | 0.05 | 0.156 | 0.017 | 0.153 | 0.011 | 0.600 |

The share of lucide's and material's on-curve numbers on a dyadic grid (unit to quarter
unit) is 73–74 %, so the hierarchical model returns essentially every on-grid number
exactly, and cuts the mean positional error to between a fifth and a quarter. The wrong
snaps that remain are values within about `3σ` of a grid point that are not on it, at
the rate R2.2 predicts, falling with resolution and with σ: at σ = 0.005 px they are lucide's
derived points at 128 px (0.3 %) and, in twemoji and simple-icons, three- and four-decimal
numbers a thousandth of a unit from an integer, which 128 px cannot separate from it (0.01
px) and 512 px mostly can. Moving such a number by under `3σ` costs nothing visible (R0.4).
The model knows only each point's role (construction or derived; R3.2); a representation
that writes a derived point as the intersection of its snapped construction (R1.2a) would
not snap it at all, which is the next refinement.

### R2.9 Corners, junctions and continuations are constraints

**Claim.** A corner is the absence of a G1 constraint at a join: the smooth join is the
nested model with one coordinate fewer (the outgoing tangent). A T-junction, where one face's
boundary runs straight through and another's ends against it, is a G1 (collinearity)
constraint between the through-face's two edges at the vertex. Both are decided by R2.1
against B4's run terms and the vertex's window term.

**Proof.** R1's vocabulary and R2.1; the window term is the likelihood of the local
configuration with or without the constraint. ∎ This is the boundary chain's shortening
proposal S1, accepted in the interface section: the decisions move to R; B3 supplies the
vertex's per-pixel terms. `occlusion.rs` already reads paint order from this collinearity;
here it becomes one more tested constraint, with an error rate.

## R3. The human prior, small

**Problem.** *In:* the artists' own files (`bench/data/corpus_svg`, the gate families,
1,408 files). *Out:* the prior odds of each constraint in each context, and the
document-level variables that change them, as a model of a few hundred numbers.

**The model.**

1. **Structure.** `P(element kind | closed shape class, style)`, `P(written segment kind |
   geometric kind, style)`, `P(smooth | join kinds, style)`, `P(axis-aligned, 45° | line,
   style)`, `P(stroke | style)`, `P(partly covered | style)`: Dirichlet-smoothed counts
   (the Krichevsky–Trofimov estimator, add ½).
2. **Coordinates.** Per axis and per kind of number (positions, radii, widths), a
   Chinese-restaurant process of concentration `α_style`, whose base measure mixes the
   document's grid (weight `π_role,style`) with a continuous background, the weight
   conditioned on the point's **role**: construction (line–line corner, open end, primitive
   centre or side) or derived (a corner where a curve meets, a tangent point).
3. **Document variables.** The grid (a pitch from the raster size and a short list of
   viewBox sizes and subdivisions), the style (a mixture over the corpus families), the
   stroke width (a tie class of its own).

About forty numbers per style, six styles, and the grid hypotheses' prior: a few hundred.

### R3.1 The numbers

Measured by `bench/theory/design_prior.py` on every file of the gate families (the 246-icon
screen set gives the same picture: axis-aligned lines 0.50, smooth curve joins 0.69,
repeated radii 0.58).

**Lines and joins.**

| family | gate count / icon | lines axis-aligned (exact) | within 0.5° of an axis, not exact | 45° | axis lines written `H`/`V` | curve joins smooth (< 2°) | turning 5–30° | corners > 30° | line–line joins collinear |
|---|---|---|---|---|---|---|---|---|---|
| lucide | 41.7 | 0.668 | 0.000 | 0.163 | 0.94 | 0.904 | 0.010 | 0.073 | 0.000 |
| material-icons | 96.5 | 0.723 | 0.005 | 0.123 | 0.98 | 0.771 | 0.006 | 0.207 | 0.006 |
| simple-icons | 253.2 | 0.463 | 0.020 | 0.018 | 0.92 | 0.713 | 0.046 | 0.167 | 0.023 |
| openmoji | 372.5 | 0.332 | 0.025 | 0.032 | 0.72 | 0.621 | 0.097 | 0.199 | 0.050 |
| twemoji | 451.4 | 0.564 | 0.008 | 0.030 | 0.95 | 0.758 | 0.026 | 0.197 | 0.014 |
| noto-emoji | 1137.0 | 0.279 | 0.039 | 0.009 | 0.88 | 0.655 | 0.094 | 0.167 | 0.100 |
| all | 444.9 | 0.452 | 0.021 | 0.040 | 0.92 | 0.687 | 0.071 | 0.177 | 0.045 |

Constraints are exact or clearly absent: the near-miss columns are small, and the turning
at curve joins is bimodal (smooth or a real corner, with 7 % between). Where artists'
numbers violate a constraint slightly, it is rounding: lucide writes its arc endpoints to two
decimals, so 44 % of its arc–line joins turn by a small nonzero angle (under 2°) and 52 % are
exactly tangent; they were drawn tangent and written rounded. The written file is what was
rendered, so whether the test imposes the tangency depends on the resolution: a turn of a
tenth of a degree is within the noise of a short arm at 128 px and not of a long one at 512
px (`chain-boundary.md` B4.3: 0.076° for a 10 px line under 8×8 supersampling), where the
trace keeps the artist's small kink, as the pixels show it. Noto's 10 % of line–line
vertices turning under 0.5° are, nearly all, points an export left on a straight run: a
place where the trace can be smaller than the artist's file.

**Segment kinds.**

| family | line | quadratic | cubic | arc | cubics that are circular arcs | circular geometry written as `A` | arcs ≤ 120° |
|---|---|---|---|---|---|---|---|
| lucide | 0.562 | 0.001 | 0.045 | 0.392 | 0.147 | 0.98 | 0.82 |
| material-icons | 0.670 | 0.000 | 0.302 | 0.028 | 0.826 | 0.10 | 0.85 |
| simple-icons | 0.329 | 0.015 | 0.469 | 0.188 | 0.241 | 0.62 | 0.95 |
| openmoji | 0.314 | 0.001 | 0.605 | 0.080 | 0.382 | 0.26 | 0.92 |
| twemoji | 0.256 | 0.000 | 0.717 | 0.027 | 0.294 | 0.12 | 0.80 |
| noto-emoji | 0.176 | 0.000 | 0.824 | 0.000 | 0.201 | 0.00 | – |

Whether a circular arc is written `A` or as a cubic is a matter of style, not geometry: 98 %
`A` in lucide, 10 % in material, none in noto. The pixels cannot tell an arc from its cubic
approximation (a quarter circle's cubic is off by 0.03 % of the radius), so this choice is
the prior's alone, and the style variable must carry it. 11–20 % of arcs exceed the fitter's
120° cap.

### R3.2 The grid governs construction points; derived points are not free

Share of numbers on the half-unit grid, by the role of the point:

| family | line–line corner | open end | primitive | corner where a curve meets | tangent point | icons with a grid (80 % of construction numbers) |
|---|---|---|---|---|---|---|
| lucide | 0.955 | 0.830 | 1.000 | 0.269 | 0.628 | 0.72 |
| material-icons | 0.797 | 0.715 | 0.816 | 0.289 | 0.768 | 0.57 |
| twemoji | 0.503 | 0.108 | 0.738 | 0.160 | 0.252 | 0.32 |
| openmoji | 0.211 | 0.165 | 0.378 | 0.107 | 0.140 | 0.10 |
| simple-icons | 0.083 | 0.155 | – | 0.015 | 0.042 | 0.01 |
| noto-emoji | 0.072 | 0.135 | 0.078 | 0.096 | 0.066 | 0.01 |

The finding the prior is built on: in the grid families, the grid is where the artist chose
a point; where two curves meet, the point is computed (lucide's badge: eight arcs of radius
4 meeting at `(8.63, 3.85)`), and is written rounded. A prior that conditions on the role
snaps the first and leaves the second to its construction (R1.2a). Twemoji draws its circles
on the half-unit grid (74 %) and little else; noto and simple-icons have no grid (two-decimal
and four-decimal numbers throughout).

### R3.3 Ties, radii, widths, and what the prior is worth

| family | x values reused | y values reused | CRP α (x, y) | radii equal to another in the icon | stroke widths per stroked icon | nats per position under the prior (flat: 8.54) |
|---|---|---|---|---|---|---|
| lucide | 0.345 | 0.396 | 11.7, 9.0 | 0.814 | 1.00 | 5.29 |
| material-icons | 0.483 | 0.505 | 13.4, 12.0 | 0.401 | – | 5.30 |
| simple-icons | 0.171 | 0.209 | 208, 156 | 0.490 | – | 8.07 |
| openmoji | 0.307 | 0.399 | 108, 67 | 0.385 | 1.23 | 7.38 |
| twemoji | 0.246 | 0.347 | 178, 100 | 0.680 | – | 7.59 |
| noto-emoji | 0.230 | 0.335 | 661, 314 | 0.727 | 1.50 | 8.05 |

The last column is the mean code length of the artists' own positions under the
hierarchical prior (reuse at the Chinese-restaurant odds; else, in an icon with a grid, the
grid at the family's on-grid weight; else the flat price inkvec charges at 512 px,
`ln(512/0.1)`; one weight per family here, the role-conditioned weights of R3.2 would lower
it further): the prior saves about 3.2 nats per position (38 %) in the grid families and
0.5–1.2 nats elsewhere. In
gate-count terms that is a third of a parameter per position, which is how much more
willing the fit should be to keep a point where the artist's style puts one.

### R3.4 Layers, symmetry, repetition (geometry of the artists' files)

| family | elements partly covered by another paint | artist's parameters in them | elements wholly hidden | elements self-symmetric | in a mirror pair | repeating another (translation) | icons mirror-symmetric (render) |
|---|---|---|---|---|---|---|---|
| lucide | 0.000 | 0.000 | 0.000 | 0.680 | 0.029 | 0.144 | 0.42 |
| material-icons | 0.000 | 0.000 | 0.000 | 0.458 | 0.000 | 0.021 | 0.38 |
| simple-icons | 0.000 | 0.000 | 0.000 | 0.166 | 0.000 | 0.000 | 0.13 |
| openmoji | 0.367 | 0.444 | 0.025 | 0.417 | 0.043 | 0.137 | 0.08 |
| twemoji | 0.563 | 0.473 | 0.005 | 0.483 | 0.035 | 0.099 | 0.20 |
| noto-emoji | 0.471 | 0.467 | 0.050 | 0.277 | 0.062 | 0.150 | 0.05 |

Openmoji's fills sit under its line art: the median partly covered fill has 96 % of its
visible boundary borrowed from the strokes painted over it, so the fill's own outline is not
observable at all and only the prior can choose it (R4.2). Writing each repeated element as a
`<use>` would save 2.4 % of the artist's gate count at the gate's price of 6 per use: the
value of repetition is in fidelity and editing (exact copies), not in the count.

### R3.5 Claims about the estimate

**R3.5a (estimation error).** The unit of independence is the icon, not the coordinate:
the numbers of one icon share its grid and its style. A frequency estimated from `N` icons
per style, across `C` contexts, is within `ε = √(ln(2C/δ)/(2N))` of its expectation for
every context with probability `1 − δ` (Hoeffding 1963 with a union bound). With `N ≈ 150`
to 310 icons per family and `C ≈ 50`, `ε ≈ 0.10–0.13` at `δ = 0.05`: about ±0.5 nats of log
odds for frequencies between 0.2 and 0.8. **Proof:** Hoeffding's inequality for the icon
means, union bound. ∎ This is the worst case; the per-context counts (hundreds to tens of
thousands of instances) make the typical error far smaller, but the bound is the one the
dependence within icons allows.

**R3.5b (decisions are robust to the prior's error).** An error of `ε` nats in a log prior
odds changes a decision of R2.1 only when `½Δχ²_c` lies within `ε` of `τ_c`. For a true
constraint that probability is `P(χ²_Δk ∈ [2τ − 2ε, 2τ])`, under 10⁻³ for `τ ≥ 6`, `ε ≤ 1`,
`Δk = 1`. **Proof:** the decision is a threshold on `½Δχ²_c`; the density of `χ²₁` above 10 is
under 10⁻³. ∎ So at 512 px the prior's precision hardly matters for true constraints; it
matters for the near-threshold false ones, and at 128 px, where `τ` is smaller and the
violations closer.

**R3.5c (calibration).** If the likelihood is calibrated and the prior is the corpus
frequency, the posterior probability that a number is on the grid, averaged over the corpus,
equals the corpus frequency (the law of total probability), and the decision's frequency
differs from it by at most the misclassification rate. **Proof:** `E[P(on | data)] =
P(on)`. ∎ Measured in the simulation of R2.8 at σ = 0.005 px: 72.8–75.2 % of lucide's and
material's positions returned exactly against 73–74 % of their on-curve numbers on a dyadic
grid in the files, wrong snaps 0–0.3 %. This is the check the chains page asks for ("the
decision rule reproduces the corpus's constraint frequencies on clean inputs"); on traced
output it becomes the gate's design-statistics axis (R4.8).

**R3.5d (style).** The style is a document variable with a posterior from document-level
evidence (stroked or filled, a grid or none, layered or not, the arc–line ratio), and the
style-dependent probabilities are its posterior mixture. **Gap:** the accuracy of style
inference from pixels is not measured yet; the families are separable on these features in
the artists' files (lucide is 100 % stroked with one width and a grid; material is filled
with a grid; the emoji families are layered without one), but that is the easy direction.

## R4. Structure and encoding

**Problem.** *In:* B4's map and R2's constrained designs per face and boundary. *Out:* the
SVG: which faces are strokes, which shapes continue under which, which are primitives,
which repeat, and the text that writes them with the fewest numbers.

### R4.1 Claim: given a paint order, layers decompose exactly

**Setting.** The planar map's faces have pairwise disjoint visible regions `V₁ … V_m` (the
background's region `V₀` is painted by nothing). Elements `E₁ … E_m` (sets of the plane, one
per face, in that face's paint) are painted in the order `1 … m`, bottom to top. The region
where element `i` shows is `E_i \ ⋃_{j>i} E_j`.

**Claim.** Every element shows exactly its face's visible region, and nothing covers the
background, if and only if for every `i`

```text
V_i  ⊆  E_i  ⊆  V_i ∪ U_i,        U_i = ⋃_{j>i} V_j.
```

The conditions involve the data `V` only, so given the order the choice of each `E_i` is
independent of every other.

**Proof.** (⇐) By induction downward from the top, show that element `i` shows `V_i` and
that `⋃_{j≥i} E_j = V_i ∪ U_i`. For `i = m`, `U_m = ∅` so `E_m = V_m`. Assume
`⋃_{j>i} E_j = U_i`. Element `i` shows `E_i \ U_i`; from `E_i ⊆ V_i ∪ U_i` this lies in `V_i`,
and from `V_i ⊆ E_i` with `V_i ∩ U_i = ∅` it contains `V_i`. And `E_i ∪ U_i = V_i ∪ U_i`. Nothing
reaches `V₀`, which is disjoint from every `V_i ∪ U_i`. (⇒) If element `i` shows `V_i` for all
`i`, the same induction gives `⋃_{j>i} E_j = U_i`, and `E_i = (E_i \ U_i) ∪ (E_i ∩ U_i) ⊆ V_i ∪
U_i` while `V_i = E_i \ U_i ⊆ E_i`. ∎

*Not from the literature* in this form (the painter's algorithm's set algebra is
elementary; its use as an exact decomposition of layered vectorisation is new as far as we
know). `docs/DESIGN.md` §2.4 rules out amodal *inpainting* as a correctness hazard because
it invents content; this claim is why completion is not that: every completion in the
interval paints exactly the same visible regions, so no visible pixel of the picture
changes and nothing is invented that can be seen.

**R4.1b (completion removes the compositing seam).** Anti-aliased "over" compositing of
`E_i` and then `E_j` (`j` above) in a pixel `p` with area coverages `a_i, a_j` gives
`a_j c_j + (1 − a_j)(a_i c_i + (1 − a_i) c_bg)`. With `E_i = V_i` exactly, along an edge
between faces `i` and `j` with no background in the pixel, `a_i = 1 − a_j`, and the
background shows with weight `a_j(1 − a_j)`: the hairline seam `seams.rs` describes. With
`E_i` completed under `E_j` across the pixel, `a_i = 1`, and the result is the exact
`a_j c_j + (1 − a_j) c_i`. Where the pixel also holds background area `b` (a three-ink
junction), the remaining error is `a_j·b·(c_i − c_bg)`. **Proof:** substitute. ∎ So the
underlap of `seams.rs` (the lower face reaching a fraction of a pixel under the upper one,
which costs 1.8 % of the parameters over the screen set, [`optimal.md`](optimal.md) §4) is
the one-pixel special case of a completion, and a completion chosen by the prior removes
the seam while lowering the count.

### R4.2 Claim: inside its interval, a face's shape is the prior's choice alone

**Claim.** Given the order, the likelihood of the picture is the same for every `E_i` in
`[V_i, V_i ∪ U_i]` (up to the junction-pixel term of R4.1b), so the most probable `E_i` is the
most probable shape under the prior among those in the interval: the simplest shape that
contains the visible part and stays within what is painted over it. Every shape in the
interval has the *owned* boundary of `V_i` (the part of `∂V_i` not in the closure of `U_i`)
in its own boundary, so the cost of describing the owned boundary is a lower bound on the
cost of any completion.

**Proof.** The first part is R4.1 and R0.2 with a constant likelihood. For the bound: near a
point of `∂V_i` outside the closure of `U_i`, every `E` in the interval coincides with `V_i`
(it contains `V_i` and adds only points of `U_i`), so the point is on `∂E`. ∎

**Candidates**, each checked for containment in the interval: primitives fitted to the owned
boundary (rectangle, circle, ellipse, rounded rectangle); the owned pieces continued
through `U_i` (lines extended to their intersection, arcs along their circle, a G1 cubic
bridge where neither, the continuation that Kellman & Shipley's (1991) relatability
describes); and the visible outline itself, which is always in the interval (today's
output). **Gap:** optimality over all shapes is not claimed, only over the candidates.

The flag `openmoji/1F3F4-E0069-E0074-E0062-E0061-E007F`: with the white triangles and the
black border above the blue, the blue face's interval contains the artist's rectangle (`x 5,
y 17, 62 × 38`), a `<rect>` of gate count 6, in place of the blue's visible outline; the
artist's whole file counts 20, today's trace 54.

### R4.3 Claim: the paint order is the only coupling, and is searched with a bound

**Claim.** The total cost is `Σ_i c_i(A_i)`, where `A_i` is the set of faces painted above
`i` that a completion of `i` can reach (in practice its neighbours: a shape continued from
`V_i` enters `U_i` through the faces it borders); it is to be minimised over the acyclic
orientations of the face adjacency graph. `Σ_i min_A c_i(A)` is
a lower bound for any orientation; branch and bound over the orientation of adjacent pairs,
started from the order the T-junctions read (R2.9, `occlusion.rs`), finds the optimum.

**Proof.** R4.1 for the decomposition; the bound because each term is minimised separately.
∎ **Gap:** no polynomial algorithm is claimed (with general costs the problem contains a
linear-ordering problem); the search is exact for the small components of icons (tens of
faces, mostly in small connected groups) and is cut to a beam otherwise, with the bound
reporting how far from optimal the beam may be.

### R4.4 Strokes, gradients and primitives are R0 decisions

Stroke or fill (`ribbons.rs`), gradient or flat (on B1's statistics: the flat ink is the
nested model with the field's coefficients removed), primitive or path (`primitives.rs`,
`choice.rs`): each compares two designs of one face by `L` of R0.2, with the prior of the
style (100 % of lucide's icons are stroked with one width; 97 % of openmoji's have strokes;
none of material's). Nothing beyond R0 is claimed; the gain is that each decision now has
the evidence's precision term and the prior.

### R4.5 Repetition and symmetry

**Claim.** An exact repetition (a translated copy) or symmetry (a mirror, a half turn) is a
constraint of R1.5, decided by R2.1 like any other, and imposed whatever the encoding; it
is written as a `<use>` of one element only when that lowers the gate's count: a copy whose
own count exceeds 6, a self-symmetric element whose half costs more than its mirror's
`<use>` saves. **Proof:** R1.5 and the counting rules. ∎ Repetition and symmetry change
fidelity (an exact copy averages the noise of every instance, R0.4) and the geometric match
more than the count (2.4 % of the artist's count, R3.4); the search for them is the
equality-saturation search of Szalinski (Nandi et al. 2020) over translations, mirrors and
rotations.

### R4.6 Claim: the encoder writes each design with the fewest gate-counted numbers

**Claim.** For every element kind of the vocabulary, the encoding with the fewest
gate-counted numbers among SVG's encodings of exactly that geometry is:

| design element | write | gate's count | the alternative |
|---|---|---|---|
| line | a path segment (`H`/`V` when axis-aligned) | 2 | `<line>`: 4 |
| axis-aligned rectangle | `<rect>` | 6 | path: 6 (tie, the primitive preferred) |
| rounded rectangle | `<rect rx>` | 6 | path: 36 |
| circle | `<circle>` | 3 | two arcs: 14 |
| axis-aligned ellipse | `<ellipse>` | 4 | two arcs: 14 |
| circular arc | `A` or a cubic, by the style prior | 7 or 6 | – |
| smooth cubic with a mirrored handle | `S` | 6 | `C`: 6 |
| translated copy of an element costing more than 6 | `<use>` | 6 | the copy |
| several subpaths of one paint | one compound path | their sum | separate paths: the same sum |

**Proof.** By enumeration of the encodings against the counting rules of `svgmodel.py`. ∎ The
table is finite and decidable, so it is the first thing proved by computation in Lean
(Phase 2, `Encoding.lean`).
Two encoding choices are not about the count: `H`, `V` and `S` change no gate count but write
fewer numbers (artists write 92 % of their axis-aligned lines as `H`/`V`), and the
document's coordinates are written in the inferred **design unit** (the viewBox of the
grid hypothesis that won in R2.6), so on-grid numbers are the artist's own small integers
and halves. A derived or free coordinate is written to the decimals its precision warrants:
the step `q` whose rounding error `q/√12` stays under half the coordinate's `σ`.

### R4.7 Claim: one picture, one file

**Claim.** With the posterior's ties broken by a fixed order (prior, then gate count, then a
lexicographic order of the written numbers), the output is a function of the picture: two
traces of the same pixels write the same file. **Proof:** determinism of every step. ∎ This
is the "normal form" of [`optimal.md`](optimal.md) §5.

### R4.8 What the gate should see

Beyond the parameter ratio, dE00 and the geometric match `geom` (mean edge displacement by
symmetric-difference area, `bench/inkvec_bench/geomatch.py`), with corner precision and
recall and segment-type agreement: the design statistics of R3 on the trace against the
artist's files, family by family (axis-aligned share, smooth-join share, on-grid share by
role, tie share, layering share). By R3.5c, a calibrated chain R reproduces them; a gap
names the link that is off.

## Interface proposal

The boundary chain's interface proposal ([`chain-boundary.md`](chain-boundary.md),
"Interface proposal") answers what this chain needs: what R asked for (per graph-like run,
the column or row sums with their variances; per vertex, raw unmixed pixels with the exact
forward model) is the special case of its **window identity** (B2.1: summed over any set of
whole pixels, a face's unmixed weights are its exact area there), which scores *any*
description, layered or not, by areas. So chain R adopts B's `Evidence` (map, ink
candidates, runs of window observations, vertices, thin strips, noise) and its
`BoundaryLikelihood` trait (`chi2_run`, `residuals_run` with the Jacobian, `run_moments`,
`chi2_local`, `chi2`, `density`, `floor`, every call returning `(χ², M)` calibrated so that
`E χ² = M` at the truth) as the interface, with the amendments below.

Two of B's results change this chain's numbers. Calibrated window variances put positions
at 0.0004–0.04 px (B4.3), well inside the 0.02–0.1 px this page's simulations first assumed;
at σ = 0.005 px the grid and the ties are recovered almost without error in the grid
families (Table R2.8). And the likelihood is sharp enough to separate a true arc from its
four-cubic approximation by about fifty standard deviations at 512 px, which makes the
forward model's agreement with the renderer a precondition of everything in R0 (amendment
A1).

### Amendments chain R needs

* **A1. Render as the corpus's renderer does, and calibrate at the artist's own file.** R0.5's
  limit and R2.2's error rates hold only if the truth's `χ²` has its nominal distribution.
  With variances this small, any difference between the evaluator's drawing of a candidate
  and resvg's (its arc-to-Bézier conversion, its curve flattening, its stroker's offset
  curves, 8× supersampling with a box filter as `bench/build_corpus_v2.py` renders) is a
  misfit growing with resolution that R would pay for with extra segments. So: the
  evaluator draws arcs, circles, ellipses and strokes by the renderer's own approximations;
  and the calibration check is run on the artist's SVG itself, scored against the
  `Evidence` of its own render (`χ²/M` near 1 per family and tier), not only on the
  starting map's smooth runs.
* **A2. Pieces and descriptions.** `Piece` gains the elliptical arc (radii, rotation) and
  `Description` gains strokes (centreline pieces, width, cap, join, drawn as the renderer
  strokes them) and layers (elements in paint order, composited per layer as the renderer
  composites; B3.2's open question). A fast path through the visible partition is exact
  except at the pixels R4.1b names, so R uses it in the search and the per-layer
  compositing to score a finished candidate.
* **A3. The floor as its own term, with its correlation.** The near-axis supersampling bias
  (up to `1/(2n)` px) is shared by every window of one edge and differs between edges. R
  adds it to the variance of every *absolute* position test (a grid label, a tie between
  two edges) and leaves it out of tests *along* one edge (an angle, a G1 join): folded into
  per-window variances it would be averaged down as if independent, and an exact grid
  number on an axis-aligned edge would be rejected at `z ≈ 9`. So: the floor's per-edge
  value and the rule "the same edge: fully correlated; different edges: independent".
* **A4. Junction reports.** Per junction node, the cyclic order of its arms and the pairs of
  arms that may continue one another (collinear or G1 within a few standard deviations),
  with their statistics: the input of R2.9 (continuation) and of R4.3 (paint order from
  T-junctions).
* **A5. Topological alternatives.** Where B's topology is uncertain beyond what strips cover
  (one junction or two corners close together; a vertex to merge or split), a small set of
  local alternatives, each with its observations, so that R chooses with the prior.
* **A6. Nothing else about precision.** R computes the Fisher information of its own
  parametrisation from `residuals_run`'s Jacobians (or `density`), and with it each free
  coordinate's price `ln(W/(√(2π)σ))` (R0.2) and every Wald statistic. B need not return
  `log det F` in a parametrisation of its own.

### Answers to the boundary chain's questions

1. **Window residuals.** Yes. R's dynamic program scores a segment by the window residuals
   of `chi2_run` (prefix moments from `run_moments` where the family is linear as a graph
   over the window axis, the closed-form integrals otherwise). Kinds the evaluator must
   integrate exactly, as the renderer draws them: line, quadratic, cubic, circular and
   elliptical arc; primitives (circle, ellipse, rectangle, rounded rectangle) through those
   pieces; stroke bands (A2).
2. **Additivity.** Breakpoints at window borders followed by a continuous polish are
   acceptable, with one rule: a candidate segment over windows `a..b` is charged only the
   windows wholly its own, and a guard window at each end goes to the vertex term, scored in
   the polish. The program's cost is then a lower bound on the polished cost, so its pruning
   stays valid, and no window is ever charged to the wrong arm. Straddling windows do not
   need to enter the program.
3. **The price of a parameter.** The refined description length of R0.2: `½χ²` plus, per
   free coordinate, `ln(W/(√(2π)σ))` from `F`, plus the prior's `−ln P(T)`. Not the fidelity
   constraint with a prior: R0.5 proves the constraint form is the high-resolution limit of
   this objective, so it is kept as the search order and as an adequacy check (when no
   candidate of the vocabulary reaches `χ² ≤ M − p + z√(2(M − p))`, the design is outside the
   vocabulary or A1 is violated, and either should be reported, not fitted away). R computes
   `log det F` itself (A6). With B's variances each free coordinate costs about 7–13 nats,
   so `λ = ln(extent/precision)` is retired, not re-tuned.
4. **Tests.** Wald for screening and ordering every candidate constraint (cheap, no refit),
   with the covariance updated after each acceptance (`F⁻¹ − F⁻¹Gᵀ(GF⁻¹Gᵀ)⁻¹GF⁻¹`), which
   reproduces the refit's sequential increments for linear models (R2.4); a refit for the
   final accepted set and for nonlinear constraints. Constraints spanning several edges'
   windows: ties of positions, radii and widths (R2.5); the grid (R2.6); parallel and
   perpendicular; concentricity and equal radii across elements; mirror pairs and
   repetition across faces (R1.5); continuation through a junction (two edges at one
   vertex). Since edges couple only through vertex and strip terms (B4.1), the Wald
   statistic of a cross-edge constraint needs only the per-edge covariance blocks.
5. **Corners.** Yes: R owns corner, fillet and smooth-join decisions (S1). R tests G1 at every
   breakpoint its program chooses, with the arms' fitted tangents, which detect a turn of a
   few tenths of a degree (B4.3's angle precision) where the fourth-difference proposal
   detects 1.6° (8-bit) or 14° (8×8). So proposals do not decide corners; they decide where
   the band keeps per-pixel terms. By the window identity a missed proposal costs
   information (the window sums two arms, B2.2), not correctness; a false one costs a little
   time. Target recall at least 0.99 down to the detection limit, false proposals up to one
   per ten windows. Starting tangents: the one-sided tangents on each side of each
   proposal, with their variances. Fillets use B's boundary-of-parameter-space null
   (`½χ²₀ + ½χ²₁`, threshold 5.41 at 1 %) for R2.2's error rates.
6. **Strokes and hidden geometry.** Accept a stroke directly (A2). For a hidden stroke, the
   flat-bottomed likelihood itself, not only its interval: R's prior is multiplied by it,
   and its soft edges matter at the noise level. For occluded parts nothing is needed: by
   R4.1 the likelihood is exactly constant over a face's interval, except at the junction
   pixels R4.1b names, which per-layer compositing scores (A2).
7. **Inks.** Yes: gradient or flat, and which gradient, is R's (S3), as nested models (flat
   inside linear, flat inside radial) priced by R0 with the style's prior (noto draws
   gradients in 165 of its 311 files; lucide, material and simple-icons never).
8. **Layers.** R hands the evaluator the layered description (elements in paint order) and
   asks it to composite per layer; the visible partition is R's fast path (A2).
9. **The floor.** Separate, with its correlation (A3).
10. **Primitives.** Draw each exactly as the gate's renderer does (A1). The design's
   geometric kind (a circle, a circular arc) and its written kind (`<circle>`, `A`, cubics)
   are separate parts of `T`: the evaluator scores each written kind as rendered, the
   likelihood separates them only where the renderer's approximations differ (large radii at
   512 px), and elsewhere the style prior chooses (R3.1: circular geometry is written as `A`
   98 % of the time in lucide, 10 % in material, never in noto). So yes, offer "circle" in
   every encoding the corpus uses.

### The boundary chain's shortening proposals

* **S1 (corners and junctions are R2's): accepted,** amended as in answer 5: R places a
  vertex by fitting its arms jointly with the vertex terms, and the proposals' job is the
  band's partition, not the decision.
* **S2 (continuation and tangency at junctions are R's constraints): accepted,** with A4:
  continuation also orders the paint (R4.3), so the junction report must carry the
  candidate continuations.
* **S3 (gradient or flat is R's): accepted.**
* **S4 (stroke or two edges, and a sub-pixel stroke's ink, are R's): accepted.** The ink of a
  hidden stroke is fixed only up to width × contrast (B1.5); R's prior chooses it (one stroke
  width per icon in lucide, 1.2–1.5 in openmoji and noto, R3.3).
* **S5 (points stop being the interface): accepted, with an order of work.** The point solve,
  `K_KINK`, `K_ANCHOR`, `DEFAULT_SIGMA_MODEL` and the curvature inflation go once the
  program scores windows and R1 guarantees that no segment has more parameters than windows
  (B4.2), not before: until then the points remain the program's input and its priors stay.
  `λ` is not re-derived as a constant but replaced by R0.2's per-coordinate terms.
* **S6 (B2 = the likelihood of any description; B4 = what the pixels can confirm):
  accepted.** One amendment of scope: B4.4 states the tests' distributions (nested, Wald,
  boundary of the parameter space); the decisions (thresholds with the Occam terms and the
  prior, the order of acceptance) are R2's, so the two pages do not each own half a test.

This chain's own shortening proposals are the same in substance (corners and continuations
as R2 decisions, R2.9; gradients as an R4 choice; thin strokes on B's strip terms; precision
computed by R for its parametrisation; the null space removed by never fitting more
parameters than windows), so the two chains agree on the cut.

### Still open between the chains

1. **The renderer floor on the corpus's actual intake** (resvg, 8× supersampled, box
   filtered): B's per-image estimator exists; its value per family and tier, and the
   calibration of A1, decide whether R2's error rates are the stated ones.
2. **Conflation at layered junction pixels** (B3.2's open question): R4.1b gives its size,
   `a_j·b·|c_i − c_bg|` per pixel where a completed shape, the shape over it and the
   background meet; the corpus measurement is phase 2 for both chains.
3. **Run coverage at 45°**: whether the switch between column and row windows leaves pixels
   counted twice or not at all; B's partition rule (one boundary piece per window, single
   pixels elsewhere) suggests neither, and R relies on it.

### Agreed in phase 2 (with the boundary chain)

1. **Item 1 above is settled by arbitration.** The engine does not replicate any renderer.
   It uses exact geometry plus a floor calibrated per image (straight, curved and stroke
   components, with a correlated part along each edge), so `χ²/M ≈ 1` whatever made the
   input. Grid and position tests use that floor.
2. **`EdgeScorer`** (in `inkvec-core`, so `inkvec-fit` needs no `inkvec-trace`):
   * It is built per edge from a `BoundaryLikelihood`.
   * `windows_between_points(i, j)` takes point indices of `map.edges[e].points` and wraps
     on a closed edge. Chain R maps its decimated and rotated indices back to these before
     calling, and passes pieces in pixel coordinates (the fitter works in content units).
   * The windows form a **partition**. Each window belongs to the span holding its centre and
     is scored with that span's piece extended past the span's end. Windows given to corners
     and junctions are a fixed set per edge. So the dynamic program's
     `Σ chi2(span) + Σ chi2_local(corner)` covers a constant number of windows.
   * `best_graph(r, degree)` is O(1) and feeds the inner loop. `chi2(r, pieces)` is
     O(len r), used only to re-score each span's best few candidates.
   * `weight_moments(r)` gives `Σ w·s^p` for `p ≤ 6`: the Fisher matrix of a graph of degree
     at most 3, for the Occam term (A6).
   * `Piece::from_svg_arc` converts an arc.
3. **`NoiseModel`** (`inkvec_core::noise`, on `ColorTrace::noise`): `sigma_flat`,
   `sigma_edge` by distance band, `psf_radius`, `psf_mu2`, `nu`, `lossy`, `window_scale`.
   * `NoiseModel::clean()` is today's behaviour bit for bit, and on a clean intake the
     estimator does not run.
   * The boundary chain folds `sigma_edge` and `window_scale` into each edge's point sigmas,
     only when the model is not clean. The fitter divides by those sigmas and scales nothing
     itself.
   * `huber_kappa(nu) = min(3, √nu)`, one definition for both chains.
   * `explained_by_psf(width, colour, ink_a, ink_b)` is the ringing null.
   * `psf_mu2` belongs to the forward model (`left_area`), not to the noise.
   * The PSF's first moment, a global shift, is not identifiable from one image and is not
     carried.
4. **Where chain R reads the noise model.**
   * The completion stage's junction zone uses `psf_radius`. It is 0 on a clean intake, so
     nothing changes there.
   * The fitter's robust loss uses `huber_kappa(nu)`.
   * Extra prior weight comes only from measured noise, never from a flag (user directive).

## Phase 2: formal, code, measurement

### Lean (`formal/InkvecTheory`, a new `Design/` directory)

| file | lemma | statement |
|---|---|---|
| `BiasVariance.lean` | `constrained_fit_error` | `‖P_K y − s‖² = ‖P_K ε‖² + ‖s − P_K s‖²` for `s ∈ L`, every `ε` (R0.4); with `noise_absorbed`, a constraint helps iff the violation is under `dim L − dim K` |
| `Evidence.lean` | `gaussian_evidence_1d`, `bayes_factor_nested` | the one-direction Gaussian integral (from Mathlib's `integral_gaussian`) and the nested-model Bayes factor of R2.1 in closed form |
| `Limit.lean` | `eventually_lexicographic` | for costs `a·g(s) + b·ln s + c` with `g` growing faster than `ln`, the order is eventually lexicographic in `(a, b, c)` (R0.5's skeleton) |
| `Grid.lean` | `snap_threshold`, `harmonic_halving` | the grid-label rule of R2.1; the factor 2 per on-grid class of R2.7 |
| `Ties.lean` | `swap_reduces`, `optimal_partition_contiguous` | the exchange inequality and contiguity of R2.5 (finite sets) |
| `Rank.lean` | `tie_rank`, `involution_fixed_dim` | rank of a tie system `= n − components` (R1.1); `dim Fix(P) = (n + tr P)/2` for `P² = 1` (R1.5) |
| `Layers.lean` | `painter_interval_iff` | the iff of R4.1 for a finite family of sets |
| `Compositing.lean` | `completion_removes_seam` | the algebra of R4.1b |
| `Encoding.lean` | `encoder_optimal` | the table of R4.6, the gate's counting rules as a function, optimality by `decide` |

### Rust

* `crates/inkvec-fit/src/design/` (new): `evidence.rs` (R0.2: Occam terms from the Fisher
  information of a constrained least-squares fit, the tangent-space determinant);
  `constraints.rs` (R1: the vocabulary, exact rational rank for the linear layer, numerical
  rank with the Weyl margin for the rest, the constrained Gauss–Newton refit);
  `ties.rs` (R2.5); `grid.rs` (R2.6–R2.8, leave-one-out); `relations.rs` (detectors and the
  R2.1 test for axis, 45°, parallel, perpendicular, G1, concentric, equal radius, mirror,
  repetition); `prior.rs` (R3's table, generated).
* `multimodel.rs`: each segment's cost becomes `½χ² + Σ Occam(σ_i) − ln P(kind | style)`,
  joins decided by R2.1 (G1 against a corner) instead of the `G1_BREAK_DEGREES` ramp. The
  dynamic program keeps its form but indexes the boundary chain's windows instead of points
  (`BoundaryLikelihood::run_moments` for lines and polynomials, `chi2_run` otherwise), with
  breakpoints at window borders, guard windows charged to the vertex term, and a joint
  polish of each vertex's end segments where corners, fillets and continuations are tested
  (interface, answers 2 and 5).
* `merge.rs`, `ribbon/refine/merge.rs`, `mirror_fit.rs`, `harmonize.rs`, `editable.rs`: their
  acceptance tests (`½Δχ² < λΔk`, IoU, `3σ + slack`) become R2.1, one rule for all.
* `crates/inkvec-cli`: a `layers` stage (R4.1–R4.3: order by T-junctions and bound, completion
  by candidates) before stacking in `emit.rs`, replacing `seams.rs`'s underlap where a
  completion exists; the encoder table of R4.6 and the design unit and decimals in
  `pathdata.rs` and `post.rs`.

### How Lean constrains the Rust

1. **Generated vectors.** `#eval` in Lean writes `formal/InkvecTheory/vectors/*.json`: the
   encoder table (R4.6), the snap and tie thresholds at a grid of `(σ, g, π, α)`, the optimal
   tie partitions of small inputs found by exhaustive enumeration, layer-interval examples
   with their verdicts. `cargo test` (`crates/inkvec-fit/tests/lean_vectors.rs`) reads them
   and compares bit for bit (rationals) or to a stated tolerance (logarithms).
2. **Generated prior.** `bench/theory/design_prior.py --emit-rust` writes `prior.rs`; a test
   recomputes its hash from the corpus so the table cannot drift from the counts.
3. **Debug assertions of the inequalities the proofs use.** A nested refit never fits better
   (`Δχ² ≥ −ε`, `merge_chi2_increase`); `k = n − rank ≥ 0` and the Weyl gap holds where a
   rank is decided; an accepted constraint's `½Δχ²` is under its `τ`; a completed shape lies
   in its interval (checked on a raster in debug builds); the encoder's count never exceeds
   an alternative's.

### Measurement

The gate (`bench/ci_gate.py`): parameter ratio per family toward 1.0 or below; dE00
non-inferior within its margin; the gated geometric match `geom`
(`bench/inkvec_bench/geomatch.py`: mean edge displacement from the artist's file by
symmetric-difference area over the artist's boundary length, in input px; Quality today
0.056 px at 128 px, 0.093 at 512, 0.097 at 512 opaque) with its reported `geom_far`; corner
precision and recall and segment-type agreement where the gate reports them; and the design
statistics of R4.8. Where this chain should move `geom`: exact grid numbers and ties
(R2.5–R2.8) put construction points on the artist's own numbers (three quarters of the
positional error removed in the grid families, in the simulation); the right corners,
continuations and segment kinds (R2.9, the style prior) remove the structural part of the
displacement, which at today's 0.06–0.1 px is most of it; completion (R4.2) moves no visible
edge, so its gain is in the count, not in `geom`. The search/model diagnostic
(`bench/theory/oracle_cost.py`) re-run with `L` as the objective: at 128 px the share of
icons where the objective prefers the artist's file should rise from 129/246, the remaining
gaps becoming search errors that R4.3's search addresses.

**Order of work, by the share of the artist's parameters each reaches:** (1) layers and
completion (R4.1–R4.3): 44–47 % of the emoji families' parameters are in partly covered
elements, and this needs nothing new from the boundary chain (the visible partition and
per-layer compositing at junction pixels); (2) ties, grid and axis constraints (R2.5–R2.8):
the exact numbers of the grid families, and `geom`; (3) the objective in the dynamic
program and merges (R0.2, R2.1), with the style prior on segment kinds, on the boundary
chain's window evaluator once it lands (the program over windows, answer 2 of the
interface); (4) the encoder (R4.6): design unit, `H`/`V`/`S`, primitives after snapping,
`<use>`; (5) the style variable (R3.5d).

## Phase 2, milestone 1: layers and completion

**What runs.** `crates/inkvec-cli/src/layers.rs`, in Quality mode between stacking and the
seams; `INKVEC_COMPLETION=0` is the A/B switch. Faces are visited from the top down. For
each opaque face with something painted over it, the stage proposes candidate shapes and
keeps the cheapest one the checker certifies. The candidates are:

* a circle, ellipse or rectangle through the boundary the face owns, or round its lower
  bound;
* the face's own rings with each covered run replaced: the corner of the owned lines either
  side, a chord a margin under the cover, the run offset under the cover, a bulge, or a
  **corner cut**.

A corner cut drops a chamfer and extends its neighbouring lines to meet under the cover. A
blur removes the tip of an acute corner, the fit writes what is left as one more segment,
and under a cover the tip can be restored for free.

**The bounds.**

* **Upper bound:** `H = V ∪ U` exactly (no margin).
* **Lower bound:** `L = V`, plus the half pixel under the cover that the underlap reached,
  tapered over 12 px towards a third paint.
* **`V` includes the seam.** A stroke can be drawn narrower than the face above's region in
  the map. The gap between them is painted today by the lower face's underlap, so `V`
  counts it as shown.
* **Blurred inputs.** `L \ N ⊆ E ⊆ H ∪ N`, where `N` is the junction zone: discs of four PSF
  radii round each junction, and only within one radius of the bounds. Its `psf_radius`
  comes from the noise model and is 0 on a clean intake. A wedge of angle `θ` loses its tip
  to a blur of radius `r` over `r/sin(θ/2)`, which is four radii at 29°.

**What is certified, and what is trusted.**

* **Checked.** The painter's interval is proved in Lean (`painter_interval_iff`,
  `replace_in_interval`; the seam algebra in `seam_of_exact_outline` and
  `junction_residue`). On every scan line the solver proposes, for each span of `L`, the
  chain of spans of `E` that covers it, and for each span of `E` a chain of spans of `H`.
  Each pair is accepted only by the generated interval kernels `span_within` and
  `span_link` (`inkvec_verified::generated::design`), which are outward rounded and follow
  `chain_cover` and `line_cover`. The slack is 0.1 px at each crossing. Neighbouring spans
  may be bridged across a crack narrower than 0.2 px, which two regions computed from
  different polygons leave where they meet. A refusal keeps the face as it was.
* **Trusted.** The scan conversion of shapes into spans (flattened within 0.02 px, lines
  0.25 px apart) and the region algebra that builds `V`, `U`, `L` and `H`.
* **Known limit.** A horizontal edge that moves less than a line spacing is not seen by the
  check. That is why the rectangle candidate puts each side on the face's own vertices when
  one lies that close, and falls back to the sampled box only when that fails.

**Canary** (`openmoji/1F3F4-E0069-E0074-E0062-E0061-E007F`, artist 20 parameters):

| tier | completion off | completion on |
|---|---|---|
| 512ss | 54 | 20 |
| 512ssop | — | 26 (20, plus the white page as a `rect`) |
| web | 112 | 68 |
| web, sigma ×2 | 60 | 38 |

On `web` the two triangles keep a chamfer at each acute corner. Under the honest `σ` the
fit writes each as a pentagon, two vertices more per corner. The corner cut would remove
them, but at those tips the blurred junction gave a sliver of the cyan face below to the
planar map. Completing the tip would paint over that sliver, so the exact interval refuses
it. The junction zone `N` is what allows it once `psf_radius` arrives.

**Design statistics** (`bench/theory/design_stats.py`, 246 screen icons, quality mode):

| | gate/icon | line | cubic | arc | axis exact | axis ≤ 0.5° | 45° | other angle | on design grid | half px | ties |
|---|---|---|---|---|---|---|---|---|---|---|---|
| artist | 367 | 0.290 | 0.669 | 0.040 | 0.504 | 0.019 | 0.039 | 0.423 | 0.184 | 0.140 | 0.533 |
| trace 512ssop | 373 | 0.608 | 0.198 | 0.194 | 0.100 | 0.084 | 0.005 | 0.795 | 0.100 | 0.169 | 0.456 |
| trace web | 415 | 0.717 | 0.211 | 0.072 | 0.045 | 0.077 | 0.001 | 0.863 | 0.059 | 0.121 | 0.508 |

The angle shares are of lines. Primitive elements are 10 % of the artist's elements and 25 %
of a trace's. The traces are not yet written the way the artists draw:

* **Angles.** The artists put half of their lines on an axis; a trace puts 10 %, and 4.5 % on
  `web`. Another 8 % of a trace's lines lie within half a degree of an axis: these are the
  axis constraints of milestone 2.
* **Grid.** A trace puts half as many numbers on the design grid as the artist, and a third
  as many on `web`.
* **`web` against clean.** The gap widens on every count. The trace moves toward lines in
  general directions, which is the noise-chasing of an under-estimated `σ` (R0.8).
* **Counts by family.**
  * noto-emoji falls to 850 per icon on `web` from 1059 clean (artist 1066): the blur hides
    detail.
  * openmoji rises to 856 from 347 (artist 323). Its inks multiply on `web` (one icon has
    49 elements against 7 clean), which is the front end's palette under noise and belongs
    to that agent.
  * lucide rises to 72 from 54 (artist 40).

## Where the present code stands

| link | inkvec today | what changes |
|---|---|---|
| R0 | `½χ² + λ·P`, `λ = ln(extent/precision)` (`cost.rs`, `FitConfig`), used by the dynamic program, merges, strokes and mirrors | per-coordinate Occam terms from B4's precision; the prior |
| R1 | no constraint system; constraints imposed one at a time by the passes that need them (`mirror_fit.rs`'s fundamental domain, `merge::snap` in research builds) | one vocabulary, exact and certified ranks, derived coordinates |
| R2 | fixed tolerances: `editable.rs` (`3σ + slack`), `harmonize.rs` (mask IoU, then evidence), the G1 break ramp | one Bayes-factor test with error rates; ties and grid jointly |
| R3 | `editable.rs`'s measured handle-angle prior (29 % of artist handles within 0.05° of an axis) | the counted model of R3 |
| R4 | `ribbons.rs` (stroke or fill), `primitives.rs`/`choice.rs`, `emit.rs` stacking, `seams.rs` underlap, `occlusion.rs` T-junction order, `<use>` from `harmonize`; fixed decimals | order and completion (R4.1–R4.3), the encoder table, the design unit |

## References

* Aldous, D. J. (1985). Exchangeability and related topics. *École d'Été de Probabilités de
  Saint-Flour XIII*, Lecture Notes in Mathematics 1117. doi:10.1007/BFb0099421.
* Carlier, A., Danelljan, M., Alahi, A., Timofte, R. (2020). DeepSVG: a hierarchical
  generative network for vector graphics animation. *NeurIPS*.
* Ellis, K., Ritchie, D., Solar-Lezama, A., Tenenbaum, J. B. (2018). Learning to infer
  graphics programs from hand-drawn images. *NeurIPS*.
* Ewens, W. J. (1972). The sampling theory of selectively neutral alleles. *Theoretical
  Population Biology* 3(1). doi:10.1016/0040-5809(72)90035-4.
* Fisher, W. D. (1958). On grouping for maximum homogeneity. *JASA* 53(284).
  doi:10.1080/01621459.1958.10501479.
* Hoeffding, W. (1963). Probability inequalities for sums of bounded random variables.
  *JASA* 58(301). doi:10.1080/01621459.1963.10500830.
* Igarashi, T., Matsuoka, S., Kawachiya, S., Tanaka, H. (1997). Interactive beautification:
  a technique for rapid geometric design. *UIST*. doi:10.1145/263407.263525.
* Kass, R. E., Raftery, A. E. (1995). Bayes factors. *JASA* 90(430).
  doi:10.1080/01621459.1995.10476572.
* Kellman, P. J., Shipley, T. F. (1991). A theory of visual interpolation in object
  perception. *Cognitive Psychology* 23(2). doi:10.1016/0010-0285(91)90009-D.
* Krichevsky, R., Trofimov, V. (1981). The performance of universal encoding. *IEEE
  Transactions on Information Theory* 27(2). doi:10.1109/TIT.1981.1056331.
* Lee, J. M. (2013). *Introduction to Smooth Manifolds*, 2nd ed. Springer.
* Li, Y., Wu, X., Chrysanthou, Y., Sharf, A., Cohen-Or, D., Mitra, N. J. (2011). GlobFit:
  consistently fitting primitives by discovering global relations. *ACM TOG* 30(4).
* MacKay, D. J. C. (1992). Bayesian interpolation. *Neural Computation* 4(3).
  doi:10.1162/neco.1992.4.3.415.
* Nandi, C., Willsey, M., Anderson, A., Wilcox, J. R., Darulova, E., Grossman, D., Tatlock, Z.
  (2020). Synthesizing structured CAD models with equality saturation and inverse
  transformations. *PLDI*. doi:10.1145/3385412.3386012.
* Natarajan, B. K. (1995). Sparse approximate solutions to linear systems. *SIAM Journal on
  Computing* 24(2). doi:10.1137/S0097539792240406.
* Pavlidis, T., Van Wyk, C. J. (1985). An automatic beautifier for drawings and
  illustrations. *SIGGRAPH*. doi:10.1145/325165.325240.
* Rissanen, J. (1978). Modeling by shortest data description. *Automatica* 14(5).
  doi:10.1016/0005-1098(78)90005-5.
* Schwarz, G. (1978). Estimating the dimension of a model. *Annals of Statistics* 6(2).
  doi:10.1214/aos/1176344136.
* Tierney, L., Kadane, J. B. (1986). Accurate approximations for posterior moments and
  marginal densities. *JASA* 81(393). doi:10.1080/01621459.1986.10478240.
* Toro-Vizcarrondo, C., Wallace, T. D. (1968). A test of the mean square error criterion for
  restrictions in linear regression. *JASA* 63(322).
* Wallace, C. S., Freeman, P. R. (1987). Estimation and inference by compact coding. *JRSS B*
  49(3). doi:10.1111/j.2517-6161.1987.tb01695.x.
* Weyl, H. (1912). Das asymptotische Verteilungsgesetz der Eigenwerte linearer partieller
  Differentialgleichungen. *Mathematische Annalen* 71. doi:10.1007/BF01456804.
