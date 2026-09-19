package com.logolabs.inkvec;

/**
 * The outcome of one trace: the SVG document and the input's pixel size.
 *
 * <p>{@code width} and {@code height} are the input's pixel dimensions -- the same numbers
 * the SVG's {@code width}/{@code height} attributes carry, unless {@code margin} grew them.
 */
public final class TraceResult {
    private final String svg;
    private final int width;
    private final int height;

    TraceResult(String svg, int width, int height) {
        this.svg = svg;
        this.width = width;
        this.height = height;
    }

    /** The SVG document, UTF-8 decoded. */
    public String svg() {
        return svg;
    }

    /** Width of the input image, in pixels. */
    public int width() {
        return width;
    }

    /** Height of the input image, in pixels. */
    public int height() {
        return height;
    }

    @Override
    public String toString() {
        return "TraceResult{" + width + "x" + height + ", " + svg.length() + " chars of SVG}";
    }
}
