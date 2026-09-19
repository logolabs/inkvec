# Language bindings

Inkvec is a Rust library first. Every other language reaches the same pipeline through one of
three layers, each a thin wrapper over the one below:

| Layer | Where | For |
|---|---|---|
| `inkvec` crate (the facade) | `crates/inkvec` | Rust; the API every binding calls |
| C ABI: `inkvec_ffi.dll` / `libinkvec_ffi.so` / `libinkvec_ffi.dylib`, static `inkvec_ffi.lib` / `libinkvec_ffi.a`, header `include/inkvec.h` | `crates/inkvec-ffi` | C, C++, and every language with a C FFI: Java (JNA, Panama), C# (P/Invoke), Go (cgo), Swift, Ruby, PHP |
| Python package `inkvec` | `crates/inkvec-py` | Python 3.9+, abi3 wheels built with PyO3 and maturin |

The browser build (`crates/inkvec-wasm`) predates this layer; moving it onto the same facade and
schema is separate work.

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
* **Typed surfaces are generated** from the schema by `bindings/codegen/`: the Python type stub
  and the options tables in this file and the READMEs.
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
3. `python bindings/codegen/generate.py` -- regenerates the Python stub and the options tables.
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
| `cutout` | bool | `false` | - | Carry the input's transparency into the SVG: a face the source drew transparent becomes a hole, one drawn at a single opacity keeps it as fill-opacity, and white artwork on a transparent ground survives. Changes nothing for an opaque input. Off by default because over white it opens faint seams along shared edges; use it for artwork that will sit on anything but white. |
| `content_units` | bool | `false` | - | Scale the fit tolerances with the raster, so a large, simple drawing gets the parameter count of a small one. Trades fidelity for parsimony: small squares can come back as circles and thin rings broken. |
| `harmonize` | bool | `true` | - | Shape harmonization (on by default): marks that repeat across the drawing are redrawn from one consensus geometry per cluster, which saves parameters. The known cost is fidelity on fine-line art: on hairlines, thin rings and small rounded details the consensus can displace thin lines by about a pixel (on the 246-icon screen set mean dE00 0.151 off vs 0.299 on). Set it to false for such artwork. |
| `harmonize_threshold` | number | `0.92` | >= 0 and <= 1 | Shape-equivalence threshold for harmonization: the outline similarity (IoU after affine normalisation) above which two marks count as the same shape. |
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
* the tracer reads a number of `INKVEC_*` environment variables as research switches; leave them
  unset.

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

### Shape harmonization (on by default)

Marks that repeat across the drawing -- a run of identical tabs, segmented rings, tiled glyphs
-- are matched by affine-normalised outline similarity (`harmonize_threshold`, default 0.92) and
redrawn from one consensus geometry per cluster. The pass exists to save parameters: on the
246-icon screen set it trims the parameter ratio from 1.466x to 1.457x the artist's count.

The known cost is fidelity on fine-line art. On marks near the resolution of the raster --
hairlines, thin rings, small rounded details -- the consensus displaces thin lines by about a
pixel. On the screen set it raises mean dE00 from 0.151 with it off to 0.299 with it on: 63 of
246 icons get measurably worse, 4 better, 179 are untouched. On such artwork set `harmonize` to
false. The option's schema description says the same in every language.

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
let opts = inkvec::Options::from_json(r#"{"colors": 16, "cutout": true}"#)?;
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

traced = inkvec.trace("logo.png", colors=16, cutout=True)   # path, bytes, file, PIL, numpy
open("logo.svg", "w").write(traced.svg)

traced = inkvec.trace_rgba(rgba_bytes, width, height)       # or a numpy (H, W, 4) array
inkvec.defaults()          # every option at its default
inkvec.options_schema()    # the JSON Schema
```

The keyword arguments are typed in the stub (`inkvec/__init__.pyi`, generated). Errors derive
from `inkvec.InkvecError`.

## Versions

There is one version: `workspace.package.version` in the root `Cargo.toml`. Every crate inherits
it, maturin reads it for the Python wheel (`dynamic = ["version"]`), `inkvec_version()` and
`inkvec.__version__` return it, and the C release archives are named after the tag. A binding
added later (npm, Maven, NuGet, ...) should read it the same way rather than carry its own.

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
failure. The committed hashes cover `x86_64-windows-msvc` and `x86_64-linux-gnu`.

The Rust facade (`crates/inkvec/tests/contract.rs`), the C ABI from Rust
(`crates/inkvec-ffi/src/lib.rs` tests) and from a foreign caller (`crates/inkvec-ffi/tests/test_c_abi.py`,
through ctypes, plus the compiled C example) and the Python package
(`crates/inkvec-py/tests/test_inkvec.py`) all assert against it.

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
```

`.github/workflows/bindings.yml` does all of this on Linux, macOS and Windows and builds the
release artifacts: the C library for windows-x64, linux-x64, linux-arm64, macos-arm64 and
macos-x64, and abi3 wheels for the same targets plus an sdist.

## Publishing

Nothing is published automatically. Every publishing job in `bindings.yml` runs only on a `v*`
tag, and the registry ones only when the repository opts in.

| Registry | Name | Needs |
|---|---|---|
| crates.io | `inkvec` and the internal crates it depends on | the repository variable `PUBLISH_CRATES=true` and a crates.io API token as the secret `CARGO_REGISTRY_TOKEN`. `cargo publish --workspace` publishes in dependency order: `inkvec-core`, `inkvec-fit`, `inkvec-trace`, `inkvec-sr`, `inkvec-restore`, `inkvec-cli`, `inkvec` (all seven pass `cargo publish --workspace --dry-run`). `inkvec-ffi`, `inkvec-py` and `inkvec-wasm` are `publish = false`. |
| PyPI | `inkvec` | the repository variable `PUBLISH_PYPI=true` and this repository and workflow (environment `pypi`) registered as a trusted publisher of the PyPI project; or an API token as the secret `PYPI_API_TOKEN`, passed as `password:` to the publish step |
| GitHub Releases | C library archives `inkvec-c-<version>-<platform>` | nothing beyond the workflow's own `GITHUB_TOKEN` |
