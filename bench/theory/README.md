# bench/theory

Experiments behind [`docs/theory/README.md`](../../docs/theory/README.md): boundary-point
readings scored against exactly known geometry.

| file | what it does |
|---|---|
| `exact_raster.py` | exact box-filter (area-coverage) rasteriser for polygons in float64, a port of font-rs's signed-area accumulation; `python3 exact_raster.py` runs its self-check against hand-computed areas |
| `strip_eval.py` | the ½-crossing, the column-sum mean and the column sum with the cubic correction, on exact renders of circles, ellipses and straight edges, noiseless, 8-bit and with added noise |
| `inkvec_compare.py` | runs the inkvec binary with `INKVEC_DUMP_CONTOUR` and scores the points stage 07 (`planar::measure_subpixel`) measured beside the estimators above |
| `attribution.py` | each icon's dE00 against the artist's render at 1024 px, split by where it lies: within a source pixel of an edge (smooth, corner, junction), 1-3 px from one, flat or shaded interior, and blobs of missing or extra features |
| `params_diag.py` | the gate's parameter count of the trace and the artist's file, by element and segment kind, per family and for stroked against filled artwork |
| `naturality_diag.py` | parameters an artist would not write: collinear vertices, straight cubics, co-circular arcs, open strokes that continue each other |
| `evidence_eval.py` | the boundary chain's variance model and precision formulas (`docs/theory/chain-boundary.md`): window sums against exact areas under 8-bit and `n × n` point supersampling, the information column sums keep, a wedge's vertex from its arms, and the smallest fillet the pixels can tell from a sharp corner |
| `renderer_floor.py` | the corpus intake (resvg at 8×, 8-bit) against the artists' own geometry drawn exactly, per family and tier, by window: quantisation, true arcs against usvg's cubics, per-element compositing, the per-edge share of the floor, and with `--fresh` a render made now (is the tier stale?) and tiny-skia's 4 × 4 sample lattice replicated, split into straight fills, curves and strokes |
| `oracle_cost.py` | the trace against the artist's own file on the input's pixels and the tracer's price per parameter: which icons the objective would rather have the artist's description for (a search error) and which it prefers the trace for (a question for the prior), `docs/theory/optimal.md` §6 |
| `geom_calibration.py` | the gate's geometric match (`geom`) against dE00 on a disc grown by known amounts in two colour contrasts: `geom` reads the displacement in either colour, dE00 reads the contrast |
| `noise_profile.py` | where a real input's noise sits: the gate's `web` tier (resized, quality-80 JPEG) against the exact raster it stands for, split into the resampling's deterministic blur and the codec's error, by distance to the nearest edge, on luma and chroma, next to the engine's own noise estimate; and the window identity under that blur and noise (the sums stay unbiased, their variance grows) |
| `floor_selfcal.py` | the engine's own windows against the truth, per input: an exact render, 8 × 8 point samples, tiny-skia's lattice, resvg now, the committed tier and the `web` JPEG, each traced by the Quality front end and its evidence built (`examples/evidence_calibrate.rs`); `χ²` per independent measurement under rounding alone and under the image's self-calibrated noise model, scored with the evaluator's own `score` (replicas, per-edge offset, Huber), by class and family, with each image's measured noise, tail and window scale |
| `design_prior.py` | the human prior of `docs/theory/chain-representation.md` (R3), counted from the artists' own files with exact rational arithmetic: segment kinds, axis-aligned and 45° lines, smooth and sharp joins, numbers on the design grid by the role of their point, ties, repeated radii, stroke widths, layers, symmetry and repetition; and two simulations of the chain's decisions on the artists' numbers with added noise (empirical-Bayes grid inference with snapping, `--part grid_sim`; coordinate ties by exact one-dimensional clustering, `--part ties_sim`). Needs no binary |
| `design_stats.py` | traces against the artists' files, as chain R asks a description to be written: the shares of lines, cubics and arcs, of lines on an axis or at 45°, of on-curve numbers on the artist's design grid and on the raster's half pixels, of values tied, and parameters per icon; per family, for any traced tier (`--trace TIER`) |
| `amodal_eval.py` | completion against the artists' hidden geometry on the emoji families: each artist element rendered alone (its full shape, including what later elements hide), every face the completion stage tried matched to the element of its colour it shows most of, and the IoU of the face before and after completion against that element's full shape; win rates per candidate kind (primitive, corner, corner cut, chord, offset, bulge, bridge) and the share refused |
| `human_gap.py` | a change read as distance to the artist's file rather than as raw values: per icon the turning gap (the gate's `turning` times the canvas width, trace against artist) and the parameter gap (`|ln ratio|`), before and after, per family, with how many icons moved closer or further; with `--faces`, whether the fills a change adds are the artist's (nearest artist fill within 2 dE00, IoU of the two colour regions). `--before`/`--after` take a binary or a directory of SVGs (`--save` keeps both sides) |

```bash
cargo build --release -p inkvec-cli
python3 bench/theory/exact_raster.py
python3 bench/theory/strip_eval.py
python3 bench/theory/evidence_eval.py all
python3 bench/theory/inkvec_compare.py --exe target/release/inkvec [--mode fast]
python3 bench/theory/attribution.py --exe target/release/inkvec --tier 512ss
python3 bench/theory/params_diag.py --exe target/release/inkvec --tier 512ss
python3 bench/theory/naturality_diag.py --exe target/release/inkvec --tier 512ss
python3 bench/theory/oracle_cost.py --exe target/release/inkvec --tier 512ss
python3 bench/theory/design_prior.py [--set screen] [--part counts|geometry|grid_sim|ties_sim|all]
python3 bench/theory/design_stats.py --trace web --exe target/release/inkvec --out /tmp/web
python3 bench/theory/design_stats.py --compare artist /tmp/web:web --families
python3 bench/theory/amodal_eval.py --exe target/release/inkvec [--tier 512ss]
python3 bench/theory/human_gap.py --before old/inkvec --after target/release/inkvec --tier web [--faces]
python3 bench/theory/geom_calibration.py
python3 bench/theory/renderer_floor.py --fresh [--per-family 6] [--tiers 128ss,512ss]
python3 bench/theory/noise_profile.py [--per-family 6]
cargo build --release -p inkvec-trace --example evidence_calibrate
python3 bench/theory/floor_selfcal.py [--sizes 128,512] [--conditions exact,ss8,lat32,fresh,committed]
python3 bench/theory/floor_selfcal.py --sizes 400 --conditions web
```

Coordinates: these scripts put pixel `(i, j)` on `[i, i+1] × [j, j+1]`; inkvec puts pixel
centres on integers, so `inkvec_compare.py` shifts inkvec's points by `+½` before scoring.
Distances are to the true polygon (circles and ellipses are 8192-gons whose radius is
area-matched to the curve, within 1e-7 px). Points within 2 px of the frame are not scored:
the frame is not a boundary of the drawing.

Needs numpy and Pillow only, except `design_prior.py`, which also needs svgelements and,
for `--part geometry`, shapely and resvg-py (the harness's renderer);
`renderer_floor.py`, which needs svgelements, shapely, scipy and resvg-py; and
`noise_profile.py`, which needs scipy and resvg-py; and `human_gap.py`, which needs
svgelements, scikit-image and resvg-py.
