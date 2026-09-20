<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec\Tests;

/**
 * The shared contract fixtures (`bindings/contract/` in the Inkvec repository, copied to
 * `tests/contract/` in the published package) and the C header, wherever this package is
 * being tested from.
 */
final class Fixtures
{
    /** The contract directory, or null when it was not shipped with this copy. */
    public static function contractDir(): ?string
    {
        foreach ([__DIR__ . '/contract', \dirname(__DIR__, 3) . '/bindings/contract'] as $dir) {
            if (is_file($dir . '/cases.json')) {
                return $dir;
            }
        }

        return null;
    }

    /**
     * Every contract case.
     *
     * @return list<array<string, mixed>>
     */
    public static function cases(): array
    {
        $dir = self::contractDir();
        if ($dir === null) {
            return [];
        }

        /** @var array{cases: list<array<string, mixed>>} $contract */
        $contract = json_decode((string) file_get_contents($dir . '/cases.json'), true, 512, \JSON_THROW_ON_ERROR);

        return $contract['cases'];
    }

    /** `crates/inkvec-ffi/include/inkvec.h`, or null outside the repository. */
    public static function header(): ?string
    {
        $path = \dirname(__DIR__, 3) . '/crates/inkvec-ffi/include/inkvec.h';

        return is_file($path) ? (string) file_get_contents($path) : null;
    }
}
