"""Paired comparison of two scorings of the same icons, and the gate's decision from it.

The problem, in standard terms
------------------------------
The regression gate scores a candidate build and a baseline build on the same fixed set of
icons and must decide whether the candidate is *not worse* on each axis (dE00, anchor
turning, parameter ratio). That is a paired two-sample comparison with a one-sided
**non-inferiority** question: is the true relative change below an allowed margin?

The score itself is deterministic (identical SVGs score bit-identically, 246/246 under
load; r2-eval 2026-10-02), so the uncertainty is not run-to-run noise. It is which icons
happen to be in the set, and how chaotically the engine answers a change that should not
matter (mirroring an icon moves its 512 px dE00 by 34 % of the mean, per icon). A bare
threshold on the set mean ("may rise 1 %") ignores both: the 0.2.4 boundary solve read
-8.5 % at 128 px with a 95 % interval of [-12.1, -5.1] %, and -0.9 % [-4.2, +2.1] % at
512 px, where the same "gain" cannot be told from zero.

What this computes
------------------
Data layout: two dicts keyed by `family/stem`, the baseline's and the candidate's value per
icon on one axis, plus each icon's family. Only icons present in both are compared.

1. **Aggregate.** The gate's number for a set: the *macro* mean (each family's plain mean,
   then the mean of those, so every family has one vote) or the *micro* mean (the plain
   mean over icons). dE00 and the parameter ratio use macro, turning uses micro, as
   `bench/svgeval.py` and the 09-19 gate defined them.
2. **Relative change.** r = A(candidate) / A(baseline) - 1, with A the aggregate.
3. **Paired, family-stratified bootstrap** of r (Koehn 2004). Resample icons with
   replacement *within each family*, keeping each family's size, and take the same resampled
   icons from both builds (that is what makes it paired: an icon's own difficulty cancels).
   Recompute both aggregates on the resample and their ratio, `reps` times. Stratifying by
   family keeps the macro mean defined on every resample (no family can vanish) and matches
   how the set was drawn (a fixed quota per family). The percentile interval of the
   replicates is the confidence interval.
4. **Decision** (`decide`), by the one-sided 95 % upper bound of the change (the 95th
   percentile of the replicates), against the *effective* margin
   `max(margin, mde)`, the requested margin floored at what this set can resolve:
     identical     no icon's value changed                          -> pass
     better        upper bound < 0 (a demonstrable gain)            -> pass
     non-inferior  upper bound < margin                             -> pass
     within-noise  margin <= upper bound < mde (the set cannot      -> pass
                   resolve the margin, and the change is not
                   detectably worse)
     worse         upper bound >= max(margin, mde)                  -> FAIL
   There is no "inconclusive" verdict. A plain non-inferiority test fails whenever the
   interval straddles the margin, and with 246 icons a broad edit straddles a 1 % margin at
   512 px whatever its true effect (its `mde` is about 4 %). That turned real gains into
   failures (the 0.2.4 boundary solve: -0.9 % at 512 px, upper bound +1.37 %). Flooring the
   margin at the minimum detectable effect keeps the requested margin wherever the data can
   test it (narrow edits, whose `mde` is small) and, where it cannot, fails only a change
   whose upper bound reaches the smallest rise the set would catch four times in five.
   The error rates this buys, for a broad edit at an under-resolved condition: a true rise
   of `mde` or more fails at least 95 % of the time; a change with no true effect passes
   about 80 % of the time. The report marks every within-noise pass, so a reviewer sees
   where the requested margin was not testable.
5. **Minimum detectable effect.** `mde` = (z_0.95 + z_0.80) x SE, the smallest true rise
   past zero that a one-sided 5 % test on this many icons would catch four times in five,
   with SE the bootstrap standard deviation of r. It says what the set can resolve: a
   margin below it is not testable with this data (Card et al. 2020).

Complexity: O(reps x n) time and O(reps x max family size) memory per call; 10,000 replicates
of 246 icons take well under a second.

Edge cases: an empty pairing raises; a baseline aggregate of exactly 0 makes r undefined, so
it is reported as 0 when the candidate is also 0 and +inf otherwise (and fails); a family of
one icon resamples to itself, contributing no spread.

Literature
----------
Method from: Koehn 2004, "Statistical Significance Tests for Machine Translation Evaluation",
  EMNLP 2004, https://aclanthology.org/W04-3250 -- paired bootstrap resampling of a fixed test
  set. Adapted: stratified by family (the set is drawn per family and the gate's dE00 is a
  family-macro mean), and the statistic is the ratio of aggregates, not a difference of
  sentence-level scores.
Method from: Lakens 2017, "Equivalence Tests: A Practical Primer for t Tests, Correlations,
  and Meta-Analyses", Soc. Psych. Pers. Sci. 8(4):355-362, PMC5502906 -- one-sided tests
  against a margin, read off the 90 % interval (the TOST convention). Adapted: only the upper
  side matters (non-inferiority, not equivalence), and the interval is a bootstrap one.
Method from: Card, Henderson, Khandelwal, Jia, Mahowald, Jurafsky 2020, "With Little Power
  Comes Great Responsibility", EMNLP 2020, https://aclanthology.org/2020.emnlp-main.745 --
  report the minimum detectable effect next to every comparison.
Not from the literature: flooring the margin at the comparison's own minimum detectable
  effect (`decide`), because the project owner wants a strict 1 % margin and no
  "inconclusive" verdicts, and the screen set cannot test 1 % for broad edits at 512 px.
  See also: Lakens 2017 (above) on choosing a smallest effect size of interest that the
  design can actually detect.
Inspired by: Blum, Hardt 2015, "The Ladder: A Reliable Leaderboard for Machine Learning
  Competitions", arXiv 1502.04585 -- move the recorded best only when a submission beats it
  by a margin. Here the "better" label (a gain whose whole one-sided interval is below
  zero) is the only one that may re-baseline; the Ladder's own threshold is a fixed step.
See also: Dror, Baumer, Shlomov, Reichart 2018, "The Hitchhiker's Guide to Testing
  Statistical Significance in NLP", ACL 2018, https://aclanthology.org/P18-1128 (choosing
  the test); Dwork et al. 2015, "The reusable holdout", Science 349,
  https://doi.org/10.1126/science.aaa9375 (why one screen set reused for every merge drifts
  towards false discoveries -- a reason to keep the margin and the set fixed).
Rejected: a paired t-test or Wilcoxon signed-rank test on per-icon deltas. Neither handles
  the family-macro aggregate or the ratio statistic directly, and per-icon deltas at 512 px
  are heavy-tailed (a few icons move by 0.2 dE00 under a mirror); the bootstrap needs no
  distributional assumption.
"""
from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np

#: Bootstrap replicates. The 5th and 95th percentiles of 10,000 replicates have a Monte
#: Carlo error of about 0.2 % of the interval width; Koehn used 1,000.
REPS = 10_000
#: Fixed seed: the same two scorings always give the same verdict.
SEED = 20261002
#: Standard normal quantiles for a one-sided 5 % test with 80 % power.
Z_95 = 1.6448536269514722
Z_80 = 0.8416212335729143
#: A per-icon change smaller than this counts as a tie in the better/worse counts. It is
#: only a reporting band; the decision uses the bootstrap, not the counts.
TIE = 1e-4

AGGREGATES = ("macro", "micro")


@dataclass(frozen=True)
class Comparison:
    """One axis, one condition: baseline against candidate on the icons both scored."""

    n: int              # icons paired
    base: float         # aggregate of the baseline over the paired icons
    cur: float          # aggregate of the candidate over the same icons
    rel: float          # cur / base - 1, the point estimate
    p025: float         # percentiles of the bootstrap distribution of rel
    p05: float
    p95: float
    p975: float
    se: float           # bootstrap standard deviation of rel
    changed: int        # icons whose value differs at all
    better: int         # icons lower by more than TIE
    worse: int          # icons higher by more than TIE

    @property
    def mde(self) -> float:
        """Smallest true relative rise a one-sided 5 % test catches with 80 % power."""
        return (Z_95 + Z_80) * self.se


@dataclass(frozen=True)
class Verdict:
    label: str          # identical | better | non-inferior | within-noise | worse
    passed: bool
    margin: float
    #: The margin actually applied, max(margin, mde); equal to `margin` unless the set could
    #: not resolve it. NaN for the verdicts that do not compare against a margin.
    effective: float = math.nan


def _ratio(cur: float, base: float) -> float:
    """r = cur / base - 1, defined as 0 for 0/0 and +inf for x/0 with x > 0."""
    if base == 0.0:
        return 0.0 if cur == 0.0 else math.inf
    return cur / base - 1.0


def compare(base: dict[str, float], cur: dict[str, float], family: dict[str, str],
            aggregate: str = "macro", reps: int = REPS, seed: int = SEED) -> Comparison:
    """Paired, family-stratified bootstrap of the relative change of an aggregate.

    `base` and `cur` map an icon key to its value on one axis; `family` maps the key to its
    family. Keys present in only one of `base` / `cur` are ignored (the caller reports
    them). `aggregate` is "macro" (mean of family means) or "micro" (mean over icons).

    For each family f with n_f paired icons, `reps` index vectors of length n_f are drawn
    uniformly with replacement; the same vector selects from both builds. Family means of
    the resample give, per replicate, A_base* and A_cur* (macro: the mean of the family
    means; micro: their n_f-weighted mean, which equals the plain mean because the family
    sizes are fixed), and r* = A_cur* / A_base* - 1. Returns the point estimate on the
    observed icons and the percentiles and standard deviation of r*.
    """
    if aggregate not in AGGREGATES:
        raise ValueError(f"aggregate must be one of {AGGREGATES}, got {aggregate!r}")
    keys = sorted(set(base) & set(cur))
    if not keys:
        raise ValueError("no icon was scored by both builds")
    fams: dict[str, list[str]] = {}
    for k in keys:
        fams.setdefault(family[k], []).append(k)
    order = sorted(fams)
    b_by = [np.array([base[k] for k in fams[f]], dtype=np.float64) for f in order]
    c_by = [np.array([cur[k] for k in fams[f]], dtype=np.float64) for f in order]
    sizes = np.array([len(fams[f]) for f in order], dtype=np.float64)

    def combine(means: np.ndarray) -> np.ndarray:
        # means: (families, ...) -> aggregate over axis 0.
        if aggregate == "macro":
            return means.mean(axis=0)
        return (means * sizes.reshape((-1,) + (1,) * (means.ndim - 1))).sum(axis=0) / sizes.sum()

    a_base = float(combine(np.array([v.mean() for v in b_by])))
    a_cur = float(combine(np.array([v.mean() for v in c_by])))
    rel = _ratio(a_cur, a_base)

    d = np.array([cur[k] - base[k] for k in keys], dtype=np.float64)
    changed = int((d != 0.0).sum())
    better, worse = int((d < -TIE).sum()), int((d > TIE).sum())
    if changed == 0:
        return Comparison(len(keys), a_base, a_cur, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0, 0, 0)

    rng = np.random.default_rng(seed)
    mb = np.empty((len(order), reps))
    mc = np.empty((len(order), reps))
    for i, (vb, vc) in enumerate(zip(b_by, c_by)):
        idx = rng.integers(0, len(vb), size=(reps, len(vb)))
        mb[i] = vb[idx].mean(axis=1)
        mc[i] = vc[idx].mean(axis=1)
    ab, ac = combine(mb), combine(mc)
    with np.errstate(divide="ignore", invalid="ignore"):
        r = np.where(ab == 0.0, np.where(ac == 0.0, 0.0, np.inf), ac / ab - 1.0)
        p025, p05, p95, p975 = (float(x) for x in np.percentile(r, [2.5, 5.0, 95.0, 97.5]))
    se = float(np.std(r[np.isfinite(r)], ddof=1)) if np.isfinite(r).sum() > 1 else math.inf
    return Comparison(len(keys), a_base, a_cur, rel, p025, p05, p95, p975, se,
                      changed, better, worse)


def decide(c: Comparison, margin: float, step: float = 0.0) -> Verdict:
    """The verdict for one comparison at a relative `margin` (0.01 = 1 %).

    Read off the one-sided 95 % upper bound (the 95th percentile; Lakens 2017) against the
    effective margin max(`margin`, `c.mde`), the requested margin floored at what this set
    can resolve (see the module docstring, step 4). identical, better (upper bound below
    -`step`), non-inferior (upper bound below `margin`) and within-noise (upper bound below
    the minimum detectable effect, when that exceeds `margin`) pass; worse (upper bound at
    or past the effective margin) fails. There is no inconclusive verdict. With a margin of
    +inf every finite comparison passes, which is how an axis is reported without being
    gated. Ties at a bound fail: the rule needs the bound strictly inside.

    `step` is the Ladder's threshold (Blum & Hardt 2015): "better", the only verdict that may
    move a baseline, needs the whole one-sided interval below -step. Without it, two icons
    that each improve by 1e-5 make a degenerate interval just below zero and read as a
    demonstrable gain (seen when Linux and Windows builds of v0.2.4 were compared).
    """
    if c.changed == 0:
        return Verdict("identical", True, margin)
    if c.p95 < -step:
        return Verdict("better", True, margin)
    if c.p95 < margin:
        return Verdict("non-inferior", True, margin, margin)
    # Floor the margin at the minimum detectable effect: where the set cannot resolve the
    # requested margin, only a change it would detect fails (module docstring, step 4).
    effective = max(margin, c.mde)
    if c.p95 < effective:
        return Verdict("within-noise", True, margin, effective)
    return Verdict("worse", False, margin, effective)
