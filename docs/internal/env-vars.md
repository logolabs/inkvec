# Environment variables

Inkvec is configured through `inkvec::Options` (and the command line built on it), never
through the environment. This page lists every `INKVEC_*` variable any crate reads, why each
one that is left is still there, and what happened to the rest. It was written for the settings
cleanup of 2026-09-26 (CHANGELOG, *Unreleased*), which started from the quality audit's finding
that the engine read 91 distinct variables at 104 sites through four copy-pasted helpers.

## Rules

- **One reader.** Engine and CLI code reads its environment only through `inkvec_core::env`
  (`flag`, `switch`, `number`, `count`, `text`, `path`). `bench/quality.py` counts the
  `std::env::var` calls anywhere else in `crates/*/src` (`env_reads`, 144 on main, 13 after) and
  fails if the count grows.
- **One meaning of `0`.** A flag is off when unset, empty or `0`, and on for anything else. A
  stage that is on by default (`INKVEC_BOPT`, `INKVEC_GRAD_REGIONS`, `INKVEC_NATIVE_ALPHA`) is
  switched off by `0`. Before, `INKVEC_NO_CARVE=0` *disabled* carving and `INKVEC_NO_TAPER=`
  (empty) was ignored while `INKVEC_PALDBG=0` turned the trace on.
- **Read once.** A variable is read the first time it is asked for and the answer kept for the
  process, so a trace never sees a value change halfway.
- **What may stay.** A variable that changes the output stays only if it is an A/B switch the
  CHANGELOG documents or a benchmark script sets. Diagnostics (stderr traces, dump files) stay:
  they never change the SVG. Experiments move behind the `research` cargo feature of the crate
  that owns them (`inkvec-trace`, `inkvec-fit`; `inkvec-cli`'s `research` turns both on).
  Everything else became a named constant next to its use.
- **Adding one.** Do not, unless it is a diagnostic or a switch a benchmark needs. A real choice
  belongs in `inkvec::Options`, where the schema, the bindings, the server and Studio see it.

Outside `crates/`, and so outside this inventory: Studio reads its own app settings
`INKVEC_STUDIO_CONFIG_DIR` and `INKVEC_STUDIO_MODEL_DIR` (and sets `INKVEC_RESTORE_ONNX` for the
restorer); the benchmark scripts read `INKVEC_TIER`, `INKVEC_BENCH_DATA`, `INKVEC_BENCH_WORK`,
`INKVEC_BENCH_CACHE`, `INKVEC_CACHE_SALT`, `INKVEC_SKIP_DISTS` and `INKVEC_NO_SCORE_CACHE`
themselves, and the tracer never sees them. `bench/ablate.py` still sets `INKVEC_IMGADJ=0`, a
switch whose stage was deleted before this cleanup; that row measures the unchanged default.

## Candidates for `inkvec::Options` (class c, not added)

| setting | today | why it is a user's choice |
|---|---|---|
| `bezier_cost`, `corner_angle` | CLI `--bezier-cost`, `--corner-angle` and Studio; replaced `INKVEC_PARAMS_CUBIC` / `INKVEC_G1_BREAK` | curve-vs-line trade and which bends read as corners; the CHANGELOG already says they are "not yet" Options fields |
| `matte` (`auto`, `white`, `black`, colour) | `INKVEC_MATTE`, removed here | which ground a transparent image is composited on when native alpha is off; `choose_matte` guesses |
| `decimals` | `INKVEC_EMIT_DECIMALS` (kept for the bench) | file size against coordinate precision; `emit_decimals` ignores `precision` today |
| `layers` | CLI `--layers` only (`INKVEC_LAYERS` removed) | translucent-layer recovery, off by default |

## Inventory

Every name read anywhere in `crates/` on main 34caf25, by class. *Read at* is where it is read now (for a removed one, where it was read on main); *set by* is every place outside the Rust sources that mentions it (bench scripts, CI, docs, bindings).

| class | count |
|---|---|
| (a) DELETE: now a named constant | 47 |
| (b) RESEARCH: `--features research` only | 35 |
| (d) DEBUG / BENCH: kept, read through `inkvec_core::env` | 33 |
| (e) Deployment, packaging and test harness: not engine settings, left as they were | 12 |
| total names read anywhere in `crates/` on main 34caf25 | 127 |

## (d) DEBUG / BENCH: kept, read through `inkvec_core::env`

| variable | type | default | read at | set by (outside the source) | what / why |
|---|---|---|---|---|---|
| `INKVEC_ABSDBG` | flag | off | crates/inkvec-trace/src/regions.rs:363 | docs/algorithm/04-regions.md | sliver-absorption rejections. |
| `INKVEC_ALPHADBG` | flag | off | crates/inkvec-cli/src/alpha.rs:821, crates/inkvec-cli/src/emit.rs:785 | docs/algorithm/01-intake.md, docs/algorithm/13-emit.md | alpha / layer / emit face dump. |
| `INKVEC_BOPT` | switch | on | crates/inkvec-cli/examples/neural_trace.rs:246, crates/inkvec-trace/src/lib.rs:1143 | bench/ablate.py, bench/spikes.py, docs/algorithm/08-boundary-solve.md | `0` skips the global boundary solve. bench/ablate.py and bench/spikes.py price the stage with it. |
| `INKVEC_BOPTDBG` | flag | off | crates/inkvec-trace/src/boundary_opt.rs:1131 | docs/algorithm/08-boundary-solve.md | boundary-solve iteration trace. |
| `INKVEC_BOPT_CELLS` | flag | off | crates/inkvec-trace/src/boundary_opt.rs:1150 | docs/algorithm/08-boundary-solve.md | boundary-solve per-cell clipped areas. |
| `INKVEC_DEBUG_FIT` | flag | off | crates/inkvec-fit/src/lib.rs:919 | nothing | line-vs-cubic run decisions in the fitter. |
| `INKVEC_DIAG` | text | off | crates/inkvec-trace/src/diag.rs:44 | docs/TRAZOR_REVIEW.md | `1`/any: diagnostics as text on stderr, `json`: as JSON lines. |
| `INKVEC_DPDBG` | flag | off | crates/inkvec-fit/src/multimodel.rs:119 | nothing | multimodel dynamic-program decisions. |
| `INKVEC_DUMP_CONTOUR` | path | unset | crates/inkvec-trace/src/planar.rs:819 | docs/algorithm/07-subpixel.md | appends refined boundary points and sigmas. |
| `INKVEC_DUMP_LABELS` | path | unset | crates/inkvec-trace/src/lib.rs:650 | docs/algorithm/04-regions.md | writes the label image as PPM. |
| `INKVEC_DUMP_MAP` | path | unset | crates/inkvec-cli/src/pipeline.rs:493 | nothing | writes the planar map (neural label generation). |
| `INKVEC_EDIT_DEBUG` | flag | off | crates/inkvec-cli/src/editable.rs:532 | nothing | editability mirror pass. |
| `INKVEC_EMIT_DECIMALS` | count | `EMIT_DECIMALS` (2) | crates/inkvec-cli/src/emit.rs:65 | bench/perceptual_probe.py, docs/algorithm/13-emit.md, docs/algorithm/constants.md | coordinate decimals; bench/perceptual_probe.py isolates rounding with it. |
| `INKVEC_EVDBG` | flag | off | crates/inkvec-trace/src/gradient.rs:1363, crates/inkvec-trace/src/gradient.rs:1469, crates/inkvec-trace/src/gradient/carve.rs:254 (+1) | docs/algorithm/05-gradients.md | fill-evidence and gradient-refusal trace. |
| `INKVEC_FAB_TIMING` | flag | off | crates/inkvec-fab/src/lib.rs:74, crates/inkvec-fab/src/lines.rs:272 | nothing | inkvec-fab timings. |
| `INKVEC_FADEDBG` | flag | off | crates/inkvec-trace/src/native.rs:1374 | nothing | native-alpha fade trace. |
| `INKVEC_GRADDBG` | text `x0,y0,x1,y1` | unset | crates/inkvec-trace/src/gradient/debug.rs:33 | nothing | gradient-fit trace for regions inside a window. |
| `INKVEC_GRAD_REGIONS` | switch | on | crates/inkvec-trace/src/gradient/regions.rs:10 | CHANGELOG.md | `0` switches region-level gradient recovery off (CHANGELOG 0.2.0). |
| `INKVEC_HARMONIZE_TOL` | number | `HARMONIZE_TOL` | crates/inkvec-cli/src/harmonize.rs:89 | bench/perceptual_probe.py | shape-harmonize tolerance; bench/perceptual_probe.py sets 1000 to disable it. |
| `INKVEC_JDBG` | flag | off | crates/inkvec-trace/src/planar/junctions.rs:188, crates/inkvec-trace/src/planar/junctions.rs:300 | docs/algorithm/07-subpixel.md | junction end-tangent fits (was `JDBG`, renamed to the prefix). |
| `INKVEC_JUNCDBG` | flag | off | crates/inkvec-trace/src/boundary_opt.rs:1277 | docs/algorithm/08-boundary-solve.md | boundary-solve junction counters. |
| `INKVEC_MERGEDBG` | flag | off | crates/inkvec-trace/src/gradient/bands.rs:324 | docs/algorithm/05-gradients.md | band-merge trace. |
| `INKVEC_NATIVE_ALPHA` | switch | on | crates/inkvec/tests/api.rs:211, crates/inkvec-cli/src/args.rs:169, crates/inkvec-cli/src/args.rs:655 | CHANGELOG.md, bindings/openapi.json, bindings/options.schema.json, crates/inkvec-py/README.md, crates/inkvec-py/python/inkvec/__init__.pyi, crates/inkvec-py/tests/test_inkvec.py, crates/inkvec-server/README.md, crates/inkvec/README.md, crates/inkvec/tests/api.rs, docs/BINDINGS.md, packages/dotnet/README.md, packages/dotnet/src/LogoLabs.Inkvec/InkvecOptions.generated.cs, packages/go/README.md, packages/go/options_generated.go, packages/java/README.md, packages/java/src/main/java/com/logolabs/inkvec/InkvecOptions.java, packages/npm/README.md, packages/npm/src/options.generated.ts, packages/php/README.md, packages/swift/README.md, packages/swift/Sources/Inkvec/InkvecOptions.generated.swift, studio/core/src/options.rs | `0` makes the CLI default `--no-native-alpha` (CHANGELOG 0.1.4; docs/BINDINGS.md). |
| `INKVEC_NO_ABSORB` | flag | off | crates/inkvec-trace/src/lib.rs:620, crates/inkvec-trace/src/native.rs:1595 | bench/ablate.py, docs/algorithm/04-regions.md | skips blend-sliver absorption. bench/ablate.py. |
| `INKVEC_NO_CARVE` | flag | off | crates/inkvec-trace/src/lib.rs:721, crates/inkvec-trace/src/lib.rs:975, crates/inkvec-trace/src/native.rs:1623 | bench/ablate.py, docs/algorithm/05-gradients.md | skips residual carving. bench/ablate.py. |
| `INKVEC_NO_TAPER` | flag | off | crates/inkvec-trace/src/planar/junctions.rs:393 | bench/ablate.py, docs/algorithm/07-subpixel.md | skips taper junction placement. bench/ablate.py. |
| `INKVEC_PALDBG` | flag | off | crates/inkvec-trace/src/color.rs:1035, crates/inkvec-trace/src/lib.rs:480, crates/inkvec-trace/src/native.rs:454 | docs/algorithm/02-coverage.md, docs/algorithm/03-palette.md | palette accept/reject trace. |
| `INKVEC_REFINEDBG` | flag | off | crates/inkvec-trace/src/centerline.rs:1023 | nothing | centerline stroke refinement. |
| `INKVEC_SUBPXDBG` | flag | off | crates/inkvec-trace/src/planar.rs:423 | docs/algorithm/07-subpixel.md | sub-pixel search per point. |
| `INKVEC_SVGMIN_DEBUG` | flag | off | crates/inkvec-svgmin/src/fit.rs:342, crates/inkvec-svgmin/src/fit.rs:513 | nothing | minifier merge decisions. |
| `INKVEC_TAPERDBG` | flag | off | crates/inkvec-trace/src/planar/junctions.rs:463 | docs/algorithm/07-subpixel.md | taper fits (was `TAPERDBG`, renamed to the prefix). |
| `INKVEC_TIMING` | flag | off | crates/inkvec-cli/src/pipeline.rs:570, crates/inkvec-cli/src/rings.rs:114, crates/inkvec-cli/src/rings.rs:139 (+10) | docs/algorithm/00-overview.md, docs/algorithm/04-regions.md, docs/algorithm/05-gradients.md, docs/algorithm/10-symmetry.md, docs/algorithm/12-repair.md, docs/algorithm/13-emit.md | stage timings on stderr (Stopwatch and per-stage logs). |
| `INKVEC_UNDERLAP` | number (px) | `UNDERLAP` | crates/inkvec-cli/src/seams.rs:61 | CHANGELOG.md | seam underlap reach, `0` off (CHANGELOG 0.2.0). |

## (b) RESEARCH: `--features research` only

| variable | type | default | read at | set by (outside the source) | what / why |
|---|---|---|---|---|---|
| `INKVEC_AXIS` | flag | off | crates/inkvec-fit/src/multimodel.rs:238 | docs/algorithm/11-fitting.md | `merge::snap_axis_aligned`. |
| `INKVEC_BOPT_CHUNKS` | count | 1 | crates/inkvec-trace/src/boundary_opt.rs:529 | docs/algorithm/08-boundary-solve.md | chunked parallel data term (changes summation order). |
| `INKVEC_BOPT_JUNC` | flag | off | crates/inkvec-trace/src/boundary_opt.rs:1148 | docs/algorithm/08-boundary-solve.md | junction wedges in the boundary solve (measured worse). |
| `INKVEC_DECODE` | flag | off | crates/inkvec-trace/src/lib.rs:1154 | docs/algorithm/00-overview.md, docs/algorithm/09-decode.md | order-first decoding (`decode` module, 2,100 lines; `decode_faces` was 57 KiB of the release binary). |
| `INKVEC_DECODEDBG` | flag | off | crates/inkvec-trace/src/decode.rs:576, crates/inkvec-trace/src/decode.rs:905, crates/inkvec-trace/src/decode.rs:912 | docs/algorithm/09-decode.md | decode trace. |
| `INKVEC_DECODE_GAIN` | number | `MIN_GAIN` 0.5 | crates/inkvec-trace/src/decode.rs:773 | docs/algorithm/09-decode.md | decode residual cut. |
| `INKVEC_DECODE_KEEPFILL` | flag | off | crates/inkvec-trace/src/decode.rs:812 | docs/algorithm/09-decode.md | decode keeps the face's fill model. |
| `INKVEC_DECODE_LEAK` | number | `LEAK_GATE` 0.05 | crates/inkvec-trace/src/decode.rs:575 | docs/algorithm/09-decode.md | decode leak gate. |
| `INKVEC_DECODE_MS` | number (ms) | caller budget | crates/inkvec-trace/src/decode.rs:573 | docs/algorithm/09-decode.md | decode time budget. |
| `INKVEC_DECODE_OVERRIDE` | number | `EVIDENCE_OVERRIDE` 0.0 | crates/inkvec-trace/src/decode.rs:748 | docs/algorithm/09-decode.md | decode evidence override. |
| `INKVEC_DECODE_RECHECK` | number | 1.0 (on) | crates/inkvec-trace/src/decode.rs:751 | docs/algorithm/09-decode.md | decode post-solve recheck. |
| `INKVEC_DECODE_SHARE` | flag | off | crates/inkvec-trace/src/decode.rs:834 | docs/algorithm/09-decode.md | decode ribbon width pooling. |
| `INKVEC_DECODE_THIN` | number (px) | `THIN_PX` 2.5 | crates/inkvec-trace/src/decode.rs:680, crates/inkvec-trace/src/decode/ribbon.rs:91 | docs/algorithm/09-decode.md | decode width gate. |
| `INKVEC_EDGE_INK_DE00` | number | 4.0 | crates/inkvec-trace/src/ink_ideas.rs:874 | nothing | ink_ideas rule 5. |
| `INKVEC_EDIT_PASSES` | text list | all | crates/inkvec-cli/src/editable.rs:791 | nothing | run only the named editability passes, to price each. |
| `INKVEC_FREE_CUBIC` | flag | off | crates/inkvec-fit/src/candidates.rs:55 | docs/algorithm/11-fitting.md | free-tangent cubic candidate (does not pay; see candidates.rs). |
| `INKVEC_G1` | flag | off | crates/inkvec-fit/src/multimodel.rs:243 | docs/algorithm/11-fitting.md, docs/algorithm/13-emit.md | `merge::snap_smooth_joins`. |
| `INKVEC_G1DBG` | flag | off | crates/inkvec-fit/src/merge/snap.rs:230 | docs/algorithm/11-fitting.md | snap_smooth_joins trace. |
| `INKVEC_INK_IDEA` | text | unset | crates/inkvec-trace/src/lib.rs:577 | crates/inkvec-trace/Cargo.toml | one of five ink-grouping rules (`ink_ideas`, already research-only). |
| `INKVEC_INK_KAPPA` | number | `KAPPA_DEFAULT` | crates/inkvec-trace/src/ink_ideas.rs:407, crates/inkvec-trace/src/ink_ideas.rs:499 | nothing | ink_ideas rules 1-2. |
| `INKVEC_INK_MAX_MERGES` | number | 400000 | crates/inkvec-trace/src/ink_ideas.rs:552 | nothing | ink_ideas rule 3. |
| `INKVEC_INK_PLANE` | switch | on | crates/inkvec-trace/src/ink_ideas.rs:122 | nothing | ink_ideas plane residuals. |
| `INKVEC_INK_TINT_MAX` | number | 3.0 | crates/inkvec-trace/src/ink_ideas.rs:224 | nothing | ink_ideas tint cap. |
| `INKVEC_LOSSY_KEEP_NOISE` | flag | off | crates/inkvec-trace/src/lib.rs:508 | nothing | keep the pre-regularisation noise estimate. |
| `INKVEC_LOSSY_REGULARIZE` | flag | off | crates/inkvec-trace/src/lib.rs:1197 | nothing | re-label a lossy intake (`regularize::labels`). |
| `INKVEC_MERGE_COMMON_PIXELS` | flag | off | crates/inkvec-trace/src/gradient/bands.rs:328 | nothing | band merge priced on common pixels. |
| `INKVEC_MERGE_DE00` | number | unset | crates/inkvec-trace/src/color.rs:1038 | nothing | perceptual (dE00) ink merge radius. |
| `INKVEC_PALETTE_RGB` | flag | off | crates/inkvec-trace/src/color.rs:68 | nothing | cluster the palette in sRGB instead of OKLab. |
| `INKVEC_POTTS_SCALE` | number | 1.0 | crates/inkvec-trace/src/ink_ideas.rs:635 | nothing | ink_ideas rule 3 price. |
| `INKVEC_SADDLE` | flag | off | crates/inkvec-trace/src/regions.rs:34 | docs/algorithm/04-regions.md, docs/algorithm/06-planar-map.md | saddle-corner face merging (`merge_saddle_faces`). |
| `INKVEC_SADDLEDBG` | flag | off | crates/inkvec-trace/src/regions.rs:37 | docs/algorithm/04-regions.md, docs/algorithm/06-planar-map.md | saddle trace. |
| `INKVEC_STRUCTURAL` | flag | off | crates/inkvec-cli/src/pipeline.rs:626, crates/inkvec-fit/src/multimodel.rs:233 | nothing | structural MDL simplifier (fit) and its transactional baseline (CLI). |
| `INKVEC_STRUCTURAL_MAX_DIST` | number (px) | 0.85 | crates/inkvec-fit/src/structural/simplify.rs:78 | nothing | structural simplifier deviation bound. |
| `INKVEC_STRUCTURAL_SIGMA_CAP` | number | 2.0 | crates/inkvec-fit/src/structural/simplify.rs:96 | nothing | structural simplifier sigma cap. |
| `INKVEC_WITNESS_TAU` | number | 0.2 | crates/inkvec-trace/src/ink_ideas.rs:797 | nothing | ink_ideas rule 4. |

## (a) DELETE: now a named constant

| variable | type | default | read at | set by (outside the source) | what / why |
|---|---|---|---|---|---|
| `INKVEC_BIMODAL` | number | 0.85 | was crates/inkvec-trace/src/gradient.rs:1420 | docs/algorithm/05-gradients.md, docs/algorithm/constants.md | `BIMODAL_MARGIN`. |
| `INKVEC_BLEND_TMIN` | number | 0.04 | was crates/inkvec-trace/src/color.rs:608 | docs/algorithm/03-palette.md, docs/algorithm/constants.md | now `color::BLEND_TMIN`. |
| `INKVEC_BOPT_ANCHOR` | number | `K_ANCHOR` 0.10 | was crates/inkvec-trace/src/boundary_opt.rs:1155 | docs/algorithm/08-boundary-solve.md | constant. |
| `INKVEC_BOPT_ITERS` | count | 48 | was crates/inkvec-trace/src/boundary_opt.rs:1117 | docs/algorithm/08-boundary-solve.md, docs/algorithm/constants.md | now `boundary_opt::ITERS`; the 24/48/96/192 sweep is in the comment. |
| `INKVEC_BOPT_KINK` | number | `K_KINK` 0.05 | was crates/inkvec-trace/src/boundary_opt.rs:1154 | docs/algorithm/08-boundary-solve.md | constant. |
| `INKVEC_BOPT_MS` | number (ms) | none | was crates/inkvec-trace/src/boundary_opt.rs:1119 | docs/algorithm/08-boundary-solve.md, docs/algorithm/constants.md | forced a wall clock; a clock is the caller's `time_budget` only. |
| `INKVEC_BREAK_EXP` | number | 2.0 | was crates/inkvec-fit/src/tangents.rs:250 | nothing | turn-cost exponent; nothing measured another. |
| `INKVEC_CONTENT_SCALE` | `1` | off | was crates/inkvec-cli/src/lib.rs:145 | docs/algorithm/01-intake.md | duplicate of `--content-units` (an Option). |
| `INKVEC_CURV_GAIN` | number | 0.35 | was crates/inkvec-trace/src/contour.rs:238 | docs/algorithm/07-subpixel.md | `contour::NONLINEARITY_GAIN`; the sweep is in its doc comment. |
| `INKVEC_DP_MAX_POINTS` | count | 768 | was crates/inkvec-fit/src/multimodel.rs:157 | nothing | `DP_MAX_POINTS`; callers use `with_dp_max_points`. |
| `INKVEC_FIT_PIXELS_CAP` | count | 65536 | was crates/inkvec-trace/src/gradient/budget.rs:32 | nothing | `FIT_PIXELS_CAP`. |
| `INKVEC_G1_BREAK` | number (deg) | 10.0 | was crates/inkvec-fit/src/cost.rs:57 | CHANGELOG.md | superseded by `--corner-angle`. |
| `INKVEC_LAYERS` | flag | off | was crates/inkvec-cli/src/alpha.rs:639 | docs/algorithm/01-intake.md | duplicate of `--layers`. |
| `INKVEC_LAYER_SIGMA` | number (levels) | 3.0 | was crates/inkvec-cli/src/alpha.rs:668 | docs/algorithm/01-intake.md | `LAYER_SIGMA_SRGB`. |
| `INKVEC_MATTE` | text | auto | was crates/inkvec-cli/src/alpha.rs:329 | docs/algorithm/01-intake.md | forced the compositing matte; see (c). |
| `INKVEC_MERGE_BREAK` | number | 2.0 | was crates/inkvec-fit/src/merge.rs:93 | docs/algorithm/11-fitting.md, docs/algorithm/constants.md | now `merge::BREAK_PARAMS` (it was already the default). |
| `INKVEC_MIN_CONTRAST` | number | 1.0 | was crates/inkvec-trace/src/gradient.rs:1342 | docs/algorithm/05-gradients.md | gradient contrast floor scale. |
| `INKVEC_NATIVE_GROUND` | number | 0.5 | was crates/inkvec-trace/src/native.rs:83 | nothing | now `native::SECOND_GROUND`. |
| `INKVEC_NOISE_MEDIAN` | flag | off | was crates/inkvec-trace/src/coverage.rs:260 | nothing | old median noise estimator; the regression test now reads it directly. |
| `INKVEC_NOISE_SIGMAS` | number | 0 / 3 (soft) | was crates/inkvec-trace/src/color.rs:1034, crates/inkvec-trace/src/native.rs:460 | docs/algorithm/02-coverage.md, docs/algorithm/03-palette.md | the soft-intake gate chooses; documented as a manual step before the gate existed. |
| `INKVEC_NO_ARCS` | flag | off | was crates/inkvec-fit/src/candidates.rs:42 | docs/algorithm/11-fitting.md | arc candidate ablation, documented only in docs/algorithm. |
| `INKVEC_NO_ELLIPSE` | flag | off | was crates/inkvec-fit/src/candidates.rs:35 | nothing | ellipse candidate ablation, undocumented. |
| `INKVEC_NO_FADES` | flag | off | was crates/inkvec-trace/src/native.rs:1649 | nothing | native fade-merge ablation, undocumented. |
| `INKVEC_NO_INK_ESCAPE` | flag | off | was crates/inkvec-trace/src/color.rs:1046 | docs/algorithm/03-palette.md | palette MDL-escape ablation. |
| `INKVEC_NO_MEASURED_SIGMA` | flag | off | was crates/inkvec-trace/src/lib.rs:542, crates/inkvec-trace/src/native.rs:1592 | nothing | ablation, undocumented. |
| `INKVEC_NO_PRIMITIVE` | flag | off | was crates/inkvec-cli/src/pipeline.rs:602 | nothing | primitive-path ablation (30.5 % of the ratio; the number is kept in the comment). |
| `INKVEC_PARAMS_CUBIC` | number | 6.0 | was crates/inkvec-fit/src/cost.rs:56 | CHANGELOG.md | superseded by `--bezier-cost` (CHANGELOG 0.2.0 said the variable still worked). |
| `INKVEC_RAMP_STEP` | number | 15.0 | was crates/inkvec-trace/src/gradient/regions.rs:16 | nothing | now `gradient::regions::RAMP_STEP_DE00`. |
| `INKVEC_RINGING` | number | `SOFT_RINGING(_LARGE)` | was crates/inkvec-trace/src/lib.rs:435 | nothing | soft-intake ringing gate; constant. |
| `INKVEC_SAME_INK_DE00` | number | 1.5 / 5.0 (soft) | was crates/inkvec-trace/src/color.rs:1038 | docs/algorithm/02-coverage.md, docs/algorithm/03-palette.md | the soft-intake gate chooses. |
| `INKVEC_SIGMA_CAP` | number | `MEASURED_SIGMA_CAP` | was crates/inkvec-trace/src/lib.rs:553 | nothing | measured-noise cap; constant. |
| `INKVEC_SIGMA_FLAT` | number | unset | was crates/inkvec-trace/src/contour.rs:335 | nothing | flat-sigma experiment; the measurement is kept as a comment in contour.rs. |
| `INKVEC_SIGMA_FLOOR` | number | 0.0 | was crates/inkvec-trace/src/contour.rs:356 | docs/algorithm/07-subpixel.md | now `contour::SIGMA_FLOOR`. |
| `INKVEC_SIGMA_SCALE` | number | `MEASURED_SIGMA_SCALE` | was crates/inkvec-trace/src/lib.rs:543 | nothing | measured-noise scale; constant. |
| `INKVEC_SMOOTH` | number | 0.0 | was crates/inkvec-fit/src/merge.rs:82 | nothing | now `merge::SMOOTH_SLACK`. |
| `INKVEC_SMOOTH_FRACTION` | number | 0.5 | was crates/inkvec-trace/src/gradient/regions.rs:38 | nothing | now `SMOOTH_FRACTION`. |
| `INKVEC_SMOOTH_STEP` | number | 3.0 | was crates/inkvec-trace/src/gradient/regions.rs:29 | nothing | now `SMOOTH_STEP`. |
| `INKVEC_STROKE_TOL` | number | `STROKE_TOL` | was crates/inkvec-cli/src/emit.rs:266 | nothing | annulus-to-stroke tolerance. |
| `INKVEC_SUBPX_WIN` | count | 1 | was crates/inkvec-trace/src/planar.rs:390 | docs/algorithm/07-subpixel.md, docs/algorithm/constants.md | now `planar::SUBPX_WIN`; LOG-43 measurement kept in the comment. |
| `INKVEC_SVGMIN_COARSE_CAP` | count | 128 | was crates/inkvec-svgmin/src/fit.rs:461 | nothing | minifier constant. |
| `INKVEC_SVGMIN_MERGE` | number | 1 (on) | was crates/inkvec-svgmin/src/fit.rs:474 | nothing | minifier merge pass switch. |
| `INKVEC_SVGMIN_PER_SEG` | count | 24 | was crates/inkvec-svgmin/src/path.rs:212 | nothing | minifier constant. |
| `INKVEC_SVGMIN_PRIM_CAP` | count | 192 | was crates/inkvec-svgmin/src/fit.rs:498 | nothing | minifier constant. |
| `INKVEC_SVGMIN_PRIM_OPEN` | number | 1 (on) | was crates/inkvec-svgmin/src/fit.rs:497 | nothing | minifier switch. |
| `INKVEC_SVGMIN_WHOLE_BELOW` | count | 256 | was crates/inkvec-svgmin/src/fit.rs:462 | nothing | minifier constant. |
| `INKVEC_TAPER_SKIP` | count | unset | was crates/inkvec-trace/src/planar/junctions.rs:476 | docs/algorithm/07-subpixel.md | bisection aid (skip the n-th taper). |
| `INKVEC_WOBBLE_PENALTY` | number | 1.0 | was crates/inkvec-fit/src/candidates.rs:419 | nothing | now `candidates::WOBBLE_PENALTY`; the sweep is in its doc comment. |

## (e) Deployment, packaging and test harness: not engine settings, left as they were

| variable | type | default | read at | set by (outside the source) | what / why |
|---|---|---|---|---|---|
| `INKVEC_BLESS` | `1` | off | crates/inkvec/tests/contract.rs:67 | .github/workflows/bindings.yml, .github/workflows/swift.yml, bindings/contract/cases.json, bindings/contract/make_fixtures.py, crates/inkvec/tests/contract.rs, docs/BINDINGS.md, packages/go/README.md, packages/go/contract_test.go, packages/npm/test/contract.test.mjs | contract test re-record (CI, docs/BINDINGS.md). |
| `INKVEC_CONTRACT_REQUIRE_HASH` | `1` | off | crates/inkvec/tests/contract.rs:68 | .github/workflows/swift.yml, crates/inkvec/tests/contract.rs, docs/BINDINGS.md, packages/dotnet/test/LogoLabs.Inkvec.Tests/ContractTests.cs, packages/go/contract_test.go, packages/npm/test/contract.test.mjs, packages/swift/README.md, packages/swift/Tests/InkvecTests/ContractTests.swift | contract test strictness (CI). |
| `INKVEC_DEFAULT_TIME_BUDGET` | number | none | crates/inkvec-server/src/lib.rs:78 | crates/inkvec-server/README.md | inkvec-server. |
| `INKVEC_DUMP_ONLY` | flag | off | crates/inkvec-cli/examples/oracle_guidance.rs:407 | nothing | example `oracle_guidance` only. |
| `INKVEC_MAX_BODY_BYTES` | count | `DEFAULT_MAX_BODY_BYTES` | crates/inkvec-server/src/lib.rs:77 | CHANGELOG.md, crates/inkvec-server/README.md, services/docker/compose.yaml | inkvec-server (compose.yaml). |
| `INKVEC_MAX_CONCURRENCY` | count | cores | crates/inkvec-server/src/lib.rs:76 | CHANGELOG.md, crates/inkvec-server/README.md, services/docker/compose.yaml | inkvec-server (compose.yaml). |
| `INKVEC_PORT` | count | 8080 | crates/inkvec-server/src/lib.rs:114 | crates/inkvec-server/README.md | inkvec-server. |
| `INKVEC_PYTHON` | path | `python` / `python3` | crates/inkvec-sr/src/external.rs:62 | packages/npm/build.mjs, packages/npm/test/api.test.mjs | interpreter for the SR tools (inkvec-sr); set by packages/npm. |
| `INKVEC_REQUEST_TIMEOUT_SECS` | count | 60 | crates/inkvec-server/src/lib.rs:82 | crates/inkvec-server/README.md | inkvec-server. |
| `INKVEC_RESTORE_ONNX` | path | beside the binary | crates/inkvec-restore/build.rs:20, crates/inkvec-restore/src/lib.rs:391 | .github/workflows/release.yml, CHANGELOG.md, PIPELINE_EXPLANATION.md, crates/inkvec-cli/Cargo.toml, studio/src-tauri/src/denoiser.rs | restorer weights (inkvec-restore, build.rs too); Studio sets it; release.yml. |
| `INKVEC_RESTORE_WEIGHTS` | path | cache dir | crates/inkvec-restore/src/lib.rs:215 | nothing | restorer weights for the Burn path. |
| `INKVEC_TOOLS_DIR` | path | beside the binary | crates/inkvec-cli/src/lib.rs:755 | nothing | where the Python SR tools live (install layout). |
