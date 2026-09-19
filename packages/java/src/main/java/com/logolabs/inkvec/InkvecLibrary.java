package com.logolabs.inkvec;

import com.sun.jna.Library;
import com.sun.jna.Pointer;

/**
 * The C ABI ({@code include/inkvec.h}), mapped 1:1 through JNA. Library name is
 * {@code inkvec_ffi} (not {@code inkvec}: on Windows its PDB would collide with the command
 * line's {@code inkvec.exe} in one build directory). Every function is safe to call from any
 * number of threads at once, with a separate {@link InkvecResultStruct} per call.
 *
 * <p>{@code len} / {@code svg_len} are {@code size_t} in C; mapped here as Java {@code long}
 * (JNA's 8-byte integer mapping), which matches {@code size_t} on every 64-bit platform this
 * library targets. Strings returned as {@link Pointer} rather than {@link String} so callers
 * decode them explicitly as UTF-8, independent of the JVM's default charset.
 */
interface InkvecLibrary extends Library {

    int inkvec_trace(byte[] bytes, long len, String optionsJson, InkvecResultStruct out);

    int inkvec_trace_rgba(
            byte[] rgba, long len, int width, int height, String optionsJson, InkvecResultStruct out);

    void inkvec_result_free(InkvecResultStruct result);

    Pointer inkvec_version();

    Pointer inkvec_build_target();

    int inkvec_abi_version();

    Pointer inkvec_options_schema();

    Pointer inkvec_default_options();
}
