# M1 — progress

**Increment 1: exact predicates (S3) + the optimal-polygon dynamic program (S4), single-model.**
Status: built, tested, and it produced one result that overturns a conclusion from M0.

Reproduce:
```
cargo test --workspace
cargo run --release --example lambda_sweep -p inkvec-fit
cargo run --release --example robustness  -p inkvec-fit
```

---

## 1. What was built

**`inkvec-core`** — `Point`, `Vec2`, and `Polyline`. The load-bearing detail is that a
`Polyline` carries a **per-point positional uncertainty** `sigma[k]`, not just positions.
Everything downstream reads its threshold from that field instead of from a tuned
constant.

**`inkvec-core::predicates`** — Shewchuk adaptive-precision orientation, in-circle and
exact segment intersection, via the `robust` crate. Topology decisions are exact;
coordinates stay `f64`.

**`inkvec-fit`** — the optimal-polygon DP, generalized off the integer lattice:

* **Straightness.** Potrace asks whether every point lies within L∞ distance 1 of the
  chord — "1" being one pixel, the only length scale an integer lattice offers. We ask
  whether each point is *statistically consistent* with the chord: `|d_k| ≤ τ·σ_k`.
  Implemented as incremental **cone intersection** — each interior point constrains the
  admissible chord direction to an angular interval of half-width `asin(τσ_k/|w_k|)`, and
  the running intersection only shrinks, so the scan stops as soon as it empties. O(1)
  per extension.
* **Cost.** The MDL objective of DESIGN.md §5.0 directly — `0.5·χ² + λ·params` — rather
  than Potrace's lexicographic (segment count, penalty). Kept O(1) per candidate segment
  by six weighted prefix sums, which is Potrace's constant-time penalty trick generalized
  to per-point weights.

Net: O(n²) per boundary, and **verified globally optimal** against exhaustive enumeration
on small inputs. That test matters — a DP that quietly returned a local optimum would be
worse than an honest greedy method, because we would trust it.

## 2. Adaptive simplification falls out of the model

The `σ`-based straightness test means detail concentrates where the boundary was actually
well localized. Given a synthetic boundary whose first half is well-localized (σ=0.05) and
whose second half is faint (σ=1.5), vertices concentrate more than 2:1 in the confident
half — with no separate heuristic and no extra knob. This is the capability Vectorizer.AI
advertises as "adaptive simplification", arriving as a consequence of the measurement
model rather than as a feature.

## 3. λ was mis-set, and it mattered

First run of the greedy-vs-DP comparison, at `λ = 1.0`, showed greedy emitting 22 segments
for a circle against the "optimal" DP's 43. That reads as the DP losing.

It was not. The two minimize different things: greedy minimizes segment *count* subject to
admissibility, the DP minimizes `0.5·χ² + λ·params`. At `λ=1` a segment costs 2.0 nats, so
the DP buys one whenever it removes more than 2.0 nats of residual. The output was not
worse — the exchange rate was wrong.

This is exactly the failure mode flagged in DESIGN.md §9.6: **a mis-set λ looks like a
pipeline defect while actually being a units problem.** It appeared within an hour of the
DP existing.

**Fix — derive it.** MDL counts description length in nats; a coordinate confined to
range `R` and written at precision `δ` costs `ln(R/δ)` nats. For a 256px canvas at 0.1px
precision that is `ln(2560) ≈ 7.85`. Now `FitConfig::from_precision(extent, precision, tau)`,
with `Default` at those values. λ is tied to two things that are known rather than tuned:
how big the image is, and how precisely we intend to write numbers.

Segment count against λ (circle, 300 samples, σ=0.5, τ=2):

| λ | 0.5 | 1.0 | 2.0 | 4.0 | **7.8** | 16 | 32 | 64 |
|---|---|---|---|---|---|---|---|---|
| circle | 50 | 43 | 37 | 33 | **27** | 23 | 22 | 22 |
| star5 | 70 | 64 | 49 | 45 | **40** | 40 | 40 | 40 |

The derived value lands just above the admissibility floor (22 / 40) — a sensible
operating point rather than an extreme. The derivation is doing real work.

## 4. Correction to M0-BASELINE §5

**M0 concluded:** Potrace's 0.0% parameter growth under boundary perturbation, against
VTracer's +18.0%, was caused by the optimal-polygon dynamic program that VTracer dropped.

**That conclusion is not supported.** It was inferred from two programs that differ in
many ways at once. The controlled version of the experiment holds the admissibility rule,
the cost function and the inputs fixed, and varies *only* locality:

| shape | greedy growth | DP growth |
|---|---:|---:|
| circle | 0.0% | 3.7% |
| rounded_rect | 7.1% | 0.0% |
| star5 | 0.0% | 0.0% |
| **median** | **0.0%** | **0.0%** |

Greedy is just as stable as the DP. **Non-locality is not what buys noise robustness.**
The more likely cause is the *tolerance model* — Potrace's "stay within one unit of the
chord", and our `τ·σ` generalization of it — which absorbs smooth perturbation because a
displaced boundary still fits inside the same envelope. VTracer's local curve-fitting
heuristics have no such envelope, so they track the wobble.

This does not weaken the case for the DP; it re-aims it. Held to a matched segment budget,
the DP places its vertices measurably better:

| shape | segments | greedy χ² | DP χ² | residual removed | greedy max dev | DP max dev |
|---|---:|---:|---:|---:|---:|---:|
| circle | 22 | 584.5 | 534.7 | **8.5%** | 1.93 | 1.93 |
| star5 | 41 | 442.7 | 266.7 | **39.8%** | 1.91 | 1.61 |

The advantage concentrates where geometry has structure — corners, varying curvature —
and nearly vanishes on a featureless circle. That is the right shape of result for this
project: logo and icon content is *made* of corners and varying curvature.

**Consequences for the design:**

1. DESIGN.md §9.4 claimed the DP was justified by robustness. Amended: it is justified by
   **fit quality at a fixed parameter budget**, and by being the structure the multi-model
   alphabet (arc, elliptical arc, cubic) plugs into without redesign.
2. The `τ·σ` admissibility envelope is more important than it looked, and it is a
   *cheap* mechanism. Worth checking whether it alone closes most of the gap to VTracer
   before the expensive stages are built.
3. The σ field must be real for any of this to hold. Everything so far runs on uniform
   σ, which is a stand-in. S2 is the next increment and the claims above are provisional
   until it exists.

## 5. Not yet done

- **S2** — sub-pixel boundary recovery. Until it exists, `σ` is uniform and the whole
  adaptive story is unexercised on real images.
- **The closed-loop case is approximate.** We cut at the point farthest from the centroid
  and solve the open problem. The true optimum is a minimum-cost *cycle*; the cut can cost
  one extra segment. Potrace solves this properly and so should we.
- **Multi-model alphabet** — arcs, elliptical arcs, cubics. The DP is structured to take
  them; only line segments are implemented.
- **Potrace's endpoint clipping** — it extends each candidate subpath by one point at each
  end before testing straightness, which improves corner placement. Not yet replicated.
- **No end-to-end path.** Nothing connects an image to this code yet, so none of it has
  been through `inkvec-bench`. The numbers above are on synthetic polylines.
