"""Check Inkvec Studio Lite's loading screen, its denoiser download and its remembered
preferences, in headless Edge.

    python studio/web/lite_check.py [--url http://127.0.0.1:8931/studio/index.html]
                                    [--out DIR] [--only boot,prefetch,cold,savedata,prefs,timing]
                                    [--base-url URL]  # an older build, for boot times side by side

Serve the built site first (`python studio/web/serve.py`). Needs `pip install playwright`,
Microsoft Edge and a network connection (the denoiser comes from Hugging Face and jsDelivr).
Screenshots are of the page only. The checks:

- boot      the loading screen at three moments of a throttled start, directly and inside a
            frame that stands in for Hugging Face's (a header and the Space's frame below it);
- prefetch  the denoiser's background download starts once the engine's bytes are in, lands
            in Cache Storage, and a second visit makes no request for it;
- cold      the denoiser turned on before it has downloaded: the rail shows percent and MB,
            the trace is shown without it, and it traces again with it when it is ready;
- savedata  with `navigator.connection.saveData`, nothing is fetched in the background;
- prefs     choices survive a reload, and a page whose storage throws still starts;
- timing    boot time (navigation to the loading screen gone), with and without the
            background download, unthrottled and on a throttled connection.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import statistics
import sys
import time

from playwright.sync_api import BrowserContext, Page, sync_playwright

HERE = pathlib.Path(__file__).resolve().parent
MODEL = "restorer.onnx"
RUNTIME = "ort-wasm-simd-threaded.asyncify.wasm"
REMOTE = ("huggingface.co", "hf.co", "cdn.jsdelivr.net")

SAVE_DATA = """
Object.defineProperty(Navigator.prototype, 'connection', {
  configurable: true,
  get: () => ({ saveData: true, effectiveType: '4g', addEventListener() {}, removeEventListener() {} }),
});
"""

# localStorage that throws on every use, as a private window with storage blocked does.
BLOCKED_STORAGE = """
Object.defineProperty(window, 'localStorage', {
  configurable: true,
  get: () => { throw new DOMException('The operation is insecure.', 'SecurityError'); },
});
"""


def throttle(ctx: BrowserContext, page: Page, mbps: float, latency_ms: float = 20) -> None:
    cdp = ctx.new_cdp_session(page)
    cdp.send("Network.enable")
    rate = mbps * 1_000_000 / 8
    cdp.send("Network.emulateNetworkConditions", {"offline": False, "latency": latency_ms, "downloadThroughput": rate, "uploadThroughput": rate})


def boot_state(page: Page) -> list | None:
    return page.evaluate(
        "(() => { const s = document.getElementById('boot-status'); if (!s || !document.getElementById('boot')) return null;"
        " return [s.textContent, document.getElementById('boot-pct').textContent, document.getElementById('boot-detail').textContent]; })()"
    )


def fetch_state(page: Page) -> dict:
    return page.evaluate("JSON.parse(JSON.stringify(window.__inkvecStudioLite?.denoiserFetch ?? null))")


def wait_fetch(page: Page, phases: tuple[str, ...], timeout: float = 600) -> dict:
    t0 = time.perf_counter()
    while time.perf_counter() - t0 < timeout:
        f = fetch_state(page)
        if f and f["phase"] in phases:
            return f
        page.wait_for_timeout(250)
    raise TimeoutError(f"the denoiser never reached {phases}: {fetch_state(page)}")


def wait_final(page: Page, timeout: float = 300) -> None:
    t0 = time.perf_counter()
    while time.perf_counter() - t0 < timeout:
        if page.evaluate("(() => { const b = document.body.innerText; return /final · \\d+ px/.test(b) && !/draft · \\d+ px/.test(b); })()"):
            return
        page.wait_for_timeout(150)
    raise TimeoutError("no final trace")


def idle(page: Page, timeout: float = 300) -> None:
    """Until the backend has had nothing running or queued for 1.2 s (past the settle timer)."""
    t0 = time.perf_counter()
    quiet = 0
    while time.perf_counter() - t0 < timeout:
        busy = page.evaluate("(() => { const b = window.__inkvecStudioLite; return Boolean(b.running) || b.queue.length > 0; })()")
        quiet = 0 if busy else quiet + 1
        if quiet >= 12:
            return
        page.wait_for_timeout(100)
    raise TimeoutError("the backend did not go quiet")


# ------------------------------------------------------------------------ boot ---


def check_boot(browser, url: str, out: pathlib.Path, note) -> None:
    """Three moments of a start on a throttled connection, then the same inside a frame."""
    ctx = browser.new_context(viewport={"width": 1440, "height": 900})
    page = ctx.new_page()
    throttle(ctx, page, 20)
    page.goto(url, wait_until="commit")
    page.wait_for_selector("#boot", state="attached")
    taken: list[str] = []
    wanted = [("early", lambda s: s[0] != "Downloading the engine" or int(s[1].rstrip("%")) < 30),
              ("download", lambda s: s[0] == "Downloading the engine" and int(s[1].rstrip("%")) >= 40),
              ("starting", lambda s: s[0] in ("Starting the engine", "Ready") or int(s[1].rstrip("%")) >= 88)]
    seen = []
    while True:
        s = boot_state(page)
        if s is None:
            break
        seen.append(s)
        for name, test in wanted:
            if name not in taken and test(s):
                page.screenshot(path=str(out / f"B{len(taken) + 1}-boot-{name}.png"))
                taken.append(name)
                note(f"boot {name}: {s}")
                break
        page.wait_for_timeout(80)
    # One frame after the hand-over: the app, with nothing left of the loading screen.
    page.wait_for_timeout(400)
    page.screenshot(path=str(out / "B4-boot-app.png"))
    steps = sorted({s[0] for s in seen})
    pcts = [int(s[1].rstrip("%")) for s in seen]
    note(f"boot direct: {len(seen)} samples, steps {steps}, percent monotonic {pcts == sorted(pcts)}, shots {taken}")
    page.goto("about:blank")
    ctx.close()

    # Inside a frame that stands in for Hugging Face's: their header, and the Space under it.
    # The app's script is held back 2 s there, so the pictures are of a start in progress rather
    # than of a local server that is faster than any real one.
    ctx = browser.new_context(viewport={"width": 1440, "height": 900})

    def slow(route):
        time.sleep(2)
        route.continue_()

    # The app's own script (a request of the framed page; a worker's fetches are not routed).
    ctx.route("**/studio/assets/main-*.js", slow)
    page = ctx.new_page()
    header = (
        "<div style='height:56px;display:flex;align-items:center;gap:10px;padding:0 20px;background:#fff;"
        "border-bottom:1px solid #e5e7eb;font:600 14px system-ui;color:#111'>"
        "<span style='color:#6b7280'>Spaces</span> Logolabs / inkvec"
        "<span style='margin-left:auto;font-weight:400;color:#6b7280'>(a stand-in for huggingface.co)</span></div>"
    )
    page.set_content(
        f"<body style='margin:0;background:#fff'>{header}"
        f"<iframe src='{url}' allow='fullscreen' style='border:0;display:block;width:100vw;height:calc(100vh - 57px)'></iframe></body>"
    )
    frame = None
    t0 = time.perf_counter()
    while frame is None and time.perf_counter() - t0 < 30:
        frame = next((f for f in page.frames if f.url.startswith(url.split("?")[0])), None)
        page.wait_for_timeout(20)
    shots = 0
    framed_states = []
    while frame and shots < 2:
        try:
            s = frame.evaluate(
                "(() => { const s = document.getElementById('boot-status'); return s && document.getElementById('boot') ? [s.textContent, document.getElementById('boot-pct').textContent, document.documentElement.className] : null })()"
            )
        except Exception:  # noqa: BLE001 - the frame is still navigating
            s = []
        if s is None:
            break
        if s:
            framed_states.append(s)
            page.screenshot(path=str(out / f"B{5 + shots}-boot-framed.png"))
            shots += 1
            page.wait_for_timeout(600)
        page.wait_for_timeout(100)
    frame_el = page.frame_locator("iframe")
    frame_el.locator(".appbar").wait_for(timeout=60_000)
    page.wait_for_timeout(600)
    page.screenshot(path=str(out / "B7-framed-app.png"))
    note(f"boot framed: {framed_states}; app shown, isolated in frame = {frame.evaluate('crossOriginIsolated') if frame else '?'}")
    page.goto("about:blank")
    ctx.close()


# -------------------------------------------------------------------- prefetch ---


def check_prefetch(browser, url: str, out: pathlib.Path, note) -> None:
    ctx = browser.new_context(viewport={"width": 1440, "height": 900})
    page = ctx.new_page()
    t0 = time.perf_counter()
    events: list[tuple[float, str, str]] = []
    page.on("request", lambda r: events.append((time.perf_counter() - t0, "request", r.url)))
    page.on("requestfinished", lambda r: events.append((time.perf_counter() - t0, "finished", r.url)))
    page.goto(url)
    page.wait_for_selector("#boot", state="detached", timeout=60_000)
    ready = time.perf_counter() - t0
    wasm_done = next((t for t, k, u in events if k == "finished" and "inkvec_studio_wasm_bg.wasm" in u), None)
    model_start = next((t for t, k, u in events if k == "request" and MODEL in u), None)
    runtime_start = next((t for t, k, u in events if k == "request" and RUNTIME in u), None)
    note(f"prefetch: engine wasm in at {wasm_done:.2f}s, model request at {model_start}, runtime request at {runtime_start}, app ready at {ready:.2f}s")
    page.wait_for_timeout(1500)
    page.screenshot(path=str(out / "P1-prefetch-running.png"))
    t1 = time.perf_counter()
    f = wait_fetch(page, ("stored", "failed"))
    note(f"prefetch: {f['phase']} after {time.perf_counter() - t1 + 1.5:.1f}s more, {f.get('total')} bytes; "
         f"model request after the engine's bytes: {model_start is not None and wasm_done is not None and model_start >= wasm_done}")
    page.screenshot(path=str(out / "P2-prefetch-stored.png"))
    caches = page.evaluate("caches.keys()")
    note(f"prefetch: Cache Storage {caches}")

    # A second visit: the same browser profile, a new page. Nothing is fetched for the denoiser.
    page.goto("about:blank")
    page2 = ctx.new_page()
    remote: list[str] = []
    page2.on("request", lambda r: remote.append(r.url) if any(h in r.url for h in REMOTE) else None)
    page2.goto(url)
    page2.wait_for_selector("#boot", state="detached", timeout=60_000)
    f2 = wait_fetch(page2, ("stored", "failed", "idle"), timeout=30)
    page2.wait_for_timeout(4000)
    note(f"second visit: phase {f2['phase']}, remote requests {len(remote)} {remote[:3]}")
    page2.goto("about:blank")
    ctx.close()


# ------------------------------------------------------------------------ cold ---


def check_cold(browser, url: str, out: pathlib.Path, note, mbps: float) -> None:
    ctx = browser.new_context(viewport={"width": 1440, "height": 900})
    page = ctx.new_page()
    page.on("pageerror", lambda e: note(f"pageerror: {e}"))
    throttle(ctx, page, mbps)
    page.goto(url)
    page.wait_for_selector("#boot", state="detached", timeout=120_000)
    page.click("button.sample >> nth=0")
    page.wait_for_selector(".palettecard .ink", timeout=180_000)
    # Straight away, with the denoiser nowhere near downloaded.
    page.click("[data-ctl='cleanUpDamage:on']")
    shot = 0
    t0 = time.perf_counter()
    lines = []
    while time.perf_counter() - t0 < 900:
        f = fetch_state(page)
        text = page.evaluate("document.querySelector('[data-ctl=denoiser-progress]')?.innerText ?? ''")
        chip = page.evaluate("document.querySelector('[data-ctl=denoiser-chip]')?.innerText ?? ''")
        if f and f["phase"] == "downloading" and "%" in text and "MB" in text and shot == 0 and (f.get("got") or 0) > 3_000_000:
            page.screenshot(path=str(out / "C1-denoiser-downloading.png"))
            shot = 1
            lines.append(f"rail {text!r} | strip {chip!r}")
        if f and f["phase"] == "downloading" and shot == 1 and f.get("total") and f["got"] > 0.6 * f["total"]:
            page.screenshot(path=str(out / "C2-denoiser-downloading-later.png"))
            shot = 2
            lines.append(f"rail {text!r}")
        if f and f["phase"] == "preparing" and shot < 3:
            page.screenshot(path=str(out / "C3-denoiser-preparing.png"))
            shot = 3
            lines.append(f"rail {text!r} | strip {chip!r}")
        if f and f["phase"] in ("ready", "failed"):
            break
        page.wait_for_timeout(200)
    f = fetch_state(page)
    for line in lines:
        note(f"cold: {line}")
    note(f"cold: {f['phase']} after {time.perf_counter() - t0:.1f}s")
    # The trace again, with the denoiser this time, and nothing left running.
    t1 = time.perf_counter()
    while page.evaluate("window.__inkvecStudioLite.denoiserRuns") < 1 and time.perf_counter() - t1 < 300:
        page.wait_for_timeout(200)
    idle(page)
    wait_final(page)
    close = page.locator("[data-ctl=chooser-auto]")
    if close.count():
        close.first.click()
    page.wait_for_timeout(1200)
    runs = page.evaluate("window.__inkvecStudioLite.denoiserRuns")
    page.screenshot(path=str(out / "C4-denoised-result.png"))
    caption = page.evaluate("document.querySelector('.mode .modestate')?.innerText")
    note(f"cold: traced again with the denoiser: {runs} network run(s); rail now {caption!r}")
    page.goto("about:blank")
    ctx.close()


# -------------------------------------------------------------------- savedata ---


def check_savedata(browser, url: str, out: pathlib.Path, note) -> None:
    ctx = browser.new_context(viewport={"width": 1440, "height": 900})
    ctx.add_init_script(SAVE_DATA)
    page = ctx.new_page()
    remote: list[str] = []
    page.on("request", lambda r: remote.append(r.url) if any(h in r.url for h in REMOTE) else None)
    page.goto(url)
    page.wait_for_selector("#boot", state="detached", timeout=60_000)
    page.wait_for_timeout(8000)
    f = fetch_state(page)
    note(f"saveData: navigator.connection.saveData={page.evaluate('navigator.connection.saveData')}, fetch {f}, remote requests in 8 s: {len(remote)}")
    page.goto("about:blank")
    ctx.close()


# ----------------------------------------------------------------------- prefs ---


def check_prefs(browser, url: str, out: pathlib.Path, note) -> None:
    ctx = browser.new_context(viewport={"width": 1440, "height": 900})
    ctx.add_init_script(SAVE_DATA)  # keep the denoiser out of it
    page = ctx.new_page()
    page.on("pageerror", lambda e: note(f"pageerror: {e}"))
    page.goto(url)
    page.wait_for_selector("#boot", state="detached", timeout=60_000)
    before_any = snapshot(page)
    page.click("button.sample >> nth=0")
    page.wait_for_selector(".palettecard .ink", timeout=180_000)
    wait_final(page)
    page.click("[data-ctl=chooser-auto]") if page.locator("[data-ctl=chooser-auto]").count() else None
    # A handful of choices from different corners of the interface.
    page.keyboard.press("Control+2")  # the Icon preset
    page.wait_for_timeout(400)
    page.click("[data-ctl='editability:switch']")
    for label in ("Wipe", "Wireframe", "Anchors", "Detail"):
        page.click(f".viewertools button:has-text('{label}')")
        page.wait_for_timeout(150)
    page.click(".railtabs [role=tab]:has-text('Tune')")
    # The export sheet: the favicon set ticked.
    page.click('button.btn.primary:has-text("Export")')
    page.wait_for_selector(".sheet", timeout=10_000)
    page.click(".sheet :text('ICO and favicon set')")
    page.wait_for_timeout(1200)
    before = snapshot(page)
    note(f"prefs defaults at first: {before_any}")
    note(f"prefs before reload:     {before}")
    page.reload()
    page.wait_for_selector("#boot", state="detached", timeout=60_000)
    page.wait_for_timeout(800)
    after = snapshot(page)
    note(f"prefs after reload:      {after}")
    same = {k: before[k] == after[k] for k in before}
    note(f"prefs restored after reload: {same}")
    page.screenshot(path=str(out / "R1-prefs-after-reload.png"))
    # The last tab, too.
    page.click('.appbar button:has-text("Fabricate")')
    page.wait_for_timeout(1200)
    page.reload()
    page.wait_for_selector("#boot", state="detached", timeout=60_000)
    page.wait_for_timeout(500)
    note(f"prefs: tab after reload {snapshot(page)['tab']}")
    page.goto("about:blank")
    ctx.close()

    # Storage that throws: the page still starts, on the defaults, and traces.
    ctx = browser.new_context(viewport={"width": 1440, "height": 900})
    ctx.add_init_script(BLOCKED_STORAGE)
    ctx.add_init_script(SAVE_DATA)
    page = ctx.new_page()
    errors: list[str] = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(url)
    page.wait_for_selector("#boot", state="detached", timeout=60_000)
    blocked = page.evaluate("(() => { try { localStorage.length; return false; } catch { return true; } })()")
    page.click("button.sample >> nth=0")
    page.wait_for_selector(".palettecard .ink", timeout=180_000)
    wait_final(page)
    page.click("[data-ctl='editability:switch']")
    page.wait_for_timeout(1500)
    page.screenshot(path=str(out / "R2-storage-blocked.png"))
    note(f"prefs with storage blocked (throws: {blocked}): booted and traced; page errors {errors}")
    page.goto("about:blank")
    ctx.close()


def snapshot(page: Page) -> dict:
    return page.evaluate(
        """(() => {
          const on = (sel) => [...document.querySelectorAll(sel)].map(e => e.textContent.trim());
          let ui = null;
          try { ui = JSON.parse(localStorage.getItem('inkvec-studio-lite:prefs') || 'null')?.ui ?? null; } catch {}
          return {
            tab: on('.appbar > .seg button[aria-pressed=true]'),
            preset: on('button.preset[aria-pressed=true] .name'),
            editable: document.querySelector('[data-ctl="editability:switch"]')?.getAttribute('aria-checked'),
            // The zoom stops are left out: zoom belongs to the image, and a new one opens fitted.
            viewer: on('.viewertools button[aria-pressed=true]').filter(t => !/^(Fit|\d+×)$/.test(t)),
            railTab: on('.railtabs [aria-selected=true]'),
            exportFavicon: ui?.export?.favicon ?? null,
          };
        })()"""
    )


# ---------------------------------------------------------------------- timing ---


def boot_time(browser, url: str, mbps: float | None, save_data: bool) -> float:
    ctx = browser.new_context(viewport={"width": 1440, "height": 900})
    if save_data:
        ctx.add_init_script(SAVE_DATA)
    page = ctx.new_page()
    if mbps:
        throttle(ctx, page, mbps)
    t0 = time.perf_counter()
    page.goto(url)
    page.wait_for_selector("#boot", state="detached", timeout=120_000)
    dt = time.perf_counter() - t0
    page.goto("about:blank")
    ctx.close()
    return dt


def check_timing(browser, url: str, base: str | None, note) -> dict:
    rows = {}
    plan = [("new, background download", url, None, False), ("new, no background download (saveData)", url, None, True)]
    if base:
        plan.insert(0, ("before (main)", base, None, False))
    for mbps in (None, 20.0):
        for label, u, _, sd in plan:
            n = 5 if mbps is None else 3
            times = [boot_time(browser, u, mbps, sd) for _ in range(n)]
            key = f"{label} @ {'localhost' if mbps is None else f'{mbps:g} Mbit/s'}"
            rows[key] = times
            note(f"timing {key}: median {statistics.median(times):.2f}s  {['%.2f' % t for t in times]}")
    return rows


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://127.0.0.1:8931/studio/index.html")
    ap.add_argument("--base-url", default=None)
    ap.add_argument("--out", default=str(HERE.parent / ".shots-web"))
    ap.add_argument("--only", default="boot,prefetch,cold,savedata,prefs,timing")
    ap.add_argument("--cold-mbps", type=float, default=40.0)
    a = ap.parse_args()
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    log = (out / "lite_check.log").open("w", encoding="utf-8")

    def note(msg: str) -> None:
        sys.stdout.buffer.write((msg + "\n").encode("utf-8"))
        sys.stdout.flush()
        log.write(msg + "\n")
        log.flush()

    only = set(a.only.split(","))
    with sync_playwright() as p:
        for name in ("boot", "prefetch", "cold", "savedata", "prefs", "timing"):
            if name not in only:
                continue
            # A browser of its own each: the old Playwright driver here has been seen to die
            # closing a page that still has the engine's worker pool alive.
            br = p.chromium.launch(channel="msedge", headless=True)
            try:
                if name == "boot":
                    check_boot(br, a.url, out, note)
                elif name == "prefetch":
                    check_prefetch(br, a.url, out, note)
                elif name == "cold":
                    check_cold(br, a.url, out, note, a.cold_mbps)
                elif name == "savedata":
                    check_savedata(br, a.url, out, note)
                elif name == "prefs":
                    check_prefs(br, a.url, out, note)
                elif name == "timing":
                    (out / "boot_times.json").write_text(json.dumps(check_timing(br, a.url, a.base_url, note), indent=2), encoding="utf-8")
            except Exception as e:  # noqa: BLE001 - report and carry on to the next check
                note(f"{name}: FAILED {e}")
            finally:
                try:
                    br.close()
                except Exception:  # noqa: BLE001
                    pass
    log.close()


if __name__ == "__main__":
    main()
