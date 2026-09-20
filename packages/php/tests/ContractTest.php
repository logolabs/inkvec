<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec\Tests;

use LogoLabs\Inkvec\Inkvec;
use LogoLabs\Inkvec\InkvecException;
use LogoLabs\Inkvec\InternalException;
use LogoLabs\Inkvec\InvalidImageException;
use LogoLabs\Inkvec\InvalidOptionsException;
use LogoLabs\Inkvec\Traced;
use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;

/**
 * The cross-language contract (`bindings/contract/cases.json`): every binding traces these
 * inputs with these options and must report the same size or the same kind of error, and --
 * on a build target the fixtures carry hashes for -- the same SVG byte for byte.
 */
final class ContractTest extends TestCase
{
    /** @return iterable<string, array{array<string, mixed>|null}> */
    public static function cases(): iterable
    {
        $cases = Fixtures::cases();
        if ($cases === []) {
            yield 'no fixtures' => [null];

            return;
        }
        foreach ($cases as $case) {
            yield $case['name'] => [$case];
        }
    }

    /** Only the cases that must match another case's SVG exactly, on every target. */
    /** @return iterable<string, array{array<string, mixed>|null}> */
    public static function pairedCases(): iterable
    {
        $paired = array_filter(Fixtures::cases(), static fn (array $c): bool => isset($c['same_svg_as']));
        if ($paired === []) {
            yield 'no fixtures' => [null];

            return;
        }
        foreach ($paired as $case) {
            yield $case['name'] => [$case];
        }
    }

    #[DataProvider('cases')]
    public function testCase(?array $case): void
    {
        $case = $this->orSkip($case);
        $expect = $case['expect'];

        if (isset($expect['error'])) {
            $this->expectException(match ($expect['error']) {
                'invalid_image' => InvalidImageException::class,
                'invalid_options' => InvalidOptionsException::class,
                'internal' => InternalException::class,
                default => InkvecException::class,
            });
            $this->traceCase($case);

            return;
        }

        $traced = $this->traceCase($case);
        self::assertSame($expect['width'], $traced->width);
        self::assertSame($expect['height'], $traced->height);
        self::assertStringStartsWith('<svg', ltrim($traced->svg));

        $target = Inkvec::buildTarget();
        $recorded = $case['svg'][$target] ?? null;
        if ($recorded === null) {
            self::markTestIncomplete("no SVG recorded for build target {$target}; sizes and errors checked");
        }
        self::assertSame($recorded['bytes'], \strlen($traced->svg), 'SVG length');
        self::assertSame($recorded['sha256'], hash('sha256', $traced->svg), 'SVG sha256');
    }

    #[DataProvider('pairedCases')]
    public function testSameSvgAsItsPair(?array $case): void
    {
        $case = $this->orSkip($case);

        $other = null;
        foreach (Fixtures::cases() as $candidate) {
            if ($candidate['name'] === $case['same_svg_as']) {
                $other = $candidate;
                break;
            }
        }
        self::assertNotNull($other, "case {$case['same_svg_as']} is not in cases.json");

        self::assertSame($this->traceCase($other)->svg, $this->traceCase($case)->svg);
    }

    /** @param array<string, mixed> $case */
    private function traceCase(array $case): Traced
    {
        $bytes = (string) file_get_contents(Fixtures::contractDir() . '/' . $case['input']);
        $options = $case['options'] === [] ? null : $case['options'];

        return $case['form'] === 'rgba'
            ? Inkvec::traceRgba($bytes, $case['width'], $case['height'], $options)
            : Inkvec::trace($bytes, $options);
    }

    /**
     * @param array<string, mixed>|null $case
     *
     * @return array<string, mixed>
     */
    private function orSkip(?array $case): array
    {
        if ($case === null) {
            self::markTestSkipped('the contract fixtures were not shipped with this copy of the package');
        }

        return $case;
    }
}
