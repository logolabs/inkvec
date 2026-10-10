# Keeping the proofs and the code from drifting apart

> A decision record. The theorems in [`formal/InkvecTheory`](../../formal/InkvecTheory/README.md)
> are about mathematical objects; the engine is Rust. Tests and CI checks run beside the
> code but do not link it to the proofs: they sample. This page compares the ways of linking
> them so that a divergence is a build failure, and chooses one.

## The requirement

1. **No drift.** A change to the code that breaks what a theorem promises must fail the
   build, not wait for a test to sample it.
2. **No limit on algorithms.** Solvers, searches and approximations stay free to change,
   in any style, with floating point.
3. **Speed.** No runtime or portability cost that the WebAssembly, Python, Go and other
   bindings would carry.

## The options

| | How proof and code are linked | Floats and fast approximations | Algorithms allowed | Runtime cost | Proof effort | Tooling |
|---|---|---|---|---|---|---|
| **A. Tests only** (until now) | Not linked; tests sample | Yes | Any | None | None | In place |
| **B. Lean is the source, Rust generated** | The Rust file is generated; a hand edit fails CI | No (exact arithmetic or rigorous intervals only) | Straight-line formulas, small exact kernels | Small kernels only | Moderate | No new tools |
| **C. Lean compiled to C, linked into Rust** | The proved function is the running function | Yes, but proofs about floats are impractical | Any, written in Lean | 2-10× slower than Rust (an estimate); the Lean runtime in every binding | Moderate to high | The Lean runtime in WebAssembly, Python and Go: very hard |
| **D. Aeneas** (Rust translated to Lean) | Proofs about the actual Rust; a change breaks them | Floats allowed but opaque to the proofs | A subset of Rust in verified functions | None (it is the Rust) | High | OCaml, a pinned Lean and Mathlib (~8 GB), access to its repository |
| **E. Verus** (proofs inside the Rust) | The compiler checks code and proof together | Weak float support | The Verus dialect in verified functions | None | Moderate for integer logic; real analysis impractical | One more toolchain |
| **F. Certified results** (free solvers, a proved checker) | Every result is checked by proved code before it is used | Yes, anywhere in the solvers | Any | One exact check per accepted result | Low to moderate (checking is easier than solving) | No new tools (the checker by B) |

F is the method of *certifying algorithms*: a solver returns its answer with a certificate,
and a small checker, proved correct once, decides whether the answer is accepted. McConnell,
Mehlhorn, Näher, Schweitzer (2011), Certifying algorithms, *Computer Science Review* 5(2),
doi:10.1016/j.cosrev.2010.09.009; with verified checkers, Alkassar, Böhme, Mehlhorn,
Rizkallah (2014), A framework for the verification of certifying computations, *Journal of
Automated Reasoning* 52, doi:10.1007/s10817-013-9289-2.

## The evaluation

Scores 1-5; a link of at least 4 is required, since it is the point.

| Criterion (weight) | A | B | C | D | E | **F, checker by B** | F, checker by D |
|---|---|---|---|---|---|---|---|
| Proof and code cannot drift (25) | 1 | 5 | 5 | 5 | 5 | 4 | 4.5 |
| Any algorithm (20) | 5 | 2 | 3 | 3 | 3 | **5** | 5 |
| Speed, floats (20) | 5 | 3 | 2 | 5 | 5 | **5** | 5 |
| Proof effort (15) | 5 | 4 | 3 | 2 | 3 | **4** | 3 |
| Tooling and portability (10) | 5 | 4 | 1 | 2 | 3 | **4** | 2 |
| Reuses the Lean theorems (10) | 1 | 5 | 5 | 4 | 1 | **5** | 5 |
| **Weighted, of 100** | fails the link | 75 | 66 | 75 | 74 | **90** | 86 |

## The decision: F, with checkers generated from Lean

* **Solvers are free.** Fitting, the dynamic programs, the stroke solve, the searches: any
  algorithm, floating point, heuristics, approximations.
* **Results are not.** Every result a theorem speaks about (a constraint accepted, a merge
  kept, a shape completed behind another, a coordinate snapped, a description scored) is
  used only if a checker accepts it, on a certificate the solver emits.
* **Checkers are generated from Lean.** A checker is a Lean definition in a small
  expression language that Lean can both reason about and print as Rust; the theorem is
  about that definition, and the Rust is generated from it into files marked as generated,
  which CI regenerates and compares. A hand edit cannot survive.
* **Checkers are exact.** The pixels are 8-bit integers, so window sums are exact
  integers; every `f64` is an exact rational, so the areas of lines and cubics are exact
  polynomials in a solver's numbers; square roots and angles (arcs) are bounded by interval
  arithmetic with outward rounding. The one trusted assumption, stated as an axiom in Lean,
  is IEEE 754's: each floating-point operation returns the correctly rounded result, so its
  value widened by one unit in the last place on each side contains the exact one.

What this guarantees: every result the engine uses satisfies what the theorems say, so a
solver change cannot break a proved property; at worst a result is refused. What it does
not: that the solver finds the best result. That is the gate's job.

A side effect: decisions made in exact arithmetic are the same on every platform. Linux and
Windows outputs differ today on 7 of the 12 cross-language contract cases, from floating
point.

Option D can be added later for hand-written Rust kernels, where a proof about the code
itself is worth its toolchain.
