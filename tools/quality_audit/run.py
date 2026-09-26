"""Local code-quality audit for the Inkvec workspace: one command, every tool, one folder.

This is the "SonarQube for Rust, locally" layer. It does not replace the ratchet in
bench/quality.py (which gates CI on a handful of counts) or the fidelity gate in
bench/ci_gate.py; it runs the heavier analysers those deliberately leave out and writes
their raw output plus a digest:

    complexity   rust-code-analysis-cli: per-function cognitive/cyclomatic complexity,
                 SLOC, Halstead, maintainability index
    modules      cargo-modules: module graphs of the engine crates, collapsed to
                 module -> module `use` edges, with cycles (SCCs) and fan-in/out
    crates       cargo metadata: the crate graph and its layering
    smells       a grep for engine-layer smells (env vars, printing, file I/O, exit)
    coverage     cargo-llvm-cov: line/region coverage per crate and file (+ LCOV)
    machete      cargo-machete: unused dependencies
    deny         cargo-deny: licences, duplicate versions, advisories, sources
    bloat        cargo-bloat: biggest crates and functions in the release `inkvec` binary
    clippy       clippy with pedantic + nursery, as a census (not a gate)
    dup          jscpd over Rust and TypeScript
    sonar        converts everything above into SonarQube's generic issue format / SARIF
                 and an LCOV with repo-relative paths (see sonarqube.md)

Opt-in (slow, or needing extra installs):

    --udeps      cargo +nightly udeps
    --mutants    cargo-mutants on a sampled set of engine files
    --profile    release CLI under samply (falls back to wall-clock timings)
    --codeql     CodeQL Rust suites -> SARIF (needs the CodeQL CLI; space-free paths)
    --semgrep    Semgrep default Rust rules + tools/quality_audit/semgrep/ -> SARIF

    python tools/quality_audit/run.py --out M:/.../out/quality-audit-2026-09-26
    python tools/quality_audit/run.py --steps complexity,modules --out /tmp/qa
    python tools/quality_audit/run.py --mutants --mutants-shard 0/16

Every step writes raw/<step>... and a section of summary.json; SUMMARY.md is regenerated
from summary.json at the end, so a partial rerun keeps the other steps' numbers.
"""

from __future__ import annotations

import argparse
import collections
import csv
import datetime as _dt
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))

import sonar_export  # noqa: E402  (sibling module)

CHEAP_STEPS = [
    "complexity", "modules", "crates", "smells", "coverage", "machete", "deny", "bloat",
    "clippy", "dup",
]
MODULE_CRATES = ["inkvec-trace", "inkvec-cli", "inkvec-fit", "inkvec-svgmin", "inkvec-fab"]
RCA_PATHS = ["crates", "studio/core/src", "studio/src-tauri/src", "studio/wasm/src"]
DEFAULT_MUTANT_FILES = [
    "crates/inkvec-trace/src/color.rs",
    "crates/inkvec-trace/src/native.rs",
    "crates/inkvec-trace/src/boundary_opt.rs",
    "crates/inkvec-trace/src/gradient.rs",
    "crates/inkvec-fit/src/multimodel.rs",
]
# Sonar's own threshold for rust:S3776 is 15; 25 is where a function stops being readable
# in one sitting. Both are reported; the generic-issue export uses --cognitive-threshold.
COGNITIVE_WARN = 15
COGNITIVE_BAD = 25
# Every tool runs below normal priority: the audit shares the machine with GPU work and
# other builds, and a mutation run or CodeQL extraction will otherwise take every core.
# Children (cargo -> rustc -> link) inherit the class.
LOW_PRIORITY = getattr(subprocess, "BELOW_NORMAL_PRIORITY_CLASS", 0)


# ------------------------------------------------------------------ process helpers

def tool_env(args) -> dict:
    env = dict(os.environ)
    cargo_bin = str(Path.home() / ".cargo" / "bin")
    env["PATH"] = cargo_bin + os.pathsep + env.get("PATH", "")
    env["CARGO_TARGET_DIR"] = str(args.target_dir)
    env.setdefault("CARGO_BUILD_JOBS", str(args.jobs))
    return env


def run(cmd, args, *, cwd=ROOT, env=None, log: Path | None = None, timeout=None,
        check=False) -> subprocess.CompletedProcess:
    """Run a command, capture stdout/stderr as text, optionally tee both into `log`."""
    env = env or tool_env(args)
    t0 = time.time()
    exe = shutil.which(cmd[0], path=env["PATH"]) or cmd[0]
    try:
        p = subprocess.run([exe, *cmd[1:]], cwd=cwd, env=env, capture_output=True,
                           text=True, encoding="utf-8", errors="replace", timeout=timeout,
                           creationflags=LOW_PRIORITY)
    except FileNotFoundError:
        p = subprocess.CompletedProcess(cmd, 127, "", f"not found: {cmd[0]}")
    except subprocess.TimeoutExpired as e:
        p = subprocess.CompletedProcess(cmd, 124, e.stdout or "", f"timeout after {timeout}s")
    dt = time.time() - t0
    if log is not None:
        log.parent.mkdir(parents=True, exist_ok=True)
        log.write_text(f"$ {' '.join(map(str, cmd))}\n# exit {p.returncode} in {dt:.1f}s\n"
                       f"--- stdout\n{p.stdout}\n--- stderr\n{p.stderr}", encoding="utf-8")
    print(f"  [{p.returncode}] {dt:6.1f}s  {' '.join(map(str, cmd))[:140]}", flush=True)
    if check and p.returncode != 0:
        raise RuntimeError(f"{cmd[0]} failed ({p.returncode}): {p.stderr[-2000:]}")
    return p


def rel(path: str | Path) -> str:
    """Repo-relative forward-slash path, or the input if it is outside the repo."""
    s = str(path).replace("\\", "/")
    root = str(ROOT).replace("\\", "/")
    if s.lower().startswith(root.lower() + "/"):
        return s[len(root) + 1:]
    i = s.find("/crates/")
    if i >= 0:
        return s[i + 1:]
    i = s.find("/studio/")
    return s[i + 1:] if i >= 0 else s


def crate_of(relpath: str) -> str:
    parts = relpath.split("/")
    if parts[0] == "crates" and len(parts) > 1:
        return parts[1]
    if parts[0] == "studio" and len(parts) > 1:
        return "studio/" + parts[1]
    return parts[0]


def is_test_path(relpath: str) -> bool:
    return any(f"/{d}/" in "/" + relpath for d in ("tests", "examples", "benches"))


CFG_TEST = re.compile(r"#\[cfg\(test\)\]\s*\n\s*(pub(\([^)]*\))?\s+)?mod\s+\w+", re.M)


def test_mod_start(text: str) -> int | None:
    """1-based line of the first `#[cfg(test)] mod`, or None."""
    m = CFG_TEST.search(text)
    return text.count("\n", 0, m.start()) + 1 if m else None


def write_json(path: Path, obj) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=1, sort_keys=False), encoding="utf-8")


def md_table(rows, headers) -> str:
    out = ["| " + " | ".join(headers) + " |", "|" + "---|" * len(headers)]
    for r in rows:
        out.append("| " + " | ".join(str(c) for c in r) + " |")
    return "\n".join(out)


# ------------------------------------------------------------------ complexity

def step_complexity(args, raw: Path) -> dict:
    dst = raw / "rca"
    if dst.exists():
        shutil.rmtree(dst)
    dst.mkdir(parents=True)
    cmd =["rust-code-analysis-cli", "-m", "-O", "json", "-o", str(dst), "-j", str(args.jobs),
           "-I", "*.rs", "-X", "*/target/*"]
    for p in RCA_PATHS:
        cmd += ["-p", p]
    run(cmd, args, log=raw / "logs" / "rca.log")
    funcs, files = [], []
    for jf in sorted(dst.rglob("*.json")):
        d = json.loads(jf.read_text(encoding="utf-8"))
        path = rel(d["name"])
        src = ROOT / path
        text = src.read_text(encoding="utf-8", errors="replace") if src.exists() else ""
        tstart = test_mod_start(text)
        testfile = is_test_path(path)
        m = d["metrics"]
        files.append({
            "file": path, "crate": crate_of(path), "test": testfile,
            "sloc": m["loc"]["sloc"], "ploc": m["loc"]["ploc"], "cloc": m["loc"]["cloc"],
            "mi": round(m["mi"]["mi_visual_studio"], 1),
            "cognitive": m["cognitive"]["sum"], "cyclomatic": m["cyclomatic"]["sum"],
            "functions": m["nom"]["functions"], "halstead_bugs": round(m["halstead"].get("bugs") or 0, 2),
        })

        def walk(space, scope):
            for s in space.get("spaces", []):
                kind = s["kind"]
                name = s.get("name") or "?"
                if kind == "function":
                    sm = s["metrics"]
                    funcs.append({
                        "file": path, "crate": crate_of(path),
                        "name": "::".join(scope + [name]), "line": s["start_line"],
                        "end": s["end_line"],
                        "test": testfile or (tstart is not None and s["start_line"] > tstart),
                        "cognitive": sm["cognitive"]["sum"], "cyclomatic": sm["cyclomatic"]["sum"],
                        "sloc": sm["loc"]["sloc"], "nargs": sm["nargs"].get("total_functions", 0),
                        "nexits": sm["nexits"]["sum"],
                        "mi": round(sm["mi"]["mi_visual_studio"], 1),
                        "halstead_effort": round(sm["halstead"].get("effort") or 0),
                    })
                    walk(s, scope + [name])
                elif kind in ("impl", "trait"):
                    walk(s, scope + [name.replace("impl ", "")])
                else:
                    walk(s, scope)

        walk(d, [])
    with (raw / "complexity_functions.csv").open("w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=list(funcs[0].keys()))
        w.writeheader()
        w.writerows(sorted(funcs, key=lambda f: -f["cognitive"]))
    with (raw / "complexity_files.csv").open("w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=list(files[0].keys()))
        w.writeheader()
        w.writerows(sorted(files, key=lambda f: f["mi"]))
    prod = [f for f in funcs if not f["test"]]
    per_crate = {}
    for c in sorted({f["crate"] for f in prod}):
        cs = sorted(f["cognitive"] for f in prod if f["crate"] == c)
        per_crate[c] = {
            "functions": len(cs),
            "sloc": sum(f["sloc"] for f in files if f["crate"] == c and not f["test"]),
            "cognitive_total": sum(cs),
            "p90": cs[int(0.9 * (len(cs) - 1))] if cs else 0,
            "max": cs[-1] if cs else 0,
            f"over_{COGNITIVE_WARN}": sum(1 for x in cs if x > COGNITIVE_WARN),
            f"over_{COGNITIVE_BAD}": sum(1 for x in cs if x > COGNITIVE_BAD),
        }
    top_cog = sorted(prod, key=lambda f: -f["cognitive"])[:30]
    top_cyc = sorted(prod, key=lambda f: -f["cyclomatic"])[:15]
    worst_mi = sorted([f for f in files if not f["test"] and f["sloc"] >= 150],
                      key=lambda f: f["mi"])[:20]
    return {
        "functions": len(prod), "test_functions": len(funcs) - len(prod), "files": len(files),
        "over_warn": sum(1 for f in prod if f["cognitive"] > COGNITIVE_WARN),
        "over_bad": sum(1 for f in prod if f["cognitive"] > COGNITIVE_BAD),
        "per_crate": per_crate,
        "top_cognitive": [{k: f[k] for k in ("file", "name", "line", "cognitive", "cyclomatic", "sloc", "nargs")} for f in top_cog],
        "top_cyclomatic": [{k: f[k] for k in ("file", "name", "line", "cognitive", "cyclomatic", "sloc")} for f in top_cyc],
        "worst_mi_files": [{k: f[k] for k in ("file", "sloc", "mi", "cognitive", "functions")} for f in worst_mi],
    }


# ------------------------------------------------------------------ modules

NODE_RE = re.compile(r'^\s*"([^"]+)" \[label="([^"|]*)\|([^"]*)".*// "([^"]+)" node')
EDGE_RE = re.compile(r'^\s*"([^"]+)" -> "([^"]+)" \[label="(owns|uses)"')


def parse_modules_dot(text: str):
    nodes, owns, uses = {}, {}, []
    for line in text.splitlines():
        m = NODE_RE.match(line)
        if m:
            nodes[m.group(1)] = m.group(4)
            continue
        m = EDGE_RE.match(line)
        if m:
            a, b, kind = m.groups()
            if kind == "owns":
                owns[b] = a
            else:
                uses.append((a, b))
    return nodes, owns, uses


def tarjan(graph: dict[str, set[str]]) -> list[list[str]]:
    index, low, stack, on, out = {}, {}, [], set(), []
    counter = [0]
    sys.setrecursionlimit(10000)

    def strong(v):
        index[v] = low[v] = counter[0]
        counter[0] += 1
        stack.append(v)
        on.add(v)
        for w in graph.get(v, ()):
            if w not in index:
                strong(w)
                low[v] = min(low[v], low[w])
            elif w in on:
                low[v] = min(low[v], index[w])
        if low[v] == index[v]:
            comp = []
            while True:
                w = stack.pop()
                on.discard(w)
                comp.append(w)
                if w == v:
                    break
            if len(comp) > 1:
                out.append(sorted(comp))

    for v in list(graph):
        if v not in index:
            strong(v)
    return out


def collapse(nodes, owns, uses, depth: int | None):
    """module -> module edges; `depth` 1 = top-level modules, None = each item's own module."""
    root = next((n for n, k in nodes.items() if k == "crate"), None)

    def module_of(n):
        while n in nodes and nodes[n] not in ("mod", "crate"):
            n = owns.get(n, root)
        return n

    def at_depth(mod):
        if depth is None or mod == root:
            return mod
        parts = mod.split("::")
        return "::".join(parts[: depth + 1])

    g: dict[str, set[str]] = collections.defaultdict(set)
    weight = collections.Counter()
    for a, b in uses:
        ma, mb = at_depth(module_of(a)), at_depth(module_of(b))
        if ma and mb and ma != mb:
            g[ma].add(mb)
            weight[(ma, mb)] += 1
    for n, k in nodes.items():
        if k in ("mod", "crate"):
            g.setdefault(at_depth(n), set())
    return root, g, weight


def render_dot(dot_path: Path, args) -> str | None:
    svg = dot_path.with_suffix(".svg")
    if shutil.which("dot"):
        p = run(["dot", "-Tsvg", str(dot_path), "-o", str(svg)], args)
        return svg.name if p.returncode == 0 else None
    # No Graphviz install: the WebAssembly build of Graphviz from npm renders the same DOT.
    # (`npx -p ... dot-wasm` does not resolve the bin on Windows; the package's bin is
    # `wasm-graphviz-cli`.)
    npx = shutil.which("npx")
    if npx and not args.no_npx:
        p = run(["npx", "--yes", "-p", "@hpcc-js/wasm-graphviz-cli", "wasm-graphviz-cli",
                 "-K", "dot", "-T", "svg", str(dot_path)], args, timeout=600)
        if p.returncode == 0 and p.stdout.lstrip().startswith("<"):
            svg.write_text(p.stdout, encoding="utf-8")
            return svg.name
    return None


def step_modules(args, raw: Path) -> dict:
    out = raw / "modules"
    out.mkdir(parents=True, exist_ok=True)
    res = {}
    for pkg in MODULE_CRATES:
        p = run(["cargo", "modules", "dependencies", "-p", pkg, "--lib", "--no-externs",
                 "--no-sysroot", "--layout", "dot"], args, log=out / f"{pkg}.log")
        if p.returncode != 0:
            res[pkg] = {"error": p.stderr[-500:]}
            continue
        (out / f"{pkg}_items.dot").write_text(p.stdout, encoding="utf-8")
        s = run(["cargo", "modules", "structure", "-p", pkg, "--lib"], args)
        (out / f"{pkg}_structure.txt").write_text(s.stdout, encoding="utf-8")
        o = run(["cargo", "modules", "orphans", "-p", pkg, "--lib"], args)
        nodes, owns, uses = parse_modules_dot(p.stdout)
        root, top, weight = collapse(nodes, owns, uses, depth=1)
        _, full, _ = collapse(nodes, owns, uses, depth=None)
        short = lambda n: n.split("::", 1)[1] if "::" in n else "(crate root)"
        # Cycles with and without the crate root: lib.rs is both the facade that calls
        # every stage and, in this repo, the home of shared types the stages import back.
        scc_top = tarjan(top)
        no_root = {a: {b for b in bs if b != root} for a, bs in top.items() if a != root}
        scc_top_noroot = tarjan(no_root)
        scc_full = tarjan(full)
        fan_in = collections.Counter(b for bs in top.values() for b in bs)
        dot = ["digraph modules {", '  rankdir=LR; node [shape=box, fontname="Inter", fontsize=10];']
        cyc_nodes = {n for c in scc_top_noroot for n in c}
        for n in top:
            color = ', style=filled, fillcolor="#f3d9cb"' if n in cyc_nodes else ""
            dot.append(f'  "{short(n)}" [label="{short(n)}"{color}];')
        for a, bs in top.items():
            for b in bs:
                w = weight[(a, b)]
                dot.append(f'  "{short(a)}" -> "{short(b)}" [label="{w}", penwidth={1 + min(w, 20) / 5:.1f}];')
        dot.append("}")
        dpath = out / f"{pkg}_modules.dot"
        dpath.write_text("\n".join(dot), encoding="utf-8")
        svg = render_dot(dpath, args)
        res[pkg] = {
            "modules": sum(1 for k in nodes.values() if k == "mod"),
            "items": len(nodes), "use_edges": len(uses),
            "top_edges": sum(len(v) for v in top.values()),
            "cycles_top": [[short(n) for n in c] for c in scc_top],
            "cycles_top_without_root": [[short(n) for n in c] for c in scc_top_noroot],
            "cycles_full": [[short(n) for n in c] for c in scc_full],
            "fan_out": {short(a): len(bs) for a, bs in sorted(top.items(), key=lambda kv: -len(kv[1]))[:10]},
            "fan_in": {short(a): n for a, n in fan_in.most_common(10)},
            "root_imported_by": sorted(short(a) for a, bs in top.items() if root in bs),
            "orphans": "No orphans" not in (o.stdout + o.stderr),
            "svg": svg,
        }
    return res


# ------------------------------------------------------------------ crate graph

# The intended layering, bottom to top. A normal (non-dev) dependency must point to a
# strictly lower layer; same-layer and upward edges are reported.
LAYERS = {
    "inkvec-core": 0,
    "inkvec-fit": 1,
    "inkvec-trace": 2, "inkvec-svgmin": 2, "inkvec-sr": 2,
    "inkvec-restore": 3, "inkvec-fab": 3,
    "inkvec-cli": 4,
    "inkvec": 5,
    "inkvec-ffi": 6, "inkvec-py": 6, "inkvec-server": 6, "inkvec-wasm": 6,
    "inkvec-studio-core": 7,
    "inkvec-studio": 8, "inkvec-studio-wasm": 8,
}


def step_crates(args, raw: Path) -> dict:
    pkgs = {}
    for manifest in ["Cargo.toml", "studio/src-tauri/Cargo.toml"]:
        p = run(["cargo", "metadata", "--format-version", "1", "--no-deps", "--manifest-path",
                 manifest], args)
        if p.returncode != 0:
            continue
        for pk in json.loads(p.stdout)["packages"]:
            pkgs[pk["name"]] = pk
    names = set(pkgs)
    edges = []
    for n, pk in pkgs.items():
        for d in pk["dependencies"]:
            if d["name"] in names:
                edges.append({"from": n, "to": d["name"], "kind": d["kind"] or "normal",
                              "optional": d["optional"]})
    viol = [e for e in edges if e["kind"] == "normal" and e["from"] in LAYERS and e["to"] in LAYERS
            and LAYERS[e["to"]] >= LAYERS[e["from"]]]
    dot = ["digraph crates {", '  rankdir=BT; node [shape=box, fontname="Inter"];']
    for n in sorted(names):
        dot.append(f'  "{n}" [label="{n}\\nL{LAYERS.get(n, "?")}"];')
    for e in edges:
        style = "dashed" if e["kind"] != "normal" else "solid"
        color = "#c9754a" if e in viol else "#333333"
        dot.append(f'  "{e["from"]}" -> "{e["to"]}" [style={style}, color="{color}"];')
    dot.append("}")
    (raw / "crates.dot").write_text("\n".join(dot), encoding="utf-8")
    svg = render_dot(raw / "crates.dot", args)
    ext = {n: sorted(d["name"] for d in pk["dependencies"] if d["name"] not in names and d["kind"] is None)
           for n, pk in pkgs.items()}
    dependents = collections.Counter(e["to"] for e in edges if e["kind"] == "normal")
    return {
        "crates": sorted(names), "edges": edges, "layer_violations": viol,
        "dependents": dict(dependents.most_common()), "external_deps": ext, "svg": svg,
    }


# ------------------------------------------------------------------ engine-layer smells

SMELLS = {
    "env_var": re.compile(r"\benv::var(_os)?\s*\("),
    "print": re.compile(r"\b(e?println|e?print|dbg)!\s*\("),
    "fs_io": re.compile(r"\bstd::fs::|\bfs::(read|write|create_dir|File)"),
    "process_exit": re.compile(r"\bprocess::exit\s*\("),
    "unwrap": re.compile(r"\.unwrap\(\)"),
    "expect": re.compile(r"\.expect\("),
    "unsafe": re.compile(r"\bunsafe\s*\{"),
    "allow_attr": re.compile(r"#\[allow\("),
}


def step_smells(args, raw: Path) -> dict:
    counts = collections.defaultdict(lambda: collections.Counter())
    where = collections.defaultdict(list)
    for p in sorted((ROOT / "crates").rglob("*.rs")):
        r = rel(p)
        if "/target/" in r or is_test_path(r) or r.endswith("/main.rs"):
            continue
        text = p.read_text(encoding="utf-8", errors="replace")
        tstart = test_mod_start(text)
        for i, line in enumerate(text.splitlines(), 1):
            if tstart and i >= tstart:
                break
            s = line.strip()
            if s.startswith("//"):
                continue
            for k, rx in SMELLS.items():
                if rx.search(s):
                    counts[crate_of(r)][k] += 1
                    if k in ("env_var", "print", "fs_io", "process_exit") and crate_of(r) in (
                            "inkvec-trace", "inkvec-fit", "inkvec-core", "inkvec-svgmin"):
                        where[k].append(f"{r}:{i}: {s[:110]}")
    env_names = collections.Counter()
    for p in (ROOT / "crates").rglob("*.rs"):
        for m in re.finditer(r'"(INKVEC_[A-Z0-9_]+|SVGIFY_[A-Z0-9_]+)"', p.read_text(encoding="utf-8", errors="replace")):
            env_names[m.group(1)] += 1
    (raw / "smells_engine_locations.txt").write_text(
        "\n".join(f"[{k}] {x}" for k, v in where.items() for x in v), encoding="utf-8")
    return {"per_crate": {c: dict(v) for c, v in sorted(counts.items())},
            "engine_locations": {k: len(v) for k, v in where.items()},
            "env_var_names": len(env_names), "env_var_top": dict(env_names.most_common(15))}


# ------------------------------------------------------------------ coverage

def step_coverage(args, raw: Path) -> dict:
    out = raw / "coverage"
    out.mkdir(parents=True, exist_ok=True)
    env = tool_env(args)
    # Coverage instruments every crate; fat LTO on top only makes the link slower.
    env.update(CARGO_PROFILE_RELEASE_LTO="false", CARGO_PROFILE_RELEASE_CODEGEN_UNITS="16")
    if not args.reuse_coverage or not (out / "summary.json").exists():
        run(["cargo", "llvm-cov", "--workspace", "--release", "--no-report", "--no-fail-fast"],
            args, env=env, log=out / "test.log", timeout=7200)
        run(["cargo", "llvm-cov", "report", "--release", "--json", "--summary-only",
             "--output-path", str(out / "summary.json")], args, env=env)
        run(["cargo", "llvm-cov", "report", "--release", "--lcov", "--output-path",
             str(out / "lcov.info")], args, env=env)
    if not (out / "summary.json").exists():
        return {"error": "no coverage summary produced; see raw/coverage/test.log"}
    data = json.loads((out / "summary.json").read_text(encoding="utf-8"))["data"][0]
    per = collections.defaultdict(lambda: [0, 0, 0, 0, 0, 0])
    rows = []
    for f in data["files"]:
        r = rel(f["filename"])
        if not r.startswith("crates/"):
            continue
        s = f["summary"]
        c = per[crate_of(r)]
        c[0] += s["lines"]["covered"]; c[1] += s["lines"]["count"]
        c[2] += s["regions"]["covered"]; c[3] += s["regions"]["count"]
        c[4] += s["functions"]["covered"]; c[5] += s["functions"]["count"]
        rows.append({"file": r, "lines": s["lines"]["count"],
                     "line_pct": round(s["lines"]["percent"], 1),
                     "region_pct": round(s["regions"]["percent"], 1)})
    pct = lambda a, b: round(100.0 * a / b, 1) if b else None
    crates = {k: {"lines": v[1], "line_pct": pct(v[0], v[1]), "region_pct": pct(v[2], v[3]),
                  "fn_pct": pct(v[4], v[5])} for k, v in sorted(per.items())}
    engine = [r for r in rows if r["file"].startswith(("crates/inkvec-trace/src", "crates/inkvec-fit/src",
                                                        "crates/inkvec-cli/src")) and r["lines"] >= 100]
    t = data["totals"]
    return {
        "total_line_pct": round(t["lines"]["percent"], 1),
        "total_region_pct": round(t["regions"]["percent"], 1),
        "per_crate": crates,
        "low_engine_files": sorted(engine, key=lambda r: r["line_pct"])[:25],
        "lcov": str(out / "lcov.info"),
    }


# ------------------------------------------------------------------ machete / udeps

def step_machete(args, raw: Path) -> dict:
    found = {}
    for target in [".", "studio/src-tauri", "studio/core", "studio/wasm"]:
        p = run(["cargo", "machete", "--with-metadata", target], args,
                log=raw / "logs" / f"machete_{target.replace('/', '_')}.log")
        crate = None
        for line in p.stdout.splitlines():
            m = re.match(r"^(\S+) -- (.+Cargo\.toml):", line)
            if m:
                crate = (m.group(1), rel(m.group(2).replace("\\", "/").lstrip("./")))
                continue
            if crate and line.startswith("\t"):
                found.setdefault(crate, set()).add(line.strip())
    results = []
    for (crate, manifest), deps in sorted(found.items()):
        mdir = (ROOT / manifest).parent
        mtext = (ROOT / manifest).read_text(encoding="utf-8") if (ROOT / manifest).exists() else ""
        for dep in sorted(deps):
            ident = dep.replace("-", "_")
            srcs = [p for p in mdir.rglob("*.rs") if "target" not in p.parts]
            used = sum(len(re.findall(rf"\b{ident}::|use {ident}\b|extern crate {ident}\b",
                                      p.read_text(encoding="utf-8", errors="replace"))) for p in srcs)
            build = (mdir / "build.rs").exists() and ident in (mdir / "build.rs").read_text(encoding="utf-8")
            line = next((i for i, l in enumerate(mtext.splitlines(), 1) if re.match(rf"^{re.escape(dep)}\s*=", l)), 1)
            verdict = ("false positive (build.rs)" if build else
                       "check (referenced in source)" if used else "unused")
            results.append({"crate": crate, "manifest": manifest, "line": line, "dep": dep,
                            "source_refs": used, "verdict": verdict})
    write_json(raw / "machete.json", results)
    return {"findings": results}


def step_udeps(args, raw: Path) -> dict:
    # Nightly artefacts in their own target dir, so they do not evict the stable ones.
    env = tool_env(args)
    env["CARGO_TARGET_DIR"] = str(args.target_dir) + "-udeps"
    p = run(["cargo", "+nightly", "udeps", "--workspace", "--all-targets", "--output", "json"],
            args, env=env, log=raw / "logs" / "udeps.log", timeout=7200)
    try:
        d = json.loads(p.stdout)
    except json.JSONDecodeError:
        return {"error": f"exit {p.returncode}: {p.stderr[-400:]}"}
    write_json(raw / "udeps.json", d)
    return {"unused": {k: v for k, v in d.get("unused_deps", {}).items()}}


# ------------------------------------------------------------------ deny

def step_deny(args, raw: Path) -> dict:
    p = run(["cargo", "deny", "--format", "json", "--config", str(ROOT / "deny.toml"),
             "check", "-s"], args, log=raw / "logs" / "deny.log", timeout=1800)
    diags, summary = [], {}
    for line in p.stderr.splitlines():
        try:
            d = json.loads(line)
        except json.JSONDecodeError:
            continue
        if d.get("type") == "summary":
            summary = d["fields"]
        elif d.get("type") == "diagnostic":
            f = d["fields"]
            if f.get("severity") in ("error", "warning"):
                krate = None
                if f.get("graphs"):
                    k = f["graphs"][0].get("Krate", {})
                    krate = f"{k.get('name')} {k.get('version')}"
                spans = [lab.get("span", "") for lab in f.get("labels", [])]
                diags.append({"severity": f["severity"], "code": f.get("code"),
                              "message": f["message"], "crate": krate, "spans": spans,
                              "line": (f.get("labels") or [{}])[0].get("line")})
    write_json(raw / "deny.json", {"summary": summary, "diagnostics": diags})
    dups = sorted(re.sub(r"found (\d+) duplicate entries for crate '(.+)'", r"\2 x\1", d["message"])
                  for d in diags if d["code"] == "duplicate")
    return {"summary": summary, "duplicates": dups,
            "licence_rejections": [d["crate"] for d in diags if d["code"] == "rejected"],
            "advisories": [d for d in diags if d["code"] and d["code"].startswith(("vulnerability", "unmaintained", "unsound", "yanked"))]}


# ------------------------------------------------------------------ bloat

def step_bloat(args, raw: Path) -> dict:
    res = {}
    for mode, extra in (("crates", ["--crates", "-n", "40"]), ("functions", ["-n", "60"])):
        p = run(["cargo", "bloat", "--release", "-p", "inkvec-cli", "--bin", "inkvec",
                 "--message-format", "json", *extra], args, log=raw / "logs" / f"bloat_{mode}.log",
                timeout=3600)
        try:
            d = json.loads(p.stdout.strip().splitlines()[-1])
        except (json.JSONDecodeError, IndexError):
            res[mode] = {"error": p.stderr[-400:]}
            continue
        write_json(raw / f"bloat_{mode}.json", d)
        res[mode] = d
    out = {}
    if "crates" in res and "crates" in res["crates"]:
        c = res["crates"]
        out["file_size"] = c.get("file-size")
        out["text_size"] = c.get("text-section-size")
        out["top_crates"] = [(x["name"], x["size"]) for x in c["crates"][:20]]
    if "functions" in res and "functions" in res["functions"]:
        out["top_functions"] = [(x.get("crate"), x["name"], x["size"]) for x in res["functions"]["functions"][:30]]
    return out


# ------------------------------------------------------------------ clippy census

def clippy_messages(stdout: str):
    for line in stdout.splitlines():
        try:
            m = json.loads(line)
        except json.JSONDecodeError:
            continue
        if m.get("reason") != "compiler-message":
            continue
        msg = m["message"]
        if msg.get("level") not in ("warning", "error"):
            continue
        code = (msg.get("code") or {}).get("code")
        if not code:
            continue
        span = next((s for s in msg.get("spans", []) if s.get("is_primary")), None)
        if not span:
            continue
        yield {"code": code, "level": msg["level"], "message": msg["message"],
               "file": rel(span["file_name"]), "line": span["line_start"],
               "end_line": span["line_end"], "col": span["column_start"],
               "end_col": span["column_end"], "package": m.get("package_id", "")}


def step_clippy(args, raw: Path) -> dict:
    base = ["cargo", "clippy", "--workspace", "--all-targets", "--message-format=json"]
    p0 = run(base, args, log=None, timeout=7200)
    p1 = run(base + ["--", "-W", "clippy::pedantic", "-W", "clippy::nursery"], args, timeout=7200)
    (raw / "clippy_default.jsonl").write_text(p0.stdout, encoding="utf-8")
    (raw / "clippy_pedantic.jsonl").write_text(p1.stdout, encoding="utf-8")

    def uniq(stdout):
        seen, out = set(), []
        for m in clippy_messages(stdout):
            key = (m["code"], m["file"], m["line"], m["col"])
            if key not in seen:
                seen.add(key)
                out.append(m)
        return out

    default = uniq(p0.stdout)
    full = uniq(p1.stdout)
    default_codes = collections.Counter(m["code"] for m in default)
    codes = collections.Counter(m["code"] for m in full)
    files = collections.Counter(m["file"] for m in full)
    prod = [m for m in full if not is_test_path(m["file"])]
    write_json(raw / "clippy_issues.json", full)
    return {
        "default_total": len(default), "default_by_lint": dict(default_codes.most_common()),
        "total": len(full), "production_total": len(prod), "lints": len(codes),
        "by_lint": dict(codes.most_common(40)),
        "by_file": dict(files.most_common(20)),
        "by_crate": dict(collections.Counter(crate_of(m["file"]) for m in full).most_common()),
    }


# ------------------------------------------------------------------ duplication

def step_dup(args, raw: Path) -> dict:
    out = raw / "jscpd"
    paths = [p for p in ["crates", "studio/core/src", "studio/src-tauri/src", "studio/wasm/src",
                         "studio/src"] if (ROOT / p).exists()]
    p = run(["npx", "--yes", "jscpd@4", *paths, "--min-tokens", str(args.dup_min_tokens),
             "--reporters", "json", "--output", str(out), "--format", "rust,typescript,tsx",
             "--ignore", "**/target/**,**/node_modules/**,**/*.d.ts,**/pkg/**", "--silent",
             "--exitCode", "0"], args, log=raw / "logs" / "jscpd.log", timeout=1800)
    rep = out / "jscpd-report.json"
    if not rep.exists():
        return {"error": f"jscpd produced no report (exit {p.returncode})"}
    d = json.loads(rep.read_text(encoding="utf-8"))
    stats = d["statistics"]
    fmts = {k: {"lines": v["total"]["lines"], "dup_lines": v["total"]["duplicatedLines"],
                "pct": round(v["total"]["percentage"], 2), "clones": v["total"]["clones"]}
            for k, v in stats["formats"].items()}
    clones = sorted(d["duplicates"], key=lambda c: -c["lines"])
    top = [{"lines": c["lines"], "format": c["format"],
            "a": f'{rel(c["firstFile"]["name"])}:{c["firstFile"]["start"]}-{c["firstFile"]["end"]}',
            "b": f'{rel(c["secondFile"]["name"])}:{c["secondFile"]["start"]}-{c["secondFile"]["end"]}'}
           for c in clones[:25]]
    return {"total_pct": round(stats["total"]["percentage"], 2), "clones": stats["total"]["clones"],
            "dup_lines": stats["total"]["duplicatedLines"], "formats": fmts, "top": top}


# ------------------------------------------------------------------ mutants

def step_mutants(args, raw: Path) -> dict:
    out = raw / "mutants"
    if not args.reuse_mutants or not (out / "mutants.out" / "outcomes.json").exists():
        env = tool_env(args)
        # Each cargo-mutants job builds in its own copy of the tree; a shared target dir
        # would serialise them on the lock (or worse, mix them).
        env.pop("CARGO_TARGET_DIR", None)
        env.update(CARGO_BUILD_JOBS=str(max(1, args.jobs // args.mutants_jobs)),
                   CARGO_PROFILE_RELEASE_LTO="false", CARGO_PROFILE_RELEASE_CODEGEN_UNITS="16",
                   CARGO_PROFILE_RELEASE_INCREMENTAL="true")
        if args.mutants_tmp:
            env.update(TMP=args.mutants_tmp, TEMP=args.mutants_tmp, TMPDIR=args.mutants_tmp)
        cmd = ["cargo", "mutants", "--profile", "release", "-j", str(args.mutants_jobs),
               "--minimum-test-timeout", "90", "-o", str(out), "--no-times"]
        for f in args.mutants_files.split(","):
            cmd += ["-f", f]
        if args.mutants_shard:
            cmd += ["--shard", args.mutants_shard, "--sharding", "round-robin"]
        run(cmd, args, env=env, log=raw / "logs" / "mutants.log", timeout=args.mutants_minutes * 60)
    return summarise_mutants(out / "mutants.out")


def summarise_mutants(mdir: Path) -> dict:
    f = mdir / "outcomes.json"
    if not f.exists():
        return {"error": f"{f} missing"}
    d = json.loads(f.read_text(encoding="utf-8"))
    outcomes = [o for o in d["outcomes"] if o.get("scenario") != "Baseline"]
    per_file = collections.defaultdict(collections.Counter)
    missed = []
    for o in outcomes:
        mut = o["scenario"]["Mutant"]
        summ = o.get("summary")
        per_file[mut["file"].replace("\\", "/")][summ] += 1
        if summ == "MissedMutant":
            missed.append({"file": mut["file"].replace("\\", "/"),
                           "line": mut["span"]["start"]["line"],
                           "function": (mut.get("function") or {}).get("function_name"),
                           "replacement": mut.get("replacement"),
                           "genre": mut.get("genre"), "name": mut.get("name")})
    tot = collections.Counter()
    for c in per_file.values():
        tot.update(c)
    viable = tot["CaughtMutant"] + tot["MissedMutant"] + tot["Timeout"]
    return {
        "totals": dict(tot), "viable": viable,
        "kill_rate": round(100.0 * (tot["CaughtMutant"] + tot["Timeout"]) / viable, 1) if viable else None,
        "per_file": {k: dict(v) for k, v in per_file.items()},
        "missed": missed,
    }


# ------------------------------------------------------------------ profile

def step_profile(args, raw: Path) -> dict:
    """Wall-clock repeats plus one samply profile per input.

    The profiled binary is the release CLI rebuilt with line tables only
    (CARGO_PROFILE_RELEASE_DEBUG=line-tables-only, own target dir): same LTO and codegen,
    but the PDB names every function. rustc embeds the PDB by file name only, and samply
    fails to find it when the path has a space, so exe + PDB + inputs are copied to the
    space-free --profile-work dir and recorded there.
    """
    import profile_report  # sibling module; imported here so the cheap steps never need it

    out = raw / "profile"
    out.mkdir(parents=True, exist_ok=True)
    prof_target = Path(str(args.target_dir) + "-prof")
    env = tool_env(args)
    env.update(CARGO_TARGET_DIR=str(prof_target), CARGO_PROFILE_RELEASE_DEBUG="line-tables-only")
    exe_name = "inkvec.exe" if os.name == "nt" else "inkvec"
    run(["cargo", "build", "--release", "-p", "inkvec-cli"], args, env=env, timeout=3600,
        log=raw / "logs" / "profile_build.log")
    work = Path(args.profile_work)
    work.mkdir(parents=True, exist_ok=True)
    for f in (exe_name, "inkvec.pdb"):
        if (prof_target / "release" / f).exists():
            shutil.copyfile(prof_target / "release" / f, work / f)
    exe = work / exe_name
    res = {}
    for inp in args.profile_inputs.split(","):
        src = Path(inp) if Path(inp).is_absolute() else ROOT / inp
        name = src.stem
        shutil.copyfile(src, work / src.name)
        times = []
        for _ in range(args.profile_repeats):
            t0 = time.time()
            p = run([str(exe), src.name, "-o", f"{name}.svg"], args, cwd=work)
            times.append(round(time.time() - t0, 3))
        (out / f"{name}.log").write_text(p.stderr, encoding="utf-8")
        entry = {"input": rel(src), "wall_s": times, "exit": p.returncode}
        if shutil.which("samply", path=env["PATH"]) and not args.no_samply:
            prof = work / f"{name}.json.gz"
            ps = run(["samply", "record", "--save-only", "--unstable-presymbolicate", "-r", "2000",
                      "-o", prof.name, "--", str(exe), src.name, "-o", f"{name}.svg"], args,
                     cwd=work, timeout=900)
            if ps.returncode == 0 and prof.exists():
                for f in (prof, work / f"{name}.json.syms.json", work / f"{name}.svg"):
                    if f.exists():
                        shutil.copyfile(f, out / f.name)
                entry["profile"] = profile_report.analyse(out / prof.name, top=25)
            else:
                entry["samply"] = f"failed: {ps.stderr[-300:]}"
        res[name] = entry
    return res


# ------------------------------------------------------------------ SARIF producers

def step_codeql(args, raw: Path) -> dict:
    cq = args.codeql or shutil.which("codeql")
    if not cq:
        return {"skipped": "CodeQL CLI not found (--codeql PATH)"}
    work = Path(args.codeql_work)
    src, db = work / "src", work / "db-rust"
    # The CodeQL Rust extractor splits its parameter file on spaces, so both the source root
    # and the database must live on a space-free path: export the tracked sources there.
    if src.exists():
        shutil.rmtree(src)
    src.mkdir(parents=True)
    arch = subprocess.run(["git", "archive", "HEAD", "crates", "studio/core", "studio/src-tauri/src",
                           "studio/src-tauri/Cargo.toml", "studio/wasm", "Cargo.toml", "Cargo.lock"],
                          cwd=ROOT, capture_output=True)
    subprocess.run(["tar", "-x", "-f", "-", "-C", str(src)], input=arch.stdout, check=True)
    env = tool_env(args)
    env["CARGO_TARGET_DIR"] = str(work / "target")
    run([cq, "database", "create", str(db), "--language=rust", f"--source-root={src}",
         "--overwrite", f"--threads={args.jobs}", "--build-mode=none"], args, env=env,
        log=raw / "logs" / "codeql_create.log", timeout=7200)
    sarif = raw / "codeql" / "rust.sarif"
    sarif.parent.mkdir(parents=True, exist_ok=True)
    tmp = work / "rust.sarif"
    run([cq, "database", "analyze", str(db),
         "codeql/rust-queries:codeql-suites/rust-security-and-quality.qls",
         "codeql/rust-queries:codeql-suites/rust-code-quality.qls",
         "--format=sarif-latest", f"--output={tmp}", f"--threads={args.jobs}", "--ram=12000"],
        args, env=env, log=raw / "logs" / "codeql_analyze.log", timeout=7200)
    if not tmp.exists():
        return {"error": "no SARIF; see raw/logs/codeql_*.log"}
    sonar_export.rebase_sarif(tmp, sarif, str(src))
    return sonar_export.sarif_summary(sarif)


def step_semgrep(args, raw: Path) -> dict:
    sg = args.semgrep or shutil.which("semgrep")
    if not sg:
        return {"skipped": "semgrep not found (--semgrep PATH)"}
    sarif = raw / "semgrep" / "semgrep.sarif"
    sarif.parent.mkdir(parents=True, exist_ok=True)
    env = tool_env(args)
    env.update(PYTHONUTF8="1", SEMGREP_SEND_METRICS="off")
    cmd = [sg, "scan", "--sarif", "--output", str(sarif), "--metrics=off", "--disable-version-check",
           "--config", "p/rust", "--config", str(HERE / "semgrep"),
           "--exclude", "target", "--exclude", "node_modules", "crates", "studio/core", "studio/src-tauri/src",
           "studio/wasm"]
    run(cmd, args, env=env, log=raw / "logs" / "semgrep.log", timeout=3600)
    if not sarif.exists():
        return {"error": "no SARIF; see raw/logs/semgrep.log"}
    return sonar_export.sarif_summary(sarif)


# ------------------------------------------------------------------ report

def write_summary_md(out: Path, s: dict) -> None:
    failed = {k: v for k, v in s.items() if isinstance(v, dict) and ("error" in v or "skipped" in v)}
    s = {k: v for k, v in s.items() if k not in failed}
    L = [f"# Inkvec quality audit - metrics digest", "",
         f"Generated {s.get('_generated')} from commit {s.get('_commit')} by tools/quality_audit/run.py.",
         "The narrative report (REPORT.md) is written by hand from these numbers.", ""]
    if failed:
        L += ["## Steps that failed or were skipped", ""]
        L += [f"- **{k}**: {str(v.get('error') or v.get('skipped'))[:300]}" for k, v in failed.items()]
        L += [""]
    c = s.get("complexity")
    if c:
        L += ["## Complexity (rust-code-analysis)", "",
              f"{c['functions']} production functions; {c['over_warn']} over cognitive {COGNITIVE_WARN}, "
              f"{c['over_bad']} over {COGNITIVE_BAD}.", "",
              md_table([(k, v["functions"], v["sloc"], v["cognitive_total"], v["p90"], v["max"],
                         v[f"over_{COGNITIVE_WARN}"], v[f"over_{COGNITIVE_BAD}"]) for k, v in c["per_crate"].items()],
                       ["crate", "fns", "sloc", "cog total", "p90", "max", f">{COGNITIVE_WARN}", f">{COGNITIVE_BAD}"]),
              "", "### Top 30 functions by cognitive complexity", "",
              md_table([(i + 1, f"`{f['file']}:{f['line']}`", f"`{f['name']}`", f["cognitive"], f["cyclomatic"],
                         f["sloc"], f["nargs"]) for i, f in enumerate(c["top_cognitive"])],
                       ["#", "location", "function", "cognitive", "cyclomatic", "sloc", "args"]),
              "", "### Worst files by maintainability index (MI, 0-100, files >= 150 SLOC)", "",
              md_table([(f"`{f['file']}`", f["sloc"], f["mi"], f["cognitive"], f["functions"]) for f in c["worst_mi_files"]],
                       ["file", "sloc", "MI", "cognitive", "fns"]), ""]
    m = s.get("modules")
    if m:
        L += ["## Module graphs (cargo-modules)", ""]
        for pkg, v in m.items():
            if "error" in v:
                L += [f"- {pkg}: error {v['error'][:200]}"]
                continue
            L += [f"### {pkg}", "",
                  f"{v['modules']} modules, {v['items']} items, {v['use_edges']} `use` edges; "
                  f"{v['top_edges']} top-level module edges. Orphans: {v['orphans']}. Graph: {v['svg'] or 'DOT only'}", "",
                  f"- Cycles among top-level modules (incl. crate root): {v['cycles_top'] or 'none'}",
                  f"- Cycles without the crate root: {v['cycles_top_without_root'] or 'none'}",
                  f"- Cycles at full module depth: {v['cycles_full'] or 'none'}",
                  f"- Modules importing from the crate root: {v['root_imported_by'] or 'none'}",
                  f"- Fan-out: {v['fan_out']}", f"- Fan-in: {v['fan_in']}", ""]
    cr = s.get("crates")
    if cr:
        L += ["## Crate graph", "", f"{len(cr['crates'])} crates, {len(cr['edges'])} internal edges.",
              f"Layer violations: {[(e['from'], e['to']) for e in cr['layer_violations']] or 'none'}",
              f"Dependents: {cr['dependents']}", ""]
    sm = s.get("smells")
    if sm:
        L += ["## Engine-layer smells (non-test library code)", "",
              md_table([(k, *[v.get(x, 0) for x in SMELLS]) for k, v in sm["per_crate"].items()],
                       ["crate", *SMELLS.keys()]),
              "", f"{sm['env_var_names']} distinct INKVEC_*/SVGIFY_* variable names in crates/.", ""]
    cv = s.get("coverage")
    if cv and "per_crate" in cv:
        L += ["## Coverage (cargo-llvm-cov, release, workspace tests)", "",
              f"Total: lines {cv['total_line_pct']}%, regions {cv['total_region_pct']}%.", "",
              md_table([(k, v["lines"], v["line_pct"], v["region_pct"], v["fn_pct"]) for k, v in cv["per_crate"].items()],
                       ["crate", "lines", "line %", "region %", "fn %"]),
              "", "Lowest-covered engine files (>= 100 lines):", "",
              md_table([(f"`{r['file']}`", r["lines"], r["line_pct"], r["region_pct"]) for r in cv["low_engine_files"]],
                       ["file", "lines", "line %", "region %"]), ""]
    mu = s.get("mutants")
    if mu and "totals" in mu:
        L += ["## Mutation testing (cargo-mutants, sampled)", "",
              f"Totals: {mu['totals']}; kill rate {mu['kill_rate']}% of {mu['viable']} viable.", "",
              md_table([(f"`{k}`", *[v.get(x, 0) for x in ("CaughtMutant", "MissedMutant", "Timeout", "Unviable")])
                        for k, v in mu["per_file"].items()], ["file", "caught", "missed", "timeout", "unviable"]),
              "", f"{len(mu['missed'])} surviving mutants: raw/mutants/mutants.out/missed.txt", ""]
    pr = s.get("profile")
    if pr:
        L += ["## Profile (release CLI, samply, CPU time over all threads)", ""]
        for name, e in pr.items():
            L += [f"### {name}", "", f"Wall clock (s): {e.get('wall_s')}", ""]
            p = e.get("profile")
            if p:
                L += [md_table([(f"`{m}`", f"{v}%") for m, v in p["by_module"][:12]], ["module (innermost Inkvec frame)", "CPU"]),
                      "", md_table([(f"`{m[:100]}`", f"{v}%") for m, v in p["inclusive_inkvec"][:12]], ["function (inclusive)", "CPU"]),
                      "", md_table([(f"`{m[:100]}`", f"{v}%") for m, v in p["self"][:12]], ["function (self)", "CPU"]), ""]
    for key, title in (("machete", "Unused dependencies (cargo-machete)"),):
        v = s.get(key)
        if v:
            L += [f"## {title}", "", md_table([(f["crate"], f["dep"], f["verdict"]) for f in v["findings"]],
                                               ["crate", "dependency", "verdict"]), ""]
    d = s.get("deny")
    if d:
        L += ["## cargo-deny", "", f"Summary: {d['summary']}", f"Licence rejections: {d['licence_rejections']}",
              f"Duplicate versions ({len(d['duplicates'])}): {', '.join(d['duplicates'])}", ""]
    b = s.get("bloat")
    if b and "top_crates" in b:
        L += ["## Binary size (cargo-bloat, release `inkvec`)", "",
              f"File {b['file_size']} bytes, .text {b['text_size']} bytes.", "",
              md_table([(n, f"{sz / 1024:.0f} KiB") for n, sz in b["top_crates"]], ["crate", ".text"]), "",
              md_table([(c, f"`{n[:90]}`", f"{sz / 1024:.1f} KiB") for c, n, sz in b.get("top_functions", [])[:20]],
                       ["crate", "function", "size"]), ""]
    cl = s.get("clippy")
    if cl:
        L += ["## Clippy census (pedantic + nursery, informational)", "",
              f"Default config: {cl['default_total']} warnings. With pedantic+nursery: {cl['total']} "
              f"({cl['production_total']} outside tests/examples) across {cl['lints']} lints.", "",
              md_table(list(cl["by_lint"].items())[:30], ["lint", "count"]), "",
              md_table(list(cl["by_file"].items())[:15], ["file", "count"]), ""]
    dp = s.get("dup")
    if dp and "top" in dp:
        L += ["## Duplication (jscpd)", "", f"{dp['total_pct']}% duplicated lines, {dp['clones']} clones.",
              "", md_table([(k, v["lines"], v["dup_lines"], v["pct"], v["clones"]) for k, v in dp["formats"].items()],
                           ["format", "lines", "dup lines", "%", "clones"]), "",
              md_table([(c["lines"], f"`{c['a']}`", f"`{c['b']}`") for c in dp["top"][:20]], ["lines", "first", "second"]), ""]
    for key in ("codeql", "semgrep"):
        v = s.get(key)
        if v:
            L += [f"## {key}", "", "```", json.dumps(v, indent=1)[:3000], "```", ""]
    so = s.get("sonar")
    if so:
        L += ["## SonarQube import files", "", "```", json.dumps(so, indent=1)[:3000], "```", ""]
    (out / "SUMMARY.md").write_text("\n".join(L), encoding="utf-8", newline="\n")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", type=Path, default=ROOT / "target-quality" / _dt.date.today().isoformat())
    ap.add_argument("--target-dir", type=Path, default=Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target-qa")))
    ap.add_argument("--steps", default=",".join(CHEAP_STEPS) + ",sonar")
    ap.add_argument("--jobs", type=int, default=8)
    ap.add_argument("--udeps", action="store_true")
    ap.add_argument("--mutants", action="store_true")
    ap.add_argument("--mutants-files", default=",".join(DEFAULT_MUTANT_FILES))
    ap.add_argument("--mutants-shard", default="0/16", help="k/n systematic sample, '' for all")
    ap.add_argument("--mutants-jobs", type=int, default=4)
    ap.add_argument("--mutants-minutes", type=int, default=120)
    ap.add_argument("--mutants-tmp", default=None, help="TMP for the mutant tree copies")
    ap.add_argument("--reuse-mutants", action="store_true", help="only re-summarise raw/mutants")
    ap.add_argument("--reuse-coverage", action="store_true", help="only re-summarise raw/coverage")
    ap.add_argument("--profile", action="store_true")
    ap.add_argument("--profile-inputs", default="studio/src-tauri/samples/flat-logo.png",
                    help="comma-separated rasters (repo-relative or absolute)")
    ap.add_argument("--profile-work", default="M:/qa-audit/prof", help="space-free dir for samply")
    ap.add_argument("--profile-repeats", type=int, default=3)
    ap.add_argument("--no-samply", action="store_true")
    ap.add_argument("--codeql", nargs="?", const="codeql", default=None, help="run CodeQL (optional CLI path)")
    ap.add_argument("--codeql-work", default="M:/qa-audit", help="space-free work dir for CodeQL")
    ap.add_argument("--semgrep", nargs="?", const="semgrep", default=None, help="run Semgrep (optional exe path)")
    ap.add_argument("--dup-min-tokens", type=int, default=100)
    ap.add_argument("--cognitive-threshold", type=int, default=COGNITIVE_WARN)
    ap.add_argument("--sonar-dir", type=Path, default=ROOT / "target-sonar",
                    help="where the SonarQube import files go (inside the repo so the scanner sees them)")
    ap.add_argument("--no-npx", action="store_true")
    args = ap.parse_args()
    if args.codeql == "codeql":
        args.codeql = shutil.which("codeql")
    if args.semgrep == "semgrep":
        args.semgrep = shutil.which("semgrep")

    out: Path = args.out
    raw = out / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    summary_path = out / "summary.json"

    def record(updates: dict) -> dict:
        # Read-modify-write: two runs with different --steps may share one output folder
        # (e.g. a long --mutants run beside a quick rerun of clippy), and neither may drop
        # the other's sections.
        cur = json.loads(summary_path.read_text(encoding="utf-8")) if summary_path.exists() else {}
        cur.update(updates)
        write_json(summary_path, cur)
        return cur

    summary = record({})
    steps = [s for s in args.steps.split(",") if s]
    for flag in ("udeps", "mutants", "profile", "codeql", "semgrep"):
        if getattr(args, flag) and flag not in steps:
            steps.insert(-1 if steps and steps[-1] == "sonar" else len(steps), flag)
    fns = {name[5:]: fn for name, fn in globals().items() if name.startswith("step_")}
    for step in steps:
        if step == "sonar":
            continue
        print(f"== {step}", flush=True)
        t0 = time.time()
        try:
            result = fns[step](args, raw)
        except Exception as e:  # a failed tool is a finding, not a reason to stop
            result = {"error": f"{type(e).__name__}: {e}"}
        summary = record({step: result, step + "_seconds": round(time.time() - t0, 1)})
    if "sonar" in steps:
        print("== sonar", flush=True)
        summary = record({"sonar": sonar_export.export_all(raw, args.sonar_dir, ROOT, args.cognitive_threshold)})
    summary = record({
        "_generated": _dt.datetime.now().isoformat(timespec="seconds"),
        "_commit": subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                                  capture_output=True, text=True).stdout.strip(),
    })
    write_summary_md(out, summary)
    print(f"wrote {out / 'SUMMARY.md'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
