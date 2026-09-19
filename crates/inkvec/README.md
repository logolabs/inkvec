# inkvec

Inkvec reads a PNG, JPEG, WebP, GIF, BMP or TIFF and writes an SVG whose geometry is decided by
the evidence in the pixels: boundaries where the anti-aliasing says they are, a `<circle>` where
there is a circle, real gradients for smooth ramps, and a coordinate count chosen by minimum
description length.

This crate is the stable library API: the whole pipeline behind the `inkvec` command line
(crate `inkvec-cli`), behind two functions and one options struct. The C library and the
Python package are built on it.

```toml
[dependencies]
inkvec = "0.1"
```

```rust
let png = std::fs::read("logo.png")?;

let mut opts = inkvec::Options::default();   // the command line's defaults
opts.colors = 16;
opts.cutout = true;

let traced = inkvec::trace(&png, &opts)?;
std::fs::write("logo.svg", &traced.svg)?;
println!("{}x{}", traced.width, traced.height);

// Raw straight-RGBA8 pixels, row-major: byte-identical to tracing the same pixels as a PNG.
let traced = inkvec::trace_rgba(&pixels, width, height, &opts)?;
```

`Options` is `#[non_exhaustive]`: start from `Options::default()` and assign fields, or parse a
JSON object with `Options::from_json`. Errors are `inkvec::Error`: `InvalidImage`,
`InvalidOptions` (unknown field, wrong type, out of range) and `Internal` (the pipeline failed or
panicked; panics never escape).

## Options

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
| `cutout` | bool | `false` | - | Carry the input's transparency into the SVG: a face the source drew transparent becomes a hole, one drawn at a single opacity keeps it as fill-opacity, and white artwork on a transparent ground survives. Changes nothing for an opaque input. Off by default because over white it opens faint seams along shared edges; use it for artwork that will sit on anything but white. |
| `content_units` | bool | `false` | - | Scale the fit tolerances with the raster, so a large, simple drawing gets the parameter count of a small one. Trades fidelity for parsimony: small squares can come back as circles and thin rings broken. |
| `harmonize` | bool | `true` | - | Shape harmonization (on by default): marks that repeat across the drawing are redrawn from one consensus geometry per cluster, which saves parameters. The known cost is fidelity on fine-line art: on hairlines, thin rings and small rounded details the consensus can displace thin lines by about a pixel (on the 246-icon screen set mean dE00 0.151 off vs 0.299 on). Set it to false for such artwork. |
| `harmonize_threshold` | number | `0.92` | >= 0 and <= 1 | Shape-equivalence threshold for harmonization: the outline similarity (IoU after affine normalisation) above which two marks count as the same shape. |
<!-- inkvec:options:end -->

The same table, as JSON Schema, is `inkvec::options_schema_json()` and the committed
`bindings/options.schema.json` every language binding is generated from.

### Shape harmonization (on by default)

Marks that repeat across the drawing -- a run of identical tabs, segmented rings, tiled glyphs
-- are matched by affine-normalised outline similarity and redrawn from one consensus geometry
per cluster, which saves parameters. The known cost is fidelity on fine-line art: on hairlines,
thin rings and small rounded details the consensus can displace thin lines by about a pixel. On
the 246-icon screen set mean dE00 is 0.151 with it off and 0.299 with it on (63 icons worse, 4
better, 179 untouched). For such artwork set `opts.harmonize = false`.

## Guarantees

* **Deterministic**: same input and options, byte-identical SVG, whatever the thread count --
  as long as `time_budget` is 0 (the default) and no `INKVEC_*` research environment variable
  is set. That holds per build target (`inkvec::build_target()`): the platform maths library
  can make another target write the same drawing slightly differently.
* **Thread-safe**: call from any number of threads; the pipeline runs on rayon's global pool.
* **No I/O**: nothing is read, written, spawned or printed.

## Not included

The command line's optional neural pre-passes -- the trained restorer (`--restore`) and the
super-resolution pre-pass (`--sr`) -- need model weights, an ML runtime or an external process,
and are not part of this API.

## Licence

Apache-2.0. <https://github.com/logolabs/inkvec>
