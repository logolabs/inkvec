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
| `oracle_cost.py` | the trace against the artist's own file on the input's pixels and the tracer's price per parameter: which icons the objective would rather have the artist's description for (a search error) and which it prefers the trace for (a question for the prior), `docs/theory/optimal.md` §6 |
| `geom_calibration.py` | the gate's geometric match (`geom`) against dE00 on a disc grown by known amounts in two colour contrasts: `geom` reads the displacement in either colour, dE00 reads the contrast |

```bash
cargo build --release -p inkvec-cli
python3 bench/theory/exact_raster.py
python3 bench/theory/strip_eval.py
python3 bench/theory/inkvec_compare.py --exe target/release/inkvec [--mode fast]
python3 bench/theory/attribution.py --exe target/release/inkvec --tier 512ss
python3 bench/theory/params_diag.py --exe target/release/inkvec --tier 512ss
python3 bench/theory/naturality_diag.py --exe target/release/inkvec --tier 512ss
python3 bench/theory/oracle_cost.py --exe target/release/inkvec --tier 512ss
python3 bench/theory/geom_calibration.py
```

Coordinates: these scripts put pixel `(i, j)` on `[i, i+1] × [j, j+1]`; inkvec puts pixel
centres on integers, so `inkvec_compare.py` shifts inkvec's points by `+½` before scoring.
Distances are to the true polygon (circles and ellipses are 8192-gons whose radius is
area-matched to the curve, within 1e-7 px). Points within 2 px of the frame are not scored:
the frame is not a boundary of the drawing.

Needs numpy and Pillow only.
