# inkvec (Java)

Inkvec turns raster logos, icons and illustrations into compact, accurate SVG: a PNG, JPEG,
WebP, GIF, BMP or TIFF (or raw RGBA pixels) in, an SVG string out. This package is a thin
[JNA](https://github.com/java-native-access/jna) binding over Inkvec's C ABI
(`crates/inkvec-ffi`, `include/inkvec.h`) -- it runs the same native pipeline as the
`inkvec` command line, with the same defaults, and contains no per-option logic of its own:
options pass through as JSON to `inkvec::Options::from_json` in Rust, the only place a value
is defaulted or validated.

- A boundary sits where the anti-aliasing says it is, to a fraction of a pixel.
- A circle is written as `<circle>`, a smooth ramp as a real gradient.
- The number of coordinates is chosen by minimum description length, not by a tolerance you
  have to guess.

Project page, method and benchmarks: <https://github.com/logolabs/inkvec>.

## Install

**This package is not on Maven Central, and is not planned to be.** It is built from the
repository, which takes two commands:

```sh
cargo build --release -p inkvec-ffi        # the native library this binds to
mvn -f packages/java/pom.xml install       # builds, tests and installs the jar locally
```

Then depend on what you just installed:

```xml
<dependency>
  <groupId>com.logolabs</groupId>
  <artifactId>inkvec</artifactId>
  <version>0.1.7</version>
</dependency>
```

`com.logolabs` is a placeholder group id; publishing to Central would need a verified
namespace (a domain you control, or `io.github.<user>`). The `release` profile in
`pom.xml` is still there for anyone who wants to publish their own build under their own
group id.

The jar the `java` workflow builds carries the native library for all five platforms
inside it, under JNA's resource prefixes, and is attached to that workflow's runs if you
would rather not build the Rust side yourself.

Java 8 or later. Pulls in one dependency, [JNA](https://github.com/java-native-access/jna).

## Use

```java
import com.logolabs.inkvec.Inkvec;
import com.logolabs.inkvec.InkvecOptions;
import com.logolabs.inkvec.TraceResult;
import java.nio.file.Files;
import java.nio.file.Path;

byte[] png = Files.readAllBytes(Path.of("logo.png"));
TraceResult r = Inkvec.trace(png, InkvecOptions.builder().colors(16).build());
Files.writeString(Path.of("logo.svg"), r.svg());
```

Raw pixels -- straight (not premultiplied) RGBA, 8 bits per channel, row-major -- go through
`traceRgba`:

```java
TraceResult r = Inkvec.traceRgba(rgba, width, height, InkvecOptions.builder().noBackground(true).build());
```

`traceRgba` on a buffer gives exactly the SVG `trace` gives for a PNG holding the same
pixels. `Inkvec.trace(Path)` reads the file for you. Every overload also takes options as a
raw JSON string (`Inkvec.trace(png, "{\"colors\": 16}")`) -- useful for an option Inkvec
added before this package's `InkvecOptions` was regenerated for it.

## API

```java
final class Inkvec {
    static TraceResult trace(byte[] image);
    static TraceResult trace(byte[] image, InkvecOptions options);
    static TraceResult trace(byte[] image, String optionsJson);
    static TraceResult trace(Path path) throws IOException;
    static TraceResult trace(Path path, InkvecOptions options) throws IOException;
    static TraceResult trace(Path path, String optionsJson) throws IOException;
    static TraceResult traceRgba(byte[] rgba, int width, int height);
    static TraceResult traceRgba(byte[] rgba, int width, int height, InkvecOptions options);
    static TraceResult traceRgba(byte[] rgba, int width, int height, String optionsJson);
    static String optionsSchema();
    static String defaults();
    static String version();
    static String buildTarget();
    static int abiVersion();
}

final class TraceResult {
    String svg();
    int width();
    int height();
}
```

- **`trace`** traces an encoded image. **`traceRgba`** traces decoded pixels. Both may be
  called from any number of threads at once (the native pipeline is itself parallel, one
  thread pool per process).
- **`optionsSchema`** and **`defaults`** return the tracer's own JSON Schema and default
  options object, as JSON text (not parsed -- this package carries no JSON library).
- **`version`** and **`buildTarget`** identify the loaded native library; see "Determinism"
  in [`docs/BINDINGS.md`](../../docs/BINDINGS.md).
- Every failure is an `InkvecException`: `InvalidImageException` (not a decodable image, or
  pixels that do not match the size given), `InvalidOptionsException` (an unknown option, a
  wrong type, a value out of range, or malformed JSON passed as a raw options string) or
  `InternalException` (the tracer failed; please report it).

## Options

`InkvecOptions` is a generated, immutable builder: every option is a fluent setter with its
default, range and description in the Javadoc, generated from the same schema
(`bindings/options.schema.json`) as every other Inkvec binding. `InkvecOptions.defaults()`
returns every option at its default; `InkvecOptions.builder()...build()` changes only what
you set.

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

## Native library

The native library, `inkvec_ffi` (`inkvec_ffi.dll` / `libinkvec_ffi.so` /
`libinkvec_ffi.dylib`), is not bundled with this README's source checkout -- release jars
carry it under JNA's per-platform resource prefix (`win32-x86-64/`, `linux-x86-64/`,
`linux-aarch64/`, `darwin-x86-64/`, `darwin-aarch64/`), where JNA extracts it automatically.
Override the search path with the system property `jna.library.path` (or this package's
alias, `inkvec.library.path`) -- both take a `File.pathSeparator`-joined list of directories,
checked before the bundled copy.

## Building and testing locally

```sh
cargo build --release -p inkvec-ffi          # from the repository root; builds inkvec_ffi
python bindings/codegen/generate.py --check  # InkvecOptions.java etc. are current
cd packages/java
mvn -q -Djna.library.path=../../target/release test
```

## Licence

Apache-2.0. See `LICENSE` and `NOTICE`.
