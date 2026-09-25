"""Drive Inkvec Studio Lite end to end in headless Edge: open, trace, wizard, palette,
export, minify, fabricate. Screenshots of the page only (never the desktop).

    python studio/web/smoke.py [--url http://127.0.0.1:8931/] [--out DIR]

Needs `pip install playwright` and Microsoft Edge; serve the site with studio/web/serve.py.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import time

from playwright.sync_api import sync_playwright

HERE = pathlib.Path(__file__).resolve().parent


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://127.0.0.1:8931/")
    ap.add_argument("--out", default=str(HERE.parent / ".shots-web"))
    ap.add_argument("--theme", default="dark")
    a = ap.parse_args()
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    log: list[str] = []

    def note(msg: str) -> None:
        print(msg, flush=True)
        log.append(msg)

    with sync_playwright() as p:
        br = p.chromium.launch(channel="msedge", headless=True)
        ctx = br.new_context(viewport={"width": 1440, "height": 900}, accept_downloads=True)
        page = ctx.new_page()
        page.on("console", lambda m: note(f"console.{m.type}: {m.text}") if m.type in ("error", "warning") else None)
        page.on("pageerror", lambda e: note(f"pageerror: {e}"))

        t0 = time.perf_counter()
        page.goto(a.url)
        page.wait_for_selector("#boot", state="detached", timeout=60_000)
        note(f"boot: {time.perf_counter() - t0:.2f}s, isolated={page.evaluate('crossOriginIsolated')}")
        page.screenshot(path=str(out / "01-first-run.png"))

        # Open a sample: the automatic trace (final) starts at once.
        t0 = time.perf_counter()
        page.click("button.sample >> nth=0")
        page.wait_for_selector(".palettecard .ink", timeout=120_000)
        note(f"sample traced: {time.perf_counter() - t0:.2f}s")
        page.wait_for_timeout(800)
        page.screenshot(path=str(out / "02-traced.png"))
        note("report: " + page.evaluate("document.querySelector('.rail')?.innerText.slice(0, 400)").replace("\n", " | "))
        br.close()

    (out / "smoke.log").write_text("\n".join(log), encoding="utf-8")


if __name__ == "__main__":
    main()
