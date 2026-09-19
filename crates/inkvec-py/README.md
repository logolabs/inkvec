# inkvec (Python)

Inkvec reads a PNG, JPEG, WebP, GIF, BMP or TIFF and writes an SVG whose geometry is decided by
the evidence in the pixels: boundaries where the anti-aliasing says they are, a `<circle>` where
there is a circle, real gradients for smooth ramps, and a coordinate count chosen by minimum
description length. This package is the Rust tracer compiled as a Python extension: no runtime
dependencies, one abi3 wheel per platform for Python 3.9 and newer.

```sh
pip install inkvec
```

## Quick start

```python
import inkvec

traced = inkvec.trace("logo.png")               # a path, bytes, a file object,
open("logo.svg", "w").write(traced.svg)          # a Pillow image or a numpy array

traced = inkvec.trace(open("logo.png", "rb").read(), colors=16, no_background=True)
print(traced.width, traced.height, len(traced.svg))
str(traced)                                      # the SVG; Jupyter renders it inline
```

Raw pixels -- straight RGBA, 8 bits per channel, row-major -- go through `trace_rgba`:

```python
traced = inkvec.trace_rgba(rgba_bytes, width, height, no_background=True)

import numpy as np                               # (H, W), (H, W, 3) or (H, W, 4) uint8
traced = inkvec.trace_rgba(np.asarray(pil_image))

from PIL import Image                            # any mode; converted to RGBA
traced = inkvec.trace(Image.open("logo.webp"))
```

`trace_rgba` on a buffer gives exactly the SVG `trace` gives for a PNG holding the same pixels.
Pillow and numpy are optional: they are used only when you pass their objects.

## Options

Keyword arguments, all optional, with the command line's defaults. `inkvec.defaults()` returns
them and `inkvec.options_schema()` returns their JSON Schema; the type stubs give every one its
type, default and description in the editor.

<!-- inkvec:options:begin -->
| Option | Type | Default | Range | Meaning |
|---|---|---|---|---|
| `precision` | number | `0.1` | > 0 | Sets the description-length cost of a coordinate, in pixels: lambda = ln(extent / precision). Smaller values buy more detail with more coordinates. It does not set the digits written; coordinates are always written at 2 decimals. |
| `min_area` | number | `2.0` | > 0 | Discard features smaller than this area, in square pixels. |
| `colors` | integer | `64` | >= 1 and <= 4096 | Maximum palette size. |
| `merge` | number | `0.035` | >= 0 | OKLab distance below which two colours are treated as one ink. |
| `max_dim` | integer | `2048` | - | Inputs larger than this on their longer side, in pixels, are traced at this size and the SVG is written at the original size. Trace time grows with the pixel count. 0 means no cap. |
| `time_budget` | number | `0.0` | >= 0 | Advisory wall-clock budget, in seconds; 0 means none. Gradient-band merging stops at 60% of it and the boundary solve gets 25%; the output is still a correct trace, with more fills or a less polished outline. A nonzero budget makes the output depend on machine speed and load, so it is no longer reproducible. |
| `margin` | number | `0.0` | >= 0 | Transparent margin around the output, as a fraction of the larger side. The viewBox grows; the geometry does not move. |
| `no_background` | bool | `false` | - | Knock the background out: the face that covers the whole canvas is not painted, so the artwork sits on transparency. |
| `minify` | bool | `false` | - | No ids or groups, no trailing zeros. Same geometry, typically about a tenth smaller. |
| `native_alpha` | bool | `true` | - | Trace transparency natively: each ink is a colour and an opacity, and the transparent ground is an ink of its own, instead of the image being composited onto a matte first. Holes stay holes, white artwork on a transparent ground traces, glows and shadows stay translucent, and a fade is one gradient of colour and opacity. An opaque input traces the same either way. On by default, as on the command line (where the environment variable INKVEC_NATIVE_ALPHA=0 turns the default off); false composites onto a matte first, as releases up to 0.1.3 did. |
| `cutout` | bool | `false` | - | With native_alpha off, carry the input's transparency into the SVG: a face the source drew transparent becomes a hole, one drawn at a single opacity keeps it as fill-opacity, and white artwork on a transparent ground survives. Changes nothing for an opaque input, and nothing with native_alpha on (the default), which already carries the transparency out. |
| `content_units` | bool | `false` | - | Scale the fit tolerances with the raster, so a large, simple drawing gets the parameter count of a small one. Trades fidelity for parsimony: small squares can come back as circles and thin rings broken. |
| `harmonize` | bool | `true` | - | Shape harmonization (on by default): marks that repeat across the drawing are redrawn from one consensus geometry per cluster, which saves parameters. A mark takes the consensus only where that stays within 0.1 px of the boundary traced for it and costs fewer parameters; a face another face is drawn against, and a fitted circle or rounded rectangle, is never moved. Set it to false to skip the pass. |
| `harmonize_threshold` | number | `0.92` | >= 0 and <= 1 | Shape-equivalence threshold for harmonization: the outline similarity (IoU after affine normalisation) above which two marks count as the same shape. |
<!-- inkvec:options:end -->

An unknown name, a wrong type or an out-of-range value raises `inkvec.InvalidOptionsError`.
Undecodable input raises `inkvec.InvalidImageError`; a failure inside the tracer raises
`inkvec.InternalError`. All three derive from `inkvec.InkvecError`.

### Shape harmonization (on by default)

Marks that repeat across the drawing -- a run of identical tabs, segmented rings, tiled glyphs
-- are matched by affine-normalised outline similarity and redrawn from one consensus geometry
per cluster, which saves parameters. Each mark is held to its own evidence: it takes the
consensus only where that lands within 0.1 px of the boundary traced for it and costs fewer
parameters, and a face another face is drawn against, or a fitted circle or rounded rectangle,
is never moved. On the 246-icon screen set it changes 2 icons, both cheaper and neither worse.
(Releases up to 0.1.3 had no such guard; there it raised mean dE00 from 0.151 to 0.299.) To skip the pass, pass `harmonize=False`.

## Behaviour

* **Deterministic.** The same input and options give byte-identical SVG, whatever the number
  of threads, as long as `time_budget` is 0 (the default) and no `INKVEC_*` research
  environment variable is set. That holds per platform: a wheel for another platform can write
  the same drawing slightly differently (`inkvec.build_target()` says which one you have).
* **Threads.** The GIL is released while tracing, so other Python threads keep running, and
  concurrent calls from several threads are safe. The tracer itself runs in parallel on all
  cores.
* **Not included.** The command line's optional neural pre-passes (`--restore`, `--sr`) need
  model weights and an ML runtime and are not part of this package.

## Licence

Apache-2.0. Source, the command line, the C library and the other bindings:
<https://github.com/logolabs/inkvec>.
