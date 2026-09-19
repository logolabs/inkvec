package com.logolabs.inkvec;

import com.sun.jna.Native;
import com.sun.jna.Pointer;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Objects;

/**
 * Inkvec: raster logos, icons and illustrations to compact, accurate SVG.
 *
 * <p>This is the Java binding over Inkvec's C ABI ({@code include/inkvec.h}), loaded through
 * <a href="https://github.com/java-native-access/jna">JNA</a>. It contains no per-option
 * logic: options are either a generated, typed {@link InkvecOptions} or a raw JSON string,
 * and the native library ({@code inkvec::Options::from_json}) is the only place a value is
 * defaulted or validated. Every method may be called from any number of threads at once.
 *
 * <pre>{@code
 * byte[] png = Files.readAllBytes(Path.of("logo.png"));
 * TraceResult r = Inkvec.trace(png, InkvecOptions.builder().colors(16).build());
 * Files.writeString(Path.of("logo.svg"), r.svg());
 * }</pre>
 *
 * <h2>Loading the native library</h2>
 *
 * The library name is {@code inkvec_ffi}. JNA looks for it, in order: the system property
 * {@code jna.library.path} (a shortcut, the system property {@code inkvec.library.path}, is
 * folded into it if set before this class loads), the classpath under JNA's per-platform
 * resource prefix ({@code win32-x86-64/inkvec_ffi.dll}, {@code linux-x86-64/libinkvec_ffi.so},
 * {@code linux-aarch64/...}, {@code darwin-x86-64/...}, {@code darwin-aarch64/...} -- where a
 * release jar bundles it), then the platform's normal library search path.
 */
public final class Inkvec {

    private static final int INKVEC_OK = 0;
    private static final int INKVEC_ERR_INVALID_ARGUMENT = 1;
    private static final int INKVEC_ERR_INVALID_IMAGE = 2;
    private static final int INKVEC_ERR_INVALID_OPTIONS = 3;
    private static final int INKVEC_ERR_INTERNAL = 4;

    private static final InkvecLibrary NATIVE;

    static {
        // A project-specific alias for jna.library.path, folded in before JNA resolves the
        // library, so a caller need not know JNA's own property name.
        String extra = System.getProperty("inkvec.library.path");
        if (extra != null && !extra.isEmpty()) {
            String existing = System.getProperty("jna.library.path", "");
            String merged = existing.isEmpty() ? extra : existing + java.io.File.pathSeparator + extra;
            System.setProperty("jna.library.path", merged);
        }
        // Marshal every String (options JSON) as UTF-8, independent of the platform default
        // charset; SVG and error text go through explicit Pointer decoding below regardless.
        if (System.getProperty("jna.encoding") == null) {
            System.setProperty("jna.encoding", "UTF-8");
        }
        NATIVE = Native.load("inkvec_ffi", InkvecLibrary.class);
    }

    private Inkvec() {
    }

    /** Trace an encoded image (PNG, JPEG, WebP, GIF, BMP or TIFF) with every option at its default. */
    public static TraceResult trace(byte[] image) {
        return trace(image, (String) null);
    }

    /** Trace an encoded image with the given typed options. */
    public static TraceResult trace(byte[] image, InkvecOptions options) {
        return trace(image, options.toJson());
    }

    /**
     * Trace an encoded image with options as a raw JSON object -- an escape hatch for an
     * option added to Inkvec before {@link InkvecOptions} is regenerated for it.
     */
    public static TraceResult trace(byte[] image, String optionsJson) {
        Objects.requireNonNull(image, "image");
        InkvecResultStruct out = new InkvecResultStruct();
        try {
            int status = NATIVE.inkvec_trace(image, image.length, optionsJson, out);
            return result(status, out);
        } finally {
            NATIVE.inkvec_result_free(out);
        }
    }

    /** Reads the whole file, then traces it with every option at its default. */
    public static TraceResult trace(Path path) throws IOException {
        return trace(Files.readAllBytes(path), (String) null);
    }

    /** Reads the whole file, then traces it with the given typed options. */
    public static TraceResult trace(Path path, InkvecOptions options) throws IOException {
        return trace(Files.readAllBytes(path), options.toJson());
    }

    /** Reads the whole file, then traces it with options as a raw JSON object. */
    public static TraceResult trace(Path path, String optionsJson) throws IOException {
        return trace(Files.readAllBytes(path), optionsJson);
    }

    /**
     * Trace raw pixels: straight (not premultiplied) RGBA, 8 bits per channel, row-major,
     * tightly packed, exactly {@code width * height * 4} bytes. Every option at its default.
     * Byte-identical to {@link #trace(byte[])} on a PNG holding the same pixels.
     */
    public static TraceResult traceRgba(byte[] rgba, int width, int height) {
        return traceRgba(rgba, width, height, (String) null);
    }

    /** Trace raw RGBA pixels with the given typed options. */
    public static TraceResult traceRgba(byte[] rgba, int width, int height, InkvecOptions options) {
        return traceRgba(rgba, width, height, options.toJson());
    }

    /** Trace raw RGBA pixels with options as a raw JSON object. */
    public static TraceResult traceRgba(byte[] rgba, int width, int height, String optionsJson) {
        Objects.requireNonNull(rgba, "rgba");
        InkvecResultStruct out = new InkvecResultStruct();
        try {
            int status = NATIVE.inkvec_trace_rgba(rgba, rgba.length, width, height, optionsJson, out);
            return result(status, out);
        } finally {
            NATIVE.inkvec_result_free(out);
        }
    }

    /** The JSON Schema (draft 2020-12) of the options: every name, type, default, range. */
    public static String optionsSchema() {
        return decode(NATIVE.inkvec_options_schema());
    }

    /** Every option at its default, as a compact JSON object. */
    public static String defaults() {
        return decode(NATIVE.inkvec_default_options());
    }

    /** The library version, e.g. {@code "0.1.3"} -- read from the loaded native library. */
    public static String version() {
        return decode(NATIVE.inkvec_version());
    }

    /**
     * The target the native library was compiled for, e.g. {@code "x86_64-windows-msvc"}.
     * Output is byte-identical between builds with the same target; another target can write
     * the same drawing slightly differently (see docs/BINDINGS.md, "Determinism").
     */
    public static String buildTarget() {
        return decode(NATIVE.inkvec_build_target());
    }

    /** The C ABI version of the loaded native library, for a compatibility check at startup. */
    public static int abiVersion() {
        return NATIVE.inkvec_abi_version();
    }

    private static TraceResult result(int status, InkvecResultStruct out) {
        if (status == INKVEC_OK) {
            // svg_len is authoritative (the header calls svg NUL-terminated too, but this
            // does not depend on that): read exactly that many bytes and decode as UTF-8.
            byte[] bytes = out.svg.getByteArray(0, (int) out.svg_len);
            String svg = new String(bytes, StandardCharsets.UTF_8);
            return new TraceResult(svg, out.width, out.height);
        }
        String message = out.error == null ? "" : out.error.getString(0, StandardCharsets.UTF_8.name());
        switch (status) {
            case INKVEC_ERR_INVALID_IMAGE:
                throw new InvalidImageException(message);
            case INKVEC_ERR_INVALID_OPTIONS:
                throw new InvalidOptionsException(message);
            case INKVEC_ERR_INTERNAL:
                throw new InternalException(message);
            case INKVEC_ERR_INVALID_ARGUMENT:
                throw new IllegalArgumentException(message);
            default:
                throw new InternalException("unknown status " + status + ": " + message);
        }
    }

    private static String decode(Pointer p) {
        return p.getString(0, StandardCharsets.UTF_8.name());
    }
}
