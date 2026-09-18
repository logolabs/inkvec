#!/usr/bin/env python3
"""Auto-pull the restorer/denoiser ONNX model weights from Hugging Face.

Repository: https://huggingface.co/Logolabs/inkvec-denoiser-001
Weights: restorer.onnx (79.9 MB)
SHA256: bdc2762157632f6f74dd91474f0598e591d49ded47e0a87642c89416b0809d6d
"""
from __future__ import annotations

import argparse
import hashlib
import os
import sys
from pathlib import Path

HF_REPO_ID = "Logolabs/inkvec-denoiser-001"
HF_FILENAME = "restorer.onnx"
HF_URL = f"https://huggingface.co/{HF_REPO_ID}/resolve/main/{HF_FILENAME}"
EXPECTED_SHA256 = "bdc2762157632f6f74dd91474f0598e591d49ded47e0a87642c89416b0809d6d"

ROOT_DIR = Path(__file__).resolve().parents[1]
DEFAULT_DEST = ROOT_DIR / "crates" / "inkvec-restore" / "models" / HF_FILENAME


def compute_sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def pull_model(dest: Path, force: bool = False, verify: bool = True) -> Path:
    if dest.is_file() and not force:
        if verify:
            actual = compute_sha256(dest)
            if actual == EXPECTED_SHA256:
                print(f"Model already present at {dest} and verified (SHA256: {actual[:16]}...)")
                return dest
            else:
                print(f"Checksum mismatch for {dest} ({actual} != {EXPECTED_SHA256}); re-downloading...")
        else:
            print(f"Model already present at {dest}")
            return dest

    dest.parent.mkdir(parents=True, exist_ok=True)
    temp_dest = dest.with_suffix(".tmp")

    print(f"Downloading {HF_FILENAME} from Hugging Face ({HF_REPO_ID})...")

    # Try huggingface_hub first
    downloaded = False
    try:
        from huggingface_hub import hf_hub_download
        hf_path = Path(hf_hub_download(repo_id=HF_REPO_ID, filename=HF_FILENAME))
        import shutil
        shutil.copy2(hf_path, dest)
        downloaded = True
        print(f"Pulled via huggingface_hub: {dest}")
    except Exception as e:
        print(f"huggingface_hub unavailable ({e}), falling back to direct download from {HF_URL}...")

    if not downloaded:
        import urllib.request
        urllib.request.urlretrieve(HF_URL, temp_dest)
        temp_dest.replace(dest)
        print(f"Downloaded to {dest}")

    if verify:
        actual = compute_sha256(dest)
        if actual != EXPECTED_SHA256:
            raise ValueError(f"Downloaded model SHA256 checksum mismatch: got {actual}, expected {EXPECTED_SHA256}")
        print(f"SHA256 verified: {actual}")

    return dest


def main():
    parser = argparse.ArgumentParser(description="Auto-pull restorer model weights from Hugging Face")
    parser.add_argument(
        "--dest",
        type=Path,
        default=DEFAULT_DEST,
        help=f"Destination path (default: {DEFAULT_DEST})",
    )
    parser.add_argument("--force", action="store_true", help="Force re-download even if file exists")
    parser.add_argument("--no-verify", action="store_true", help="Skip SHA256 verification")
    args = parser.parse_args()

    pull_model(dest=args.dest, force=args.force, verify=not args.no_verify)


if __name__ == "__main__":
    main()
