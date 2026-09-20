<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec;

/**
 * The outcome of one trace: the SVG document and the input's size in pixels.
 *
 * The SVG's `width` and `height` attributes carry the same numbers unless `margin` grew
 * them; its `viewBox` can be a smaller coordinate space when the input was reduced for
 * tracing (`max_dim`, or an exact pixel-block upscale that was undone).
 */
final class Traced implements \Stringable
{
    public function __construct(
        /** The SVG document, UTF-8. */
        public readonly string $svg,
        /** Width of the input image in pixels. */
        public readonly int $width,
        /** Height of the input image in pixels. */
        public readonly int $height,
    ) {
    }

    /** The SVG, so a Traced can be echoed or written straight out. */
    public function __toString(): string
    {
        return $this->svg;
    }
}
