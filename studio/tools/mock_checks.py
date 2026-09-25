#!/usr/bin/env python3
"""Regression checks for the Studio's interface, driven in the browser dev mock.

The Rust half of the Studio has `cargo test`; the TypeScript half has no test runner, and
some of its bugs live only in how the interface sequences calls (a fresh trace landing on
top of a snap, several snaps racing each other). These checks drive the real interface in
headless Microsoft Edge against `dev/index.html`, the mock backend `npm run mock` serves,
and read what it would have sent to the backend. The desktop app itself is never launched:
it reads and writes the user's real preferences file.

    cd studio && npx vite --port 1437 --strictPort      # in one terminal
    python studio/tools/mock_checks.py                  # exit 1 if any check fails

Needs `pip install playwright` and Edge.
"""
from __future__ import annotations

import argparse
import json
import sys
from typing import Callable

from playwright.sync_api import Page, sync_playwright

W, H = 1440, 900

# Installed before the app starts: records every command the interface sends, and answers
# the few the mock leaves unanswered in the way these checks need (a folder for Export, a
# palette match that maps each traced ink to its own brand colour).
INIT = r"""
(() => {
  window.__calls = [];
  const extra = {
    'plugin:dialog|open': (a) => (a.options && a.options.directory ? 'C:\\mock\\out' : undefined),
    match_palette: (a) => a.traced.map((from, i) => ({ from, to: window.__brand[i % window.__brand.length], de00: 1.5 })),
  };
  window.__brand = ['#102030', '#405060', '#708090', '#a0b0c0', '#c0d0e0', '#e0f0a0'];
  const wrap = (orig) => async (cmd, args, opts) => {
    window.__calls.push({ cmd, args: JSON.parse(JSON.stringify(args ?? null)) });
    if (extra[cmd]) {
      const r = await extra[cmd](args || {});
      if (r !== undefined) return r;
    }
    return orig(cmd, args, opts);
  };
  const internals = window.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__ || {};
  let current;
  Object.defineProperty(internals, 'invoke', {
    configurable: true,
    get() { return current; },
    set(fn) { current = wrap(fn); },
  });
})();
"""

# The store the page is running (dev/boot.ts puts it on the window).
STATE = "() => window.__store.state"


def fresh(browser, base: str) -> Page:
    pg = browser.new_page(viewport={"width": W, "height": H})
    pg.add_init_script(INIT)
    pg.goto(f"{base}/dev/index.html")
    pg.wait_for_selector(".appbar")
    pg.wait_for_function("() => window.__store")
    pg.wait_for_timeout(500)
    return pg


def open_sample(pg: Page, label: str) -> None:
    pg.click(f"button.sample:has-text('{label}')")
    pg.wait_for_selector(".chip.final", timeout=15000)
    pg.click("[data-ctl=chooser-auto]")
    pg.wait_for_timeout(400)


def flat_inks(pg: Page) -> list[dict]:
    return [i for i in pg.evaluate(STATE)["palette"] if i["kind"] != "gradient"]


def calls(pg: Page, cmd: str) -> list[dict]:
    return [c["args"] for c in pg.evaluate("() => window.__calls") if c["cmd"] == cmd]


def export_now(pg: Page) -> dict:
    """Open the export sheet, press its button, and return the request `write_export` got."""
    pg.click(".railfoot button.btn.primary:has-text('Export')")
    pg.wait_for_selector(".sheet button.btn.primary:not([disabled])", timeout=10000)
    pg.click(".sheet button.btn.primary")
    pg.wait_for_function("() => window.__calls.some(c => c.cmd === 'write_export')", timeout=20000)
    return calls(pg, "write_export")[-1]["request"]


def check_a_snap_reaches_the_export(pg: Page) -> list[str]:
    """Export re-traces; the snap the user made must survive it, in every format."""
    open_sample(pg, "Flat logo")
    ink = flat_inks(pg)[0]
    pg.click(f"[data-ctl='ink:{ink['hex']}']")
    pg.fill(".popover input.numberfield", "#123456")
    pg.click(".popover button.btn:has-text('Snap')")
    pg.wait_for_function(
        "() => (window.__store.state.svg || '').includes('#123456')", timeout=5000
    )
    request = export_now(pg)
    after = pg.evaluate(STATE)
    problems = []
    if "#123456" not in request["svg"]:
        problems.append("the exported SVG lost the snap")
    if ink["traced"] in request["svg"]:
        problems.append(f"the exported SVG still paints {ink['traced']}")
    if not any(i["hex"] == "#123456" for i in request["palette"]):
        problems.append("palette.json lost the snap")
    if "#123456" not in (after["svg"] or ""):
        problems.append("the drawing on screen lost the snap after the export's fresh trace")
    return problems


def check_a_pasted_palette_snaps_every_ink(pg: Page) -> list[str]:
    """"Snap N inks" snaps all N, not whichever answer came back last."""
    open_sample(pg, "Flat logo")
    flats = flat_inks(pg)
    pg.click("button.reset:has-text('Paste brand palette')")
    pg.fill(".modal textarea", "#102030")
    pg.wait_for_timeout(300)
    pg.click(".modal button.btn.primary")
    wanted = [pg.evaluate("() => window.__brand")[i % 6] for i in range(len(flats))]
    try:
        pg.wait_for_function(
            "(w) => { const s = window.__store.state.svg || ''; return w.every(c => s.includes(c)); }",
            arg=wanted,
            timeout=5000,
        )
    except Exception:  # noqa: BLE001 - reported below
        pass
    svg = pg.evaluate(STATE)["svg"]
    missing = [c for c in wanted if c not in svg]
    return [f"{len(missing)} of {len(wanted)} pasted colours never reached the drawing"] if missing else []


CHECKS: list[tuple[str, Callable[[Page], list[str]]]] = [
    ("a snap reaches the export", check_a_snap_reaches_the_export),
    ("a pasted palette snaps every ink", check_a_pasted_palette_snaps_every_ink),
]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", default="http://127.0.0.1:1437", help="where vite serves the studio")
    ap.add_argument("-k", help="run only the checks whose name contains this")
    args = ap.parse_args()
    failed = 0
    with sync_playwright() as p:
        browser = p.chromium.launch(channel="msedge", headless=True)
        try:
            for name, check in CHECKS:
                if args.k and args.k not in name:
                    continue
                pg = fresh(browser, args.base.rstrip("/"))
                try:
                    problems = check(pg)
                except Exception as e:  # noqa: BLE001 - a crash is a failure, reported as one
                    problems = [f"crashed: {e}"]
                finally:
                    pg.close()
                failed += bool(problems)
                print(f"{'FAIL' if problems else 'ok  '}  {name}")
                for line in problems:
                    print(f"        {line}")
        finally:
            browser.close()
    print(json.dumps({"checks": len(CHECKS), "failed": failed}))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
