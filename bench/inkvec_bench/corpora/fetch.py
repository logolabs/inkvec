"""Fetch real ground-truth SVG corpora.

Everything measured so far is a synthetic corpus of hand-made shapes, which is good for
isolating a capability and useless for claiming general performance: the shapes were
chosen by the same person who chose the algorithms. These are the sets the closest
published work (AnchorFlow) reports on, so results here are comparable to something.

Sampling is deterministic given a seed, so a corpus can be rebuilt exactly. Each corpus
records its source and licence in a manifest next to the files.
"""
from __future__ import annotations

import json
import random
import re
from dataclasses import dataclass
from pathlib import Path

import requests

TIMEOUT = 30


@dataclass(frozen=True)
class Source:
    name: str
    repo: str
    ref: str
    path: str
    licence: str
    #: Keep only paths matching this, after the directory listing.
    keep: str = r"\.svg$"


SOURCES: dict[str, Source] = {
    "noto-emoji": Source(
        "noto-emoji", "googlefonts/noto-emoji", "main", "svg",
        "Apache-2.0 (code) / CC-BY-4.0 (emoji)",
    ),
    "twemoji": Source(
        "twemoji", "jdecked/twemoji", "main", "assets/svg",
        "MIT (code) / CC-BY-4.0 (graphics)",
    ),
    "material-icons": Source(
        "material-icons", "marella/material-design-icons", "main", "svg/outlined",
        "Apache-2.0",
    ),
    "fluent-emoji": Source(
        "fluent-emoji", "microsoft/fluentui-emoji", "main", "assets",
        "MIT", keep=r"Color/.*\.svg$",
    ),
}


def _list_tree(src: Source) -> list[str]:
    """Every file path in the repo under `src.path`, via one recursive tree request."""
    url = f"https://api.github.com/repos/{src.repo}/git/trees/{src.ref}?recursive=1"
    r = requests.get(url, timeout=TIMEOUT, headers={"Accept": "application/vnd.github+json"})
    r.raise_for_status()
    tree = r.json().get("tree", [])
    pat = re.compile(src.keep)
    prefix = src.path.rstrip("/") + "/"
    return sorted(
        e["path"]
        for e in tree
        if e.get("type") == "blob" and e["path"].startswith(prefix) and pat.search(e["path"])
    )


def fetch(name: str, out_root: Path, limit: int = 120, seed: int = 0) -> tuple[int, str]:
    """Download a deterministic sample of one corpus. Returns (count, note)."""
    src = SOURCES[name]
    dest = Path(out_root) / name
    dest.mkdir(parents=True, exist_ok=True)

    try:
        paths = _list_tree(src)
    except Exception as e:  # network, rate limit, renamed repo
        return 0, f"listing failed: {type(e).__name__}: {e}"
    if not paths:
        return 0, "no SVGs found at that path (repo layout may have changed)"

    rng = random.Random(seed)
    sample = paths if len(paths) <= limit else rng.sample(paths, limit)
    sample.sort()

    written = 0
    failures = 0
    for p in sample:
        # Flatten the path so the filename stays unique and stable.
        stem = re.sub(r"[^A-Za-z0-9._-]+", "_", p[len(src.path) :].strip("/"))
        target = dest / stem
        if target.exists():
            written += 1
            continue
        raw = f"https://raw.githubusercontent.com/{src.repo}/{src.ref}/{p}"
        try:
            r = requests.get(raw, timeout=TIMEOUT)
            r.raise_for_status()
            target.write_bytes(r.content)
            written += 1
        except Exception:
            failures += 1

    (dest / "manifest.json").write_text(
        json.dumps(
            {
                "name": src.name,
                "repo": src.repo,
                "ref": src.ref,
                "path": src.path,
                "licence": src.licence,
                "seed": seed,
                "limit": limit,
                "available": len(paths),
                "written": written,
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    note = f"{written} files" + (f", {failures} failed" if failures else "")
    return written, note


def fetch_all(out_root: Path, names: list[str] | None = None, limit: int = 120, seed: int = 0):
    results = {}
    for name in names or list(SOURCES):
        n, note = fetch(name, out_root, limit, seed)
        results[name] = (n, note)
        print(f"  {name:16s} {note}")
    return results
