#!/usr/bin/env python3
"""Upload `web/` to the Inkvec Hugging Face Space.

`web/` *is* the Space: the page, the worker, the denoiser module, the samples and both
built WebAssembly packages. ONNX Runtime and the denoiser weights are deliberately not in
it -- the page fetches those at run time, from jsDelivr and from
`Logolabs/inkvec-denoiser-001`.

Build the packages first, or the Space gets whatever `web/pkg*` happens to hold:

    tools/build_wasm.sh
    HF_TOKEN=hf_... tools/deploy_space.py

The token needs write access to the Space and nothing else; a fine-grained token scoped to
`spaces/Logolabs/inkvec` is enough. It is read from the environment and never written
anywhere.
"""
from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

SPACE = "Logolabs/inkvec"
ROOT = Path(__file__).resolve().parents[1]
WEB = ROOT / "web"

# Everything the page needs at run time, and nothing else. `delete_patterns` clears files a
# previous upload left behind under these prefixes, so a renamed or dropped file does not
# linger on the Space.
ALLOW = ["*.html", "*.js", "*.md", "*.svg", "*.json", "samples/*", "pkg/**", "pkg-threads/**"]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--space", default=SPACE, help=f"target Space (default {SPACE})")
    ap.add_argument("--message", default="web: the denoiser in the browser, on WebGPU through ONNX Runtime Web")
    ap.add_argument("--dry-run", action="store_true", help="list what would be uploaded and stop")
    args = ap.parse_args()

    missing = [p for p in ("index.html", "worker.js", "denoise.js", "README.md",
                           "pkg/inkvec_wasm_bg.wasm", "pkg-threads/inkvec_wasm_bg.wasm")
               if not (WEB / p).is_file()]
    if missing:
        print(f"error: web/ is missing {', '.join(missing)}; run tools/build_wasm.sh first", file=sys.stderr)
        return 1

    files = sorted(p for p in WEB.rglob("*") if p.is_file())
    total = sum(p.stat().st_size for p in files)
    print(f"{len(files)} files, {total / 1048576:.1f} MB from {WEB}")
    packaged = 0
    for f in files:
        rel = f.relative_to(WEB)
        if rel.parts[0] in ("pkg", "pkg-threads"):
            packaged += 1
        else:
            print(f"  {rel}")
    print(f"  pkg/ and pkg-threads/: {packaged} files")

    if args.dry_run:
        print("\n--dry-run: nothing uploaded")
        return 0

    token = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN")
    if not token:
        print("error: set HF_TOKEN to a token with write access to the Space", file=sys.stderr)
        return 1

    from huggingface_hub import HfApi

    api = HfApi(token=token)
    who = api.whoami()
    print(f"\nuploading to {args.space} as {who.get('name')}")
    url = api.upload_folder(
        repo_id=args.space,
        repo_type="space",
        folder_path=str(WEB),
        allow_patterns=ALLOW,
        delete_patterns=ALLOW,
        commit_message=args.message,
    )
    print(f"done: {url}")
    print(f"the Space rebuilds itself; watch https://huggingface.co/spaces/{args.space}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
