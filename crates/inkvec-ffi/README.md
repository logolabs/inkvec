# inkvec-ffi -- the C library

Inkvec's C ABI: a shared library (`inkvec_ffi.dll`, `libinkvec_ffi.so`, `libinkvec_ffi.dylib`), a
static one (`inkvec_ffi.lib`, `libinkvec_ffi.a`) and one header, `include/inkvec.h`, generated
by cbindgen from `src/lib.rs`. It is what C and C++ link against and what Java (JNA, Panama),
C# (P/Invoke), Go (cgo), Swift, Ruby and PHP bind to.

```c
#include "inkvec.h"

InkvecResult r = INKVEC_RESULT_INIT;
int status = inkvec_trace(png, png_len, "{\"colors\": 16, \"no_background\": true}", &r);
if (status == INKVEC_OK)
    fwrite(r.svg, 1, r.svg_len, out);
else
    fprintf(stderr, "inkvec: %s\n", r.error);
inkvec_result_free(&r);
```

| Function | |
|---|---|
| `inkvec_trace(bytes, len, options_json, out)` | encoded PNG, JPEG, WebP, GIF, BMP or TIFF |
| `inkvec_trace_rgba(rgba, len, width, height, options_json, out)` | raw straight RGBA8, row-major, `len == width * height * 4` |
| `inkvec_result_free(result)` | releases `svg` and `error`; safe on NULL and twice |
| `inkvec_options_schema()` | JSON Schema of the options (static) |
| `inkvec_default_options()` | every option at its default, as JSON (static) |
| `inkvec_version()`, `inkvec_build_target()`, `inkvec_abi_version()` | identification |

Options are one JSON object -- NULL or `""` for the defaults -- validated by the library against
the schema. The header never changes when an option is added. Status codes: `INKVEC_OK` (0),
`INKVEC_ERR_INVALID_ARGUMENT` (1), `INKVEC_ERR_INVALID_IMAGE` (2), `INKVEC_ERR_INVALID_OPTIONS` (3),
`INKVEC_ERR_INTERNAL` (4). No panic crosses the boundary, every function is thread-safe, and
`struct_size` keeps `InkvecResult` extensible.

A complete program is `examples/c/trace.c`; `tests/test_c_abi.py` drives the library through
ctypes the way a foreign binding does. The full contract, the options table and the build and
linking notes are in [`docs/BINDINGS.md`](../../docs/BINDINGS.md).

Apache-2.0.
