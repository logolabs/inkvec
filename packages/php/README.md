# logolabs/inkvec

Inkvec turns raster logos, icons and illustrations into compact, accurate SVG. This package
is the PHP binding: the tracer's C ABI (`inkvec_ffi`) called through PHP's FFI extension, so
a trace runs as native code inside your process -- no subprocess, no HTTP hop, no copy of
the image on the way in. A PNG, JPEG, WebP, GIF, BMP or TIFF (or raw RGBA pixels) in, an SVG
string out. It runs the same pipeline as the `inkvec` command line with the same defaults.

- A boundary sits where the anti-aliasing says it is, to a fraction of a pixel.
- A circle is written as `<circle>`, a smooth ramp as a real gradient.
- The number of coordinates is chosen by minimum description length, not a tolerance you
  have to guess.

Project page, method and benchmarks: <https://github.com/logolabs/inkvec>. Try it in the
browser: <https://huggingface.co/spaces/logolabs/inkvec>.

## Install

```sh
composer require logolabs/inkvec
vendor/bin/inkvec-fetch-library      # the native library for this machine
```

PHP 8.1 or later with the FFI extension (`php -m | grep FFI`; on Debian and Ubuntu it is
`php8.x-ffi`, on Alpine `php8x-ffi`, and it is built in on Windows).

The second command downloads `libinkvec_ffi.so` / `.dylib` / `inkvec_ffi.dll` for this
platform from the [releases](https://github.com/logolabs/inkvec/releases) and puts it inside
the package, where the binding looks first. It is a per-platform binary and Composer
installs one set of files for every platform, so it is a command rather than an install
hook. Any other copy works too -- point `INKVEC_LIBRARY` at it, install it where the system
loader looks, or build it yourself:

```sh
cargo build --release -p inkvec-ffi     # target/release/libinkvec_ffi.so
```

## Use

```php
use LogoLabs\Inkvec\Inkvec;
use LogoLabs\Inkvec\Options;

$traced = Inkvec::traceFile('logo.png', new Options(colors: 16));
file_put_contents('logo.svg', $traced->svg);
echo "{$traced->width}x{$traced->height}\n";
```

```php
$traced = Inkvec::trace($bytes);                                   // bytes already in memory
$traced = Inkvec::trace($bytes, ['colors' => 16, 'minify' => true]); // an array of options
$traced = Inkvec::trace($bytes, '{"no_background": true}');          // raw JSON
$traced = Inkvec::traceRgba($rgba, $width, $height);                 // straight RGBA8 pixels
```

Every option is optional; one left `null` on `Options` takes the tracer's own default
(`Inkvec::defaults()`). An unknown option, or a value of the wrong type or out of range, is
an `InvalidOptionsException` naming the option -- checked by the tracer itself, not by this
package, so an option added to Inkvec works here as soon as you pass it in an array or as
raw JSON, even before `Options` has been regenerated for it.

Raw pixels from GD:

```php
$image = imagecreatefrompng('logo.png');
$rgba = '';
for ($y = 0; $y < imagesy($image); ++$y) {
    for ($x = 0; $x < imagesx($image); ++$x) {
        $c = imagecolorat($image, $x, $y);
        // GD stores 7-bit alpha, 0 opaque .. 127 transparent; Inkvec takes 8-bit, 255 opaque.
        $a = (int) round((127 - (($c >> 24) & 0x7F)) * 255 / 127);
        $rgba .= chr(($c >> 16) & 0xFF) . chr(($c >> 8) & 0xFF) . chr($c & 0xFF) . chr($a);
    }
}
$traced = Inkvec::traceRgba($rgba, imagesx($image), imagesy($image));
```

## In a web request

A trace is CPU-bound and takes tens to hundreds of milliseconds, so a queue worker or a
command is usually the right place for it -- those run on the CLI SAPI, where FFI is
available as soon as the extension is loaded, and nothing below applies.

To trace inside a PHP-FPM request, preload the library. `ffi.enable` defaults to `preload`
in a web SAPI, which allows FFI only through scopes registered during opcache preloading;
it also means one open library per pool instead of one per request. In your `php.ini`:

```ini
opcache.preload=/srv/app/preload.php
opcache.preload_user=www-data
```

```php
<?php // /srv/app/preload.php
require __DIR__ . '/vendor/autoload.php';
LogoLabs\Inkvec\Inkvec::preload();
```

`Inkvec::preload()` writes a small FFI header naming the library it found and hands it to
`FFI::load()`, which registers the scope `INKVEC`; every request then picks it up with no
further parsing. Without preloading you can instead set `ffi.enable=true`, which lets any
code in the process call any shared library -- weigh that before doing it.

`Inkvec::available()` reports whether a trace would work in the current process, so an
application can decide between tracing in-process and queueing the work elsewhere.

## API

```php
namespace LogoLabs\Inkvec;

final class Inkvec
{
    public static function trace(string $image, Options|array|string|null $options = null): Traced;
    public static function traceFile(string $path, Options|array|string|null $options = null): Traced;
    public static function traceRgba(string $rgba, int $width, int $height, Options|array|string|null $options = null): Traced;

    public static function version(): string;          // the native library's version
    public static function buildTarget(): string;      // e.g. x86_64-linux-gnu
    public static function abiVersion(): int;
    public static function optionsSchema(): array;     // decoded; ...Json() for the text
    public static function defaults(): array;          // decoded; ...Json() for the text

    public static function available(): bool;
    public static function useLibrary(?string $path): void;
    public static function libraryPath(): ?string;   // null when preloaded
    public static function preloaded(): bool;
    public static function preload(?string $library = null, ?string $headerDir = null): string;
}

final class Traced implements Stringable
{
    public readonly string $svg;
    public readonly int $width;
    public readonly int $height;
}

class InkvecException extends RuntimeException {}
final class InvalidImageException extends InkvecException {}
final class InvalidOptionsException extends InkvecException {}
final class InternalException extends InkvecException {}
final class LibraryException extends InkvecException {}
```

- **`trace`** takes the bytes of an encoded image, not a path (`traceFile` reads one).
  **`traceRgba`** takes straight (not premultiplied) RGBA8, row-major, `width * height * 4`
  bytes -- byte-identical to `trace` on a PNG holding the same pixels.
- **`Options`** (`src/Options.php`) is generated from the tracer's own JSON Schema;
  its properties are the schema's names in camelCase (`noBackground`), while an array of
  options uses the names the tracer knows (`no_background`). See Options below.
- Every failure is an `InkvecException`: `InvalidImageException` (not a decodable image, or
  raw pixels that do not match the size given), `InvalidOptionsException` (an unknown
  option, wrong type or out-of-range value), `InternalException` (the tracer itself failed;
  worth a bug report), or `LibraryException` (no usable native library). The message is the
  native library's own wording.
- `version`, `buildTarget`, `abiVersion`, `optionsSchema` and `defaults` read the running
  native library directly, so they can never drift from it. The binding refuses a library
  whose C ABI version it was not written against.
- The library is opened once per process (once per pool when preloaded) and the handle is
  reused; the image and the SVG are the only bytes copied per trace.

The output is deterministic: the same input and options give the same bytes on one build
target, as long as `time_budget` is 0 (the default). See docs/BINDINGS.md, "Determinism",
for how that can differ across build targets. The cross-language contract fixtures
(`bindings/contract/` in the repository) pin those bytes; `tests/ContractTest.php` checks
every case against them.

## Where the library is looked for

In order, stopping at the first that loads:

1. the path given to `Inkvec::useLibrary()`;
2. the environment variable `INKVEC_LIBRARY`, or `INKVEC_LIB`;
3. `lib/<platform>/` and `lib/` inside this package (where `inkvec-fetch-library` puts it);
4. `target/release/` of the Inkvec checkout, when the package sits inside one;
5. the plain library name, left to the system loader (`LD_LIBRARY_PATH`, `PATH`, ...).

`Inkvec::libraryPath()` reports which one was taken, and the exception raised when none
loads lists every path tried. Under preloading the search happened in the preloading
process, so `libraryPath()` is null there and `Inkvec::preloaded()` is true.

## Options

Every option is optional. Names and meanings are the Rust library's (`inkvec::Options`);
`Options` and this table are generated from its schema, and the tracer checks every value --
an unknown name is an error, not a silent no-op.

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

## Limits

- The optional neural pre-passes of the command line -- the trained restorer (`--restore`)
  and the super-resolution pre-pass (`--sr`) -- are not in this package: they need model
  weights and an ML runtime. For heavily compressed input, clean it first or use the
  command line.
- Built for graphic artwork -- logos, icons, illustrations, diagrams. Photographs trace to a
  large number of flat regions.
- A trace is synchronous and CPU-bound, and the tracer is internally parallel (rayon): one
  trace can use every core of the machine. Sizing a pool, count a trace as heavy work.
- FFI means the tracer shares your process. A crash in it would take the process with it;
  the C ABI catches panics and returns them as `InternalException` instead.

## Development

From a checkout of <https://github.com/logolabs/inkvec>, in `packages/php`:

```sh
cargo build --release -p inkvec-ffi     # the native library the tests load
composer install
composer test                            # or: php vendor/bin/phpunit
```

`src/Options.php` comes from `bindings/codegen/php.py`; regenerate it with
`python bindings/codegen/generate.py` after changing `inkvec::Options`, never by hand.

## Licence

Apache-2.0. See `LICENSE` and `NOTICE`.
