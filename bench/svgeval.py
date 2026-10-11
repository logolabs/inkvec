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
        # INKVEC_TIER wins here as it does in `tier()`. Without this, a serial run (workers=1,
        # scored in this process) of `INKVEC_TIER=512ss` read the 128ss rasters, because this
        # line had already resolved the tier from the devset file.
        TIER = os.environ.get("INKVEC_TIER") or str(d.get("tier", "128"))
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
        for png in sorted((fam_dir / base_tier(tier())).glob("*.png")):
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


def set_tier(name: str) -> None:
    """Score against raster tier `name` from now on, in this process and in every pool
    worker it starts afterwards.

    Both halves are needed. Spawned workers (Windows, macOS) are fresh interpreters that
    resolve the tier from the environment; forked workers (Linux) inherit this module's
    globals as they are at the fork, and a global already resolved to another tier would
    win over the environment. Setting only one of the two scores the wrong rasters on one
    platform and the right ones on the other."""
    global TIER, _TIER_RESOLVED
    os.environ["INKVEC_TIER"] = name
    TIER = name
    _TIER_RESOLVED = True


#: A tier named `<base>op` is tier `<base>` flattened onto white: the same artwork as an
#: opaque logo on a white page, the input most users send. The r2-eval research measured
#: that it behaves differently from the transparent raster (v0.2.4 at 512 px: params ratio
#: median 1.19 transparent vs 1.50 opaque; one background rect more), so the gate scores
#: both. The flattened rasters are derived on first use, not committed.
OPAQUE_SUFFIX = "op"


def base_tier(name: str) -> str:
    """The committed raster tier a tier is read from: `512ssop` -> `512ss`, else itself."""
    if name.endswith(OPAQUE_SUFFIX) and len(name) > len(OPAQUE_SUFFIX):
        return name[: -len(OPAQUE_SUFFIX)]
    return name


def flatten_onto_white(src: Path, dst: Path) -> None:
    """Composite an RGBA raster over white and save it as 8-bit RGB.

    out = rgb * a + (1 - a), per channel, in straight (non-premultiplied) alpha with every
    value in [0, 1], then quantised as `floor(255 out + 0.5)`. This is the arithmetic the
    r2-eval research used for its `512ssop` numbers, kept identical so they are comparable.
    The write is atomic: two pool workers may derive the same file at once.
    """
    from PIL import Image
    a = np.asarray(Image.open(src).convert("RGBA"), dtype=np.float32) / 255.0
    rgb = a[..., :3] * a[..., 3:4] + (1.0 - a[..., 3:4])
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp = dst.with_name(f"{dst.stem}.{os.getpid()}.tmp.png")
    Image.fromarray(_rgb8(rgb), "RGB").save(tmp)
    try:
        os.replace(tmp, dst)
    except OSError:
        tmp.unlink(missing_ok=True)


def item_paths(it: dict) -> tuple[Path, Path]:
    """The input raster at the current tier and the artist's SVG for one icon.

    A committed raster wins, PNG or (a tier stored as JPEG files: `web`) JPEG. For an opaque
    tier (`<base>op`) with no committed raster, the base tier's raster is flattened onto white
    into the cache (`CACHE/raster/<tier>/`) once, and that file is the input."""
    t = tier()
    png = ROOT / "bench" / "data" / "corpus_raster" / it["corpus"] / t / f"{it['stem']}.png"
    jpg = png.with_suffix(".jpg")
    if not png.exists() and jpg.exists():
        png = jpg  # a tier committed as JPEG files (`web`), traced as the tracer reads them
    gt = ROOT / "bench" / "data" / "corpus_svg" / it["corpus"] / f"{it['stem']}.svg"
    if not png.exists() and base_tier(t) != t:
        src = ROOT / "bench" / "data" / "corpus_raster" / it["corpus"] / base_tier(t) / f"{it['stem']}.png"
        derived = CACHE / "raster" / t / it["corpus"] / f"{it['stem']}.png"
        if not derived.exists() and src.exists():
            flatten_onto_white(src, derived)
        png = derived
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
    # Geometric match to the artist's file (`inkvec_bench/geomatch.py`): the mean distance,
    # in input pixels, between the trace's edges and the artist's, by the area between
    # them; and the part of it more than a pixel from any artist edge.
    geom: float = 0.0
    geom_far: float = 0.0
    svg: str = ""
    # SHA-256 of the emitted SVG's bytes. Two builds that emit the same bytes for an icon
    # score it identically, so the gate can tell "unchanged" from "changed by a tie".
    sha256: str = ""
    # True when the SVG's bytes matched the ones a baseline scored (`Job.known`), so the
    # scores above are the baseline's, not recomputed; `mirror` and `dists` are then NaN.
    reused: bool = False
    # The design battery (`inkvec_bench/design.py`) when asked for (`Job.human`):
    # {"trace": {statistic: value}, "div": {statistic: distance from the artist's file}}.
    human: dict = field(default_factory=dict)
    # CPU seconds this icon cost in its worker: the tracer ("trace"), the gate's signals
    # ("score", 0 when reused) and the design battery ("human").
    cpu: dict = field(default_factory=dict)


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


#: Bumped whenever the scorer's answer for the same SVG changes, so per-icon numbers that
#: an older scorer cached (`cache_save`) or a baseline recorded (`bench/ci_gate.py`) are
#: never compared with new ones as if they were the same measurement.
#:
#: 2 (2026-10-02): the reference render is always the 8-bit one, on a cache miss as on a
#: hit (`gt_render`). Version 1 scored a cold cache against the float render, which moved
#: the screen set's macro dE00 by +0.111 % and single icons by up to 0.037 between a fresh
#: checkout (CI) and a warm one (every local run); see `gt_render`.
#:
#: 3 (2026-10-04): `turning` reads each path command by command (`inkvec_bench/turning.py`).
#: Version 2 took every "x,y" pair as a point, so an arc's radii and flags counted as
#: points (215 of 246 screen-set traces carry arcs) and one polyline ran through every
#: subpath; primitives (`<rect>`, `<circle>`, ...) were not read at all. dE00, ratio and
#: self_res are unchanged.
#:
#: 4 (2026-10-10): `geom` and `geom_far`, the geometric match to the artist's file
#: (`inkvec_bench/geomatch.py`), are scored. Every earlier signal is unchanged.
SCORER_VERSION = 4


def _rgb8(img: np.ndarray) -> np.ndarray:
    """Quantise a composited float render in [0, 1] to 8-bit RGB, the way the cache stores it:
    round half up, `floor(255 x + 0.5)`, after clipping to [0, 1]."""
    return (np.clip(img, 0, 1) * 255 + 0.5).astype(np.uint8)


def _from_rgb8(img8: np.ndarray) -> np.ndarray:
    """The float32 image the scorer compares against: an 8-bit RGB array divided by 255.

    Both branches of `gt_render` go through this one function, so they return the same
    dtype and the same bits: a uint8 cast to float32 is exact, and dividing a float32 array
    by the Python float 255.0 gives float32 under NumPy's scalar promotion (NEP 50)."""
    return np.asarray(img8, dtype=np.float32) / 255.0


def gt_render(gt: Path, corpus: str, stem: str) -> np.ndarray:
    """The artist's SVG rendered at JUDGE_SIZE over white, as the 8-bit image the cache holds.

    The render is cached on disk as an RGB PNG (a few hundred KB each). The function returns
    the *same array* whether it rendered the file just now or read the cache: on a miss it
    quantises the render to 8 bits, writes that, and returns what a later hit would decode.

    Why this matters. Version 1 returned the float render on a miss and the 8-bit PNG on
    every later call, so the score depended on the cache's history rather than on the SVGs.
    CI always starts cold; a local checkout is usually warm. On v0.2.4's own 246 traces the
    float reference gave a macro dE00 of 0.12819 and the 8-bit one 0.12833 (+0.111 %, at
    most 0.037 on one icon), which is the "load-sensitive" wobble of 2026-09-25 (0.14418
    vs 0.14432, +0.097 %, the lower number on the cold, fresh worktree): measured by the
    r2-eval research, 2026-10-02. With both branches returning the stored image, scoring is
    a pure function of the two SVGs.

    Why 8 bits and not the float render. DISTS (`_dists_in_parent`) already reads the cached
    PNG, so the 8-bit image is what every other metric sees; storing floats instead would
    cost 12 MB per icon at 1024 px. The quantisation error, at most 0.5/255 per channel, is
    far below a just-noticeable difference.

    The write is atomic (a temporary file, then `os.replace`), because pool workers scoring
    the same icon under different conditions can miss at the same moment; a reader must
    never decode a half-written PNG. If another worker won the race, its file is identical
    (rendering is deterministic), so losing the replace is harmless.

    Not from the literature: a cache-coherence fix, because the score must be a function of
    its inputs alone. See also: Bouthillier et al. 2021, "Accounting for Variance in Machine
    Learning Benchmarks", MLSys 2021, arXiv 2103.03098, on removing incidental sources of
    variation from a benchmark before reading its differences.
    """
    from PIL import Image
    from inkvec_bench import render
    cp = CACHE / f"gt{JUDGE_SIZE}" / corpus / f"{stem}.png"
    if cp.exists():
        return _from_rgb8(np.asarray(Image.open(cp).convert("RGB")))
    ref8 = _rgb8(render.composite(render.render(gt.read_text(encoding="utf-8"), JUDGE_SIZE, JUDGE_SIZE)))
    cp.parent.mkdir(parents=True, exist_ok=True)
    tmp = cp.with_name(f"{cp.stem}.{os.getpid()}.tmp.png")
    Image.fromarray(ref8, "RGB").save(tmp, optimize=False)
    try:
        os.replace(tmp, cp)
    except OSError:
        # Windows refuses to replace a file another process has open; that process wrote
        # the same bytes, so drop ours.
        tmp.unlink(missing_ok=True)
    return _from_rgb8(ref8)


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
    """Where one build's per-icon scores live, keyed by everything that changes them: the
    build, the tracer arguments, the intake tier, the judging size, a free salt, and the
    scorer version (so numbers from an older scorer are never read back)."""
    args = " ".join(extra_args)
    env = os.environ.get("INKVEC_CACHE_SALT", "")
    tag = hashlib.sha1(f"{args}|{tier()}|{JUDGE_SIZE}|{env}|v{SCORER_VERSION}".encode()).hexdigest()[:8]
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
                "self_res", "turning", "mirror", "geom", "geom_far", "sha256")


def structure_signals(svg: str, src_png: Path) -> dict:
    """What the fidelity metrics cannot see, measured from our own output alone.

    * **self_res** - our SVG rendered back at the *input* resolution against the input
      raster. It needs no ground truth at all, and over 152 icons it tracks the true error
      against the artist at 0.907 (Spearman 0.899); ranking by it, the worst twenty catch
      seventeen of the genuinely worst twenty. That makes it a defect detector for assets
      we have no source for.
    * **turning** - total absolute turning of each subpath's control polygon, per unit
      length (`inkvec_bench/turning.py`, which says exactly what is read). This is
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
        from inkvec_bench.turning import turning
        out["turning"] = float(turning(svg))
    except BaseException:
        pass
    return out


def is_opaque(src_png: Path) -> bool:
    """Whether an icon is compared over white: an opaque tier (flattened onto white) or a
    file with no alpha at all (a JPEG), as it was made. A transparent tier's icon stays
    transparent even where its pixels happen to be all opaque, so its numbers stay
    comparable with its baseline."""
    from PIL import Image
    with Image.open(src_png) as im:
        return base_tier(tier()) != tier() or "A" not in im.getbands()


def geometric_match(svg: str, gt: Path, src_png: Path) -> dict:
    """`geom` and `geom_far` of our SVG against the artist's file (`inkvec_bench/geomatch.py`),
    at the input raster's size; over white when the input is opaque. The artist's side is
    kept in the cache (`geomatch.artist_side`): it depends on the artist's bytes, the size
    and the page only."""
    from PIL import Image
    from inkvec_bench import geomatch as gm
    gm.CACHE_DIR = CACHE / "geomatch"
    with Image.open(src_png) as im:
        w = im.size[0]
    return gm.geomatch(svg, gt.read_text(encoding="utf-8"), w, is_opaque(src_png))


def artist_design(gt: Path, raster_px: int) -> dict:
    """The design battery's profile of the artist's file (`inkvec_bench/design.py`) for a
    `raster_px` raster, kept in the cache by the file's hash, the raster size and the
    battery's version: it is the same in every run."""
    from inkvec_bench import design
    text = gt.read_text(encoding="utf-8")
    key = hashlib.sha256(f"{design.VERSION}|{raster_px}|{text}".encode()).hexdigest()[:32]
    cp = CACHE / "design" / f"{key}.json"
    if cp.exists():
        try:
            return json.loads(cp.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            pass
    prof = design.profile(text, raster_px=raster_px)
    cp.parent.mkdir(parents=True, exist_ok=True)
    tmp = cp.with_name(f"{cp.stem}.{os.getpid()}.tmp")
    tmp.write_text(json.dumps(prof), encoding="utf-8")
    try:
        os.replace(tmp, cp)
    except OSError:
        tmp.unlink(missing_ok=True)
    return prof


def human_stats(svg: str, gt: Path, raster_px: int) -> dict:
    """{"trace": {statistic: one number}, "artist": {statistic: one number}, "div":
    {statistic: distance from the artist's file}} of one trace (`inkvec_bench/design.py`);
    empty if the trace cannot be read."""
    from inkvec_bench import design
    try:
        art_text = gt.read_text(encoding="utf-8")
        c = design.compare(svg, art_text, raster_px, artist_design(gt, raster_px))
    except Exception:  # noqa: BLE001 - a statistic must never fail a score
        return {}
    return {side: {k: design.summary_value(k, c[side][k]) for k in design.STATS}
            for side in ("trace", "artist")} | {"div": c["div"]}


@dataclass(frozen=True)
class Job:
    """One icon to trace and score; picklable, so it is what the pool's workers receive.

    `tier` is set in the worker, so one pool serves every condition. `known` is a
    baseline's (SHA-256 prefix, {axis: value}) for the icon: when the new SVG's bytes have
    that hash, the icon is not scored again and takes those values (`ImageScore.reused`).
    `human` asks for the design battery."""

    exe: Path
    it: dict
    out_dir: Path
    extra_args: tuple = ()
    keep_svg: bool = False
    tier: str = ""
    known: tuple | None = None
    human: bool = False


def _trace(job: Job) -> tuple[dict | None, Path, float, subprocess.CompletedProcess | None]:
    """Run the tracer on one icon: (failure or None, output path, seconds, result)."""
    it = job.it
    out = Path(job.out_dir) / f"{it['corpus']}__{it['stem']}.svg"
    t0 = time.time()
    try:
        r = subprocess.run([str(job.exe), str(item_paths(it)[0]), "-o", str(out), "--quiet",
                            *job.extra_args], capture_output=True, timeout=TRACE_TIMEOUT)
    except subprocess.TimeoutExpired:
        return {"fail": f"{it['stem']}: timeout after {TRACE_TIMEOUT}s"}, out, 0.0, None
    dt = time.time() - t0
    if r.returncode != 0:
        return ({"fail": f"{it['stem']}: exit {r.returncode}: "
                         f"{r.stderr.decode('utf-8', 'replace')[-300:]}"}, out, dt, r)
    return None, out, dt, r


def score_svg(svg: str, it: dict, png: Path, gt: Path, render_path: Path | None = None) -> dict:
    """Every gate signal of one SVG against the artist's file: the scorer itself, a pure
    function of the two files (and the input raster, for `self_res`). Returns the signals,
    or {"fail": reason}. `render_path`, when given, receives the 1024 px render as a PNG
    (for DISTS, scored in the parent)."""
    from PIL import Image
    from inkvec_bench import render, svgmodel
    from inkvec_bench.metrics import color as mcolor
    try:
        b = render.composite(render.render(svg, JUDGE_SIZE, JUDGE_SIZE))
    except BaseException as e:  # resvg raises odd things on malformed output
        return {"fail": f"{it['stem']}: render {type(e).__name__}"}
    ref = gt_render(gt, it["corpus"], it["stem"])
    if render_path is not None:
        Image.fromarray((np.clip(b, 0, 1) * 255 + 0.5).astype(np.uint8)).save(render_path, optimize=False)
    sig = structure_signals(svg, png)
    try:
        sig.update(geometric_match(svg, gt, png))
    except BaseException as e:  # resvg raises odd things on malformed output
        return {"fail": f"{it['stem']}: geometric match {type(e).__name__}"}
    return dict(**sig, de00=float(mcolor.delta_e00(ref, b)["de00_mean"]),
                ratio=svgmodel.parse(svg).n_params / max(1, it["gt_params"]))


def score_one(job: Job) -> dict:
    """Trace + render + colour error for one icon. Top-level so a process pool can
    pickle it. DISTS is *not* computed here: it needs torch and a VGG, which would put
    ~1 GB and a slice of the GPU into every worker; the parent scores it from the saved
    render instead (`score_set`), unless `INKVEC_SKIP_DISTS=1` (the gate), when no render
    is saved at all.

    An icon whose SVG bytes match `job.known`'s hash is not scored: its scores are the
    known ones, exactly what scoring the same bytes again would give (the scorer is a pure
    function of the files), at the cost of the trace alone."""
    if job.tier:
        set_tier(job.tier)
    it = job.it
    png, gt = item_paths(it)
    if not png.exists() or not gt.exists():
        return {"fail": f"{it['stem']}: missing input"}
    c0 = _children_cpu()
    fail, out, dt, _ = _trace(job)
    if fail:
        return fail
    cpu = {"trace": _children_cpu() - c0}
    c0 = time.process_time()
    raw = out.read_bytes()                  # hashed as written ...
    svg = out.read_text(encoding="utf-8")   # ... and scored as before (newline-translated)
    sha = hashlib.sha256(raw).hexdigest()
    res = {"stem": it["stem"], "corpus": it["corpus"], "seconds": dt,
           "svg": svg if job.keep_svg else "", "sha256": sha}
    if job.known and job.known[0] and sha.startswith(job.known[0]):
        res.update(job.known[1], dists=float("nan"), mirror=float("nan"), reused=True)
    else:
        want_dists = os.environ.get("INKVEC_SKIP_DISTS") != "1"
        rp = Path(job.out_dir) / f"{it['corpus']}__{it['stem']}.render.png" if want_dists else None
        sig = score_svg(svg, it, png, gt, rp)
        if "fail" in sig:
            return sig
        res.update(sig, dists=0.0)
        if rp is not None:
            res["_render"] = str(rp)
            res["_gt"] = str(CACHE / f"gt{JUDGE_SIZE}" / it["corpus"] / f"{it['stem']}.png")
    cpu["score"] = time.process_time() - c0
    if job.human:
        from PIL import Image
        c0 = time.process_time()
        with Image.open(png) as im:
            res["human"] = human_stats(svg, gt, im.size[0])
        cpu["human"] = time.process_time() - c0
    res["cpu"] = cpu
    if not job.keep_svg:
        out.unlink(missing_ok=True)
    return res


def _children_cpu() -> float:
    """CPU seconds of every child process this one has waited for (the tracer)."""
    try:
        import resource
    except ImportError:  # Windows: no rusage; the breakdown reads 0 there
        return 0.0
    r = resource.getrusage(resource.RUSAGE_CHILDREN)
    return r.ru_utime + r.ru_stime


def _dists_in_parent(results: list[dict]) -> None:
    """DISTS for every scored icon, in this process only: one VGG, on the GPU when there
    is one. Reads the renders the workers saved and deletes them."""
    skip = os.environ.get("INKVEC_SKIP_DISTS") == "1"
    from PIL import Image
    if not skip:
        from inkvec_bench.metrics import raster
    for res in results:
        if "fail" in res or "_render" not in res:
            continue
        rp, gp = Path(res.pop("_render")), Path(res.pop("_gt"))
        if not skip:
            b = np.asarray(Image.open(rp).convert("RGB"), dtype=np.float32) / 255.0
            ref = np.asarray(Image.open(gp).convert("RGB"), dtype=np.float32) / 255.0
            res["dists"] = float(raster.dists_distance(ref, b))
        rp.unlink(missing_ok=True)


@contextmanager
def scoring_pool(workers: int):
    """A process pool of `workers` scoring workers, one core each, for `score_set(pool=...)`:
    one pool for every condition of a run, so each worker imports the scorer once.

    BLAS and rayon threads are pinned in the *parent* environment for the pool's life:
    children inherit it and numpy reads it at import, which happens before any initializer
    runs. Setting it only in the initializer is too late; each worker then starts a
    full-width OpenBLAS pool, and 12 workers x 16 threads took a 4-minute scoring pass to
    three hours (measured 2026-09-02)."""
    saved = {k: os.environ.get(k) for k in PIN_VARS}
    for k in PIN_VARS:
        os.environ[k] = "1"
    pool = ProcessPoolExecutor(max_workers=max(1, workers), initializer=_init_worker)
    try:
        yield pool
    finally:
        pool.shutdown()
        for k, v in saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


def score_set(exe: Path, name: str, items: list[dict], out_dir: Path, extra_args=(),
              keep_svgs: bool = False, workers: int = 1, use_cache: bool = True,
              known: dict | None = None, human: bool = False, pool=None) -> SetScore:
    """Trace and score `items`. `known` maps `family/stem` to a baseline's (SHA-256 prefix,
    {axis: value}): those icons are scored only if their SVG bytes changed (`score_one`).
    `human` adds the design battery to every icon. `pool` is a `scoring_pool` to run in
    (else one is made for this set when `workers` > 1)."""
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
    known = known or {}
    jobs = [Job(exe, it, out_dir, tuple(extra_args), keep_svgs, tier(),
                known.get(f"{it['corpus']}/{it['stem']}"), human) for it in items]
    if pool is not None and jobs:
        results = list(pool.map(score_one, jobs, chunksize=1))
    elif workers <= 1 or len(jobs) < 2:
        # Serial = the tracer runs the way a user runs it, with its own rayon pool on
        # every core. That is the wall time the sub-5 s rule is about (the canary);
        # single-threaded times are 5-10x longer and would trip the gate on nothing.
        results = list(map(score_one, jobs))
    else:
        with scoring_pool(min(workers, len(jobs))) as p:
            results = list(p.map(score_one, jobs, chunksize=1))
    _dists_in_parent(results)
    if cached or (use_cache and not keep_svgs
                  and os.environ.get("INKVEC_NO_SCORE_CACHE") != "1"):
        fresh = {
            f"{r['corpus']}/{r['stem']}": {k: r[k] for k in CACHE_FIELDS}
            for r in results if "fail" not in r and not r.get("reused")
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
