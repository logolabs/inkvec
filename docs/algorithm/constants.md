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
| `REF_EXTENT` | 128.0 px | `inkvec-cli/src/units.rs:17-31` | intake size every pixel-denominated constant was tuned at; above it `price_in_raster_units` scales `--min-area` and lambda | measured (3.54x/6.02x/11.62x the artist's parameters at 128/256/512 px, `units.rs:17-26`; the guard: 128ss objective 0.4005 -> 0.4112 without it, `inkvec-cli/src/lib.rs:416-422`) |
| `INTAKE_SCALE_FLOOR` | 1.5 | `inkvec-cli/src/lib.rs:116-118` | below this, `--intake-scale` leaves the input alone | measured (every corpus image reads 1.00) |
| `INTAKE_SCALE_CAP` | 8.0 | `inkvec-cli/src/lib.rs:119-121` | ceiling on how much `--intake-scale` discards | motivated |
| `--max-dim` default | 2048 px | `inkvec-cli/src/args.rs:229, 316-320` | ceiling on traced (not emitted) size, applied at decode and again in `intake` | motivated |
| `--time-budget` split | 0.6 merge / 0.25 boundary-solve | `inkvec-cli/src/pipeline.rs:139-149` | advisory wall-clock split between the two stages that read a clock | none |
| boundary-solve budget floor | 50 ms | `inkvec-cli/src/pipeline.rs:145` | least wall-clock budget the boundary solve gets under any `--time-budget` | none |
| `MAX_FACTOR` (`pixel_grid`) | 32 | `inkvec-cli/src/alpha/unblock.rs:69` | largest replication factor the unblock pre-pass tries | motivated |
| `smallest` (`pixel_grid`) | 64 px | `inkvec-cli/src/alpha/unblock.rs:78-81` | least side a factor must leave; under 128 px a raster is never unblocked | motivated |
| block-constant tolerance | 1/512 per channel | `inkvec-cli/src/alpha/unblock.rs:23-25, 188` | how exactly a block must match to count as replication | motivated |
| `SHARP` (`change_gcd`) | 1/256 per channel | `inkvec-cli/src/alpha/unblock.rs:131-132` | neighbour difference that counts as a change position for the unblock gcd filter | derived (twice the block tolerance, `unblock.rs:43-51`) |
| `INTAKE_PARALLEL_MIN` | 65,536 px (256 x 256) | `inkvec-cli/src/alpha.rs:825-827` | below this the transparency scan and the flatten run on the calling thread | motivated |
| `FLATTEN_CHUNK` | 16,384 px | `inkvec-cli/src/alpha.rs:828-829` | pixels per parallel job of the flatten, smallest job of the transparency scan | none |
| `PARALLEL_MIN_PIXELS` (load) | 65,536 px (256 x 256) | `inkvec-trace/src/load.rs:274-276` | below this the byte-to-float conversion runs on the calling thread | motivated |
| `CONVERT_CHUNK_PIXELS` | 65,536 px | `inkvec-trace/src/load.rs:278-280` | pixels per parallel job of the byte-to-float conversion (64 jobs at 2048 x 2048) | motivated |
| `UNIT` | `k / 255`, k = 0..=255 | `inkvec-trace/src/load.rs:282-296` | the float each 8-bit sample becomes | derived (the old division's quotients, checked bit for bit) |
| `DEFAULT_MAX_ALLOC` | 512 MiB | `inkvec-trace/src/load.rs:211-212` | decode allocation limit every uncapped decode keeps | derived (the `image` crate's own default, checked by `the_capped_limit_extends_the_library_default`, `load.rs:953-969`) |
| `CAPPED_EXTRA_ALLOC` | 768 MiB (64-bit), 0 (32-bit) | `inkvec-trace/src/load.rs:214-223` | extra allocation a decode that will be capped may make, 1.25 GiB in all | motivated (holds 16384 x 16384 RGBA 8-bit or 12000 x 12000 16-bit, still refuses the 20000 x 20000 fuzz bomb; nothing on wasm's 4 GiB) |
| `SRGB_TOLERANCE` | 1 level | `inkvec-trace/src/load/icc.rs:66-67` | largest probe-colour move for an embedded profile to count as sRGB (image left untouched) | motivated (`icc.rs:30-34`: converting would only add a level of rounding noise) |
| `ROWS_PER_JOB` | 64 rows | `inkvec-trace/src/load/icc.rs:69-70` | rows per parallel job of the ICC-to-sRGB conversion | none |
| `COMPOSITE_PARALLEL_MIN` | 65,536 px (256 x 256) | `inkvec-trace/src/coverage.rs:235-236` | below this the composite over white runs on the calling thread | motivated |
| `MARGIN` (`choose_matte`) | 10.0 (CIEDE2000) | `inkvec-cli/src/alpha.rs:416` | closeness to a matte candidate to count as "swallowed" | none |
| `SWALLOWED` | 0.33 | `inkvec-cli/src/alpha.rs:324-329` | share of drawn-and-translucent mass a matte may swallow | motivated |
| `LOST_TO_WHITE` | 0.5 | `inkvec-cli/src/alpha.rs:331-341` | share of the silhouette lost to a white matte above which the cutout is turned on (without native alpha) | measured (white marks on transparent read 1.00, no screen-set icon above 0.32; the dE00-10 margin cost `emoji_u1f5a8` 0.53 -> 0.76) |
| `DRAWN` | 0.5 | `inkvec-cli/src/alpha.rs:409-417` | alpha above which a pixel counts as silhouette, not glow | motivated |
| `DRAWN_FLOOR` | 0.05 | `inkvec-cli/src/alpha.rs:418` | alpha below which a pixel is ignored entirely | none |
| `SOFT_SHARE` | 0.05 | `inkvec-cli/src/alpha.rs:438` | glow share that keeps white without the candidate ladder | none |
| `FLAT_ALPHA` | 0.02 | `inkvec-cli/src/alpha.rs:439` | neighbour-alpha spread counted as "flat" translucency | none |
| `DEGRADED_RESIDUAL` / `--sr-threshold` | 0.5 | `inkvec-sr/src/detect.rs:13-36` | interior-residual threshold above which `--sr auto` cleans | measured (30 icons, 5 conditions) |
| `--sr-scale` default | 2 | `inkvec-cli/src/args.rs:252` | output scale of the SR pre-pass | none |
| interior-residual normalisation | `sum / 9n` | `inkvec-sr/src/detect.rs:103-110` | matches the reference Python implementation | derived (deliberate match, not a bug) |

## 02 — Coverage ([02-coverage.md](02-coverage.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `DEFAULT_SIGMA_MODEL` | 0.05 px | `inkvec-trace/src/coverage.rs:63-81` | floor added in quadrature to noise-derived positional sigma; 79% of the gate set's 86,060 boundary points sit at 0.050-0.060 | measured (analytic circles, `inkvec-trace/tests/subpixel.rs:107-167`; 0.10 moves the gate like `--lambda-scale 4.0`, `coverage.rs:73-80`) |
| `MAX_SIGMA` | 4.0 px (total clamped to `[0.001, 4]`) | `inkvec-trace/src/coverage.rs:162-170` | ceiling on positional uncertainty when the gradient vanishes | none |
| plateau fraction | 0.001 | `inkvec-trace/src/coverage.rs:441-444, 460` | how much of the luminance extreme defines `fg`/`bg` | motivated |
| extreme-band tolerance | 0.15 | `inkvec-trace/src/coverage.rs:465-466` | which pixels near each extreme are averaged into `fg`/`bg` | none |
| `LAPLACIAN_KERNEL` | `[4, -1, -1, -1, -1]` | `inkvec-trace/src/coverage.rs:300-306` | the noise estimate's 4-neighbour kernel | derived (the loop's arithmetic, pinned by `coverage.rs:1229-1240`) |
| Laplacian noise gain | sqrt(20), computed from `LAPLACIAN_KERNEL` | `inkvec-trace/src/coverage.rs:395` | corrects the 4-neighbour Laplacian's noise gain | derived; a hardcoded `sqrt(6)` was a bug, fixed 2026-09-08 |
| `NOISE_QUANTILE` | 0.10 | `inkvec-trace/src/coverage.rs:409-410` | quantile of the absolute Laplacian read as the noise level (was the median) | measured (median read 53 levels on a noiseless labyrinth against 0.57; 246 icons at 128/512/1024 px byte-identical, `coverage.rs:380-392`) |
| `Z10` | 0.12566 | `inkvec-trace/src/coverage.rs:412-415` | divides the 10th percentile to give a Gaussian sigma | derived (`Phi^-1(0.55)`, the half-normal's 10% point) |
| `MAD_TO_SIGMA` | 0.6745 (test-only) | `inkvec-trace/src/coverage.rs:308-310` | the old median-to-sigma conversion, kept for the regression test | derived (standard statistical constant) |
| `NOISE_FLOOR` | 0.5/255 | `inkvec-trace/src/coverage.rs:312-317` | minimum `estimate_noise` can return | derived (half an 8-bit quantisation step; callers divide by it; test `coverage.rs:1242-1249`) |
| short-input noise | 1/255 | `inkvec-trace/src/coverage.rs:365-367` | what `estimate_noise` returns under 3x3 or for a short buffer | none |
| `confidence_penalty` floor | `saturation.max(0.05)` | `inkvec-trace/src/coverage.rs:536` | prevents unbounded penalty near-zero saturation | none |
| `confidence_penalty` cap | 8.0 | `inkvec-trace/src/coverage.rs:536` | ceiling on sigma inflation from low saturation | none |
| `EDGE_FLOOR` (`ringing_score`) | 24/255 | `inkvec-trace/src/coverage.rs:628-629` | gradient above which a pixel is an edge the ring is measured around | motivated |
| `CORE_D` / `RING_IN` / `RING_OUT` | 1 / 3 / 7 px (chamfer units 3 / 9 / 21) | `inkvec-trace/src/coverage.rs:630-634` | the core band (anti-aliasing) and the ring band (ringing) of `ringing_score` | motivated (ringing comes from an 8x8 DCT block and does not scale, `inkvec-trace/src/color.rs:436-440`) |
| `MIN_SAMPLES` (`ringing_score`) | 64 | `inkvec-trace/src/coverage.rs:635-636` | fewest core, ring or hot-pair samples for a non-zero score | motivated |
| `EDGE_FLOOR` (`intake_scale`) | 2/255 | `inkvec-trace/src/coverage.rs:833-834` | minimum first difference counted as a real edge | motivated |
| `MAX_W` | 64.0 | `inkvec-trace/src/coverage.rs:835-837` | clamp on a single edge-width observation | motivated |
| `MIN_W` | 0.25 | `inkvec-trace/src/coverage.rs:838` | lower clamp on a single edge-width observation | none |
| minimum edge observations | 16 | `inkvec-trace/src/coverage.rs:876-878` | below this, `intake_scale` returns 1.0 | none |
| `OVERSAMPLE_TOL` | 3.0 (8-bit levels) | `inkvec-trace/src/coverage/oversample.rs:10-24` | absolute round-trip error threshold for `oversample_factor` | measured; only safe downstream of `intake_scale`, never as a gate alone |
| `OVERSAMPLE_KEEP` | 0.5 (of `flat_error`) | `inkvec-trace/src/coverage/oversample.rs:26-53` | largest share of the image's detail a round trip may lose and still count as lossless (relative test, so a lone small shape on an empty canvas is not read as 8x) | measured (margins, 2026-10-02: corpus icons at 512/1024 px at most 0.31/0.17, the 38 px² disc 0.36/0.77/1.60 at /2, /4, /8; the half is a choice between them) |
| min. size for `oversample_factor` | 16x16, `sw`/`sh >= 8` | `inkvec-trace/src/coverage/oversample.rs:119-128` | avoids measuring on too little data | none |

## 03 — Palette ([03-palette.md](03-palette.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `SAME_INK_DE00` | 1.5 (CIEDE2000) | `inkvec-trace/src/color.rs:251-268` | perceptual floor: colours this close are one ink | measured (swept 1.0 -> 0.4140, 1.5 -> 0.4124 on the screen set) |
| `SOFT_SAME_INK_DE00` | 5.0 | `inkvec-trace/src/color.rs:493-505` | same-ink floor on soft/oversampled intake | measured (a real brand mark upscaled 4x: 76 fills where the drawing has five) |
| `SOFT_NOISE_SIGMAS` | 3.0 | `inkvec-trace/src/color.rs:409-417` | noise-merge threshold, gated on soft-intake evidence only | measured (costs 10.9% objective if run unconditionally) |
| `NOISE_SIGMAS` | 0.0 | `inkvec-trace/src/color.rs:663-680` | clean-intake noise-merge threshold, deliberately off; read at `inkvec-trace/src/lib.rs:357-361` and `inkvec-trace/src/native.rs:837-841` | measured (costs 0.4328 -> 0.4451 on the screen set if always on) |
| `SOFT_INTAKE_EDGE` | 1.75 px | `inkvec-trace/src/color.rs:398-407` | edge width above which the intake is soft | measured (980-raster corpus edge-width survey: native max 1.50) |
| `SOFT_RINGING` | 0.12 | `inkvec-trace/src/color.rs:419-434` | ringing score above which the intake is soft (images under `RINGING_MIN_DIM`) | measured (240 clean rasters at 128ss: max 0.1023, highest real artwork 0.0370) |
| `SOFT_RINGING_LARGE` | 0.05 | `inkvec-trace/src/color.rs:436-449` | the same gate when both sides are at least `RINGING_MIN_DIM` | measured (at 512 px: zero false positives, 88-89% of JPEG caught at q85/60/40) |
| `RINGING_MIN_DIM` | 256 px | `inkvec-trace/src/color.rs:451-452` | smallest side at which `SOFT_RINGING_LARGE` applies | motivated (the fixed 3-7 px ring isolates one boundary only on larger images; screen set bit-identical below it) |
| `MEASURED_SIGMA_SCALE` | 1.0 | `inkvec-trace/src/color.rs:454-463` | share of `regularize::residual_sigma` believed when the noise is raised after labelling on a soft intake | measured (78 JPEG-re-encoded-as-PNG traces) |
| `MEASURED_SIGMA_CAP` | 8.0 levels | `inkvec-trace/src/color.rs:465-491` | ceiling on that measured noise (non-binding: `residual_sigma` clamps itself to 8) | measured (791 traces across all classes) |
| `DEFAULT_MERGE_DISTANCE` | 0.035 (OKLab) | `inkvec-trace/src/color.rs:175-200` | palette merge radius | measured (swept 0.055/0.040/0.035/0.030 on the full set; 0.035 best) |
| `MIN_INK_WEIGHT` | 0.004 | `inkvec-trace/src/color.rs:202-208` | minimum claimed share of the image to count as ink; the first ink is exempt (`color/mdl.rs:214`), and on the transparent-image walk also the first ink that draws something (`native/palette.rs:227-261`) | motivated |
| `BLEND_INTERIOR_FRACTION` | 0.25 | `inkvec-trace/src/color.rs:210-225` | interior-fraction threshold, anti-aliasing vs. ink | measured (swept against conflicting optima on two corpora) |
| `BLEND_STRADDLE_FRACTION` | 0.5 | `inkvec-trace/src/color.rs:227-238` | straddle-fraction threshold | motivated |
| `STRADDLE_STEP` | 0.12 | `inkvec-trace/src/color.rs:239-242` | how far along the A-B axis a neighbour must sit to count as the far side (clipped to half the room left, floor 0.02, `color/mdl.rs:380-383`) | motivated |
| `BLEND_TMIN` | 0.04 | `inkvec-trace/src/color.rs:507-509` | interior-mixture band on the A-B colour axis (was the env variable `INKVEC_BLEND_TMIN`) | none |
| blend chord tolerance | 1.6 x `merge_distance` | `inkvec-trace/src/color/mdl.rs:299` | how far (OKLab) a candidate may sit from the chord between two inks and still be a blend of them (same in `native/palette.rs:379`) | none |
| `JND_FLOOR` | 0.012 (OKLab) | `inkvec-trace/src/color.rs:657-661` | below this, two colours are never treated as separate inks (gates the MDL escape, `color/mdl.rs:238`) | none |
| `PARAMS_PER_INK` | 3.0 | `inkvec-trace/src/color.rs:244-245` | one parameter per OKLab channel | derived |
| `BINS` | 24 per OKLab axis | `inkvec-trace/src/color/mdl.rs:394-395` | the candidate grid of `frequency_modes` | none |
| `STAT_PIXELS` | 65536 | `inkvec-trace/src/color.rs:702-715` | cap on per-candidate statistical pass cost (stride `color.rs:726-743`) | derived (matches the 128px tuning point) |
| `SPREAD_SAMPLES` | 8192 | `inkvec-trace/src/color/distinct.rs:196-198` | most pixels the spread's median is taken over | motivated |
| `CLEAR_INK_ALPHA` | 0.02 | `inkvec-trace/src/native.rs:374-375` | opacity at or below which a native ink is the clear ground: not counted against `max_colors`, does not use up the rarity exemption | none |
| `LEVEL_GAP` / `LEVEL_SPREAD` / `MIN_SHARE` / `CLEAR` (`split_alpha_inks`) | 0.15 / 0.06 / 0.02 / 0.05 | `inkvec-trace/src/color.rs:908-914` | how opacity levels of one ink are cut, kept and snapped to clear (inks with at least 16 pixels, `color.rs:930`) | motivated |

## 04 — Regions ([04-regions.md](04-regions.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `min_region` shipped default | 2 px | `inkvec-cli/src/args.rs:226`, `inkvec-cli/src/pipeline.rs:173`, `inkvec-trace/src/lib.rs:205` | smallest component kept by despeckle; `min_region.max(2)` is the carve's minimum feature size (`inkvec-trace/src/lib.rs:662`); scaled by the oversampling factor squared above `REF_EXTENT` (`inkvec-cli/src/lib.rs:328-342`) | measured (a 9 px colour-mode floor was tried and measured worse, `inkvec-cli/src/pipeline.rs:166-171`) |
| `SADDLE_SIGMAS` | 3.0 | `inkvec-trace/src/regions.rs:40-44` | sigma a corner's coverage must clear 0.5 by before a saddle resolves (research build only) | motivated (a standard "three sigma" bar, not swept) |
| `MAX_FACES` | 65,535 (`u16::MAX`) | `inkvec-trace/src/regions.rs:205-208` | most faces a face map can number; past it `cap_components` merges the smallest components into their neighbours (`inkvec-trace/src/regions.rs:293-416`) | derived (`u16` face ids, `u16::MAX` reserved for the outside of the image) |
| absorption rounds | 2 | `inkvec-trace/src/regions.rs:768-769, 792` | rounds of whole-sliver absorption | motivated (one dissolved sliver can leave a neighbour thinner) |
| absorption thinness gate | interior < area/5 | `inkvec-trace/src/regions.rs:678-679, 704` | candidate slivers for blend absorption | motivated |
| absorption dominant-neighbour gate | top 3 neighbours >= 4/5 of foreign contacts | `inkvec-trace/src/regions.rs:680-683, 717-719` | which components are candidate slivers | motivated |
| absorption pass threshold | >= 4/5 pixels within tolerance | `inkvec-trace/src/regions.rs:684-685, 742` | whether a qualifying component is absorbed | motivated |
| translucency gate | alpha < 0.99 | `inkvec-trace/src/regions.rs:736, 1002` | when the white backdrop joins the candidate inks | none |
| `tol` (both blend passes) | `max(3*sigma_noise, 0.025)` sRGB | `inkvec-trace/src/regions.rs:764-766, 787, 959` | how close a pixel must be to a mixture hypothesis | motivated (the floor keeps near-zero sigma on clean synthetic input from rejecting blends over float rounding; 0.025 itself not derived) |
| `reassign_blend_pixels::ROUNDS` | 4 | `inkvec-trace/src/regions.rs:958` | rounds of per-pixel reassignment | none |
| reassign "beats own label" margin | residual < 0.5 * distance to own ink | `inkvec-trace/src/regions.rs:940-941, 1018` | how much better a blend hypothesis must be to move a pixel | motivated |

## 05 — Gradients ([05-gradients.md](05-gradients.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `PARAMS_FLAT` | 3.0 | `inkvec-trace/src/gradient.rs:76-77` | flat-fill description length | derived |
| `PARAMS_LINEAR` | 10.0 | `inkvec-trace/src/gradient.rs:78-79` | linear-gradient description length | derived |
| `PARAMS_RADIAL` | 9.0 | `inkvec-trace/src/gradient.rs:80-81` | circular-radial description length | derived |
| `PARAMS_RADIAL_ELLIPTIC` | 11.0 | `inkvec-trace/src/gradient.rs:82-83` | elliptical-radial description length | derived |
| `PARAMS_STOP` | 4.0 | `inkvec-trace/src/gradient.rs:84-85` | cost of each interior stop | derived |
| `MAX_MID_STOPS` | 2 | `inkvec-trace/src/gradient.rs:86-88` | most interior stops fitted | measured (corpus stop-count survey) |
| `MIN_GRADIENT_PIXELS` | 16 | `inkvec-trace/src/gradient.rs:90-91` | fewest interior samples before a gradient is attempted; interior a flat component needs to keep its own colour in the merge write-back | motivated |
| `BIMODAL_MARGIN` | 0.85 | `inkvec-trace/src/gradient.rs:92-96` | ramp-vs-step decision threshold; a constant (`inkvec-trace/src/gradient.rs:916`), the `INKVEC_BIMODAL` override removed | motivated |
| `MIN_VISIBLE_CONTRAST` | 1.5/255 | `inkvec-trace/src/gradient.rs:98-100` | floor on visible contrast for any gradient candidate | motivated |
| `MIN_RAMP_SUPPORT` | 0.10 | `inkvec-trace/src/gradient.rs:101-107` | least fraction of samples a gradient must visibly shade | motivated (concrete regressions, value not derived) |
| `QUANT_HALF_STEP` | 0.5/255 | `inkvec-trace/src/gradient.rs:109-110` | residual dead zone from 8-bit quantisation | derived |
| `MIN_SHARED_BOUNDARY` | 3 | `inkvec-trace/src/gradient/bands.rs:22-23` | fewest seam pixel pairs before a union is considered | none |
| `MERGE_WORK_FLOOR` | 2^28 (268,435,456) units | `inkvec-trace/src/gradient/bands.rs:69-71` | least work cap any image's band merge gets | measured (11x the gate's largest spend, 23.2 M; above all 772 non-pathological stress images, largest 191 M; `inkvec-trace/src/gradient/bands.rs:40-53`) |
| `MERGE_WORK_PER_PIXEL` | 32 units/px | `inkvec-trace/src/gradient/bands.rs:25-67` | band-merge work cap past about 2900 x 2900 px: `max(MERGE_WORK_FLOOR, 32 * w * h)` | measured (twice the masthead's 16 units/px) |
| `MODEL_WORK_PER_SAMPLE` | 13 | `inkvec-trace/src/gradient/bands.rs:73-95` | gathered pixels one scored sample of a union fit is charged as (`union_work`) | measured (0.25 us/pixel gather vs 13.4 ms full-sample model selection, 2,243 union fits on `brands/sangchaimeter`) |
| `MAX_FIT_SAMPLES` | 4096 | `inkvec-trace/src/gradient/budget.rs:29-31` | most samples one fit scores | none |
| `FIT_PIXELS_CAP` | 65536 | `inkvec-trace/src/gradient/budget.rs:32-42` | pixels one fit gathers before it is sampled down | measured (identical output to fitting every pixel on two profiling logos at 1024 and 2048 px, `inkvec-trace/src/gradient/bands.rs:610-616`) |
| `CENTRE_SEARCH_SAMPLES` | 1024 | `inkvec-trace/src/gradient/budget.rs:43-45` | samples a radial or elliptical centre search evaluates per candidate centre | none |
| `MAX_ASPECT` | 8.0 | `inkvec-trace/src/gradient/fit.rs:521-523` | largest aspect of an elliptical gradient | motivated |
| `RAMP_STEP_DE00` | 15.0 (CIEDE2000) | `inkvec-trace/src/gradient/regions.rs:19-21` | largest ink difference for two flat bands to be tried as one ramp (region recovery) | none |
| `SMOOTH_STEP` | 3.0 (CIE76, Lab) | `inkvec-trace/src/gradient/regions.rs:23-29` | largest pixel-pair step inside one smooth region | motivated |
| `SMOOTH_FRACTION` | 0.5 | `inkvec-trace/src/gradient/regions.rs:31-33` | fraction of a seam's pixel pairs that must be smooth steps | none |
| `CARVE_RESIDUAL` | 0.06 | `inkvec-trace/src/gradient/carve.rs:16-20` | floor on the carve candidate threshold `max(8*sigma_noise, 0.06)` | motivated |
| `CARVE_MAX` | 64 | `inkvec-trace/src/gradient/carve.rs:21-22` | most features carved from one image | motivated |
| `CARVED_PAINT_ALPHA` | 0.5 | `inkvec-trace/src/native.rs:689-698` | mean opacity at or above which a carved feature named by the clear ink is renamed to the nearest visible ink (native-alpha path, `name_carved_paint`) | measured (residue 0.14-0.36, painting it cost dE00 on 5 of 8 icons; paint 0.80-1.00) |
| `bic_lambda(n)` | 0.5 * ln(n) | `inkvec-trace/src/gradient.rs:320-327` | fill-selection lambda | derived (Bayesian information criterion) |
| `IRLS_ROUNDS` | 2 | `inkvec-trace/src/gradient/stops.rs:68-69` | Huber reweighting rounds when a stop profile is fitted | none |
| `MAX_SLIVER_MISFIT` | 4.0 | `inkvec-trace/src/gradient/stops.rs:71-73` (test at `inkvec-trace/src/gradient/stops.rs:432-482`) | rejects a new interior stop when the smaller side of its segment holds under 1/10 of the subsamples *and* its median residual exceeds 4x the other side's (floored at 1/255) | motivated; reachable from both the band merger and `fit_fill` via `fit_pixels` -> `fit_samples` -> `fit_mid_stops`; no stated derivation for 4.0 itself |

## 06 — Planar map ([06-planar-map.md](06-planar-map.md))

`build` itself has no free numeric constants — the topology decision is exact integer
equality, not a threshold. The one constant this stage's tests exercise indirectly belongs
to the upstream saddle merge; the two sizes after it choose only speed, never the map:

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `SADDLE_SIGMAS` (research build) | 3.0 | `inkvec-trace/src/regions.rs:44` | see 04-regions.md; reused unmodified here | motivated |
| `DIGIT` (`radix_sort_by_node`) | 11 bits (2,048 buckets) | `inkvec-trace/src/planar/cracks.rs:185` | digit width of the incidence radix sort; three passes at 2048 px | motivated (16 KiB of counters stay in L1 cache) |
| `LANES` (`RowRuns::new`) | 16 labels | `inkvec-trace/src/planar/runs.rs:84` | how many labels a run is extended by at once | motivated (two 128-bit compares on x86-64) |

## 07 — Sub-pixel ([07-subpixel.md](07-subpixel.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `MIN_UNMIX_CONTRAST` | 0.02 | `inkvec-trace/src/planar.rs:414` | floor on unmixing contrast below which a point does not move | none |
| `CORNER_COS` | 0.5 (60 deg) | `inkvec-trace/src/planar.rs:424` | turning angle above which the tangent window narrows to 1 point | motivated (geometric bound) |
| `PAR_VERTICES` | 64 | `inkvec-trace/src/planar.rs:506` | fewest points before an edge's vertices are refined in parallel; smallest chunk one thread takes | motivated (a task of ~30 µs at 0.44 µs per vertex against rayon's few-µs split cost); schedule only, output identical |
| `PAR_MAP_VERTICES` | 512 | `inkvec-trace/src/planar.rs:520` | fewest boundary vertices in the map for the refinement, and symmetry detection beside it, to use threads | measured (per-icon serial/parallel timings by vertex count, `planar.rs:512-519`); schedule only, output identical |
| `SUBPX_WIN` (was `INKVEC_SUBPX_WIN`, *removed*) | 1 (the variable allowed 1..=8) | `inkvec-trace/src/planar.rs:416-419` (measurement at `:854-864`) | width of the tangent-estimation window | measured (two points each side improved dE00 on a 620-icon subset but cost DISTS and caused a face-order regression on one icon; left at 1, LOG-43) |
| `DEFAULT_SIGMA_MODEL` | 0.05 px | `inkvec-trace/src/coverage.rs:81` | see 02-coverage.md | measured |
| `CONTRAST_REF` | 0.25 | `inkvec-trace/src/planar.rs:1246` | reference contrast for `simplify_faint`'s inflation | none |
| `MAX_INFLATION` | 4.0 | `inkvec-trace/src/planar.rs:1247` | cap on `simplify_faint`'s sigma multiplier | none |
| `JUNCTION_MAX_MOVE` | 1.5 px | `inkvec-trace/src/planar/junctions.rs:73` | rejects an intersection solution beyond this move | none (the doc comment says only what it bounds; the 0.5 px trial 07-subpixel.md cites is not in the code) |
| `JUNCTION_MIN_CONDITION` | 0.02 | `inkvec-trace/src/planar/junctions.rs:78` | minimum eigenvalue ratio admitted for a junction intersection | derived (equivalent to rejecting crossings below ~16 degrees) |
| `junction_fit_points()` | 6 | `inkvec-trace/src/planar/junctions.rs:36` | interior points for near-junction extrapolation | none |
| `junction_skip()` | 1 | `inkvec-trace/src/planar/junctions.rs:43` | points nearest the junction excluded from the tangent fit | motivated |
| `junction_curvature_points()` | 16 | `inkvec-trace/src/planar/junctions.rs:50` | points used to test for significant curvature | none |
| `MIN_QUADRATIC_POINTS` | 5 | `inkvec-trace/src/planar/junctions.rs:215` | minimum points before a quadratic tangent model is tried | none |
| `CURVATURE_SIGNIFICANCE` | 3.0 | `inkvec-trace/src/planar/junctions.rs:216` | sigma threshold for preferring a quadratic tangent | motivated ("3-sigma" convention) |
| `TAPER_DEGREES` | 55.0 | `inkvec-trace/src/planar/junctions.rs:468` | branch angle admitted for taper testing | motivated (deliberately loose; see 07-subpixel.md) |
| `TAPER_MAX_MOVE` | 8.0 px | `inkvec-trace/src/planar/junctions.rs:470` | largest move a taper estimate may make | motivated |
| `TAPER_MAX_CONSUMED` | 0.35 | `inkvec-trace/src/planar/junctions.rs:472` | fraction of the shortest incident boundary a taper move may consume | none (the doc comment says what it bounds, not why 0.35) |
| `TAPER_MAX_SIGMA` | 1.0 px | `inkvec-trace/src/planar/junctions.rs:474` | largest standard error a taper estimate may carry | motivated |
| `TRIM_MIN_POINTS` | 4 | `inkvec-trace/src/planar/junctions.rs:402` | fewest points an edge keeps after trimming | motivated |
| `TRIM_LOOK` | 3 | `inkvec-trace/src/planar/junctions.rs:405` | how far ahead to look when deciding an edge's direction | motivated |
| `MIN_WIDTH` / `MAX_WIDTH` | 0.05 / 6.0 px | `inkvec-trace/src/taper.rs:104,108` | taper sample admission band | motivated |
| `MIN_SAMPLES` | 4 | `inkvec-trace/src/taper.rs:111` | fewest taper samples trusted | none |
| `MAX_DEFECT` | 0.15 px | `inkvec-trace/src/taper.rs:121` | largest tangent-circle residual admitted | measured (calibrated against three worked cases) |

## 08 — Boundary solve ([08-boundary-solve.md](08-boundary-solve.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `MAX_STEP` | 0.35 px | `inkvec-trace/src/boundary_opt.rs:110` | largest displacement of any point in one L-BFGS step | none |
| `MAX_TOTAL` | 1.0 px | `inkvec-trace/src/boundary_opt.rs:114` | total leash from the point's starting position; also why the band never moves | motivated |
| `K_KINK` | 0.05 | `inkvec-trace/src/boundary_opt.rs:121` | kink weight, fraction of the data term's initial value | motivated (scaling rule derived, value not swept) |
| `K_ANCHOR` | 0.10 | `inkvec-trace/src/boundary_opt.rs:124` | anchor weight: a point 1 px from its start costs this fraction of the average point's share of the data term's initial value | motivated (scaling rule derived, value not swept) |
| `JUNCTION_ANCHOR` | 4.0 | `inkvec-trace/src/boundary_opt.rs:126` | multiplier on `w_anchor` at a junction point | none (its doc comment gives the reason, `planar::refine_junctions` has already placed the point, but not why fourfold) |
| `MIN_CONTRAST` | 2.0/255 | `inkvec-trace/src/boundary_opt.rs:128` | colour or opacity difference that counts as a boundary when choosing where alpha is a fourth channel | none |
| `GRID_LIMIT` | 1e9 px | `inkvec-trace/src/boundary_opt.rs:222` | largest coordinate whose gridlines `crossings` walks; a segment beyond it contributes no crossings | motivated (far inside the range where `m += 1.0` is exact, far outside any image) |
| `GRID_MAX_SPAN` | 2^20 gridlines | `inkvec-trace/src/boundary_opt.rs:226` | most gridlines `crossings` walks along one axis of one segment | motivated (a segment inside the image crosses at most its width or height) |
| `EPS` (in `priors`) | 1e-4 | `inkvec-trace/src/boundary_opt.rs:440` | floor inside the kink term's square root | motivated |
| fold-guard floor | `s > 0.1` | `inkvec-trace/src/boundary_opt.rs:662` | how far the guard halves the displacement before giving up (scales 1, 1/2, 1/4, 1/8, 1/16) | none |
| `REACH` | 1 px | `inkvec-trace/src/boundary_opt/band.rs:71` | band width (Chebyshev) round the pixels the starting boundary crosses | derived (follows from `MAX_TOTAL`) |
| `PARALLEL_CELLS` | 16384 band pixels | `inkvec-trace/src/boundary_opt/band.rs:151` | below it the band runs are evaluated on one thread (the result is the same either way) | none |
| `TABLE_BUDGET_FLOOR` | 256 MiB | `inkvec-trace/src/boundary_opt/band.rs:852` | floor of the band-table budget `max(TABLE_BUDGET_FLOOR, TABLE_BUDGET_PER_PIXEL · w · h)`; past the budget the boundary solve is skipped and the measured boundary kept | measured (11x the gate's largest band table, 22.2 MB on the 2048 px masthead; 2.4x the largest of 772 stress images, 106 MB; `INKVEC_DIAG`, 2026-10-02) |
| `TABLE_BUDGET_PER_PIXEL` | 32 bytes a pixel | `inkvec-trace/src/boundary_opt/band.rs:856` | growth of the band-table budget above its floor (binds above about 2900 x 2900 px) | motivated (lets an uncapped 8192 px trace keep twice the masthead's 14 bytes a pixel) |
| frame snap | 1e-3 px | `inkvec-trace/src/boundary_opt/band.rs:1196` | how close to a frame line a point of a frame edge is snapped onto it and pinned | none |
| `MEMORY` | 3 | `inkvec-trace/src/boundary_opt/lbfgs.rs:41` | L-BFGS pairs kept | measured (3 did as well as 7, 15 or 30) |
| `C1` | 1e-4 | `inkvec-trace/src/boundary_opt/lbfgs.rs:43` | Armijo sufficient-decrease constant | motivated (the textbook value, Nocedal & Wright 2006) |
| `MAX_TRIALS` | 8 | `inkvec-trace/src/boundary_opt/lbfgs.rs:45` | step halvings per line search | none |
| `MAX_ITERS` | 32 | `inkvec-trace/src/boundary_opt/lbfgs.rs:49` | iterations per independent part (the former `INKVEC_BOPT_ITERS`, *removed*) | measured (24 read 0.3666, 32 read 0.3585, 48 read 0.3558 on the screen set; 48 up to 1.9x slower) |
| `PARAM_TOL` | 0.005 px | `inkvec-trace/src/boundary_opt/lbfgs.rs:52` | stop once no point moves more in a step | derived (half the SVG's 0.01 px) |
| `FUNC_TOL` | 1e-4 | `inkvec-trace/src/boundary_opt/lbfgs.rs:55` | stop once a step lowers the changeable energy by less than this fraction | none |
| `COARSE` | 4 px | `inkvec-trace/src/boundary_opt/folds.rs:33` | side of the fold guard's coarse join grid | motivated (about the length of a swept segment's range) |
| `MAX_COARSE_CELLS` | 64 | `inkvec-trace/src/boundary_opt/folds.rs:38` | a swept range over more coarse cells is paired with every segment directly instead of entering the grid | motivated (the planar map never has one; keeps a degenerate segment from filling the grid) |
| `MAX_RANGE_AREA` (segment bucket limit) | cell range `(x1-x0)*(y1-y0) <= 64` | `inkvec-trace/src/boundary_opt/folds.rs:42` | segments the fold guard counts (larger ones are never counted) | motivated (kept exactly from the hash grid it replaced) |

## 09 — Decode ([09-decode.md](09-decode.md))

The `decode` module is compiled only in a research build (`inkvec-trace/src/lib.rs:80-81`).

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `LEAK_GATE` | 0.05 | `inkvec-trace/src/decode.rs:62` | diagnostic threshold on `leak` (not gated on) | none |
| `MIN_VERTS` / `MAX_VERTS` | 3 / 16 | `inkvec-trace/src/decode.rs:64-65` | vertex-count range a candidate order may propose | none |
| `MAX_DEV` | 2.0 px | `inkvec-trace/src/decode.rs:67` | worst-case chord deviation before a face is "something else" | none |
| `CURVE_BIAS_PX` | 0.35 px | `inkvec-trace/src/decode.rs:69` | mean-offset threshold in `is_polygonal`'s curve test | none |
| `MIN_GAIN` | 0.5 | `inkvec-trace/src/decode.rs:76` | residual-cut fraction of `sse0` required for a decode | measured (screen set: 10 worse/7 better at 1.0, better on every axis at 0.5) |
| `EVIDENCE_OVERRIDE` | 0.0 (off) | `inkvec-trace/src/decode.rs:87` | residual-ratio threshold to skip the post-solve shape recheck | measured trade-off (0.5 fixes one case, costs 0.6% objective; default declines the trade) |
| `CURVE_MIN_SAMPLES` | 8 | `inkvec-trace/src/decode.rs:89` | fewest ring samples before the curve test applies | none |
| `THIN_PX` | 2.5 px | `inkvec-trace/src/decode.rs:95` | width above which a face is not attempted | measured (conditioning cliff location) |
| `PARAMS_PER_RIBBON` | 10.0 | `inkvec-trace/src/decode.rs:97` | parameter charge for one pooled ribbon | derived |
| `SHARE_MAX_PX` | 1.75 px | `inkvec-trace/src/decode.rs:100` | widest ribbon `share_widths` will pool | motivated |
| `MAX_RING_POINTS` | 512 | `inkvec-trace/src/decode.rs:102` | largest ring `ring_of` will accept | motivated |
| `PIXELS_PER_UNKNOWN` | 4 | `inkvec-trace/src/decode.rs:112` | boundary-cut band pixels required per free coordinate | motivated |
| `MAX_BBOX_PIXELS` | 20,000 | `inkvec-trace/src/decode.rs:113` | largest face bounding box attempted | none |
| `GN_ITERS` | 14 | `inkvec-trace/src/decode.rs:118` | Gauss-Newton iteration cap | none |
| `FD_STEP` | 0.01 px | `inkvec-trace/src/decode.rs:119` | finite-difference step for the coverage derivative | none |
| `MAX_STEP` | 0.35 px | `inkvec-trace/src/decode.rs:120` | per-iteration vertex step clamp; shared value with `boundary_opt::MAX_STEP` | motivated |
| `MAX_TOTAL` | 1.0 px | `inkvec-trace/src/decode.rs:129` | cumulative leash from a vertex's starting position | measured (uncapped: one emoji frame drifted to a 13-sided polygon) |
| `DECODED_SIGMA` | 0.05 px | `inkvec-trace/src/decode.rs:1261` | sigma given to written-back samples | motivated (matches `coverage::DEFAULT_SIGMA_MODEL`) |
| `SAMPLE_PX` | 1.0 px | `inkvec-trace/src/decode.rs:1262` | spacing of written-back samples along a decoded edge | motivated |
| GN damping schedule | mu0 1e-3, x4/reject, /3/accept, floor 1e-7, cap 1e9 | `inkvec-trace/src/decode.rs:1066,1181,1185-1186` | Levenberg step damping | motivated (standard schedule shape) |
| ridge in `varpro` | `1e-6 * trace / k` | `inkvec-trace/src/decode.rs:402` | regularises a fill column with no pixel support | motivated |
| turning-corner angle floor | 0.4 rad | `inkvec-trace/src/decode.rs:251` | minimum turning angle counted as a corner | none |

## 10 — Symmetry ([10-symmetry.md](10-symmetry.md))

No numeric thresholds. Every test is exact equality on integer pixel coordinates or exact
half-integer lattice points (`inkvec-trace/src/symmetry.rs:19-21,176-186,220,243,249,272`)
— a deliberate design choice, not an omission.

## 11 — Curve fitting ([11-fitting.md](11-fitting.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `PARAMS_LINE` | 2.0 | `inkvec-fit/src/lib.rs:119` | line parameter cost | derived |
| `PRUNE_SLACK` | 4.0 | `inkvec-fit/src/lib.rs:127` (shared by `inkvec-fit/src/multimodel.rs:139`) | safety factor on the scan cut-off | motivated |
| `CORNER_CHAMFER` | 1.0 px | `inkvec-fit/src/lib.rs:585` | corner-adjustment chamfer allowance | derived (one level-set sampling step) |
| `CORNER_TURN_MIN` | pi/6 (30 deg) | `inkvec-fit/src/lib.rs:588` | when a vertex meeting is treated as a corner | none |
| `CORNER_DEGREES` | 45.0 | `inkvec-fit/src/lib.rs:921` | corner-vs-smooth-join threshold of `fit_path` | none |
| max-shift factor | 3.0x max(sigma), floor 0.25 | `inkvec-fit/src/multimodel.rs:991` | corner intersection displacement cap | motivated |
| `PARAMS_CUBIC` | 6.0 | `inkvec-fit/src/multimodel.rs:131` | default cubic parameter cost (`--bezier-cost` reprices it per trace, `cost.rs`) | derived |
| `PRUNE_PATIENCE` | 8 | `inkvec-fit/src/multimodel.rs:147` | consecutive over-budget spans before the scan stops | measured, value not stated |
| `DP_MAX_POINTS` | 768 | `inkvec-fit/src/multimodel.rs:152` | decimation threshold | none |
| `DP_PAR_MIN_POINTS` | 128 | `inkvec-fit/src/multimodel/scan.rs:78` | shortest polyline whose scan is shared between threads | motivated (speed only) |
| `G1_BREAK_DEGREES` | 10.0 | `inkvec-fit/src/tangents.rs:23` | tangent break below which a join is nearly free (`--corner-angle` overrides) | none; swept empirically |
| `TANGENT_WINDOW_MAX` | 16 | `inkvec-fit/src/tangents.rs:26` | widest one-sided tangent window | none |
| `MAX_RESIDUAL_SAMPLES` | 32 | `inkvec-fit/src/candidates.rs:42` | cubic residual evaluation points (O(1) cap) | none |
| `NEWTON_STEPS` | 3 | `inkvec-fit/src/candidates.rs:45` | Newton steps for point-to-cubic projection | none |
| `MAX_ARM` | 1.0 | `inkvec-fit/src/candidates.rs:48` (used by `inkvec-fit/src/merge.rs:132`) | largest admissible control arm, as a fraction of chord | derived |
| `FREE_MAX_SWING` | 75.0 deg | `inkvec-fit/src/candidates.rs:52` | how far a free cubic's tangent may depart from the estimate | none |
| `DIRECTION_SAMPLES` | 8 | `inkvec-fit/src/candidates.rs:1137` | monotone-sweep samples for arc validity | none (asserted) |
| `MAX_ASPECT` | 12.0 | `inkvec-fit/src/candidates.rs:1298` | most elongated ellipse worth fitting | none |
| `MIN_POINTS` (ellipse) | 24 | `inkvec-fit/src/candidates.rs:1299` | minimum points to try an ellipse | none |
| `LENGTH_STRIDE` | 16 | `inkvec-fit/src/candidates.rs:1300` | ellipse candidate-length sampling | motivated (keeps the O(n) fit to a sparse grid) |
| bow-penalty factor | 4.0 | `inkvec-fit/src/candidates.rs:921` | arc-vs-line residual ratio that triggers the bow penalty | none |
| `PARAMS_ARC` | 5.0 | `inkvec-fit/src/curves.rs:194` | circular arc cost (default prices) | derived |
| `PARAMS_ARC_WRITTEN` | 7.0 | `inkvec-fit/src/curves.rs:218` | circular arc cost under `--arcs-as-written` | the numbers SVG writes |
| `CAP_TURN_DEGREES` | 90.0 | `inkvec-fit/src/candidates/turn.rs:42` | turn past which one cubic is charged as two, under `--arcs-as-written` | derived (Goldapp 1991: a cubic's circle error grows as the sweep to the sixth); 90 not swept |
| `WRITTEN_ARCS_LAMBDA_SCALE` | 0.8 | `inkvec-cli/src/lib.rs:494` | λ multiplier under `--arcs-as-written` | measured (swept 0.6/0.7/0.8/1.0 on gate v2) |
| `PARAMS_ELLIPTICAL_ARC` | 7.0 | `inkvec-fit/src/curves.rs:225` | elliptical arc cost | derived |
| `MAX_ARC_DEGREES` | 120.0 | `inkvec-fit/src/primitives.rs:60` | longest single-arc sweep | derived (conditioning argument) |
| `MAX_REDUCED_CHI2` | 4.0 | `inkvec-fit/src/primitives.rs:416` | primitive acceptance gate | derived (tau^2 at default tau=2) |
| `PARAMS_CIRCLE` / `PARAMS_ELLIPSE` / `PARAMS_ROUND_RECT` / `PARAMS_RECT` | 3.0 / 5.0 / 6.0 / 4.0 | `inkvec-fit/src/primitives.rs:42-49` | primitive parameter costs | derived |
| cost-floor slack | `1e-9·(tr S + Σw·C²)` | `inkvec-fit/src/choice.rs:187` | absolute shrink of the single-line chi² floor that lets the image frame skip its dynamic program | derived (dwarfs the rounding of the scatter and of the sampled chi² below 2^20 points) |
| `BREAK_PARAMS` | 2.0 | `inkvec-fit/src/merge.rs:88` | joins a free cubic no longer meets smoothly (`INKVEC_MERGE_BREAK` *removed*) | derived |
| `MAX_SPAN` | 96 | `inkvec-fit/src/merge.rs:92` | longest merge run attempted, in measured points | motivated (bounds the pass at O(n · span)) |
| `MAX_RUN` | 4 | `inkvec-fit/src/merge.rs:99` | segments a merge run may absorb | none (was overridable, never swept) |
| `MAX_ROUNDS` | 6 | `inkvec-fit/src/merge.rs:109` | merge sweeps | motivated |
| `SMOOTH_SLACK` | 0.0 | `inkvec-fit/src/merge.rs:115` | parameters a merged curve may lose by and still be taken | measured (the `INKVEC_SMOOTH` experiment never moved the default) |
| `SAMPLES` | 96 | `inkvec-fit/src/merge.rs:118` | curve samples of the residual that decides a merge | none |
| `SEARCH_DEGREES` | 100.0 | `inkvec-fit/src/merge.rs:126` | free-cubic rotation search width | motivated (a 60-degree clamp put the optimum outside the search) |
| `COARSE_SAMPLES` | 24 | `inkvec-fit/src/merge.rs:129` | curve samples while ranking the coarse grid | none |
| `ANGLES` | ±90, ±65, ±45, ±22, 0 deg | `inkvec-fit/src/merge/grid.rs:83` | the free-cubic grid's rotations of each end direction | none |
| `ARMS` | 0.15, 0.3, 0.45, 0.6, 0.8 | `inkvec-fit/src/merge/grid.rs:86` | the grid's arm lengths, in chords | none |
| `SCREEN_FLOOR` / `SCREEN_CEIL` | 1e-290 / 1e300 | `inkvec-fit/src/merge/residual.rs:204,208` | range where the residual's lower-bound screen is used (0 outside) | derived (normal-number range of the error bound) |
| `SCREEN_SHRINK` | 1 − 1e-12 | `inkvec-fit/src/merge/residual.rs:213` | lower bound's margin below the exact residual term | derived (~4,500 ulp; covers a `hypot` error up to ~2,000 ulp) |
| `REORDER_MARGIN` | 1 + 1e-12 | `inkvec-fit/src/merge/residual.rs:246` | middle-first partial sum against the bound | derived (Higham 1993, eq. 2.6) |
| `SHARPEN_MAX_CHORD` | 2.5 | `inkvec-fit/src/merge.rs:569` | chamfer-cubic chord ceiling | motivated (chamfer ~1px/side) |
| `SHARPEN_MAX_EDGE` | 8.0 | `inkvec-fit/src/merge.rs:574` | short-edge-with-chamfers ceiling | none |
| `SHARPEN_MIN_TURN` | `CORNER_TURN_MIN` | `inkvec-fit/src/merge.rs:580` | corner-vs-smooth threshold | shared with `CORNER_TURN_MIN` by design since 2026-09-08 |
| `PARAMS_AXIS_LINE` (research) | 1.0 | `inkvec-fit/src/merge/snap.rs:17` | axis-snapped line cost | derived |
| `MAX_AXIS_DEV_SIGMA` (research) | 3.0 | `inkvec-fit/src/merge/snap.rs:22` | per-sample axis-snap deviation cap | none |
| `PARAMS_SMOOTH_CUBIC` (research) | 4.0 | `inkvec-fit/src/merge/snap.rs:204` | `S`-shorthand cost | derived |
| G1 pre-filter angle (research) | 20 deg | `inkvec-fit/src/merge/snap.rs:344` | when a smooth-join candidate is worth the refit | none |
| `FLATTEN` | 16 | `inkvec-fit/src/simple.rs:63` | flattening resolution for the self-crossing test | motivated (below render-visibility floor) |
| `EPS` (endpoint coincidence) | 1e-6 px | `inkvec-fit/src/simple.rs:67` | adjacency exemption tolerance | derived |

## 12 — Repair ([12-repair.md](12-repair.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `ROUNDS` | 10 | `inkvec-cli/src/rings.rs:282` | max rounds of the outer repair loop (each refit pinned or halved) | none |
| `MERGE_BUDGET` | 96 segments | `inkvec-cli/src/rings.rs:456` | per-boundary merge affordability (4x this is the global ceiling) | measured (~8ms/segment) |
| `RING_SAMPLES` | 4 | `inkvec-cli/src/rings.rs:907` | interior samples per curved segment for area/containment | none |
| crossing-pair limit | 32 | `inkvec-cli/src/rings.rs:326,576` | max crossing pairs reported per ring per call | none |
| explosion thresholds | `c > 32 && c > 4*f` | `inkvec-cli/src/rings.rs:542` | when a capped refit is discarded for the unconstrained fit | measured (`bulma` case); the pair (32, 4) not separately justified |
| cap initial value | `polys[k].len().max(2)` | `inkvec-cli/src/rings.rs:287` | starting span cap per edge | derived |
| halving rule | `(cap[k]/2).max(1)` | `inkvec-cli/src/rings.rs:400,421` | tightening schedule, keeps repair logarithmic | derived |
| `LOCAL_ROUNDS` | 4 | `inkvec-cli/src/rings.rs:600` | pinned refits per edge before the repair only halves its cap | none (bounds the pins, so the cap still terminates) |
| pin or cap | lower `MultimodelFit::cost`, ties to the pin | `inkvec-cli/src/rings.rs:405` | which refit a crossing edge keeps | the fit's own objective |
| `BLK` | 16 | `inkvec-fit/src/simple.rs:306` | bounding-box block size for the all-pairs prune | motivated (speed only) |

`FLATTEN` and `EPS` are shared with `inkvec-fit/src/simple.rs` — see 11-fitting.md.

## 13 — Emit ([13-emit.md](13-emit.md))

| name | value | file:line | controls | basis |
|---|---|---|---|---|
| `EMIT_DECIMALS` | 2 | `inkvec-cli/src/pathdata.rs:44` | coordinate decimal places | derived — see 13-emit.md, "Coordinate precision" |
| `MIN_RING_AREA` | 0.25 px² | `inkvec-cli/src/rings.rs:34` | smallest ring area worth emitting | motivated |
| `S`-shorthand tolerance (`fmt_ring_with`) | 10^-decimals (0.01 px default) | `inkvec-cli/src/pathdata.rs:216` | rounded-space reflection test | derived |
| `S`-shorthand tolerance (`fmt_fitted`) | 5e-4 px, raw-space | `inkvec-cli/src/pathdata.rs:76` | stroke-path reflection test | none |
| ring segment minimum | 2 | `inkvec-cli/src/pathdata.rs:275` | a ring is discarded below this | derived (it encloses nothing) |
| path point minimum | 3 | `inkvec-cli/src/pathdata.rs:137` | `fmt_path` early return | derived (degenerate-polygon guard) |
| arc rotation precision | 3 decimals | `inkvec-cli/src/pathdata.rs:114,258,338` | arc `phi` formatting | none |
| rounded-rect radius floor | 1e-4 | `inkvec-cli/src/primitive.rs:76,281` | below this, written as a plain rect | none |
| ellipse rotation floor | 1e-3 | `inkvec-cli/src/primitive.rs:258` | below this, no `transform` written | none |
| alpha-ramp endpoint precision | 2 decimals | `inkvec-cli/src/emit.rs:724` | independent of `EMIT_DECIMALS` | none |
| `RAMP_MIN_INTERIOR` | 64 px | `inkvec-cli/src/alpha.rs:93` | fewest interior pixels before a plane is fitted to a face's alpha; `face_alpha` skips smaller faces without calling the fit | none (the fit's own first test, now named) |
| `RAMP_MIN_FADE` | 0.15 (opacity) | `inkvec-cli/src/alpha.rs:87` | least opacity change across a face for it to count as a fade | none |
| `RAMP_MAX_RESIDUAL` | 0.06 (opacity) | `inkvec-cli/src/alpha.rs:90` | largest RMS residual of the alpha plane | none |
| opacity precision | 3 decimals | `inkvec-cli/src/emit.rs:724,766,772` | `fill-opacity`/`stop-opacity` | none |
| `INKVEC_EMIT_DECIMALS` (env) | overrides `EMIT_DECIMALS` | `inkvec-cli/src/pathdata.rs:50` | the mechanism used to isolate rounding from segment price in the 7.2% measurement | measured |
| `EVENODD` | ` fill-rule="evenodd"` | `inkvec-cli/src/emit/winding.rs:102` | written only on a path the winding pass cannot read; every other path is wound by nesting depth and carries no `fill-rule` | derived (the old output as a safe fallback) |
| `JND` (gradient demotion) | 0.02 (OKLab) | `inkvec-cli/src/pipeline/demote.rs:41` | a gradient whose every pair of stops is closer than this is painted flat | none ("a conservative multiple of a just-noticeable difference") |
| `LAYER_SIGMA_SRGB` | 3/255 | `inkvec-cli/src/alpha/layers.rs:22` | colour uncertainty a `--layers` hypothesis is judged against | measured (smallest value that finds a known layer) |
| `LAYER_MAX_DE00` | 1.0 dE00 | `inkvec-cli/src/alpha/layers.rs:32` | `--layers` reproduction guard: every covered face within this of its own colour | motivated (about one just-noticeable difference, the tolerance the merge after it already spends) |
| `CANVAS_TOL` | 0.25 px | `inkvec-cli/src/post.rs:129` | `--no-background` match of a fitted canvas `<rect>` | measured (a fitted canvas rect lands 0.01-0.02 px off; the path match uses `EMIT_DECIMALS`) |
| margin viewBox precision | 2 decimals | `inkvec-cli/src/post.rs:336` | `--margin` growth | none |
| `ci_gate.py` ratio margin | 3% relative at the one-sided 95% upper bound (dE00 1%, turning 2%) | `bench/ci_gate.py:138` `MARGINS` | regression gate on parameter count vs. artist | measured (project's compactness regression budget) |

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
| face-id limit (`write_faces`) | 65,534 faces (`u16::MAX - 1`) | `inkvec-trace/src/fast/faces/runs.rs:513`; the same count as `CAPPED` in `inkvec-trace/src/fast/bands.rs:200` | most faces Fast numbers; past it the smallest components are merged into their neighbours (`regions::cap_components`, as Quality's `split_components` does past `MAX_FACES`) until this many remain, and at this count or more the ramp pass's palette precheck steps aside | motivated (face ids are u16 and `u16::MAX` is the outside of the planar map; the source does not say why Fast stops one below Quality's `MAX_FACES` = 65,535) |
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
| `MAX_TURN` / `MAX_RUN` | 3.10 rad / 24 pieces | `inkvec-trace/src/fast/curve.rs:21, 24` | most a merge of two or more pieces may turn in total / longest run one merge may cover; a single piece is always its own cubic whatever it turns (since `3513beb`) | motivated |
| turn-sign threshold (`optimise_run`) | 1e-6 rad | `inkvec-trace/src/fast/curve.rs:299` | a piece turning less than this counts as turning either way in the one-way test of a merge | none |
| `TOL` (prims) | 0.3 px | `inkvec-trace/src/fast/prims.rs:20` | largest distance from any point to an accepted circle or ellipse; `2 * TOL` is the screen for the Kasa circle and Taubin's conic | none |
| `ROUGHLY_ROUND` / `MIN_POINTS` / `MIN_RADIUS` / `MIN_AREA_SHARE` | 0.6 / 12 / 1.5 px / 0.8 | `inkvec-trace/src/fast/prims.rs:23, 25, 27, 30` | gates of the primitive cascade | motivated (`MIN_AREA_SHARE` rejects a sliver along an arc) / none |
| point weight (prims) | sigma 0.5 px | `inkvec-trace/src/fast/prims.rs:164` | the per-point sigma every circle and ellipse fit in the primitive test uses | none |
