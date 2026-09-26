# Language bindings

Inkvec is a Rust library first. Every other language reaches the same pipeline through one of
these, each a thin wrapper over the facade or the C ABI:

| Layer | Where | For |
|---|---|---|
| `inkvec` crate (the facade) | `crates/inkvec` | Rust; the API every binding calls |
| C ABI: `inkvec_ffi.dll` / `libinkvec_ffi.so` / `libinkvec_ffi.dylib`, static `inkvec_ffi.lib` / `libinkvec_ffi.a`, header `include/inkvec.h` | `crates/inkvec-ffi` | C, C++, and every language with a C FFI: Java (JNA), C# (P/Invoke), Swift, Ruby, PHP |
| Python package `inkvec` | `crates/inkvec-py` | Python 3.9+, abi3 wheels built with PyO3 and maturin |
| npm package `@logolabs/inkvec` | `packages/npm` over `crates/inkvec-wasm` | JavaScript and TypeScript: browsers, Node.js, Deno, Bun; a single-threaded and a threaded WebAssembly build |
| Maven package `com.logolabs:inkvec` | `packages/java` over the C ABI | Java 8+, a JNA binding |
| NuGet package `LogoLabs.Inkvec` | `packages/dotnet` over `crates/inkvec-ffi` | .NET: `netstandard2.0` (.NET Framework 4.6.1+, Unity) and `net8.0`, P/Invoke |
| HTTP service `inkvec-server`, as a Docker image | `crates/inkvec-server`, `services/docker/` | Any language, over HTTP: `POST` an image, get SVG back |
| Go module `github.com/logolabs/inkvec-go` | `packages/go` over the C ABI compiled to `wasm32-wasip1` | Go 1.25+, pure Go through wazero: no cgo, no native library |
| Swift package `Inkvec`, from the mirror `github.com/logolabs/inkvec-swift` | `packages/swift` over the C ABI | Swift 5.9+: macOS and iOS through a prebuilt XCFramework, Linux against `libinkvec_ffi` |
| Composer package `logolabs/inkvec` | `packages/php` over the C ABI | PHP 8.1+ through ext-FFI, the library called in-process; preloadable for PHP-FPM |

The WebAssembly crate (`crates/inkvec-wasm`) calls the facade: `trace_json` and
`trace_rgba_json` take the options as JSON, `default_options_json` and `options_schema_json`
read them back. Its older positional `trace` export is what the web demo (`web/worker.js`) still
calls; it is kept until the page moves over.

It also exposes the denoiser pre-pass for a page that runs the network itself, which the web
demo does through ONNX Runtime Web: `prepare` returns an `Intake` — the pipeline stopped where
the restorer sits — whose `denoiser_input` / `take_denoiser_output` are `inkvec_restore`'s own
`network_input` / `network_output`, whose `residual` is `inkvec_restore::decide`'s signal for
`--restore auto`, and whose `trace` finishes the job with soft intake forced. `trace` is
`prepare(...).traceOnce()` with nothing in between. `denoiser_model_url`,
`denoiser_model_sha256` and `denoiser_threshold` are the constants the CLI uses, so a page
cannot end up fetching or trusting a different model.

## One source of truth

Nothing about an option is written twice.

* **`inkvec::Options`** (`crates/inkvec/src/options.rs`) defines every option: its name, type,
  default (taken from the command line's `Args::default()`), doc comment and range. It derives
  `serde` and `schemars::JsonSchema`.
* **`bindings/options.schema.json`** is generated from it (JSON Schema draft 2020-12) and
  committed. Validation (`Options::validate`) reads its ranges back from the same schema, so the
  documented range and the enforced range are one attribute on the field.
* **Every binding takes options as a JSON object** and hands it to `Options::from_json`: the C ABI
  takes a JSON string, the Python package serialises its keyword arguments. No binding contains
  per-option code, so none of them change when an option does.
* **Typed surfaces are generated** from the schema by `bindings/codegen/`: the Python type stub,
  the npm package's TypeScript `Options`, the Go module's `Options` struct, and the options
  tables in this file and the READMEs.
* **`bindings/contract/`** holds inputs, options and what every binding must produce for them:
  the reported size or error everywhere, and the SVG byte for byte per build target.

### Adding, changing or removing an option

1. Add the field to `Options` in `crates/inkvec/src/options.rs`, with a doc comment (it becomes
   the description in every language) and, for a number, its range
   (`#[schemars(range(min = .., max = ..))]` or `#[schemars(extend("exclusiveMinimum" = 0))]`).
   Give it its default in `Default for Options` and map it in `Options::to_args`. The compiler
   refuses to build until both are done, and a unit test fails if the option reaches nothing.
2. `cargo test -p inkvec` -- regenerates `bindings/options.schema.json` and fails once so the
   change is seen; run it again and it passes.
3. `python bindings/codegen/generate.py` -- regenerates the Python stub, the TypeScript types
   and the options tables.
4. If the change alters output for the contract inputs:
   `INKVEC_BLESS=1 cargo test -p inkvec --release --test contract`.
5. Commit. The C header, the C library's API, the Python module and every other binding are
   untouched.

CI fails on any stale artifact: the schema test, `python bindings/codegen/generate.py --check`,
and `cbindgen --verify` for the header.

### Adding a language

A binding for a new language needs three things, all driven by files in this repository:

1. **Calls.** Wrap the C ABI (`include/inkvec.h`) or, for WebAssembly, the facade. Pass options as
   one JSON object; do not restate them.
2. **Types.** Add a generator to `bindings/codegen/` -- a module with
   `render(options) -> {path: text}` that reads the options through `schema.load()` (see
   `python_stub.py`) -- and list it in `generate.py`. `schema.load()` refuses an option type it
   has not been taught, so a new kind of option fails loudly rather than rendering wrongly.
3. **Tests.** Run every case in `bindings/contract/cases.json` and compare with its `expect`
   block.

## The shared contract

### Input

Two forms, in every language:

* **Encoded image**: the bytes of a PNG, JPEG, WebP, GIF, BMP or TIFF file. The container is read
  as well as the pixels: JPEG and lossy WebP are traced with the noise-aware intake the command
  line uses for them.
* **Raw pixels**: straight (not premultiplied) RGBA, 8 bits per channel, row-major, tightly
  packed, exactly `width * height * 4` bytes. The result is byte-identical to the encoded form on
  a PNG holding the same pixels.

### Options

All optional; a missing option takes its default, which is the command line's. An unknown name,
a value of the wrong type, a non-finite number or a value out of range is an `invalid_options`
error naming the option.

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
| `editability` | bool | `false` | - | Spend parameters on structure an artist can edit: joins between curves made G1-smooth, handles snapped to the axes and to 45 degrees, handles of one curve made equal in length, nodes that nearly share a coordinate made to share it, and rings that are their own mirror image locked into exact mirrors. Every change is guarded to the fit's own tolerance -- 3 sigma of the source point plus half a pixel, or 1.5 px for a mirror lock -- so the picture stays within a fraction of a pixel of the default trace; the price measured on 25 icons is about 0.04 dE00. Off by default. |
| `native_alpha` | bool | `true` | - | Trace transparency natively: each ink is a colour and an opacity, and the transparent ground is an ink of its own, instead of the image being composited onto a matte first. Holes stay holes, white artwork on a transparent ground traces, glows and shadows stay translucent, and a fade is one gradient of colour and opacity. An opaque input traces the same either way. On by default, as on the command line (where the environment variable INKVEC_NATIVE_ALPHA=0 turns the default off); false composites onto a matte first, as releases up to 0.1.3 did. |
| `cutout` | bool | `false` | - | With native_alpha off, carry the input's transparency into the SVG: a face the source drew transparent becomes a hole, one drawn at a single opacity keeps it as fill-opacity, and white artwork on a transparent ground survives. Changes nothing for an opaque input, and nothing with native_alpha on (the default), which already carries the transparency out. |
| `content_units` | bool | `false` | - | Scale the fit tolerances with the raster, so a large, simple drawing gets the parameter count of a small one. Trades fidelity for parsimony: small squares can come back as circles and thin rings broken. |
| `harmonize` | bool | `true` | - | Shape harmonization (on by default): marks that repeat across the drawing are redrawn from one consensus geometry per cluster, which saves parameters. A mark takes the consensus only where that stays within 0.1 px of the boundary traced for it and costs fewer parameters; a face another face is drawn against, and a fitted circle or rounded rectangle, is never moved. Set it to false to skip the pass. |
| `harmonize_threshold` | number | `0.92` | >= 0 and <= 1 | Shape-equivalence threshold for harmonization: the outline similarity (IoU after affine normalisation) above which two marks count as the same shape. |
| `merge_colors` | string | `""` | - | Colour groups: fills to draw as one, so the shapes between them join rather than being recoloured. Empty (the default) changes nothing. Groups are separated by ';' and members by ','; a member is a colour '#rrggbb' as it appears in a trace of the same image, or a gradient written as its stop colours joined by '>'. An optional '=' says what the group becomes: '=#rrggbb' a flat colour, '=@n' its n-th member (1-based; a gradient there is refitted over the whole group); without it, the member covering the most of the image. Example: '#c0392b,#e74c3c;#f00>#00f,#0a0=@1'. A group costs one extra trace. |
<!-- inkvec:options:end -->

### Output

The SVG document (UTF-8) and the input's width and height in pixels. The SVG's `width` and
`height` attributes carry the same numbers unless `margin` grew them. Its `viewBox` can be a
smaller coordinate space than the presented size when the input was reduced for tracing
(`max_dim`, or an exact pixel-block upscale that was undone).

### Errors

| Kind | Rust | C status | Python |
|---|---|---|---|
| `invalid_image` -- not a decodable image, or pixels that do not match the size given | `Error::InvalidImage` | `INKVEC_ERR_INVALID_IMAGE` (2) | `InvalidImageError` |
| `invalid_options` -- unknown option, wrong type, out of range | `Error::InvalidOptions` | `INKVEC_ERR_INVALID_OPTIONS` (3) | `InvalidOptionsError` |
| `internal` -- the pipeline failed or panicked | `Error::Internal` | `INKVEC_ERR_INTERNAL` (4) | `InternalError` |
| invalid call -- NULL pointer, `struct_size` too small | - | `INKVEC_ERR_INVALID_ARGUMENT` (1) | `TypeError` |

Every error carries a human-readable message. No panic crosses a language boundary.

### Threading

Every entry point may be called from any number of threads at once. The pipeline is itself
parallel (rayon, one global pool per process). Python releases the GIL for the whole trace.

### Determinism

On one build target, the same input and options give byte-identical SVG across runs, thread
counts and languages, with two exceptions:

* a nonzero `time_budget` stops stages by wall clock, so the output depends on machine speed
  and load;
* a release build still reads a few `INKVEC_*` environment variables: diagnostics, and A/B
  switches the benchmark scripts use to turn a shipped stage off (`INKVEC_BOPT=0`,
  `INKVEC_NO_CARVE=1`, ...). They are listed in `docs/internal/env-vars.md`; leave them unset.

**Across build targets the bytes can differ.** The pipeline takes a few transcendental functions
(cube roots, trigonometry, logarithms) from the platform's maths library, whose last-bit
rounding differs between, for example, glibc and the MSVC runtime, and a last-bit difference can
tip a near-tie downstream. Measured on the 96 px contract sample: `x86_64-linux-gnu` and
`x86_64-windows-msvc` write the same shapes, but one compound path lists its subpaths in a
different order (9003 bytes against 8991); the two 64 px samples come out identical. Every
binding reports the target it was built for -- `inkvec::build_target()`,
`inkvec_build_target()`, `inkvec.build_target()` -- and the contract records hashes per target.
The maths library is the system's C runtime (glibc, the Windows UCRT, Apple's libm), loaded at
run time, so two versions of it could in principle differ as well; that has not been measured. The committed
`x86_64-linux-gnu` hashes were recorded on Debian 12 (glibc 2.36).

### Transparency (native, on by default)

Every ink is a colour and an opacity, and the transparent ground is an ink of its own, so holes
stay holes, a translucent panel keeps its `fill-opacity`, white artwork on a transparent ground
traces, and a glow or fade is one gradient of `stop-color` and `stop-opacity`. An opaque input
traces the same either way. `native_alpha` set to false composites onto a matte first, as
releases up to 0.1.3 did; `cutout` matters only then. The default is the command line's, so,
as there, the environment variable `INKVEC_NATIVE_ALPHA=0` turns it off in a process that sets
it (and makes `cargo test -p inkvec` report the schema as changed).

### Shape harmonization (on by default)

Marks that repeat across the drawing -- a run of identical tabs, segmented rings, tiled glyphs
-- are matched by affine-normalised outline similarity (`harmonize_threshold`, default 0.92) and
redrawn from one consensus geometry per cluster, which saves parameters. Each mark is held to
its own evidence: it takes the consensus only where that lands within 0.1 px of the boundary
traced for it and costs fewer parameters, and a face another face is drawn against, or a fitted
circle or rounded rectangle, is never moved. On the 246-icon screen set it changes 2 icons, both
cheaper and neither worse. Releases up to 0.1.3 had no such guard, and there it raised mean
dE00 on that set from 0.151 to 0.299. Set `harmonize` to false to skip the pass.

### Not included

The command line's optional neural pre-passes -- the trained restorer (`--restore`) and the
super-resolution pre-pass (`--sr`) -- need model weights and an ML runtime or an external
process, and are in no binding. Neither are the command line's research switches (`--tau`,
`--bilevel`, `--strokes`, `--layers`, ...); each becomes available everywhere by adding it to
`Options` as above.

## Quick starts

### Rust

```toml
[dependencies]
inkvec = "0.1"
```

```rust
let png = std::fs::read("logo.png")?;
let mut opts = inkvec::Options::default();
opts.colors = 16;
let traced = inkvec::trace(&png, &opts)?;
std::fs::write("logo.svg", &traced.svg)?;

// Options as JSON, validated, exactly as the other bindings pass them:
let opts = inkvec::Options::from_json(r#"{"colors": 16, "no_background": true}"#)?;
let traced = inkvec::trace_rgba(&rgba, width, height, &opts)?;
```

`Options` is `#[non_exhaustive]`: start from `Options::default()` and assign fields.

### C

Link against the library from a release archive (or `cargo build --release -p inkvec-ffi`, which
writes `target/release/inkvec_ffi.dll` + `inkvec_ffi.dll.lib`, or `libinkvec_ffi.so` /
`libinkvec_ffi.dylib`, and the static `inkvec_ffi.lib` / `libinkvec_ffi.a`), with
`crates/inkvec-ffi/include` on the include path. The library is `inkvec_ffi` rather than
`inkvec` because on Windows its debug-symbol file would collide with the command line's
`inkvec.exe` in one build directory.

```c
#include "inkvec.h"

InkvecResult r = INKVEC_RESULT_INIT;        /* zeroed, struct_size recorded */
int status = inkvec_trace(png, png_len, "{\"colors\": 16}", &r);
if (status == INKVEC_OK)
    fwrite(r.svg, 1, r.svg_len, out);
else
    fprintf(stderr, "inkvec: %s\n", r.error);
inkvec_result_free(&r);                      /* always, success or not */
```

* Options: a JSON object, or NULL / `""` for the defaults. `inkvec_options_schema()` and
  `inkvec_default_options()` return the schema and the defaults as static JSON strings.
* Ownership: `r.svg` and `r.error` belong to the library until `inkvec_result_free`. The strings
  from `inkvec_version`, `inkvec_build_target`, `inkvec_options_schema` and
  `inkvec_default_options` are static.
* Forward compatibility: `struct_size` lets a later library append fields to `InkvecResult`
  without writing past an older caller's struct. `inkvec_abi_version()` changes only on a
  breaking change.
* Linking the static library needs the platform's system libraries as well. Measured with
  `cargo rustc --release -p inkvec-ffi --crate-type staticlib -- --print native-static-libs`:
  on x86_64 Linux `-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc`, on Windows (MSVC)
  `kernel32.lib ntdll.lib userenv.lib ws2_32.lib dbghelp.lib`. Run the same command for any other
  target.
* `inkvec_build_target()` names the target the library was built for (see Determinism), and
  `inkvec_abi_version()` the ABI.

A complete program is `crates/inkvec-ffi/examples/c/trace.c`.

### Python

```sh
pip install inkvec
```

```python
import inkvec

traced = inkvec.trace("logo.png", colors=16)                # path, bytes, file, PIL, numpy
open("logo.svg", "w").write(traced.svg)

traced = inkvec.trace_rgba(rgba_bytes, width, height)       # or a numpy (H, W, 4) array
inkvec.defaults()          # every option at its default
inkvec.options_schema()    # the JSON Schema
```

The keyword arguments are typed in the stub (`inkvec/__init__.pyi`, generated). Errors derive
from `inkvec.InkvecError`.

### JavaScript and TypeScript

```sh
npm install @logolabs/inkvec
```

```js
import { trace, traceRGBA, defaults, optionsSchema } from "@logolabs/inkvec";

const svg = await trace(bytes, { colors: 16 });                 // Uint8Array, Buffer, ArrayBuffer, Blob
const svg2 = await traceRGBA(ctx.getImageData(0, 0, w, h));   // or (pixels, width, height, options)
```

One ES module entry for browsers, Node.js, Deno and Bun; `@logolabs/inkvec/threads` is the
threaded build (cross-origin isolated pages inside a Web Worker, or Node.js `worker_threads`).
The option names are the schema's, typed by the generated `Options` interface. Errors are
`InkvecError` with `code` set to the error kind, plus `load_failed` for a WebAssembly module
that could not be loaded. See `packages/npm/README.md`.

### Java

```sh
cargo build --release -p inkvec-ffi     # then: mvn -f packages/java/pom.xml install
```

```java
TraceResult r = Inkvec.trace(png, InkvecOptions.builder().colors(16).build());
System.out.println(r.svg());
```

A [JNA](https://github.com/java-native-access/jna) binding over the C ABI (`inkvec_ffi`):
Java 8 or later, one dependency. `InkvecOptions` is a generated, immutable builder; every
method also takes options as a raw JSON string. Errors are `InkvecException` subclasses
(`InvalidImageException`, `InvalidOptionsException`, `InternalException`). Not published to
Maven Central, and not planned to be: build it from the repository, or take the jar the
`java` workflow attaches to its runs, which carries all five platforms' native libraries.
See `packages/java/README.md`.

### C#

```sh
dotnet add package LogoLabs.Inkvec
```

```csharp
using LogoLabs.Inkvec;

var traced = Inkvec.TraceFile("logo.png", new InkvecOptions { Colors = 16 });
File.WriteAllText("logo.svg", traced.Svg);
```

P/Invoke over the C ABI, `netstandard2.0` (.NET Framework 4.6.1+, Unity) and `net8.0`
(`LibraryImport` source generation there, `DllImport` on `netstandard2.0`, one shared
declarations file). `InkvecOptions` is generated from the schema, same as every other
binding's typed surface; a raw-JSON overload of `Trace`/`TraceRgba` reaches an option before
it has been regenerated for. Errors are `InvalidImageException`, `InvalidOptionsException` and
`InkvecInternalException`, all deriving from `InkvecException`. See `packages/dotnet/README.md`.

### Swift

```swift
// Package.swift: .package(url: "https://github.com/logolabs/inkvec-swift", from: "0.1.3")
import Inkvec

let traced = try Inkvec.trace(contentsOf: url, options: InkvecOptions(colors: 16))
let fromPixels = try Inkvec.traceRGBA(rgba, width: w, height: h)    // or a CGImage, UIImage, NSImage
let raw = try Inkvec.trace(png, optionsJSON: #"{"no_background": true}"#)
```

`packages/swift` wraps the C ABI: options are encoded to one JSON object, errors are
`InkvecError` (`invalidImage`, `invalidOptions`, `internal`). `InkvecOptions` -- optional
camelCase properties, `nil` meaning the default -- and the package's copy of `inkvec.h` are
generated by `bindings/codegen/swift.py`. Apple platforms (macOS 10.15+, iOS 13+) get the
library as a static XCFramework from the mirror `github.com/logolabs/inkvec-swift`, whose
manifest `packages/swift/scripts/render_mirror.py` writes; on Linux the same package links
`libinkvec_ffi` as a system library. The tests run the contract through the typed and the
raw entry points. See `packages/swift/README.md`.

### Go

```sh
go get github.com/logolabs/inkvec-go
```

```go
traced, err := inkvec.Trace(ctx, png, &inkvec.Options{Colors: inkvec.Ptr(16)})
traced, err = inkvec.TraceRGBA(ctx, img.Pix, w, h, nil)          // straight RGBA8
traced, err = inkvec.TraceJSON(ctx, png, `{"colors": 16}`)       // options passed through
errors.Is(err, inkvec.ErrInvalidOptions)                          // ErrInvalidImage, ErrInternal
```

Pure Go: the C ABI compiled to `wasm32-wasip1` by `tools/build_go_wasm.sh`, embedded
(`//go:embed`) and run by wazero, one module instance per concurrent call; no cgo. The
`Options` struct (pointer fields, nil for the default) is generated by
`bindings/codegen/golang.py`. One trace runs on one core, about twice the native
single-threaded time. The build rewrites the module's floating-point `select`s as integer
ones (`tools/wasm_float_select.py`) around a wazero amd64 compiler bug that returned the wrong
operand. `go get` cannot run a build step, so users get the module from the mirror repository
`github.com/logolabs/inkvec-go`, which `.github/workflows/go.yml` fills from `packages/go` with
the compiled `inkvec.wasm` on a `v*` tag; here the `.wasm` is built, not committed. See
`packages/go/README.md`.

### PHP

```sh
composer require logolabs/inkvec
vendor/bin/inkvec-fetch-library      # libinkvec_ffi for this platform, from the releases
```

```php
use LogoLabs\Inkvec\Inkvec;
use LogoLabs\Inkvec\Options;

$traced = Inkvec::traceFile('logo.png', new Options(colors: 16));
file_put_contents('logo.svg', $traced->svg);

$traced = Inkvec::trace($bytes, ['colors' => 16]);            // options as an array
$traced = Inkvec::traceRgba($rgba, $width, $height);          // straight RGBA8
```

`packages/php` calls the C ABI through PHP's FFI extension: no subprocess and no copy of the
image on the way in (a PHP string is passed straight to `inkvec_trace`). The library is found
through `Inkvec::useLibrary()`, `INKVEC_LIBRARY`, the package's own `lib/`, a sibling
`target/release/`, or the system loader, and is refused unless its `inkvec_abi_version()`
matches. Options are the generated `Options` (camelCase properties, `null` meaning the
default, `bindings/codegen/php.py`), an array keyed by the tracer's own names, or raw JSON;
errors are `InvalidImageException`, `InvalidOptionsException`, `InternalException` and
`LibraryException`, all `InkvecException`. In a web SAPI, where `ffi.enable` defaults to
`preload`, `Inkvec::preload()` registers the library from an `opcache.preload` script and
every request shares it. See `packages/php/README.md`.

### HTTP (Docker)

`crates/inkvec-server` is the facade behind axum: `POST /trace` (raw image bytes, or
`multipart/form-data` with an `image` part), options as `?options=<JSON>`, an
`X-Inkvec-Options` header, generic query parameters typed against the schema below, or a
multipart `options` part -- all converted mechanically and handed to
`inkvec::Options::from_json`, same as every other binding. `GET /options/schema`,
`/options/defaults`, `/healthz`, `/version` and `/openapi.json` (an OpenAPI 3.1 document
generated by `bindings/codegen/openapi.py` from `bindings/options.schema.json`, embedded in
the binary) round it out. `services/docker/Dockerfile` builds it as a distroless, non-root
container; see `crates/inkvec-server/README.md` for endpoints, configuration and a curl
walkthrough. Not published anywhere; nothing here is pushed by this repository automatically
(see "Publishing" below).

## Versions

There is one version: `workspace.package.version` in the root `Cargo.toml`. Every crate inherits
it, maturin reads it for the Python wheel (`dynamic = ["version"]`), `inkvec_version()` and
`inkvec.__version__` return it, the C release archives are named after the tag, and
`packages/npm/build.mjs` writes it into the npm package's `package.json`. A binding added later
(Maven, NuGet, ...) should read it the same way rather than carry its own.

The files that cannot read it hold a copy: the npm package (`packages/npm/build.mjs`), the
Maven POM (`packages/java/build.py`), the OpenAPI document (`bindings/codegen/generate.py`),
both wasm-pack packages under `web/`, Inkvec Studio's manifests (its `src-tauri` is a
workspace of its own) and both `Cargo.lock` files. `python tools/check_versions.py` lists every
copy that disagrees with `Cargo.toml` and names the tool that rewrites it; CI runs it, and the
release job runs it with `--tag`.

## The contract fixtures

`bindings/contract/`:

* `tiny.png` (96 x 96, opaque) and `white_on_clear.png` (64 x 64, white on transparent), with
  their pixels as raw RGBA8 in `tiny.rgba` and `white_on_clear.rgba`. `make_fixtures.py`
  rewrites them; they change rarely.
* `cases.json`: each case names an input, a form (`encoded` or `rgba`) and options, and records

  * `expect` -- what every build must report: the width and height, or `error`, the kind of
    error;
  * `svg` -- per build target, the SVG's length in bytes and SHA-256. A binding built for a target
    with an entry must reproduce it exactly; on a target without one it checks everything else;
  * `same_svg_as` (optional) -- a case whose SVG must be identical on every target: raw pixels and
    the PNG holding them, capped or not.

  The cases are written by hand; `expect` and `svg` are produced by the Rust facade.

After a deliberate change to the tracer's output, regenerate the expectations with one command:

```sh
INKVEC_BLESS=1 cargo test -p inkvec --release --test contract
```

It rewrites `expect` and this target's hashes and drops every other target's, which the change
made stale. To record another target's hashes (its output unchanged), run the same test there
with `INKVEC_BLESS=add`; CI uploads a `cases.json` blessed this way from each platform it tests.
`INKVEC_CONTRACT_REQUIRE_HASH=1` turns a target without recorded hashes from a note into a
failure. The committed hashes cover `x86_64-windows-msvc`, `x86_64-linux-gnu`,
`wasm32-unknown` -- both WebAssembly builds, recorded from the npm package with
`INKVEC_BLESS=add node --test test/contract.test.mjs` in `packages/npm` -- and `wasm32-wasip1`,
recorded from the Go module with `INKVEC_BLESS=add go test -run TestContract` in `packages/go`
(the same as `wasm32-unknown` on 8 of the 12 traced cases; the four uncapped 96 px ones
differ).

The Rust facade (`crates/inkvec/tests/contract.rs`), the C ABI from Rust
(`crates/inkvec-ffi/src/lib.rs` tests) and from a foreign caller (`crates/inkvec-ffi/tests/test_c_abi.py`,
through ctypes, plus the compiled C example), the Python package
(`crates/inkvec-py/tests/test_inkvec.py`) and the npm package, on both of its builds
(`packages/npm/test/contract.test.mjs`) all assert against it.

## Building and testing locally

```sh
cargo test --release -p inkvec -p inkvec-ffi            # facade, schema drift, contract, C ABI
python bindings/codegen/generate.py --check             # generated stubs and tables current

cargo build --release -p inkvec-ffi                     # the C library
cbindgen --config crates/inkvec-ffi/cbindgen.toml --crate inkvec-ffi \
         --output crates/inkvec-ffi/include/inkvec.h    # the header (add --verify to check)
python -m pytest crates/inkvec-ffi/tests                # the C ABI through ctypes

pip install maturin
maturin build --release -m crates/inkvec-py/Cargo.toml  # abi3 wheel in target/wheels
pip install target/wheels/inkvec-*.whl pytest pillow numpy
python -m pytest crates/inkvec-py/tests

cd packages/npm && npm ci                               # the npm package (needs wasm-pack and
node build.mjs && npm test                              # a nightly toolchain with rust-src)

cd packages/php && composer install                     # the PHP package (needs ext-ffi and
vendor/bin/phpunit                                      # the C library built above)
```

`.github/workflows/bindings.yml` does all of this on Linux, macOS and Windows and builds the
release artifacts: the C library for windows-x64, linux-x64, linux-arm64, macos-arm64 and
macos-x64, abi3 wheels for the same targets plus an sdist, and the `LogoLabs.Inkvec` nupkg
carrying whichever of those native libraries the C library job produced.

## Publishing

Nothing is published automatically. Every publishing job in `bindings.yml` runs only on a `v*`
tag, and the registry ones only when the repository opts in.

| Registry | Name | Needs |
|---|---|---|
| crates.io | `inkvec` and the internal crates it depends on | the repository variable `PUBLISH_CRATES=true` and a crates.io API token as the secret `CARGO_REGISTRY_TOKEN`. `cargo publish --workspace` publishes in dependency order: `inkvec-core`, `inkvec-fit`, `inkvec-trace`, `inkvec-sr`, `inkvec-restore`, `inkvec-cli`, `inkvec` (all seven pass `cargo publish --workspace --dry-run`). `inkvec-ffi`, `inkvec-py` and `inkvec-wasm` are `publish = false`. |
| PyPI | `inkvec` | the repository variable `PUBLISH_PYPI=true` and this repository and workflow (environment `pypi`) registered as a trusted publisher of the PyPI project; or an API token as the secret `PYPI_API_TOKEN`, passed as `password:` to the publish step |
| GitHub Releases | C library archives `inkvec-c-<version>-<platform>` | nothing beyond the workflow's own `GITHUB_TOKEN` |
| Go (mirror repository) | `github.com/logolabs/inkvec-go` | the repository `logolabs/inkvec-go` with a `main` branch, and the private half of an SSH deploy key with write access to it as the secret `GO_MIRROR_DEPLOY_KEY`; `.github/workflows/go.yml` copies `packages/go`, the built `inkvec.wasm`, the licence and the contract fixtures there on a `v*` tag matching `Cargo.toml`, commits and tags it |
| npm | `@logolabs/inkvec` | an npm organisation `logolabs` (the scope) and an automation token with publish rights to it as the secret `NPM_TOKEN`; `.github/workflows/npm.yml` publishes the tarball it built and tested, with provenance, on a `v*` tag whose version matches `package.json` |
| NuGet | `LogoLabs.Inkvec` | the repository variable `PUBLISH_NUGET=true` and a NuGet.org API key as the secret `NUGET_API_KEY`; the `dotnet-pack` job in `bindings.yml` gathers the native libraries `c-library` built for every platform into one nupkg first |
| Packagist (mirror repository) | `logolabs/inkvec` | Composer publishes from a repository root, so PHP users get the package from the mirror `github.com/logolabs/inkvec-php`: with the repository variable `PUBLISH_PHP=true` and the private half of an SSH deploy key with write access to it as the secret `PHP_MIRROR_DEPLOY_KEY`, `.github/workflows/php.yml` copies `packages/php`, the licence and the contract fixtures there on a `v*` tag, commits and tags it. The mirror carries no native library; `vendor/bin/inkvec-fetch-library` downloads the matching C release archive |
| SwiftPM (mirror repository) | `github.com/logolabs/inkvec-swift`, product `Inkvec` | on a `v*` tag matching the workspace version, `.github/workflows/swift.yml` attaches `InkvecFFI.xcframework.zip` to this repository's release (its own `GITHUB_TOKEN`; the repository must be public for SwiftPM to download it), then -- with the repository variable `PUBLISH_SWIFT=true` and a token that may push to the mirror as the secret `SWIFT_MIRROR_TOKEN` -- commits the rendered mirror there and pushes the same tag |
