# Two chains: from pixels to the fewest coordinates

> The problem of [`optimal.md`](optimal.md), split into two chains of mathematical problems,
> each link taking the previous link's output as its input. One chain recovers the picture
> (boundary solving), the other writes it with the fewest coordinates a person would use
> (representation). Each link is a problem worth a proof, sized so that no link can be
> solved by an assumption a longer link would have to justify. The two chains meet at one
> interface, which their owners negotiate.

## The goal

Write the artist's design back with **the fewest coordinates**, as close to the artist's
own file as the pixels allow: the same shapes, the same kinds of segments, the same
constraints (a horizontal line is horizontal, two radii that are equal are equal, a point
on the design grid is on it), and so the same count of numbers or fewer. Fidelity is a
constraint, not a currency: the description must explain the pixels within their noise;
among those that do, the fewest free coordinates win; among equals, the most human.

Judged three ways (`bench/ci_gate.py`): colour error against the artist's render (dE00),
parameters against the artist's file, and geometric match to the artist's file (edge
displacement by area, corner and segment-type agreement), the last being the most direct
measure of "the same design".

## Chain B: boundary solving (pixels → a likelihood over curves)

**B1. Inks and mixtures.** *In:* the raster. *Out:* the inks (flat colours; linear or radial
colour fields) and, per pixel, the mixture weights of the at most two inks it mixes (three at
a junction pixel). *Claims to prove:* the palette is identifiable from interior pixels; given
it, the two-ink weights are unique and unbiased, with an error bound under 8-bit quantisation;
a junction pixel's three weights are identifiable only with geometry from its neighbours (so
B3 needs B1's two-ink pixels around it, not the junction pixel itself); a gradient ink's field
is identifiable from its interior and edge pixels unmix against the field's value there.
*Known:* `coverage.rs`, `bench/theory/attribution.py` (interior colour error is ~0 for flat
inks; gradients are the main interior error).

**B2. A likelihood for any curve.** *In:* B1's weights along a two-ink run. *Out:* the exact
likelihood of an arbitrary candidate boundary curve, not of extracted points. *Claims to
prove:* under the box filter and independent quantisation noise, the column (or row) sums
of one ink's weights are a sufficient statistic for the boundary within the run
(`column_sum_eq_average`); the log-likelihood of any curve `f` is a quadratic in the
differences `S_col − ∫_col f`, with known variances, so `χ²(curve)` is computable in time
linear in the run's columns; its Fisher information per unit length bounds the precision of
any curve parameter (Cramér–Rao). *Known:* `planar/strip.rs`, `Coverage`, `Noise`,
`Supersampling`.

**B3. Where runs end: corners, junctions, thin strokes.** *In:* B1's weights near a point
where B2's assumptions fail (a tangent discontinuity, three inks, a feature under two pixels
wide). *Out:* a local parametric model fitted to the raw weights (a wedge: two rays and a
vertex; a junction: `k` rays and a vertex; a thin stroke: a centreline and a width), with its
posterior. *Claims to prove:* each model is identifiable from the pixels within a stated
radius; the estimator's bias and variance, against the Cramér–Rao bound; when the model
should be refused (curvature within the radius). *Known:* `planar/junctions.rs`,
`ThinStroke`, `LevelSetBias` (the ½-crossing rounds corners), `bench/theory/attribution.py`
(corners ~4 %, junctions ~1 % of colour error, but visually first).

**B4. The planar map as one likelihood.** *In:* B2's runs and B3's vertices. *Out:* a planar
map whose total log-likelihood is a sum of local terms, one per run and one per vertex, with
the coupling between a vertex and the runs that meet it made explicit. *Claims to prove:* the
decomposition is exact up to stated cross terms (pixels shared between a vertex model and a
run); the box filter's null space (`polyline_kernel`) is a property of the parametrisation,
not of the data, and vanishes for curves with no more coefficients than columns
(`polynomial_boundary_determined`); the precision of every coordinate the representation
might write. *This is the interface:* what chain R evaluates every description against.

## Chain R: representation (a likelihood → the fewest human coordinates)

**R1. Constraints and degrees of freedom.** *In:* B4's map and likelihood. *Out:* a
vocabulary of design constraints (incidence, G1 tangency, axis alignment, parallel and
perpendicular, equal length and radius, concentricity, mirror and rotational symmetry,
repetition by translation, a lattice or design grid, one stroke width) and, for any set of
them, the free coordinates that remain. *Claims to prove:* at a regular point the free
coordinates are the raw coordinates minus the rank of the constraint Jacobian; how to compute
the rank and a minimal parametrisation for the constraint graphs icons produce (decomposition
into rigid clusters); when two constraint sets give the same design (redundancy).

**R2. Which constraints hold.** *In:* R1's candidates, B4's likelihood, R3's prior. *Out:* the
accepted constraint set. *Claims to prove:* a constraint is a nested model, so imposing a true
one never moves the fit away from the truth (`natural_model_closer`) and raises `χ²` by a
`χ²_Δk` variable (`merge_chi2_increase`); the test "accept when the evidence ratio times the
prior odds exceeds one" is consistent (a false constraint's `Δχ²` grows with the boundary
length, a true one's does not) and has stated error rates at a given resolution; accepting
constraints in order of evidence and re-solving keeps the accepted set consistent (no two
accepted constraints contradict).

**R3. The human prior, small.** *In:* the artists' own files in the corpus. *Out:* prior odds
for each constraint in each context (segment kinds on either side, element kind, family), and
the document-level latent variables that change them (a design grid and its unit; one stroke
width; a symmetry axis), as a model of a few hundred numbers counted from the files, not a
neural network. *Claims to prove:* the estimate's error (Dirichlet smoothing, a
generalisation bound over icons); empirical Bayes for the document variables (a grid
inferred from some coordinates raises the prior of the rest, without double counting); that
the resulting decision rule reproduces the corpus's constraint frequencies on clean inputs
(calibration).

**R4. Structure and encoding.** *In:* R2's constrained geometry. *Out:* the SVG. *Claims to
prove:* the choice between structures (stroke or fill, layers completed behind what covers
them, primitives, repetition by `<use>`) decomposes per face up to stated couplings and can
be searched with bounds (a free-coordinate lower bound prunes); the encoder that writes a
constrained element with the fewest numbers the gate counts is optimal over its alternatives
(`<rect>`, `<circle>`, a stroke, `H`/`V`, arcs, `<use>`); and the objective itself:
lexicographic (fit within tolerance, then fewest free coordinates, then the prior) against
description length, with the conditions under which they agree.

## The interface, and what may move across it

B4's output is R's input: a map, a likelihood any curve can be evaluated against, and the
precision of each coordinate. Two things may move:

* **Vertices.** If R's constraints (incidence, tangency, a corner as two lines meeting) can be
  evaluated against B2's likelihood plus raw-pixel terms near the vertex, B3 shrinks to
  providing those terms, and corners become R2 decisions.
* **Gradients.** Whether a fill is a gradient, and which kind, is a representation choice
  (R4) on B1's evidence.

The owners negotiate these, and anything else that shortens a chain without hiding a
problem in an assumption.

## Formal, then code

Each link: an informal statement and proof first (`chain-boundary.md`,
`chain-representation.md`); the load-bearing lemmas then in Lean
(`formal/InkvecTheory`); then code that the mathematics constrains: rational constants and
decision rules generated from, or checked against, the Lean development by a test
(`cargo test`), and runtime checks (debug assertions) of the inequalities a proof relies on.
