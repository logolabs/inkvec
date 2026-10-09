# Handoff: the 0.2.7 test and alpha work, as it stands for the release

Three commits landed on `main` after `7d2547e` (`b267edf`, `b56ff1f`, `206a1dd`): new test
suites, three changes to the tracer aimed at failing `bench/cases`, and budget updates. They
turned CI red. A follow-up kept what measured well, reverted what did not, and made the test
suites run on every CI platform. This file records what is in the tree now and why.

## Speed

Fast mode was never slowed down. Timed on the tracer alone (28 corpus icons at 512 px,
median of three runs), before and after these commits:

| | `7d2547e` | `206a1dd` | Fast output |
|---|---|---|---|
| Fast | 20.6 ms/icon | 20.2 ms/icon | byte-identical on all 28 |
| Quality | 607 ms/icon | 618 ms/icon | |

The ~5-7 s that `bench/cases.py` prints is the Python harness (8x rasterisation of the
truth and the trace, process start, SciPy checks), not the tracer, so it cannot compare
Fast with Quality.

## Tracer changes

| Change | Fixes case | Effect on the corpus (`bench/alpha_eval.py`) | Status |
|---|---|---|---|
| Near-opaque ink needs no interior (`native::TRANSLUCENT_BELOW = 0.98`) | `glyph_ring_bar` | all 1386 icons: 18 change, 11 better, 7 worse, means unchanged | **kept** |
| Tiered ordering of native palette modes (opaque before frequent) | `sawtooth_diag` | 360 icons: 39 change, 20 better, 19 worse; worst losses +64 % error (`simple-icons/delphi`, `simple-icons/glitch`, `noto-emoji/emoji_u1f432`); mean worse | **reverted** |
| Half-turn (180°) symmetry in `symmetry.rs` / `mirror_fit.rs` | `mirror_r` | none of 360 icons change | **reverted** |
| `Polyline::new` maps an infinite sigma to `MIN_SIGMA` | none | unreachable (every sigma source clamps), and it gives a point with no information the largest weight | **reverted** |

Why the half-turn was reverted rather than fixed:

* It needed a 0.08 px mean-deviation cut-off, applied to the half-turn only, to keep
  `junction_quad` passing. Without it that case fails in both modes: the label map of an
  off-centre junction is half-turn symmetric at pixel resolution, so enforcing the symmetry
  moved the true junction 0.27 px. The cut-off hid that; it did not fix it.
* With it, the Go package's test on macOS (wazero, arm64) panicked in `symmetry::enforce`
  with an index of 2147484096 into a 256-point edge. The same input traces identically before
  and after natively, so this is the runtime miscompiling the new code shape, like the float
  `select` bug `tools/wasm_float_select.py` works around on amd64. The revert restores the
  code that passed there.

## Tests

The new suites are kept: rotation and reflection equivariance of the fitters and primitive
fits, finite-difference checks of the boundary solve's gradients, degenerate geometry and
raster input, and line-art invariants. They found no defect that reaches a trace: the guards
that came with them (empty input to decimation and arc lengths, non-finite angles) cover
preconditions every caller already meets, and no output changed. What they add is a guard
on properties that already held. Notably, the
shipping fitter (`optimal_multimodel`) and `optimal_polygon` are exactly equivariant; the
two-pass reference `fit_path` is not (3 vs 6 segments on a rotated S-curve), and it does not
ship.

Made to run on every CI platform:

* `inkvec-cli` `test_sr_prepass_execution` and `test_restore_prepass_execution` ran a Python
  one-liner (the SR one needed Pillow, which no CI runner has). Both now use an in-process
  stand-in, reached through a `#[cfg(test)]` branch of `build_upscaler` / `build_restorer`.
* `test_run_cli_exit_codes` set `INKVEC_DUMP_MAP` mid-run. `inkvec_core::env` caches each
  variable for the process on first read, so that could turn every later trace in the
  binary into a map dump. The dump branch is no longer exercised there.
* `inkvec-server`'s two `--healthcheck` tests set and read `INKVEC_PORT` in parallel; they
  now hold a lock, and the failing one points at a closed port instead of 8080.
* `inkvec-fit`'s degenerate-input tests asserted wall-clock times under 50 ms; those
  assertions are gone (nothing in this repository reads the clock to decide anything).
* `inkvec-fit` had made `candidates`, `decimate` and `tangents` public for those tests,
  which broke `cargo doc` (public docs linking private items). They are crate-private again,
  and the three test files moved from `tests/` into `src/*_tests.rs`.

## Budget

`bench/quality_budget.json`: `cases:quality` 38 (37 + `glyph_ring_bar`), `cases:fast` 26
(unchanged). The coverage floors raised in `b267edf` and `test_functions` 1088 stand.

## Since: the open cases closed

On `main` after this handoff, every `bench/cases` case passes in Quality (42/42) and Fast
gains `mirror_r` (27/42):

* `sawtooth_diag`, `ribbon_w1.5`: the native palette reads a hairline as its opaque ink and
  drops a translucent ink that is only the coverage of its opaque twin
  (`native/palette.rs`), and a thin face of coverage is painted its ink (`native.rs`).
* `ribbon_w1`: the sub-pixel refinement reads a stroke too thin to cover a pixel from its
  coverage centroid and width (`planar::ridge_offset`).
* `mirror_r`: the half turn is a symmetry, and every symmetry is enforced only when the
  refined boundary confirms it (`symmetry::AGREEMENT_Z`) -- which is what had made the
  first half-turn attempt pull `junction_quad`'s junction onto the centre.

See `CHANGELOG.md` `[Unreleased]` for the measurements.
