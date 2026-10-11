"""Geometric match to the artist's file: how far the trace's edges are from the artist's.

dE00 asks whether two renders *look* alike, pixel by pixel; a boundary 0.1 px off along a
black-on-white edge and a slightly wrong fill colour can cost it the same. This asks the
geometric question directly: paint both files flat (no anti-aliasing), give every pixel the
artist's colour it shows, and measure the area where the two disagree. Divided by the length
of the artist's own boundaries, that area is the **mean edge displacement**: the average
distance, in pixels of the traced raster, between where the trace puts an edge and where
the artist did. A trace whose every edge is moved out by `δ` reads `δ` (the area between two
curves `δ` apart is `δ` times their length), whatever the colours, which is what makes it a
calibrated measure of "the same design" rather than of "a similar picture".

* `geom`      the mean edge displacement, source px: the mismatched area over the artist's
              boundary length.
* `geom_far`  the part of it farther than one source pixel (city-block) from any artist
              edge, in the same units: missing or extra features, wrong colours over whole
              regions, rather than edges a little off.

Both read 0 for the artist's own file. Each pixel takes the nearest of the artist's colours
(in CIELAB, with alpha as a fourth coordinate, so a transparent hole is its own colour);
shaded art with more than `MAX_COLOURS` colours has them clustered first, so a gradient is
compared as a few bands rather than as noise. Both files are drawn by the
gate's own renderer and viewBox fitting, at `RES` times the input's size, so they share
one frame.

The boundary length is the four-direction Cauchy-Crofton estimate from the artist's label
map: a curve of length `L` crosses the rows, columns and two diagonals `∫(|sin θ| + |cos θ|
+ |sin(θ − 45°)| + |cos(θ − 45°)|) ds` times in all (diagonals counted at their spacing
`1/√2`), which averages `8/π` per unit length; within 5 % for any direction.

Method from: the symmetric difference of two regions as their distance (the area
difference metric of shape matching; e.g. Veltkamp, Hagedoorn (2001), State of the art in
shape matching, Principles of Visual Information Retrieval, Springer, doi:10.1007/978-1-4471-3702-3_4);
the length estimate from Cauchy-Crofton (Santaló 1976, Integral Geometry and Geometric
Probability). Not from the literature: area over length as a calibrated mean displacement
between a vector truth and a vector trace.
"""

from __future__ import annotations

import hashlib
import os
import re
from dataclasses import dataclass
from pathlib import Path

import numpy as np

#: Render pixels per pixel of the input raster: the flat renders are `RES` times the input's
#: size (2048 px for a 512 px input). The area is point-sampled, so it is unbiased at any
#: size and this sets only its noise.
RES = 4
#: More distinct artist colours than this are clustered (shaded emoji).
MAX_COLOURS = 48
#: Alpha's weight beside CIELAB (0-100) when matching colours.
ALPHA_WEIGHT = 100.0


def _crisp(svg: str) -> str:
    """`svg` drawn without anti-aliasing: `shape-rendering="crispEdges"` on its root."""
    return re.sub(r"<svg\b", '<svg shape-rendering="crispEdges"', svg, count=1)


def _flat(svg: str, size: int) -> np.ndarray:
    """`svg` drawn without anti-aliasing at `size` px, as 8-bit RGBA, in the gate's frame
    (`render.fit_viewbox`)."""
    import io
    from PIL import Image
    from inkvec_bench import render
    png = render.render_to_png(_crisp(svg), size, size)
    img = Image.open(io.BytesIO(png)).convert("RGBA")
    if img.size != (size, size):
        img = img.resize((size, size), Image.NEAREST)
    return np.asarray(img, dtype=np.uint8)


def _codes(rgba: np.ndarray, opaque: bool) -> np.ndarray:
    """Per pixel, its 8-bit RGBA packed in one integer (over white, opaque, when the
    input was flattened onto a white page).

    Over white, each channel is `(c a + 255 (255 - a) + 127) // 255`. That is `c` itself
    where `a` is 255 and 255 where `a` is 0, so only the partly covered pixels (few, in a
    render without anti-aliasing) are computed; the packing reads the four bytes as one
    big-endian word. Both give the same integers as the arithmetic on every pixel."""
    u = np.ascontiguousarray(rgba, dtype=np.uint8)
    if opaque:
        a = u[..., 3]
        part = (a != 0) & (a != 255)
        u = u.copy()
        u[a == 0] = 255
        if part.any():
            p = u[part].astype(np.uint32)
            pa = p[:, 3:4]
            p[:, :3] = (p[:, :3] * pa + 255 * (255 - pa) + 127) // 255
            u[part] = p.astype(np.uint8)
        u[..., 3] = 255
    return u.view(">u4")[..., 0].astype(np.uint32)


def _features(codes: np.ndarray) -> np.ndarray:
    """CIELAB of each packed colour over white, and its alpha scaled to match."""
    from skimage.color import rgb2lab
    c = codes.astype(np.uint64)
    rgba = np.stack([(c >> 24) & 255, (c >> 16) & 255, (c >> 8) & 255, c & 255], -1) / 255.0
    a = rgba[:, 3:4]
    rgb = rgba[:, :3] * a + (1.0 - a)
    lab = rgb2lab(np.clip(rgb, 0, 1)[:, None, :])[:, 0, :]
    return np.concatenate([lab, ALPHA_WEIGHT * a], axis=-1)


def _palette(feat: np.ndarray, counts: np.ndarray) -> np.ndarray:
    """The artist's colours (`feat`, one row per distinct colour, `counts` pixels each):
    all of them, or `MAX_COLOURS` clusters (weighted k-means, seeded with the most
    frequent colours, fixed iterations)."""
    if len(feat) <= MAX_COLOURS:
        return feat
    order = np.argsort(-counts, kind="stable")
    centres = feat[order[:MAX_COLOURS]].copy()
    w = counts.astype(np.float64)
    for _ in range(12):
        lab = ((feat[:, None, :] - centres[None, :, :]) ** 2).sum(-1).argmin(1)
        for k in range(len(centres)):
            m = lab == k
            if m.any():
                centres[k] = (feat[m] * w[m, None]).sum(0) / w[m].sum()
    return centres


def _labels(codes: np.ndarray, palette: np.ndarray) -> np.ndarray:
    """Index of the nearest palette colour per pixel.

    Read run by run along the rows: a flat render is long runs of one colour, so the
    distinct colours and their nearest palette entries are found from the runs' values
    (a few thousand, not millions) and spread back over the runs. The same distinct colours
    in the same sorted order as `np.unique` of every pixel, so the same labels."""
    flat = codes.reshape(-1)
    if flat.size == 0:
        return np.zeros(codes.shape, dtype=np.intp)
    starts = np.flatnonzero(flat[1:] != flat[:-1]) + 1
    starts = np.concatenate(([0], starts))
    uniq, inv = np.unique(flat[starts], return_inverse=True)
    d = ((_features(uniq)[:, None, :] - palette[None, :, :]) ** 2).sum(-1)
    per_run = d.argmin(1)[inv.reshape(-1)]
    runs = np.diff(np.append(starts, flat.size))
    return np.repeat(per_run, runs).reshape(codes.shape)


def boundary_length(lab: np.ndarray) -> float:
    """Four-direction Cauchy-Crofton length of the label map's boundaries, render px."""
    n_h = np.count_nonzero(lab[:, 1:] != lab[:, :-1])
    n_v = np.count_nonzero(lab[1:, :] != lab[:-1, :])
    n_d = np.count_nonzero(lab[1:, 1:] != lab[:-1, :-1])
    n_a = np.count_nonzero(lab[1:, :-1] != lab[:-1, 1:])
    return float(np.pi / 8.0 * (n_h + n_v + (n_d + n_a) / np.sqrt(2.0)))


@dataclass(frozen=True)
class ArtistSide:
    """Everything `geomatch` reads from the artist's file at one size: a function of the
    file's bytes, the size and the page (over white or not), so kept between runs."""

    labels: np.ndarray    # nearest-palette index per render pixel
    palette: np.ndarray   # the artist's colours (`_palette`), one row each
    length: float         # boundary length of `labels`, render px (`boundary_length`)
    near: np.ndarray      # within one source pixel (city-block, RES render px) of an edge


#: Where `artist_side` keeps its results between runs (None: recomputed every time).
#: `svgeval` points it at its cache. Entries are keyed by everything that changes them.
CACHE_DIR: Path | None = None
#: Bumped whenever an entry's meaning changes.
CACHE_FORMAT = 1


def _cache_key(artist: str, size: int, opaque: bool) -> str:
    """Hash of the artist's bytes and everything else an `ArtistSide` depends on: the size,
    the page, this module's constants, the renderer and the libraries that do the
    arithmetic."""
    import importlib.metadata as md

    import skimage
    try:
        resvg = md.version("resvg_py")
    except md.PackageNotFoundError:
        resvg = "?"
    h = hashlib.sha256(artist.encode("utf-8"))
    h.update(f"|{size}|{int(opaque)}|{RES}|{MAX_COLOURS}|{ALPHA_WEIGHT}|{CACHE_FORMAT}"
             f"|resvg {resvg}|numpy {np.__version__}|skimage {skimage.__version__}".encode())
    return h.hexdigest()[:32]


def _compute_artist_side(artist: str, size: int, opaque: bool) -> ArtistSide:
    from scipy import ndimage
    ca = _codes(_flat(artist, size), opaque)
    uniq, counts = np.unique(ca.reshape(-1), return_counts=True)
    pal = _palette(_features(uniq), counts)
    la = _labels(ca, pal)
    edges = np.zeros(la.shape, dtype=bool)
    edges[:, 1:] |= la[:, 1:] != la[:, :-1]
    edges[1:, :] |= la[1:, :] != la[:-1, :]
    # Within one source pixel of an artist edge (city-block distance): RES steps.
    near = ndimage.binary_dilation(edges, iterations=RES)
    return ArtistSide(la.astype(np.uint8), pal, boundary_length(la), near)


def artist_side(artist: str, size: int, opaque: bool) -> ArtistSide:
    """The artist's side of `geomatch` at `size` px, from `CACHE_DIR` when it holds it.

    An entry stores the labels and the near-edge mask losslessly (the labels fit a byte:
    `MAX_COLOURS` < 256) and the palette as float64, so a hit gives the same numbers as
    computing it. The write is atomic: pool workers may miss on the same file at once."""
    path = CACHE_DIR / f"{_cache_key(artist, size, opaque)}.npz" if CACHE_DIR else None
    if path is not None and path.exists():
        try:
            with np.load(path) as z:
                near = np.unpackbits(z["near"], count=size * size).reshape(size, size)
                return ArtistSide(z["labels"], z["palette"], float(z["length"]), near.astype(bool))
        except Exception:  # noqa: BLE001 - a damaged entry is recomputed and replaced
            path.unlink(missing_ok=True)
    side = _compute_artist_side(artist, size, opaque)
    if path is not None:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_name(f"{path.stem}.{os.getpid()}.tmp.npz")
        np.savez_compressed(tmp, labels=side.labels, palette=side.palette,
                            length=np.float64(side.length), near=np.packbits(side.near))
        try:
            os.replace(tmp, path)
        except OSError:
            tmp.unlink(missing_ok=True)
    return side


def geomatch(ours: str, artist: str, src_px: int, opaque: bool = False) -> dict:
    """`geom` and `geom_far` (module documentation) of the trace `ours` against `artist`,
    both SVG text, for an input raster `src_px` pixels across."""
    size = RES * src_px
    art = artist_side(artist, size, opaque)
    if art.length <= 0:
        return {"geom": 0.0, "geom_far": 0.0}
    lb = _labels(_codes(_flat(ours, size), opaque), art.palette)
    scale = src_px / size                       # source px per render px
    miss = art.labels != lb
    area = float(np.count_nonzero(miss))
    far = float(np.count_nonzero(miss & ~art.near))
    # Area in source px² over length in source px: source px.
    return {"geom": area * scale * scale / (art.length * scale),
            "geom_far": far * scale * scale / (art.length * scale)}


def _self_check() -> None:
    """A disc against itself reads 0; against the disc `δ` larger, `δ` (to the estimate's
    few per cent)."""
    disc = ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">'
            '<circle cx="12" cy="12" r="{r}" fill="#d22f27"/>'
            '<rect x="3" y="3" width="6" height="4" fill="#123456"/></svg>')
    base = disc.format(r=7.0)
    assert geomatch(base, base, 512)["geom"] == 0.0
    for delta in (0.05, 0.25, 1.0):
        r = 7.0 + delta * 24 / 512           # delta source px at 512 px
        g = geomatch(disc.format(r=r), base, 512)["geom"]
        # The disc's share of the boundary, against the rectangle's unmoved edges.
        disc_len, rect_len = 2 * np.pi * 7.0, 20.0
        want = delta * disc_len / (disc_len + rect_len)
        assert abs(g - want) < 0.06 * want + 0.004, (delta, g, want)


if __name__ == "__main__":
    _self_check()
    print("geomatch self-check passed")
