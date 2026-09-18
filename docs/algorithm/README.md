# The inkvec pipeline

How a raster image becomes an SVG, stage by stage.

Every stage has two documents:

- **`NN-name.md`** — the technical reference, for someone about to change the code.
- **`NN-name.html`** — the same subject in plain language with diagrams, for someone who
  wants to understand what the tool does without reading Rust. Open it in a browser;
  `index.html` is the front door.

Beside them, [`constants.md`](constants.md) inventories every constant and threshold in the
system, where each number came from, and which of them look like they were fitted to one
example rather than derived or measured.

## The stages

| # | Stage | Source | Reference | Plain language |
|---|---|---|---|---|
| — | Overview | — | [00-overview.md](00-overview.md) | [00-overview.html](00-overview.html) |
| 1 | Intake | `inkvec-cli`, `inkvec-sr` | [01-intake.md](01-intake.md) | [01-intake.html](01-intake.html) |
| 2 | Coverage | `trace/coverage.rs` | [02-coverage.md](02-coverage.md) | [02-coverage.html](02-coverage.html) |
| 3 | Palette | `trace/color.rs` | [03-palette.md](03-palette.md) | [03-palette.html](03-palette.html) |
| 4 | Regions | `trace/lib.rs` | [04-regions.md](04-regions.md) | [04-regions.html](04-regions.html) |
| 5 | Gradients | `trace/gradient.rs` | [05-gradients.md](05-gradients.md) | [05-gradients.html](05-gradients.html) |
| 6 | Planar map | `trace/planar.rs` | [06-planar-map.md](06-planar-map.md) | [06-planar-map.html](06-planar-map.html) |
| 7 | Sub-pixel | `trace/planar.rs` | [07-subpixel.md](07-subpixel.md) | [07-subpixel.html](07-subpixel.html) |
| 8 | Boundary solve | `trace/boundary_opt.rs` | [08-boundary-solve.md](08-boundary-solve.md) | [08-boundary-solve.html](08-boundary-solve.html) |
| 9 | Decode | `trace/decode.rs` | [09-decode.md](09-decode.md) | [09-decode.html](09-decode.html) |
| 10 | Symmetry | `trace/symmetry.rs` | [10-symmetry.md](10-symmetry.md) | [10-symmetry.html](10-symmetry.html) |
| 11 | Curve fitting | `inkvec-fit` | [11-fitting.md](11-fitting.md) | [11-fitting.html](11-fitting.html) |
| 12 | Repair | `cli/rings.rs` | [12-repair.md](12-repair.md) | [12-repair.html](12-repair.html) |
| 13 | Emit | `cli/emit.rs` | [13-emit.md](13-emit.md) | [13-emit.html](13-emit.html) |

## The one idea

An anti-aliased pixel is not a blurry approximation of a shape. It is a **measurement** of
how much of that pixel the shape covers, and it comes with a computable uncertainty.
Reading it that way rather than thresholding it away is the difference between this tracer
and the ones that came before it, and it is why the pipeline can state how confident it is
about every boundary point it emits.

Everything downstream is model selection under a description-length cost:

```
cost = 0.5 * chi2 + lambda * params
```

where `chi2` is squared error scaled by that measured uncertainty, `params` counts the
numbers that will be written into the SVG, and `lambda = ln(extent / precision)`. Each
stage either sharpens the measurement or proposes a cheaper description of it.

## Keeping these documents honest

Every factual claim in these files is supposed to be traceable to code in this repository,
cited as `file.rs:LINE`. Where a document says a number was measured, the measurement
should be named — which corpus, which set, which date. Anything that could not be
established is marked `**Unverified:**` rather than smoothed over.

If you change a stage, change its `.md`. If you change a constant, change its row in
`constants.md`. The `.html` files carry no numbers that are not also in the `.md`, so they
only need revisiting when the *concept* changes.

## Related documents

- [`../DESIGN.md`](../DESIGN.md) — the original design rationale.
- [`../M0-BASELINE.md`](../M0-BASELINE.md) — the baseline measurements this project set out
  to beat.
- [`../research-notes/`](../research-notes/) — working notes from individual investigations.
- `bench/quality.py` — the code-quality ratchet, including the budget of long functions and
  untested modules.
