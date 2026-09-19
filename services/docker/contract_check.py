"""Run the shared contract (bindings/contract/cases.json) over HTTP against a running
inkvec-server -- the same cases every binding is tested against, exercised as
`.github/workflows/docker.yml` exercises them against the freshly built container.

    python services/docker/contract_check.py --base-url http://localhost:8080

Needs nothing the container doesn't already expose: `/healthz`, `/version` and `POST /trace`
with `?options=` (see crates/inkvec-server/README.md). Standard library only, so it runs the
same way in the workflow's job container and on a developer's machine. It also works against a
plain `cargo run -p inkvec-server` process -- no Docker required to exercise this script
itself, only to exercise the image it is meant for.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = ROOT / "bindings" / "contract"

ERROR_STATUS = {"invalid_image": 400, "invalid_options": 422, "internal": 500}


def request(base_url: str, path: str, method: str = "GET", body: bytes | None = None, headers: dict | None = None):
    """(status, body, headers): headers as a lowercase-keyed dict -- HTTP header names are
    case-insensitive and axum sends them lowercase, so callers look them up lowercase too."""
    url = base_url.rstrip("/") + path
    req = urllib.request.Request(url, data=body, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return resp.status, resp.read(), {k.lower(): v for k, v in resp.headers.items()}
    except urllib.error.HTTPError as e:
        return e.code, e.read(), {k.lower(): v for k, v in (e.headers or {}).items()}


def check_case(base_url: str, target: str, case: dict) -> str | None:
    """None on success, else a failure message."""
    name = case["name"]
    image = (CONTRACT / case["input"]).read_bytes()
    query = urllib.parse.urlencode({"options": json.dumps(case["options"])})
    status, body, headers = request(
        base_url,
        f"/trace?{query}",
        method="POST",
        body=image,
        headers={"Content-Type": "application/octet-stream"},
    )

    expect = case["expect"]
    if "error" in expect:
        want_status = ERROR_STATUS[expect["error"]]
        if status != want_status:
            return f"{name}: expected {want_status} ({expect['error']}), got {status}: {body[:200]!r}"
        parsed = json.loads(body)
        code = parsed.get("error", {}).get("code")
        if code != expect["error"]:
            return f"{name}: expected error code {expect['error']}, got {code}"
        return None

    if status != 200:
        return f"{name}: expected 200, got {status}: {body[:200]!r}"
    if headers.get("x-inkvec-width") != str(expect["width"]) or headers.get("x-inkvec-height") != str(
        expect["height"]
    ):
        got = f"{headers.get('x-inkvec-width')}x{headers.get('x-inkvec-height')}"
        return f"{name}: size headers {got}, want {expect['width']}x{expect['height']}"

    svg_expect = case.get("svg", {}).get(target)
    if svg_expect:
        digest = hashlib.sha256(body).hexdigest()
        if len(body) != svg_expect["bytes"] or digest != svg_expect["sha256"]:
            return (
                f"{name}: SVG mismatch on {target}: {len(body)}b/{digest} vs "
                f"{svg_expect['bytes']}b/{svg_expect['sha256']}"
            )
    return None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base-url", default="http://localhost:8080")
    ap.add_argument("--target", default=None, help="build_target to check SVG hashes against (default: ask /version)")
    args = ap.parse_args()

    status, _body, _headers = request(args.base_url, "/healthz")
    if status != 200:
        print(f"/healthz: expected 200, got {status}", file=sys.stderr)
        return 1

    status, body, _headers = request(args.base_url, "/version")
    if status != 200:
        print(f"/version: expected 200, got {status}", file=sys.stderr)
        return 1
    info = json.loads(body)
    target = args.target or info["build_target"]
    print(f"inkvec-server {info['version']} ({target}) at {args.base_url}")

    cases = json.loads((CONTRACT / "cases.json").read_text("utf-8"))
    checked = 0
    failures: list[str] = []
    for case in cases["cases"]:
        if case["form"] != "encoded":
            continue  # no raw-pixel endpoint on this service; see crates/inkvec-server/tests/contract.rs
        msg = check_case(args.base_url, target, case)
        if msg:
            failures.append(msg)
        else:
            checked += 1

    for msg in failures:
        print(msg, file=sys.stderr)
    print(f"checked {checked} case(s) against {args.base_url}, {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
