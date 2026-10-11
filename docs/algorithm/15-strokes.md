# Stage 15 — Stroke detection (on by default; `--no-detect-strokes` turns it off)

> Finds the faces of the colour trace that were drawn as strokes — a centreline and one
> width — and writes each as a stroked path where that is the shorter description of its
> measured boundary at a fit as good as its outline's.

**Source:** `crates/inkvec-trace/src/ribbon.rs` (the per-face driver, `fit_face`) and
`crates/inkvec-trace/src/ribbon/`: `boundary.rs` (rings, inward normals, pairing),
`graph.rs` (topology), `chordal.rs` (constrained Delaunay chordal axis), `refine.rs` (the
stroke solve) and `refine/normal.rs` (its Jacobian rows and normal equations), `dist.rs` (exact distances and their gradients), `skyline.rs` (the envelope
Cholesky), `bvh.rs` (nearest-piece search), `grid.rs` (bucket grid and ray walks),
`join.rs` (joins and caps as residual models), `score.rs` (the score), `refine/merge.rs`
(merges, the splits run backwards). The decision and the SVG elements:
`crates/inkvec-cli/src/ribbons.rs`.
**Entry points:** `ribbon::fit_face()` (`crates/inkvec-trace/src/ribbon.rs:327`), called per
candidate face by `ribbons::choose()` (`crates/inkvec-cli/src/ribbons.rs:173`), which
`ribbons::stage()` (`crates/inkvec-cli/src/ribbons.rs:79`) runs unless
`--no-detect-strokes` / `Options::detect_strokes = false` (`crates/inkvec-cli/src/args.rs:199`,
`crates/inkvec/src/options.rs:112`) switches it off; for the benchmarks `INKVEC_RIBBONS=0`
switches it off and `INKVEC_RIBBONS=1` on (`crates/inkvec-cli/src/ribbons.rs:70`).
**Pipeline position:** Quality mode, colour output only (not `--mode fast`, not
`--monochrome`): after every boundary is fitted, repaired and mirrored and every face's
fill and transparency are settled, before the document is written
(`crates/inkvec-cli/src/pipeline.rs:438`). The emitter drops the faces written as strokes
from the fill tree and paints the strokes on top (`crates/inkvec-cli/src/emit.rs:181`,
`write_ribbons`, `crates/inkvec-cli/src/emit.rs:1310`).

## What problem this solves

Line art — lucide entirely, openmoji's black outlines, much of every icon set — is drawn as
centrelines with one stroke width. Traced as filled outlines, every stroke costs both of its
sides plus its caps: the artist's own lucide geometry written as outlines needs 3.87× the
artist's parameters, and the curve fitter already sits within 6% of that floor (r2-compact
research, 2026-10-02). No fitting change reaches it; only a change of representation does.
This stage is that change, face by face, and it decides each face by description length —
the same currency the fitter uses — with a fidelity bound on top.

## The passes, for one face (`fit_face`, `ribbon.rs:327`)

1. **Boundary and width by pairing** (`boundary.rs`). The face's rings after the boundary
   solve, with inward normals voted against the face mask. Every boundary point walks
   along its inward normal to the far side (`Boundary::across`, `boundary.rs:247`, a ray
   walk in `grid.rs`); an anti-parallel hit (within `COS_PAIR` = cos 25°,
   `boundary.rs:50`) is a width sample. The median's core gives the width `w0` and the
   share of the outline that is sleeve (`stroke_width`, `boundary.rs:279`). Faces with a
   share under `MIN_PAIRED_SHARE` = 0.35 (`ribbon.rs:102`) or narrower than `MIN_WIDTH` =
   2 px (`ribbon.rs:94`) are declined.
2. **Medial graph and centre samples** (`graph::medial`, `graph.rs:309`), read once per
   face: Zhang-Suen thinning of the face mask as a graph (`skeleton`, `graph.rs:325`), and
   at every node a cross-section from the nearest boundary point across to the other side
   (`cross_section`, `graph.rs:342`); a sample is reliable when the far side is
   anti-parallel and the width agrees with `w0`.
3. **Topology readings** (`hypothesis`, `ribbon.rs:481`; `TopoOptions`). Under round joins
   the readings are tried in turn while the previous one is a misfit: no rebuilt corners;
   rebuilt corners; and the **chordal axis** with rebuilt corners. Each reading keeps the
   branches' reliable cores (`cores`, `graph.rs:375`), drops spurs and merges junction
   connectors (`classify`, `graph.rs:567`), rebuilds junctions as the least-squares
   meeting point of the sleeve ends (`meeting_point`, `graph.rs:707`) and caps from the
   boundary walk round the terminal (`cap_walk`, `graph.rs:787`), continues sleeves through
   junctions (`continuation`, `graph.rs:841`) and walks the result into chains
   (`assemble`, `graph.rs:884`).
4. **Chordal axis** (`chordal.rs`). For faces whose raster skeleton has the wrong topology
   — strokes thick against their own length — a constrained Delaunay triangulation of the
   boundary points (`Cdt::new`, `chordal.rs:90`: Lawson's incremental flips with exact
   predicates, then every boundary segment forced in by Sloan's flips), the inside found
   by a flood from each segment's inner side, and the graph through chord midpoints:
   sleeve triangles link their two chords, junction triangles link their three to the
   centroid (`axis`, `chordal.rs:458`). Branches with a free end and under half a width of
   core are dropped there (the axis sends a spur into every ear of the outline).
5. **Centreline fit** (`fit_chain`, `ribbon.rs:710`). Chains are thinned to
   `CHAIN_SAMPLES_PER_WIDTH` = 8 samples per stroke width (`ribbon.rs:624`; `decimate`,
   `ribbon.rs:665`: runs of measured samples between anchors replaced by their middle
   sample with sigma `sqrt(Σσ²)/m`), then fitted by the outline fitter's MDL dynamic
   program (`inkvec_fit::multimodel`) or a whole primitive when that is cheaper, in
   parallel. A coarse fit at an eighth of the samples first declines readings whose
   centrelines already cost `COARSE_DECLINE` = 1.25× the budget (`ribbon.rs:647`).
6. **Stroke solve** (`refine::solve_adaptive`, `refine.rs:895`). Levenberg-Marquardt over
   every control point and the half-width against the boundary residual
   `r_p = (d(p, C) - h)/σ_p` (see the solve below), then up to `MAX_SPLITS` = 12
   (`refine.rs:858`) splits of the segment carrying the most residual, each kept only when
   the description length falls.
7. **Merges** (`merge::merges`, `refine/merge.rs:91`). The splits run backwards: every
   joint of a centreline is offered the segments that could replace the two it joins
   (`joint`, `refine/merge.rs:362`) -- the line through their outer ends; an arc on
   the circle of either one that is an arc, of both when they turn the same way, or through
   the three points (`arc_about`, `refine/merge.rs:431`); the cubic with the outer
   tangents, its arms fitted by Schneider's least squares (`cubic_through`,
   `refine/merge.rs:471`) -- every cubic and arc the line on its chord, and a
   closed path of arcs on one circle that circle (`common_circle`, `refine/merge.rs:596`).
   Each is measured, unsolved, against the boundary points the replaced segments explained;
   the best is kept outright when the strokes' description length already falls, else
   solved for `TRIAL_ITERS` iterations and kept when it falls then, drawable and with the
   strokes' chi-squared still within the decision's fidelity bound (the caller's
   `chi2_cap`). The fitter's arcs stop at 120° (`MAX_ARC_DEGREES`), so a 270° arc drawn
   by the artist came back as three; a cap as an arc and a short line continuing it; a
   straight run as two lines meeting at 1°. A merged arc within `HALF_TURN_BAND_DEGREES` =
   15° of a half turn is written as the exact half circle on its chord, and the solve
   refuses any arc that drifts into that band otherwise (`conditioned`,
   `refine.rs:1142`): there SVG's rebuilt centre moves by `R/k` times any rounding
   of the radius.
8. **Score and checks** (`score::score`, `score.rs:154`; `uncovered`, `ribbon.rs:432`;
   `refine::drawable`, `refine.rs:1182`): the chi-squared of the strokes' painted outline
   against every boundary point, the face's interior pixels checked as painted
   (`MAX_UNCOVERED` = 0.2% of its pixels, `ribbon.rs:425`), and no cusp (no
   micro-segment or hairpin under miter joins).

Passes 3-8 run with round joins and round caps first; when the round fit leaves a point
more than `MITER_TRIGGER` = 0.25 px out (`ribbon.rs:415`) the face is also read under miter
joins, and when the better reading has free ends and leaves a point more than
`BUTT_TRIGGER` = 0.1 of the width out (`ribbon.rs:411`), under butt caps. The cheaper by
`χ²/2 + λ·k` wins (`cheaper`, `ribbon.rs:390`).

## The stroke model (`join.rs`, `score.rs`)

A round stroke paints the points within `h` of its centreline, so a boundary point's
residual is `d(p, C) - h`. A miter join at a convex vertex paints the kite of the two offset
lines: `miter_gauge` (`join.rs:178`) returns `max(n1·q, n2·q)` (bevelled past SVG's default
miter limit 4). A butt cap stops the stroke on the line through its end: `butt_gauge`
(`join.rs:161`) returns `max((p - e)·t + h, |(p - e)×t|)`, whose residual is the signed
distance to the end face — independent of `h`, which the Jacobian row records.

## The stroke solve (`refine.rs`)

Unknowns: every centreline control point, circular arcs' radii, circle primitives' centre
and radius, and the half-width. Energy
`E = Σ_p ((d_p - h)/σ_p)² + Σ_j ((θ_j - θ_j⁰)/σ_a)²` with a weak anchor (`ANCHOR` = 2 px,
`refine.rs:88`; the half-width's `ANCHOR_HALF` = 5% of itself, `refine.rs:96`).

- **Measurement** (`rows`, `refine.rs:709`). Each boundary point's nearest piece among the
  centrelines flattened to chords within `FLAT_TOL` = 0.02 px (`refine.rs:547`;
  `flat_pieces`, `refine.rs:571`: de Casteljau halving of cubics, equal-angle arcs), found
  by a bounding-box hierarchy built along the pieces' order (`PieceTree`, `bvh.rs:97`),
  names its segment and a parameter; the exact distance is then polished on that segment
  (`dist.rs`: closed form for lines and circular arcs, Newton on the foot condition for
  cubics and elliptical arcs). Points are measured in parallel and summed in order.
- **Jacobian** (`analytic_row`, `refine/normal.rs:133`). By the envelope theorem
  `∂d/∂θ = -n·∂C(t*)/∂θ`: `-B_i(t*)·n` for a cubic's control points, `-(1 - u, u)·n` for a
  line's, and for arcs the chain rule through SVG's endpoint-to-centre conversion
  (`circular_arc_grad`, `dist.rs:306`; `ellipse_arc_grad`, `dist.rs:402`, through the map
  that takes the ellipse to a unit circle). Gauge rows (a miter vertex, a butt end) keep
  central differences. Rows are fixed arrays of at most `ROW_CAP` = 20 entries
  (`refine/normal.rs:88`).
- **Normal equations** (`normal_equations`, `refine/normal.rs:254`). Each unknown couples only to
  its own segment's neighbours and to `h`, so `JᵀJ` is kept as the envelope of its lower
  triangle (`profile`, `refine/normal.rs:50`) and solved by an envelope Cholesky
  (`Skyline::cholesky_solve`, `skyline.rs:130`), which gives the dense factorisation's
  result bit for bit.
- **Steps.** A step that lowers `E` and leaves every cubic free of cusps is taken (the cusp
  test runs before the measurement); `μ` shrinks by 3 after a step and grows by 4 after a
  refusal. Stops after `MAX_ITERS` = 30 (`refine.rs:74`), when `E` falls by under 1e-5
  relatively, or after `MAX_RETRIES` = 8 refusals (`refine.rs:83`); more than `MAX_VARS` =
  400 unknowns (`refine.rs:101`) skips the solve.
- **Splits** (`solve_adaptive`, `refine.rs:895`). The segment carrying the most residual
  is split (a line becomes a cubic on its chord, a cubic is cut by de Casteljau, an arc at
  its worst point). Before the split is solved, one Gauss-Newton step prices it
  (`predicted_gain`, `refine.rs:833`, the score test): a split predicted to gain under
  `SCREEN_KAPPA` = 0.25 of its price is refused unsolved (`refine.rs:872`). A solved trial
  runs `TRIAL_ITERS` = 8 iterations (`refine.rs:80`) and is kept when the description
  length falls; the kept strokes are solved once more at the end.

## The decision (`ribbons.rs`)

For each opaque, flat-filled, not transparent face of 16 px² to half the image:

`ΔL = (χ²_stroke - χ²_outline)/2 + λ·(k_stroke - k_vanish)`,

`χ²_outline` the face's fitted edges against their own measured points (`outline_chi2`,
`ribbons.rs:347`), `k_vanish` the parameters only this face writes in the stacked document
(`vanishing_params`, `ribbons.rs:385`). The face becomes strokes when `ΔL < 0`,
`k_stroke < k_vanish`, and `χ²_stroke - χ²_outline <= FIT_MARGIN_SD·sqrt(2N)` on its `N`
boundary points, `FIT_MARGIN_SD` = 2 (`ribbons.rs:291`; `decide`, `ribbons.rs:257`): the
strokes must fit as well as the outline within two standard deviations of a chi-squared
statistic. Each centreline is its own element, as stroke icon sets draw them: a `<path>`, or
a stroked `<circle>`/`<ellipse>`/`<rect>` for a centreline that is a whole primitive. One
element carries `fill="none" stroke=… stroke-width=… stroke-linecap=… stroke-linejoin=…`
itself; several go in one `<g>` that carries them (`element`, `ribbons.rs:500`). One path
per centreline rather than one compound path costs no numbers (a subpath's move-to is a
path's) and keeps a reader that walks a path's points as one line from reading the pen
lifts between strokes as turns.

**Faces that leave with it** (`absorbed_by`, `crates/inkvec-cli/src/ribbons.rs:430`). On an
opaque page the inside of a stroked ring is a face of its own, painted the page's colour on
top of the ring's filled disc. Once the ring is a stroke, that face paints what already
shows: the face the ring sits in is painted across all of its area underneath. So a child
the candidate alone encloses, painted exactly the colour of the face the candidate sits in
(flat, opaque, the same `#rrggbb`), leaves with it, and its outline counts in `k_vanish`.
The emitter drops it only where its nearest painted ancestor after the drop still has its
colour and nothing is punched out of it (`crates/inkvec-cli/src/emit.rs:188`). Lucide
`closed-caption` on white: a black rounded rectangle under a white one (12 parameters)
becomes one stroked rectangle (7), as the artist drew it.

## Constants

| Constant | Value | Where | Why |
|---|---|---|---|
| `MIN_WIDTH` | 2 px | `ribbon.rs:94` | below it a stroke has no interior pixel of its own |
| `MIN_PAIRED_SHARE` | 0.35 | `ribbon.rs:102` | lucide pairs at 81.5%; 0.5 refused `navigation-2-off` |
| `COS_PAIR` | 0.906 (25°) | `boundary.rs:50` | refuses caps beyond their first eighth and concave corners |
| `SIGMA_FRAME` | 0.05 px | `boundary.rs:74` | the canvas frame is known exactly |
| `SIGMA_JUNCTION`, `SIGMA_CAP` | 0.25, 0.15 px | `graph.rs:68`, `graph.rs:72` | rebuilt points, extrapolated |
| `COS_CONTINUE` | 0.766 (40°) | `graph.rs:76` | sleeves continue through a junction |
| `MITER_TRIGGER` | 0.25 px | `ribbon.rs:415` | round fits of round art sit at 0.05-0.3 px |
| `BUTT_TRIGGER` | 0.1 w | `ribbon.rs:411` | a butt end fitted round misses by up to 0.2 w |
| `MAX_UNCOVERED` | 0.002 | `ribbon.rs:425` | a blob read as a ring of stroke paints a hole |
| `MAX_PRE_RMS` | 0.15 w | `ribbon.rs:460` | a blob read as a stroke starts a quarter of a width out |
| `CHAIN_SAMPLES_PER_WIDTH` | 8 | `ribbon.rs:624` | 4 cost 5% more parameters at 512 px |
| `COARSE_STRIDE`, `COARSE_DECLINE` | 8, 1.25 | `ribbon.rs:627`, `ribbon.rs:647` | calibrated on every reading (see the code) |
| `MAX_ITERS`, `TRIAL_ITERS`, `MAX_RETRIES` | 30, 8, 8 | `refine.rs:83`, `refine.rs:80`, `refine.rs:83` | |
| `ANCHOR`, `ANCHOR_HALF` | 2 px, 0.05 | `refine.rs:88`, `refine.rs:96` | holds unseen variables; keeps a blob from shrinking to a ring |
| `MAX_VARS` | 400 | `refine.rs:101` | |
| `FLAT_TOL` | 0.02 px | `refine.rs:547` | chords stand in for segments only in the search |
| `MAX_SPLITS`, `SCREEN_KAPPA` | 12, 0.25 | `refine.rs:858`, `refine.rs:872` | see `SCREEN_KAPPA`'s measurement |
| `ROW_CAP` | 20 | `refine/normal.rs:88` | half-width plus two segments' variables |
| `MITER_MIN_SEG`, `MITER_MAX_TURN_COS` | 0.5 px, -0.866 | `refine.rs:1067`, `refine.rs:1070` | renderers draw short or hairpin miters otherwise |
| `MITER_LIMIT` | 4 | `join.rs:39` | SVG's default `stroke-miterlimit` |
| `MAX_MERGE_TRIALS` | 12 | `refine/merge.rs:25` | merges solved per face; one kept unsolved is free |
| `MERGE_SLACK` | 1 | `refine/merge.rs:33` | the local estimate may sit at twice the price saved; the solve buys back part |
| `HALF_TURN_BAND_DEGREES` | 15° | `refine/merge.rs:44` | at 165° a radius rounded to 0.005 px moves the arc 0.04 px |
| `MAX_SWEEP_DEGREES` | 350° | `refine/merge.rs:48` | closer to a full turn the centreline is a circle |
| `FIT_MARGIN_SD` | 2 | `ribbons.rs:291` | the gear and `wxt` (see the code) |

## Measured (2026-10-04, switch on, judged as the gate judges)

Against v0.2.5 on the gate's screen set (246 icons), Quality: dE00 0.1283 → 0.1163 at
128 px, 0.0482 → 0.0449 at 512 px, 0.0490 → 0.0461 at 512 px with margin; parameter ratio
1.511 → 1.032, 1.783 → 1.248, 1.986 → 1.570. The gate's `turning` signal reads +1.3% and
+2.1% at 128 and 512 px: it reads every number in a path's data as a point, so an arc's
radii and flags count as points; with arcs read as arcs and every subpath its own line the
same quantity falls 4.9% and 3.5%. held_a at 128 px: dE00 0.1310 → 0.1186, parameter ratio
1.471 → 1.022. Fast mode is unchanged (the stage does not run there). Whole trace at 512 px
on lucide and openmoji: 1.5-1.7× v0.2.5's wall time, the stage's worst icon 2.4 s.

## Measured (2026-10-10, on by default, judged as the gate judges)

Against the committed baseline (the same build with the stage off), Quality, 246 icons,
paired bootstrap: the stage as it was (2026-10-04), then with the merges, then with the
faces that leave with a stroke.

| condition | axis | stage alone | + merges | + absorbed faces |
|---|---|---|---|---|
| quality-128ss | dE00 | −7.90 % | −8.20 % | −8.18 % |
| | parameter ratio | 1.495 → 0.994 | 0.957 | 0.954 |
| | turning | −5.75 % | −6.09 % | −6.11 % |
| quality-512ss | dE00 | −7.88 % | −8.54 % | −8.57 % |
| | parameter ratio | 1.728 → 1.217 | 1.170 | 1.168 |
| | turning | −5.72 % | −6.00 % | −6.02 % |
| quality-512ssop | dE00 | −6.57 % | −7.03 % | −11.35 % |
| | parameter ratio | 1.967 → 1.566 | 1.531 | 1.414 |
| | turning | −4.82 % | −4.87 % | −5.58 % |

Every row is a demonstrable gain (the one-sided 95 % bound below zero). Lucide at 512 px
went from 1.46 to 1.16 times the artist's parameters with the merges; `closed-caption` and
`iteration-ccw` now carry exactly the artist's structure (one 219° and one 270° arc where
the trace had three arcs and two short lines each). Fast mode is unchanged (the stage does
not run there). Whole trace at 512 px, minimum of two runs per icon, one process at a
time: 1.16× the time with the stage off over the screen set (lucide 1.29×, material-icons
1.31×, openmoji 1.24×, simple-icons 1.23×, twemoji 1.09×, noto-emoji 1.05×), the worst
icon 0.8 s more.

## Literature

- Pairing: *Inspired by* Hilaire, Tombre (2006), doi:10.1109/TPAMI.2006.127; Tombre et
  al. (2000), doi:10.1007/3-540-40953-X_1.
- Topology: *Method from* Zhang, Liu, Li, Wu, Wen (2022), doi:10.1111/cgf.14485; the chordal
  axis *method from* Prasad (2005), doi:10.1007/978-3-540-31965-8_25, and Prasad (2007),
  doi:10.1016/j.imavis.2006.06.025; *inspired by* Favreau, Lafarge, Bousseau (2016),
  doi:10.1145/2897824.2925946; see also Berio, Leymarie, Asente, Echevarria (2022),
  doi:10.1145/3505246.
- Triangulation: Chew (1989), doi:10.1007/BF01553881; Guibas, Stolfi (1985),
  doi:10.1145/282918.282923; Sloan (1993), doi:10.1016/0045-7949(93)90239-A; Shewchuk
  (1997), doi:10.1007/PL00009321.
- Stroke model: the SVG 1.1 specification, 11.4 (joins, caps, miter limit).
- Solve: Levenberg (1944), doi:10.1090/qam/10666; Marquardt (1963), doi:10.1137/0111030;
  envelope theorem as in Milgrom, Segal (2002), doi:10.1111/1468-0262.00296; envelope
  Cholesky, Jennings (1966), doi:10.1093/comjnl/9.3.281; see also Triggs et al. (2000),
  doi:10.1007/3-540-44480-7_21; nearest-piece search inspired by Lauterbach et al. (2009),
  doi:10.1111/j.1467-8659.2009.01377.x, and Fukunaga, Narendra (1975),
  doi:10.1109/T-C.1975.224297; flattening inspired by Lane, Riesenfeld (1980),
  doi:10.1109/TPAMI.1980.4766968; split screen by the score test of Rao (1948),
  doi:10.1017/S0305004100023987, and Moré (1978), doi:10.1007/BFb0067700; splits inspired
  by Schneider (1990), Graphics Gems.
- Merges: *Inspired by* Pavlidis, Horowitz (1974), Segmentation of plane curves, IEEE
  Transactions on Computers C-23(8), doi:10.1109/T-C.1974.224041 (split-and-merge); the
  merged cubic's arms by Schneider (1990), above; why a true merge cannot cost fidelity:
  `formal/InkvecTheory/InkvecTheory/Naturality.lean` (`natural_model_closer`).
- Decision: Rissanen (1978), doi:10.1016/0005-1098(78)90005-5; the fit margin is not from
  the literature (see also Wilks 1938, doi:10.1214/aoms/1177732360).
