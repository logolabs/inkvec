"""Check the Space's presentation page in headless Edge: the page, its gallery and results,
and the ways into Inkvec Studio Lite (the button, a sample, a dropped file), with threads.

    python studio/web/landing_check.py [--url http://127.0.0.1:8931/] [--out DIR]

Serve the built site first (`python studio/web/serve.py`). Page screenshots only.
"""

from __future__ import annotations

import argparse
import pathlib
import sys

from playwright.sync_api import sync_playwright

HERE = pathlib.Path(__file__).resolve().parent


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://127.0.0.1:8931/")
    ap.add_argument("--out", default=str(HERE.parent / ".shots-web"))
    a = ap.parse_args()
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)

    def note(msg: str) -> None:
        sys.stdout.buffer.write((msg + "\n").encode("utf-8"))
        sys.stdout.flush()

    with sync_playwright() as p:
        br = p.chromium.launch(channel="msedge", headless=True)
        page = br.new_context(viewport={"width": 1440, "height": 900}, bypass_csp=True).new_page()
        errors: list[str] = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.on("console", lambda m: errors.append(m.text) if m.type == "error" else None)
        requests: list[str] = []
        page.on("request", lambda r: requests.append(r.url))

        page.goto(a.url)
        page.wait_for_selector("#cases .case", timeout=30_000)
        page.wait_for_timeout(800)
        page.screenshot(path=str(out / "L1-landing.png"))
        note(f"landing: isolated={page.evaluate('crossOriginIsolated')}, {page.locator('#cases .case').count()} gallery cases, "
             f"{page.locator('#bars tbody tr').count()} table rows, first row {page.inner_text('#bars tbody tr >> nth=0')!r}")
        page.locator("#new").scroll_into_view_if_needed()
        page.screenshot(path=str(out / "L2-whats-new.png"))
        page.locator("#showcase").scroll_into_view_if_needed()
        page.click('#galtools button:has-text("Handles")')
        page.click('#cases .case >> nth=1')
        page.click('#galtools button:has-text("inkvec · VTracer 1.0, best flags")')
        page.click('#galtools button:has-text("4×")')
        page.wait_for_timeout(500)
        page.locator("#showcase").scroll_into_view_if_needed()
        page.screenshot(path=str(out / "L3-gallery.png"))
        note(f"gallery chips: {page.inner_text('#galchips')!r}")
        page.locator("#versus").scroll_into_view_if_needed()
        page.wait_for_timeout(300)
        page.screenshot(path=str(out / "L4-results.png"))
        other = sorted({u.split('/')[2] for u in requests if u.startswith("http") and "127.0.0.1" not in u})
        note(f"third-party requests from the landing page: {other or 'none'}")

        # The button: into the Studio, with threads.
        page.click("#open")
        page.wait_for_url("**/studio/index.html", timeout=30_000)
        page.wait_for_selector("#boot", state="detached", timeout=60_000)
        info = page.evaluate("window.__inkvecStudioLite?.info")
        note(f"Open Inkvec Studio Lite -> {page.url} engine={info}")
        page.screenshot(path=str(out / "L5-studio-from-landing.png"))

        # A sample link: the Studio opens it at once.
        page.goto(a.url)
        page.wait_for_selector("#samples a", timeout=30_000)
        page.click("#samples a >> nth=1")
        page.wait_for_selector(".palettecard .ink", timeout=120_000)
        note(f"sample link -> {page.url}; opened {page.inner_text('.filechip .name')!r}")

        # A dropped (here: chosen) file: handed over through IndexedDB.
        page.goto(a.url)
        page.wait_for_selector("#drop", timeout=30_000)
        page.set_input_files("#file", str(HERE.parent / "src-tauri" / "samples" / "icon-64.png"))
        page.wait_for_url("**/studio/**", timeout=30_000)
        page.wait_for_selector(".palettecard .ink", timeout=120_000)
        page.wait_for_timeout(600)
        note(f"file handed over -> {page.url}; opened {page.inner_text('.filechip .name')!r}")
        page.screenshot(path=str(out / "L6-handoff.png"))

        # The Showcase screen in the Studio.
        page.click('.appbar button:has-text("Showcase")')
        page.wait_for_selector(".sc-case", timeout=30_000)
        page.wait_for_timeout(600)
        page.screenshot(path=str(out / "L7-studio-showcase.png"))
        note(f"Studio Lite showcase: {page.locator('.sc-case').count()} cases")

        note(f"errors: {[e for e in errors if 'favicon' not in e][:6]}")
        try:
            br.close()
        except Exception:  # noqa: BLE001 - the old driver can fail to close a page with workers
            pass


if __name__ == "__main__":
    main()
