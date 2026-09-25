"""Drive Inkvec Studio Lite end to end in headless Edge, and time its traces.

    python studio/web/smoke.py [--url http://127.0.0.1:8931/] [--out DIR] [--bench IMG ...]

Serve the site first (`python studio/web/serve.py`). Needs `pip install playwright` and
Microsoft Edge. Screenshots are of the page only. The flow: open a sample (the automatic
final trace), the Custom wizard and its previews, a draft and a final after a preset change,
merging two inks into a colour group, export as a download, Minify, Fabricate, Settings,
About, Full screen, the embedded "Open in its own tab", and a phone. `--bench` then traces
each image at 512, 1024 and 2048 px through the page's own backend and reports wall time and
the WebAssembly memory high-water mark.
"""

from __future__ import annotations

import argparse
import base64
import json
import pathlib
import sys
import time

from playwright.sync_api import Page, sync_playwright

HERE = pathlib.Path(__file__).resolve().parent
STUDIO = HERE.parent


def status_line(page: Page) -> str:
    return page.evaluate(
        "[...document.querySelectorAll('footer, .statusbar, .status')].map(e => e.innerText).join(' | ')"
    )


def wait_final(page: Page, timeout: float = 180) -> float:
    """Wait until the viewer's badge says final and nothing is tracing; returns seconds."""
    t0 = time.perf_counter()
    while time.perf_counter() - t0 < timeout:
        state = page.evaluate(
            "(() => { const b = document.body.innerText; return { final: /final · \\d+ px/.test(b), draft: /draft · \\d+ px/.test(b) }; })()"
        )
        if state["final"] and not state["draft"]:
            return time.perf_counter() - t0
        page.wait_for_timeout(100)
    raise TimeoutError("no final trace")


def idle(page: Page, timeout: float = 180) -> None:
    """Wait until the settle timer has fired and the backend has nothing running or queued."""
    t0 = time.perf_counter()
    quiet = 0
    while time.perf_counter() - t0 < timeout:
        busy = page.evaluate("(() => { const b = window.__inkvecStudioLite; return Boolean(b.running) || b.queue.length > 0; })()")
        quiet = 0 if busy else quiet + 1
        # 1.2 s of quiet covers the 800 ms settle before the final is queued.
        if quiet >= 12:
            return
        page.wait_for_timeout(100)
    raise TimeoutError("the backend did not go quiet")


def flow(page: Page, url: str, out: pathlib.Path, note) -> None:
    t0 = time.perf_counter()
    page.goto(url)
    page.wait_for_selector("#boot", state="detached", timeout=60_000)
    info = page.evaluate("window.__inkvecStudioLite?.info")
    note(f"boot {time.perf_counter() - t0:.2f}s isolated={page.evaluate('crossOriginIsolated')} engine={info}")
    page.screenshot(path=str(out / "01-first-run.png"))

    # A sample: the automatic trace starts at once, final.
    t0 = time.perf_counter()
    page.click("button.sample >> nth=2")
    page.wait_for_selector(".palettecard .ink", timeout=180_000)
    wait_final(page)
    note(f"open + automatic final trace (crest): {time.perf_counter() - t0:.2f}s; last job {page.evaluate('window.__inkvecStudioLite.last')}")
    page.wait_for_timeout(500)
    page.screenshot(path=str(out / "02-traced-chooser.png"))

    # The Custom wizard, from the chooser: previews of this image under each choice.
    page.click("[data-ctl=chooser-custom]")
    t0 = time.perf_counter()
    page.wait_for_function("document.querySelectorAll('.kthumb img[src]').length >= 3", timeout=180_000)
    n = page.evaluate("document.querySelectorAll('.kthumb img[src]').length")
    note(f"wizard: {n} previews in {time.perf_counter() - t0:.2f}s")
    page.wait_for_timeout(1500)
    page.screenshot(path=str(out / "03-wizard.png"))
    page.click("[data-ctl=wiz-close]")
    page.wait_for_timeout(300)

    # A preset change: a draft at once, the final once the controls settle (for an image no
    # larger than the draft size, the draft is the final and the final comes from the cache).
    t0 = time.perf_counter()
    page.keyboard.press("Control+2")
    page.wait_for_timeout(300)
    idle(page)
    note(f"preset change (Icon): settled at {time.perf_counter() - t0:.2f}s")
    page.screenshot(path=str(out / "04-after-preset.png"))
    page.keyboard.press("Control+1")
    page.wait_for_timeout(300)
    idle(page)

    # Colour groups: tick two inks and merge them.
    inks = page.locator(".inklist .ink .inkpick")
    note(f"palette: {inks.count()} inks")
    if inks.count() >= 2:
        inks.nth(0).click()
        inks.nth(1).click()
        page.click('.mergebar button:has-text("Merge")')
        page.wait_for_timeout(300)
        wait_final(page)
        page.wait_for_timeout(800)
        groups = page.evaluate("document.querySelectorAll('.cgroups:not(.suggestions) .cgroup').length")
        report = page.evaluate("[...document.querySelectorAll('.creport')].map(e => e.textContent)")
        note(f"colour groups after merge: {groups}; report {report}")
        page.screenshot(path=str(out / "05-colour-group.png"))

    # Export: the sheet, then a download (a zip of every file).
    page.click('button.btn.primary:has-text("Export")')
    page.wait_for_selector(".sheet", timeout=10_000)
    page.wait_for_timeout(1500)
    page.screenshot(path=str(out / "06-export-sheet.png"))
    with page.expect_download(timeout=180_000) as dl:
        page.click('.sheet button.btn.primary:has-text("Download")')
    d = dl.value
    path = out / d.suggested_filename
    d.save_as(path)
    note(f"export download: {d.suggested_filename} {path.stat().st_size:,} bytes")
    page.wait_for_timeout(500)

    # Minify: an SVG from disk, minified, downloaded.
    page.click('.appbar button:has-text("Minify SVG")')
    with page.expect_file_chooser() as fc:
        page.click('button:has-text("Open an SVG")')
    fc.value.set_files(str(STUDIO / "dev" / "traced" / "crest-filigree.svg"))
    page.wait_for_timeout(2500)
    page.screenshot(path=str(out / "07-minify.png"))
    with page.expect_download(timeout=60_000) as dl:
        page.click('button:has-text("Download SVG")')
    note(f"minify download: {dl.value.suggested_filename}; {page.evaluate('document.body.innerText').count('bytes')} byte mentions")

    # Fabricate: the current trace, cut into sheets, downloaded.
    page.click('.appbar button:has-text("Fabricate")')
    page.wait_for_timeout(300)
    use = page.locator('button:has-text("Use the current trace")')
    if use.count():
        use.first.click()
    page.wait_for_timeout(3000)
    page.screenshot(path=str(out / "08-fabricate.png"))
    with page.expect_download(timeout=60_000) as dl:
        page.click('.railfoot button.btn.primary')
    note(f"fabricate download: {dl.value.suggested_filename}")

    # Settings and About, and Full screen.
    page.click('.appbar button:has-text("Vectorize")')
    page.click('.appbar button:has-text("Settings")')
    page.wait_for_timeout(500)
    page.screenshot(path=str(out / "09-settings.png"), full_page=True)
    page.click('button:has-text("Done")')
    page.click('.appbar button:has-text("About")')
    page.wait_for_timeout(800)
    page.screenshot(path=str(out / "10-about.png"))
    page.click('button:has-text("Done")')


def fullscreen(browser, url: str, out: pathlib.Path, note) -> None:
    page = browser.new_page(viewport={"width": 1440, "height": 900})
    page.goto(url)
    page.wait_for_selector("#boot", state="detached", timeout=60_000)
    page.click("[data-ctl=fullscreen]")
    page.wait_for_timeout(500)
    note(f"full screen: {page.evaluate('Boolean(document.fullscreenElement)')}, label {page.inner_text('[data-ctl=fullscreen]')!r}")


def embedded(browser, url: str, out: pathlib.Path, note) -> None:
    page = browser.new_page(viewport={"width": 1440, "height": 900})
    page.set_content(f"<body style='margin:0'><iframe src='{url}' style='border:0;width:100vw;height:100vh'></iframe></body>")
    frame = page.frame_locator("iframe")
    frame.locator("[data-ctl=own-tab]").wait_for(timeout=60_000)
    frame.locator("button.sample img").first.wait_for(timeout=30_000)
    page.wait_for_timeout(1500)
    note(f"embedded: own-tab button shown; full screen button {'shown' if frame.locator('[data-ctl=fullscreen]').count() else 'hidden (frame not allowed)'}")
    page.screenshot(path=str(out / "11-embedded.png"))
    page.goto("about:blank")


def phone(browser, url: str, out: pathlib.Path, note) -> None:
    ctx = browser.new_context(viewport={"width": 390, "height": 844}, is_mobile=True, has_touch=True, device_scale_factor=2)
    page = ctx.new_page()
    page.goto(url)
    page.wait_for_selector(".phonenote", timeout=60_000)
    note("phone: 'best on a larger screen' note shown")
    page.screenshot(path=str(out / "12-phone.png"))
    page.goto("about:blank")


BENCH_JS = """
async ({ b64, name, sizes, tier }) => {
  const be = window.__inkvecStudioLite;
  const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
  const t0 = performance.now();
  const info = await be.invoke("open_bytes", { bytes, name });
  const openMs = performance.now() - t0;
  const prefs = await be.invoke("load_prefs");
  const rows = [];
  for (const size of sizes) {
    const settings = { ...prefs.trace, traceSize: size };
    const done = new Promise((resolve) => {
      let stop = null;
      be.listen("trace:done", (e) => { stop?.(); resolve(e.payload); }).then((u) => (stop = u));
    });
    const t = performance.now();
    await be.invoke("start_trace", { request: { settings, tier } });
    const r = await done;
    const ms = performance.now() - t;
    const o = r.outcome;
    rows.push({
      size, tier, ms: Math.round(ms), state: o.state, traced: o.tracedPx ?? null, tierOut: o.tier ?? null,
      de00: o.report?.meanDe00 ?? null, coords: o.report?.coordinates ?? null,
      engineS: o.report?.seconds ?? null, memMB: be.last?.mem ? Math.round(be.last.mem / 1048576) : null,
    });
  }
  return { name, w: info.width, h: info.height, openMs: Math.round(openMs), rows };
}
"""


def bench(browser, url: str, images: list[pathlib.Path], note, label: str) -> list[dict]:
    results = []
    for img in images:
        # A fresh page per image, so each one's memory high-water mark is its own.
        page = browser.new_page()
        page.goto(url)
        page.wait_for_selector("#boot", state="detached", timeout=60_000)
        info = page.evaluate("window.__inkvecStudioLite.info")
        b64 = base64.b64encode(img.read_bytes()).decode()
        r = page.evaluate(BENCH_JS, {"b64": b64, "name": img.name, "sizes": [512, 1024, 2048], "tier": "final"})
        d = page.evaluate(BENCH_JS, {"b64": b64, "name": img.name, "sizes": [1024], "tier": "draft"})
        r["draft"] = d["rows"][0]
        r["threads"] = info["threads"]
        results.append(r)
        for row in r["rows"] + [r["draft"]]:
            note(
                f"[{label}] {r['name']} {r['w']}x{r['h']} threads={info['threads']} {row['tier']}@{row['size']}: "
                f"{row['ms']} ms wall (engine {row['engineS']}s), traced {row['traced']} px, "
                f"dE00 {row['de00']}, {row['coords']} coords, wasm memory {row['memMB']} MB"
            )
        page.goto("about:blank")
    return results


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://127.0.0.1:8931/")
    ap.add_argument("--url-single", default=None, help="the same site without isolation headers, for a one-core bench")
    ap.add_argument("--out", default=str(STUDIO / ".shots-web"))
    ap.add_argument("--bench", nargs="*", default=[])
    ap.add_argument("--skip-flow", action="store_true")
    a = ap.parse_args()
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    logfile = (out / "smoke.log").open("w", encoding="utf-8")

    def note(msg: str) -> None:
        sys.stdout.buffer.write((msg + "\n").encode("utf-8"))
        sys.stdout.flush()
        logfile.write(msg + "\n")
        logfile.flush()

    with sync_playwright() as p:
        br = p.chromium.launch(channel="msedge", headless=True)
        if not a.skip_flow:
            ctx = br.new_context(viewport={"width": 1440, "height": 900}, accept_downloads=True, bypass_csp=True)
            page = ctx.new_page()
            page.on("console", lambda m: note(f"console.{m.type}: {m.text}") if m.type in ("error", "warning") else None)
            page.on("pageerror", lambda e: note(f"pageerror: {e}"))
            flow(page, a.url, out, note)
            # Navigated away rather than closed: the old Playwright driver here dies closing a
            # page that still has the engine's worker pool alive.
            page.goto("about:blank")
            # Each in a browser of its own: the full-screen flow above has been seen to take
            # the old Playwright driver down with it when the next page opens in the same one.
            for check in (embedded, phone, fullscreen):
                other = p.chromium.launch(channel="msedge", headless=True)
                try:
                    check(other, a.url, out, note)
                except Exception as e:  # noqa: BLE001 - report and carry on to the bench
                    note(f"{check.__name__}: FAILED {e}")
                finally:
                    try:
                        other.close()
                    except Exception:  # noqa: BLE001 - the old driver can fail to close a full-screen page
                        pass
        results = {}
        if a.bench:
            imgs = [pathlib.Path(x) for x in a.bench]
            results["isolated"] = bench(br, a.url, imgs, note, "threads")
            if a.url_single:
                results["single"] = bench(br, a.url_single, imgs, note, "one core")
            (out / "bench.json").write_text(json.dumps(results, indent=2), encoding="utf-8")
        try:
            br.close()
        except Exception:  # noqa: BLE001 - the old driver can fail to close a page with workers
            pass
    logfile.close()


if __name__ == "__main__":
    main()
