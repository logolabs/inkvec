# Constants and thresholds

Every numeric constant and threshold in the tracing pipeline, in one place, with where it
comes from. This is the companion table [`README.md`](README.md) and the `.html` pages
refer to.

Each row gives the constant's current location as `file.rs:line` inside `crates/`, and a
one-line statement of what it controls. The **basis** column classifies how the value was
arrived at, using four labels:

- **derived** — an exact consequence of another fact (a unit conversion, a parameter count,
  a geometric identity). Changing the fact changes the constant.
- **measured** — chosen or confirmed by running the corpus and recording a number (a sweep,
  an A/B test, a regression that killed an earlier value).
- **motivated** — the existence and rough shape of the constant is explained in the source
  (why a floor or a cap is needed at all), but the specific number is not swept or derived.
- **none** — no derivation is stated anywhere in the source. This is the list of constants
  most likely to have been fitted to one example rather than measured across the corpus;
  a reader tuning the pipeline should treat these as the first candidates to re-derive.

Full context, measured numbers and quoted doc comments for every row below live in that
stage's own reference document — this table only summarizes. Paths are relative to
`crates/`.

## 01 — Intake ([01-intake.md](01-intake.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `REF_EXTENT` | 128.0 px | `inkvec-cli/src/lib.rs:90-99` | intake size every pixel-denominated constant was tuned at | measured |
| `INTAKE_SCALE_FLOOR` | 1.5 | `inkvec-cli/src/lib.rs:150-151` | below this, `--intake-scale` leaves the input alone | measured |
| `INTAKE_SCALE_CAP` | 8.0 | `inkvec-cli/src/lib.rs:153-154` | ceiling on how much `--intake-scale` discards | motivated |
| `--max-dim` default | 2048 px | `inkvec-cli/src/args.rs:113-116` | ceiling on traced (not emitted) size | motivated |
| `--time-budget` split | 0.6 merge / 0.25 boundary-solve | `inkvec-cli/src/args.rs:117-120` | advisory wall-clock split between the two most expensive stages | none |
| `MAX_FACTOR` (`pixel_grid`) | 32 | `inkvec-cli/src/alpha.rs:389` | largest replication factor the unblock pre-pass tries | motivated |
| `smallest` (`pixel_grid`) | 64 px | `inkvec-cli/src/alpha.rs:393` | floor below which unblocking is not attempted | motivated |
| block-constant tolerance | 1/512 per channel | `inkvec-cli/src/alpha.rs:343-345, 505` | how exactly a block must match to count as replication | motivated |
| `SHARP` (`change_gcd`) | 1/256 per channel | `inkvec-cli/src/alpha.rs:445` | neighbour difference that counts as a change position for the unblock gcd filter | derived (twice the block tolerance, `alpha.rs:363-371`) |
| `INTAKE_PARALLEL_MIN` | 65,536 px (256 x 256) | `inkvec-cli/src/alpha.rs:1011` | below this the transparency scan and the flatten run on the calling thread | motivated |
| `FLATTEN_CHUNK` | 16,384 px | `inkvec-cli/src/alpha.rs:1013` | pixels per parallel job of the flatten, smallest job of the transparency scan | none |
| `PARALLEL_MIN_PIXELS` (load) | 65,536 px (256 x 256) | `inkvec-trace/src/load.rs:103` | below this the byte-to-float conversion runs on the calling thread | motivated |
| `CONVERT_CHUNK_PIXELS` | 65,536 px | `inkvec-trace/src/load.rs:107` | pixels per parallel job of the byte-to-float conversion (64 jobs at 2048 x 2048) | motivated |
| `UNIT` | `k / 255`, k = 0..=255 | `inkvec-trace/src/load.rs:115` | the float each 8-bit sample becomes | derived (the old division's quotients, checked bit for bit) |
| `COMPOSITE_PARALLEL_MIN` | 65,536 px (256 x 256) | `inkvec-trace/src/coverage.rs:234` | below this the composite over white runs on the calling thread | motivated |
| `MARGIN` (`choose_matte`) | 10.0 (CIEDE2000) | `inkvec-cli/src/alpha.rs` | closeness to a matte candidate to count as "swallowed" | none |
| `SWALLOWED` | 0.33 | `inkvec-cli/src/alpha.rs:271-306` | share of drawn-and-translucent mass a matte may swallow | motivated |
| `DRAWN` | 0.5 | `inkvec-cli/src/alpha.rs:297-307` | alpha above which a pixel counts as silhouette, not glow | motivated |
| `DRAWN_FLOOR` | 0.05 | `inkvec-cli/src/alpha.rs` | alpha below which a pixel is ignored entirely | none |
| `SOFT_SHARE` | 0.05 | `inkvec-cli/src/alpha.rs` | glow share that keeps white without the candidate ladder | none |
| `FLAT_ALPHA` | 0.02 | `inkvec-cli/src/alpha.rs` | neighbour-alpha spread counted as "flat" translucency | none |
| CI-gate literal | 0.33 | `inkvec-cli/src/alpha.rs:457` | warns when white would swallow more than this, without `--cutout` | duplicates `SWALLOWED` as a separate literal — see 01-intake.md Open questions |
| `DEGRADED_RESIDUAL` / `--sr-threshold` | 0.5 | `inkvec-sr/src/detect.rs:1-36` | interior-residual threshold above which `--sr auto` cleans | measured (30 icons, 5 conditions) |
| `--sr-scale` default | 2 | `inkvec-sr/src/detect.rs` | output scale of the SR pre-pass | none |
| interior-residual normalisation | `sum / 9n` | `inkvec-sr/src/detect.rs:97-104` | matches the reference Python implementation | derived (deliberate match, not a bug) |

## 02 — Coverage ([02-coverage.md](02-coverage.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `DEFAULT_SIGMA_MODEL` | 0.05 px | `inkvec-trace/src/coverage.rs:39,59-60` | floor added in quadrature to noise-derived positional sigma | measured (analytic circles, `tests/subpixel.rs:107-167`) |
| `MAX_SIGMA` | 4.0 px | `inkvec-trace/src/coverage.rs:106` | ceiling on positional uncertainty when the gradient vanishes | none |
| plateau fraction | 0.001 | `inkvec-trace/src/coverage.rs:200-202` | how much of the luminance extreme defines `fg`/`bg` | motivated |
| extreme-band tolerance | 0.15 | `inkvec-trace/src/coverage.rs` | which pixels near each extreme are averaged into `fg`/`bg` | none |
| `MAD_TO_SIGMA` | 0.6745 | `inkvec-trace/src/coverage.rs:174-177` | MAD-to-Gaussian-sigma conversion | derived (standard statistical constant) |
| Laplacian noise gain | sqrt(20), computed from `LAPLACIAN_KERNEL` | `inkvec-trace/src/coverage.rs:172,229` | corrects the 4-neighbour Laplacian's noise gain | derived; a hardcoded `sqrt(6)` was a bug, fixed 2026-09-08 |
| noise floor | 0.5/255 | `inkvec-trace/src/coverage.rs:1007` | minimum `estimate_noise` can return | derived (palette divides by it, `coverage.rs:1004-1011`) |
| `confidence_penalty` floor | `saturation.max(0.05)` | `inkvec-trace/src/coverage.rs` | prevents unbounded penalty near-zero saturation | none |
| `confidence_penalty` cap | 8.0 | `inkvec-trace/src/coverage.rs` | ceiling on sigma inflation from low saturation | none |
| `EDGE_FLOOR` | 2/255 | `inkvec-trace/src/coverage.rs:339` | minimum first difference counted as a real edge | motivated |
| `MAX_W` | 64.0 | `inkvec-trace/src/coverage.rs:341-342` | clamp on a single edge-width observation | motivated |
| `MIN_W` | 0.25 | `inkvec-trace/src/coverage.rs` | lower clamp on a single edge-width observation | none |
| minimum edge observations | 16 | `inkvec-trace/src/coverage.rs:375-377` | below this, `intake_scale` returns 1.0 | none |
| `OVERSAMPLE_TOL` | 3.0 (8-bit levels) | `inkvec-trace/src/coverage.rs:436-440` | round-trip error threshold for `oversample_factor` | measured; only safe downstream of `intake_scale`, never as a gate alone |
| min. size for `oversample_factor` | 16x16, `sw`/`sh >= 8` | `inkvec-trace/src/coverage.rs` | avoids measuring on too little data | none |

## 03 — Palette ([03-palette.md](03-palette.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `SAME_INK_DE00` | 1.5 (CIEDE2000) | `inkvec-trace/src/color.rs:285-302` | perceptual floor: colours this close are one ink | measured (swept 1.0 -> 0.4145, 1.5 -> 0.4124 on the screen set) |
| `SOFT_SAME_INK_DE00` | 5.0 | `inkvec-trace/src/color.rs:427-439` | same-ink floor on soft/oversampled intake | measured (incorpo mark, 4x upscale) |
| `SOFT_NOISE_SIGMAS` | 3.0 | `inkvec-trace/src/color.rs:417-425` | noise-merge threshold, gated on soft-intake evidence only | measured (costs 10.9% objective if run unconditionally) |
| `NOISE_SIGMAS` | 0.0 | `inkvec-trace/src/color.rs:617-634` | clean-intake default noise-merge threshold, deliberately off | measured (costs 0.4328 -> 0.4451 on the screen set if always on) |
| `SOFT_INTAKE_EDGE` | 1.75 px | `inkvec-trace/src/color.rs:406-415` | gates whether soft-intake handling runs at all | measured (980-raster corpus edge-width survey) |
| `DEFAULT_MERGE_DISTANCE` | 0.035 (OKLab) | `inkvec-trace/src/color.rs:100-125` | palette merge radius | measured (swept 0.055/0.040/0.035/0.030 on the full set; 0.035 best) |
| `MIN_INK_WEIGHT` | 0.004 | `inkvec-trace/src/color.rs:127-133` | minimum accumulated weight to count as a real ink | motivated |
| `BLEND_IMMUNE_WEIGHT` | 0.03 | `inkvec-trace/src/color.rs:135-147` | image share above which a colour is never dismissed as anti-aliasing | motivated; **not read anywhere in current `extract_palette_mdl`** — see 03-palette.md Open questions |
| `BLEND_INTERIOR_FRACTION` | 0.25 | `inkvec-trace/src/color.rs:149-164` | interior-fraction threshold, anti-aliasing vs. ink | measured (swept against conflicting optima on two corpora) |
| `BLEND_STRADDLE_FRACTION` | 0.5 | `inkvec-trace/src/color.rs:166-177` | straddle-fraction threshold | motivated |
| `STRADDLE_STEP` | 0.12 | `inkvec-trace/src/color.rs:178-181` | quantisation-noise floor for straddle detection | motivated |
| `JND_FLOOR` | 0.012 (OKLab) | `inkvec-trace/src/color.rs:611-615` | below this, two colours are never treated as separate inks | none |
| `PARAMS_PER_INK` | 3.0 | `inkvec-trace/src/color.rs:279` | one parameter per OKLab channel | derived |
| `STAT_PIXELS` | 65536 | `inkvec-trace/src/color.rs:692-705` | cap on per-candidate statistical pass cost | derived (matches the 128px tuning point) |
| `INKVEC_BLEND_TMIN` (*removed*) (env) | 0.04 | `inkvec-trace/src/color.rs:535-538` | interior-mixture band on the A-B colour axis | none |

## 04 — Regions ([04-regions.md](04-regions.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `min_region` shipped default | 2 px | `inkvec-cli/src/args.rs:64`, `inkvec-trace/src/lib.rs:210` | smallest component kept before despeckle/carve | measured (a 9px colour-mode floor was tried and measured worse) |
| `SADDLE_SIGMAS` | 3.0 | `inkvec-trace/src/regions.rs:13` | sigma a corner's coverage must clear 0.5 by before a saddle resolves | motivated (a standard "three sigma" bar, not swept) |
| absorption thinness gate | interior < area/5 | `inkvec-trace/src/regions.rs` | candidate slivers for blend absorption | motivated |
| absorption dominant-neighbour gate | top neighbours >= 4/5 of foreign contacts | `inkvec-trace/src/regions.rs` | which components are candidate slivers | motivated |
| absorption pass threshold | >= 4/5 pixels within tolerance | `inkvec-trace/src/regions.rs` | whether a qualifying component is absorbed | motivated |
| `tol` (both blend passes) | `max(3*sigma_noise, 0.025)` sRGB | `inkvec-trace/src/regions.rs:278,449` | how close a pixel must be to a mixture hypothesis | measured floor (~6.4/255), no stated reason for that specific floor |
| `reassign_blend_pixels::ROUNDS` | 4 | `inkvec-trace/src/regions.rs` | erosion-like rounds in per-pixel reassignment | none |
| reassign "beats own label" margin | residual < 0.5 * own residual | `inkvec-trace/src/regions.rs` | how much better a blend hypothesis must be to move a pixel | motivated |

## 05 — Gradients ([05-gradients.md](05-gradients.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `PARAMS_FLAT` | 3.0 | `inkvec-trace/src/gradient.rs` | flat-fill description length | derived |
| `PARAMS_LINEAR` | 10.0 | `inkvec-trace/src/gradient.rs:50-51` | linear-gradient description length | derived |
| `PARAMS_RADIAL` | 9.0 | `inkvec-trace/src/gradient.rs:52-53` | circular-radial description length | derived |
| `PARAMS_RADIAL_ELLIPTIC` | 11.0 | `inkvec-trace/src/gradient.rs:54-55` | elliptical-radial description length | derived |
| `PARAMS_STOP` | 4.0 | `inkvec-trace/src/gradient.rs:56-57` | cost of each interior stop | derived |
| `MAX_MID_STOPS` | 2 | `inkvec-trace/src/gradient.rs:58-59` | most interior stops fitted | measured (corpus stop-count survey) |
| `MIN_GRADIENT_PIXELS` | 16 | `inkvec-trace/src/gradient.rs:62` | fewest interior pixels before a gradient is attempted | motivated |
| `BIMODAL_MARGIN` | 0.85 | `inkvec-trace/src/gradient.rs:64-68` | ramp-vs-step decision threshold; overridable via `INKVEC_BIMODAL` (*removed*) | motivated |
| `MIN_VISIBLE_CONTRAST` | 1.5/255 | `inkvec-trace/src/gradient.rs:70-72` | floor on visible contrast for any gradient candidate | motivated |
| `MIN_RAMP_SUPPORT` | 0.10 | `inkvec-trace/src/gradient.rs:73-79` | least fraction of samples a gradient must visibly shade | motivated (concrete regressions, value not derived) |
| `QUANT_HALF_STEP` | 0.5/255 | `inkvec-trace/src/gradient.rs:81-82` | residual dead zone from 8-bit quantisation | derived |
| `MIN_SHARED_BOUNDARY` | 3 | `inkvec-trace/src/gradient.rs:2054` | shortest shared border before a union is considered | none |
| `MAX_FIT_SAMPLES` | 4096 | `inkvec-trace/src/gradient.rs:2056-2063` | cap on samples one fill evaluation walks | measured (runtime: 5ms vs 46s on a 512px image) |
| `FIT_PIXELS_CAP` | 65536 | `inkvec-trace/src/gradient.rs:2067` | pixels any fill fit sees, uniformly subsampled | motivated (cross-referenced to `MAX_FIT_SAMPLES`) |
| `CENTRE_SEARCH_SAMPLES` | 1024 | `inkvec-trace/src/gradient.rs:2080-2084` | radial-centre hill-climb sample count | measured (same output as 4096, 4x cheaper) |
| `CARVE_RESIDUAL` | 0.06 | `inkvec-trace/src/gradient.rs:2515-2519` | floor on the carve candidate threshold | motivated |
| `CARVE_MAX` | 64 | `inkvec-trace/src/gradient.rs:2520-2521` | most features carved from one image | motivated |
| `bic_lambda(n)` | 0.5 * ln(n) | `inkvec-trace/src/gradient.rs:292-298` | fill-selection lambda | derived (Bayesian information criterion) |
| `MAX_SLIVER_MISFIT` | 4.0 | `inkvec-trace/src/gradient/stops.rs:276` (doc at `:13-15`) | rejects a candidate interior stop when the smaller sample-count side of its split holds under 1/10 of the fitted samples *and* its median residual exceeds 4x the larger side's (floored at 1/255) | motivated; reachable from both `merge_gradient_bands_with_ink` and `fit_fill` via the shared `fit_pixels` -> `fit_samples` -> `fit_mid_stops` path; no stated derivation for 4.0 itself |

## 06 — Planar map ([06-planar-map.md](06-planar-map.md))

`build` itself has no free numeric constants — the topology decision is exact integer
equality, not a threshold. The one constant this stage's tests exercise indirectly belongs
to the upstream saddle merge; the two sizes after it choose only speed, never the map:

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `SADDLE_SIGMAS` | 3.0 | `inkvec-trace/src/regions.rs:13` | see 04-regions.md; reused unmodified here | motivated |
| `DIGIT` (`radix_sort_by_node`) | 11 bits (2,048 buckets) | `inkvec-trace/src/planar/cracks.rs:185` | digit width of the incidence radix sort; three passes at 2048 px | motivated (16 KiB of counters stay in L1 cache) |
| `LANES` (`RowRuns::new`) | 16 labels | `inkvec-trace/src/planar/runs.rs:84` | how many labels a run is extended by at once | motivated (two 128-bit compares on x86-64) |

## 07 — Sub-pixel ([07-subpixel.md](07-subpixel.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `MIN_UNMIX_CONTRAST` | 0.02 | `inkvec-trace/src/planar.rs:414` | floor on unmixing contrast below which a point does not move | none |
| `CORNER_COS` | 0.5 (60 deg) | `inkvec-trace/src/planar.rs:424` | turning angle above which the tangent window narrows to 1 point | motivated (geometric bound) |
| `PAR_VERTICES` | 64 | `inkvec-trace/src/planar.rs:506` | fewest points before an edge's vertices are refined in parallel; smallest chunk one thread takes | motivated (a task of ~30 µs at 0.44 µs per vertex against rayon's few-µs split cost); schedule only, output identical |
| `PAR_MAP_VERTICES` | 512 | `inkvec-trace/src/planar.rs:520` | fewest boundary vertices in the map for the refinement, and symmetry detection beside it, to use threads | measured (per-icon serial/parallel timings by vertex count, `planar.rs:512-519`); schedule only, output identical |
| `INKVEC_SUBPX_WIN` (*removed*) (env) | default 1, range 1..=8 | `inkvec-trace/src/planar.rs:353-362` | width of the tangent-estimation window | measured (widening to 2 improved dE00 but cost DISTS and caused a face-order regression; left at 1) |
| `DEFAULT_SIGMA_MODEL` | 0.05 px | `inkvec-trace/src/coverage.rs:39` | see 02-coverage.md | measured |
| `CONTRAST_REF` | 0.25 | `inkvec-trace/src/planar.rs:688` | reference contrast for `simplify_faint`'s inflation | none |
| `MAX_INFLATION` | 4.0 | `inkvec-trace/src/planar.rs:689` | cap on `simplify_faint`'s sigma multiplier | none |
| `JUNCTION_MAX_MOVE` | 1.5 px | `inkvec-trace/src/planar.rs:753` | rejects an intersection solution beyond this move | measured (0.5px tried first, made results worse) |
| `JUNCTION_MIN_CONDITION` | 0.02 | `inkvec-trace/src/planar.rs:757` | minimum eigenvalue ratio admitted for a junction intersection | derived (equivalent to rejecting crossings below ~16 degrees) |
| `junction_fit_points()` | 6 | `inkvec-trace/src/planar.rs:733` | interior points for near-junction extrapolation | none |
| `junction_skip()` | 1 | `inkvec-trace/src/planar.rs:738` | points nearest the junction excluded from the tangent fit | motivated |
| `junction_curvature_points()` | 16 | `inkvec-trace/src/planar.rs:742` | points used to test for significant curvature | none |
| `MIN_QUADRATIC_POINTS` | 5 | `inkvec-trace/src/planar.rs:885` | minimum points before a quadratic tangent model is tried | none |
| `CURVATURE_SIGNIFICANCE` | 3.0 | `inkvec-trace/src/planar.rs:886` | sigma threshold for preferring a quadratic tangent | motivated ("3-sigma" convention) |
| `TAPER_DEGREES` | 55.0 | `inkvec-trace/src/planar.rs:1203` | branch angle admitted for taper testing | motivated (deliberately loose; see 07-subpixel.md) |
| `TAPER_MAX_MOVE` | 8.0 px | `inkvec-trace/src/planar.rs:1208` | largest move a taper estimate may make | motivated |
| `TAPER_MAX_CONSUMED` | 0.35 | `inkvec-trace/src/planar/junctions.rs:387` | fraction of the shortest incident boundary a taper move may consume | none at the current declaration — an earlier revision's derivation comment was lost when this constant moved into `planar/junctions.rs` during the module split |
| `TAPER_MAX_SIGMA` | 1.0 px | `inkvec-trace/src/planar.rs:1224` | largest standard error a taper estimate may carry | motivated |
| `TRIM_MIN_POINTS` | 4 | `inkvec-trace/src/planar.rs:1112` | fewest points an edge keeps after trimming | motivated |
| `TRIM_LOOK` | 3 | `inkvec-trace/src/planar.rs:1116` | how far ahead to look when deciding an edge's direction | motivated |
| `MIN_WIDTH` / `MAX_WIDTH` | 0.05 / 6.0 px | `inkvec-trace/src/planar/taper.rs:98,102` | taper sample admission band | motivated |
| `MIN_SAMPLES` | 4 | `inkvec-trace/src/planar/taper.rs:105` | fewest taper samples trusted | none |
| `MAX_DEFECT` | 0.15 px | `inkvec-trace/src/planar/taper.rs:115` | largest tangent-circle residual admitted | measured (calibrated against three worked cases) |

## 08 — Boundary solve ([08-boundary-solve.md](08-boundary-solve.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `MAX_STEP` | 0.35 px | `inkvec-trace/src/boundary_opt.rs:110` | largest displacement of any point in one L-BFGS step | none |
| `MAX_TOTAL` | 1.0 px | `inkvec-trace/src/boundary_opt.rs:114` | total leash from the point's starting position; also why the band never moves | motivated |
| `K_KINK` | 0.05 | `inkvec-trace/src/boundary_opt.rs:121` | kink weight, fraction of the data term's initial value | motivated (scaling rule derived, value not swept) |
| `K_ANCHOR` | 0.10 | `inkvec-trace/src/boundary_opt.rs:124` | anchor weight, fraction of the data term's initial value | motivated |
| `JUNCTION_ANCHOR` | 4.0 | `inkvec-trace/src/boundary_opt.rs:126` | multiplier on `w_anchor` at a junction point | motivated |
| `MIN_CONTRAST` | 2.0/255 | `inkvec-trace/src/boundary_opt.rs:128` | colour or opacity difference that counts as a boundary when choosing where alpha is a fourth channel | none |
| `EPS` (in `priors`) | 1e-4 | `inkvec-trace/src/boundary_opt.rs:440` | floor inside the kink term's square root | motivated |
| `REACH` | 1 px | `inkvec-trace/src/boundary_opt/band.rs:71` | band width (Chebyshev) round the pixels the starting boundary crosses | derived (follows from `MAX_TOTAL`) |
| `MEMORY` | 3 | `inkvec-trace/src/boundary_opt/lbfgs.rs:41` | L-BFGS pairs kept | measured (3 did as well as 7, 15 or 30) |
| `C1` | 1e-4 | `inkvec-trace/src/boundary_opt/lbfgs.rs:43` | Armijo sufficient-decrease constant | motivated (the textbook value, Nocedal & Wright 2006) |
| `MAX_TRIALS` | 8 | `inkvec-trace/src/boundary_opt/lbfgs.rs:45` | step halvings per line search | none |
| `MAX_ITERS` | 32 | `inkvec-trace/src/boundary_opt/lbfgs.rs:49` | iterations per independent part | measured (24 read 0.3666, 32 read 0.3585, 48 read 0.3558 on the screen set; 48 up to 1.9x slower) |
| `PARAM_TOL` | 0.005 px | `inkvec-trace/src/boundary_opt/lbfgs.rs:52` | stop once no point moves more in a step | derived (half the SVG's 0.01 px) |
| `FUNC_TOL` | 1e-4 | `inkvec-trace/src/boundary_opt/lbfgs.rs:55` | stop once a step lowers the changeable energy by less than this fraction | none |
| `INKVEC_BOPT_ITERS` (*removed*) (env) | 48 | `inkvec-trace/src/boundary_opt/lbfgs.rs` | iteration cap of the former conjugate-gradient solve; now `MAX_ITERS` | measured (24 was found unconverged) |
| `INKVEC_BOPT_MS` (*removed*) (env) | 1200 ms | `inkvec-trace/src/boundary_opt.rs` | time budget; now only the caller's `budget_ms` | measured (60s budget gave the same result) |
| segment bucket limit | cell range `(x1-x0)*(y1-y0) <= 64` | `inkvec-trace/src/boundary_opt/folds.rs` | segments the fold guard counts | motivated (kept exactly from the hash grid it replaced) |
| fold-guard floor | `s > 0.1` | `inkvec-trace/src/boundary_opt.rs:658` | how far the guard halves the displacement before giving up | none |

## 09 — Decode ([09-decode.md](09-decode.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `LEAK_GATE` | 0.05 | `inkvec-trace/src/decode.rs` | diagnostic threshold on `leak` (not gated on) | none |
| `MIN_VERTS` / `MAX_VERTS` | 3 / 16 | `inkvec-trace/src/decode.rs` | vertex-count range a candidate order may propose | none |
| `MAX_DEV` | 2.0 px | `inkvec-trace/src/decode.rs` | worst-case chord deviation before a face is "something else" | none |
| `CURVE_BIAS_PX` | 0.35 px | `inkvec-trace/src/decode.rs` | mean-offset threshold in `is_polygonal`'s curve test | none |
| `MIN_GAIN` | 0.5 | `inkvec-trace/src/decode.rs` | residual-cut fraction of `sse0` required for a decode | measured (screen set: 10 worse/7 better at 1.0, better on every axis at 0.5) |
| `EVIDENCE_OVERRIDE` | 0.0 (off) | `inkvec-trace/src/decode.rs` | residual-ratio threshold to skip the post-solve shape recheck | measured trade-off (0.5 fixes one case, costs 0.6% objective; default declines the trade) |
| `CURVE_MIN_SAMPLES` | 8 | `inkvec-trace/src/decode.rs` | fewest ring samples before the curve test applies | none |
| `THIN_PX` | 2.5 px | `inkvec-trace/src/decode.rs` | width above which a face is not attempted | measured (conditioning cliff location) |
| `PARAMS_PER_RIBBON` | 10.0 | `inkvec-trace/src/decode.rs` | parameter charge for one pooled ribbon | derived |
| `SHARE_MAX_PX` | 1.75 px | `inkvec-trace/src/decode.rs` | widest ribbon `share_widths` will pool | motivated |
| `MAX_RING_POINTS` | 512 | `inkvec-trace/src/decode.rs` | largest ring `ring_of` will accept | motivated |
| `PIXELS_PER_UNKNOWN` | 4 | `inkvec-trace/src/decode.rs` | boundary-cut band pixels required per free coordinate | motivated |
| `MAX_BBOX_PIXELS` | 20,000 | `inkvec-trace/src/decode.rs` | largest face bounding box attempted | none |
| `GN_ITERS` | 14 | `inkvec-trace/src/decode.rs` | Gauss-Newton iteration cap | none |
| `FD_STEP` | 0.01 px | `inkvec-trace/src/decode.rs` | finite-difference step for the coverage derivative | none |
| `MAX_STEP` | 0.35 px | `inkvec-trace/src/decode.rs` | per-iteration vertex step clamp; shared value with `boundary_opt::MAX_STEP` | motivated |
| `MAX_TOTAL` | 1.0 px | `inkvec-trace/src/decode.rs` | cumulative leash from a vertex's starting position | measured (uncapped: one emoji frame drifted to a 13-sided polygon) |
| `DECODED_SIGMA` | 0.05 px | `inkvec-trace/src/decode.rs` | sigma given to written-back samples | motivated (matches `coverage::DEFAULT_SIGMA_MODEL`) |
| `SAMPLE_PX` | 1.0 px | `inkvec-trace/src/decode.rs` | spacing of written-back samples along a decoded edge | motivated |
| GN damping schedule | mu0 1e-3, x4/reject, /3/accept, floor 1e-7, cap 1e9 | `inkvec-trace/src/decode.rs` | Levenberg step damping | motivated (standard schedule shape) |
| ridge in `varpro` | `1e-6 * trace / k` | `inkvec-trace/src/decode.rs` | regularises a fill column with no pixel support | motivated |
| turning-corner angle floor | 0.4 rad | `inkvec-trace/src/decode.rs` | minimum turning angle counted as a corner | none |

## 10 — Symmetry ([10-symmetry.md](10-symmetry.md))

No numeric thresholds. Every test is exact equality on integer pixel coordinates or exact
half-integer lattice points (`inkvec-trace/src/symmetry.rs:19-21,156-163,243-245,250-252`)
— a deliberate design choice, not an omission.

## 11 — Curve fitting ([11-fitting.md](11-fitting.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `PARAMS_LINE` | 2.0 | `inkvec-fit/src/lib.rs:41` | line parameter cost | derived |
| `PRUNE_SLACK` | 4.0 | `inkvec-fit/src/lib.rs:45`, `multimodel.rs:150` | safety factor on scan cut-off | motivated |
| `CORNER_CHAMFER` | 1.0 px | `inkvec-fit/src/lib.rs:457` | corner-adjustment chamfer allowance | derived (one level-set sampling step) |
| `CORNER_TURN_MIN` | pi/6 (30 deg) | `inkvec-fit/src/lib.rs:460` | when a vertex meeting is treated as a corner | none |
| `CORNER_DEGREES` | 45.0 | `inkvec-fit/src/lib.rs:722` | corner-vs-smooth-join threshold | none |
| max-shift factor | 3.0x max(sigma), floor 0.25 | `inkvec-fit/src/lib.rs:800` | corner intersection displacement cap | motivated |
| `PARAMS_CUBIC` | 6.0 | `inkvec-fit/src/multimodel.rs:102` | cubic parameter cost | derived |
| `G1_BREAK_DEGREES` | 10.0 | `inkvec-fit/src/multimodel.rs:137` | tangent break below which a join is nearly free | none; swept empirically |
| `MAX_ARM` | 1.0 | `inkvec-fit/src/multimodel.rs:142`, `merge.rs:117` | largest admissible control arm, as a fraction of chord | derived |
| `MAX_RESIDUAL_SAMPLES` | 32 | `inkvec-fit/src/multimodel.rs:147` | cubic residual evaluation points (O(1) cap) | none |
| `PRUNE_PATIENCE` | 8 | `inkvec-fit/src/multimodel.rs:158` | over-budget candidates before scan stops | measured, value not stated |
| `DP_MAX_POINTS` | 768 | `inkvec-fit/src/multimodel.rs:163` | decimation threshold | none |
| `TANGENT_WINDOW_MAX` | 16 | `inkvec-fit/src/multimodel.rs:166` | widest one-sided tangent window | none |
| `NEWTON_STEPS` | 3 | `inkvec-fit/src/multimodel.rs:170` | Newton steps for arc-length projection | none |
| `FREE_MAX_SWING` | 75.0 deg | `inkvec-fit/src/multimodel.rs:1103` | how far a free cubic's tangent may depart from the estimate | none |
| `DIRECTION_SAMPLES` | 8 | `inkvec-fit/src/multimodel.rs:1491` | monotone-sweep samples for arc validity | none (asserted) |
| `MAX_ASPECT` | 12.0 | `inkvec-fit/src/multimodel.rs:1666` | most elongated ellipse worth fitting | none |
| `MIN_POINTS` (ellipse) | 24 | `inkvec-fit/src/multimodel.rs:1669` | minimum points to try an ellipse | none |
| `LENGTH_STRIDE` | 16 | `inkvec-fit/src/multimodel.rs:1676` | ellipse candidate-length sampling | none; doc comment ("every fourth length") does not match the value |
| bow-penalty factor | 4.0 | `inkvec-fit/src/multimodel.rs:1301` | arc-vs-line residual ratio that triggers the bow penalty | none |
| `PARAMS_ARC` | 5.0 | `inkvec-fit/src/curves.rs:158` | circular arc cost | derived |
| `PARAMS_ELLIPTICAL_ARC` | 7.0 | `inkvec-fit/src/curves.rs:165` | elliptical arc cost | derived |
| `MAX_ARC_DEGREES` | 120.0 | `inkvec-fit/src/primitives.rs:50` | longest single-arc sweep | derived (conditioning argument) |
| `MAX_REDUCED_CHI2` | 4.0 | `inkvec-fit/src/primitives.rs:1081` | primitive acceptance gate | derived (tau^2 at default tau=2) |
| `PARAMS_CIRCLE` / `PARAMS_ELLIPSE` / `PARAMS_ROUND_RECT` / `PARAMS_RECT` | 3.0 / 5.0 / 6.0 / 4.0 | `inkvec-fit/src/primitives.rs:32-39` | primitive parameter costs | derived |
| `BREAK_PARAMS` | 2.0 | `inkvec-fit/src/merge.rs:53` | join a free cubic no longer meets smoothly; overridable via `INKVEC_MERGE_BREAK` (*removed*) | derived |
| `MAX_SPAN` | 96 | `inkvec-fit/src/merge.rs:57` | longest merge run attempted | none |
| `MAX_RUN` | 4 | `inkvec-fit/src/merge.rs:65` | segments a merge run may absorb | none (was overridable, no longer swept) |
| `MAX_ROUNDS` | 6 | `inkvec-fit/src/merge.rs:75` | merge sweep passes | motivated |
| `SEARCH_DEGREES` | 100.0 | `inkvec-fit/src/merge.rs:111` | free-cubic angle search width | motivated (a 60-degree clamp put the optimum outside the search) |
| `SHARPEN_MAX_CHORD` | 2.5 | `inkvec-fit/src/merge.rs:472` | chamfer-cubic chord ceiling | motivated (chamfer ~1px/side) |
| `SHARPEN_MAX_EDGE` | 8.0 | `inkvec-fit/src/merge.rs:477` | short-edge-with-chamfers ceiling | none |
| `SHARPEN_MIN_TURN` | pi/6 (30 deg) | `inkvec-fit/src/merge.rs:479` | corner-vs-smooth threshold | none; duplicates `CORNER_TURN_MIN` |
| `PARAMS_AXIS_LINE` | 1.0 | `inkvec-fit/src/merge.rs:646` | axis-snapped line cost | derived |
| `MAX_AXIS_DEV_SIGMA` | 3.0 | `inkvec-fit/src/merge.rs:651` | per-sample axis-snap deviation cap | none |
| `PARAMS_SMOOTH_CUBIC` | 4.0 | `inkvec-fit/src/merge.rs:831` | `S`-shorthand cost | derived |
| G1 pre-filter angle | 20 deg | `inkvec-fit/src/merge.rs:902` | when a smooth-join candidate is worth the exact refit | none |
| `FLATTEN` | 16 | `inkvec-fit/src/simple.rs:51` | flattening resolution for the self-crossing test | motivated (below render-visibility floor) |
| `MAX_REPAIRS` | 8 | `inkvec-fit/src/simple.rs:55` | span-cap halvings before falling back | derived (2^8 covers any contour produced) |
| `EPS` (endpoint coincidence) | 1e-6 px | `inkvec-fit/src/simple.rs:59` | adjacency exemption tolerance | derived |

## 12 — Repair ([12-repair.md](12-repair.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `ROUNDS` | 10 | `inkvec-cli/src/rings.rs:64` | max halving rounds of the outer repair loop | none; one comment refers to "15 rounds" (`rings.rs:222`), inconsistent with this value |
| `MERGE_BUDGET` | 96 segments | `inkvec-cli/src/rings.rs:164` | per-boundary merge affordability (4x this is the global ceiling) | measured (~8ms/segment) |
| `RING_SAMPLES` | 4 | `inkvec-cli/src/rings.rs:456` | interior samples per curved segment for area/containment | none |
| crossing-pair limit | 32 | `inkvec-cli/src/rings.rs:96,260` | max crossing pairs reported per ring per call | none |
| explosion thresholds | `c > 32 && c > 4*f` | `inkvec-cli/src/rings.rs:230` | when a capped refit is discarded for the unconstrained fit | measured (`bulma` case); the pair (32, 4) not separately justified |
| cap initial value | `polys[k].len().max(2)` | `inkvec-cli/src/rings.rs:69` | starting span cap per edge | derived |
| halving rule | `(cap[k]/2).max(1)` | `inkvec-cli/src/rings.rs:126,131` | tightening schedule, keeps repair logarithmic | derived |
| `BLK` | 16 | `inkvec-cli/src/rings.rs:227` | bounding-box block size for the all-pairs prune | motivated (speed only) |

`FLATTEN`, `MAX_REPAIRS` and `EPS` are shared with `inkvec-fit/src/simple.rs` — see 11-fitting.md.

## 13 — Emit ([13-emit.md](13-emit.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `EMIT_DECIMALS` | 2 | `inkvec-cli/src/emit.rs:54` | coordinate decimal places | derived — see 13-emit.md, "Coordinate precision" |
| `MIN_RING_AREA` | 0.25 px² | `inkvec-cli/src/emit.rs:57` | smallest ring area worth emitting | motivated |
| `S`-shorthand tolerance (`fmt_ring`) | 10^-decimals (0.01 px default) | `inkvec-cli/src/emit.rs:193` | rounded-space reflection test | derived |
| `S`-shorthand tolerance (`fmt_fitted`) | 5e-4, raw-space | `inkvec-cli/src/emit.rs:77` | stroke-path reflection test | none |
| ring segment minimum | 2 | `inkvec-cli/src/emit.rs:252` | a ring is discarded below this | none |
| path point minimum | 3 | `inkvec-cli/src/emit.rs:133` | `fmt_path` early return | derived (degenerate-polygon guard) |
| arc rotation precision | 3 decimals | `inkvec-cli/src/emit.rs:105,235,291` | arc `phi` formatting | none |
| rounded-rect radius floor | 1e-4 | `inkvec-cli/src/emit.rs:305,367` | below this, written as a plain rect | none |
| ellipse rotation floor | 1e-3 | `inkvec-cli/src/emit.rs:344` | below this, no `transform` written | none |
| alpha-ramp endpoint precision | 2 decimals | `inkvec-cli/src/emit.rs:715` | independent of `EMIT_DECIMALS` | none |
| `RAMP_MIN_INTERIOR` | 64 px | `inkvec-cli/src/alpha.rs:102` | fewest interior pixels before a plane is fitted to a face's alpha; `face_alpha` skips smaller faces without calling the fit | none (the fit's own first test, now named) |
| `RAMP_MIN_FADE` | 0.15 (opacity) | `inkvec-cli/src/alpha.rs:96` | least opacity change across a face for it to count as a fade | none |
| `RAMP_MAX_RESIDUAL` | 0.06 (opacity) | `inkvec-cli/src/alpha.rs:99` | largest RMS residual of the alpha plane | none |
| opacity precision | 3 decimals | `inkvec-cli/src/emit.rs:626,660,905` | `fill-opacity`/`stop-opacity` | none |
| `INKVEC_EMIT_DECIMALS` (env) | overrides `EMIT_DECIMALS` | `inkvec-cli/src/emit.rs:126` | the mechanism used to isolate rounding from segment price in the 7.2% measurement | measured |
| background-match precision | 2 decimals (hard-coded) | `inkvec-cli/src/post.rs:42-52` | `--no-background` element matching | none; coupled to `EMIT_DECIMALS` but not derived from it — breaks silently if the two diverge |
| margin viewBox precision | 2 decimals | `inkvec-cli/src/post.rs:147` | `--margin` growth | none |
| `ci_gate.py` ratio limit | 5% relative | `bench/ci_gate.py:33` | regression gate on parameter count vs. artist | measured (project's compactness regression budget) |

## 14 — Fast mode ([14-fast-mode.md](14-fast-mode.md))

Fast mode's own stages, under `inkvec-trace/src/fast/`. Several rows only choose a
schedule (serial or parallel, how the work is cut) or where an exact shortcut applies:
those say so, and none of them can change the output. The fitter round of 2026-09-30 added
`RUN_MAX_STEP`, `RUN_MIN_TOL`, `PARALLEL_MIN` and `PARALLEL_CHUNK` (all in `polygon.rs`).

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `BINS` | 2^16 | `inkvec-trace/src/fast/palette.rs:106` | size of the bin key space: 5+5+5 bits opaque, 4+4+4+4 bits with opacity | derived |
| `MIN_FLAT` | 3 | `inkvec-trace/src/fast/palette.rs:108` | flat pixels a bin needs before it can found an ink | none |
| `SAME_INK` (fast) | 0.012 (OKLab) | `inkvec-trace/src/fast/palette.rs:110` | inks this close are one ink whether or not their bins touch | none (same value as Quality's `JND_FLOOR`) |
| `MAX_THIN_CANDIDATES` | 48 | `inkvec-trace/src/fast/palette.rs:112` | most candidates the thin-ink blend test weighs | motivated (the test is cubic in them) |
| `MIN_PAIRED` | 8 | `inkvec-trace/src/fast/palette.rs:114` | paired pixels a bin needs to be a thin ink: a stroke 8 px long | motivated |
| `THIN_SHARE` | 0.0005 | `inkvec-trace/src/fast/palette.rs:116` | share of the image a thin ink's paired pixels must be (8 px at 128 px, about 2,100 at 2048 px) | motivated |
| `BELOW_HALF` | 0.5 - 2^-25 | `inkvec-trace/src/fast/palette.rs:172` | addend that makes truncation round half away from zero in `level` | derived (proof in the doc comment; identical on every float in [0, 1] for both grids, checked exhaustively) |
| `START_BITS` / `GOLDEN` (`Slots`) | 64 entries / `0x9E37_79B9` | `inkvec-trace/src/fast/palette.rs:342-344` | starting size of the key-to-bin hash table (doubling, at most half full) and its multiplier | derived (`GOLDEN` is floor(2^32 / phi), multiplicative hashing, Knuth TAOCP vol. 3 §6.4); start size motivated (512 bytes per row band) |
| `EXACT_SUM_MAX_PIXELS` | 2^22 (2048 x 2048) | `inkvec-trace/src/fast/palette.rs:445` | most pixels for which the per-bin f64 sums are provably exact, so row bands may be merged in any order | derived (the exact-sum lemma, `palette.rs:447-476`) |
| `LOW` (`in_exact_set`) | 2^-8 | `inkvec-trace/src/fast/palette.rs:480` | smallest non-zero value in the exact set | derived (every 8-bit value is 0 or at least 1/255) |
| `MIN_BAND_ROWS` | 16 | `inkvec-trace/src/fast/palette.rs:637` | fewest rows in a band of the parallel histogram | motivated (each band keys two rows beyond its own); schedule only |
| `TARGET_BANDS` | 64 | `inkvec-trace/src/fast/palette.rs:639` | about how many bands an image is cut into | motivated (rayon balance); schedule only |
| `REACH` (fast) | 4 px | `inkvec-trace/src/fast/palette.rs:1113` | farthest a blend pixel looks for ink-coloured neighbours | none |
| `KEEP_OWN` | 3.0 x `merge_distance` | `inkvec-trace/src/fast/palette.rs:1117` | how near its own nearest ink an unexplained pixel must be to keep it | motivated |
| `PARALLEL_MIN_PIXELS` (palette) | 65,536 px (256 x 256) | `inkvec-trace/src/fast/palette.rs:1135` | below this, and with one rayon worker, every palette pass runs on the calling thread | measured (at 128 px each parallel pass cost more than it saved); schedule only |
| `F32_COUNT_LIMIT` | 2^24 | `inkvec-trace/src/fast/palette.rs:1379` | where the old f32 running count of ones stopped growing, reproduced in `ink_shares` | derived |
| `BLEND_TOL` | 0.04 (sRGB and opacity) | `inkvec-trace/src/fast/faces.rs:157` | largest distance from a pixel to the line between two inks for it to count as their blend | none |
| `SAME_ALPHA` | 0.02 | `inkvec-trace/src/fast/faces.rs:198` | largest opacity difference between two inks that can be one ink | none |
| `SCAN_LANES` | 8 labels | `inkvec-trace/src/fast/faces/runs.rs:126` | labels compared per step while a run is scanned (one 128-bit vector of u16) | motivated; speed only |
| `SPECKLE_PER_512` / `SPECKLE_MAX` | 4.0 px per 512 x 512 / 16.0 px | `inkvec-trace/src/fast/front.rs:32-34` | the speckle floor grows with image area, from `min_region` up to VTracer's 4 x 4 patch | measured (20 brand logos at 2048 px: 14% fewer coordinates, dE00 0.0729 -> 0.0725; masthead 19% at +0.01) |
| `RAMP_STEP` | 0.09 (OKLab) | `inkvec-trace/src/fast/bands.rs:42` | largest ink distance between two adjacent faces for them to be bands of one ramp; also the palette precheck | motivated (one or two merge distances) |
| `MIN_CONTACT` | 3 px edges | `inkvec-trace/src/fast/bands.rs:44` | shared border that counts as adjacency, not a corner touch | motivated |
| `MIN_PIXELS` (bands) | 64 | `inkvec-trace/src/fast/bands.rs:46` | smallest cluster worth a gradient | none |
| `SAMPLE_PIXELS` / `CHECK_SAMPLES` | 65,536 / 4,096 | `inkvec-trace/src/fast/bands.rs:48-50` | pixels gathered per cluster for its fit / sampled by the acceptance test | motivated / none |
| `RATIO` / `SLACK` | 1.25 / 1/255 | `inkvec-trace/src/fast/bands.rs:54, 61` | a gradient is kept when its RMS residual is at most `RATIO` x the bands' plus `SLACK` | motivated |
| `FLAT_ENOUGH` / `INVISIBLE` | 1.5/255 / 2/255 | `inkvec-trace/src/fast/bands.rs:56, 59` | two bands this well explained stay flat / a gradient this close is kept regardless | motivated |
| `FastFit` defaults | `poly_tol` 0.5, `vertex_box` 0.5, `corner_tol` 0.25, `opt_tol` 0.2, `flat` 0.05 px | `inkvec-trace/src/fast/mod.rs:91-101` | the fitter's tolerances: polygon, vertex box, corner test (Potrace's `alphamax` as a distance), curve merge (Potrace's `opttolerance`), cubic-to-line | none |
| `FAINT` / `GRADIENT_LOOSEN` / `MAX_LOOSEN` | 0.12 / 1.6 / 3.0 | `inkvec-trace/src/fast/mod.rs:199, 201, 203` | tolerances grow as `FAINT / contrast` up to `MAX_LOOSEN` on a faint boundary; a gradient face's boundary is fitted as if its contrast were at most `FAINT / GRADIENT_LOOSEN` | motivated |
| first-point drop (`fit_edge`) | rings of 8 points or more | `inkvec-trace/src/fast/mod.rs:233` | a ring that long drops its first point, the lattice node the refinement left up to 0.6 px off the edge | none |
| image frame (`frame_rectangle`) | the rectangle `[-0.5, w - 0.5] x [-0.5, h - 0.5]` | `inkvec-trace/src/fast/mod.rs:283-284` | a ring every point of which lies exactly on this border, passing each corner once, is written as its four lines instead of fitted | derived (pixel centres at integers; the border nodes are exact and never refined) |
| `MAX_SPAN` (polygon) | 160 points | `inkvec-trace/src/fast/polygon.rs:204` | most points one polygon side may span | motivated (without it the scan is quadratic on long straight boundaries) |
| `RUN_MAX_STEP` | 4.0 px | `inkvec-trace/src/fast/polygon.rs:209` | longest lattice step the polygon scan takes in closed form | derived (bounds the step in the closed form's exactness proof; points are about 1 px apart, so it never binds); speed only |
| `RUN_MIN_TOL` | 1/16 px | `inkvec-trace/src/fast/polygon.rs:213` | smallest tolerance at which lattice runs are scanned in closed form (the fitter's own is at least 0.5 px) | derived (bounds the proof's margins); speed only |
| admission slack (`Cone::admits`) | 1e-9 | `inkvec-trace/src/fast/polygon.rs:156` | how far outside the cone a side's direction may fall (cross products) and still be admitted | none (the closed-form proof shows its margins are far beyond it) |
| `PARALLEL_MIN` (polygon) | 2048 points | `inkvec-trace/src/fast/polygon.rs:502` | boundaries this long scan their anchors in parallel before the sequential relaxation | measured (phase-1 replay: none of 6,645 screen edges, 16 of 564 at 2048 px, which include the image frame and set the fit's wall time); schedule only |
| `PARALLEL_CHUNK` | 128 anchors | `inkvec-trace/src/fast/polygon.rs:505` | anchors per parallel task (rayon `with_min_len`) | motivated (enough that a task outweighs its scheduling); schedule only |
| `CORNER_COS` (`denoise`) | 0.64 (50 deg) | `inkvec-trace/src/fast/smooth.rs:277` | two-step turn above which the smoothing filter keeps a point as a corner | none |
| `JOIN_MAX` | 0.5 px | `inkvec-trace/src/fast/smooth.rs:280` | largest distance a join is moved off its side's line towards the data | none |
| `RIDGE` | 1e-3 | `inkvec-trace/src/fast/smooth.rs:169` | pull towards the polygon's own vertex in the constrained vertex placement | motivated (keeps parallel sides well posed) |
| `MAX_TURN` / `MAX_RUN` | 3.10 rad / 24 pieces | `inkvec-trace/src/fast/curve.rs:21, 24` | most a merged cubic may turn / longest run one merge may cover | motivated |
| `TOL` (prims) | 0.3 px | `inkvec-trace/src/fast/prims.rs:20` | largest distance from any point to an accepted circle or ellipse; `2 * TOL` is the screen for the Kasa circle and Taubin's conic | none |
| `ROUGHLY_ROUND` / `MIN_POINTS` / `MIN_RADIUS` / `MIN_AREA_SHARE` | 0.6 / 12 / 1.5 px / 0.8 | `inkvec-trace/src/fast/prims.rs:23, 25, 27, 30` | gates of the primitive cascade | motivated (`MIN_AREA_SHARE` rejects a sliver along an arc) / none |
| point weight (prims) | sigma 0.5 px | `inkvec-trace/src/fast/prims.rs:164` | the per-point sigma every circle and ellipse fit in the primitive test uses | none |
