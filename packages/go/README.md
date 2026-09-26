# inkvec-go

Inkvec turns raster logos, icons and illustrations into compact, accurate SVG. This module is
the tracer for Go: a PNG, JPEG, WebP, GIF, BMP or TIFF (or raw RGBA pixels) in, an SVG out,
with the same pipeline and defaults as the `inkvec` command line.

It is pure Go. The tracer is Inkvec's C ABI compiled to WebAssembly (`wasm32-wasip1`),
embedded in the package and run by [wazero](https://wazero.io): no cgo, no C toolchain, no
native library to install, and `GOOS`/`GOARCH` cross-compilation works as for any Go code.

Project page, method and benchmarks: <https://github.com/logolabs/inkvec>.

## Install

```sh
go get github.com/logolabs/inkvec-go
```

Go 1.25 or later (wazero's floor). The package name is `inkvec`.

## Use

```go
import inkvec "github.com/logolabs/inkvec-go"

png, _ := os.ReadFile("logo.png")
traced, err := inkvec.Trace(ctx, png, &inkvec.Options{
	Colors:       inkvec.Ptr(16),
	NoBackground: inkvec.Ptr(true),
})
if err != nil {
	return err
}
os.WriteFile("logo.svg", []byte(traced.SVG), 0o644)
fmt.Println(traced.Width, traced.Height) // the input's size in pixels
```

Raw pixels -- straight RGBA, 8 bits per channel, `width*height*4` bytes -- for example an
`*image.NRGBA` whose stride is `4*width`:

```go
traced, err := inkvec.TraceRGBA(ctx, img.Pix, img.Rect.Dx(), img.Rect.Dy(), nil)
```

The SVG is byte-identical to `Trace` on a PNG holding the same pixels.

| Function | |
|---|---|
| `Trace(ctx, image, *Options)` | encoded image bytes |
| `TraceRGBA(ctx, pixels, width, height, *Options)` | raw straight RGBA8 |
| `TraceJSON(ctx, image, optionsJSON)`, `TraceRGBAJSON(...)` | options as a JSON object, passed to the tracer untouched |
| `Defaults()`, `OptionsSchema()` | every option's default; the JSON Schema of the options |
| `Version()`, `BuildTarget()` | the tracer's version (the workspace's `Cargo.toml`) and build target, `wasm32-wasip1` |
| `NewEngine(ctx, *Config)` | an engine of your own (below); the functions above share a default one |

### Errors

Every trace error is an `*inkvec.Error` (`Code`: `invalid_image`, `invalid_options` or
`internal`; `Message`: the tracer's text) and matches one sentinel with `errors.Is`:

```go
_, err := inkvec.TraceJSON(ctx, png, `{"colours": 8}`)
errors.Is(err, inkvec.ErrInvalidOptions) // true: "unknown field `colours`, expected one of ..."
```

`ErrInvalidImage` (not decodable, or pixels that do not match the size), `ErrInvalidOptions`
(unknown option, wrong type, out of range) and `ErrInternal` (the tracer failed or panicked).
A context that ends first returns `ctx.Err()`.

## Options

`Options` is generated from the Rust library's option schema
(`bindings/options.schema.json`, by `bindings/codegen/golang.py`). Every field is a pointer;
nil takes the default, and `inkvec.Ptr(v)` sets one. The struct travels to the tracer as one
JSON object that Rust checks in full, so nothing about an option is restated in Go. An
option added to the tracer after this struct was generated can be passed with `TraceJSON`.

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
| `mode` | string | `"quality"` | - | Which engine traces the image. "quality" (the default) is the full engine: the best fidelity and the fewest parameters, at about half a second for a 512 px logo. "fast" is a Potrace-class fit on the same palette, planar map and emitter, with a one-pass gradient check in place of gradient recovery: several times faster (tens of milliseconds at 512 px), a little less faithful, with somewhat more parameters. Options that only steer quality stages (precision, content_units, harmonize, harmonize_threshold, time_budget) are ignored in fast mode. |
| `merge_colors` | string | `""` | - | Colour groups: fills to draw as one, so the shapes between them join rather than being recoloured. Empty (the default) changes nothing. Groups are separated by ';' and members by ','; a member is a colour '#rrggbb' as it appears in a trace of the same image, or a gradient written as its stop colours joined by '>'. An optional '=' says what the group becomes: '=#rrggbb' a flat colour, '=@n' its n-th member (1-based; a gradient there is refitted over the whole group); without it, the member covering the most of the image. Example: '#c0392b,#e74c3c;#f00>#00f,#0a0=@1'. A group costs one extra trace. |
<!-- inkvec:options:end -->

## Concurrency, memory and cancellation

An `Engine` compiles the module once and keeps a pool of module instances. Each instance
runs one trace at a time, on one core, in its own linear memory, so calls from many
goroutines run in parallel up to `Config.MaxInstances` (default `GOMAXPROCS`) and wait
beyond it. WebAssembly memory never shrinks: an instance that grew past
`Config.RetainMemory` (default 256 MiB) is closed after its call rather than kept. Measured
linear memory after one trace: 2 MiB for a 96 px icon, 22 MiB for a 512 px logo, 48 to 96
MiB for 768 px logos.

A call returns as soon as its context ends. By default the abandoned trace finishes in the
background (holding its instance), because wazero's mid-call interruption
(`Config.Interruptible`) made tracing about 3.5 times slower in our measurements.
`Config.CompilationCacheDir` keeps the compiled module on disk for the next process; the
compilation took about half a second on a 16-thread machine.

## Performance

One trace is single-threaded here (WebAssembly outside the browser has no threads in this
build), while the native command line spreads a trace over every core. Rough numbers on a
Ryzen 7 5800X that was busy with other work, wall-clock, native times including process
start:

| Image | this package | native, 1 thread | native, 16 threads |
|---|---|---|---|
| 96 x 96 icon | 1.0 s | 0.5 s | 0.35 s |
| 512 x 512 logo | 9-11 s | 4.8 s | 1.3 s |

About twice the native single-threaded time. Throughput scales with parallel calls; for one
large image at a time, the native command line or the C library is faster. wazero's
interpreter, which it uses on platforms its compiler does not support (anything but amd64
and arm64), is about a hundred times slower again.

## Output and the build target

`BuildTarget()` is `wasm32-wasip1`, and the output is the same on every host. It is not
always byte-identical to a native build or to the browser build (`@logolabs/inkvec`,
`wasm32-unknown`): those take a few maths functions from other C libraries, whose last-bit
rounding can tip a near-tie. The cross-language contract (`bindings/contract/cases.json`)
records the hashes per target.

The embedded module has its floating-point `select` instructions rewritten as integer ones
(`tools/wasm_float_select.py` in the Inkvec repository): wazero's amd64 compiler, v1.8.0
through at least v1.12.0, can return the wrong operand from a floating-point `select`, and
without the rewrite traces came out different from what the WebAssembly semantics give (and
from wazero's own interpreter). The test `TestWazeroFloatSelect` reports whether the bug is
still there.

## Building from the Inkvec repository

The module's source is `packages/go` in <https://github.com/logolabs/inkvec>; this
repository is a mirror of it that adds the compiled `inkvec.wasm`, which `go get` cannot
build. There:

```sh
rustup target add wasm32-wasip1
tools/build_go_wasm.sh              # cargo + wasm-opt (optional) + the select rewrite
cd packages/go && go test ./...     # the contract, errors, concurrency, cancellation
```

Until the script has run, the package does not compile (`pattern inkvec.wasm: no matching
files found`). `INKVEC_BLESS=add go test -run TestContract` records this target's hashes in
the contract after a deliberate change to the tracer's output, and `INKVEC_CLI=<path to a
native inkvec> go test -run TimingVsNative -v` times the two side by side.

Apache-2.0.
