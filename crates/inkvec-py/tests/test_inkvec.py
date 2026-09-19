"""The Python package against its promises and the shared contract.

    pip install crates/inkvec-py[test]      # or: maturin develop -m crates/inkvec-py/Cargo.toml
    python -m pytest crates/inkvec-py/tests
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import re
import threading
import time
import xml.etree.ElementTree as ET
from pathlib import Path

import pytest

import inkvec

ROOT = Path(__file__).resolve().parents[3]
CONTRACT = ROOT / "bindings" / "contract"
CASES = json.loads((CONTRACT / "cases.json").read_text(encoding="utf-8"))["cases"]
SVG_NS = "{http://www.w3.org/2000/svg}"

ERRORS = {
    "invalid_image": inkvec.InvalidImageError,
    "invalid_options": inkvec.InvalidOptionsError,
    "internal": inkvec.InternalError,
}


def tiny() -> bytes:
    return (CONTRACT / "tiny.png").read_bytes()


def run_case(case: dict) -> inkvec.Traced:
    data = (CONTRACT / case["input"]).read_bytes()
    if case["form"] == "encoded":
        return inkvec.trace(data, **case["options"])
    return inkvec.trace_rgba(data, case["width"], case["height"], **case["options"])


@pytest.mark.parametrize("case", CASES, ids=[c["name"] for c in CASES])
def test_contract(case):
    expect = case["expect"]
    if "error" in expect:
        with pytest.raises(ERRORS[expect["error"]]):
            run_case(case)
        return
    t = run_case(case)
    assert {"width": t.width, "height": t.height} == expect
    # The exact bytes are recorded per build target; see inkvec.build_target().
    want = case.get("svg", {}).get(inkvec.build_target())
    if want is not None:
        svg = t.svg.encode("utf-8")
        assert {"bytes": len(svg), "sha256": hashlib.sha256(svg).hexdigest()} == want


@pytest.mark.parametrize(
    "case", [c for c in CASES if "same_svg_as" in c], ids=lambda c: c["name"]
)
def test_contract_pairs_agree(case):
    other = next(c for c in CASES if c["name"] == case["same_svg_as"])
    assert run_case(case).svg == run_case(other).svg


def test_a_png_traces_to_valid_svg():
    t = inkvec.trace(tiny())
    root = ET.fromstring(t.svg)
    assert root.tag == SVG_NS + "svg"
    assert (t.width, t.height) == (96, 96)
    assert root.get("width") == "96" and root.get("height") == "96"
    assert str(t) == t.svg and t._repr_svg_() == t.svg
    assert "96x96" in repr(t)


def test_paths_and_file_objects_are_read(tmp_path):
    p = tmp_path / "tiny.png"
    p.write_bytes(tiny())
    expected = inkvec.trace(tiny()).svg
    assert inkvec.trace(p).svg == expected
    assert inkvec.trace(str(p)).svg == expected
    with open(p, "rb") as f:
        assert inkvec.trace(f).svg == expected
    assert inkvec.trace(bytearray(tiny())).svg == expected


def test_white_on_transparent_with_cutout_paints_no_background():
    data = (CONTRACT / "white_on_clear.rgba").read_bytes()
    t = inkvec.trace_rgba(data, 64, 64, cutout=True)
    root = ET.fromstring(t.svg)
    shapes = [el for el in root.iter() if el.tag.split("}")[-1] in ("path", "rect", "circle", "ellipse")]
    assert shapes, t.svg
    for el in shapes:
        # Nothing spans the canvas: no background rect, no path through its corners.
        if el.tag == SVG_NS + "rect":
            assert float(el.get("width")) < 60 and float(el.get("height")) < 60, t.svg
        assert "-0.50,-0.50" not in (el.get("d") or ""), t.svg
        # The artwork survives, in its own colour.
        assert el.get("fill") in ("#ffffff", "#fff"), t.svg
    # Control: an opaque image does paint its canvas.
    opaque = ET.fromstring(inkvec.trace(tiny()).svg)
    assert any(
        el.tag == SVG_NS + "rect" and float(el.get("width")) >= 96 for el in opaque.iter()
    )


def test_output_is_deterministic():
    first = inkvec.trace(tiny(), colors=16).svg
    assert all(inkvec.trace(tiny(), colors=16).svg == first for _ in range(3))


def test_pillow_and_numpy_inputs_match_the_file():
    Image = pytest.importorskip("PIL.Image")
    np = pytest.importorskip("numpy")
    import io

    expected = inkvec.trace(tiny()).svg
    im = Image.open(io.BytesIO(tiny()))
    assert inkvec.trace(im).svg == expected
    rgba = np.asarray(im.convert("RGBA"))
    assert inkvec.trace_rgba(rgba).svg == expected
    assert inkvec.trace(rgba).svg == expected
    assert inkvec.trace(np.asarray(im.convert("RGB"))).svg == expected
    assert inkvec.trace_rgba(rgba.tobytes(), 96, 96).svg == expected
    assert inkvec.trace_rgba(memoryview(rgba.tobytes()), 96, 96).svg == expected
    with pytest.raises(inkvec.InvalidImageError):
        inkvec.trace_rgba(rgba.astype(np.float32))
    with pytest.raises(inkvec.InvalidImageError):
        inkvec.trace_rgba(rgba, 95, 96)
    # numpy scalars are fine as option values.
    assert inkvec.trace(tiny(), colors=np.int64(16)).svg == inkvec.trace(tiny(), colors=16).svg


@pytest.mark.parametrize(
    "options, needle",
    [
        ({"colours": 8}, "colours"),
        ({"colors": 0}, "colors"),
        ({"colors": 16.5}, "u32"),
        ({"precision": float("nan")}, "precision"),
        ({"harmonize_threshold": 2}, "harmonize_threshold"),
        ({"cutout": "yes"}, "bool"),
        ({"minify": object()}, "not a bool or a number"),
    ],
)
def test_bad_options_raise_naming_the_problem(options, needle):
    with pytest.raises(inkvec.InvalidOptionsError, match=re.escape(needle)):
        inkvec.trace(tiny(), **options)
    assert issubclass(inkvec.InvalidOptionsError, inkvec.InkvecError)


def test_bad_inputs():
    with pytest.raises(inkvec.InvalidImageError):
        inkvec.trace(b"not an image")
    with pytest.raises(inkvec.InvalidImageError):
        inkvec.trace_rgba(b"\0" * 15, 2, 2)
    with pytest.raises(TypeError):
        inkvec.trace(12345)
    with pytest.raises(TypeError):
        inkvec.trace_rgba(b"\0" * 16)


def test_schema_and_defaults_describe_the_keyword_arguments():
    schema = inkvec.options_schema()
    defaults = inkvec.defaults()
    assert schema["title"] == "InkvecOptions"
    assert list(defaults) == list(schema["properties"])
    for name, prop in schema["properties"].items():
        assert defaults[name] == prop["default"]
    assert defaults["colors"] == 64 and defaults["harmonize"] is True
    committed = json.loads((ROOT / "bindings" / "options.schema.json").read_text(encoding="utf-8"))
    assert schema == committed
    # Passing every default explicitly is the same as passing none.
    assert inkvec.trace(tiny(), **defaults).svg == inkvec.trace(tiny()).svg
    assert "harmonize_threshold" in inkvec.__doc__


def test_generated_bindings_are_current():
    spec = importlib.util.spec_from_file_location("generate", ROOT / "bindings" / "codegen" / "generate.py")
    gen = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gen)
    stale = [p.relative_to(ROOT).as_posix() for p in gen.stale()]
    assert not stale, f"regenerate with `python bindings/codegen/generate.py`: {stale}"


def test_the_gil_is_released_while_tracing():
    big = (ROOT / "web" / "samples" / "1075thefan.png").read_bytes()
    done = threading.Event()
    result = {}

    def work():
        result["t"] = inkvec.trace(big)
        done.set()

    worker = threading.Thread(target=work)
    worker.start()
    ticks = 0
    start = time.perf_counter()
    while not done.is_set():
        ticks += 1
    elapsed = time.perf_counter() - start
    worker.join()
    assert result["t"].width == 512
    # With the GIL held for the whole trace this loop would barely run.
    assert elapsed < 0.05 or ticks > 10_000, (ticks, elapsed)


def test_concurrent_calls_are_independent():
    expected = inkvec.trace(tiny()).svg
    out = [None] * 4

    def work(i):
        out[i] = inkvec.trace(tiny()).svg

    threads = [threading.Thread(target=work, args=(i,)) for i in range(4)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert out == [expected] * 4


def test_version_is_the_workspace_version():
    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    assert re.search(rf'^version = "{re.escape(inkvec.__version__)}"', cargo, re.M)
    assert re.match(r"^(x86_64|aarch64|x86|arm|wasm32)[a-z0-9_]*-[a-z]+", inkvec.build_target())


def test_packaged_licence_files_match_the_repository():
    for name in ("LICENSE", "NOTICE"):
        assert (ROOT / "crates" / "inkvec-py" / name).read_bytes() == (ROOT / name).read_bytes(), name
