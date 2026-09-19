package com.logolabs.inkvec;

import com.sun.jna.Pointer;
import com.sun.jna.Structure;
import com.sun.jna.Structure.FieldOrder;

/**
 * Mirrors the C {@code InkvecResult} struct field for field (see {@code include/inkvec.h}).
 * {@code svg_len} is declared {@code long}: every platform this library targets is 64-bit, so
 * a native {@code size_t} and JNA's 8-byte {@code long} mapping agree.
 *
 * <p>Zero-initialised and {@code struct_size} set by the constructor, exactly like
 * {@code INKVEC_RESULT_INIT} in the header. Passed by reference to every native call; JNA
 * writes it to native memory before the call and reads it back after, so the fields below
 * reflect the library's output without any explicit {@code read()}.
 */
@FieldOrder({"struct_size", "status", "svg", "svg_len", "width", "height", "error"})
public final class InkvecResultStruct extends Structure implements Structure.ByReference {
    public int struct_size;
    public int status;
    public Pointer svg;
    public long svg_len;
    public int width;
    public int height;
    public Pointer error;

    public InkvecResultStruct() {
        super();
        this.struct_size = size();
    }
}
