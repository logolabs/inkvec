# Changelog

All notable changes to Inkvec are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project uses
[Semantic Versioning](https://semver.org/) once it reaches 1.0 (before that, the library
API in particular should be treated as unstable release to release).

## [Unreleased]

## [0.1.5] - 2026-09-21

### Added

- **Inkvec Studio Lite** (`studio/`): a desktop app for Windows, macOS and Linux, built
  on Tauri v2 over this repository's own engine. It turns a raster logo into an SVG and
  minimises SVGs you already have, and it measures what it did rather than asserting it:
  the quality report's colour difference is CIEDE2000 between the raster the tracer saw
  and a render of the SVG it wrote, not an estimate. A "what this trace could not
  recover" panel states, from measurements, what tracing lost — lettering that came back
  as outlines, a lossy source, strokes baked into fills, a continuous ramp, features
  below the speckle floor — and says so plainly when nothing was. Everything runs on the
  user's own machine: the only outbound requests the binary can make are the update
  check and the optional denoiser download, and both say exactly what they send.

  Three tabs (Vectorize, Minify SVG, Batch), a split comparison viewer with wipe, A/B and
  a pixel-grid detail mode, eighteen advanced controls taking their tooltip copy verbatim
  from the engine's own option documentation, brand-colour snapping, an export sheet with
  measured byte counts and a designed asset-pack README, and a share card rendered in the
  app. Dark and light themes at full token parity. Built and packaged on all three
  platforms by the new `studio` workflow, and released as `.deb`/`.rpm`/`.AppImage`,
  `.dmg` and NSIS `.exe`/`.msi` alongside the command-line archives.

  The preset tray takes saved presets as well as the seven built-ins, the palette copies
  itself out as CSS custom properties, and detail mode carries a callout naming the worst
  corner it measured and offering to go there. The app draws the Inkvec mark, mono,
  generated from `web/logo.svg` so the two cannot drift.

  The installers carry the `inkvec` command-line tool with them, and Settings can put it
  on `PATH` and add a *Vectorize with Inkvec Studio* entry to the image right-click menu
  — each row naming the exact path it wrote, with a Remove that takes it back out. On
  Windows the uninstaller removes both.

  `studio/src-tauri` is a cargo workspace of its own, excluded from the root one, so
  `cargo build --workspace` keeps working without a webview SDK.

- **`inkvec_trace::with_stage_sink`**: a thread-local hook that reports each pipeline
  stage boundary as it is passed, as a name and the milliseconds it took. `Stopwatch::mark`
  feeds it. Additive, a no-op when nothing is installed, and restores the previous sink on
  panic. It exists because a trace takes about a second and an embedder's only other
  options for that second are a bare spinner or a fabricated sequence of stages; the
  stage names are documented as explicitly unstable.

- **Documentation site** (`tools/build_site.py`): the repo's docs rendered to static HTML
  in the LogoLabs design system and published to GitHub Pages
  (logolabs.github.io/inkvec) by the new `docs` workflow — the 14-stage algorithm series
  plus its plain-language diagram edition, the pipeline explanation with rendered
  equations, design, bindings, limitations, results, changelog and project documents.

- **`inkvec-svgmin`** (`crates/inkvec-svgmin`): rewrite an existing SVG's paths as the
  fewest segments that draw the same picture, by the tracer's own minimum-description-length
  objective. Corners in the source are hard breaks and survive exactly; the tolerance is
  stated at a viewing size (`--tolerance 0.1 --judge 1024`); only `d` attributes change and a
  path that would not get smaller is left byte for byte. Whole subpaths that are circles,
  ellipses or rectangles are written as those elements, and the `d` text itself is
  minimised: relative or absolute per command, whichever is shorter, repeated letters and
  needless separators and leading zeros dropped, `H`/`V`/`S` where they say the same thing,
  and each number at the fewest decimals the tolerance allows. Everything that is not path
  data is shortened too — colours, numeric attributes, presentation attributes restating
  what is already inherited, `style="fill:…"` as an attribute, attributes shared by every
  child of a group moved onto the group, unreferenced ids, empty groups, comments,
  `<metadata>`, whitespace — by rules re-implemented from [SVGO](https://github.com/svg/svgo)
  (MIT; see NOTICE), with `--no-document` to turn them off. `<title>`, and any `<desc>`
  somebody wrote, are never removed. On 40 corpus artist files: 23.9% of the numbers and
  23.5% of the bytes removed at a mean dE00 of 0.0077 against the original (worst 0.036).
  On the tracer's own output the geometry gives up 5.8% — its emitter is already
  description-length minimal — but the bytes give up 22.1%. On a nine-file spread it beats
  SVGO's defaults on bytes, 33.5% to 29.7%, and running both beats either.
- **`--minify` re-encodes the path data** through that writer, taking about 9.6% off a
  trace with nothing rounded and no pixel changed. `inkvec-svgmin --bytes-only` is the
  same rewrite as a standalone tool, and `--decimals` there is how precision is spent for
  bytes on purpose (two decimals takes 18% instead of 9.6%, and moves pixels). Tens of
  milliseconds for a small icon, under a second for a curve-dense logo, 6.5 s for the
  corpus's heaviest file (213 KB, 8,608 segments).

- **The denoiser in the browser** (`web/denoise.js`, `web/worker.js`, `crates/inkvec-wasm`).
  The `--restore` pre-pass now runs on the Space, off by default and with the same three
  modes (`off`, `auto`, `on`). The page loads the same `restorer.onnx` from
  `Logolabs/inkvec-denoiser-001`, verified against the same SHA-256 the CLI checks, into
  ONNX Runtime Web (pinned 1.30.0): **WebGPU** where the browser has it, ONNX Runtime's
  WebAssembly kernels where it does not, and the result line says which ran. Measured
  against native ONNX Runtime on a 512-px JPEG, the WebAssembly kernels agree to 4.2e-7 —
  after the 8-bit quantisation the tracer reads, one channel of one pixel in 786,432 differs
  by one level.

  Nothing around the network is reimplemented in JavaScript. `inkvec-restore` gains
  `network_input` / `network_output` (the compositing, the pad to a multiple of 16, the crop,
  the quantisation and the extreme-snapping its in-process backends already did, now callable
  by a backend this crate cannot reach), and `crates/inkvec-wasm` exposes them, plus
  `inkvec_restore::decide` for the `auto` decision, on an `Intake` object the page holds
  across the round trip. `inkvec_cli::trace_image_sized` is split into `intake` and
  `trace_prepared` at exactly the seam the restorer sits in, so the browser denoises the
  raster the tracer will see — after `--max-dim`, after the unblock — rather than one the
  pipeline would go on to resample; ONNX Runtime Web's session is asynchronous where the
  pipeline is not, which is why it cannot be a `Restore` backend like the others.
  `inkvec::trace_rgba_restored` is the facade's version of the trace that follows, with soft
  intake forced as `--restore` forces it. The page also shows the denoised raster next to the
  input, and reports the residual when `auto` decides not to denoise.

  A browser whose only WebGPU adapter is a software one (SwiftShader, lavapipe) is treated as
  having no GPU: measured in a headless Chromium, a 512-px pass the WebAssembly kernels
  finished in about eight seconds had still not returned ten minutes into the software
  adapter.

  While a run is going the page shows its steps rather than one spinner: the weights, the
  denoiser pass and the trace, each with its own track and its own elapsed time, and `auto`'s
  opening trace named as the check it is. The weights are counted in bytes; the network pass
  and the trace report nothing until they are done, so their tracks sweep rather than
  inventing a percentage.
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
- **Composer package `logolabs/inkvec`** (`packages/php`). The PHP binding: the C ABI called
  in-process through ext-FFI, PHP 8.1+. `Inkvec::trace`, `traceFile`, `traceRgba`,
  `defaults`, `optionsSchema`, `version`, `buildTarget`; options are the generated `Options`
  (camelCase properties, `null` meaning the tracer's own default, `bindings/codegen/php.py`),
  an array keyed by the tracer's own names, or raw JSON; errors are `InvalidImageException`,
  `InvalidOptionsException`, `InternalException` and `LibraryException`. A PHP string goes to
  the tracer as it is, with no copy into an FFI buffer; the library is found through
  `Inkvec::useLibrary()`, `INKVEC_LIBRARY`, the package's `lib/` (filled by
  `vendor/bin/inkvec-fetch-library` from the C release archives), a sibling
  `target/release/`, or the system loader, and is refused unless its C ABI version matches.
  `Inkvec::preload()` registers it from an `opcache.preload` script, which is what makes a
  trace possible in a PHP-FPM request (`ffi.enable=preload`) and shares one open library
  across a pool. `tests/HeaderTest.php` holds the FFI declarations to `include/inkvec.h`.
  Released through a mirror repository by `.github/workflows/php.yml`; not published yet.
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

### Fixed

- **`--margin` on reduced input.** The margin was silently dropped whenever the SVG was
  presented at a larger size than it was traced at (`--max-dim` capped the input, or an exact
  pixel-block upscale was undone). The presented size now grows with the viewBox.

- **The release itself.** The first 0.1.5 tag run published no command-line archives and no
  installers: `bundle.category` in `tauri.conf.json` was `"Graphics"`, which is not one of
  the names Tauri's bundler accepts, so every Studio job on every platform built the app and
  then failed on `invalid category` — and the publish job waits on all of them. It is
  `"GraphicsAndDesign"` now. The Linux Studio jobs failed earlier still, in the link: ONNX
  Runtime's prebuilt needs glibc 2.38 and GCC 13's libstdc++, which Ubuntu 22.04 does not
  have, so those rows build on 24.04. The AppImage needed one more thing nobody had
  reached yet: `xdg-utils`, without which the bundler stops after writing the AppDir.

- **The release notes claimed a signature the builds do not have.** Nothing here is
  code-signed on any platform — no certificate is configured — but the notes said the
  Windows installer was signed by LogoLabs SRL. They now say it is unsigned, and give the
  first-run steps for Windows SmartScreen and macOS Gatekeeper next to the checksums that
  do establish provenance.

- **The npm publish.** `npm publish dist-npm/*.tgz` failed on the 0.1.5 tag with a git
  authentication error: npm reads a bare `dir/file.tgz` as the GitHub shorthand
  `owner/repo` and went looking for `ssh://git@github.com/dist-npm/<file>.tgz.git`. The path
  now starts with `./`, and the step checks that exactly one tarball was packed before
  publishing it.

- **The Java package is built from this repository, not from Maven Central.** The
  publish-to-Central job is gone: it could never have run, because `java.yml` is triggered
  by `workflow_run` and a `workflow_run` event's `github.ref` is the default branch even
  when the run it followed was a tag, so its `refs/tags/v` guard was false every time.
  Publishing would also need a verified namespace the placeholder group id `com.logolabs`
  does not have. `packages/java/README.md` and `docs/BINDINGS.md` now give the two commands
  that build the jar instead of a dependency block for an artifact that does not exist.
  The test job no longer looks for a hard-coded `inkvec-0.1.4.jar`, which every version
  bump turned into a path that is not there.

- **Studio CI packages the app instead of only compiling it.** `--no-bundle` is what let
  the category bug reach a tag: packaging was the one step no CI run had ever executed, on
  any platform. The `studio` workflow now builds the same bundle formats the release does
  — `deb`/`rpm`/`AppImage`, `app`/`dmg`, `nsis`/`msi` — and fails if a bundler produces
  nothing, so the next installer that will not build says so on the pull request.

- **Which system each download runs on**, measured rather than assumed. A new
  `.github/scripts/abi_floor.py` reads each built binary — the versioned symbols it imports
  on Linux (weak references reported but not counted, since the loader may leave them null),
  the minimum macOS in its Mach-O load commands — and fails the build if it asks for more
  than the floor that row declares. The command-line archives keep the glibc 2.17 floor they
  had; the macOS archives now pin their deployment target (10.12 Intel, 11.0 Apple silicon)
  instead of inheriting whatever rustc's default happens to be that release. The restorer
  flavour's own higher floor is stated in the archive's `RESTORER.txt` and in the README's
  download table, next to the plain archive that runs on far older systems.

## [0.1.4] - 2026-09-20

### Changed

- **Transparency is traced natively, by default.** An ink is a colour and an opacity and the
  transparent ground is an ink, instead of the image being composited onto white first:
  holes stay holes, white artwork on a transparent ground traces, translucent panels keep
  `fill-opacity`, and a glow or fade is one gradient of `stop-color` and `stop-opacity`.
  Opaque input traces byte-for-byte as before. `--no-native-alpha` (or
  `INKVEC_NATIVE_ALPHA=0`) restores the old path. Dark-ground pixel error on the screen set
  0.063 -> 0.0017.
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
