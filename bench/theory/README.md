# bench/theory

Experiments behind [`docs/theory/README.md`](../../docs/theory/README.md): boundary-point
readings scored against exactly known geometry.

| file | what it does |
|---|---|
| `exact_raster.py` | exact box-filter (area-coverage) rasteriser for polygons in float64, a port of font-rs's signed-area accumulation; `python3 exact_raster.py` runs its self-check against hand-computed areas |
| `strip_eval.py` | the ½-crossing, the column-sum mean and the column sum with the cubic correction, on exact renders of circles, ellipses and straight edges, noiseless, 8-bit and with added noise |
| `inkvec_compare.py` | runs the inkvec binary with `INKVEC_DUMP_CONTOUR` and scores the points stage 07 (`planar::measure_subpixel`) measured beside the estimators above |

```bash
cargo build --release -p inkvec-cli
python3 bench/theory/exact_raster.py
python3 bench/theory/strip_eval.py
python3 bench/theory/inkvec_compare.py --exe target/release/inkvec [--mode fast]
```

Coordinates: these scripts put pixel `(i, j)` on `[i, i+1] × [j, j+1]`; inkvec puts pixel
centres on integers, so `inkvec_compare.py` shifts inkvec's points by `+½` before scoring.
Distances are to the true polygon (circles and ellipses are 8192-gons whose radius is
area-matched to the curve, within 1e-7 px). Points within 2 px of the frame are not scored:
the frame is not a boundary of the drawing.

Needs numpy and Pillow only.
