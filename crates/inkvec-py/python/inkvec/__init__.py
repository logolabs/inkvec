"""Inkvec: raster logos, icons and illustrations to compact, accurate SVG.

    import inkvec
    svg = inkvec.trace("logo.png", colors=16).svg

Keyword arguments are the tracer's options, with the command line's defaults. Their
names, types, defaults and ranges come from one schema (``inkvec.options_schema()``,
``inkvec.defaults()``); this module does not restate them, so it never falls out of date
with the Rust library it wraps.
"""

from __future__ import annotations

import json
import math
import os
from typing import Any, Dict, Optional, Tuple

from ._inkvec import (
    InkvecError,
    InternalError,
    InvalidImageError,
    InvalidOptionsError,
    Traced,
    __version__,
    _build_target,
    _default_options_json,
    _options_schema_json,
    _trace,
    _trace_rgba,
)

__all__ = [
    "trace",
    "trace_rgba",
    "options_schema",
    "defaults",
    "build_target",
    "Traced",
    "InkvecError",
    "InvalidImageError",
    "InvalidOptionsError",
    "InternalError",
    "__version__",
]


def options_schema() -> Dict[str, Any]:
    """The JSON Schema of the options: every name, type, default, description and range."""
    return json.loads(_options_schema_json())


def defaults() -> Dict[str, Any]:
    """Every option at its default."""
    return json.loads(_default_options_json())


def build_target() -> str:
    """The target the native library was compiled for, e.g. ``"x86_64-linux-gnu"``.

    Output is byte-identical between builds with the same target; another target can write
    the same drawing slightly differently (subpaths in another order, a last digit).
    """
    return _build_target()


def trace(image: Any, **options: Any) -> Traced:
    """Trace an image to SVG. See the type stub or ``help(inkvec)`` for the options.

    ``image`` is encoded image bytes (PNG, JPEG, WebP, GIF, BMP or TIFF), a path to such a
    file, a binary file object, a Pillow image, or a uint8 numpy array of shape (H, W),
    (H, W, 3) or (H, W, 4).
    """
    opts = _options_json(options)
    if isinstance(image, (str, os.PathLike)):
        with open(image, "rb") as f:
            return _trace(f.read(), opts)
    if isinstance(image, (bytes, bytearray, memoryview)):
        return _trace(bytes(image), opts)
    if hasattr(image, "read"):
        return _trace(bytes(image.read()), opts)
    pixels = _pixels_of(image)
    if pixels is None:
        raise TypeError(
            "trace() takes image bytes, a path, a binary file, a Pillow image or a numpy "
            f"array, not {type(image).__name__}"
        )
    data, width, height = pixels
    return _trace_rgba(data, width, height, opts)


def trace_rgba(
    pixels: Any, width: Optional[int] = None, height: Optional[int] = None, **options: Any
) -> Traced:
    """Trace raw straight-RGBA8 pixels (row-major, tightly packed) to SVG.

    ``pixels`` is a bytes-like buffer of exactly ``width * height * 4`` bytes, or a Pillow
    image or uint8 numpy array, whose size is then read from it.
    """
    opts = _options_json(options)
    found = _pixels_of(pixels)
    if found is not None:
        data, w, h = found
        if (width is not None and width != w) or (height is not None and height != h):
            raise InvalidImageError(f"the pixels are {w}x{h}, not {width}x{height}")
        return _trace_rgba(data, w, h, opts)
    if width is None or height is None:
        raise TypeError("trace_rgba() needs width and height for a flat pixel buffer")
    return _trace_rgba(memoryview(pixels).tobytes(), int(width), int(height), opts)


def _options_json(options: Dict[str, Any]) -> str:
    """The keyword arguments as the JSON object the Rust side parses and validates."""
    for name, value in options.items():
        if isinstance(value, float) and not math.isfinite(value):
            raise InvalidOptionsError(f"`{name}` must be a finite number")
    try:
        return json.dumps(options, default=_plain, allow_nan=False)
    except (TypeError, ValueError) as e:
        raise InvalidOptionsError(str(e)) from None


def _plain(value: Any) -> Any:
    """numpy scalars and the like, as the Python number they hold."""
    item = getattr(value, "item", None)
    if callable(item):
        return item()
    raise TypeError(f"option value {value!r} ({type(value).__name__}) is not a bool or a number")


def _pixels_of(obj: Any) -> Optional[Tuple[bytes, int, int]]:
    """A Pillow image or numpy array as (RGBA8 bytes, width, height); None for anything else."""
    if all(hasattr(obj, a) for a in ("mode", "size", "convert", "tobytes")):
        im = obj if obj.mode == "RGBA" else obj.convert("RGBA")
        w, h = im.size
        return im.tobytes(), int(w), int(h)
    if hasattr(obj, "shape") and hasattr(obj, "dtype"):
        import numpy as np

        a = np.asarray(obj)
        if a.dtype != np.uint8:
            raise InvalidImageError(f"pixel arrays must be uint8, not {a.dtype}")
        if a.ndim == 3 and a.shape[2] == 1:
            a = a[:, :, 0]
        if a.ndim == 2:
            a = np.stack([a, a, a], axis=-1)
        if a.ndim != 3 or a.shape[2] not in (3, 4):
            raise InvalidImageError(
                f"expected an (H, W), (H, W, 3) or (H, W, 4) array, got shape {a.shape}"
            )
        if a.shape[2] == 3:
            a = np.concatenate([a, np.full(a.shape[:2] + (1,), 255, np.uint8)], axis=-1)
        h, w = a.shape[:2]
        return np.ascontiguousarray(a).tobytes(), int(w), int(h)
    return None


def _options_help() -> str:
    lines = ["", "Options (keyword arguments of trace and trace_rgba):", ""]
    for name, prop in options_schema()["properties"].items():
        lines.append(f"  {name} = {prop['default']!r}")
        lines.append(f"      {prop['description']}")
    return "\n".join(lines)


__doc__ = (__doc__ or "") + _options_help()
