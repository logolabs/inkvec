/*
 * Trace an image file to SVG through the Inkvec C API.
 *
 *   trace <input-image> [output.svg] [options-json]
 *
 *   trace logo.png logo.svg '{"colors": 16, "cutout": true}'
 *
 * Without an output path the SVG goes to stdout. Build against the library and header:
 *
 *   cc  trace.c -I ../../include -L <lib-dir> -linkvec_ffi -o trace      (Linux, macOS)
 *   cl  trace.c /I ..\..\include <lib-dir>\inkvec_ffi.dll.lib            (Windows, MSVC)
 */
#ifdef _MSC_VER
#define _CRT_SECURE_NO_WARNINGS /* fopen is fine here; MSVC would rather it were fopen_s */
#endif
#include <stdio.h>
#include <stdlib.h>

#include "inkvec.h"

/* The whole file in one malloc'd buffer, or NULL. */
static unsigned char *read_file(const char *path, size_t *len) {
    FILE *f = fopen(path, "rb");
    unsigned char *buf = NULL;
    long n;
    if (!f) return NULL;
    if (fseek(f, 0, SEEK_END) == 0 && (n = ftell(f)) >= 0 && fseek(f, 0, SEEK_SET) == 0) {
        buf = (unsigned char *)malloc(n > 0 ? (size_t)n : 1);
        if (buf && fread(buf, 1, (size_t)n, f) != (size_t)n) {
            free(buf);
            buf = NULL;
        }
        *len = (size_t)n;
    }
    fclose(f);
    return buf;
}

int main(int argc, char **argv) {
    size_t len = 0;
    unsigned char *bytes;
    const char *options = argc > 3 ? argv[3] : NULL; /* NULL: every option at its default */
    InkvecResult r = INKVEC_RESULT_INIT;
    int status;
    FILE *out;

    if (argc < 2) {
        fprintf(stderr, "usage: %s <input-image> [output.svg] [options-json]\n", argv[0]);
        return 2;
    }
    bytes = read_file(argv[1], &len);
    if (!bytes) {
        perror(argv[1]);
        return 1;
    }
    fprintf(stderr, "inkvec %s (C ABI %u)\n", inkvec_version(), (unsigned)inkvec_abi_version());

    status = inkvec_trace(bytes, len, options, &r);
    free(bytes);
    if (status != INKVEC_OK) {
        fprintf(stderr, "error %d: %s\n", status, r.error);
        inkvec_result_free(&r);
        return 1;
    }

    out = argc > 2 ? fopen(argv[2], "wb") : stdout;
    if (!out) {
        perror(argv[2]);
        inkvec_result_free(&r);
        return 1;
    }
    fwrite(r.svg, 1, r.svg_len, out);
    if (out != stdout) fclose(out);
    fprintf(stderr, "%ux%u, %lu bytes of SVG\n", (unsigned)r.width, (unsigned)r.height,
            (unsigned long)r.svg_len);

    inkvec_result_free(&r); /* releases r.svg; r.error is NULL on success */
    return 0;
}
