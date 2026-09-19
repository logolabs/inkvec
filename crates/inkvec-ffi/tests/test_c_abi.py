"""The C library seen from another language, through nothing but its C ABI.

ctypes stands in for JNA, P/Invoke, cgo and the rest: it loads the shared library, lays out
InkvecResult by hand, reads the status codes from include/inkvec.h, and runs the shared
contract cases (bindings/contract/cases.json) through inkvec_trace and inkvec_trace_rgba.

    cargo build --release -p inkvec-ffi
    python -m pytest crates/inkvec-ffi/tests/test_c_abi.py

INKVEC_LIB overrides the library path (default: target/release/<platform name>).
INKVEC_C_EXAMPLE, when set, is the compiled examples/c/trace.c, which then traces every
encoded contract case too.
"""

from __future__ import annotations

import ctypes
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
CONTRACT = ROOT / "bindings" / "contract"
HEADER = ROOT / "crates" / "inkvec-ffi" / "include" / "inkvec.h"
CASES = json.loads((CONTRACT / "cases.json").read_text(encoding="utf-8"))["cases"]

# The constants every binding needs, read from the header rather than restated.
CONST = {
    m.group(1): int(m.group(2))
    for m in re.finditer(r"#define (INKVEC_\w+) (\d+)", HEADER.read_text(encoding="utf-8"))
}
ERROR_KINDS = {
    CONST["INKVEC_ERR_INVALID_IMAGE"]: "invalid_image",
    CONST["INKVEC_ERR_INVALID_OPTIONS"]: "invalid_options",
    CONST["INKVEC_ERR_INTERNAL"]: "internal",
}


class InkvecResult(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_uint32),
        ("status", ctypes.c_int32),
        ("svg", ctypes.c_void_p),
        ("svg_len", ctypes.c_size_t),
        ("width", ctypes.c_uint32),
        ("height", ctypes.c_uint32),
        ("error", ctypes.c_void_p),
    ]


def fresh() -> InkvecResult:
    return InkvecResult(struct_size=ctypes.sizeof(InkvecResult))


def library_path() -> Path:
    if os.environ.get("INKVEC_LIB"):
        return Path(os.environ["INKVEC_LIB"])
    name = {"win32": "inkvec_ffi.dll", "darwin": "libinkvec_ffi.dylib"}.get(
        sys.platform, "libinkvec_ffi.so"
    )
    return ROOT / "target" / "release" / name


@pytest.fixture(scope="module")
def lib():
    path = library_path()
    if not path.exists():
        pytest.skip(f"{path} not built (cargo build --release -p inkvec-ffi)")
    lib = ctypes.CDLL(str(path))
    p_result = ctypes.POINTER(InkvecResult)
    lib.inkvec_trace.argtypes = [ctypes.c_char_p, ctypes.c_size_t, ctypes.c_char_p, p_result]
    lib.inkvec_trace.restype = ctypes.c_int32
    lib.inkvec_trace_rgba.argtypes = [
        ctypes.c_char_p, ctypes.c_size_t, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_char_p, p_result,
    ]
    lib.inkvec_trace_rgba.restype = ctypes.c_int32
    lib.inkvec_result_free.argtypes = [p_result]
    lib.inkvec_result_free.restype = None
    for name in ("inkvec_version", "inkvec_build_target", "inkvec_options_schema", "inkvec_default_options"):
        getattr(lib, name).restype = ctypes.c_char_p
    lib.inkvec_abi_version.restype = ctypes.c_uint32
    return lib


def run_case(lib, case) -> tuple:
    """(expect block, SVG bytes or None) for one contract case, through the C ABI."""
    data = (CONTRACT / case["input"]).read_bytes()
    options = json.dumps(case["options"]).encode("utf-8")
    r = fresh()
    if case["form"] == "encoded":
        status = lib.inkvec_trace(data, len(data), options, ctypes.byref(r))
    else:
        status = lib.inkvec_trace_rgba(
            data, len(data), case["width"], case["height"], options, ctypes.byref(r)
        )
    try:
        assert status == r.status
        if status == CONST["INKVEC_OK"]:
            assert not r.error
            return {"width": r.width, "height": r.height}, ctypes.string_at(r.svg, r.svg_len)
        assert not r.svg and r.svg_len == 0
        assert ctypes.string_at(r.error).decode("utf-8")
        return {"error": ERROR_KINDS[status]}, None
    finally:
        lib.inkvec_result_free(ctypes.byref(r))
        assert not r.svg and not r.error


@pytest.mark.parametrize("case", CASES, ids=[c["name"] for c in CASES])
def test_contract(lib, case):
    expect, svg = run_case(lib, case)
    assert expect == case["expect"]
    want = case.get("svg", {}).get(lib.inkvec_build_target().decode())
    if svg is not None and want is not None:
        assert {"bytes": len(svg), "sha256": hashlib.sha256(svg).hexdigest()} == want
    if "same_svg_as" in case:
        other = next(c for c in CASES if c["name"] == case["same_svg_as"])
        assert svg == run_case(lib, other)[1]


def test_static_strings(lib):
    version = lib.inkvec_version().decode()
    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    assert re.search(rf'^version = "{re.escape(version)}"', cargo, re.M)
    assert lib.inkvec_abi_version() == CONST["INKVEC_ABI_VERSION"]
    schema = json.loads(lib.inkvec_options_schema())
    defaults = json.loads(lib.inkvec_default_options())
    assert set(defaults) == set(schema["properties"])
    committed = json.loads((ROOT / "bindings" / "options.schema.json").read_text(encoding="utf-8"))
    assert schema == committed


def test_a_short_struct_is_refused_untouched(lib):
    r = fresh()
    r.struct_size = 8
    r.status = 77
    status = lib.inkvec_trace(b"x", 1, None, ctypes.byref(r))
    assert status == CONST["INKVEC_ERR_INVALID_ARGUMENT"]
    assert r.status == 77 and not r.error


def test_c_example(lib, tmp_path):
    exe = os.environ.get("INKVEC_C_EXAMPLE")
    if not exe:
        pytest.skip("INKVEC_C_EXAMPLE not set")
    target = lib.inkvec_build_target().decode()
    for case in (c for c in CASES if c["form"] == "encoded"):
        out = tmp_path / f"{case['name']}.svg"
        args = [exe, str(CONTRACT / case["input"]), str(out), json.dumps(case["options"])]
        p = subprocess.run(args, capture_output=True)
        if "error" in case["expect"]:
            assert p.returncode == 1, case["name"]
            continue
        assert p.returncode == 0, p.stderr.decode(errors="replace")
        want = case.get("svg", {}).get(target)
        if want is not None:
            assert hashlib.sha256(out.read_bytes()).hexdigest() == want["sha256"], case["name"]
