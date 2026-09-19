# LogoLabs.Inkvec

Inkvec turns raster logos, icons and illustrations into compact, accurate SVG. This package
is the .NET binding: P/Invoke over the Rust tracer's C ABI (`inkvec_ffi`), no other
dependency on `net8.0` (on `netstandard2.0` it needs `System.Memory` for `Span<T>`). A PNG,
JPEG, WebP, GIF, BMP or TIFF (or raw RGBA pixels) in, an SVG string out. It runs the same
pipeline as the `inkvec` command line with the same defaults.

- A boundary sits where the anti-aliasing says it is, to a fraction of a pixel.
- A circle is written as `<circle>`, a smooth ramp as a real gradient.
- The number of coordinates is chosen by minimum description length, not a tolerance you
  have to guess.

Project page, method and benchmarks: <https://github.com/logolabs/inkvec>. Try it in the
browser: <https://huggingface.co/spaces/logolabs/inkvec>.

## Install

```sh
dotnet add package LogoLabs.Inkvec
```

Targets `netstandard2.0` (.NET Framework 4.6.1+, Mono, Unity, older .NET) and `net8.0`.
Native libraries ship in the package for `win-x64`, `win-arm64`, `linux-x64`,
`linux-arm64`, `osx-x64` and `osx-arm64`; a `PackageReference` in an SDK-style project picks
the right one for the build automatically.

## Use

```csharp
using LogoLabs.Inkvec;

var traced = Inkvec.TraceFile("logo.png", new InkvecOptions { Colors = 16 });
File.WriteAllText("logo.svg", traced.Svg);
Console.WriteLine($"{traced.Width}x{traced.Height}");
```

```csharp
// Bytes already in memory, a stream, or raw decoded pixels:
var traced = Inkvec.Trace(File.ReadAllBytes("logo.png"));
var traced2 = Inkvec.Trace(File.OpenRead("logo.png"));
var traced3 = Inkvec.TraceRgba(rgbaBytes, width, height, new InkvecOptions { NoBackground = true });
```

Every option is optional; a property left `null` on `InkvecOptions` takes the tracer's own
default (`Inkvec.Defaults`, as JSON). An unknown option, or a value of the wrong type or out
of range, is an `InvalidOptionsException` naming the option -- checked by the tracer itself,
not by this package, so an option added to Inkvec works here as soon as you pass its raw
JSON name (see the `string`-options overloads) even before `InkvecOptions` has been
regenerated for it.

```csharp
Inkvec.Trace(bytes, """{"colors": 16, "no_background": true}"""); // raw JSON, same effect
```

## API

```csharp
namespace LogoLabs.Inkvec;

public static class Inkvec
{
    public static Traced Trace(ReadOnlySpan<byte> image, InkvecOptions? options = null);
    public static Traced Trace(byte[] image, InkvecOptions? options = null);
    public static Traced Trace(ReadOnlySpan<byte> image, string? optionsJson);
    public static Traced Trace(byte[] image, string? optionsJson);
    public static Traced Trace(Stream stream, InkvecOptions? options = null);
    public static Traced TraceFile(string path, InkvecOptions? options = null);

    public static Traced TraceRgba(ReadOnlySpan<byte> rgba, int width, int height, InkvecOptions? options = null);
    public static Traced TraceRgba(byte[] rgba, int width, int height, InkvecOptions? options = null);
    public static Traced TraceRgba(ReadOnlySpan<byte> rgba, int width, int height, string? optionsJson);
    public static Traced TraceRgba(byte[] rgba, int width, int height, string? optionsJson);

    public static string Version { get; }
    public static string BuildTarget { get; }
    public static uint AbiVersion { get; }
    public static string OptionsSchema { get; }
    public static string Defaults { get; }
}

public sealed class Traced
{
    public string Svg { get; }
    public uint Width { get; }
    public uint Height { get; }
}

public abstract class InkvecException : Exception { }
public sealed class InvalidImageException : InkvecException { }
public sealed class InvalidOptionsException : InkvecException { }
public sealed class InkvecInternalException : InkvecException { }
```

- **`Trace`** traces an encoded image. **`TraceRgba`** traces straight (not premultiplied)
  RGBA8 pixels, row-major, `width * height * 4` bytes -- byte-identical to `Trace` on a PNG
  holding the same pixels.
- **`InkvecOptions`** (`InkvecOptions.generated.cs`) is generated from the tracer's own JSON
  Schema; see Options below. Its `ToJson()` is what crosses the C ABI.
- Every failure is an `InkvecException`: `InvalidImageException` (not a decodable image, or
  raw pixels that do not match the size given), `InvalidOptionsException` (an unknown
  option, wrong type or out-of-range value), or `InkvecInternalException` (the tracer
  itself failed; worth a bug report). The message is the native library's own wording.
- Every method may be called from any number of threads at once; there is no `async`
  surface because a trace is CPU-bound, not I/O -- wrap a call in `Task.Run` to keep it off
  a thread you care about.
- `Version`, `BuildTarget`, `AbiVersion`, `OptionsSchema` and `Defaults` read the running
  native library directly, so they can never drift from it.

The output is deterministic: the same input and options give the same bytes on one build
target, as long as `time_budget` is 0 (the default). See docs/BINDINGS.md, "Determinism",
for how that can differ across build targets. The cross-language contract fixtures
(`bindings/contract/` in the repository) pin those bytes; `test/LogoLabs.Inkvec.Tests`
checks every case against them.

## Options

Every option is optional. Names and meanings are the Rust library's (`inkvec::Options`);
`InkvecOptions` and this table are generated from its schema, and the tracer checks every
value -- an unknown name is an error, not a silent no-op.

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

## Limits

- The optional neural pre-passes of the command line -- the trained restorer (`--restore`)
  and the super-resolution pre-pass (`--sr`) -- are not in this package: they need model
  weights and an ML runtime. For heavily compressed input, clean it first or use the
  command line.
- Built for graphic artwork -- logos, icons, illustrations, diagrams. Photographs trace to a
  large number of flat regions.
- On `netstandard2.0`, a classic (non-SDK-style, or SDK-style without a `RuntimeIdentifier`)
  .NET Framework project may not copy the right `runtimes/<rid>/native/` asset to its output
  automatically; if `Inkvec.Version` throws a `DllNotFoundException`, copy the native
  library for your platform from the NuGet package's `runtimes/` folder next to your
  executable, or set `<RuntimeIdentifier>` in your project. `net8.0` SDK-style projects do
  not need this.

## Licence

Apache-2.0. See `LICENSE` and `NOTICE`.
