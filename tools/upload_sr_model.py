#!/usr/bin/env python3
"""Upload the SR model package to Hugging Face.

Repository: https://huggingface.co/Logolabs/inkvec-sr-001
Files:
  - logo_sr_x4.pt    (~20 MB, fp16 weights + arch kwargs)
  - mambairv2_arch.py (standalone MambaIRv2 architecture)
  - requirements.txt  (Python dependencies)
  - README.md          (model card)
  - assets/            (masthead, colophon, brand lockup, acknowledgement banner)

Requires: pip install huggingface_hub
Auth:     huggingface-cli login
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

HF_REPO_ID = "Logolabs/inkvec-sr-001"

ROOT_DIR = Path(__file__).resolve().parents[1]
PACKAGE_DIR = ROOT_DIR / "out" / "sr" / "logo_sr_package"
MODEL_CARD_DIR = ROOT_DIR / "out" / "hf_current_sr"

# Files to upload: (local_path, hf_path)
CARD_ASSETS = ["masthead.png", "colophon.png",
               "logolabs-brand-lockup.png", "acknowledgment-banner.png"]
FILES = [
    (PACKAGE_DIR / "weights" / "logo_sr_x4.pt", "logo_sr_x4.pt"),
    (PACKAGE_DIR / "mambairv2_arch.py",          "mambairv2_arch.py"),
    (PACKAGE_DIR / "requirements.txt",           "requirements.txt"),
    (MODEL_CARD_DIR / "README.md",               "README.md"),
    *[(MODEL_CARD_DIR / "assets" / name, f"assets/{name}") for name in CARD_ASSETS],
]


def main():
    parser = argparse.ArgumentParser(
        description="Upload the inkvec SR model package to Hugging Face")
    parser.add_argument("--dry-run", action="store_true",
                        help="List what would be uploaded without uploading")
    parser.add_argument("--repo-id", default=HF_REPO_ID,
                        help=f"HF repo ID (default: {HF_REPO_ID})")
    parser.add_argument("--commit-message", default="Upload inkvec-sr-001 model package",
                        help="Commit message for the upload")
    args = parser.parse_args()

    # Validate all files exist
    missing = [(local, hf) for local, hf in FILES if not local.exists()]
    if missing:
        print("ERROR: The following files are missing:", file=sys.stderr)
        for local, hf in missing:
            print(f"  {local} -> {hf}", file=sys.stderr)
        sys.exit(1)

    # Show what will be uploaded
    print(f"Repository: {args.repo_id}")
    print(f"Files to upload:")
    for local, hf in FILES:
        size_mb = local.stat().st_size / (1024 * 1024)
        print(f"  {local.name:25s} -> {hf:25s} ({size_mb:.1f} MB)")

    if args.dry_run:
        print("\n[DRY RUN] No files uploaded.")
        return

    try:
        from huggingface_hub import HfApi
    except ImportError:
        print("ERROR: huggingface_hub is not installed. Run: pip install huggingface_hub",
              file=sys.stderr)
        sys.exit(1)

    api = HfApi()

    # Create repo if it doesn't exist
    print(f"\nCreating repo {args.repo_id} (if needed)...")
    api.create_repo(repo_id=args.repo_id, repo_type="model", exist_ok=True)

    # Upload all files
    for local, hf in FILES:
        print(f"  Uploading {local.name} -> {hf}...")
        api.upload_file(
            path_or_fileobj=str(local),
            path_in_repo=hf,
            repo_id=args.repo_id,
            commit_message=f"Add {hf}",
        )

    print(f"\nDone! View at: https://huggingface.co/{args.repo_id}")


if __name__ == "__main__":
    main()
