"""Everything the evolution loop needs to score one candidate of inkvec.

A candidate is one Rust source file (a *cell*: the palette, the smoother, ...) with
`// EVOLVE-BLOCK-START` / `// EVOLVE-BLOCK-END` markers around the part the mutator may
touch. Scoring it means: drop it into a build slot, build, run that crate's tests, trace
the evaluation set, judge at 1024 against the artist's SVG, and compare with the baseline
the run started from.

Build slots are private copies of the workspace, each with its own `target/`, so N
evaluations can compile at once without cargo's lock serialising them and with real
incremental builds (only the evolved crate recompiles). `warm_slots` makes them.

Scoring is parallel across images (`workers` processes; each traces, renders and takes
the colour error with single-threaded BLAS/rayon so W workers use W cores; DISTS runs
once in the parent, on the GPU when there is one, so no worker loads torch). Timing is
judged on a small *serial* canary with the tracer's own multi-threading, one icon per
family, because wall time under a parallel run measures contention, not the tracer.

The objective is macro-averaged over families (noto, twemoji, lucide, ...): every family
gets one vote, so a set that is 30 % emoji cannot be won by fixing emoji idioms.

Nothing here knows about ShinkaEvolve; `evaluate.py` is the adapter.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
from concurrent.futures import ProcessPoolExecutor
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
BENCH = ROOT / "bench"
WORK = Path(os.environ.get("INKVEC_BENCH_WORK", BENCH / "data" / "_work"))
STATE = BENCH / "data" / "_state"
CACHE = Path(os.environ.get("INKVEC_BENCH_CACHE", BENCH / "data" / "_cache"))
sys.path.insert(0, str(BENCH))

JUDGE_SIZE = 1024
EXE = "inkvec.exe" if os.name == "nt" else "inkvec"

# What may evolve. `start`/`end` are line anchors (substring match, first occurrence);
# the block runs from the start anchor to the end anchor exclusive, or to EOF.
CELLS = {
    "palette": dict(
        file="crates/inkvec-trace/src/color.rs",
        crate="inkvec-trace",
        start="pub const DEFAULT_MERGE_DISTANCE",
        end=None,
        prompt="cells/palette.md",
    ),
    "smooth": dict(
        file="crates/inkvec-fit/src/smooth.rs",
        crate="inkvec-fit",
        start="const MIN_RUN",
        end=None,
        prompt="cells/smooth.md",
    ),
    "gradient": dict(
        file="crates/inkvec-trace/src/gradient.rs",
        crate="inkvec-trace",
        start=None,
        end=None,
        prompt="cells/gradient.md",
    ),
}

# Objective and constraints. dE00 runs ~0.3-1.3 on this corpus, DISTS ~0.03-0.15; the
# weight puts a typical DISTS improvement on the same footing as a typical dE00 one.
DISTS_WEIGHT = 10.0
MAX_IMAGE_REGRESSION_DE = 0.10   # no single icon may get this much worse than baseline
MAX_RATIO_GROWTH = 1.15          # mean params/GT-params may not grow more than this
MAX_TRACE_SECONDS = 5.0          # p95 per image (serial canary)
MAX_TRACE_SECONDS_HARD = 12.0    # any image (serial canary)
HELD_NOISE_DE = 0.005            # held-A may not regress beyond this
BUILD_TIMEOUT = 1800
TRACE_TIMEOUT = 120
CANARY = 8


# ----------------------------------------------------------------------------- sets
def load_sets() -> dict:
    """dev / held_a / held_b / full. Prefers the stratified ~1k corpus (devset_v2.json);
    falls back to the 200-icon set with held split in halves."""
    v2 = ROOT / "bench" / "devset_v2.json"
    if v2.exists():
        d = json.loads(v2.read_text(encoding="utf-8"))
        global TIER, _TIER_RESOLVED
        TIER = str(d.get("tier", "128"))
        _TIER_RESOLVED = True
        sets = {k: d[k] for k in ("dev", "held_a", "held_b", "full")}
        # A stratified quarter of the full set, for screening a change before it is worth
        # a full run: every fourth icon of each family, which keeps the family mix and the
        # ordering the corpus builder chose. It overlaps dev and held on purpose - it is a
        # cheap first read, never the thing a change is promoted on.
        by_fam: dict[str, list] = {}
        for it in sets["full"]:
            by_fam.setdefault(it["corpus"], []).append(it)
        screen = []
        for fam in sorted(by_fam):
            screen.extend(by_fam[fam][::4])
        sets["screen"] = screen
        sets["all"] = _every_corpus_icon(sets["full"])
        return sets
    sets = json.loads((ROOT / "bench" / "devset.json").read_text(encoding="utf-8"))
    held = sets["held"]
    return {"dev": sets["dev"], "held_a": held[0::2], "held_b": held[1::2],
            "full": sets["dev"] + held}


def _every_corpus_icon(full: list) -> list:
    """Every icon on disk that has both a ground truth and a raster at the working tier.

    `full` is a stratified 980 drawn from a corpus of 1406, so 426 icons have never been
    scored by anything. They are not held-out in any meaningful sense -- nothing was tuned
    against them because nothing ever looked at them -- which makes this the widest honest
    read available, and the right set for a release check rather than a daily one.

    The order is deterministic (family, then stem) so a sample of it is reproducible.
    """
    known = {(i["corpus"], i["stem"]) for i in full}
    fields = {k: v for i in full for k, v in i.items() if k not in ("corpus", "stem")}
    out = list(full)
    for fam_dir in sorted((ROOT / "bench" / "data" / "corpus_raster").iterdir()):
        if not fam_dir.is_dir():
            continue
        fam = fam_dir.name
        for png in sorted((fam_dir / tier()).glob("*.png")):
            stem = png.stem
            if (fam, stem) in known:
                continue
            if not (ROOT / "bench" / "data" / "corpus_svg" / fam / f"{stem}.svg").exists():
                continue
            out.append({**fields, "corpus": fam, "stem": stem})
    return out


def sample(items: list, fraction: float, seed: int = 0) -> list:
    """A stratified random `fraction` of `items`, reproducible from `seed`.

    Stratified by family: a plain uniform sample of a corpus that is 22 % twemoji and 1.4 %
    synthetic will some days carry no synthetic icons at all, and the synthetic family is
    the one that holds the hard geometric cases. Sampling within each family keeps the mix
    the corpus was built to have, so two samples at the same fraction are comparable to
    each other and to the whole.

    The seed defaults to 0, so a sampled run is repeatable by default; pass a different one
    deliberately to ask whether a result survives a different draw. The score cache is keyed
    per icon, not per set, so a sample costs nothing that the full set has already paid for
    and two samples that overlap share their scores.
    """
    import random

    if not 0 < fraction <= 1:
        raise ValueError(f"sample fraction must be in (0, 1], got {fraction}")
    by_fam: dict[str, list] = {}
    for it in items:
        by_fam.setdefault(it["corpus"], []).append(it)
    rng = random.Random(seed)
    out = []
    for fam in sorted(by_fam):
        pool = by_fam[fam]
        # Round up, so a small family never vanishes from the sample entirely.
        k = max(1, round(len(pool) * fraction))
        out.extend(rng.sample(pool, min(k, len(pool))))
    out.sort(key=lambda i: (i["corpus"], i["stem"]))
    return out


TIER = "128"   # raster directory under corpus_raster/<family>/; devset_v2 may override
_TIER_RESOLVED = False


def tier() -> str:
    """The intake directory. Read from devset_v2.json on first use *in this process*:
    pool workers are fresh interpreters that never call `load_sets`, and a module
    global set in the parent does not reach them - the first supersampled-intake
    benchmark silently scored the old 4-level rasters in every worker."""
    global TIER, _TIER_RESOLVED
    if not _TIER_RESOLVED:
        # An environment override, checked first and inside `tier()` rather than at import,
        # for the same reason the file is read here: pool workers are fresh interpreters.
        # Setting a module global in the parent does not reach them, but the environment
        # does. This is what makes a run against the 256/512/1024 tiers possible at all.
        env = os.environ.get("INKVEC_TIER")
        if env:
            TIER = env
            _TIER_RESOLVED = True
            return TIER
        v2 = ROOT / "bench" / "devset_v2.json"
        if v2.exists():
            try:
                TIER = str(json.loads(v2.read_text(encoding="utf-8")).get("tier", "128"))
            except Exception:
                pass
        _TIER_RESOLVED = True
    return TIER


def item_paths(it: dict) -> tuple[Path, Path]:
    png = ROOT / "bench" / "data" / "corpus_raster" / it["corpus"] / tier() / f"{it['stem']}.png"
    gt = ROOT / "bench" / "data" / "corpus_svg" / it["corpus"] / f"{it['stem']}.svg"
    return png, gt


# ----------------------------------------------------------------------------- cells
def initial_program(cell: str) -> str:
    """The cell's source at HEAD with the EVOLVE-BLOCK markers inserted."""
    c = CELLS[cell]
    lines = (ROOT / c["file"]).read_text(encoding="utf-8").splitlines()
    if "EVOLVE-BLOCK-START" in "\n".join(lines):
        return "\n".join(lines) + "\n"
    s = 0 if c["start"] is None else next(i for i, l in enumerate(lines) if c["start"] in l)
    e = len(lines) if c["end"] is None else next(i for i, l in enumerate(lines) if c["end"] in l)
    out = lines[:s] + ["// EVOLVE-BLOCK-START"] + lines[s:e] + ["// EVOLVE-BLOCK-END"] + lines[e:]
    return "\n".join(out) + "\n"


# ----------------------------------------------------------------------------- slots
def _workspace_files():
    yield ROOT / "Cargo.toml"
    if (ROOT / "Cargo.lock").exists():
        yield ROOT / "Cargo.lock"


def slot_dir(i: int) -> Path:
    return WORK / f"slot_{i}"


def make_slot(i: int) -> Path:
    """Copy the workspace sources into slot i (target/ is kept if it exists)."""
    d = slot_dir(i)
    d.mkdir(parents=True, exist_ok=True)
    for f in _workspace_files():
        shutil.copy2(f, d / f.name)
    dst = d / "crates"
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(ROOT / "crates", dst, ignore=shutil.ignore_patterns("target"))
    return d


def cargo_env() -> dict:
    env = dict(os.environ)
    cargo_bin = Path.home() / ".cargo" / "bin"
    if cargo_bin.exists():
        env["PATH"] = str(cargo_bin) + os.pathsep + env.get("PATH", "")
    env.setdefault("CARGO_TERM_COLOR", "never")
    # Tune for the local build machine (native CPU optimisation). Only set when
    # the caller did not, so a cross-compile or a portability build can override it.
    env.setdefault("RUSTFLAGS", "-C target-cpu=native")
    return env


def warm_slots(n: int, jobs_per_build: int | None = None) -> None:
    for i in range(n):
        d = make_slot(i)
        cmd = ["cargo", "build", "--release", "-p", "inkvec-cli"]
        if jobs_per_build:
            cmd += ["-j", str(jobs_per_build)]
        print(f"warming {d} ...", flush=True)
        subprocess.run(cmd, cwd=d, env=cargo_env(), check=True, timeout=BUILD_TIMEOUT)


@contextmanager
def acquire_slot(n: int):
    """Block until one of n slots is free; yields its directory."""
    WORK.mkdir(parents=True, exist_ok=True)
    while True:
        for i in range(n):
            lock = WORK / f"slot_{i}.lock"
            fh = open(lock, "a+")
            try:
                if os.name == "nt":
                    import msvcrt
                    msvcrt.locking(fh.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError:
                fh.close()
                continue
            try:
                if not slot_dir(i).exists():
                    make_slot(i)
                yield slot_dir(i)
                return
            finally:
                try:
                    if os.name == "nt":
                        fh.seek(0)
                        msvcrt.locking(fh.fileno(), msvcrt.LK_UNLCK, 1)
                    else:
                        fcntl.flock(fh.fileno(), fcntl.LOCK_UN)
                finally:
                    fh.close()
        time.sleep(2.0)


# ----------------------------------------------------------------------------- build
@dataclass
class BuildResult:
    ok: bool
    exe: Path | None
    seconds: float
    error: str = ""


def _tail(b: bytes, n: int = 4000) -> str:
    return b.decode("utf-8", "replace")[-n:]


def build_candidate(slot: Path, cell: str, program_path: Path, jobs: int | None = None,
                    run_tests: bool = True) -> BuildResult:
    c = CELLS[cell]
    shutil.copy2(program_path, slot / c["file"])
    env = cargo_env()
    t0 = time.time()
    j = ["-j", str(jobs)] if jobs else []
    r = subprocess.run(["cargo", "build", "--release", "-p", "inkvec-cli", *j], cwd=slot,
                       env=env, capture_output=True, timeout=BUILD_TIMEOUT)
    if r.returncode != 0:
        return BuildResult(False, None, time.time() - t0, "BUILD FAILED\n" + _tail(r.stderr))
    if run_tests:
        r = subprocess.run(["cargo", "test", "--release", "-p", c["crate"], *j], cwd=slot,
                           env=env, capture_output=True, timeout=BUILD_TIMEOUT)
        if r.returncode != 0:
            return BuildResult(False, None, time.time() - t0,
                               "TESTS FAILED\n" + _tail(r.stdout) + _tail(r.stderr, 1500))
    exe = slot / "target" / "release" / EXE
    return BuildResult(exe.exists(), exe if exe.exists() else None, time.time() - t0,
                       "" if exe.exists() else "no executable produced")


# ----------------------------------------------------------------------------- scoring
@dataclass
class ImageScore:
    stem: str
    corpus: str
    de00: float
    dists: float
    ratio: float
    seconds: float
    # Structure signals. dE00 and DISTS both ask "does it look like the target"; none of
    # them asks "is it built sensibly", and a change can improve every fidelity number
    # while turning a stroke into a sawtooth. These three see that, and none of them needs
    # the artist's file, so they also work on a customer's asset.
    self_res: float = 0.0
    turning: float = 0.0
    mirror: float = 0.0
    svg: str = ""


@dataclass
class SetScore:
    name: str
    images: list[ImageScore] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)

    def family_means(self) -> dict[str, dict]:
        fam: dict[str, list[ImageScore]] = {}
        for i in self.images:
            fam.setdefault(i.corpus, []).append(i)
        return {f: dict(n=len(v), de00=float(np.mean([i.de00 for i in v])),
                        dists=float(np.mean([i.dists for i in v])),
                        ratio=float(np.mean([i.ratio for i in v])))
                for f, v in fam.items()}

    def _macro(self, key: str) -> float:
        fm = self.family_means()
        return float(np.mean([m[key] for m in fm.values()])) if fm else float("inf")

    @property
    def de00(self) -> float:
        return self._macro("de00")

    @property
    def dists(self) -> float:
        return self._macro("dists")

    @property
    def ratio(self) -> float:
        return self._macro("ratio")

    @property
    def objective(self) -> float:
        """Lower is better. Macro-averaged over families."""
        return self.de00 + DISTS_WEIGHT * self.dists

    def times(self) -> np.ndarray:
        return np.array([i.seconds for i in self.images]) if self.images else np.array([0.0])

    def to_json(self) -> dict:
        return {
            "name": self.name, "n": len(self.images), "failures": self.failures,
            "de00": self.de00, "dists": self.dists, "ratio": self.ratio,
            "objective": self.objective,
            "families": self.family_means(),
            "time_p95": float(np.percentile(self.times(), 95)),
            "time_max": float(self.times().max()),
            # Keyed by family/stem: the same stem exists in several families
            # (bluetooth in lucide and material), and a stem-only key silently dropped
            # one of them.
            "images": {f"{i.corpus}/{i.stem}": dict(stem=i.stem, corpus=i.corpus, de00=i.de00,
                                                    dists=i.dists, ratio=i.ratio, seconds=i.seconds)
                       for i in self.images},
        }


def memory_gb() -> int:
    """Physical RAM, for sizing the scoring pool: DISTS on a 1024x1024 pair holds ~2 GB
    of VGG activations per process, so 12 workers on a 32 GB desktop swap. Budget
    3 GB per worker."""
    try:
        import psutil
        return int(psutil.virtual_memory().total // 2**30)
    except Exception:
        pass
    try:
        if os.name == "nt":
            import ctypes
            class MS(ctypes.Structure):
                _fields_ = [("dwLength", ctypes.c_ulong), ("dwMemoryLoad", ctypes.c_ulong),
                            ("ullTotalPhys", ctypes.c_ulonglong), ("ullAvailPhys", ctypes.c_ulonglong),
                            ("ullTotalPageFile", ctypes.c_ulonglong), ("ullAvailPageFile", ctypes.c_ulonglong),
                            ("ullTotalVirtual", ctypes.c_ulonglong), ("ullAvailVirtual", ctypes.c_ulonglong),
                            ("ullAvailExtendedVirtual", ctypes.c_ulonglong)]
            m = MS(); m.dwLength = ctypes.sizeof(MS)
            ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(m))
            return int(m.ullTotalPhys // 2**30)
        return int(os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") // 2**30)
    except Exception:
        return 64


PIN_VARS = ("OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS",
            "NUMEXPR_NUM_THREADS", "RAYON_NUM_THREADS")


def _init_worker():
    """One core per worker: the tracer's rayon pool and torch/BLAS must not each grab
    every core, or W workers oversubscribe W× and the run is slower than serial."""
    for k in PIN_VARS:
        os.environ.setdefault(k, "1")
    try:
        import torch
        torch.set_num_threads(int(os.environ["OMP_NUM_THREADS"]))
    except Exception:
        pass


def gt_render(gt: Path, corpus: str, stem: str) -> np.ndarray:
    """GT rendered at JUDGE_SIZE, cached on disk as PNG (a few hundred KB each)."""
    from PIL import Image
    from inkvec_bench import render
    cp = CACHE / f"gt{JUDGE_SIZE}" / corpus / f"{stem}.png"
    if cp.exists():
        return np.asarray(Image.open(cp).convert("RGB"), dtype=np.float32) / 255.0
    ref = render.composite(render.render(gt.read_text(encoding="utf-8"), JUDGE_SIZE, JUDGE_SIZE))
    cp.parent.mkdir(parents=True, exist_ok=True)
    Image.fromarray((np.clip(ref, 0, 1) * 255 + 0.5).astype(np.uint8)).save(cp, optimize=False)
    return ref


# --- result cache ---------------------------------------------------------------------
#
# Scoring an icon costs about six CPU-seconds, nearly all of it the trace, and a paired
# A/B over the full set is therefore some twenty-five CPU-minutes at best. Most of that is
# work already done: the "before" side of a comparison is almost always a build that has
# been scored before, often several times in one session.
#
# So a build's per-icon numbers are kept on disk, keyed by the hash of the executable
# itself together with everything else that changes the answer (tracer arguments, the
# intake tier, the resolution judged at). A rerun of the same build is then free, and a
# comparison against a known build costs half of what it did.
#
# What is deliberately not cached: runs that keep the SVGs (the caller wants the files),
# and the serial canary (its whole point is a wall-clock measurement).


def exe_key(exe: Path) -> str:
    """Content hash of a build, so a rebuilt-but-identical binary still hits."""
    h = hashlib.sha1()
    with open(exe, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()[:16]


def _cache_path(exe: Path, extra_args) -> Path:
    args = " ".join(extra_args)
    env = os.environ.get("INKVEC_CACHE_SALT", "")
    tag = hashlib.sha1(f"{args}|{tier()}|{JUDGE_SIZE}|{env}".encode()).hexdigest()[:8]
    return CACHE / "scores" / f"{exe_key(exe)}-{tag}.json"


def cache_load(exe: Path, extra_args) -> dict:
    cp = _cache_path(exe, extra_args)
    if not cp.exists():
        return {}
    try:
        return json.loads(cp.read_text(encoding="utf-8"))
    except Exception:
        return {}


def cache_save(exe: Path, extra_args, entries: dict) -> None:
    cp = _cache_path(exe, extra_args)
    cp.parent.mkdir(parents=True, exist_ok=True)
    merged = cache_load(exe, extra_args)
    merged.update(entries)
    tmp = cp.with_suffix(".tmp")
    tmp.write_text(json.dumps(merged), encoding="utf-8")
    tmp.replace(cp)


CACHE_FIELDS = ("de00", "dists", "ratio", "seconds", "corpus", "stem",
                "self_res", "turning", "mirror")


def structure_signals(svg: str, src_png: Path) -> dict:
    """What the fidelity metrics cannot see, measured from our own output alone.

    * **self_res** - our SVG rendered back at the *input* resolution against the input
      raster. It needs no ground truth at all, and over 152 icons it tracks the true error
      against the artist at 0.907 (Spearman 0.899); ranking by it, the worst twenty catch
      seventeen of the genuinely worst twenty. That makes it a defect detector for assets
      we have no source for.
    * **turning** - total absolute turning of the emitted anchors, per unit length. This is
      the one that sees a sawtooth: on `openmoji/1F3A1` the teeth *lower* self_res from
      0.0146 to 0.0122 while turning goes 277 to 828 and the parameter count doubles. A
      boundary that turns three times as far to describe the same shape is wrong however
      well it renders.
    * **mirror** - the rendered output against its own mirror. Artists draw exact symmetry
      and we used to lose it by two orders of magnitude; nothing else notices.
    """
    import numpy as np
    from PIL import Image
    from inkvec_bench import render

    out: dict = {"self_res": 0.0, "turning": 0.0, "mirror": 0.0}
    try:
        src = np.asarray(Image.open(src_png).convert("RGBA"), dtype=np.float32) / 255.0
        h, w = src.shape[:2]
        a = src[..., 3:4]
        target = src[..., :3] * a + (1.0 - a)          # over white, as the tracer sees it
        ours = render.composite(render.render(svg, w, h))
        out["self_res"] = float(np.abs(ours - target).mean())
    except BaseException:
        pass
    try:
        big = render.composite(render.render(svg, 256, 256))
        out["mirror"] = float(np.abs(big - big[:, ::-1]).mean())
    except BaseException:
        pass
    try:
        total_turn = 0.0
        total_len = 0.0
        for d in re.findall(r'\sd="([^"]+)"', svg):
            pts = [(float(x), float(y))
                   for x, y in re.findall(r"(-?\d+\.?\d*),(-?\d+\.?\d*)", d)]
            for i in range(1, len(pts) - 1):
                ax, ay = pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1]
                bx, by = pts[i + 1][0] - pts[i][0], pts[i + 1][1] - pts[i][1]
                na = (ax * ax + ay * ay) ** 0.5
                nb = (bx * bx + by * by) ** 0.5
                if na > 1e-9 and nb > 1e-9:
                    c = max(-1.0, min(1.0, (ax * bx + ay * by) / (na * nb)))
                    total_turn += abs(np.arccos(c))
                    total_len += na
        out["turning"] = float(total_turn / total_len) if total_len > 1e-9 else 0.0
    except BaseException:
        pass
    return out


def score_one(args: tuple) -> dict:
    """Trace + render + colour error for one icon. Top-level so a process pool can
    pickle it. DISTS is *not* computed here: it needs torch and a VGG, which would put
    ~1 GB and a slice of the GPU into every worker; the parent scores it from the saved
    render instead (`score_set`)."""
    exe, it, out_dir, extra_args, keep_svg = args
    from PIL import Image
    from inkvec_bench import render, svgmodel
    from inkvec_bench.metrics import color as mcolor
    png, gt = item_paths(it)
    if not png.exists() or not gt.exists():
        return {"fail": f"{it['stem']}: missing input"}
    out = Path(out_dir) / f"{it['corpus']}__{it['stem']}.svg"
    t0 = time.time()
    try:
        r = subprocess.run([str(exe), str(png), "-o", str(out), "--quiet", *extra_args],
                           capture_output=True, timeout=TRACE_TIMEOUT)
    except subprocess.TimeoutExpired:
        return {"fail": f"{it['stem']}: timeout after {TRACE_TIMEOUT}s"}
    dt = time.time() - t0
    if r.returncode != 0:
        return {"fail": f"{it['stem']}: exit {r.returncode}: "
                        f"{r.stderr.decode('utf-8', 'replace')[-300:]}"}
    svg = out.read_text(encoding="utf-8")
    try:
        b = render.composite(render.render(svg, JUDGE_SIZE, JUDGE_SIZE))
    except BaseException as e:  # resvg raises odd things on malformed output
        return {"fail": f"{it['stem']}: render {type(e).__name__}"}
    ref = gt_render(gt, it["corpus"], it["stem"])
    rp = Path(out_dir) / f"{it['corpus']}__{it['stem']}.render.png"
    Image.fromarray((np.clip(b, 0, 1) * 255 + 0.5).astype(np.uint8)).save(rp, optimize=False)
    sig = structure_signals(svg, png)
    res = dict(stem=it["stem"], corpus=it["corpus"], **sig,
               de00=float(mcolor.delta_e00(ref, b)["de00_mean"]),
               dists=0.0,
               ratio=svgmodel.parse(svg).n_params / max(1, it["gt_params"]),
               seconds=dt, svg=svg if keep_svg else "")
    res["_render"] = str(rp)
    res["_gt"] = str(CACHE / f"gt{JUDGE_SIZE}" / it["corpus"] / f"{it['stem']}.png")
    if not keep_svg:
        out.unlink(missing_ok=True)
    return res


def _dists_in_parent(results: list[dict]) -> None:
    """DISTS for every scored icon, in this process only: one VGG, on the GPU when there
    is one. Reads the renders the workers saved and deletes them."""
    skip = os.environ.get("INKVEC_SKIP_DISTS") == "1"
    from PIL import Image
    if not skip:
        from inkvec_bench.metrics import raster
    for res in results:
        if "fail" in res:
            continue
        rp, gp = Path(res.pop("_render")), Path(res.pop("_gt"))
        if not skip:
            b = np.asarray(Image.open(rp).convert("RGB"), dtype=np.float32) / 255.0
            ref = np.asarray(Image.open(gp).convert("RGB"), dtype=np.float32) / 255.0
            res["dists"] = float(raster.dists_distance(ref, b))
        rp.unlink(missing_ok=True)


def score_set(exe: Path, name: str, items: list[dict], out_dir: Path, extra_args=(),
              keep_svgs: bool = False, workers: int = 1, use_cache: bool = True) -> SetScore:
    out_dir.mkdir(parents=True, exist_ok=True)
    ss = SetScore(name)
    # Anything already scored for this exact build comes off disk; only the rest is traced.
    cached: dict = {}
    if use_cache and not keep_svgs and os.environ.get("INKVEC_NO_SCORE_CACHE") != "1":
        have = cache_load(exe, tuple(extra_args))
        todo = []
        for it in items:
            k = f"{it['corpus']}/{it['stem']}"
            entry = have.get(k)
            if entry is not None and all(f in entry for f in CACHE_FIELDS):
                cached[k] = entry
            else:
                todo.append(it)
        if cached:
            print(f"{name}: {len(cached)} of {len(items)} from cache", flush=True)
        items = todo
    jobs = [(exe, it, out_dir, tuple(extra_args), keep_svgs) for it in items]
    if workers <= 1 or len(jobs) < 2:
        # Serial = the tracer runs the way a user runs it, with its own rayon pool on
        # every core. That is the wall time the sub-5 s rule is about (the canary);
        # single-threaded times are 5-10x longer and would trip the gate on nothing.
        results = list(map(score_one, jobs))
    else:
        # Pin BLAS/rayon threads in the *parent* environment before the pool spawns:
        # children inherit it and numpy reads it at import, which happens before any
        # initializer runs. Setting it only in the initializer is too late — each
        # worker then starts a full-width OpenBLAS pool, and 12 workers x 16 threads
        # took a 4-minute scoring pass to three hours (measured 2026-09-02).
        saved = {k: os.environ.get(k) for k in PIN_VARS}
        for k in PIN_VARS:
            os.environ[k] = "1"
        try:
            pool = ProcessPoolExecutor(max_workers=min(workers, len(jobs)),
                                       initializer=_init_worker)
            results = list(pool.map(score_one, jobs, chunksize=1))
            pool.shutdown()
        finally:
            for k, v in saved.items():
                if v is None:
                    os.environ.pop(k, None)
                else:
                    os.environ[k] = v
    _dists_in_parent(results)
    if cached or (use_cache and not keep_svgs
                  and os.environ.get("INKVEC_NO_SCORE_CACHE") != "1"):
        fresh = {
            f"{r['corpus']}/{r['stem']}": {k: r[k] for k in CACHE_FIELDS}
            for r in results if "fail" not in r
        }
        if fresh:
            cache_save(exe, tuple(extra_args), fresh)
    results = results + [dict(v, svg="") for v in cached.values()]
    for res in results:
        if "fail" in res:
            ss.failures.append(res["fail"])
        else:
            ss.images.append(ImageScore(**res))
    return ss


def canary_pick(dev: list[dict], n: int = CANARY) -> list[dict]:
    """One icon per family first (dev is sorted by family, so a plain prefix would be
    eight icons of the same kind), then round-robin until n."""
    fams: dict[str, list[dict]] = {}
    for it in dev:
        fams.setdefault(it["corpus"], []).append(it)
    out, k = [], 0
    while len(out) < n and any(len(v) > k for v in fams.values()):
        for v in fams.values():
            if len(v) > k and len(out) < n:
                out.append(v[k])
        k += 1
    return out


# ----------------------------------------------------------------------------- baseline
def baseline_path(cell: str) -> Path:
    return STATE / f"baseline_{cell}.json"


def load_baseline(cell: str) -> dict | None:
    p = baseline_path(cell)
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else None


def save_baseline(cell: str, canary: SetScore, dev: SetScore, held_a: SetScore) -> None:
    STATE.mkdir(parents=True, exist_ok=True)
    baseline_path(cell).write_text(json.dumps(
        {"canary": canary.to_json(), "dev": dev.to_json(), "held_a": held_a.to_json()},
        indent=1), encoding="utf-8")


# ----------------------------------------------------------------------------- verdict
@dataclass
class Verdict:
    correct: bool
    combined_score: float
    public: dict
    private: dict
    text_feedback: str
    error: str = ""


def per_image_deltas(cur: SetScore, base: dict) -> list[tuple[str, float, float]]:
    """(stem, dE00 delta, DISTS delta) vs the baseline, worst first."""
    bi = base["images"]
    rows = [(i.stem, i.de00 - bi[f"{i.corpus}/{i.stem}"]["de00"], i.dists - bi[f"{i.corpus}/{i.stem}"]["dists"])
            for i in cur.images if f"{i.corpus}/{i.stem}" in bi]
    rows.sort(key=lambda r: -r[1])
    return rows


def constraint_report(canary: SetScore, dev: SetScore, base_all: dict | None) -> list[str]:
    """Hard-constraint violations as human-readable lines (empty = all good)."""
    bad = []
    if dev.failures:
        bad.append(f"{len(dev.failures)} image(s) failed to trace or render: "
                   + "; ".join(dev.failures[:3]))
    base = base_all["dev"] if base_all else None
    bcan = base_all.get("canary") if base_all else None
    p95, mx = float(np.percentile(canary.times(), 95)), float(canary.times().max())
    if bcan:
        # Timing is judged on the serial canary against the same canary at baseline, so
        # a slow icon HEAD already has is not held against a candidate, only a candidate
        # making it slower. Absolute budgets still apply.
        p95_cap = max(MAX_TRACE_SECONDS, 1.25 * bcan["time_p95"])
        max_cap = max(MAX_TRACE_SECONDS_HARD, 1.5 * bcan["time_max"])
        if p95 > p95_cap:
            bad.append(f"canary trace p95 {p95:.2f}s exceeds {p95_cap:.2f}s "
                       f"(baseline {bcan['time_p95']:.2f}s)")
        if mx > max_cap:
            bad.append(f"slowest canary icon {mx:.1f}s exceeds {max_cap:.1f}s "
                       f"(baseline {bcan['time_max']:.1f}s)")
    if base:
        if dev.ratio > base["ratio"] * MAX_RATIO_GROWTH:
            bad.append(f"parameter ratio {dev.ratio:.3f} grew past "
                       f"{base['ratio'] * MAX_RATIO_GROWTH:.3f} (baseline {base['ratio']:.3f}); "
                       "an MDL tracer must not buy fidelity with more paths")
        worst = per_image_deltas(dev, base)[:1]
        if worst and worst[0][1] > MAX_IMAGE_REGRESSION_DE:
            bad.append(f"{worst[0][0]} regressed by {worst[0][1]:+.3f} dE00 "
                       f"(limit {MAX_IMAGE_REGRESSION_DE})")
    return bad


def feedback_text(dev: SetScore, base: dict | None, held_a: SetScore | None,
                  held_base: dict | None, violations: list[str], svgs: dict[str, str],
                  canary: SetScore) -> str:
    lines = []
    lines.append(f"dev ({len(dev.images)} icons, macro-averaged over "
                 f"{len(dev.family_means())} families): dE00 {dev.de00:.4f}, DISTS "
                 f"{dev.dists:.4f}, params/GT {dev.ratio:.3f}, objective {dev.objective:.4f}; "
                 f"serial canary p95 {np.percentile(canary.times(), 95):.2f}s")
    if base:
        lines[-1] += (f"   [baseline dE00 {base['de00']:.4f}, DISTS {base['dists']:.4f}, "
                      f"ratio {base['ratio']:.3f}, objective {base['objective']:.4f}]")
        fm, bfm = dev.family_means(), base.get("families", {})
        lines.append("per family dE00 (delta vs baseline): " + ", ".join(
            f"{f} {m['de00']:.3f} ({m['de00'] - bfm[f]['de00']:+.3f})" if f in bfm
            else f"{f} {m['de00']:.3f}" for f, m in sorted(fm.items())))
        rows = per_image_deltas(dev, base)
        better = [r for r in rows if r[1] < -1e-4]
        worse = [r for r in rows if r[1] > 1e-4]
        lines.append(f"icons better {len(better)} / worse {len(worse)} / unchanged "
                     f"{len(rows) - len(better) - len(worse)}")
        if worse:
            lines.append("largest regressions (dE00 delta, DISTS delta): " + ", ".join(
                f"{s} {d:+.3f}/{dd:+.4f}" for s, d, dd in worse[:5]))
        if better:
            lines.append("largest gains: " + ", ".join(
                f"{s} {d:+.3f}/{dd:+.4f}" for s, d, dd in sorted(better, key=lambda r: r[1])[:5]))
    else:
        lines.append("per family dE00: " + ", ".join(
            f"{f} {m['de00']:.3f} (n={m['n']})" for f, m in sorted(dev.family_means().items())))
    if held_a is not None:
        lines.append(f"held-A ({len(held_a.images)} icons): dE00 {held_a.de00:.4f}, "
                     f"DISTS {held_a.dists:.4f}, ratio {held_a.ratio:.3f}"
                     + (f"   [baseline dE00 {held_base['de00']:.4f}]" if held_base else ""))
    if violations:
        lines.append("HARD CONSTRAINT VIOLATIONS: " + " | ".join(violations))
    lines.extend(disease_report(dev, base, svgs))
    return "\n".join(lines)


def disease_report(dev: SetScore, base: dict | None, svgs: dict[str, str], n: int = 3) -> list[str]:
    """gt_diff attribution for the icons that carry the most error (or regressed most)."""
    try:
        import gt_diff  # bench/gt_diff.py
    except Exception:
        return []
    pick = [r[0] for r in per_image_deltas(dev, base)[:n] if r[1] > 1e-4] if base else []
    pick += [i.stem for i in sorted(dev.images, key=lambda i: -i.de00)[:n] if i.stem not in pick]
    by_stem = {i.stem: i for i in dev.images}
    out = []
    for stem in pick[:n + 1]:
        if stem not in svgs or stem not in by_stem:
            continue
        _, gt = item_paths({"corpus": by_stem[stem].corpus, "stem": stem})
        try:
            d, _ = gt_diff.compare(stem, gt.read_text(encoding="utf-8"), svgs[stem], 512, 128)
        except Exception as e:
            out.append(f"{stem}: gt_diff failed ({type(e).__name__})")
            continue
        attr = ", ".join(f"{k} {v:.0%}" for k, v in d.attribution.items())
        cls = ", ".join(f"{k} {v}" for k, v in d.classes.items() if v)
        fills = ", ".join(f"{k} {v}" for k, v in sorted(d.fills.items(), key=lambda kv: -kv[1])[:4])
        out.append(f"{stem} ({by_stem[stem].corpus}): dE00 {d.de_mean:.2f}; error by cause: {attr}; "
                   f"GT regions: {cls}; GT->ours fills: {fills}; contour displacement mean "
                   f"{d.disp_ours_to_gt.get('mean', 0):.2f}px")
    return out


def evaluate_program(cell: str, program_path: Path, results_dir: Path, n_slots: int,
                     jobs: int | None = None, workers: int = 1, run_tests: bool = True) -> Verdict:
    """The whole cascade for one candidate. Never raises for a bad candidate."""
    results_dir.mkdir(parents=True, exist_ok=True)
    sets = load_sets()
    base_all = load_baseline(cell)
    base = base_all["dev"] if base_all else None
    held_base = base_all["held_a"] if base_all else None
    canary_items = canary_pick(sets["dev"])

    with acquire_slot(n_slots) as slot:
        b = build_candidate(slot, cell, program_path, jobs=jobs, run_tests=run_tests)
        if not b.ok:
            return Verdict(False, -1e9, {}, {}, b.error, b.error)

        # Stage 1: serial canary — crashes, pathological slowness, and the timing gate.
        can = score_set(b.exe, "canary", canary_items, results_dir / "svg", workers=1)
        if can.failures or can.times().max() > 4 * MAX_TRACE_SECONDS_HARD:
            msg = "canary failed: " + "; ".join(can.failures) if can.failures else \
                f"canary too slow: {can.times().max():.1f}s"
            return Verdict(False, -1e9, {}, {}, msg, msg)

        # Stage 2: the dev set in parallel, SVGs kept for the disease report.
        dev = score_set(b.exe, "dev", sets["dev"], results_dir / "svg", keep_svgs=True,
                        workers=workers)
        svgs = {i.stem: i.svg for i in dev.images if i.svg}
        violations = constraint_report(can, dev, base_all)

        # Stage 3: held-A for the baseline itself and for candidates that are not worse
        # than it on dev; everything else is scored on dev alone.
        held_a = None
        if base is None or (not violations and dev.objective <= base["objective"] + 1e-9):
            held_a = score_set(b.exe, "held_a", sets["held_a"], results_dir / "svg_held",
                               workers=workers)
            if held_a.failures:
                violations.append(f"held-A failures: {'; '.join(held_a.failures[:3])}")
            elif held_base and held_a.de00 > held_base["de00"] + HELD_NOISE_DE:
                violations.append(f"held-A dE00 {held_a.de00:.4f} regressed vs baseline "
                                  f"{held_base['de00']:.4f} (dev-set overfit)")

    if base is None:
        # First evaluation of a run = the initial program. It defines the baseline.
        save_baseline(cell, can, dev, held_a if held_a else SetScore("held_a"))

    score = -dev.objective if not violations else -dev.objective - 10.0
    public = {
        "dev_de00": dev.de00, "dev_dists": dev.dists, "dev_ratio": dev.ratio,
        "dev_objective": dev.objective,
        "canary_p95_s": float(np.percentile(can.times(), 95)),
        "build_s": b.seconds, "n_failures": len(dev.failures),
    }
    if held_a is not None:
        public.update({"held_a_de00": held_a.de00, "held_a_dists": held_a.dists,
                       "held_a_ratio": held_a.ratio})
    private = {"canary": can.to_json(), "dev": dev.to_json(),
               "held_a": held_a.to_json() if held_a else None}
    (results_dir / "scores.json").write_text(json.dumps(private, indent=1), encoding="utf-8")
    text = feedback_text(dev, base, held_a, held_base, violations, svgs, can)
    return Verdict(not violations, score, public, private, text,
                   " | ".join(violations) if violations else "")
