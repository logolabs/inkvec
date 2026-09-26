"""Turn the audit's raw tool output into files SonarQube imports, and check them.

SonarQube Community Build reads three kinds of external input, all set in
tools/quality_audit/sonar-project.properties:

    sonar.externalIssuesReportPaths   generic issue JSON (the "rules" + "issues" format of
                                      SonarQube 10.3+), one file per tool, written here
    sonar.sarifReportPaths            SARIF 2.1.0 (CodeQL, Semgrep), copied here with their
                                      URIs made repo-relative
    sonar.rust.lcov.reportPaths       LCOV from cargo-llvm-cov, with source paths rewritten

SonarQube cannot run on this machine yet (Docker is not started), so `validate()` checks
every generic file against the documented schema before the scanner ever sees it: required
fields, enum values, rule references, and -- the one that fails a whole import -- that every
primary location is a real file with a start line inside it. Columns are never emitted:
Sonar rejects the report on an out-of-range column, and line granularity is enough here.

    python tools/quality_audit/sonar_export.py --raw OUT/raw --sonar-dir target-sonar
    python tools/quality_audit/sonar_export.py --validate target-sonar/*.json
"""

from __future__ import annotations

import argparse
import collections
import csv
import json
import re
import sys
from pathlib import Path

CLEAN_CODE_ATTRIBUTES = {
    "FORMATTED", "CONVENTIONAL", "IDENTIFIABLE", "CLEAR", "LOGICAL", "COMPLETE", "EFFICIENT",
    "FOCUSED", "DISTINCT", "MODULAR", "TESTED", "LAWFUL", "TRUSTWORTHY", "RESPECTFUL",
}
SOFTWARE_QUALITIES = {"SECURITY", "RELIABILITY", "MAINTAINABILITY"}
IMPACT_SEVERITIES = {"BLOCKER", "HIGH", "MEDIUM", "LOW", "INFO"}
# Directories the properties file lists under sonar.sources/sonar.tests: an issue anywhere
# else is on a file the scanner never indexes, and Sonar drops it.
INDEXED_PREFIXES = ("crates/", "studio/core/", "studio/src-tauri/src/", "studio/wasm/", "studio/src/")


class Report:
    """One generic-issue file: rules are declared once, issues reference them."""

    def __init__(self, engine: str):
        self.engine = engine
        self.rules: dict[str, dict] = {}
        self.issues: list[dict] = []

    def rule(self, rid: str, name: str, description: str, attribute: str,
             quality: str, severity: str) -> str:
        if rid not in self.rules:
            self.rules[rid] = {
                "id": rid, "name": name, "description": description, "engineId": self.engine,
                "cleanCodeAttribute": attribute,
                "impacts": [{"softwareQuality": quality, "severity": severity}],
            }
        return rid

    def issue(self, rid: str, message: str, path: str, line: int, end_line: int | None = None,
              effort: int = 0, secondary: list[tuple[str, str, int, int | None]] | None = None):
        loc = {"message": message[:4000], "filePath": path, "textRange": {"startLine": max(1, int(line))}}
        if end_line and int(end_line) > int(line):
            loc["textRange"]["endLine"] = int(end_line)
        it = {"ruleId": rid, "effortMinutes": int(effort), "primaryLocation": loc}
        if secondary:
            it["secondaryLocations"] = [
                {"message": m[:4000], "filePath": p, "textRange": {"startLine": max(1, int(a)),
                                                                   **({"endLine": int(b)} if b and int(b) > int(a) else {})}}
                for m, p, a, b in secondary]
        self.issues.append(it)

    def dump(self, path: Path) -> dict:
        path.parent.mkdir(parents=True, exist_ok=True)
        doc = {"rules": list(self.rules.values()), "issues": self.issues}
        path.write_text(json.dumps(doc, indent=1), encoding="utf-8")
        return {"file": path.name, "rules": len(self.rules), "issues": len(self.issues)}


# ------------------------------------------------------------------ converters

def from_clippy(raw: Path, root: Path) -> Report | None:
    f = raw / "clippy_issues.json"
    if not f.exists():
        return None
    default_codes = set()
    dflt = raw / "clippy_default.jsonl"
    if dflt.exists():
        for line in dflt.read_text(encoding="utf-8").splitlines():
            try:
                m = json.loads(line)
            except json.JSONDecodeError:
                continue
            code = ((m.get("message") or {}).get("code") or {}).get("code")
            if code:
                default_codes.add(code)
    rep = Report("clippy")
    for m in json.loads(f.read_text(encoding="utf-8")):
        # A lint whose primary span is inside a std macro (`/rustc/<hash>/library/...`)
        # has no file in this repository to hang on.
        if not (root / m["file"]).is_file():
            continue
        code = m["code"]
        in_default = code in default_codes
        rustc = not code.startswith("clippy::")
        attribute = "CONVENTIONAL" if rustc else "CLEAR"
        sev = "MEDIUM" if (in_default or m["level"] == "error") else "LOW"
        url = (f"https://rust-lang.github.io/rust-clippy/master/index.html#{code.split('::')[-1]}"
               if not rustc else "https://doc.rust-lang.org/rustc/lints/listing/index.html")
        group = "enabled by the workspace's own lint config" if in_default else "pedantic/nursery census only"
        rid = rep.rule(code, code, f"{'rustc' if rustc else 'Clippy'} lint `{code}` ({group}). See {url}",
                       attribute, "MAINTAINABILITY", sev)
        rep.issue(rid, m["message"], m["file"], m["line"], m.get("end_line"), effort=5)
    return rep


def from_complexity(raw: Path, threshold: int) -> Report | None:
    f = raw / "complexity_functions.csv"
    if not f.exists():
        return None
    rep = Report("rust-code-analysis")
    rid = rep.rule("cognitive-complexity", "Function cognitive complexity above threshold",
                   f"rust-code-analysis cognitive complexity above {threshold} (Sonar's own S3776 "
                   "threshold is 15). Split the function along its phases.",
                   "FOCUSED", "MAINTAINABILITY", "MEDIUM")
    rid_hi = rep.rule("cognitive-complexity-high", "Function cognitive complexity far above threshold",
                      f"rust-code-analysis cognitive complexity above {threshold * 3}.",
                      "FOCUSED", "MAINTAINABILITY", "HIGH")
    with f.open(encoding="utf-8") as fh:
        for r in csv.DictReader(fh):
            if r["test"] == "True":
                continue
            cog = float(r["cognitive"])
            if cog <= threshold:
                continue
            rep.issue(rid_hi if cog > threshold * 3 else rid,
                      f"`{r['name']}` has cognitive complexity {cog:.0f} (> {threshold}), "
                      f"cyclomatic {float(r['cyclomatic']):.0f}, {float(r['sloc']):.0f} lines.",
                      r["file"], int(r["line"]), None, effort=int(5 + cog - threshold))
    return rep


def from_mutants(raw: Path) -> Report | None:
    f = raw / "mutants" / "mutants.out" / "outcomes.json"
    if not f.exists():
        return None
    rep = Report("cargo-mutants")
    rid = rep.rule("missed-mutant", "Mutant survived the tests",
                   "cargo-mutants changed this code and every test still passed: the behaviour "
                   "here is not pinned by any test of the owning crate.",
                   "TESTED", "RELIABILITY", "MEDIUM")
    rid_t = rep.rule("timeout-mutant", "Mutant made the tests time out",
                     "Usually caught (an infinite loop); listed for completeness.",
                     "TESTED", "RELIABILITY", "INFO")
    for o in json.loads(f.read_text(encoding="utf-8"))["outcomes"]:
        if o.get("scenario") == "Baseline":
            continue
        s = o.get("summary")
        if s not in ("MissedMutant", "Timeout"):
            continue
        mu = o["scenario"]["Mutant"]
        fn = (mu.get("function") or {}).get("function_name") or "?"
        path = mu["file"].replace("\\", "/")
        line = mu["span"]["start"]["line"]
        what = mu.get("name") or f"replace with {mu.get('replacement')}"
        rep.issue(rid if s == "MissedMutant" else rid_t,
                  f"Surviving mutant in `{fn}`: {what}", path, line,
                  mu["span"]["end"]["line"], effort=15)
    return rep


def crate_root_source(root: Path, manifest_rel: str) -> str | None:
    mdir = (root / manifest_rel).parent
    for cand in ("src/lib.rs", "src/main.rs"):
        if (mdir / cand).exists():
            return (Path(manifest_rel).parent / cand).as_posix()
    return None


def from_machete(raw: Path, root: Path) -> Report | None:
    f = raw / "machete.json"
    if not f.exists():
        return None
    rep = Report("cargo-machete")
    rid = rep.rule("unused-dependency", "Dependency declared but not used",
                   "cargo-machete found no reference to this dependency in the crate's sources. "
                   "Anchored on the crate's root source file because Sonar only keeps issues on "
                   "indexed source files; the Cargo.toml line is in the message.",
                   "FOCUSED", "MAINTAINABILITY", "LOW")
    for r in json.loads(f.read_text(encoding="utf-8")):
        if r["verdict"].startswith("false positive"):
            continue
        anchor = crate_root_source(root, r["manifest"])
        if not anchor:
            continue
        rep.issue(rid, f"{r['manifest']}:{r['line']}: `{r['dep']}` looks unused ({r['verdict']}).",
                  anchor, 1, effort=5)
    return rep


def workspace_members(root: Path) -> dict[str, str]:
    """crate name -> repo-relative root source file, for every crate under crates/ and studio/."""
    out = {}
    for m in list(root.glob("crates/*/Cargo.toml")) + list(root.glob("studio/*/Cargo.toml")):
        txt = m.read_text(encoding="utf-8")
        mm = re.search(r'^name\s*=\s*"([^"]+)"', txt, re.M)
        src = crate_root_source(root, m.relative_to(root).as_posix())
        if mm and src:
            out[mm.group(1)] = src
    return out


def from_deny(raw: Path, root: Path) -> Report | None:
    f = raw / "logs" / "deny.log"
    if not f.exists():
        return None
    members = workspace_members(root)
    rep = Report("cargo-deny")
    kinds = {
        "duplicate": ("duplicate-version", "Several versions of one crate in the graph", "DISTINCT", "MAINTAINABILITY", "LOW"),
        "rejected": ("licence-rejected", "Licence outside the permissive allowlist", "LAWFUL", "SECURITY", "MEDIUM"),
        "vulnerability": ("advisory-vulnerability", "RustSec vulnerability", "TRUSTWORTHY", "SECURITY", "HIGH"),
        "unmaintained": ("advisory-unmaintained", "RustSec: crate unmaintained", "TRUSTWORTHY", "SECURITY", "LOW"),
        "unsound": ("advisory-unsound", "RustSec: unsound API", "TRUSTWORTHY", "RELIABILITY", "MEDIUM"),
        "yanked": ("advisory-yanked", "Yanked crate version", "TRUSTWORTHY", "RELIABILITY", "LOW"),
    }

    def first_member(graphs):
        stack = list(graphs)
        while stack:
            g = stack.pop(0)
            name = (g.get("Krate") or {}).get("name")
            if name in members:
                return name
            stack.extend(g.get("parents") or [])
        return None

    for line in f.read_text(encoding="utf-8").splitlines():
        try:
            d = json.loads(line)
        except json.JSONDecodeError:
            continue
        if d.get("type") != "diagnostic":
            continue
        fl = d["fields"]
        code = fl.get("code")
        if code not in kinds or fl.get("severity") not in ("error", "warning"):
            continue
        rid, name, attr, qual, sev = kinds[code]
        rep.rule(rid, name, f"cargo-deny `{code}` (tools/quality_audit/deny.toml).", attr, qual, sev)
        member = first_member(fl.get("graphs") or []) or "inkvec-cli"
        krate = ""
        if fl.get("graphs"):
            k = fl["graphs"][0].get("Krate", {})
            krate = f" [{k.get('name')} {k.get('version')}]"
        rep.issue(rid, f"{fl['message']}{krate}; reached through workspace crate {member}.",
                  members.get(member, "crates/inkvec-cli/src/lib.rs"), 1, effort=10)
    return rep


def from_jscpd(raw: Path, root: Path) -> Report | None:
    f = raw / "jscpd" / "jscpd-report.json"
    if not f.exists():
        return None
    rep = Report("jscpd")
    rid = rep.rule("duplicated-block", "Duplicated block", "jscpd found the same token sequence in two places.",
                   "DISTINCT", "MAINTAINABILITY", "LOW")

    def relp(p):
        p = p.replace("\\", "/")
        r = str(root).replace("\\", "/")
        return p[len(r) + 1:] if p.lower().startswith(r.lower()) else p

    for c in json.loads(f.read_text(encoding="utf-8"))["duplicates"]:
        a, b = c["firstFile"], c["secondFile"]
        rep.issue(rid, f"{c['lines']} duplicated lines (also at {relp(b['name'])}:{b['start']}).",
                  relp(a["name"]), a["start"], a["end"], effort=max(5, c["lines"] // 2),
                  secondary=[("the other copy", relp(b["name"]), b["start"], b["end"])])
    return rep


# ------------------------------------------------------------------ SARIF + LCOV

def rebase_sarif(src: Path, dst: Path, base: str) -> None:
    """Make artifact URIs repo-relative (CodeQL writes them relative to its source root, or
    as file:/// URIs of the exported copy)."""
    d = json.loads(src.read_text(encoding="utf-8"))
    b = base.replace("\\", "/").rstrip("/")
    prefixes = [f"file:///{b}/", f"file:/{b}/", f"{b}/"]

    def fix(u: str) -> str:
        # Semgrep on Windows writes `crates\\inkvec-ffi\\src\\lib.rs`; a SARIF URI uses `/`.
        u2 = u.replace("%20", " ").replace("\\", "/")
        for p in prefixes:
            if u2.lower().startswith(p.lower()):
                return u2[len(p):]
        return u2

    for run in d.get("runs", []):
        for res in run.get("results", []):
            for loc in res.get("locations", []) + res.get("relatedLocations", []):
                al = loc.get("physicalLocation", {}).get("artifactLocation", {})
                if "uri" in al:
                    al["uri"] = fix(al["uri"])
                    al.pop("uriBaseId", None)
        for art in run.get("artifacts", []):
            al = art.get("location", {})
            if "uri" in al:
                al["uri"] = fix(al["uri"])
                al.pop("uriBaseId", None)
    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_text(json.dumps(d), encoding="utf-8")


def sarif_summary(path: Path) -> dict:
    d = json.loads(path.read_text(encoding="utf-8"))
    rules, levels, files = collections.Counter(), collections.Counter(), collections.Counter()
    for run in d.get("runs", []):
        for r in run.get("results", []):
            rules[r.get("ruleId")] += 1
            levels[r.get("level", "warning")] += 1
            locs = r.get("locations") or [{}]
            files[locs[0].get("physicalLocation", {}).get("artifactLocation", {}).get("uri")] += 1
    return {"file": path.name, "version": d.get("version"), "results": sum(rules.values()),
            "by_rule": dict(rules.most_common(25)), "by_level": dict(levels),
            "top_files": dict(files.most_common(10))}


def rewrite_lcov(src: Path, dst: Path, root: Path, prefix: str) -> int:
    r = str(root).replace("\\", "/").lower()
    n = 0
    out = []
    for line in src.read_text(encoding="utf-8").splitlines():
        if line.startswith("SF:"):
            p = line[3:].replace("\\", "/")
            if p.lower().startswith(r + "/"):
                p = p[len(r) + 1:]
            out.append("SF:" + (prefix.rstrip("/") + "/" + p if prefix else p))
            n += 1
        else:
            out.append(line)
    dst.write_text("\n".join(out) + "\n", encoding="utf-8", newline="\n")
    return n


# ------------------------------------------------------------------ validation

def validate(path: Path, root: Path) -> list[str]:
    """Problems that would make SonarQube reject or silently drop the file."""
    errs = []
    try:
        d = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        return [f"{path.name}: not JSON: {e}"]
    if set(d) - {"rules", "issues"}:
        errs.append(f"{path.name}: unexpected top-level keys {set(d) - {'rules', 'issues'}}")
    rules = {}
    for r in d.get("rules", []):
        for k in ("id", "name", "description", "engineId"):
            if not isinstance(r.get(k), str) or not r.get(k):
                errs.append(f"{path.name}: rule {r.get('id')} missing {k}")
        if r.get("cleanCodeAttribute") not in CLEAN_CODE_ATTRIBUTES:
            errs.append(f"{path.name}: rule {r.get('id')} bad cleanCodeAttribute {r.get('cleanCodeAttribute')}")
        imps = r.get("impacts") or []
        if not imps:
            errs.append(f"{path.name}: rule {r.get('id')} has no impacts")
        for im in imps:
            if im.get("softwareQuality") not in SOFTWARE_QUALITIES or im.get("severity") not in IMPACT_SEVERITIES:
                errs.append(f"{path.name}: rule {r.get('id')} bad impact {im}")
        if r.get("id") in rules:
            errs.append(f"{path.name}: duplicate rule id {r.get('id')}")
        rules[r.get("id")] = r
    line_counts: dict[str, int] = {}

    def check_loc(loc, where):
        if not isinstance(loc.get("message"), str) or not loc["message"]:
            errs.append(f"{where}: empty message")
        fp = loc.get("filePath")
        if not isinstance(fp, str) or not fp:
            errs.append(f"{where}: missing filePath")
            return
        if not fp.startswith(INDEXED_PREFIXES):
            errs.append(f"{where}: {fp} is outside sonar.sources/sonar.tests (issue would be dropped)")
        full = root / fp
        if fp not in line_counts:
            line_counts[fp] = len(full.read_text(encoding="utf-8", errors="replace").splitlines()) if full.exists() else -1
        n = line_counts[fp]
        if n < 0:
            errs.append(f"{where}: {fp} does not exist")
            return
        tr = loc.get("textRange")
        if tr is None:
            return
        s, e = tr.get("startLine"), tr.get("endLine", tr.get("startLine"))
        if not isinstance(s, int) or s < 1 or s > max(n, 1):
            errs.append(f"{where}: startLine {s} outside 1..{n} of {fp}")
        if not isinstance(e, int) or e < s or e > max(n, 1):
            errs.append(f"{where}: endLine {e} invalid for {fp} ({n} lines)")
        if "startColumn" in tr or "endColumn" in tr:
            errs.append(f"{where}: columns present (the exporter never writes them)")

    for i, it in enumerate(d.get("issues", [])):
        where = f"{path.name}#issue{i}"
        if it.get("ruleId") not in rules:
            errs.append(f"{where}: ruleId {it.get('ruleId')} not declared in rules")
        if "effortMinutes" in it and (not isinstance(it["effortMinutes"], int) or it["effortMinutes"] < 0):
            errs.append(f"{where}: bad effortMinutes")
        if "primaryLocation" not in it:
            errs.append(f"{where}: missing primaryLocation")
            continue
        check_loc(it["primaryLocation"], where)
        for j, sl in enumerate(it.get("secondaryLocations") or []):
            check_loc(sl, f"{where}.secondary{j}")
    return errs


def validate_sarif(path: Path, root: Path) -> list[str]:
    errs = []
    d = json.loads(path.read_text(encoding="utf-8"))
    if d.get("version") != "2.1.0":
        errs.append(f"{path.name}: SARIF version {d.get('version')} (Sonar needs 2.1.0)")
    missing = 0
    for run in d.get("runs", []):
        if not run.get("tool", {}).get("driver", {}).get("name"):
            errs.append(f"{path.name}: run without tool.driver.name")
        for r in run.get("results", []):
            if not r.get("ruleId") or not (r.get("message") or {}).get("text"):
                errs.append(f"{path.name}: result without ruleId/message.text")
            for loc in r.get("locations", []):
                uri = loc.get("physicalLocation", {}).get("artifactLocation", {}).get("uri", "")
                if uri and not (root / uri).exists():
                    missing += 1
    if missing:
        errs.append(f"{path.name}: {missing} locations point at files not in the repo")
    return errs


# ------------------------------------------------------------------ driver

def export_all(raw: Path, sonar_dir: Path, root: Path, threshold: int = 15) -> dict:
    sonar_dir.mkdir(parents=True, exist_ok=True)
    for old in sonar_dir.glob("*.json"):
        old.unlink()
    written, problems = {}, []
    for name, rep in (("clippy", from_clippy(raw, root)),
                      ("complexity", from_complexity(raw, threshold)),
                      ("mutants", from_mutants(raw)),
                      ("machete", from_machete(raw, root)),
                      ("deny", from_deny(raw, root)),
                      ("jscpd", from_jscpd(raw, root))):
        p = sonar_dir / f"{name}.generic.json"
        if rep is None:
            # sonar-project.properties lists every file; an empty report keeps the scanner
            # from failing on a tool that was not run this time.
            p.write_text('{"rules": [], "issues": []}', encoding="utf-8")
            written[name] = "no input (empty report written)"
            continue
        written[name] = rep.dump(p)
        problems += validate(p, root)
    for name, src in (("codeql", raw / "codeql" / "rust.sarif"), ("semgrep", raw / "semgrep" / "semgrep.sarif")):
        if src.exists():
            dst = sonar_dir / f"{name}.sarif"
            rebase_sarif(src, dst, str(root))
            written[name] = sarif_summary(dst)["results"]
            problems += validate_sarif(dst, root)
        else:
            empty = {"version": "2.1.0", "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
                     "runs": [{"tool": {"driver": {"name": name, "rules": []}}, "results": []}]}
            (sonar_dir / f"{name}.sarif").write_text(json.dumps(empty), encoding="utf-8")
            written[name] = "no input (empty SARIF written)"
    lcov =raw / "coverage" / "lcov.info"
    if lcov.exists():
        written["lcov"] = rewrite_lcov(lcov, sonar_dir / "lcov.info", root, "")
        rewrite_lcov(lcov, sonar_dir / "lcov.docker.info", root, "/usr/src")
    (sonar_dir / "VALIDATION.txt").write_text("\n".join(problems) or "OK: no problems", encoding="utf-8")
    return {"dir": str(sonar_dir), "files": written, "validation_problems": len(problems),
            "validation_sample": problems[:10]}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--raw", type=Path)
    ap.add_argument("--sonar-dir", type=Path, default=Path(__file__).resolve().parents[2] / "target-sonar")
    ap.add_argument("--threshold", type=int, default=15)
    ap.add_argument("--validate", nargs="*", type=Path)
    a = ap.parse_args()
    root = Path(__file__).resolve().parents[2]
    if a.validate:
        errs = []
        for p in a.validate:
            errs += validate_sarif(p, root) if p.suffix == ".sarif" else validate(p, root)
        print("\n".join(errs) or "OK")
        return 1 if errs else 0
    print(json.dumps(export_all(a.raw, a.sonar_dir, root, a.threshold), indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
