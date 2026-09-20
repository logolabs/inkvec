<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec\Tests;

use LogoLabs\Inkvec\Inkvec;
use LogoLabs\Inkvec\InkvecException;
use LogoLabs\Inkvec\InvalidImageException;
use LogoLabs\Inkvec\InvalidOptionsException;
use LogoLabs\Inkvec\Library;
use LogoLabs\Inkvec\LibraryException;
use PHPUnit\Framework\TestCase;

/** Every failure is an InkvecException of the kind the C ABI reported, carrying its message. */
final class ErrorTest extends TestCase
{
    private function png(): string
    {
        $dir = Fixtures::contractDir();
        if ($dir === null) {
            self::markTestSkipped('the contract fixtures were not shipped with this copy of the package');
        }

        return (string) file_get_contents($dir . '/tiny.png');
    }

    public function testNotAnImage(): void
    {
        $this->expectException(InvalidImageException::class);
        Inkvec::trace('not an image at all');
    }

    public function testUnknownOptionNamesIt(): void
    {
        try {
            Inkvec::trace($this->png(), ['colours' => 8]);
            self::fail('expected an InvalidOptionsException');
        } catch (InvalidOptionsException $e) {
            self::assertStringContainsString('colours', $e->getMessage());
            self::assertInstanceOf(InkvecException::class, $e);
        }
    }

    public function testValueOutOfRange(): void
    {
        $this->expectException(InvalidOptionsException::class);
        Inkvec::trace($this->png(), ['colors' => 0]);
    }

    public function testValueOfTheWrongType(): void
    {
        $this->expectException(InvalidOptionsException::class);
        Inkvec::trace($this->png(), ['cutout' => 'yes']);
    }

    public function testMalformedJsonOptions(): void
    {
        $this->expectException(InvalidOptionsException::class);
        Inkvec::trace($this->png(), '{"colors": ');
    }

    public function testRgbaThatDoesNotMatchTheSizeGiven(): void
    {
        $this->expectException(InvalidImageException::class);
        Inkvec::traceRgba(str_repeat("\0", 64 * 64 * 4), 60, 64);
    }

    public function testAnUnreadableFile(): void
    {
        $this->expectException(InkvecException::class);
        Inkvec::traceFile(sys_get_temp_dir() . '/inkvec-no-such-file.png');
    }

    public function testAFailedTraceLeavesTheLibraryUsable(): void
    {
        try {
            Inkvec::trace($this->png(), ['colors' => 0]);
        } catch (InvalidOptionsException) {
            // expected
        }

        self::assertSame(96, Inkvec::trace($this->png())->width);
    }

    public function testAMissingLibraryIsALibraryException(): void
    {
        Inkvec::version(); // make sure a library is loaded, whatever order the tests ran in

        // A different library cannot be swapped in underneath a loaded one: the refusal
        // leaves the loaded one in place, rather than half-changing the process.
        $this->expectException(LibraryException::class);
        Inkvec::useLibrary(sys_get_temp_dir() . '/no-such-' . Library::soname());
    }
}
