# The optimal vectorizer, informally

> What the best possible raster-to-vector tracer would compute, whether anyone has found
> it, how far inkvec is from it stage by stage, and which approximations to make next.
> Informal on purpose: the claims that are theorems are proved in
> [`formal/InkvecTheory`](../../formal/InkvecTheory/README.md) and named here; the rest is
> argument, with the measurement that would test it.

## Summary

* **Nobody has found or proved an optimal algorithm for the general problem**, and none
  can exist in general: the problem contains NP-hard segmentation, and with an
  unrestricted prior it is the uncomputable problem of finding the shortest program that
  draws an image. But the *optimum itself* can be stated exactly: the most probable SVG
  given the pixels, under the exact rendering model and a model of how people write SVG
  (§2).
* **For clean artwork the problem splits in two.** The *picture* (where the boundaries
  are, what colour the fills are) is determined by the pixels down to the anti-aliasing
  and quantisation floor, and inkvec is already near that floor (§3, §4). The *program*
  is not: many SVGs draw the same picture (strokes or fills, one arc or three, a rectangle
  under two triangles or two cut-out shapes), and pixels cannot choose between them. Only a
  prior over human SVGs can (§5).
* So the remaining distance to the optimum is almost entirely **representation**: the
  prior, and the search over structures it ranks. That is what "naturality" means, and why
  cutting parameters has lowered colour error rather than raised it: among descriptions
  that draw the truth, the one with the fewest parameters is the most faithful
  (`natural_model_closer`, proved).
* **Measured:** a diagnostic splits each icon's gap into *search error* (our own objective
  ranks the artist's file first and the search missed it) and *model error* (the
  objective prefers something else) (§6). At 512 px it is search on 85-88 % of the icons;
  at 128 px the objective prefers the trace on about half, where the prior must decide.
* **Plan:** build the optimum as a slow reference ("oracle mode") and measure every fast
  stage by its gap to it; add the moves the search lacks (layers completed behind what
  covers them, merges for filled outlines, more than one hypothesis per structural
  choice); then a code-length prior that reproduces the artists' statistics, whose most
  probable output is a canonical "normal form" (§5, §7).

## 1. The problem

An artist writes an SVG document `S`: shapes, their paint, their order. A renderer draws
it, anti-aliases it (exactly: each pixel gets the area of each shape inside it; in
practice 8×8 or 16×16 supersampling, or analytic coverage), and the result is quantised to
8 bits, perhaps compressed. We see the raster `R` and must write `Ŝ`.

Three things make `Ŝ` good, and the gate measures all three:

1. **Fidelity.** `Ŝ` draws the same picture as `S`, at any resolution (the gate renders
   both at 1024 px and compares by CIEDE2000).
2. **Compactness.** `Ŝ` uses about as many numbers as `S` (the gate's parameter ratio).
3. **Naturalness.** `Ŝ` is written the way a person writes SVG: a line where they drew a
   line, a stroke where they used a pen, a circle where they used a circle, layers where
   they layered. This is what makes an SVG editable, and it is what "clean" means.

`S` cannot be recovered in general, for two separate reasons:

* **Picture ambiguity.** Different pictures give identical pixels. The box filter has a
  null space: zigzags along a pixel border, sub-pixel ripples, anything that moves area
  within a column without changing the column's total (`sine_ripple_invisible`,
  `polyline_kernel`, `column_fiber`). Detail below the pixel is gone.
* **Program ambiguity.** Different SVGs give identical pictures. A ring is a stroked circle
  or a black disc under a white one; a 270° arc is one arc or three; a flag's background
  is a rectangle under two triangles or the complement of the triangles.

Picture ambiguity is resolved by a prior on *shapes* (smoothness, few segments). Program
ambiguity is resolved only by a prior on *programs*: how people write SVG.

## 2. The optimum

Bayes' rule says what the best decision is. With a likelihood `P(R | S)` (how probable these
pixels are if the artist wrote `S`), a prior `P(S)` (how probable it is that a person writes
`S`), and a loss `ℓ(Ŝ, S)` (what a wrong answer costs), the optimal output minimises the
expected loss under the posterior:

```
Ŝ* = argmin over Ŝ of   Σ_S  P(S | R) · ℓ(Ŝ, S),        P(S | R) ∝ P(R | S) · P(S)
```

For clean art the posterior over *pictures* is sharply peaked (§3), so this is very nearly
the most probable description (MAP). Taking logarithms:

```
Ŝ* ≈ argmin over S of   ½ χ²(R, raster(S))   +   CodeLength(S)
```

* `½ χ²` is the likelihood: the squared difference between the pixels and the pixels `S`
  would make, in units of the noise (8-bit quantisation is ±½ level, a standard deviation of
  0.11 % of full scale; the renderer's own anti-aliasing differs from the artist's by more,
  and that mismatch is the real noise floor).
* `CodeLength(S) = −log P(S)` is the prior, in nats: how surprising this SVG would be from a
  person.

That is the minimum description length principle (Rissanen 1978), and it is the objective
inkvec already optimises, with two differences that matter:

1. **inkvec measures `χ²` on boundary points**, extracted from the pixels first, not on the
   pixels themselves. Where extraction is exact (smooth edges between two fills, the
   column-sum theorem) nothing is lost. Where it is not (corners, junctions, strokes under
   two pixels, three colours in one pixel) the two-step estimate is a worse estimator than
   the pixels allow.
2. **inkvec's prior is flat:** `CodeLength = λ · (number of parameters)`, every parameter
   priced the same, `λ = ln(extent / precision)`. That price is not arbitrary. It is the
   Laplace approximation of the code length of a real number known to within `precision`
   in a range `extent` (Rissanen's optimal precision), so it is the right price for a
   coordinate a person picks uniformly at random. People do not: they put points on a grid,
   draw lines at 0°, 45° and 90°, reuse coordinates, repeat radii, mirror halves. A prior
   that knows this charges a point on the design grid `log(grid points)` instead of
   `log(extent / precision)`, a horizontal line one number instead of two, a repeated
   radius almost nothing. Under such a prior the MAP *is* the canonical form (§5).

## 3. What is known

| Question | Answer | Where |
|---|---|---|
| Is the forward model known? | Exactly: a pixel's coverage is the area of the shape in it; a column of coverages sums to the boundary's mean height over that column, whatever its slope or curvature | `column_sum_eq_average`; Puckett (1991), Trujillo-Pino et al. (2013) |
| Can sub-pixel detail be recovered? | No: the box filter's null space is exactly the zigzag (for one vertex per pixel border) and any area-preserving rearrangement within a column | `polyline_kernel`, `column_fiber` |
| Is a clean boundary determined by its pixels? | Yes: a polynomial boundary of degree `< n` is fixed by `n` columns; a line by two; a cubic by four | `polynomial_boundary_determined`, `strip_reading_exact` |
| How precisely? | Per column, 8-bit quantisation alone allows about 0.001 px; 8×8 supersampling allows up to 1/16 px of bias; a ½-crossing reading is biased up to 0.086 px; thresholding first, up to ½ px | `quantize_minimax`, `ss_column_sum_close`, `levelSetBias_max`, `binarize_minimax` |
| Is optimal segmentation tractable? | Along one chain with fixed candidate breakpoints, yes: an exact dynamic program (Kolesnikov 2012), which inkvec runs per edge. Over a whole image with a region prior, no: Potts energy minimisation is NP-hard (Boykov, Veksler, Zabih 2001) | `inkvec_fit::multimodel` |
| Is the shortest program computable? | Not under an unrestricted prior (Kolmogorov complexity). Under a fixed grammar (SVG's elements) and a fixed prior it is a finite, if large, search | — |
| Does fewer parameters mean less fidelity? | Not when the smaller model still draws the truth: its least-squares fit is closer to the truth for every noise, by exactly the noise in the dropped directions; each surplus parameter costs one `σ²` of squared error | `natural_model_closer`, `natural_model_gain`, `noise_absorbed` |
| What do people judge a vectorisation by? | Accuracy, simplicity, continuity and regularity (straight, parallel, symmetric, axis-aligned), measured in user studies | Hoshyari, Dominici, Sheffer, Carr, Wang, Ceylan, Shen (2018), *Perception-driven semi-structured boundary vectorization*, SIGGRAPH; Favreau, Lafarge, Bousseau (2016) |
| Has anyone optimised SVGs against the pixels directly? | Yes, locally, without a prior on programs: DiffVG (Li et al. 2020), LIVE (Ma et al. 2022). They fit, but the output is not what a person would write | — |
| Has anyone learned the human prior? | Yes, as generators: DeepSVG (Carlier et al. 2020), Im2Vec (Reddy et al. 2021), StarVector (Rodriguez et al., 2023 onward). They write in a human style but do not match the pixels to sub-pixel precision | — |

No published method combines the exact likelihood, a prior over human programs, and a
global search over structure, and that combination is the optimum of §2. Each piece
exists separately.

## 4. How far inkvec is from the optimum

Measured on the gate's 246 icons (Quality, stroke detection on, after this round's
merges), against the artist's own files:

| Stage | What the optimum does | What inkvec does | Gap |
|---|---|---|---|
| Palette, regions | The fewest flat colours (or gradients) that explain every pixel as a mixture of at most two (three at a junction) | Palette walk, unmixing, gradient fits | Small: interior fill colour is exact for most families; gradients (noto-emoji) are the main interior error (`bench/theory/attribution.py`) |
| Boundary points | The best unbiased estimate from the pixels | Column sums where two fills meet (unbiased, variance `k·σ²`); probes elsewhere | At the floor on smooth edges: equivalent displacement about 0.025 source px at 512 px, the 8×8 supersampling floor. Not at corners, junctions, thin strokes |
| Curves per edge | MAP segmentation under the human prior | Exact dynamic program under the flat prior; arcs at most 120° | The prior, and the 120° cap |
| Structure | MAP over strokes vs fills, layering, primitives, symmetry, repetition | Greedy moves verified by description length: strokes, merges, primitives, mirrors, harmonisation | Greedy search, and moves it does not have (amodal completion, continuation through a junction, shared coordinates) |
| Emission | The canonical form of the MAP | Fixed decimals, faces in paint order | No canonicalisation |

What the numbers say: 83-92 % of the remaining colour error lies within one source pixel
of a smooth edge, at the measurement floor; corners hold about 4 % and junctions 1 %. The
parameter ratio is 0.95 at 128 px, 1.17 at 512 px and 1.41 on opaque 512 px logos. The
remaining distance is in representation, not in fitting.

The gate already shows the theorems at work. Writing stroke-drawn faces as strokes, and
merging what the solve split, *raised* the trace's residual against its own input
(`self_res` +2.5 % to +3.5 %) while *lowering* its error against the artist's file (dE00
−8 % to −11 %). That is `merge_chi2_increase` and `natural_model_closer` side by side: the
smaller true model fits the pixels a little worse, because it no longer absorbs their
noise, and the truth better, for the same reason. A tracer tuned to its own residual alone
would have refused both changes.

Two concrete examples of representation the search cannot reach today:

* `openmoji/1F3F4-E0069-E0074-E0062-E0061-E007F` (a flag): the artist drew a blue
  rectangle and two white triangles over it (20 parameters). inkvec draws the blue *visible
  part*, a complicated polygon, and the triangles (54). The move missing is **amodal
  completion**: a face cut by the faces painted over it is the simplest shape that
  continues behind them.
* `synthetic/mosaic_grid6`: every corner of a cell where two cells sit side by side gains
  a 6 px taper, which is the underlap that hides anti-aliasing seams (`crates/inkvec-cli/src/seams.rs`).
  It costs 1.8 % of the parameters over the screen set, a choice made for the renderer, not
  for the picture.

## 5. The prior: writing SVG the way a person does

A prior over SVG programs is a code: frequent choices get short codewords. The MAP under
it is the description a person would most probably have written. A sharp enough prior
makes that choice deterministic, so the same picture always comes back as the same file:
a single normal form. This is the "mode collapse to one clean format" asked for, and it is
a feature here: two traces of one logo should be the same file.

What the prior has to know, all measurable on the artists' files in the corpus:

* **Elements.** Line sets are strokes of one width; circles, ellipses and rounded
  rectangles are written as such; emoji are layered shapes.
* **Segments.** Lines where straight, circular arcs where circular (of any sweep), cubics
  otherwise, joined smoothly (G1) where smooth.
* **Numbers.** Coordinates on the design grid when there is one (lucide and material:
  a 24-unit grid with half-unit steps); angles of 0°, 45°, 90°; coordinates shared between
  points (a vertical edge shares its x); radii and widths from a short list; halves that
  mirror exactly; repeated parts identical.
* **Layers.** Shapes completed behind the shapes that cover them, back to front.

As a code length: a coordinate on a detected grid costs `log(grid points)`; a coordinate
equal to one already written costs `log(how many have been written)`; a free one costs
`log(extent / precision)` as now; an axis-aligned line costs one number; an arc's sweep is
free (SVG's flags). The grid itself is a parameter of the document, paid once, and is only
used when it pays for itself, so an icon not designed on a grid loses nothing.

**The inkvec normal form**, which the MAP under that prior produces:

1. Back-to-front layers, each shape completed behind the shapes over it.
2. A primitive where the shape is one; a stroke where the face is a pen line of one width.
3. Lines, arcs and cubics as above, the fewest that fit; smooth joins written smooth.
4. Coordinates snapped to the document's grid and axes wherever that is within the
   measurement's own error; exact symmetry and exact repetition where the drawing has them.
5. One deterministic writing order and number format.

**Keeping the statistics the same.** The prior can be checked without any icon's ground
truth: the distribution of these features over inkvec's output should match their
distribution over the artists' files (element kinds, segment kinds, angle histogram,
fraction of coordinates on a grid, symmetry rate). A distance between the two histograms
belongs in the gate as a reported axis, next to the parameter ratio, which is the first
of these statistics.

**Hand-written or learned.** Start hand-written: each rule above is a few lines, can be
tested alone, and is priced in the same currency as everything else. A learned model of
SVG syntax (a small sequence model over path commands) is the later step, used only to
*score* candidate descriptions, never to generate them, so the pixels keep the last word.

## 6. Search error or model error: the diagnostic that decides

For each icon, compute the objective of §2 twice: on inkvec's output, and on the artist's
own file fitted to the same pixels (its structure kept, its numbers refined).

* **The artist's file scores better** (lower cost): a better description exists and the
  search did not find it. That is a **search error**: add moves, widen the search.
* **inkvec's output scores better, but the artist's file is better on the gate**: the
  objective prefers the wrong description. That is a **model error**: fix the prior or the
  loss.
* **Both agree**: the icon is at the optimum of this objective.

The split is standard in machine-translation decoding, where it decides whether to
improve the search or the model. Here it turns "where is the remaining gap" from a guess
into a per-icon count (`bench/theory/oracle_cost.py`).

**Measured (2026-10-10, Quality, stroke detection on).** The fit is the squared difference
from the input's pixels with each file rendered at the input's size (over white and black,
so transparency counts; over white only for the opaque tier), in units of the artist
file's own per-pixel residual, the renderer floor; a parameter costs inkvec's `λ`. The
artist's file is the truth, so it always fits to the floor; the question is whether the
objective *prefers* it.

| tier | objective prefers the artist's file | prefers the trace | the trace is larger and fits worse |
|---|---|---|---|
| 128 px | 129 / 246 | 117 | 64 |
| 512 px | 208 / 246 | 38 | 128 |
| 512 px, opaque | 216 / 246 | 30 | 165 |

At 512 px the objective already ranks the artist's description first on 85-88 % of the
icons, and the trace did not find it: those gaps are **search errors**, and they stay the
majority with the noise scale taken 16 times larger (159 of 246). Most are wrong structure
rather than wrong numbers: the trace is both larger and further from the pixels on 128
icons (noto-emoji's shaded emoji lead, 1.5-2× the artist's parameters at several times
its residual), which is what a missing representation (a gradient, a layer, a completed
shape) looks like. At 128 px the pixels say less, and the objective prefers the trace on
nearly half the icons: there the choice between descriptions is the prior's, which is the
case for a prior that knows how people draw (§5). So the order of work follows: search
moves first for 512 px and above, the prior for small rasters.

## 7. From the optimum to fast approximations

The optimum is too slow to run on every trace, but it can run once, as a reference: an
**oracle mode** that searches much harder (several structural hypotheses kept at each
choice, pixel-space refinement at corners and junctions, every merge and completion
tried), slow enough for minutes per icon. Then every fast stage is judged by its distance
to that reference rather than to the artist's file, which separates "the fast stage
approximates badly" from "the objective is wrong".

Ranked by expected gain against cost:

1. **The search/model diagnostic** (§6), now measured: at 512 px the gap is mostly search.
   Run it on every change that claims a structural gain.
2. **A grid-, axis- and repetition-aware code length, with snapping.** The prior of §5,
   hand-written. Expected: naturalness and editability directly; fidelity where a snapped
   value is the artist's exact one; fewer parameters where a shared value replaces a free
   one.
3. **Amodal completion.** A face cut by faces painted over it is offered as the simplest
   shape continuing behind them, and kept when the description length falls. The largest
   remaining parameter cost in opaque logos and emoji (the flag: 54 → about 20).
4. **Merges for filled outlines.** The stroke solve's merges (this round) applied to
   outlines: arcs beyond 120°, collinear lines, straight cubics; 1.5-5 % of the
   parameters in the filled families, where the naturality count finds them.
5. **Search beyond greedy.** Keep a few hypotheses at each structural choice (stroke or
   fill, layer order, which merge first) instead of one: a beam. Split and merge moves are
   already the moves of reversible-jump MCMC (Green 1995) taken at zero temperature; a beam
   or annealing is the next step toward the optimum.
6. **Pixel-space polish where extraction fails.** At corners, junctions and strokes under
   two pixels, fit the local geometry to the pixels themselves, with exact coverage as the
   forward model. These are 5 % of today's error, but they are where a person looks first.
7. **Perceptual precision.** An edge between two similar colours needs less positional
   precision than an edge between black and white: its displaced area costs `contrast ×
   area` in colour error. Pricing each edge's parameters at its own precision (`λ_edge =
   ln(extent / precision_edge)`, `precision_edge` at a just-noticeable difference over its
   contrast) spends the parameters where a viewer would see them.

## 8. Can the optimum be found?

For a fixed grammar (SVG's elements and segments), a fixed prior and a fixed perceptual
loss, the optimum is a well-defined finite search, and on icon-sized inputs a slow
exhaustive-enough search is feasible: that is the oracle mode. What cannot be had is a
proof that any *fast* algorithm reaches it on every input, because the general problem is
NP-hard. The workable notion of "optimal" is therefore empirical and measurable: the fast
tracer's gap to the oracle, per icon, should be zero on most icons and small on the rest,
and the oracle's own choices should match the artists' files wherever the pixels allow.

The theorems settle what can be: the picture is recoverable down to the floor, and a
true simplification never costs fidelity. Everything else is the prior, which the
artists' own files measure.

## References

* Rissanen, J. (1978). Modeling by shortest data description. *Automatica* 14(5).
  doi:10.1016/0005-1098(78)90005-5.
* Boykov, Y., Veksler, O., Zabih, R. (2001). Fast approximate energy minimization via graph
  cuts. *IEEE TPAMI* 23(11). doi:10.1109/34.969114. (Potts energy minimisation is NP-hard.)
* Kolesnikov, A. (2012). Segmentation and multi-model approximation of digital curves.
  *Pattern Recognition Letters* 33(9). doi:10.1016/j.patrec.2012.01.021.
* Green, P. J. (1995). Reversible jump Markov chain Monte Carlo computation and Bayesian
  model determination. *Biometrika* 82(4). doi:10.1093/biomet/82.4.711.
* Hoshyari, S., Dominici, E. A., Sheffer, A., Carr, N., Wang, Z., Ceylan, D., Shen, I.-C.
  (2018). Perception-driven semi-structured boundary vectorization. *ACM TOG* 37(4).
  doi:10.1145/3197517.3201312.
* Favreau, J.-D., Lafarge, F., Bousseau, A. (2016). Fidelity vs. simplicity: a global
  approach to line drawing vectorization. *ACM TOG* 35(4). doi:10.1145/2897824.2925946.
* Li, T.-M., Lukáč, M., Gharbi, M., Ragan-Kelley, J. (2020). Differentiable vector
  graphics rasterization for editing and learning. *ACM TOG* 39(6).
  doi:10.1145/3414685.3417871.
* Ma, X., Zhou, Y., Xu, X., Sun, B., Filev, V., Orlov, N., Fu, Y., Shi, H. (2022). Towards
  layer-wise image vectorization (LIVE). *CVPR*.
* Carlier, A., Danelljan, M., Alahi, A., Timofte, R. (2020). DeepSVG: a hierarchical
  generative network for vector graphics animation. *NeurIPS*.
* Reddy, P., Gharbi, M., Lukáč, M., Mitra, N. J. (2021). Im2Vec: synthesizing vector
  graphics without vector supervision. *CVPR*.
* Rodriguez, J. A., et al. StarVector: generating scalable vector graphics code from
  images and text (2023 onward).
* Puckett, E. G. (1991); Trujillo-Pino, A., et al. (2013): see the theory page,
  [`README.md`](README.md) §2.
