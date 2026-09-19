# Changelog

All notable changes to Inkvec are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project uses
[Semantic Versioning](https://semver.org/) once it reaches 1.0 (before that, the library
API in particular should be treated as unstable release to release).

## [Unreleased]

### Added

- **Language-binding foundation** (`docs/BINDINGS.md`). The `inkvec` crate is a small stable
  library API (`trace`, `trace_rgba`, `Options`, `Traced`, `Error`); `inkvec-ffi` is a C
  library (`inkvec_ffi`) with a cbindgen header, `include/inkvec.h`; `inkvec-py` is the `inkvec`
  Python package (PyO3, abi3 wheels for 3.9+). Options live once, in `inkvec::Options`, whose
  JSON Schema (`bindings/options.schema.json`) the bindings take their options through and
  their typed stubs are generated from; `bindings/contract/` holds the cases every binding must
  reproduce. Nothing is published yet.
- **npm package `@logolabs/inkvec`** (`packages/npm`). The WebAssembly build for JavaScript
  and TypeScript: one ES module entry for browsers, Node.js, Deno and Bun, and a threaded
  build at `@logolabs/inkvec/threads` (cross-origin isolated pages in a Web Worker, or Node.js
  `worker_threads`). `trace`, `traceRGBA` (canvas `ImageData`), `defaults`, `optionsSchema`;
  options go to the facade as JSON and their TypeScript types are generated from the schema.
  `crates/inkvec-wasm` gains `trace_json` / `trace_rgba_json` on the facade; the positional
  `trace` the web demo calls is unchanged. Not published yet.
- **Java package `com.logolabs:inkvec`** (`packages/java`). A JNA binding over `inkvec-ffi`'s
  C ABI, Java 8+: `Inkvec.trace`, `Inkvec.traceRgba`, `defaults`, `optionsSchema`; a generated,
  immutable `InkvecOptions` builder plus a raw-JSON-options overload on every method, and
  `InkvecException` subclasses per error kind. Options are generated from the schema by
  `bindings/codegen/java.py`. Placeholder group id `com.logolabs`; not published yet.
- **NuGet package `LogoLabs.Inkvec`** (`packages/dotnet`). The .NET binding: P/Invoke over
  `inkvec_ffi`, `netstandard2.0` (.NET Framework 4.6.1+, Unity) and `net8.0`
  (`LibraryImport` source generation there, `DllImport` on `netstandard2.0`). `Inkvec.Trace`,
  `TraceRgba`, `TraceFile`; options are the generated `InkvecOptions` (nullable properties,
  `null` meaning the tracer's own default) or a raw JSON string; errors are
  `InvalidImageException`, `InvalidOptionsException` and `InkvecInternalException`. Native
  libraries for `win-x64`, `win-arm64`, `linux-x64`, `linux-arm64`, `osx-x64` and
  `osx-arm64` ship under `runtimes/`. Not published yet.
- **Swift package** (`packages/swift`). `Inkvec.trace` (image data, a file URL, a `CGImage`,
  `UIImage` or `NSImage`), `Inkvec.traceRGBA`, raw-JSON variants of both, `defaults`,
  `optionsSchema`, `version`, `buildTarget`; errors are `InkvecError`. `InkvecOptions` is
  generated from the options schema (`bindings/codegen/swift.py`). Apple platforms get the C
  library as a static XCFramework (`packages/swift/scripts/build-xcframework.sh`) through a
  mirror repository, `logolabs/inkvec-swift`, that `.github/workflows/swift.yml` updates on a
  release tag when the repository opts in; Linux links `libinkvec_ffi` as a system library.
  The contract passes on x86_64 Linux; the macOS and iOS builds have not run yet. Not
  published yet.
- **Docker HTTP service** (`crates/inkvec-server`, `services/docker/Dockerfile`). `POST
  /trace` traces raw image bytes or a `multipart/form-data` `image` part to `image/svg+xml`;
  options come from a `?options=` query parameter, an `X-Inkvec-Options` header, generic query
  parameters typed against the schema, or a multipart `options` part, all handed to
  `inkvec::Options::from_json` unchanged -- no option is named in the service. `GET
  /options/schema`, `/options/defaults`, `/healthz`, `/version`, and a generated OpenAPI 3.1
  document at `/openapi.json` (`bindings/codegen/openapi.py`). Traces run on a blocking pool
  behind a concurrency limit (`INKVEC_MAX_CONCURRENCY`, `503 busy` past it) and a body-size
  cap (`INKVEC_MAX_BODY_BYTES`, `413`); the image is `rust:1.98-bookworm` building a
  distroless, non-root runtime. Not published or pushed anywhere by this repository.
- **Go module `github.com/logolabs/inkvec-go`** (`packages/go`). Pure Go, no cgo: the C ABI
  compiled to `wasm32-wasip1` (`tools/build_go_wasm.sh`), embedded and run by wazero, one
  module instance per concurrent call. `Trace`, `TraceRGBA`, `TraceJSON` (options passed
  through untouched), `Defaults`, `OptionsSchema`, errors matching `ErrInvalidImage`,
  `ErrInvalidOptions`, `ErrInternal`; the `Options` struct is generated from the schema
  (`bindings/codegen/golang.py`). `inkvec-ffi` gains `inkvec_alloc` / `inkvec_dealloc` on WASI
  only (not in the header), and `inkvec::build_target()` names that build `wasm32-wasip1`
  instead of `wasm32-unknown`, which it shared with the browser build although its output
  differs. Released through a mirror repository by `.github/workflows/go.yml`; not published
  yet.

### Changed

- **Transparency is traced natively, by default.** An ink is a colour and an opacity and the
  transparent ground is an ink, instead of the image being composited onto white first:
  holes stay holes, white artwork on a transparent ground traces, translucent panels keep
  `fill-opacity`, and a glow or fade is one gradient of `stop-color` and `stop-opacity`.
  Opaque input traces byte-for-byte as before. `--no-native-alpha` (or
  `INKVEC_NATIVE_ALPHA=0`; the option `native_alpha: false` in the language bindings)
  restores the old path. Dark-ground pixel error on the screen set 0.063 -> 0.0017.
- **`--cutout`** only matters with `--no-native-alpha` now.
- **Shape harmonization is held to the traced boundary.** A repeated shape takes the
  cluster's consensus geometry only where that stays within 0.1 px of where its own pixels
  put it and costs fewer parameters. A face that another face is drawn against — one punched
  out of the faces below it, or one with a translucent face in its hole — is never moved,
  so harmonizing can no longer open a gap onto a transparent ground; nor is a fitted circle
  or rounded rectangle. Screen set with the default flags: mean dE00 0.299 -> 0.148 (native
  transparency and this together), no icon above dE00 1.0, and the alpha-channel error of
  harmonized icons back to the unharmonized level (22 better, 0 worse).

### Fixed

- **Holes and outlines of fitted primitives.** A hole written as two half-circle arcs had its
  ends and radius rounded separately and bulged by up to 0.76 px; a face with a hole drew its
  own outline from the traced ring rather than its primitive. A ring with one hole of uniform
  width is written as one stroked shape.
- **The same input gives the same SVG on every machine.** Without `--time-budget`, the
  boundary solve still stopped on a fixed 1200 ms wall clock (and the opt-in decode stage on
  600 ms), so a slower CPU, a loaded CI runner or WebAssembly could write different bytes for
  the same image. Only a caller's time budget runs a clock now; otherwise the solve stops on
  its iteration count. No output changes on the 246-icon screen set or on 1024 px rasters;
  two of eight complex 2048 px emoji change, for at most 1.4 s more time.
- **Nested holes.** A hole inside another hole of the same face is no longer written twice
  (even-odd filled it back in).
- **White artwork on a transparent ground** under `--no-native-alpha` no longer traces to a
  white rectangle.
- **`--margin` on reduced input.** The margin was silently dropped whenever the SVG was
  presented at a larger size than it was traced at (`--max-dim` capped the input, or an exact
  pixel-block upscale was undone). The presented size now grows with the viewBox.

## [0.1.3] - 2026-09-18

### Fixed

- **Release archives.** Keep the default Intel-macOS archive in the release matrix while
  omitting its unsupported optional ONNX-Runtime restorer variant, which previously
  prevented the full multi-platform release from publishing.

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
