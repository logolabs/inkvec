# Inkvec for Swift

Inkvec turns raster logos, icons and illustrations into compact, accurate SVG. This package
is its Swift API: a PNG, JPEG, WebP, GIF, BMP or TIFF (or raw RGBA pixels, or a `CGImage`)
in, an SVG string out. It calls the native Rust tracer through its C library, `inkvec_ffi`,
and runs the same pipeline as the `inkvec` command line with the same defaults.

Project page, method and benchmarks: <https://github.com/logolabs/inkvec>. This package's
source is `packages/swift` there; Apple apps install it from the mirror
[`logolabs/inkvec-swift`](https://github.com/logolabs/inkvec-swift), which carries the same
sources and a prebuilt XCFramework.

## Install

In `Package.swift`:

```swift
dependencies: [
    .package(url: "https://github.com/logolabs/inkvec-swift", from: "0.1.3"),
],
targets: [
    .target(name: "MyApp", dependencies: [.product(name: "Inkvec", package: "inkvec-swift")]),
]
```

or in Xcode, *File > Add Package Dependencies...* with the same URL. SwiftPM downloads
`InkvecFFI.xcframework.zip` from the GitHub release of that version and checks it against
the checksum in the mirror's manifest. The XCFramework holds static libraries, linked into
your binary, so there is nothing to embed or sign separately.

On Linux, see [Linux](#linux).

## Use

```swift
import Inkvec

let traced = try Inkvec.trace(contentsOf: URL(fileURLWithPath: "logo.png"),
                              options: InkvecOptions(colors: 16, noBackground: true))
try traced.svg.write(toFile: "logo.svg", atomically: true, encoding: .utf8)
print(traced.width, traced.height)   // the input's size in pixels
```

From memory, from pixels, from the Apple frameworks:

```swift
let fromBytes = try Inkvec.trace(pngData)                                  // Data of an image file
let fromPixels = try Inkvec.traceRGBA(rgba, width: 512, height: 512)       // straight RGBA8
let fromImage = try Inkvec.trace(cgImage)                                  // CGImage, UIImage, NSImage
let raw = try Inkvec.trace(pngData, optionsJSON: #"{"colors": 16}"#)       // options as JSON
```

A trace is synchronous and CPU-bound -- well under a second for an icon, seconds for a large
logo -- so run it off the main thread:

```swift
let svg = try await Task.detached(priority: .userInitiated) {
    try Inkvec.trace(pngData).svg
}.value
```

## API

```swift
enum Inkvec {
    static func trace(_ image: Data, options: InkvecOptions = .init()) throws -> Traced
    static func trace(contentsOf url: URL, options: InkvecOptions = .init()) throws -> Traced
    static func trace(_ image: Data, optionsJSON: String?) throws -> Traced
    static func traceRGBA(_ pixels: Data, width: Int, height: Int, options: InkvecOptions = .init()) throws -> Traced
    static func traceRGBA(_ pixels: Data, width: Int, height: Int, optionsJSON: String?) throws -> Traced
    static func trace(_ image: CGImage, options: InkvecOptions = .init()) throws -> Traced   // Apple platforms
    static func trace(_ image: UIImage, options: InkvecOptions = .init()) throws -> Traced   // UIKit
    static func trace(_ image: NSImage, options: InkvecOptions = .init()) throws -> Traced   // AppKit

    static let version: String         // "0.1.3": the Inkvec release the library was built from
    static let buildTarget: String     // "aarch64-macos", "aarch64-ios", "x86_64-linux-gnu", ...
    static let optionsSchema: String   // JSON Schema of the options, read from the library
    static let defaultsJSON: String    // every option at its default, as JSON
    static let defaults: InkvecOptions // the same, decoded: every property set

    struct Traced: Hashable, Sendable { let svg: String; let width: Int; let height: Int }
}
typealias Traced = Inkvec.Traced

struct InkvecOptions: Codable, Hashable, Sendable { ... }   // generated, see Options

enum InkvecError: Error, Hashable, Sendable {
    case invalidImage(String)     // not a decodable image, or pixels that do not match the size
    case invalidOptions(String)   // unknown option, wrong type, out of range, malformed JSON
    case `internal`(String)       // the tracer failed; please report it
    var kind: String { get }      // "invalid_image", "invalid_options", "internal"
    var message: String { get }
}
```

- **`trace(_:)`** reads the container as well as the pixels: JPEG and lossy WebP are traced
  with the noise-aware intake the command line uses for them.
- **`traceRGBA`** takes straight (not premultiplied) RGBA, 8 bits per channel, row-major,
  tightly packed, exactly `width * height * 4` bytes. The pixels of a PNG trace to the same
  bytes as the PNG.
- **`trace(_: CGImage)`** converts the image to straight sRGB RGBA8 with vImage and calls
  `traceRGBA`. `UIImage` and `NSImage` go through their `CGImage`; a `UIImage`'s
  `imageOrientation` is not applied.
- **`trace(contentsOf:)`** throws Foundation's error when the file cannot be read.
- Every other failure is an `InkvecError` carrying the library's message; it is also a
  `LocalizedError`.

## Options

Every property of `InkvecOptions` is optional: `nil` leaves the option out of the JSON the
tracer receives, so it takes the Rust library's default. The names are the schema's in
camelCase (`no_background` is `noBackground`); `InkvecOptions` and this table are generated
from the schema, and the tracer checks every value -- an unknown name or a value out of range
is `InkvecError.invalidOptions`, not a silent no-op.

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
<!-- inkvec:options:end -->

`trace(_:optionsJSON:)` and `traceRGBA(_:width:height:optionsJSON:)` take the same options
as a JSON object under the schema's own (snake_case) names, passed through as is.

## Threads, determinism

- Every function may be called from any number of threads at once (the C library
  guarantees it). Each call blocks its thread for the length of the trace while the tracer
  spreads its work over a thread pool it manages, one per process.
- `Traced`, `InkvecOptions` and `InkvecError` are `Sendable` value types.
- On one build target (`Inkvec.buildTarget`), the same input and options give byte-identical
  SVG across runs, thread counts and languages, as long as `timeBudget` is 0 (the default).
  Another target can write the same drawing slightly differently (subpaths in another
  order, a last digit), because the tracer takes a few transcendental functions from the
  platform's maths library. The repository's contract fixtures pin the bytes per target.

## Platforms

| Platform | Architectures | Library | Status |
|---|---|---|---|
| macOS 10.15+ | arm64, x86_64 | XCFramework, static | CI job written (`.github/workflows/swift.yml`); not yet run |
| iOS 13+ | arm64 | XCFramework, static | CI builds it; not yet run |
| iOS Simulator | arm64, x86_64 | XCFramework, static | CI links the tests for it; not yet run |
| Linux (glibc) | x86_64; arm64 untested | `libinkvec_ffi.so` or `.a`, as a system library | `swift test` passes: x86_64 Debian 12, Swift 6.3.3 |
| tvOS, watchOS, visionOS, Mac Catalyst | - | not built | - |
| Windows | - | not supported | - |

tvOS, watchOS and visionOS are tier-3 Rust targets, with no prebuilt standard library: they
need a nightly toolchain with `-Z build-std`, which the release does not use. Windows would
link `inkvec_ffi.dll.lib` the same way Linux links the `.so`; it has not been tried.

## Linux

There is no binary for Linux in the package: the mirror's manifest declares the
XCFramework only when SwiftPM runs on macOS, and on other hosts a system-library target,
`InkvecFFI`, whose module map links `inkvec_ffi`. Get the library -- the C archive of a
release (`inkvec-c-<version>-linux-x64.tar.gz`, with `lib/libinkvec_ffi.so`), or
`cargo build --release -p inkvec-ffi` in a checkout -- and tell the linker where it is:

```sh
LIB=/opt/inkvec/lib
swift build -Xlinker -L"$LIB" -Xlinker -rpath -Xlinker "$LIB"
```

(or `LIBRARY_PATH=$LIB` for the linker and `LD_LIBRARY_PATH=$LIB` at run time). Use the
library of the package's version: a library with another C ABI is refused
(`InkvecError.internal`), but one with other options is not detected, and `InkvecOptions`
would then name options the library does not have, or miss new ones.

To link statically, point `-L` at a directory that holds `libinkvec_ffi.a` and no `.so`. No
further flags were needed on Debian 12 with Swift 6.3.3: Swift's own link already brings in
the system libraries the static library asks for
(`-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc`, from
`cargo rustc --release -p inkvec-ffi --crate-type staticlib -- --print native-static-libs`).

Cross-compiling from a Mac to Linux with a Swift SDK does not work with the mirror, which
picks the XCFramework on a macOS host; build on Linux or from this repository instead.

## Building and testing from the repository

```sh
cargo build --release -p inkvec-ffi
cd packages/swift
LIB="$PWD/../../target/release"
swift test -Xlinker -L"$LIB" -Xlinker -rpath -Xlinker "$LIB"
```

`Package.swift` here links the library as a system library, on macOS and Linux alike. The
tests run every case of the shared contract (`bindings/contract/cases.json`) through the raw
and the typed entry points, compare the SVG hashes recorded for `Inkvec.buildTarget`
(`INKVEC_CONTRACT_REQUIRE_HASH=1` turns a target without hashes into a failure), and check
that the generated options and the copy of the C header are current. They find the
repository by walking up from the test sources, or through `INKVEC_REPO_ROOT`.

Generated here, never edited by hand (`python bindings/codegen/generate.py` writes them,
`--check` fails CI when they are stale):

- `Sources/Inkvec/InkvecOptions.generated.swift`, from `bindings/options.schema.json`;
- `Sources/InkvecFFI/inkvec.h`, a copy of `crates/inkvec-ffi/include/inkvec.h`;
- the options table above.

An option added, changed or removed in `inkvec::Options` therefore reaches Swift through
`cargo test -p inkvec` (the schema) and `generate.py`, with no Swift edited by hand.

### The XCFramework and the mirror

```sh
packages/swift/scripts/build-xcframework.sh            # macOS with Xcode and rustup
```

builds `inkvec-ffi`'s static library for `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`aarch64-apple-ios`, `aarch64-apple-ios-sim` and `x86_64-apple-ios`, joins the macOS and the
simulator pairs with `lipo`, and writes `packages/swift/build/InkvecFFI.xcframework`, its
zip and the zip's SwiftPM checksum. Each slice carries the header and a module map in
`Headers/InkvecFFI/`.

`scripts/render_mirror.py` writes the mirror's tree -- `Package.swift` from
`mirror/Package.swift.in`, the Swift sources, the C module for Linux, this README and the
licence -- with the binary target pointing at a release URL and checksum, or, for testing,
at a local XCFramework.

`.github/workflows/swift.yml` runs the Linux tests, builds the XCFramework on macOS, runs
the tests against it through a rendered mirror, and links them for the iOS Simulator. On a
`v*` tag whose version is the workspace version it attaches `InkvecFFI.xcframework.zip` to
the GitHub release of the tag, and -- when the repository variable `PUBLISH_SWIFT` is `true`
and the secret `SWIFT_MIRROR_TOKEN` holds a token that may push to `logolabs/inkvec-swift`
(a fine-grained token with *Contents: read and write* on that repository) -- commits the
rendered mirror there and pushes the same tag.

## Limits

- The command line's optional neural pre-passes -- the trained restorer (`--restore`) and the
  super-resolution pre-pass (`--sr`) -- are not in this package.
- A trace cannot be cancelled; `timeBudget` bounds it (and makes the output depend on
  machine speed).
- Built for graphic artwork -- logos, icons, illustrations, diagrams. Photographs trace to a
  large number of flat regions.

## Licence

Apache-2.0. See `LICENSE` and `NOTICE`.
