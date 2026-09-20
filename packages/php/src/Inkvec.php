<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec;

use FFI;

/**
 * Inkvec: raster logos, icons and illustrations to compact, accurate SVG.
 *
 * This is the PHP binding over Inkvec's C ABI (`crates/inkvec-ffi`), called through ext-FFI
 * -- the tracer itself runs as native code in this process, with no subprocess, no HTTP hop
 * and no copy of the image on the way in. It holds no per-option logic: options travel as
 * one JSON object, and the native library (`inkvec::Options::from_json`) is the only place a
 * value is defaulted or validated.
 *
 *     $traced = Inkvec::traceFile('logo.png', new Options(colors: 16));
 *     file_put_contents('logo.svg', $traced->svg);
 *
 * Every method may be called as often as you like; the library is loaded once per process
 * (see {@see preload()} for pools) and is thread-safe, though PHP itself is not threaded.
 */
final class Inkvec
{
    private function __construct()
    {
    }

    /**
     * Trace an encoded image: the bytes of a PNG, JPEG, WebP, GIF, BMP or TIFF.
     *
     * @param string                    $image   the file's bytes, not a path
     * @param Options|array|string|null $options typed options, an array keyed by the
     *                                           tracer's own option names, a raw JSON
     *                                           object, or null for every default
     *
     * @throws InvalidImageException   the bytes are not an image Inkvec can decode
     * @throws InvalidOptionsException an unknown option, or a value of the wrong type or out of range
     * @throws InternalException       the tracer failed; worth a bug report
     * @throws LibraryException        the native library could not be loaded
     */
    public static function trace(string $image, Options|array|string|null $options = null): Traced
    {
        $ffi = Library::ffi();
        $out = $ffi->new('InkvecResult');
        $out->struct_size = FFI::sizeof($out);

        try {
            $status = $ffi->inkvec_trace($image, \strlen($image), self::optionsJson($options), FFI::addr($out));

            return self::result($status, $out);
        } finally {
            $ffi->inkvec_result_free(FFI::addr($out));
        }
    }

    /**
     * Read a file and trace it. The whole file is read into memory first, as the tracer
     * works on bytes.
     *
     * @param Options|array|string|null $options see {@see trace()}
     *
     * @throws InkvecException if the file cannot be read, plus everything {@see trace()} throws
     */
    public static function traceFile(string $path, Options|array|string|null $options = null): Traced
    {
        $bytes = @file_get_contents($path);
        if ($bytes === false) {
            throw new InkvecException("inkvec: cannot read {$path}");
        }

        return self::trace($bytes, $options);
    }

    /**
     * Trace raw pixels: straight (not premultiplied) RGBA, 8 bits per channel, row-major,
     * tightly packed, exactly `width * height * 4` bytes. The SVG is byte-identical to
     * {@see trace()} on a PNG holding the same pixels.
     *
     * Pixels that do not match the size given are an {@see InvalidImageException}, raised by
     * the tracer, not counted here.
     *
     * @param Options|array|string|null $options see {@see trace()}
     */
    public static function traceRgba(
        string $rgba,
        int $width,
        int $height,
        Options|array|string|null $options = null
    ): Traced {
        $ffi = Library::ffi();
        $out = $ffi->new('InkvecResult');
        $out->struct_size = FFI::sizeof($out);

        try {
            $status = $ffi->inkvec_trace_rgba(
                $rgba,
                \strlen($rgba),
                $width,
                $height,
                self::optionsJson($options),
                FFI::addr($out)
            );

            return self::result($status, $out);
        } finally {
            $ffi->inkvec_result_free(FFI::addr($out));
        }
    }

    /** The native library's version, e.g. `0.1.4`. */
    public static function version(): string
    {
        return Library::ffi()->inkvec_version();
    }

    /**
     * The target the native library was built for, e.g. `x86_64-linux-gnu`. Output is
     * byte-identical between builds for one target; another target can write the same
     * drawing slightly differently (docs/BINDINGS.md, "Determinism").
     */
    public static function buildTarget(): string
    {
        return Library::ffi()->inkvec_build_target();
    }

    /** The C ABI version of the loaded library; always {@see Library::ABI_VERSION} here. */
    public static function abiVersion(): int
    {
        return Library::ffi()->inkvec_abi_version();
    }

    /**
     * The JSON Schema (draft 2020-12) of the options: every name, type, default, description
     * and range, read from the library itself, so it can never drift from it.
     *
     * @return array<string, mixed>
     */
    public static function optionsSchema(): array
    {
        return json_decode(self::optionsSchemaJson(), true, 512, \JSON_THROW_ON_ERROR);
    }

    /** The same schema, as the library's own JSON text. */
    public static function optionsSchemaJson(): string
    {
        return Library::ffi()->inkvec_options_schema();
    }

    /**
     * Every option at its default.
     *
     * @return array<string, bool|int|float>
     */
    public static function defaults(): array
    {
        return json_decode(self::defaultsJson(), true, 512, \JSON_THROW_ON_ERROR);
    }

    /** The same defaults, as the library's own compact JSON object. */
    public static function defaultsJson(): string
    {
        return Library::ffi()->inkvec_default_options();
    }

    /**
     * Whether a trace would work here: ext-FFI present and a native library that loads.
     * Useful to pick between tracing in-process and some other route at runtime.
     */
    public static function available(): bool
    {
        try {
            Library::ffi();

            return true;
        } catch (LibraryException) {
            return false;
        }
    }

    /**
     * Load this native library instead of searching for one. Pass null to forget it. Must
     * be called before the first trace of the process.
     *
     * @see Library::candidates() for what is searched otherwise
     */
    public static function useLibrary(?string $path): void
    {
        Library::use($path);
    }

    /**
     * The library in use, or null when none has been loaded yet -- and also when it was
     * preloaded, since the path was resolved in the preloading process, not this request
     * ({@see preloaded()}).
     */
    public static function libraryPath(): ?string
    {
        return Library::path();
    }

    /** Whether this process traces through a preloaded scope rather than its own load. */
    public static function preloaded(): bool
    {
        return Library::isPreloaded();
    }

    /**
     * Open the library once for a whole PHP-FPM pool, from an `opcache.preload` script:
     *
     *     require __DIR__ . '/vendor/autoload.php';
     *     LogoLabs\Inkvec\Inkvec::preload();
     *
     * Requests then reach it through an FFI scope, which is also what makes tracing possible
     * when `ffi.enable=preload` (the safe setting for a web SAPI). Returns the path of the
     * generated FFI header.
     */
    public static function preload(?string $library = null, ?string $headerDir = null): string
    {
        return Library::preload($library, $headerDir);
    }

    /**
     * Options as the C ABI takes them: one JSON object, or null for the defaults. An array
     * is encoded as it stands -- its keys are the tracer's own option names (`no_background`,
     * not `noBackground`) -- and a string is passed through untouched, so an option added to
     * Inkvec is reachable before {@see Options} has been regenerated for it. Either way the
     * native library is what validates the result.
     */
    private static function optionsJson(Options|array|string|null $options): ?string
    {
        if ($options === null) {
            return null;
        }
        if ($options instanceof Options) {
            return $options->toJson();
        }
        if (\is_array($options)) {
            return $options === []
                ? '{}'
                : json_encode($options, \JSON_THROW_ON_ERROR | \JSON_PRESERVE_ZERO_FRACTION);
        }

        return trim($options) === '' ? null : $options;
    }

    /**
     * One filled `InkvecResult` as a {@see Traced}, or the matching exception. The strings
     * are copied out of the library's memory here; the caller frees the result either way.
     *
     * @param \FFI\CData $out
     */
    private static function result(int $status, $out): Traced
    {
        if ($status === Library::OK) {
            return new Traced(
                $out->svg_len > 0 ? FFI::string($out->svg, $out->svg_len) : '',
                $out->width,
                $out->height
            );
        }

        $message = $out->error !== null
            ? FFI::string($out->error)
            : "the tracer failed with status {$status}";

        throw match ($status) {
            Library::ERR_INVALID_IMAGE => new InvalidImageException($message),
            Library::ERR_INVALID_OPTIONS => new InvalidOptionsException($message),
            Library::ERR_INTERNAL => new InternalException($message),
            default => new InkvecException($message),
        };
    }
}
