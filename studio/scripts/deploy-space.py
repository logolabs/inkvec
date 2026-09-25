#!/usr/bin/env python3
"""Upload the Space (studio/dist-web: the presentation page, Inkvec Studio Lite under studio/).

    python studio/scripts/deploy-space.py                      # dry run: what would go up
    python studio/scripts/deploy-space.py --yes                # upload to Logolabs/inkvec
    python studio/scripts/deploy-space.py --space you/test --yes

Build first (`npm run build:web` in studio/). The token is read from `HF_TOKEN`, or from a
prior `huggingface-cli login`; it needs write access to the Space. Nothing is uploaded
without `--yes`.

`HfApi.upload_folder` rather than `hf upload`: the CLI's upload path has answered 402 for
this Space before, the API call has not. Files the site no longer has (the old demo's
`index.html` companions, its engine package) are removed in the same commit unless
`--keep-stale` is given, so the Space is exactly the build.

Needs `pip install huggingface_hub`.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

STUDIO = Path(__file__).resolve().parents[1]
SITE = STUDIO / "dist-web"

# Everything the page needs on its first load. A build missing one of these is broken.
REQUIRED = [
    # The presentation page, at the root.
    "index.html",
    "showcase.json",
    "README.md",
    # Inkvec Studio Lite, under studio/.
    "studio/index.html",
    "studio/denoise.js",
    "studio/pkg/inkvec_studio_wasm.js",
    "studio/pkg/inkvec_studio_wasm_bg.wasm",
    "studio/pkg-threads/inkvec_studio_wasm.js",
    "studio/pkg-threads/inkvec_studio_wasm_bg.wasm",
    "studio/samples/flat-logo.png",
    "studio/guide/index.html",
]


def check(site: Path) -> list[str]:
    problems = [f"missing {r}" for r in REQUIRED if not (site / r).is_file()]
    card = (site / "README.md").read_text(encoding="utf-8") if (site / "README.md").is_file() else ""
    # Without these two the Space serves a page that cannot have threads or the denoiser.
    for header in ("cross-origin-embedder-policy: require-corp", "cross-origin-opener-policy: same-origin"):
        if header not in card:
            problems.append(f"README.md front matter lacks `{header}`")
    if not re.search(r"^sdk:\s*static\s*$", card, re.M):
        problems.append("README.md front matter is not `sdk: static`")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--space", default="Logolabs/inkvec", help="owner/name of the Space")
    ap.add_argument("--dir", default=str(SITE), help="the built site")
    ap.add_argument("--yes", action="store_true", help="actually upload")
    ap.add_argument("--keep-stale", action="store_true", help="leave remote files the build does not have")
    ap.add_argument("--message", default="Inkvec 0.2: the presentation page, and Inkvec Studio Lite under studio/")
    a = ap.parse_args()

    site = Path(a.dir)
    problems = check(site)
    if problems:
        print("The build is not ready to deploy:", *problems, sep="\n  ", file=sys.stderr)
        return 1

    files = sorted(p for p in site.rglob("*") if p.is_file())
    total = sum(p.stat().st_size for p in files)
    print(f"{len(files)} files, {total / 1024 / 1024:.2f} MB, from {site}")
    for p in files:
        print(f"  {p.relative_to(site).as_posix():<60} {p.stat().st_size:>10,}")
    if not a.yes:
        print(f"\nDry run. Add --yes to upload to https://huggingface.co/spaces/{a.space}")
        return 0

    try:
        from huggingface_hub import HfApi
    except ImportError:
        print("pip install huggingface_hub", file=sys.stderr)
        return 1
    api = HfApi(token=os.environ.get("HF_TOKEN"))
    who = api.whoami()
    print(f"\nUploading as {who.get('name')} to {a.space}")
    info = api.upload_folder(
        repo_id=a.space,
        repo_type="space",
        folder_path=str(site),
        commit_message=a.message,
        # Everything remote is replaced by the build; huggingface_hub never deletes the
        # Space's .gitattributes, whatever the pattern says.
        delete_patterns=None if a.keep_stale else ["*"],
    )
    print(f"Done: {info}")
    print(f"Open https://huggingface.co/spaces/{a.space} (and its direct URL for full screen).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
