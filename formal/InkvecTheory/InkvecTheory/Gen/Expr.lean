/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import Mathlib.Analysis.SpecialFunctions.Pow.Real
import Mathlib.Analysis.SpecialFunctions.Sqrt

/-!
# Checkers generated from the definitions the theorems are about

The engine's solvers are free (floating point, heuristics, any algorithm); a result a theorem
speaks about is used only if a checker accepts it (`docs/theory/verification.md`). This file
is the bridge: a small expression language that Lean can reason about and print as Rust.

* `Expr`: straight-line arithmetic over numbered inputs (`+ − × ÷`, `√`, rational
  literals). A kernel is an `Expr`; its theorem is about `Expr.evalR`.
* `Expr.evalQ`: exact rational evaluation, for inputs that are integers or rationals (sums
  of 8-bit pixels, rational stencils). Agrees with `evalR` wherever it returns a value
  (`evalQ_sound`).
* `Expr.evalI`: interval evaluation through any implementation of the interval operations
  that encloses the exact ones (`Enclosing`). Whatever it returns contains the true value
  (`evalI_sound`): the checker's interval holds the real number the theorem is about.

The Rust runtime (`crates/inkvec-verified`) implements the interval operations in `f64`
with each result widened outward by one unit in the last place. That it satisfies
`Enclosing` is the one trusted assumption, IEEE 754's: `+ − × ÷ √` return the correctly
rounded result, which lies within half a unit in the last place of the exact one, so the
neighbouring floats on either side bracket it. It is stated here as a hypothesis of
`evalI_sound`, not as an axiom of the library, so the audit stays at Lean's three.
-/

namespace Inkvec.Gen

/-- Straight-line arithmetic over the inputs `x 0, x 1, …`. -/
inductive Expr where
  | lit (q : ℚ)
  | var (i : ℕ)
  | neg (a : Expr)
  | add (a b : Expr)
  | sub (a b : Expr)
  | mul (a b : Expr)
  | div (a b : Expr)
  | sqrt (a : Expr)
  deriving Inhabited

namespace Expr

/-- The real value: what the theorems are about. -/
noncomputable def evalR (x : ℕ → ℝ) : Expr → ℝ
  | lit q => q
  | var i => x i
  | neg a => -evalR x a
  | add a b => evalR x a + evalR x b
  | sub a b => evalR x a - evalR x b
  | mul a b => evalR x a * evalR x b
  | div a b => evalR x a / evalR x b
  | sqrt a => Real.sqrt (evalR x a)

/-- The exact rational value, or `none` for a division by zero or a square root (which the
rational checker refuses). -/
def evalQ (x : ℕ → ℚ) : Expr → Option ℚ
  | lit q => some q
  | var i => some (x i)
  | neg a => (evalQ x a).map (fun u => -u)
  | add a b => do let u ← evalQ x a; let v ← evalQ x b; pure (u + v)
  | sub a b => do let u ← evalQ x a; let v ← evalQ x b; pure (u - v)
  | mul a b => do let u ← evalQ x a; let v ← evalQ x b; pure (u * v)
  | div a b => do
      let u ← evalQ x a
      let v ← evalQ x b
      if v = 0 then none else pure (u / v)
  | sqrt _ => none

/-- **The rational checker computes the real value.** -/
theorem evalQ_sound (x : ℕ → ℚ) :
    ∀ (e : Expr) (q : ℚ), evalQ x e = some q → (q : ℝ) = evalR (fun i => (x i : ℝ)) e := by
  intro e
  induction e with
  | lit r => intro q h; simp [evalQ] at h; simp [evalR, h]
  | var i => intro q h; simp [evalQ] at h; simp [evalR, h]
  | neg a ha =>
      intro q h
      simp only [evalQ, Option.map_eq_some_iff] at h
      obtain ⟨u, hu, rfl⟩ := h
      simp [evalR, ha u hu]
  | add a b ha hb =>
      intro q h
      simp only [evalQ, Option.bind_eq_bind, Option.bind_eq_some_iff, Option.pure_def,
        Option.some.injEq] at h
      obtain ⟨u, hu, v, hv, rfl⟩ := h
      simp [evalR, ha u hu, hb v hv]
  | sub a b ha hb =>
      intro q h
      simp only [evalQ, Option.bind_eq_bind, Option.bind_eq_some_iff, Option.pure_def,
        Option.some.injEq] at h
      obtain ⟨u, hu, v, hv, rfl⟩ := h
      simp [evalR, ha u hu, hb v hv]
  | mul a b ha hb =>
      intro q h
      simp only [evalQ, Option.bind_eq_bind, Option.bind_eq_some_iff, Option.pure_def,
        Option.some.injEq] at h
      obtain ⟨u, hu, v, hv, rfl⟩ := h
      simp [evalR, ha u hu, hb v hv]
  | div a b ha hb =>
      intro q h
      simp only [evalQ, Option.bind_eq_bind, Option.bind_eq_some_iff] at h
      obtain ⟨u, hu, v, hv, h⟩ := h
      by_cases hv0 : v = 0
      · simp [hv0] at h
      · simp only [hv0, ite_false, Option.pure_def, Option.some.injEq] at h
        subst h
        simp [evalR, ha u hu, hb v hv]
  | sqrt a _ => intro q h; simp [evalQ] at h

end Expr

/-! ### Intervals -/

/-- A closed interval of reals; a checker's enclosure of a value. -/
structure Iv where
  lo : ℝ
  hi : ℝ

namespace Iv

/-- `v` lies in `I`. -/
def Mem (I : Iv) (v : ℝ) : Prop := I.lo ≤ v ∧ v ≤ I.hi

/-- `I` lies inside `J`. -/
def Sub (I J : Iv) : Prop := J.lo ≤ I.lo ∧ I.hi ≤ J.hi

theorem Mem.of_sub {I J : Iv} {v : ℝ} (h : I.Mem v) (s : I.Sub J) : J.Mem v :=
  ⟨s.1.trans h.1, h.2.trans s.2⟩

/-- The exact interval operations. -/
def neg (I : Iv) : Iv := ⟨-I.hi, -I.lo⟩
def add (I J : Iv) : Iv := ⟨I.lo + J.lo, I.hi + J.hi⟩
def sub (I J : Iv) : Iv := ⟨I.lo - J.hi, I.hi - J.lo⟩
noncomputable def mul (I J : Iv) : Iv :=
  ⟨min (min (I.lo * J.lo) (I.lo * J.hi)) (min (I.hi * J.lo) (I.hi * J.hi)),
   max (max (I.lo * J.lo) (I.lo * J.hi)) (max (I.hi * J.lo) (I.hi * J.hi))⟩
/-- The reciprocal, for an interval that excludes zero. -/
noncomputable def recip (I : Iv) : Iv := ⟨1 / I.hi, 1 / I.lo⟩
/-- The square root, for an interval of non-negative numbers. -/
noncomputable def sqrt (I : Iv) : Iv := ⟨Real.sqrt I.lo, Real.sqrt I.hi⟩

theorem mem_neg {I : Iv} {a : ℝ} (h : I.Mem a) : (neg I).Mem (-a) :=
  ⟨by simp [neg]; linarith [h.2], by simp [neg]; linarith [h.1]⟩

theorem mem_add {I J : Iv} {a b : ℝ} (ha : I.Mem a) (hb : J.Mem b) : (add I J).Mem (a + b) :=
  ⟨by simp [add]; linarith [ha.1, hb.1], by simp [add]; linarith [ha.2, hb.2]⟩

theorem mem_sub {I J : Iv} {a b : ℝ} (ha : I.Mem a) (hb : J.Mem b) : (sub I J).Mem (a - b) :=
  ⟨by simp [sub]; linarith [ha.1, hb.2], by simp [sub]; linarith [ha.2, hb.1]⟩

/-- `a·b` lies between the least and greatest product of the endpoints. -/
theorem mem_mul {I J : Iv} {a b : ℝ} (ha : I.Mem a) (hb : J.Mem b) : (mul I J).Mem (a * b) := by
  obtain ⟨ha1, ha2⟩ := ha
  obtain ⟨hb1, hb2⟩ := hb
  -- For fixed `b`, `t ↦ t·b` is monotone one way or the other, so `a·b` lies between
  -- `lo·b` and `hi·b`; each of those lies between the products with `J`'s endpoints.
  have key : ∀ t, I.lo ≤ t → t ≤ I.hi → ∀ s, J.lo ≤ s → s ≤ J.hi →
      min (min (I.lo * J.lo) (I.lo * J.hi)) (min (I.hi * J.lo) (I.hi * J.hi)) ≤ t * s ∧
      t * s ≤ max (max (I.lo * J.lo) (I.lo * J.hi)) (max (I.hi * J.lo) (I.hi * J.hi)) := by
    intro t ht1 ht2 s hs1 hs2
    have e1 : min (I.lo * s) (I.hi * s) ≤ t * s ∧ t * s ≤ max (I.lo * s) (I.hi * s) := by
      rcases le_total 0 s with h0 | h0
      · exact ⟨(min_le_left _ _).trans (mul_le_mul_of_nonneg_right ht1 h0),
          (mul_le_mul_of_nonneg_right ht2 h0).trans (le_max_right _ _)⟩
      · exact ⟨(min_le_right _ _).trans (mul_le_mul_of_nonpos_right ht2 h0),
          (mul_le_mul_of_nonpos_right ht1 h0).trans (le_max_left _ _)⟩
    have e2 : ∀ u, min (u * J.lo) (u * J.hi) ≤ u * s ∧ u * s ≤ max (u * J.lo) (u * J.hi) := by
      intro u
      rcases le_total 0 u with h0 | h0
      · exact ⟨(min_le_left _ _).trans (mul_le_mul_of_nonneg_left hs1 h0),
          (mul_le_mul_of_nonneg_left hs2 h0).trans (le_max_right _ _)⟩
      · exact ⟨(min_le_right _ _).trans (mul_le_mul_of_nonpos_left hs2 h0),
          (mul_le_mul_of_nonpos_left hs1 h0).trans (le_max_left _ _)⟩
    obtain ⟨l1, u1⟩ := e2 I.lo
    obtain ⟨l2, u2⟩ := e2 I.hi
    constructor
    · refine le_trans ?_ e1.1
      rcases min_choice (I.lo * s) (I.hi * s) with h | h <;> rw [h]
      · exact (min_le_left _ _).trans l1
      · exact (min_le_right _ _).trans l2
    · refine e1.2.trans ?_
      rcases max_choice (I.lo * s) (I.hi * s) with h | h <;> rw [h]
      · exact u1.trans (le_max_left _ _)
      · exact u2.trans (le_max_right _ _)
  exact key a ha1 ha2 b hb1 hb2

/-- The reciprocal of a number in an interval that excludes zero. -/
theorem mem_recip {I : Iv} {b : ℝ} (hb : I.Mem b) (h0 : 0 < I.lo ∨ I.hi < 0) :
    (recip I).Mem (1 / b) := by
  obtain ⟨h1, h2⟩ := hb
  rcases h0 with hp | hn
  · have hb0 : 0 < b := lt_of_lt_of_le hp h1
    exact ⟨one_div_le_one_div_of_le hb0 h2, one_div_le_one_div_of_le hp h1⟩
  · have hb0 : b < 0 := lt_of_le_of_lt h2 hn
    exact ⟨one_div_le_one_div_of_neg_of_le hn h2, one_div_le_one_div_of_neg_of_le hb0 h1⟩

theorem mem_sqrt {I : Iv} {a : ℝ} (ha : I.Mem a) : (sqrt I).Mem (Real.sqrt a) :=
  ⟨Real.sqrt_le_sqrt ha.1, Real.sqrt_le_sqrt ha.2⟩

end Iv

/-- An implementation of the interval operations, as the Rust runtime provides: each may
refuse (`none`: an overflow, a divisor interval that does not exclude zero, a square root of
an interval reaching below zero, a non-finite endpoint), and what it returns encloses the
exact operation on its arguments. -/
structure Enclosing where
  lit : ℚ → Option Iv
  neg : Iv → Option Iv
  add : Iv → Iv → Option Iv
  sub : Iv → Iv → Option Iv
  mul : Iv → Iv → Option Iv
  div : Iv → Iv → Option Iv
  sqrt : Iv → Option Iv
  lit_ok : ∀ q K, lit q = some K → K.Mem q
  neg_ok : ∀ I K, neg I = some K → (Iv.neg I).Sub K
  add_ok : ∀ I J K, add I J = some K → (Iv.add I J).Sub K
  sub_ok : ∀ I J K, sub I J = some K → (Iv.sub I J).Sub K
  mul_ok : ∀ I J K, mul I J = some K → (Iv.mul I J).Sub K
  div_ok : ∀ I J K, div I J = some K →
    (0 < J.lo ∨ J.hi < 0) ∧ (Iv.mul I (Iv.recip J)).Sub K
  sqrt_ok : ∀ I K, sqrt I = some K → 0 ≤ I.lo ∧ (Iv.sqrt I).Sub K

namespace Expr

/-- Interval evaluation through the implementation `E`, from enclosures `X` of the inputs. -/
def evalI (E : Enclosing) (X : ℕ → Iv) : Expr → Option Iv
  | lit q => E.lit q
  | var i => some (X i)
  | neg a => (evalI E X a).bind E.neg
  | add a b => do let I ← evalI E X a; let J ← evalI E X b; E.add I J
  | sub a b => do let I ← evalI E X a; let J ← evalI E X b; E.sub I J
  | mul a b => do let I ← evalI E X a; let J ← evalI E X b; E.mul I J
  | div a b => do let I ← evalI E X a; let J ← evalI E X b; E.div I J
  | sqrt a => (evalI E X a).bind E.sqrt

/-- **The interval checker encloses the real value.** Whenever the interval evaluation
returns an interval, the real value the theorems are about lies in it, given enclosures of
the inputs. -/
theorem evalI_sound (E : Enclosing) (X : ℕ → Iv) (x : ℕ → ℝ) (hx : ∀ i, (X i).Mem (x i)) :
    ∀ (e : Expr) (K : Iv), evalI E X e = some K → K.Mem (evalR x e) := by
  intro e
  induction e with
  | lit q => intro K h; exact E.lit_ok q K h
  | var i => intro K h; simp [evalI] at h; subst h; exact hx i
  | neg a ha =>
      intro K h
      simp only [evalI, Option.bind_eq_some_iff] at h
      obtain ⟨I, hI, hK⟩ := h
      exact (Iv.mem_neg (ha I hI)).of_sub (E.neg_ok I K hK)
  | add a b ha hb =>
      intro K h
      simp only [evalI, Option.bind_eq_bind, Option.bind_eq_some_iff] at h
      obtain ⟨I, hI, J, hJ, hK⟩ := h
      exact (Iv.mem_add (ha I hI) (hb J hJ)).of_sub (E.add_ok I J K hK)
  | sub a b ha hb =>
      intro K h
      simp only [evalI, Option.bind_eq_bind, Option.bind_eq_some_iff] at h
      obtain ⟨I, hI, J, hJ, hK⟩ := h
      exact (Iv.mem_sub (ha I hI) (hb J hJ)).of_sub (E.sub_ok I J K hK)
  | mul a b ha hb =>
      intro K h
      simp only [evalI, Option.bind_eq_bind, Option.bind_eq_some_iff] at h
      obtain ⟨I, hI, J, hJ, hK⟩ := h
      exact (Iv.mem_mul (ha I hI) (hb J hJ)).of_sub (E.mul_ok I J K hK)
  | div a b ha hb =>
      intro K h
      simp only [evalI, Option.bind_eq_bind, Option.bind_eq_some_iff] at h
      obtain ⟨I, hI, J, hJ, hK⟩ := h
      obtain ⟨h0, hs⟩ := E.div_ok I J K hK
      have hm := Iv.mem_mul (ha I hI) (Iv.mem_recip (hb J hJ) h0)
      simp only [evalR]
      rw [div_eq_mul_one_div]
      exact hm.of_sub hs
  | sqrt a ha =>
      intro K h
      simp only [evalI, Option.bind_eq_some_iff] at h
      obtain ⟨I, hI, hK⟩ := h
      obtain ⟨_, hs⟩ := E.sqrt_ok I K hK
      exact (Iv.mem_sqrt (ha I hI)).of_sub hs

/-- **A checker's verdict.** If the interval evaluation of `e` returns an interval whose
lower end is at least zero, the real value is at least zero: a condition `e ≥ 0` a theorem
needs holds. A straddling interval is no verdict, and the checker refuses. -/
theorem nonneg_of_evalI (E : Enclosing) (X : ℕ → Iv) (x : ℕ → ℝ) (hx : ∀ i, (X i).Mem (x i))
    (e : Expr) (K : Iv) (h : evalI E X e = some K) (hlo : 0 ≤ K.lo) : 0 ≤ evalR x e :=
  hlo.trans (evalI_sound E X x hx e K h).1

end Expr

end Inkvec.Gen
