<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec\Tests;

use LogoLabs\Inkvec\Library;
use PHPUnit\Framework\TestCase;

/**
 * `Library::CDEF` against `crates/inkvec-ffi/include/inkvec.h`: the declarations PHP's FFI
 * parser reads must be the ones cbindgen wrote, and the status codes must be the header's.
 * The C ABI cannot change here without this failing, which is the point -- nothing about it
 * is meant to be maintained twice.
 *
 * Only runs inside a checkout of the Inkvec repository; the published package ships no
 * header.
 */
final class HeaderTest extends TestCase
{
    private string $header;

    protected function setUp(): void
    {
        $header = Fixtures::header();
        if ($header === null) {
            self::markTestSkipped('include/inkvec.h is not part of this copy of the package');
        }
        $this->header = self::stripComments($header);
    }

    public function testStatusCodesAndAbiVersionAreTheHeaderS(): void
    {
        $defines = [];
        preg_match_all('/#define (INKVEC_\w+) (\d+)/', $this->header, $matches, \PREG_SET_ORDER);
        foreach ($matches as $m) {
            $defines[$m[1]] = (int) $m[2];
        }

        self::assertNotSame([], $defines, 'no INKVEC_* defines found in the header');
        self::assertSame($defines['INKVEC_OK'], Library::OK);
        self::assertSame($defines['INKVEC_ERR_INVALID_ARGUMENT'], Library::ERR_INVALID_ARGUMENT);
        self::assertSame($defines['INKVEC_ERR_INVALID_IMAGE'], Library::ERR_INVALID_IMAGE);
        self::assertSame($defines['INKVEC_ERR_INVALID_OPTIONS'], Library::ERR_INVALID_OPTIONS);
        self::assertSame($defines['INKVEC_ERR_INTERNAL'], Library::ERR_INTERNAL);
        self::assertSame($defines['INKVEC_ABI_VERSION'], Library::ABI_VERSION);
    }

    public function testTheResultStructHasTheHeaderSFieldsInOrder(): void
    {
        self::assertSame(self::fields($this->header), self::fields(Library::CDEF));
    }

    public function testEveryFunctionIsDeclaredTheSameWay(): void
    {
        $fromHeader = self::functions($this->header);
        $fromCdef = self::functions(Library::CDEF);

        self::assertNotSame([], $fromHeader, 'no inkvec_* functions found in the header');
        self::assertSame(array_keys($fromHeader), array_keys($fromCdef), 'the same functions, in order');
        foreach ($fromHeader as $name => $signature) {
            self::assertSame($signature, $fromCdef[$name], "{$name} is declared differently");
        }
    }

    private static function stripComments(string $c): string
    {
        return (string) preg_replace('~/\*.*?\*/~s', ' ', $c);
    }

    /**
     * The fields of `struct InkvecResult`, as "type name" pairs in declaration order.
     *
     * @return list<string>
     */
    private static function fields(string $c): array
    {
        if (!preg_match('/struct InkvecResult\s*\{(.*?)\}/s', self::stripComments($c), $m)) {
            self::fail('no struct InkvecResult found');
        }
        $fields = [];
        foreach (explode(';', $m[1]) as $field) {
            $field = self::normalise($field);
            if ($field !== '') {
                $fields[] = $field;
            }
        }

        return $fields;
    }

    /**
     * Every `inkvec_*` declaration, keyed by name, as "<return> (<parameter types>)" with
     * parameter names dropped -- so a renamed parameter is not a failure but a changed type
     * is.
     *
     * @return array<string, string>
     */
    private static function functions(string $c): array
    {
        $c = self::stripComments($c);
        preg_match_all('/([A-Za-z_][\w \t*]*?)\s*\b(inkvec_\w+)\s*\(([^)]*)\)\s*;/s', $c, $matches, \PREG_SET_ORDER);

        $out = [];
        foreach ($matches as $m) {
            $params = array_map(
                static fn (string $p): string => self::dropName(self::normalise($p)),
                explode(',', $m[3])
            );
            $out[$m[2]] = self::normalise($m[1]) . ' (' . implode(', ', $params) . ')';
        }

        return $out;
    }

    /**
     * One declaration in a single spelling: no `struct` keyword, one space between tokens,
     * and `const uint8_t *` read as `const char *` -- the binding's one deliberate
     * difference from the header, the same pointer to the ABI, which lets a PHP string be
     * passed with no copy.
     */
    private static function normalise(string $decl): string
    {
        $decl = (string) preg_replace('/\bstruct\s+/', '', $decl);
        $decl = (string) preg_replace('/\bconst\s+uint8_t\s*\*/', 'const char *', $decl);
        $decl = (string) preg_replace('/\s+/', ' ', $decl);
        $decl = (string) preg_replace('/\s*\*\s*/', ' *', $decl);

        return trim($decl);
    }

    /** "const char *bytes" -> "const char *"; "void" stays as it is. */
    private static function dropName(string $param): string
    {
        return trim((string) preg_replace('/\b\w+$/', '', $param)) ?: $param;
    }
}
