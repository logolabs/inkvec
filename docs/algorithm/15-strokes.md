# Stage 15 — Stroke detection (`--detect-strokes`, off by default)

> Finds the faces of the colour trace that were drawn as strokes — a centreline and one
> width — and writes each as a stroked path where that is the shorter description of its
> measured boundary at a fit as good as its outline's.

**Source:** `crates/inkvec-trace/src/ribbon.rs` (the per-face driver, `fit_face`) and
`crates/inkvec-trace/src/ribbon/`: `boundary.rs` (rings, inward normals, pairing),
`graph.rs` (topology), `chordal.rs` (constrained Delaunay chordal axis), `refine.rs` (the
stroke solve), `dist.rs` (exact distances and their gradients), `skyline.rs` (the envelope
Cholesky), `bvh.rs` (nearest-piece search), `grid.rs` (bucket grid and ray walks),
`join.rs` (joins and caps as residual models), `score.rs` (the score). The decision and the
SVG elements: `crates/inkvec-cli/src/ribbons.rs`.
**Entry points:** `ribbon::fit_face()` (`crates/inkvec-trace/src/ribbon.rs:317`), called per
candidate face by `ribbons::choose()` (`crates/inkvec-cli/src/ribbons.rs:168`), which
`ribbons::stage()` (`crates/inkvec-cli/src/ribbons.rs:77`) runs when
`--detect-strokes` / `Options::detect_strokes` is set (`crates/inkvec-cli/src/args.rs:181`,
`crates/inkvec/src/options.rs:110`) or, for the benchmarks, `INKVEC_RIBBONS=1`
(`crates/inkvec-cli/src/ribbons.rs:67`).
**Pipeline position:** Quality mode, colour output only (not `--mode fast`, not
`--monochrome`): after every boundary is fitted, repaired and mirrored and every face's
fill and transparency are settled, before the document is written
(`crates/inkvec-cli/src/pipeline.rs:426`). The emitter drops the faces written as strokes
from the fill tree and paints the strokes on top (`crates/inkvec-cli/src/emit.rs:180`,
`write_ribbons`, `crates/inkvec-cli/src/emit.rs:1299`).

## What problem this solves

Line art — lucide entirely, openmoji's black outlines, much of every icon set — is drawn as
centrelines with one stroke width. Traced as filled outlines, every stroke costs both of its
sides plus its caps: the artist's own lucide geometry written as outlines needs 3.87× the
artist's parameters, and the curve fitter already sits within 6% of that floor (r2-compact
research, 2026-10-02). No fitting change reaches it; only a change of representation does.
This stage is that change, face by face, and it decides each face by description length —
the same currency the fitter uses — with a fidelity bound on top.

## The passes, for one face (`fit_face`, `ribbon.rs:317`)

1. **Boundary and width by pairing** (`boundary.rs`). The face's rings after the boundary
   solve, with inward normals voted against the face mask. Every boundary point walks
   along its inward normal to the far side (`Boundary::across`, `boundary.rs:247`, a ray
   walk in `grid.rs`); an anti-parallel hit (within `COS_PAIR` = cos 25°,
   `boundary.rs:50`) is a width sample. The median's core gives the width `w0` and the
   share of the outline that is sleeve (`stroke_width`, `boundary.rs:279`). Faces with a
   share under `MIN_PAIRED_SHARE` = 0.35 (`ribbon.rs:95`) or narrower than `MIN_WIDTH` =
   2 px (`ribbon.rs:87`) are declined.
2. **Medial graph and centre samples** (`graph::medial`, `graph.rs:309`), read once per
   face: Zhang-Suen thinning of the face mask as a graph (`skeleton`, `graph.rs:325`), and
   at every node a cross-section from the nearest boundary point across to the other side
   (`cross_section`, `graph.rs:342`); a sample is reliable when the far side is
   anti-parallel and the width agrees with `w0`.
3. **Topology readings** (`hypothesis`, `ribbon.rs:467`; `TopoOptions`). Under round joins
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
5. **Centreline fit** (`fit_chain`, `ribbon.rs:688`). Chains are thinned to
   `CHAIN_SAMPLES_PER_WIDTH` = 8 samples per stroke width (`ribbon.rs:602`; `decimate`,
   `ribbon.rs:643`: runs of measured samples between anchors replaced by their middle
   sample with sigma `sqrt(Σσ²)/m`), then fitted by the outline fitter's MDL dynamic
   program (`inkvec_fit::multimodel`) or a whole primitive when that is cheaper, in
   parallel. A coarse fit at an eighth of the samples first declines readings whose
   centrelines already cost `COARSE_DECLINE` = 1.25× the budget (`ribbon.rs:625`).
6. **Stroke solve** (`refine::solve_adaptive`, `refine.rs:894`). Levenberg-Marquardt over
   every control point and the half-width against the boundary residual
   `r_p = (d(p, C) - h)/σ_p` (see the solve below), then up to `MAX_SPLITS` = 12
   (`refine.rs:857`) splits of the segment carrying the most residual, each kept only when
   the description length falls.
7. **Score and checks** (`score::score`, `score.rs:154`; `uncovered`, `ribbon.rs:420`;
   `refine::drawable`, `refine.rs:1451`): the chi-squared of the strokes' painted outline
   against every boundary point, the face's interior pixels checked as painted
   (`MAX_UNCOVERED` = 0.2% of its pixels, `ribbon.rs:413`), and no cusp (no
   micro-segment or hairpin under miter joins).

Passes 3-7 run with round joins and round caps first; when the round fit leaves a point
more than `MITER_TRIGGER` = 0.25 px out (`ribbon.rs:403`) the face is also read under miter
joins, and when the better reading has free ends and leaves a point more than
`BUTT_TRIGGER` = 0.1 of the width out (`ribbon.rs:399`), under butt caps. The cheaper by
`χ²/2 + λ·k` wins (`cheaper`, `ribbon.rs:378`).

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
`refine.rs:87`; the half-width's `ANCHOR_HALF` = 5% of itself, `refine.rs:95`).

- **Measurement** (`rows`, `refine.rs:708`). Each boundary point's nearest piece among the
  centrelines flattened to chords within `FLAT_TOL` = 0.02 px (`refine.rs:546`;
  `flat_pieces`, `refine.rs:570`: de Casteljau halving of cubics, equal-angle arcs), found
  by a bounding-box hierarchy built along the pieces' order (`PieceTree`, `bvh.rs:97`),
  names its segment and a parameter; the exact distance is then polished on that segment
  (`dist.rs`: closed form for lines and circular arcs, Newton on the foot condition for
  cubics and elliptical arcs). Points are measured in parallel and summed in order.
- **Jacobian** (`analytic_row`, `refine.rs:1153`). By the envelope theorem
  `∂d/∂θ = -n·∂C(t*)/∂θ`: `-B_i(t*)·n` for a cubic's control points, `-(1 - u, u)·n` for a
  line's, and for arcs the chain rule through SVG's endpoint-to-centre conversion
  (`circular_arc_grad`, `dist.rs:306`; `ellipse_arc_grad`, `dist.rs:402`, through the map
  that takes the ellipse to a unit circle). Gauge rows (a miter vertex, a butt end) keep
  central differences. Rows are fixed arrays of at most `ROW_CAP` = 20 entries
  (`refine.rs:1108`).
- **Normal equations** (`normal_equations`, `refine.rs:1274`). Each unknown couples only to
  its own segment's neighbours and to `h`, so `JᵀJ` is kept as the envelope of its lower
  triangle (`profile`, `refine.rs:1070`) and solved by an envelope Cholesky
  (`Skyline::cholesky_solve`, `skyline.rs:130`), which gives the dense factorisation's
  result bit for bit.
- **Steps.** A step that lowers `E` and leaves every cubic free of cusps is taken (the cusp
  test runs before the measurement); `μ` shrinks by 3 after a step and grows by 4 after a
  refusal. Stops after `MAX_ITERS` = 30 (`refine.rs:73`), when `E` falls by under 1e-5
  relatively, or after `MAX_RETRIES` = 8 refusals (`refine.rs:82`); more than `MAX_VARS` =
  400 unknowns (`refine.rs:100`) skips the solve.
- **Splits** (`solve_adaptive`, `refine.rs:894`). The segment carrying the most residual
  is split (a line becomes a cubic on its chord, a cubic is cut by de Casteljau, an arc at
  its worst point). Before the split is solved, one Gauss-Newton step prices it
  (`predicted_gain`, `refine.rs:832`, the score test): a split predicted to gain under
  `SCREEN_KAPPA` = 0.25 of its price is refused unsolved (`refine.rs:871`). A solved trial
  runs `TRIAL_ITERS` = 8 iterations (`refine.rs:79`) and is kept when the description
  length falls; the kept strokes are solved once more at the end.

## The decision (`ribbons.rs`)

For each opaque, flat-filled, not transparent face of 16 px² to half the image:

`ΔL = (χ²_stroke - χ²_outline)/2 + λ·(k_stroke - k_vanish)`,

`χ²_outline` the face's fitted edges against their own measured points (`outline_chi2`,
`ribbons.rs:334`), `k_vanish` the parameters only this face writes in the stacked document
(`vanishing_params`, `ribbons.rs:372`). The face becomes strokes when `ΔL < 0`,
`k_stroke < k_vanish`, and `χ²_stroke - χ²_outline <= FIT_MARGIN_SD·sqrt(2N)` on its `N`
boundary points, `FIT_MARGIN_SD` = 2 (`ribbons.rs:278`; `decide`, `ribbons.rs:244`): the
strokes must fit as well as the outline within two standard deviations of a chi-squared
statistic. The element is `<path fill="none" stroke=… stroke-width=… stroke-linecap=…
stroke-linejoin=…>`, plus stroked `<circle>`/`<ellipse>`/`<rect>` for centrelines that are
whole primitives (`element`, `ribbons.rs:412`).

## Constants

| Constant | Value | Where | Why |
|---|---|---|---|
| `MIN_WIDTH` | 2 px | `ribbon.rs:87` | below it a stroke has no interior pixel of its own |
| `MIN_PAIRED_SHARE` | 0.35 | `ribbon.rs:95` | lucide pairs at 81.5%; 0.5 refused `navigation-2-off` |
| `COS_PAIR` | 0.906 (25°) | `boundary.rs:50` | refuses caps beyond their first eighth and concave corners |
| `SIGMA_FRAME` | 0.05 px | `boundary.rs:74` | the canvas frame is known exactly |
| `SIGMA_JUNCTION`, `SIGMA_CAP` | 0.25, 0.15 px | `graph.rs:68`, `graph.rs:72` | rebuilt points, extrapolated |
| `COS_CONTINUE` | 0.766 (40°) | `graph.rs:76` | sleeves continue through a junction |
| `MITER_TRIGGER` | 0.25 px | `ribbon.rs:403` | round fits of round art sit at 0.05-0.3 px |
| `BUTT_TRIGGER` | 0.1 w | `ribbon.rs:399` | a butt end fitted round misses by up to 0.2 w |
| `MAX_UNCOVERED` | 0.002 | `ribbon.rs:413` | a blob read as a ring of stroke paints a hole |
| `MAX_PRE_RMS` | 0.15 w | `ribbon.rs:448` | a blob read as a stroke starts a quarter of a width out |
| `CHAIN_SAMPLES_PER_WIDTH` | 8 | `ribbon.rs:602` | 4 cost 5% more parameters at 512 px |
| `COARSE_STRIDE`, `COARSE_DECLINE` | 8, 1.25 | `ribbon.rs:605`, `ribbon.rs:625` | calibrated on every reading (see the code) |
| `MAX_ITERS`, `TRIAL_ITERS`, `MAX_RETRIES` | 30, 8, 8 | `refine.rs:73`, `refine.rs:79`, `refine.rs:82` | |
| `ANCHOR`, `ANCHOR_HALF` | 2 px, 0.05 | `refine.rs:87`, `refine.rs:95` | holds unseen variables; keeps a blob from shrinking to a ring |
| `MAX_VARS` | 400 | `refine.rs:100` | |
| `FLAT_TOL` | 0.02 px | `refine.rs:546` | chords stand in for segments only in the search |
| `MAX_SPLITS`, `SCREEN_KAPPA` | 12, 0.25 | `refine.rs:857`, `refine.rs:871` | see `SCREEN_KAPPA`'s measurement |
| `ROW_CAP` | 20 | `refine.rs:1108` | half-width plus two segments' variables |
| `MITER_MIN_SEG`, `MITER_MAX_TURN_COS` | 0.5 px, -0.866 | `refine.rs:1362`, `refine.rs:1365` | renderers draw short or hairpin miters otherwise |
| `MITER_LIMIT` | 4 | `join.rs:39` | SVG's default `stroke-miterlimit` |
| `FIT_MARGIN_SD` | 2 | `ribbons.rs:278` | the gear and `wxt` (see the code) |

## Measured (2026-10-04, switch on, judged as the gate judges)

Against v0.2.5 at 128 px: the screen set's macro dE00 0.1283 → 0.1163 and parameter ratio
1.503 → 0.958; held_a 0.1310 → 0.1186 and 1.471 → 1.022. On lucide and openmoji at 512 px the
stage's per-icon cost is described in the stage's commit messages and the wave report; the
gate verdicts are in that report.

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
- Decision: Rissanen (1978), doi:10.1016/0005-1098(78)90005-5; the fit margin is not from
  the literature (see also Wilks 1938, doi:10.1214/aoms/1177732360).
