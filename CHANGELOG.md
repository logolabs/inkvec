# Changelog

All notable changes to Inkvec are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project uses
[Semantic Versioning](https://semver.org/) once it reaches 1.0 (before that, the library
API in particular should be treated as unstable release to release).

## [Unreleased]

## [0.1.2] - 2026-09-18

### Fixed

- **Published benchmark provenance.** Replace unreproducible pre-release figures and
  visuals with measurements tied to the pinned 0.1.1 release executable; regenerate the
  README assets and distributions from the recorded 21-case data.
- **Large-input intake.** Preserve exact-area resampling when the decode cap applies,
  avoiding a filter mismatch with the corpus's supersampled raster convention.

## [0.1.1] - 2026-09-18

### Fixed

- **Release verification.** Pin CI and release builds to Rust 1.98.0 so formatting,
  lint and benchmark measurements do not change when the rolling stable channel advances.
  The quality baseline now records the reproducible release build rather than an
  unrecoverable pre-release snapshot.
- **Release packaging.** Use GitHub's supported `macos-15-intel` runner for the Intel
  macOS archive.
- **Documentation checks.** Escape SVG element names in Rust documentation and refresh
  the generated third-party component notice.

## [0.1.0] - 2026-09-15

Initial public release.

### Added

- **Tracer core.** Reads PNG, JPEG, WebP, GIF, BMP and TIFF and writes SVG geometry
  positioned by the evidence in the pixels rather than a fixed tolerance: colour and
  bilevel (two-tone) artwork, stroked line art (`--strokes`, emitted as centerline
  strokes only when the drawing is genuinely stroked), and primitives (`<circle>`,
  ellipses, rounded rectangles) recognised and written directly instead of as curves.
- **Gradients.** Linear and radial fills are fitted where the evidence supports them,
  with bands that were originally one gradient merged back together.
- **Curve fitting** by dynamic programming over lines, cubics, arcs and whole primitives,
  and a planar map that shares boundaries between faces so an edge moves once for both
  sides.
- **CLI flag families**: `--restore <auto|on|off>` (trained-network cleanup before
  tracing), `--sr <auto|on|off>` (super-resolution pre-pass), `--lossy <auto|on|off>`
  (noise-aware intake), `--max-dim`, `--time-budget`, `--no-background`, `--minify`,
  among others — see `inkvec --help`.
- **Browser demo.** `crates/inkvec-wasm` compiles the same pipeline to WebAssembly; the
  static page in `web/` runs it entirely client-side, with a threaded build
  (`web/pkg-threads`) for faster tracing where cross-origin isolation is available.
- **Optional trained restorer** (`--restore`, feature `restore-model`): a small network
  run through ONNX Runtime that removes JPEG/WebP/AI-decoder damage before tracing.
  Alternate pure-Rust backends are available through Burn (`restore-burn` on CPU,
  `restore-wgpu` on GPU). The network weights (`restorer.onnx`) are not bundled with
  Inkvec; see the CLI's `--restore-weights` / `INKVEC_RESTORE_ONNX`.
- **SR pre-pass** (`--sr`) via the packaged Python tool (`tools/inkvec_sr`), or any
  external command through `--sr-command`, as a complementary way to clean up the same
  class of damaged input.

### Known limitations

- `--time-budget` is advisory: it does not bound how long tracing takes on noise-like
  input, only on well-behaved artwork.
- Decoding allocates the full raster before `--max-dim` is applied, so a very large image
  can use significant memory even though the trace itself runs at the capped size.
- The library API (the `inkvec-*` crates used as libraries, as opposed to the `inkvec`
  binary's CLI surface) is not yet stable and may change before 1.0.
