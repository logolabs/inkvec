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
        # One gallery, every set in a panel that scrolls inside itself beside the viewer.
        page.locator("#showcase").scroll_into_view_if_needed()
        page.wait_for_timeout(600)
        geo = page.evaluate("""(() => { const p = document.getElementById('cases'), v = document.getElementById('galview');
            return {panel: Math.round(p.getBoundingClientRect().height), viewer: Math.round(v.getBoundingClientRect().height),
                    scrolls: p.scrollHeight > p.clientHeight, sets: [...p.querySelectorAll('.sethead')].map(e => e.textContent)}; })()""")
        note(f"gallery panel: {geo}")
        page.screenshot(path=str(out / "L3a-gallery-top.png"))
        shots = [("brand__365retailmarkets_com", "L3b-brand-365retail"), ("brand__abrinor_fr", "L3c-brand-abrinor"),
                 ("brands__sangchaimeter_com", "L3d-scm"), ("noto-emoji__emoji_u1f478_1f3fd", "L3e-princess")]
        for key, name in shots:
            page.click(f'#cases .case[data-key="{key}"]')
            page.click('#galtools button:has-text("inkvec · VTracer 1.0, best flags")')
            page.wait_for_timeout(700)
            page.locator("#showcase").scroll_into_view_if_needed()
            page.screenshot(path=str(out / f"{name}.png"))
            note(f"gallery {name}: {page.inner_text('#galcap')!r} {page.inner_text('#galchips')!r}".replace("\n", " "))
        # Arrow keys walk the list, and the selected case stays in view inside the panel.
        page.focus('#cases .case[data-key="noto-emoji__emoji_u1f478_1f3fd"]')
        for _ in range(12):
            page.keyboard.press("ArrowDown")
        page.wait_for_timeout(800)
        sel = page.evaluate("""(() => { const p = document.getElementById('cases'), b = p.querySelector('.case[aria-pressed=true]');
            const pr = p.getBoundingClientRect(), br = b.getBoundingClientRect();
            return {key: b.dataset.key, inView: br.top >= pr.top && br.bottom <= pr.bottom, scrollTop: Math.round(p.scrollTop), pageY: Math.round(scrollY)}; })()""")
        note(f"after 12 x ArrowDown: {sel}")
        page.locator("#showcase").scroll_into_view_if_needed()
        page.screenshot(path=str(out / "L3f-gallery-scrolled.png"))
        # A narrow window: one horizontal strip.
        page.set_viewport_size({"width": 700, "height": 900})
        page.wait_for_timeout(400)
        page.locator("#cases").scroll_into_view_if_needed()
        strip = page.evaluate("(() => { const p = document.getElementById('cases'); return {h: Math.round(p.getBoundingClientRect().height), scrollsX: p.scrollWidth > p.clientWidth}; })()")
        note(f"narrow: {strip}")
        page.screenshot(path=str(out / "L3g-gallery-narrow.png"))
        page.set_viewport_size({"width": 1440, "height": 900})
        page.locator("#versus").scroll_into_view_if_needed()
        page.wait_for_timeout(300)
        page.screenshot(path=str(out / "L4-results.png"))
        other = sorted({u.split('/')[2] for u in requests if u.startswith("http") and "127.0.0.1" not in u})
        note(f"third-party requests from the landing page: {other or 'none'}")

        # The button: into the Studio, with threads, through the desktop's splash.
        page.click("#open")
        page.wait_for_url("**/studio/index.html", timeout=30_000)
        page.wait_for_timeout(900)
        page.screenshot(path=str(out / "L5a-studio-splash.png"))
        note(f"loading screen: splash frame {page.locator('#boot iframe').count()}, "
             f"edition {page.frame_locator('#boot iframe').locator('.edition').inner_text()!r}")
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
        page.wait_for_selector("#boot", state="detached", timeout=30_000)
        page.wait_for_timeout(600)
        note(f"file handed over -> {page.url}; opened {page.inner_text('.filechip .name')!r}")
        page.screenshot(path=str(out / "L6-handoff.png"))

        note(f"app bar: {page.inner_text('.appbar')!r}".replace("\n", " | "))

        # The Showcase screen in the Studio: the same one scrolling list.
        page.click('.appbar button:has-text("Showcase")')
        page.wait_for_selector(".sc-case", timeout=30_000)
        page.wait_for_timeout(900)
        page.screenshot(path=str(out / "L7-studio-showcase.png"))
        geo = page.evaluate("""(() => { const l = document.querySelector('.sc-list'), v = document.querySelector('.sc-viewer'), t = document.querySelector('.sc-tools');
            return {cases: l.querySelectorAll('.sc-case').length, list: Math.round(l.getBoundingClientRect().height), viewer: Math.round(v.getBoundingClientRect().height),
                    scrolls: l.scrollHeight > l.clientHeight, toolsOneLine: t.scrollHeight < 48}; })()""")
        note(f"Studio Lite showcase: {geo}")
        page.click('.sc-case[title^="SCM"]')
        page.wait_for_timeout(700)
        page.screenshot(path=str(out / "L8-studio-showcase-scm.png"))
        page.click('.sc-case[title^="Princess"]')
        for _ in range(9):
            page.keyboard.press("ArrowDown")
        page.wait_for_timeout(800)
        page.screenshot(path=str(out / "L9-studio-showcase-scrolled.png"))
        chosen = page.evaluate('document.querySelector(".sc-case[aria-pressed=true]").title')
        note(f"Studio list after 9 x ArrowDown: {chosen!r}")

        # Inside a frame, as Hugging Face shows the Space: a gap above our bar, gone in full screen.
        host = br.new_page(viewport={"width": 1440, "height": 900})
        for target, name in ((a.url, "F1-framed-landing"), (a.url.rstrip("/") + "/studio/index.html" if a.url.endswith("/") else a.url, "F2-framed-studio")):
            if "studio" in name and not target.endswith("studio/index.html"):
                target = a.url + "studio/index.html"
            host.set_content(
                "<body style='margin:0;background:#fff'><div style='height:56px;background:#f5f5f5;border-bottom:1px solid #ddd;"
                "font:600 15px sans-serif;padding:18px'>huggingface.co header (stand-in)</div>"
                f"<iframe src='{target}' allow='fullscreen; cross-origin-isolated' style='border:0;width:100%;height:calc(100vh - 57px)'></iframe></body>"
            )
            frame = host.frame_locator("iframe")
            frame.locator(".top, .appbar").first.wait_for(timeout=60_000)
            if "studio" in name:
                frame.locator("#boot").wait_for(state="detached", timeout=60_000)
            host.wait_for_timeout(600)
            framed = host.frames[1].evaluate("document.documentElement.className")
            host.screenshot(path=str(out / f"{name}.png"), clip={"x": 0, "y": 0, "width": 1440, "height": 220})
            note(f"{name}: html class {framed!r}")
        frame.locator("[data-ctl=fullscreen]").click()
        host.wait_for_timeout(700)
        note(f"framed studio in full screen: html class {host.frames[1].evaluate('document.documentElement.className')!r}, "
             f"body padding {host.frames[1].evaluate('getComputedStyle(document.body).paddingTop')}")
        host.goto("about:blank")

        note(f"errors: {[e for e in errors if 'favicon' not in e][:6]}")
        try:
            br.close()
        except Exception:  # noqa: BLE001 - the old driver can fail to close a page with workers
            pass


if __name__ == "__main__":
    main()
