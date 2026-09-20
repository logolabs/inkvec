#!/usr/bin/env python3
"""Build the Inkvec documentation site from the repo's own markdown.

The site is static HTML written to `site/`: the repo's docs rendered into the LogoLabs
system (warm charcoal ground, one copper accent, Playfair Display + Inter + IBM Plex
Mono), a sidebar, per-page tables of contents, and nothing else -- no framework, no
client-side JavaScript beyond KaTeX on the one page that has equations.

    python tools/build_site.py                 # --base /inkvec/, for GitHub Pages
    python tools/build_site.py --base /        # local preview: serve site/ at the root

Every internal link is resolved against the page's own source directory and mapped to
its site page. Files that are not on the site -- source files, internal working notes --
link to github.com instead. A link to a path that exists nowhere is a build error.
"""
from __future__ import annotations

import argparse
import html as html_mod
import json
import posixpath
import re
import shutil
import sys
from pathlib import Path

import markdown
from markdown import Extension
from markdown.extensions.fenced_code import FencedCodeExtension
from markdown.extensions.tables import TableExtension
from markdown.extensions.toc import TocExtension
from markdown.treeprocessors import Treeprocessor
from pygments import highlight as pyg_highlight
from pygments.formatters import HtmlFormatter
from pygments.lexers import get_lexer_by_name
from pygments.style import Style
from pygments.token import (Comment, Error, Generic, Keyword, Name, Number,
                            Operator, Punctuation, String, Text as TextToken)
from pygments.util import ClassNotFound

if sys.stdout and hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "site"
GITHUB = "https://github.com/logolabs/inkvec"
SPACE = "https://huggingface.co/spaces/Logolabs/inkvec"
LOGOLABS = "https://logolabs.org"
DEFAULT_BASE = "/inkvec/"

STAGES = [
    ("00-overview", "Overview"),
    ("01-intake", "Intake"),
    ("02-coverage", "Coverage"),
    ("03-palette", "Palette"),
    ("04-regions", "Regions"),
    ("05-gradients", "Gradients"),
    ("06-planar-map", "Planar map"),
    ("07-subpixel", "Sub-pixel refinement"),
    ("08-boundary-solve", "Boundary solve"),
    ("09-decode", "Decode"),
    ("10-symmetry", "Symmetry"),
    ("11-fitting", "Curve fitting"),
    ("12-repair", "Repair"),
    ("13-emit", "Emit"),
]

# Site slug -> (markdown source, sidebar label). "" is the home page. The stage pages
# are algorithm/NN-x.html from docs/algorithm/NN-x.md; both lists are joined below.
DOCS = {
    "": ("README.md", "Overview"),
    "design.html": ("docs/DESIGN.md", "Design"),
    "pipeline.html": ("PIPELINE_EXPLANATION.md", "Pipeline"),
    "bindings.html": ("docs/BINDINGS.md", "Bindings"),
    "limitations.html": ("docs/LIMITATIONS.md", "Limitations"),
    "ai-usage.html": ("docs/AI_USAGE.md", "AI usage"),
    "licences.html": ("docs/THIRD_PARTY.md", "Licences"),
    "results.html": ("docs/results/2026-09-15.md", "Results"),
    "changelog.html": ("CHANGELOG.md", "Changelog"),
    "contributing.html": ("CONTRIBUTING.md", "Contributing"),
    "security.html": ("SECURITY.md", "Security"),
    "code-of-conduct.html": ("CODE_OF_CONDUCT.md", "Code of conduct"),
    "algorithm/index.html": ("docs/algorithm/README.md", "How inkvec works"),
    "algorithm/constants.html": ("docs/algorithm/constants.md", "Constants"),
}

NAV = [
    ("Getting started", [("", "Overview"), ("pipeline.html", "The pipeline, explained")]),
    ("How it works", [("algorithm/index.html", "The series")]
     + [(f"algorithm/{slug}.html", label) for slug, label in STAGES]
     + [("algorithm/constants.html", "Constants and thresholds")]),
    ("Reference", [("design.html", "Design"), ("bindings.html", "Bindings"),
                   ("limitations.html", "Limitations"), ("ai-usage.html", "AI usage"),
                   ("licences.html", "Licences")]),
    ("Project", [("results.html", "Results"), ("changelog.html", "Changelog"),
                 ("contributing.html", "Contributing"), ("security.html", "Security policy"),
                 ("code-of-conduct.html", "Code of conduct")]),
]


class BuildError(Exception):
    pass


# --------------------------------------------------------------------------- markdown


class InkvecCodeStyle(Style):
    """Pygments colours from the LogoLabs ramp: copper keywords, gold numbers, mint
    strings, faint italic comments, cream text."""

    background_color = ""
    highlight_color = "#2a2724"
    styles = {
        TextToken: "#faf8f5",
        Punctuation: "#bfb9b1",
        Comment: "italic #8a837b",
        Keyword: "#c9754a",
        Keyword.Type: "#b8976c",
        Operator: "#c9754a",
        Name: "#faf8f5",
        Name.Function: "#d4896a",
        Name.Class: "#d4896a",
        Name.Builtin: "#b8976c",
        Name.Tag: "#c9754a",
        Name.Attribute: "#b8976c",
        Name.Decorator: "#d4896a",
        Number: "#b8976c",
        String: "#34d399",
        Error: "#f87171",
        Generic.Deleted: "#f87171",
        Generic.Inserted: "#34d399",
        Generic.Emph: "italic",
        Generic.Strong: "bold",
    }


def rewrite_links(text: str, src_dir: str, resolver: "LinkResolver") -> str:
    """Rewrite repo-relative link/image targets at the markdown-source level, fence by
    fence. Needed because raw-HTML blocks (the `<p align="center">` heroes) are stashed
    verbatim by python-markdown and never reach the tree processor, which stays
    registered as a backstop for ordinary `[text](target)` links."""
    external = ("#", "/", "http://", "https://", "mailto:")

    def sub(m: re.Match) -> str:
        target = m.group(2)
        if target.startswith(external):
            return m.group(0)
        try:
            return m.group(1) + resolver.resolve(src_dir, target) + m.group(3)
        except BuildError:
            raise

    def line_sub(line: str) -> str:
        parts = re.split(r"(`[^`]*`)", line)  # inline code spans are left alone
        for i, part in enumerate(parts):
            if part.startswith("`"):
                continue
            part = re.sub(r"(\]\()([^)\s]+)((?:\s[^)]*)?\))", sub, part)
            part = re.sub(r'((?:src|href)=")([^"]+)(")', sub, part)
            part = re.sub(r"^(\s*\[[^\]]+\]:\s*)(\S+)(.*)$", sub, part)  # reference defs
            parts[i] = part
        return "".join(parts)

    out, in_fence = [], False
    for line in text.split("\n"):
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
            out.append(line)
        elif in_fence:
            out.append(line)
        else:
            out.append(line_sub(line))
    return "\n".join(out)


def github_slugify(value: str, separator: str = "-") -> str:
    """Anchor ids that match GitHub's, so cross-document #links written for GitHub
    resolve here too."""
    value = re.sub(r"[^\w\- ]", "", value.strip().lower())
    return value.replace(" ", separator)


def wrap_math(text: str) -> str:
    """Put `$$...$$` display equations inside raw-HTML blocks so the markdown pass
    leaves the backslashes and underscores alone; KaTeX renders them client-side.
    Handles both one-line (`$$x$$`) and multi-line blocks. Fenced code is untouched."""
    out, in_fence, in_math, buf = [], False, False, []
    for line in text.split("\n"):
        if line.lstrip().startswith("```"):
            if in_math:  # unterminated block; give up gracefully
                out.append(f'<div class="math">{chr(10).join(buf)}</div>')
                in_math, buf = False, []
            in_fence = not in_fence
            out.append(line)
            continue
        if in_fence or in_math:
            if in_math:
                if line.lstrip().endswith("$$"):
                    buf.append(line.strip())
                    out.append(f'<div class="math">{chr(10).join(buf)}</div>')
                    out.append("")
                    in_math, buf = False, []
                else:
                    buf.append(line.strip())
            else:
                out.append(line)
            continue
        if line.lstrip().startswith("$$"):
            if line.rstrip().endswith("$$") and len(line.strip()) > 2:
                out.append("")
                out.append(f'<div class="math">{line.strip()}</div>')
                out.append("")
            else:
                in_math, buf = True, [line.strip()]
            continue
        out.append(line)
    if buf:
        out.append(f'<div class="math">{chr(10).join(buf)}</div>')
    return "\n".join(out)


class SiteLinksTreeprocessor(Treeprocessor):
    """Rewrite href/src on the rendered tree -- code samples are text nodes there, so
    this can never corrupt an example the way a regex on the markdown could."""

    def __init__(self, md, src_dir: str, resolver: "LinkResolver"):
        super().__init__(md)
        self.src_dir, self.resolver = src_dir, resolver

    def run(self, root):
        for el in root.iter():
            if el.tag == "a" and el.get("href"):
                el.set("href", self.resolver.resolve(self.src_dir, el.get("href")))
            elif el.tag == "img" and el.get("src"):
                el.set("src", self.resolver.resolve(self.src_dir, el.get("src")))
        return root


class SiteLinks(Extension):
    def __init__(self, src_dir: str, resolver: "LinkResolver"):
        super().__init__()
        self.src_dir, self.resolver = src_dir, resolver

    def extendMarkdown(self, md):
        md.treeprocessors.register(
            SiteLinksTreeprocessor(md, self.src_dir, self.resolver), "sitelinks", 15)


CODE_RE = re.compile(r'<pre><code(?: class="language-([\w#+.-]+)")?>(.*?)</code></pre>', re.S)


def highlight_code(page_html: str) -> str:
    def repl(m):
        lang, code = m.group(1), html_mod.unescape(m.group(2))
        try:
            lexer = get_lexer_by_name(lang or "text", stripnl=False)
        except ClassNotFound:
            return m.group(0)  # e.g. mermaid: keep it as plain text
        return pyg_highlight(code, lexer, FORMATTER)

    return CODE_RE.sub(repl, page_html)


FORMATTER = HtmlFormatter(style=InkvecCodeStyle, cssclass="codehilite")


# ------------------------------------------------------------------------------ links

# markdown path (repo-relative) -> site slug. The hand-rendered docs/algorithm/*.html
# pages (plain language, inline diagrams) are served verbatim under algorithm/plain/,
# so links written against them land there rather than on the generated pages.
SOURCE_TO_SITE = {src: slug for slug, (src, _) in DOCS.items()}
SOURCE_TO_SITE.update({
    "docs/algorithm": "algorithm/",
    "docs/results": "results.html",
    **{f"docs/algorithm/{s}.md": f"algorithm/{s}.html" for s, _ in STAGES},
    **{f"docs/algorithm/{s}.html": f"algorithm/plain/{s}.html" for s, _ in STAGES},
    "docs/algorithm/index.html": "algorithm/plain/index.html",
})


class LinkResolver:
    def __init__(self, base: str):
        self.base = base

    def resolve(self, src_dir: str, href: str) -> str:
        if href.startswith(("#", "/", "http://", "https://", "mailto:")):
            return href
        path, _, frag = href.partition("#")
        if not path:
            return href  # a fragment on this page
        if src_dir:
            path = posixpath.normpath(posixpath.join(src_dir, path))
        else:
            path = posixpath.normpath(path)
        frag = f"#{frag}" if frag else ""
        if path.startswith("docs/assets/"):
            return f"{self.base}assets/{path[len('docs/assets/'):]}{frag}"
        site = SOURCE_TO_SITE.get(path)
        if site is not None:
            return self.base + site + frag
        if (ROOT / path).exists():
            kind = "tree" if (ROOT / path).is_dir() else "blob"
            return f"{GITHUB}/{kind}/main/{path}{frag}"
        raise BuildError(f"dangling internal link {href!r} on {src_dir or '<root>'}")


# ----------------------------------------------------------------------------- chrome

CSS = """\
/* LogoLabs design system: warm charcoal ramp, one copper accent, Playfair Display +
 * Inter + IBM Plex Mono. Tokens match web/index.html and the design-system zip. */
:root{
  --paper:#1a1816;--card:#211f1c;--stage:#141210;--void:#0c0a09;--surface:#2a2724;--highlight:#35322e;
  --ink:#faf8f5;--dim:rgba(250,248,245,.75);--faint:rgba(250,248,245,.55);--muted:rgba(250,248,245,.4);
  --rule:rgba(250,248,245,.08);--rule2:#2a2724;
  --accent:#c9754a;--accent-hover:#d4896a;--accent-ink:#1a1816;--accent-soft:rgba(201,117,74,.12);
  --gold:#b8976c;--good:#34d399;--bad:#f87171;
  --font-display:"Playfair Display",Georgia,serif;--font-body:"Inter",-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;
  --font-mono:"IBM Plex Mono",ui-monospace,SFMono-Regular,Consolas,monospace;
}
*{box-sizing:border-box}
html{color-scheme:dark}
body{margin:0;background:var(--paper);color:var(--ink);font:16px/1.65 var(--font-body);
  -webkit-font-smoothing:antialiased;text-rendering:optimizeLegibility}
a{color:var(--accent);text-decoration:none}
a:hover{color:var(--accent-hover)}
::selection{background:rgba(201,117,74,.35)}

/* top bar */
.top{position:sticky;top:0;z-index:10;display:flex;align-items:center;justify-content:space-between;
  padding:0 20px;height:56px;background:rgba(26,24,22,.92);backdrop-filter:blur(8px);border-bottom:1px solid var(--rule)}
.brand{display:flex;align-items:center;gap:10px;color:var(--ink)}
.brand img{width:22px;height:22px}
.brand span{font-family:var(--font-display);font-weight:700;font-size:19px;letter-spacing:-.01em}
.toplinks{display:flex;gap:22px;font-size:13.5px;font-weight:500}
.toplinks a{color:var(--dim)}
.toplinks a:hover{color:var(--accent)}

/* shell: sidebar / article / toc rail */
.shell{display:grid;grid-template-columns:240px minmax(0,1fr) 200px;gap:0;max-width:1400px;margin:0 auto}
.side{border-right:1px solid var(--rule);padding:28px 18px 60px 24px;position:sticky;top:56px;
  height:calc(100vh - 56px);overflow-y:auto;scrollbar-width:thin}
.side .grp{margin:0 0 22px}
.side .grp h4{margin:0 0 8px;font:500 10.5px/1 var(--font-mono);letter-spacing:.14em;text-transform:uppercase;
  color:var(--muted)}
.side a{display:block;padding:4px 8px;margin:1px -8px;border-radius:6px;font-size:13.5px;color:var(--faint)}
.side a:hover{color:var(--ink);background:var(--card)}
.side a[aria-current]{color:var(--accent);background:var(--accent-soft)}
.side a .n{font:500 10.5px/1 var(--font-mono);color:var(--muted);margin-right:7px}
.side a[aria-current] .n{color:var(--accent)}

main{padding:44px 48px 90px;min-width:0}
article{max-width:46rem}
article h1,article h2,article h3,article h4{font-family:var(--font-display);font-weight:600;
  letter-spacing:-.01em;line-height:1.2;color:var(--ink);scroll-margin-top:76px}
article h1{font-size:2.1rem;margin:0 0 18px}
article h2{font-size:1.45rem;margin:2.2em 0 .6em;padding-top:.7em;border-top:1px solid var(--rule)}
article h3{font-size:1.12rem;margin:1.8em 0 .5em}
article p{margin:0 0 1em;color:var(--dim)}
article li{color:var(--dim);margin:.3em 0}
article strong{color:var(--ink);font-weight:600}
article hr{border:0;border-top:1px solid var(--rule);margin:2.5em 0}
article img{max-width:100%;border-radius:10px;height:auto}
article a{text-decoration:underline;text-decoration-color:rgba(201,117,74,.35);text-underline-offset:3px}
article a:hover{text-decoration-color:var(--accent)}
.md-anchor{position:relative;top:-70px}

code{font-family:var(--font-mono);font-size:.86em;background:var(--surface);
  border:1px solid var(--rule);border-radius:5px;padding:.1em .35em;color:var(--ink)}
.codehilite{background:var(--stage)!important;border:1px solid var(--rule);border-radius:10px;
  padding:14px 16px;overflow-x:auto;margin:0 0 1.2em}
.codehilite pre{margin:0;background:transparent!important}
.codehilite code{background:transparent;border:0;padding:0;font-size:13px;line-height:1.6}

table{border-collapse:collapse;width:100%;margin:0 0 1.2em;font-size:14px;display:block;overflow-x:auto}
th,td{border:1px solid var(--rule2);padding:7px 11px;text-align:left;vertical-align:top;color:var(--dim)}
th{background:var(--card);color:var(--ink);font-weight:600;font-size:13px}
tbody tr:nth-child(2n){background:rgba(250,248,245,.02)}
table code{white-space:nowrap}

blockquote{margin:0 0 1.2em;padding:10px 16px;border-left:3px solid var(--accent);
  background:var(--accent-soft);border-radius:0 8px 8px 0}
blockquote p{margin:0 0 .5em;color:var(--ink)}
blockquote p:last-child{margin:0}

.math{overflow-x:auto;overflow-y:hidden;padding:6px 2px;margin:0 0 1.2em}
.katex-display{margin:.4em 0!important}
.katex{color:var(--ink)}

/* per-page toc rail */
.rail{padding:52px 20px 60px 0;position:sticky;top:56px;height:calc(100vh - 56px);overflow-y:auto}
.rail h5{margin:0 0 10px;font:500 10.5px/1 var(--font-mono);letter-spacing:.14em;text-transform:uppercase;
  color:var(--muted)}
.rail ul{list-style:none;margin:0;padding:0}
.rail ul ul{padding-left:12px}
.rail a{display:block;padding:3px 0;font-size:12.5px;color:var(--muted);line-height:1.45}
.rail a:hover{color:var(--accent)}
.rail .l2>a{color:var(--faint)}

/* index hero */
.hero{padding:8px 0 34px;border-bottom:1px solid var(--rule);margin-bottom:34px}
.eyebrow{font:500 11px/1 var(--font-mono);letter-spacing:.22em;color:var(--accent);margin:0 0 14px}
.hero h1{font-family:var(--font-display);font-size:3.4rem;font-weight:700;letter-spacing:-.02em;margin:0 0 14px;line-height:1.05}
.hero .pitch{font-size:1.14rem;color:var(--faint);max-width:34rem;margin:0 0 24px}
.cta{display:flex;gap:12px;flex-wrap:wrap}
.btn{display:inline-block;padding:10px 20px;border-radius:999px;font-size:14px;font-weight:600;
  border:1px solid var(--rule2);color:var(--ink);background:var(--card)}
.btn:hover{border-color:var(--highlight);color:var(--ink)}
.btn.primary{background:var(--accent);border-color:var(--accent);color:var(--accent-ink)}
.btn.primary:hover{background:var(--accent-hover);border-color:var(--accent-hover);color:var(--accent-ink)}
.btn.ghost{background:transparent}

/* pointer to the diagram edition */
.plainlink{margin:-6px 0 1.6em;font-size:14px}
.plainlink a{font-weight:500}

/* about block on the landing page */
.about{margin-top:3.2em;padding-top:1.4em;border-top:1px solid var(--rule)}
.about p{color:var(--faint);max-width:38rem}

/* prev / next */
.pagenav{display:flex;justify-content:space-between;gap:14px;margin-top:3.2em;padding-top:1.2em;
  border-top:1px solid var(--rule)}
.pagenav a{flex:1;border:1px solid var(--rule2);background:var(--card);border-radius:10px;padding:11px 15px;
  text-decoration:none}
.pagenav a:hover{border-color:rgba(201,117,74,.45)}
.pagenav .dir{display:block;font:500 10.5px/1 var(--font-mono);letter-spacing:.12em;color:var(--muted);margin-bottom:4px}
.pagenav .lbl{color:var(--ink);font-weight:600;font-size:14px}
.pagenav .next{text-align:right}

footer{border-top:1px solid var(--rule);padding:26px 24px;display:flex;justify-content:space-between;
  gap:16px;flex-wrap:wrap;font-size:13px;color:var(--muted)}
footer a{color:var(--dim)}
footer a:hover{color:var(--accent)}

@media (max-width:1150px){.rail{display:none}.shell{grid-template-columns:220px minmax(0,1fr)}}
@media (max-width:820px){
  .shell{grid-template-columns:minmax(0,1fr)}
  .side{position:static;height:auto;border-right:0;border-bottom:1px solid var(--rule);padding:18px 24px}
  .side .grp{display:inline-block;vertical-align:top;margin:0 26px 14px 0}
  main{padding:30px 22px 70px}
  .hero h1{font-size:2.5rem}
}
"""

PAGE_SHELL = """\
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>__TITLE__</title>
<meta name="description" content="__DESC__">
<link rel="icon" type="image/svg+xml" href="__BASE__favicon.svg">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Playfair+Display:ital,wght@0,500;0,600;0,700;1,500&family=Inter:wght@400;500;600;700&family=IBM+Plex+Mono:wght@400;500&display=swap">
<link rel="stylesheet" href="__BASE__site.css">
__HEAD__
</head>
<body>
<header class="top">
  <a class="brand" href="__BASE__"><img src="__BASE__logo-mono.svg" alt=""> <span>Inkvec</span></a>
  <nav class="toplinks">
    <a href="__SPACE__">Try in the browser</a>
    <a href="__GITHUB__">GitHub</a>
  </nav>
</header>
<div class="shell">
  <aside class="side">__NAV__</aside>
  <main><article>__CONTENT__</article></main>
  <aside class="rail">__RAIL__</aside>
</div>
<footer>
  <span>Inkvec &middot; LogoLabs</span>
  <span>Released as part of LogoLabs' work on <a href="__LOGOLABS__">logo generation using AI</a></span>
  <span><a href="__GITHUB__/blob/main/LICENSE">Apache-2.0</a> &middot; <a href="__GITHUB__">source</a>
  &middot; <a href="__SPACE__">demo</a></span>
</footer>
</body>
</html>
"""

KATEX_HEAD = """\
<link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.css">
<script defer src="https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.js"></script>
<script defer src="https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/contrib/auto-render.min.js" onload="renderMathInElement(document.body,{delimiters:[{left:'$$',right:'$$',display:true},{left:'$',right:'$',display:false}]})"></script>"""


def render_nav(base: str, active: str) -> str:
    parts = []
    for group, items in NAV:
        rows = []
        for slug, label in items:
            current = ' aria-current="page"' if slug == active else ""
            m = re.match(r"^(\d\d) (.+)$", label)
            body = f'<span class="n">{m.group(1)}</span>{m.group(2)}' if m else label
            rows.append(f'<a href="{base}{slug}"{current}>{body}</a>')
        parts.append(f'<div class="grp"><h4>{group}</h4>{"".join(rows)}</div>')
    return "".join(parts)


def render_rail(toc_tokens: list) -> str:
    def walk(tokens):
        rows = []
        for t in tokens:
            if t["level"] < 2:
                rows.extend(walk(t["children"]))
                continue
            kids = walk(t["children"])
            cls = "l2" if t["level"] == 2 else "l3"
            rows.append(f'<li class="{cls}"><a href="#{t["id"]}">{t["name"]}</a>{kids}</li>')
        return f'<ul>{"".join(rows)}</ul>' if rows else ""

    inner = walk(toc_tokens)
    return f"<h5>On this page</h5>{inner}" if inner else ""


def rewrite_plain_hrefs(text: str, resolver: "LinkResolver") -> str:
    """The hand-rendered plain-language pages move from docs/algorithm/ to
    algorithm/plain/ on the site. Links between them stay relative; everything else
    (.md references, other docs) resolves through the site's link map."""
    stays = re.compile(r"(?:\d\d-[\w-]+|index)\.html(?:#[\w-]*)?$")

    def sub(m: re.Match) -> str:
        target = m.group(1)
        if target == "assets/doc.css" or stays.fullmatch(target):
            return m.group(0)
        return f'href="{resolver.resolve("docs/algorithm", target)}"'

    return re.sub(r'href="([^"]+)"', sub, text)


PLAIN_BANNER = (
    '<div style="display:flex;justify-content:space-between;gap:12px;flex-wrap:wrap;'
    'align-items:center;background:#1a1816;color:#faf8f5;padding:10px 20px;'
    'font:500 13px Inter,-apple-system,Segoe UI,sans-serif">'
    '<span><span style="color:#c9754a;font-family:\'IBM Plex Mono\',monospace;font-size:11px;'
    'letter-spacing:.12em">DIAGRAM EDITION</span>&nbsp; the stages in plain language</span>'
    '<a href="__BACK__" style="color:#c9754a">Reference edition &rarr;</a></div>')


_stage_meta_cache: dict = {}


def stage_meta(slug: str) -> tuple:
    """(number, title, one-line lede) parsed from the stage markdown's header block."""
    if slug in _stage_meta_cache:
        return _stage_meta_cache[slug]
    text = (ROOT / "docs/algorithm" / f"{slug}.md").read_text(encoding="utf-8")
    h1 = re.search(r"^# (.+)$", text, re.M)
    quote = re.search(r"^> (.+?)(?:\n\n|\n\*\*)", text, re.S | re.M)
    title = h1.group(1).strip() if h1 else slug
    m = re.match(r"^Stage (\d\d) — (.+)$", title)
    number = m.group(1) if m else ""
    short = m.group(2).strip() if m else title
    lede = ""
    if quote:
        lede = re.sub(r"\s+", " ", quote.group(1)).strip()
        cut = lede.find("— ", 10)
        if 0 < cut < 150:
            lede = lede[:cut].rstrip(" -—,")
        if len(lede) > 140:
            lede = lede[:137].rstrip() + "…"
    out = (number, short, lede)
    _stage_meta_cache[slug] = out
    return out


def first_paragraph(html: str, limit: int = 165) -> str:
    m = re.search(r"<p>(.*?)</p>", html, re.S)
    if not m:
        return "Inkvec documentation"
    text = re.sub(r"<[^>]+>", "", m.group(1))
    text = re.sub(r"\s+", " ", text).strip()
    return text[:limit].rstrip() + ("…" if len(text) > limit else "")


# ------------------------------------------------------------------------------ build

def strip_readme_chrome(body: str) -> str:
    """The README opens with the GitHub hero image and the badge row; the site hero
    replaces them."""
    return re.sub(r"<p[^>]*>(?:(?!</p>).)*?(?:img\.shields\.io|github-hero\.png)(?:(?!</p>).)*?</p>",
                  "", body, flags=re.S)


def convert_page(slug: str, src: str, resolver: LinkResolver) -> tuple:
    """-> (body_html, toc_tokens, title)"""
    path = ROOT / src
    text = path.read_text(encoding="utf-8")
    src_dir = posixpath.dirname(src)
    text = rewrite_links(text, src_dir, resolver)
    md = markdown.Markdown(
        extensions=[FencedCodeExtension(), TableExtension(), SiteLinks(src_dir, resolver),
                    TocExtension(slugify=github_slugify, toc_depth="1-3")],
        output_format="html5")
    body = md.convert(wrap_math(text))
    body = highlight_code(body)
    h1 = re.search(r"<h1[^>]*>(.*?)</h1>", body, re.S)
    title = re.sub(r"<[^>]+>", "", h1.group(1)).strip() if h1 else DOCS[slug][1]
    return body, md.toc_tokens, title


def write_page(out: Path, base: str, slug: str, title: str, body: str,
               toc_tokens: list, extra_head: str = "", pagenav: str = ""):
    doc = (PAGE_SHELL
           .replace("__TITLE__", f"{html_mod.escape(title)} · Inkvec")
           .replace("__DESC__", html_mod.escape(first_paragraph(body)))
           .replace("__BASE__", base)
           .replace("__SPACE__", SPACE)
           .replace("__GITHUB__", GITHUB)
           .replace("__LOGOLABS__", LOGOLABS)
           .replace("__HEAD__", extra_head)
           .replace("__NAV__", render_nav(base, slug))
           .replace("__RAIL__", render_rail(toc_tokens))
           .replace("__CONTENT__", body + pagenav))
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(doc, encoding="utf-8")


def build(base: str) -> int:
    errors: list[str] = []
    resolver = LinkResolver(base)

    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    assets = OUT / "assets"
    shutil.copytree(ROOT / "docs/assets", assets)
    # The app mark, in its two colours, is the favicon; the sidebar topbar carries the
    # mono variant recoloured to the accent.
    shutil.copyfile(ROOT / "web/favicon.svg", OUT / "favicon.svg")
    logo = (ROOT / "web/logo.svg").read_text(encoding="utf-8")
    (OUT / "logo-mono.svg").write_text(
        logo.replace("#1d1c28", "#c9754a").replace("#08F4FB", "#c9754a")
            .replace("#1d1c28".upper(), "#c9754a"),
        encoding="utf-8")
    (OUT / "site.css").write_text(CSS + "\n" + FORMATTER.get_style_defs(".codehilite") + "\n",
                                  encoding="utf-8")

    pages = dict(DOCS)
    for slug, _ in STAGES:
        pages[f"algorithm/{slug}.html"] = (f"docs/algorithm/{slug}.md", _)

    written = []
    for slug in sorted(pages):
        src, _label = pages[slug]
        try:
            body, toc_tokens, title = convert_page(slug, src, resolver)
        except BuildError as e:
            errors.append(str(e))
            continue
        extra = KATEX_HEAD if '<div class="math">' in body else ""
        if slug == "":
            extra += ('<script type="application/ld+json">' + json.dumps({
                "@context": "https://schema.org",
                "@type": "SoftwareSourceCode",
                "name": "Inkvec",
                "url": "https://logolabs.github.io/inkvec/",
                "codeRepository": GITHUB,
                "license": "https://www.apache.org/licenses/LICENSE-2.0",
                "applicationCategory": "DesignApplication",
                "operatingSystem": "Cross-platform",
                "description": "Inkvec converts raster logos and icons into exact, "
                               "editable SVG: boundaries decided by the evidence in the "
                               "pixels, primitives recognised as primitives, real "
                               "gradients, path count chosen by minimum description length.",
                "author": {"@type": "Organization", "name": "LogoLabs", "url": LOGOLABS},
            }, separators=(",", ":")) + '</script>')
        pagenav = ""
        if slug.startswith("algorithm/") and re.fullmatch(r"algorithm/(\d\d-[\w-]+)\.html", slug):
            stage = slug.split("/")[1][:-5]
            body = (f'<p class="plainlink">This stage also exists as a '
                    f'<a href="{base}algorithm/plain/{stage}.html">plain-language page '
                    f'with diagrams</a>.</p>' + body)
            idx = [s for s, _ in STAGES].index(stage)
            prev_s = STAGES[idx - 1][0] if idx > 0 else None
            next_s = STAGES[idx + 1][0] if idx + 1 < len(STAGES) else None
            def cell(direction, s):
                if not s:
                    return '<span></span>'
                lbl = stage_meta(s)[1]
                return (f'<a class="{direction}" href="{base}algorithm/{s}.html">'
                        f'<span class="dir">{"Previous" if direction == "prev" else "Next"}</span>'
                        f'<span class="lbl">{lbl}</span></a>')
            pagenav = f'<nav class="pagenav">{cell("prev", prev_s)}{cell("next", next_s)}</nav>'
        if slug == "":
            body = strip_readme_chrome(body)
            hero = (f'<section class="hero"><p class="eyebrow">LOGOLABS</p>'
                    f'<h1>Inkvec</h1>'
                    f'<p class="pitch">Exact SVG from logos and icons. The geometry is decided '
                    f'by the evidence in the pixels &mdash; not by a tolerance slider.</p>'
                    f'<div class="cta"><a class="btn primary" href="{SPACE}">Try it in the browser</a>'
                    f'<a class="btn" href="{base}algorithm/">How it works</a>'
                    f'<a class="btn ghost" href="{GITHUB}/releases">Download</a></div></section>')
            body = hero + body + (
                f'<section class="about"><h2>About</h2>'
                f'<p>Inkvec is open source under Apache-2.0, released as part of '
                f'LogoLabs&rsquo; work on <a href="{LOGOLABS}">logo generation using AI</a>. '
                f'The studio&rsquo;s home is <a href="{LOGOLABS}">logolabs.org</a>.</p></section>')
        write_page(OUT / (slug or "index.html"), base, slug, title, body, toc_tokens,
                   extra_head=extra, pagenav=pagenav)
        written.append(slug)

    print(f"{len(written)} pages -> {OUT} (base {base})")

    # The plain-language edition: the hand-rendered pages served verbatim (they carry
    # their own light stylesheet and inline diagrams), with links routed and a small
    # dark banner marking them as part of the site.
    plain_out = OUT / "algorithm/plain"
    (plain_out / "assets").mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / "docs/algorithm/assets/doc.css", plain_out / "assets/doc.css")
    for name in ["index.html"] + [f"{s}.html" for s, _ in STAGES]:
        text = (ROOT / "docs/algorithm" / name).read_text(encoding="utf-8")
        text = rewrite_plain_hrefs(text, resolver)
        back = "../index.html" if name == "index.html" else f"../{name}"
        banner = PLAIN_BANNER.replace("__BACK__", back)
        text = re.sub(r"(<body[^>]*>)", lambda m: m.group(1) + banner, text, count=1)
        (plain_out / name).write_text(text, encoding="utf-8")
    print(f"  algorithm/plain/: {len(STAGES) + 1} pages + doc.css (diagram edition)")

    (OUT / "404.html").write_text(
        PAGE_SHELL
        .replace("__TITLE__", "Not found · Inkvec")
        .replace("__DESC__", "Page not found")
        .replace("__BASE__", base)
        .replace("__SPACE__", SPACE)
        .replace("__GITHUB__", GITHUB)
        .replace("__HEAD__", "")
        .replace("__NAV__", render_nav(base, ""))
        .replace("__RAIL__", "")
        .replace("__CONTENT__",
                 '<h1>Not found</h1><p>That page does not exist. The '
                 f'<a href="{base}">documentation index</a> does.</p>'),
        encoding="utf-8")
    for slug in written:
        print(f"  {slug or 'index.html'}")
    if errors:
        print("\nbuild errors:", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        return 1
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", default=DEFAULT_BASE,
                    help=f"URL prefix for every internal link (default {DEFAULT_BASE!r}; "
                         "use / to preview locally)")
    args = ap.parse_args()
    base = args.base if args.base.endswith("/") else args.base + "/"
    return build(base)


if __name__ == "__main__":
    raise SystemExit(main())
