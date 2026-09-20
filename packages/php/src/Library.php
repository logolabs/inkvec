<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec;

use FFI;

/**
 * Loading the native library and holding the one FFI handle the binding calls through.
 *
 * The library is Inkvec's C ABI (`crates/inkvec-ffi`): `libinkvec_ffi.so` on Linux,
 * `libinkvec_ffi.dylib` on macOS, `inkvec_ffi.dll` on Windows. It is not shipped inside the
 * Composer package -- Composer installs one set of files for every platform, and this is a
 * per-platform binary -- so it is found, in order:
 *
 *   1. the path given to {@see Inkvec::useLibrary()};
 *   2. the environment variable `INKVEC_LIBRARY`, or `INKVEC_LIB`;
 *   3. `lib/` inside this package (where `bin/inkvec-fetch-library` puts it), either
 *      directly or under a platform folder such as `lib/linux-x64/`;
 *   4. `target/release/` of the Inkvec checkout, when this package sits inside one;
 *   5. the plain library name, left to the system's own loader (`LD_LIBRARY_PATH`,
 *      `DYLD_LIBRARY_PATH`, `PATH`, `/usr/local/lib`, ...).
 *
 * The handle is built once per process. Under PHP-FPM that means once per request, which
 * costs a parse of {@see self::CDEF}; {@see Inkvec::preload()} moves that to opcache
 * preloading, where the library is loaded once for the life of the pool and every request
 * picks it up through {@see FFI::scope()}.
 *
 * @internal Use {@see Inkvec}. This class is public only so an application can reach
 *           {@see Inkvec::useLibrary()} and {@see Inkvec::preload()} through it.
 */
final class Library
{
    /** Status codes of the C ABI (`include/inkvec.h`). */
    public const OK = 0;
    public const ERR_INVALID_ARGUMENT = 1;
    public const ERR_INVALID_IMAGE = 2;
    public const ERR_INVALID_OPTIONS = 3;
    public const ERR_INTERNAL = 4;

    /**
     * The C ABI version this binding is written against (`INKVEC_ABI_VERSION`). A library
     * reporting anything else is refused rather than called into.
     */
    public const ABI_VERSION = 1;

    /** The FFI scope {@see Inkvec::preload()} registers the library under. */
    public const SCOPE = 'INKVEC';

    /**
     * The C ABI as PHP's FFI parser reads it: `crates/inkvec-ffi/include/inkvec.h` without
     * the preprocessor lines, which `FFI::cdef()` does not take. Two deliberate spellings,
     * both the same pointers to the ABI: the image buffers are `const char *` rather than
     * `const uint8_t *`, so a PHP string goes to the tracer as it is, with no copy into an
     * FFI buffer; and the static strings are returned as `const char *`, which FFI hands
     * back as PHP strings. `tests/HeaderTest.php` checks every declaration here against the
     * header, so the two cannot drift.
     */
    public const CDEF = <<<'C'
        typedef struct InkvecResult {
            uint32_t struct_size;
            int32_t status;
            char *svg;
            size_t svg_len;
            uint32_t width;
            uint32_t height;
            char *error;
        } InkvecResult;

        int32_t inkvec_trace(const char *bytes, size_t len, const char *options_json, InkvecResult *out);
        int32_t inkvec_trace_rgba(const char *rgba, size_t len, uint32_t width, uint32_t height, const char *options_json, InkvecResult *out);
        void inkvec_result_free(InkvecResult *result);
        const char *inkvec_version(void);
        const char *inkvec_build_target(void);
        uint32_t inkvec_abi_version(void);
        const char *inkvec_options_schema(void);
        const char *inkvec_default_options(void);
        C;

    private static ?FFI $ffi = null;

    /** The path {@see Inkvec::useLibrary()} set, or the one a load ended up using. */
    private static ?string $path = null;

    private static bool $explicit = false;

    /** Whether the handle came from a scope registered by {@see preload()}. */
    private static bool $preloaded = false;

    private function __construct()
    {
    }

    /**
     * Load this library instead of searching for one. Takes effect on the next call; pass
     * null to forget it and search again.
     *
     * @throws LibraryException if a handle was already built for another library
     */
    public static function use(?string $path): void
    {
        if (self::$ffi !== null && $path !== self::$path) {
            throw new LibraryException(sprintf(
                'inkvec: the library %s is already loaded in this process; set the path before the first trace',
                self::$path ?? '(preloaded)'
            ));
        }

        self::$path = $path;
        self::$explicit = $path !== null;
    }

    /**
     * The library this process traces through, or null when nothing has been loaded yet --
     * and also when the handle came from a preloaded scope, where the path was recorded in
     * the preloading process and does not reach a request ({@see isPreloaded()}).
     */
    public static function path(): ?string
    {
        return self::$path;
    }

    /** Whether this process is calling through a scope registered by {@see preload()}. */
    public static function isPreloaded(): bool
    {
        return self::$preloaded;
    }

    /**
     * The FFI handle, built on first use and kept for the rest of the process.
     *
     * @throws LibraryException if ext-ffi is missing, no library is found, or the one found
     *                          reports a C ABI this binding does not speak
     */
    public static function ffi(): FFI
    {
        if (self::$ffi !== null) {
            return self::$ffi;
        }

        if (!\extension_loaded('ffi')) {
            throw new LibraryException(
                'inkvec: the FFI extension is not loaded. Install it (php-ffi) and enable it; '
                . 'in a web SAPI FFI is restricted to preloaded scopes unless ffi.enable=true '
                . '(see Inkvec::preload()).'
            );
        }

        // A preloaded scope is the cheap path: the library is already open and its
        // declarations are parsed, for the life of the pool.
        if (!self::$explicit) {
            try {
                $ffi = self::checked(FFI::scope(self::SCOPE), 'the preloaded library');
                self::$preloaded = true;

                return self::$ffi = $ffi;
            } catch (\FFI\Exception) {
                // Nothing preloaded under our scope; open the library ourselves.
            }
        }

        $tried = [];
        foreach (self::candidates() as $candidate) {
            $tried[] = $candidate;
            try {
                $ffi = FFI::cdef(self::CDEF, $candidate);
            } catch (\FFI\Exception) {
                continue;
            }
            self::$path = $candidate;

            return self::$ffi = self::checked($ffi, $candidate);
        }

        throw new LibraryException(sprintf(
            "inkvec: could not load the native library (%s). Tried:\n  %s\n"
            . 'Fetch it with `vendor/bin/inkvec-fetch-library`, or point INKVEC_LIBRARY at a copy '
            . '(see https://github.com/logolabs/inkvec/releases).',
            self::soname(),
            implode("\n  ", $tried)
        ));
    }

    /**
     * Every path {@see ffi()} would try, in order, on this machine.
     *
     * @return list<string>
     */
    public static function candidates(): array
    {
        $soname = self::soname();
        $paths = [];

        if (self::$explicit && self::$path !== null) {
            return [self::$path];
        }

        foreach (['INKVEC_LIBRARY', 'INKVEC_LIB'] as $name) {
            $value = getenv($name);
            if (\is_string($value) && $value !== '') {
                $paths[] = $value;
            }
        }

        $package = \dirname(__DIR__);
        $paths[] = $package . '/lib/' . self::platform() . '/' . $soname;
        $paths[] = $package . '/lib/' . $soname;

        // packages/php inside an Inkvec checkout: `cargo build --release -p inkvec-ffi`.
        $paths[] = \dirname($package, 2) . '/target/release/' . $soname;

        // Last, the system's own loader search.
        $paths[] = $soname;

        return array_values(array_unique($paths));
    }

    /** The shared library's file name on this platform. */
    public static function soname(): string
    {
        return match (\PHP_OS_FAMILY) {
            'Windows' => 'inkvec_ffi.dll',
            'Darwin' => 'libinkvec_ffi.dylib',
            default => 'libinkvec_ffi.so',
        };
    }

    /**
     * This machine as the release archives name it (`inkvec-c-<version>-<platform>`):
     * `linux-x64`, `linux-arm64`, `macos-arm64`, `macos-x64` or `windows-x64`.
     */
    public static function platform(): string
    {
        $os = match (\PHP_OS_FAMILY) {
            'Windows' => 'windows',
            'Darwin' => 'macos',
            default => 'linux',
        };
        $machine = strtolower(php_uname('m'));
        $arch = match (true) {
            \in_array($machine, ['arm64', 'aarch64'], true) => 'arm64',
            \in_array($machine, ['x86_64', 'amd64', 'x64'], true) => 'x64',
            default => $machine,
        };

        return $os . '-' . $arch;
    }

    /**
     * Register the library with opcache preloading, so every request of a pool shares one
     * open library and parses no declarations. Call it from the script named by
     * `opcache.preload`:
     *
     *     require __DIR__ . '/vendor/autoload.php';
     *     LogoLabs\Inkvec\Inkvec::preload();
     *
     * It writes a small FFI header (the declarations plus the resolved library path) and
     * hands it to {@see FFI::load()}, which is the only way an FFI scope survives into a
     * request when `ffi.enable=preload` -- the default of a hardened web SAPI.
     *
     * @param string|null $library   the library to preload; by default the first of
     *                               {@see candidates()} that exists
     * @param string|null $headerDir where to keep the generated header (default: the system
     *                               temporary directory)
     *
     * @return string the header that was loaded
     *
     * @throws LibraryException if no library is found or FFI refuses to load it
     */
    public static function preload(?string $library = null, ?string $headerDir = null): string
    {
        if (!\extension_loaded('ffi')) {
            throw new LibraryException('inkvec: the FFI extension is not loaded; preloading needs it.');
        }

        $library ??= self::firstExisting();
        $header = sprintf(
            "#define FFI_SCOPE \"%s\"\n#define FFI_LIB \"%s\"\n\n%s\n",
            self::SCOPE,
            str_replace(['\\', '"'], ['\\\\', '\\"'], $library),
            self::CDEF
        );

        $dir = $headerDir ?? sys_get_temp_dir();
        $path = $dir . '/inkvec-' . substr(hash('sha256', $header), 0, 16) . '.h';
        if (!is_file($path) || file_get_contents($path) !== $header) {
            if (@file_put_contents($path, $header) === false) {
                throw new LibraryException("inkvec: could not write the FFI header {$path}");
            }
        }

        try {
            $ffi = FFI::load($path);
        } catch (\FFI\Exception $e) {
            throw new LibraryException("inkvec: preloading {$library} failed: " . $e->getMessage(), 0, $e);
        }
        if ($ffi === false) {
            throw new LibraryException("inkvec: preloading {$library} failed (FFI::load returned false)");
        }

        self::$path = $library;
        self::$preloaded = true;
        self::$ffi = self::checked($ffi, $library);

        return $path;
    }

    /** The first candidate that is a readable file, for preloading. */
    private static function firstExisting(): string
    {
        foreach (self::candidates() as $candidate) {
            if (is_file($candidate)) {
                return $candidate;
            }
        }

        throw new LibraryException(sprintf(
            'inkvec: no %s found to preload; fetch it with `vendor/bin/inkvec-fetch-library` '
            . 'or point INKVEC_LIBRARY at a copy.',
            self::soname()
        ));
    }

    /** Refuse a library whose C ABI this binding was not written against. */
    private static function checked(FFI $ffi, string $what): FFI
    {
        $abi = $ffi->inkvec_abi_version();
        if ($abi !== self::ABI_VERSION) {
            throw new LibraryException(sprintf(
                'inkvec: %s reports C ABI version %d, but this binding speaks version %d; '
                . 'install the library that matches it.',
                $what,
                $abi,
                self::ABI_VERSION
            ));
        }

        return $ffi;
    }
}
