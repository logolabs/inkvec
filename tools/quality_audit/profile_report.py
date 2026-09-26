"""Summarise a samply profile (Firefox-profiler JSON + the --unstable-presymbolicate sidecar).

samply records on Windows through ETW/xperf and saves an unsymbolicated profile; with
`--unstable-presymbolicate` it also writes `<profile>.syms.json`, a per-library table of
(rva, size, symbol). This joins the two and answers "where does trace time go":

    self       CPU time in the leaf function
    inclusive  CPU time with the function anywhere on the stack (counted once per sample)
    module     every sample charged to the innermost Inkvec frame's module
               (inkvec_trace::gradient::bands, ...), so time spent in std, rayon or the
               allocator on a module's behalf is charged to that module

Samples are weighted by the thread's CPU delta when the profile has it, so idle rayon
workers parked in the kernel do not count.

    python tools/quality_audit/profile_report.py raw/profile/flat-logo-512.json.gz
"""

from __future__ import annotations

import bisect
import collections
import gzip
import json
import re
import sys
from pathlib import Path

OURS = re.compile(r"^(inkvec[a-z_]*)::")
GENERIC_TAIL = re.compile(r"<[^<>]*>")


def load(path: Path):
    opener = gzip.open if path.suffix == ".gz" else open
    with opener(path, "rt", encoding="utf-8") as fh:
        prof = json.load(fh)
    syms_path = Path(str(path)[: -len(".gz")] + ".syms.json") if path.suffix == ".gz" else Path(str(path) + ".syms.json")
    syms = json.loads(syms_path.read_text(encoding="utf-8")) if syms_path.exists() else {"string_table": [], "data": []}
    return prof, syms


def clean(name: str) -> str:
    # `impl$3::foo<ref$<...>>` -> readable; closures keep their `closure$N`.
    prev = None
    while prev != name:
        prev, name = name, GENERIC_TAIL.sub("", name)
    return name


def module_of(name: str) -> str | None:
    if not OURS.match(name):
        return None
    keep = []
    for p in name.split("::")[:-1]:
        # Stop at the first type (`Cubic`, `Problem`) or compiler-made segment: the module
        # path is what the rollup is about.
        if p[:1].isupper() or p.startswith(("impl$", "closure$", "enum2$", "<")):
            break
        keep.append(p)
    return "::".join(keep[:3]) if keep else name.split("::")[0]


def analyse(path: Path, top: int = 30) -> dict:
    prof, syms = load(path)
    strings = syms["string_table"]
    tables = {}
    for lib in syms["data"]:
        st = sorted(lib["symbol_table"], key=lambda s: s["rva"])
        tables[lib["debug_name"].lower()] = ([s["rva"] for s in st], st)
    libs = prof["libs"]

    def resolve(lib_index, addr):
        if lib_index is None:
            return "?"
        lib = libs[lib_index]
        t = tables.get((lib.get("debugName") or "").lower())
        if not t:
            return lib.get("name", "?")
        rvas, st = t
        i = bisect.bisect_right(rvas, addr) - 1
        if i >= 0 and addr < st[i]["rva"] + max(st[i]["size"], 1):
            return clean(strings[st[i]["symbol"]])
        return f"{lib.get('name', '?')}+0x{addr:x}"

    self_t, incl_t, mod_t = collections.Counter(), collections.Counter(), collections.Counter()
    total = 0.0
    threads_used = 0
    for th in prof["threads"]:
        if "inkvec" not in (th.get("processName") or "").lower():
            continue
        s, stk, fr, fn, rs = th["samples"], th["stackTable"], th["frameTable"], th["funcTable"], th["resourceTable"]
        weights = s.get("threadCPUDelta") or s.get("weight") or [1] * s["length"]
        cache: dict[int, str] = {}

        def frame_name(f):
            if f in cache:
                return cache[f]
            func = fr["func"][f]
            res = fn["resource"][func] if func is not None else None
            lib = rs["lib"][res] if res is not None and res >= 0 else None
            addr = fr["address"][f]
            n = resolve(lib, addr) if addr is not None and addr >= 0 else th["stringArray"][fn["name"][func]]
            cache[f] = n
            return n

        used = False
        for i in range(s["length"]):
            st = s["stack"][i]
            w = weights[i] if weights[i] is not None else 0
            if st is None or not w:
                continue
            used = True
            total += w
            names = []
            while st is not None:
                names.append(frame_name(stk["frame"][st]))
                st = stk["prefix"][st]
            self_t[names[0]] += w
            for n in set(names):
                incl_t[n] += w
            mod = next((m for m in (module_of(n) for n in names) if m), "(outside inkvec)")
            mod_t[mod] += w
        threads_used += used
    pct = lambda c: [(n, round(100.0 * v / total, 1)) for n, v in c.most_common(top)] if total else []
    ours_incl = collections.Counter({n: v for n, v in incl_t.items() if OURS.match(n)})
    return {
        "profile": path.name, "threads_with_cpu": threads_used,
        "cpu_units": prof["meta"].get("sampleUnits", {}).get("threadCPUDelta", "samples"),
        "total_cpu": total,
        "self": pct(self_t), "inclusive_inkvec": pct(ours_incl), "by_module": pct(mod_t),
    }


def main() -> int:
    for p in sys.argv[1:]:
        r = analyse(Path(p))
        print(f"== {r['profile']}  threads with CPU: {r['threads_with_cpu']}  total CPU: {r['total_cpu']:.0f} {r['cpu_units']}")
        for key in ("by_module", "inclusive_inkvec", "self"):
            print(f"-- {key}")
            for n, v in r[key][:25]:
                print(f"  {v:5.1f}%  {n[:150]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
