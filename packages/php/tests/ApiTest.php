<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec\Tests;

use LogoLabs\Inkvec\Inkvec;
use LogoLabs\Inkvec\Library;
use LogoLabs\Inkvec\Options;
use LogoLabs\Inkvec\Traced;
use PHPUnit\Framework\TestCase;

/** The surface a caller sees: the entry points, what they report, and how options travel. */
final class ApiTest extends TestCase
{
    private static function png(): string
    {
        $dir = Fixtures::contractDir();
        if ($dir === null) {
            self::markTestSkipped('the contract fixtures were not shipped with this copy of the package');
        }

        return (string) file_get_contents($dir . '/tiny.png');
    }

    public function testTheLibraryLoadsAndReportsItself(): void
    {
        self::assertTrue(Inkvec::available());
        self::assertMatchesRegularExpression('/^\d+\.\d+\.\d+/', Inkvec::version());
        self::assertNotSame('', Inkvec::buildTarget());
        self::assertSame(Library::ABI_VERSION, Inkvec::abiVersion());
        if (Inkvec::preloaded()) {
            self::assertNull(Inkvec::libraryPath(), 'a preloaded scope carries no path into the request');
        } else {
            self::assertNotNull(Inkvec::libraryPath());
        }
    }

    public function testDefaultsAndSchemaComeFromTheLibrary(): void
    {
        $defaults = Inkvec::defaults();
        $schema = Inkvec::optionsSchema();

        self::assertArrayHasKey('colors', $defaults);
        self::assertSame('object', $schema['type']);
        self::assertSame(
            array_keys($schema['properties']),
            array_keys($defaults),
            'every option in the schema has a default'
        );

        // The typed class is generated from the same schema, so it covers exactly those options.
        $typed = array_keys((new Options(
            precision: 0.1,
            minArea: 2.0,
            colors: 64,
            merge: 0.035,
            maxDim: 2048,
            timeBudget: 0.0,
            margin: 0.0,
            noBackground: false,
            minify: false,
            nativeAlpha: true,
            cutout: false,
            contentUnits: false,
            harmonize: true,
            harmonizeThreshold: 0.92,
            mergeColors: '',
        ))->toArray());
        self::assertSame(array_keys($schema['properties']), $typed);
    }

    public function testTraceReportsTheInputSize(): void
    {
        $traced = Inkvec::trace(self::png());

        self::assertInstanceOf(Traced::class, $traced);
        self::assertSame(96, $traced->width);
        self::assertSame(96, $traced->height);
        self::assertStringContainsString('<svg', $traced->svg);
        self::assertSame($traced->svg, (string) $traced, 'a Traced stringifies to its SVG');
    }

    public function testTraceFileReadsTheFile(): void
    {
        $dir = Fixtures::contractDir();
        if ($dir === null) {
            self::markTestSkipped('no fixtures');
        }

        self::assertSame(Inkvec::trace(self::png())->svg, Inkvec::traceFile($dir . '/tiny.png')->svg);
    }

    public function testTypedArrayAndRawJsonOptionsAgree(): void
    {
        $png = self::png();
        $typed = Inkvec::trace($png, new Options(colors: 8, minify: true, noBackground: true, harmonize: false));
        $array = Inkvec::trace($png, ['colors' => 8, 'minify' => true, 'no_background' => true, 'harmonize' => false]);
        $json = Inkvec::trace($png, '{"colors": 8, "minify": true, "no_background": true, "harmonize": false}');

        self::assertSame($typed->svg, $array->svg);
        self::assertSame($typed->svg, $json->svg);
    }

    public function testNoOptionsIsTheSameAsEmptyOptions(): void
    {
        $png = self::png();
        $svg = Inkvec::trace($png)->svg;

        self::assertSame($svg, Inkvec::trace($png, new Options())->svg);
        self::assertSame($svg, Inkvec::trace($png, [])->svg);
        self::assertSame($svg, Inkvec::trace($png, '')->svg);
        self::assertSame($svg, Inkvec::trace($png, '{}')->svg);
    }

    public function testRgbaMatchesThePngOfTheSamePixels(): void
    {
        $dir = Fixtures::contractDir();
        if ($dir === null) {
            self::markTestSkipped('no fixtures');
        }
        $rgba = (string) file_get_contents($dir . '/tiny.rgba');

        self::assertSame(96 * 96 * 4, \strlen($rgba));
        self::assertSame(Inkvec::trace(self::png())->svg, Inkvec::traceRgba($rgba, 96, 96)->svg);
    }

    public function testTracingIsRepeatable(): void
    {
        $png = self::png();

        self::assertSame(Inkvec::trace($png)->svg, Inkvec::trace($png)->svg);
    }

    public function testOptionsOnlySerialisesWhatWasSet(): void
    {
        self::assertSame('{}', (new Options())->toJson());
        self::assertSame(['colors' => 16], (new Options(colors: 16))->toArray());
        self::assertSame('{"colors":16}', (new Options(colors: 16))->toJson());
        self::assertSame('{"margin":0.0}', (new Options(margin: 0.0))->toJson(), 'a set zero is still sent');
        self::assertSame(
            '{"min_area":4.0,"no_background":true}',
            (new Options(minArea: 4.0, noBackground: true))->toJson(),
            'properties are camelCase, the JSON keeps the tracer\'s own names'
        );
    }
}
