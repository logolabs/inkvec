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
| `design_prior.py` | the human prior of `docs/theory/chain-representation.md` (R3), counted from the artists' own files with exact rational arithmetic: segment kinds, axis-aligned and 45° lines, smooth and sharp joins, numbers on the design grid by the role of their point, ties, repeated radii, stroke widths, layers, symmetry and repetition; and two simulations of the chain's decisions on the artists' numbers with added noise (empirical-Bayes grid inference with snapping, `--part grid_sim`; coordinate ties by exact one-dimensional clustering, `--part ties_sim`). Needs no binary |

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
python3 bench/theory/geom_calibration.py
python3 bench/theory/renderer_floor.py --fresh [--per-family 6] [--tiers 128ss,512ss]
python3 bench/theory/noise_profile.py [--per-family 6]
```

Coordinates: these scripts put pixel `(i, j)` on `[i, i+1] × [j, j+1]`; inkvec puts pixel
centres on integers, so `inkvec_compare.py` shifts inkvec's points by `+½` before scoring.
Distances are to the true polygon (circles and ellipses are 8192-gons whose radius is
area-matched to the curve, within 1e-7 px). Points within 2 px of the frame are not scored:
the frame is not a boundary of the drawing.

Needs numpy and Pillow only, except `design_prior.py`, which also needs svgelements and,
for `--part geometry`, shapely and resvg-py (the harness's renderer);
`renderer_floor.py`, which needs svgelements, shapely, scipy and resvg-py; and
`noise_profile.py`, which needs scipy and resvg-py.
