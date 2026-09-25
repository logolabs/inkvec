#!/usr/bin/env python3
"""Screenshots for the Studio user guide, taken from the browser dev mock.

The guide's pictures are of the real interface, driven in headless Microsoft Edge against
`dev/index.html` (the mock backend `npm run mock` serves). The desktop app itself is never
launched: it reads and writes the user's real preferences file.

    cd studio && npx vite --port 1437 --strictPort      # in one terminal
    python studio/tools/guide_shots.py --svgmin target/release/inkvec-svgmin.exe

Writes WebP files into `studio/public/guide/img/`, the one copy both the documentation site
and the guide bundled with the app use. Needs `pip install playwright pillow` and Edge.

The mock's traces are SVGs the real CLI wrote, and the Fabricate tab's sheets are the real
engine's; its report numbers are counted from those SVGs rather than measured. A few
commands the mock does not answer are answered here, from real output where there is one:
the Minify tab's rewrite comes from `inkvec-svgmin` itself, and the export sizes are those
of files encoded in the page. The app bar is relabelled "Inkvec Studio", the desktop app's
name from 0.2.0 on.
"""
from __future__ import annotations

import argparse
import io
import json
import re
import subprocess
import sys
from pathlib import Path

from PIL import Image
from playwright.sync_api import Page, sync_playwright

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "studio/public/guide/img"
UNICORN = ROOT / "studio/dev/fab/unicorn.svg"
W, H = 1440, 900

# Installed before the app starts: wraps the mock's `invoke` (see dev/mock.ts) so the few
# commands it leaves unanswered get an answer, and the trace size it reports matches the
# sample (the mock says 2048 px for every final trace; the app traces a 512 px sample at 512).
INIT = r"""
(() => {
  const extra = window.__guideExtra = window.__guideExtra || {};
  let side = 512;
  const wrap = (orig) => async (cmd, args, opts) => {
    if (cmd === 'plugin:event|emit' && args && args.event === 'trace:done') {
      const o = args.payload && args.payload.outcome;
      if (o && o.state === 'traced') {
        const px = Math.min(o.tier === 'draft' ? 512 : 2048, side);
        o.tracedPx = px; o.report.tracedPx = px; o.sourcePx = [side, side];
      }
    }
    if (extra[cmd]) {
      const r = await extra[cmd](args || {});
      if (r !== undefined) return r;
    }
    const r = await orig(cmd, args, opts);
    if ((cmd === 'open_sample' || cmd === 'open_path' || cmd === 'open_bytes') && r) side = Math.max(r.width, r.height);
    return r;
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


def relabel(pg: Page) -> None:
    pg.evaluate("""() => {
      for (const el of document.querySelectorAll('.brand')) {
        const t = [...el.childNodes].find(n => n.nodeType === 3);
        if (t) t.textContent = 'Inkvec Studio';
      }
    }""")


def save(pg: Page, name: str, clip: tuple[int, int, int, int] | None = None, width: int | None = None,
         quality: int = 80) -> None:
    relabel(pg)
    pg.wait_for_timeout(250)
    shot = pg.screenshot(clip=dict(zip("x y width height".split(), clip)) if clip else None)
    im = Image.open(io.BytesIO(shot)).convert("RGB")
    if width and im.width > width:
        im = im.resize((width, round(im.height * width / im.width)), Image.LANCZOS)
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / f"{name}.webp"
    im.save(path, "WEBP", quality=quality, method=6)
    print(f"  {path.relative_to(ROOT)}  {im.width}x{im.height}  {path.stat().st_size / 1024:.0f} KB")


def fresh(browser, base: str, query: str = "") -> Page:
    pg = browser.new_page(viewport={"width": W, "height": H})
    pg.add_init_script(INIT)
    pg.goto(f"{base}/dev/index.html{query}")
    pg.wait_for_selector(".appbar")
    pg.wait_for_timeout(800)
    return pg


def open_sample(pg: Page, label: str) -> None:
    pg.click(f"button.sample:has-text('{label}')")
    # The mock's full trace takes about a second and a half of staged events.
    pg.wait_for_selector(".chip.final", timeout=15000)
    pg.wait_for_timeout(400)


def rail_tab(pg: Page, name: str) -> None:
    pg.click(f".railseg button[role=tab]:has-text('{name}')")
    pg.wait_for_timeout(300)


def shots(browser, base: str, svgmin: Path | None) -> None:
    # --- first run -------------------------------------------------------------------
    pg = fresh(browser, base, "?fresh=1")
    save(pg, "first-run", width=1200)

    # --- the chooser, then the workspace --------------------------------------------
    open_sample(pg, "Flat logo")
    save(pg, "chooser", clip=(250, 480, 590, 390))
    pg.click("[data-ctl=chooser-auto]")
    pg.wait_for_timeout(300)
    save(pg, "workspace", width=1200)

    # --- overlays and detail ---------------------------------------------------------
    for label in ("Wireframe", "Anchors", "Handles"):
        pg.click(f"button.toggle:has-text('{label}')")
    pg.click(".viewertools .seg button:has-text('4×')")
    pg.wait_for_timeout(500)
    save(pg, "overlays", clip=(0, 44, 1088, 520), width=1000)
    for label in ("Wireframe", "Anchors", "Handles"):
        pg.click(f"button.toggle:has-text('{label}')")
    pg.click("button.toggle:has-text('Certainty')")
    pg.wait_for_timeout(600)
    pg.click("button.reset:has-text('Find the worst corner')")
    pg.wait_for_timeout(700)
    save(pg, "detail-certainty", clip=(0, 44, 1088, 520), width=1000)
    pg.close()

    # --- result and tune, on the crest (it has a loss to report) ------------------------
    pg = fresh(browser, base)
    open_sample(pg, "Crest, filigree")
    pg.click("[data-ctl=chooser-auto]")
    pg.wait_for_timeout(300)
    pg.evaluate("() => { const s = document.querySelector('.railscroll'); if (s) s.scrollTop = 150; }")
    save(pg, "result", clip=(1088, 44, 352, 640))
    rail_tab(pg, "Tune")
    pg.click("[data-ctl='precision:field']")
    pg.fill("[data-ctl='precision:field']", "0.05")
    pg.press("[data-ctl='precision:field']", "Enter")
    pg.wait_for_selector(".chip.final", timeout=15000)
    pg.wait_for_timeout(500)
    pg.evaluate("() => { const s = document.querySelector('.railscroll'); if (s) s.scrollTop = 0; }")
    save(pg, "tune", clip=(1088, 44, 352, 700))
    pg.close()

    # --- the wizard --------------------------------------------------------------------
    pg = fresh(browser, base)
    open_sample(pg, "Flat logo")
    pg.click("[data-ctl=chooser-custom]")
    pg.wait_for_timeout(3500)  # the preview drafts, one at a time
    save(pg, "wizard-kind", width=1200)
    pg.close()

    # A JPEG: the mock opens any .jpg path as a lossy crest, so Clean-up is offered.
    pg = fresh(browser, base, "?launch=C:/Brand/northwind-photo.jpg")
    pg.wait_for_selector(".chip.final", timeout=15000)
    pg.wait_for_timeout(1200)
    save(pg, "auto-chose-photo", clip=(250, 470, 590, 400))
    pg.click("[data-ctl=chooser-custom]")
    pg.wait_for_timeout(600)
    pg.click("[data-ctl=wiz-next]")
    pg.wait_for_timeout(600)
    save(pg, "wizard-cleanup", clip=(1039, 44, 401, 720))
    pg.close()

    # --- the palette and colour groups, on the emoji ------------------------------------
    pg = fresh(browser, base)
    open_sample(pg, "Emoji, gradients")
    pg.click("[data-ctl=chooser-auto]")
    pg.wait_for_timeout(400)
    pg.evaluate("""() => { const c = document.querySelector('.palettecard');
      if (c) c.scrollIntoView({block: 'start'}); }""")
    pg.wait_for_timeout(300)
    pg.hover(".cgroup.suggested")
    pg.wait_for_timeout(500)
    save(pg, "palette", clip=(740, 44, 700, 856), width=700)
    pg.close()

    # --- export ------------------------------------------------------------------------
    pg = fresh(browser, base)
    pg.evaluate(PLAN_EXPORT)
    open_sample(pg, "Flat logo")
    pg.click("[data-ctl=chooser-auto]")
    pg.wait_for_timeout(300)
    pg.click(".railfoot button.btn.primary:has-text('Export')")
    pg.wait_for_timeout(1500)
    save(pg, "export", clip=(1088, 470, 352, 430))
    pg.close()

    # --- minify ------------------------------------------------------------------------
    if svgmin:
        pg = fresh(browser, base)
        pg.evaluate("r => { window.__guideExtra.minify_svg = () => r; }", minify_result(svgmin))
        pg.click(".seg button:has-text('Minify SVG')")
        pg.wait_for_timeout(300)
        pg.click("button:has-text('Open an SVG')")
        pg.wait_for_timeout(1200)
        save(pg, "minify", width=1200)
        pg.close()
    else:
        print("  minify: skipped (pass --svgmin)")

    # --- fabricate ---------------------------------------------------------------------
    pg = fresh(browser, base)
    pg.click(".seg button:has-text('Fabricate')")
    pg.wait_for_timeout(300)
    pg.click(".minifyrail button:has-text('Open an SVG')")
    pg.wait_for_timeout(1000)
    # The mock's sheets were cut at 60 mm; say so, so the size and the sheets agree.
    set_width(pg, "60")
    pg.click(".minifyrail button.btn:has-text('Adhesive vinyl')")
    pg.wait_for_timeout(800)
    save(pg, "fabricate-vinyl", width=1200)
    pg.click(".minifyrail button.btn:has-text('Iron-on')")
    pg.wait_for_timeout(800)
    card_to_top(pg, "Colours, bottom sheet first")
    save(pg, "fabricate-htv-rail", clip=fab_rail(pg, 560))
    pg.click(".minifyrail button.btn:has-text('Laser or sign cutter')")
    pg.wait_for_timeout(800)
    card_to_top(pg, "Cutting")
    save(pg, "fabricate-laser-rail", clip=fab_rail(pg, 720))
    pg.close()

    # --- batch -------------------------------------------------------------------------
    pg = fresh(browser, base)
    pg.evaluate(BATCH)
    pg.click(".seg button:has-text('Batch')")
    pg.wait_for_timeout(300)
    pg.click("button:has-text('Choose a folder…')")
    pg.wait_for_timeout(800)
    save(pg, "batch", clip=(0, 44, 1440, 420), width=1200)
    pg.close()

    # --- settings ----------------------------------------------------------------------
    pg = fresh(browser, base)
    pg.click(".appbar button:has-text('Settings')")
    pg.wait_for_timeout(600)
    save(pg, "settings", width=1200)
    pg.close()

    # --- help --------------------------------------------------------------------------
    pg = fresh(browser, base)
    pg.keyboard.press("F1")
    pg.wait_for_timeout(1500)
    save(pg, "help", width=1200)
    pg.close()


def fab_rail(pg: Page, height: int) -> tuple[int, int, int, int]:
    """The top `height` pixels of the Fabricate tab's rail, which is wider than Vectorize's."""
    box = pg.locator(".minifyrail").bounding_box()
    return (round(box["x"]), round(box["y"]), round(box["width"]), height)


def card_to_top(pg: Page, eyebrow: str) -> None:
    """Scroll the Fabricate rail so the card headed `eyebrow` is at its top."""
    pg.evaluate("""(label) => {
      const heads = [...document.querySelectorAll('.minifyrail .card .eyebrow')];
      const head = heads.find(e => e.textContent.trim().toLowerCase() === label.toLowerCase());
      const card = head && head.closest('.card');
      const s = document.querySelector('.minifyrail .railscroll');
      if (card && s) s.scrollTop += card.getBoundingClientRect().top - s.getBoundingClientRect().top - 12;
    }""", eyebrow)
    pg.wait_for_timeout(250)


def set_width(pg: Page, mm: str) -> None:
    field = pg.locator(".minifyrail .sentence input.inlinefield").first
    field.fill(mm)
    field.press("Enter")
    pg.wait_for_timeout(500)


# The export sheet asks the backend what each format would weigh. The mock answers with
# placeholder groups; this builds the files in the page from the drawing on screen (the SVG
# as it is, PNG encoded by the browser) and reports their sizes.
PLAN_EXPORT = r"""
() => {
  const png = async (svg, size) => {
    const img = new Image();
    const url = URL.createObjectURL(new Blob([svg], {type: 'image/svg+xml'}));
    await new Promise((ok, bad) => { img.onload = ok; img.onerror = bad; img.src = url; });
    const c = document.createElement('canvas'); c.width = size; c.height = size;
    c.getContext('2d').drawImage(img, 0, 0, size, size);
    const blob = await new Promise(ok => c.toBlob(ok, 'image/png'));
    return blob.size;
  };
  window.__guideExtra.plan_export = async ({request}) => {
    const f = request.formats, svg = request.svg, out = [];
    const min = svg.replace(/\s+/g, ' ').replace(/ ?(id|class)="[^"]*"/g, '');
    const pngs = {};
    for (const s of [16, 32, 48, 512, 1024, 2048]) pngs[s] = await png(svg, s);
    const ico = 6 + 48 + pngs[16] + pngs[32] + pngs[48];
    if (f.svg) out.push({name: 'flat-logo.svg', bytes: svg.length, group: 'svg'});
    if (f.svgMinified) out.push({name: 'flat-logo.min.svg', bytes: min.length, group: 'svgMinified'});
    for (const s of f.pngSizes) out.push({name: `png/flat-logo-${s}.png`, bytes: pngs[s], group: 'png'});
    if (f.favicon) {
      for (const s of [16, 32, 48]) out.push({name: `favicon/favicon-${s}.png`, bytes: pngs[s], group: 'favicon'});
      out.push({name: 'favicon/favicon.ico', bytes: ico, group: 'favicon'});
    }
    if (f.assetPack) {
      const inside = svg.length + min.length + pngs[512] + pngs[1024] + pngs[2048] + pngs[16] + pngs[32] + pngs[48] + ico + 900;
      out.push({name: 'flat-logo-assets.zip', bytes: inside, group: 'assetPack'});
    } else {
      out.push({name: 'palette.json', bytes: 300, group: 'svg'});
    }
    return out;
  };
}
"""

BATCH = r"""
() => {
  const files = ['acme-mark.png', 'acme-wordmark.png', 'badge-round.webp', 'crest-scan.jpg',
    'footer-icon.png', 'hero-emblem.png', 'partner-logo-01.png', 'partner-logo-02.png',
    'partner-logo-03.jpg', 'social-avatar.png', 'sticker-sheet.png', 'wave-icon.png'];
  window.__guideExtra['plugin:dialog|open'] = ({options}) => options && options.directory ? 'C:\\Brand\\logos' : undefined;
  window.__guideExtra.batch_scan = () => files.map(f => 'C:\\Brand\\logos\\' + f);
}
"""


def measured_difference(a: str, b: str) -> float | None:
    """Mean dE00 between renders of two SVGs at 1024 px, as the tab measures it, using the
    bench's renderer; None (shown as a dash) where the bench is not installed."""
    try:
        sys.path.insert(0, str(ROOT / "bench"))
        import crosscompare_current as cc
        from skimage.color import deltaE_ciede2000, rgb2lab
    except ImportError:
        return None
    return float(deltaE_ciede2000(rgb2lab(cc.rgb(a, 1024)), rgb2lab(cc.rgb(b, 1024))).mean())


def minify_result(svgmin: Path) -> dict:
    """What the Minify tab would show for the fixture drawing, from the real minifier."""
    run = subprocess.run([str(svgmin), str(UNICORN), "--stats"], capture_output=True, text=True, check=True)
    before = UNICORN.read_text(encoding="utf-8")
    after = run.stdout
    m = re.search(r"(\d+) paths .* segments (\d+) -> (\d+), params (\d+) -> (\d+).*?(\d+) runs guarded", run.stderr)
    if not m:
        raise SystemExit(f"unexpected --stats line: {run.stderr!r}")
    paths, seg0, seg1, par0, par1, guarded = map(int, m.groups())
    return {
        "svg": after, "bytesBefore": len(before.encode()), "bytesAfter": len(after.encode()),
        "numbersBefore": par0, "numbersAfter": par1, "pathsBefore": paths, "pathsAfter": paths,
        "segmentsBefore": seg0, "segmentsAfter": seg1, "primitives": 0, "guarded": guarded,
        "toleranceUnits": 0.0125, "differenceDe00": measured_difference(before, after), "ms": 4.0,
        "removed": [],
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", default="http://127.0.0.1:1437", help="where vite serves the studio")
    ap.add_argument("--svgmin", type=Path, help="an inkvec-svgmin binary, for the Minify shot")
    args = ap.parse_args()
    with sync_playwright() as p:
        browser = p.chromium.launch(channel="msedge", headless=True)
        try:
            shots(browser, args.base.rstrip("/"), args.svgmin)
        finally:
            browser.close()
    total = sum(f.stat().st_size for f in OUT.glob("*.webp"))
    print(f"{len(list(OUT.glob('*.webp')))} images, {total / 1024:.0f} KB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
