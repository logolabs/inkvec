# Stage 03 — Palette

> Decides how many inks a raster contains, and what colour each one is — by minimum
> description length, not by clustering or a fixed distance threshold.

**Source:** `crates/inkvec-trace/src/color.rs` (the decisions and their constants), with the walk
in `color/mdl.rs`, the per-colour index in `color/distinct.rs` and the representation test for rare
candidates in `color/represent.rs`; the transparent-image mirror is `native.rs` and
`native/palette.rs`. The fill snap that runs after the fill fit is `color/snap.rs`
**Entry point:** `extract_palette_mdl()` (`color.rs:847-918`), which runs `mdl::extract`
(`color/mdl.rs:117-173`)
**Pipeline position:** first stage of the Quality colour front end. Called from
`trace_color_full_with_alpha` (`inkvec-trace/src/lib.rs:393-408`) through `extract_palette_mdl_ids`,
which shares the image's colour ids with the labelling; stopwatch mark `palette`
(`inkvec-trace/src/lib.rs:409`). It runs after noise estimation (`coverage::estimate_noise`,
`inkvec-trace/src/lib.rs:311`, stage 02) and the three intake measurements that switch it between
its clean and soft settings: edge width (`coverage::intake_scale`, `inkvec-trace/src/lib.rs:344`),
ringing (`coverage::ringing_score`, `inkvec-trace/src/lib.rs:349`) and the container format
(`ColorOptions::lossy_intake`). Followed by `label_image` (mark `labels`,
`inkvec-trace/src/lib.rs:528`) and despeckling (mark `despeckle`, `inkvec-trace/src/lib.rs:532`). An
image with transparency traced natively runs the two-ground mirror, `native::extract_palette`
(`native.rs:285-346`, called at `native.rs:856-871`). Fast mode does not run this stage; it has its
own histogram palette (`fast/front.rs:7-10`, stage 14).

## What problem this solves

A raster does not come labelled with "this many inks." Two failure directions are both real
and both visible in the final SVG:

- **Too few inks** merges colours the artist kept apart, and the region that should have been
  two flat fills gets painted as one flat colour — or, worse, the merged residual gets explained
  away as a spurious gradient. The doc comment on `extract_palette_mdl` gives a concrete case:
  "Ten concentric rings of ten distinct hues came back as six colours, because the five pale rings
  fell inside the threshold of each other — and the five that vanished then made their regions
  look like gradients, which is where a DISTS@4x of 0.197 came from" (`color.rs:860-863`).
- **Too many inks** turns measurement noise, JPEG ringing, or one anti-aliased ramp into
  dozens of spurious colours, each of which becomes its own face, its own boundary, and its own
  path. `SOFT_SAME_INK_DE00`'s doc comment gives the concrete cost: "76 distinct fills where the
  drawing has five, `#030303` alone emitted as 82 separate paths against a single `#000000` at
  1x" (`color.rs:581-582`).

Both failures come from the same root cause: **a fixed distance threshold cannot be right
everywhere in a perceptual colour space.** Saturated inks sit far apart and survive any
reasonable threshold; pale ones cluster close together and do not, even when the artist meant
them as distinct. The module's answer is to stop asking "is this colour close enough to an
existing one?" and start asking "does keeping this colour separate pay for itself?" — a model
selection question, with an explicit cost function, rather than a clustering question with an
implicit one.

## Inputs and outputs

**Input:**
- `rgb: &[[f32; 3]]` — sRGB pixels in `[0, 1]`, composited onto white, row-major.
- `width`, `height`.
- `merge_distance: f32` — the fixed OKLab-distance threshold two colours must clear before either
  is even considered separate (`DEFAULT_MERGE_DISTANCE`, see below).
- `max_colors: usize` — hard cap on palette size (64 by default, `inkvec-trace/src/lib.rs:197`).
- `ev: PaletteEvidence` (`color.rs:826-845`) — see below.

**Output:** `Palette` (`color.rs:695-710`):

| field | type | meaning |
|---|---|---|
| `colors` | `Vec<Oklab>` | the recovered inks, in OKLab, in acceptance order |
| `rgb` | `Vec<[f32; 3]>` | the same, converted to sRGB |
| `weight` | `Vec<f32>` | each ink's share of the image: the pixels whose nearest ink it is, within `merge_distance` (`color/mdl.rs:517-557`) |
| `alpha` | `Vec<f32>` | opacity of each entry; all `1.0` from this walk until `split_alpha_inks` runs |

The result always has at least one ink: with nothing accepted it is the most frequent mode, and
an empty image gives white with weight 0 (`color.rs:905-907`).

## How it works

### `PaletteEvidence` — what the palette needs to know about the measurement

```rust
pub struct PaletteEvidence {
    /// Per-channel pixel noise in sRGB units, from [`crate::coverage::estimate_noise`].
    /// Zero disables the split test that this noise level would otherwise justify.
    pub sigma_noise: f64,
    /// Nats charged per parameter — the same exchange rate every other stage prices
    /// against. An ink costs `lambda * PARAMS_PER_INK` before it explains anything.
    pub lambda: f64,
    /// How many measured standard deviations two inks must lie apart before they count
    /// as two.
    pub noise_sigmas: f32,
    /// Perceptual distance below which two candidate inks are one ink. Raised on a soft
    /// intake; see [`SOFT_SAME_INK_DE00`].
    pub same_ink_de00: f32,
}
```

(`color.rs:826-845`). The doc comment on the struct explains why it is a named struct rather than
trailing arguments: "These three arrived as trailing numbers and were easy to transpose — two
`f64` and an `f32`, all plausible in any order, and a swap would have quietly changed how many
inks the image was found to have. Naming them makes that impossible" (`color.rs:828-830`).

Where each field comes from, concretely, at the call site (`inkvec-trace/src/lib.rs:402-407`):

- `sigma_noise` — `coverage::estimate_noise` on the whole image's luminance (stage 02). On a soft
  intake it is raised *after* the palette, once there are labels to measure the residual against
  (`regularize::residual_sigma`, `inkvec-trace/src/lib.rs:429-491`); the palette sees the
  pre-label estimate.
- `lambda` — `gradient::bic_lambda(img.width * img.height)` = `0.5 * ln(n)` (`gradient.rs:320-327`),
  the Bayesian information criterion choice: "it grows slowly with region size, so a large region
  has to earn a gradient with proportionally more evidence than a small one does"
  (`gradient.rs:322-324`). This is the same `lambda` used throughout the pipeline's cost function,
  `cost = 0.5*chi2 + lambda*params`.
- `noise_sigmas` and `same_ink_de00` — switched between a clean-intake value and a soft-intake
  value (`inkvec-trace/src/lib.rs:336-368`) by three upstream signals, any one of which is enough:
  the edge width (`intake_scale > SOFT_INTAKE_EDGE`), the container format
  (`ColorOptions::lossy_intake`, from `lossy_container`, `load.rs:58-96`), and the ringing score
  (`ringing_score > SOFT_RINGING`, or `SOFT_RINGING_LARGE` when both sides are at least
  `RINGING_MIN_DIM`). The ringing score is "deliberately the *third* way into the soft-intake
  branch": it adds "the case both of them miss, which is the common one: a JPEG re-saved as PNG,
  where the container lies and the edges are still one pixel wide" (`color.rs:511-514`). See
  `SOFT_NOISE_SIGMAS` and `SOFT_SAME_INK_DE00` below for what each value is and why it must stay
  gated rather than always on.

### The extraction loop

`extract_palette_mdl` (`color.rs:847-918`) documents the procedure in four stages
(`color.rs:872-895`); the walk itself is `mdl::extract` (`color/mdl.rs:117-173`):

**1. Number the distinct colours.** Every distinct colour is converted once to OKLab, to sRGB
(from the OKLab value) and to linear light (`ClassicView`, `color/mdl.rs:26-86`), and every later
per-pixel quantity is read through the pixel's colour id (see "How it is computed" below).

**2. Mode-finding by frequency, not binning.** Colours are bucketed into a coarse `24^3` grid over
OKLab (`BINS = 24`; `L` over `[0, 1]`, `a` and `b` over `[−0.4, 0.4]`, clamped; cell
`round(x · 23)` per axis) purely to make counting tractable — the candidate itself is the *mean*
OKLab colour of the pixels that fall into that cell, summed in `f64` in pixel order, not the cell
centre, so the recovered colour is not snapped to a grid point (`frequency_modes`,
`color/mdl.rs:452-498`). Modes are sorted by descending pixel count with the cell key as a
tie-breaker (`color/mdl.rs:496`). The tie-break is not cosmetic: sorting by count alone left
equal-frequency colours in hash-map order, and "the junction accuracy test measured 0.054-0.134 px
across ten consecutive runs of one binary" (recorded on the test-only oracle,
`color/reference_tests.rs:699-703`).

**3. For each mode, in frequency order, decide whether it survives as its own ink.** This is
where the model-selection logic lives (`Walk::accepts`, `color/mdl.rs:198-289`), and it runs the
following gauntlet per candidate `c`, in this order, stopping once `max_colors` inks are accepted
(`color/mdl.rs:147-149`):

- **Rare or not.** The candidate's *claim* counts how many pixels would actually choose `c` —
  not the coarse bin count `n`, but the pixels strictly nearer to `c` than to every ink accepted
  so far, read from the per-colour nearest-ink distance (`DistinctImage::claim`,
  `color/distinct.rs:301-340`; on images over `STAT_PIXELS` it counts every `s`-th pixel and
  scales back up by `s`). If the claimed share is below `MIN_INK_WEIGHT` (0.4%) and at least one
  ink is already accepted, the candidate is *rare* (`color/mdl.rs:217-219`). Until 2026-10-03 a
  rare candidate was dropped here; now it goes on through the cheaper gates below and must then be
  *represented* (the fourth bullet). The first ink accepted is never rare, so the palette is never
  empty; the transparent-image walk exempts one more candidate, the first ink that draws
  anything; see below.
- **Perceptual floor (`SAME_INK_DE00` / `SOFT_SAME_INK_DE00`).** Converted to sRGB and compared
  by CIEDE2000 against the accepted ink nearest to it in OKLab (`same_ink_as_accepted`,
  `color.rs:935-962`, called at `color/mdl.rs:226-228`). Below the floor, the candidate is folded
  in regardless of pixel count or evidence: "A description-length argument cannot rescue an ink
  nobody can distinguish, so this floor is applied before the MDL escape, not inside it"
  (`color.rs:340-342`).
- **Fixed-threshold / noise-gated separation (`nearest <= merge_distance.max(reach)`).** If the
  candidate sits within `merge_distance` — or within `reach = noise_sigmas * spread`, whichever
  is larger — of an existing ink, it is folded in *unless* it can buy its way out with the MDL
  test below (`color/mdl.rs:225`, `color/mdl.rs:229-248`). `spread` is the lower median distance
  from `c` of the pixels it claims within `merge_distance` (`DistinctImage::spread`,
  `color/distinct.rs:342-359`), measured only when `noise_sigmas` is non-zero
  (`color/mdl.rs:210-216`). The doc comment on `spread` records why it is members, not
  territory ("a spread over that rejected every colour after the first (screen set 0.4328 ->
  1.2461 before it was restricted to `tol`)"), and the median, not the mean ("a mean is pulled up
  by them (2 % on the screen set)").
- **Representation (rare candidates only).** A rare candidate is an ink when at least
  `max(8, n · 8 / 16384)` of its claimed pixels are colours that no convex mixture of the inks
  around them, and no resampling overshoot of those inks, explains within `max(3σ, 0.025)`
  (`represent::unexplained`, called at `color/mdl.rs:249-268`); see "Representation" below.
- **Blend test.** If none of the above disqualifies it, the candidate is checked against whether
  it is explained as a coverage-weighted mixture of two already-accepted inks, and whether it is
  thin and straddling (`BlendEvidence`, `color/mdl.rs:329-406`) — see below.
- **Escape interior.** A candidate that the separation gate let through only by the MDL escape,
  and that is not a blend, must have `interior >= BLEND_INTERIOR_FRACTION`
  (`escape_needs_interior`, `color.rs:247-321`, read at `color/mdl.rs:288`); see "The escape rule"
  below.

**4. Refit.** Once the palette is decided, every entry is moved to the mean of the pixels that
chose it, where a pixel chooses its nearest ink only if that ink is within `merge_distance`, so
anti-aliased pixels far from every entry do not pull the means (`refine_to_members`,
`color/mdl.rs:500-557`). All entries leave the walk with `alpha = 1.0` (`color/mdl.rs:166`) —
extraction always runs on an opaque-matted image, because unmixing a boundary needs two opaque
colours (`inkvec-trace/src/lib.rs:255-256`).

### The MDL escape — `worth_it`

The core of the "worth keeping" decision is `color/mdl.rs:239-244`:

```rust
let worth_it = sigma_noise > 0.0
    && nearest > JND_FLOOR
    && nearest > reach
    && 0.5 * (claim as f64) * ((nearest as f64 / sigma_noise).powi(2))
        > lambda * PARAMS_PER_INK;
```

Read as a sentence: a candidate inside the fixed merge distance is kept anyway only if (a) a
noise estimate actually exists, (b) it clears the just-noticeable-difference floor, (c) it clears
the noise-scaled `reach`, and (d) — the substantive test — folding it into the nearest ink would
cost more in residual than minting it costs in description length:

```text
0.5 * claim * (nearest / sigma_noise)^2   >   lambda * PARAMS_PER_INK
```

The left side is the Gaussian negative log-likelihood of explaining `claim` pixels, each `nearest`
OKLab-units from where they'd be forced to sit, given measurement noise `sigma_noise`. The right
side is what a new ink costs: `lambda` nats per parameter, `PARAMS_PER_INK = 3.0` parameters
(`color.rs:323-324`, one per OKLab channel) per ink. The doc comment is explicit about the units:
`nearest` is an OKLab distance and `sigma_noise` an sRGB one, "both scales run over about
`[0, 1]`, and the test treats the ratio as a number of standard deviations" (`color.rs:899-903`).
"A mode with thousands of pixels and a separation far above the noise is therefore kept however
close the fixed threshold would call it, while a handful of pixels a hair away from an existing
ink is folded in — which is exactly the behaviour wanted from both" (`color.rs:868-870`). This is
the same `cost = 0.5*chi2 + lambda*params` objective that governs curve fitting and gradient
selection elsewhere in the pipeline — the palette is not a special case, it is the same rule
applied to "how many colours" instead of "how many segments."

### The blend test — telling an anti-aliased ramp from real ink

A colour that lies on the segment between two already-accepted inks might be a real third ink
(a pastel between white and red, say) or it might just be a coverage-weighted blend pixel from
the boundary between those two inks. Both look identical in colour space near the middle of the
segment, so colour alone cannot decide. Three tests run in sequence, each only when the one
before it leaves the verdict open (`BlendEvidence::measure`, `color/mdl.rs:346-392`):

1. **`blend_pairs_cached`** (`color.rs:603-693`) checks whether `c` lands on the segment between
   any two accepted inks: `t = ((p − A) · (B − A)) / |B − A|²` must lie in
   `[BLEND_TMIN, 1 − BLEND_TMIN]` and the residual to the nearest point of the chord, measured
   back in OKLab, must be at most `1.6 × merge_distance` (`color/mdl.rs:354`). The check runs in
   **linear light** (compositing is linear there) and also in sRGB (because some pipelines
   composite in gamma space anyway) (`color.rs:611-613`), trying every pair and keeping every
   match — "A pale pink is within tolerance of the white–grey axis as well as the white–red one,
   and only the pair its pixels actually lie between can say whether it straddles them; the
   caller tries them all" (`color.rs:615-618`).
2. **`interior`** (`DistinctImage::interior`, `color/distinct.rs:361-395`), measured for a blend
   and for a candidate the escape let through, is one step of 4-neighbour erosion of the pixels
   `c` claims: the fraction whose four in-image neighbours are also claimed (a neighbour outside
   the image counts as claimed). This has to be measured against the pixels `c` would actually
   take from the current palette state, not a fixed-radius ball around `c`: "A ball is the obvious
   choice and it is wrong: an anti-aliased colour sits close to one end of its ramp, so a ball
   around it swallows the solid region as well as the band, and the band then measures as solid
   (tried: the green-circle case went from 29 faces to 40)" (`color/distinct.rs:367-371`).
   Anti-aliasing is a one-pixel band with almost no interior; a real ink covers area and is almost
   entirely interior.
3. **`straddle`** (`color/mdl.rs:409-450`, counted by `Neighbourhoods::straddle`,
   `color/distinct.rs:505-528`), run only when `interior < BLEND_INTERIOR_FRACTION`
   (`color/mdl.rs:366`) and taken as the largest over the matching pairs. This asks something
   erosion cannot: does the candidate's pixel neighbourhood actually straddle the two inks it
   supposedly blends — does a claimed pixel's 3x3 neighbourhood hold a pixel further towards ink
   A *and* one further towards ink B along their axis, by `STRADDLE_STEP` (clipped to half the
   room left on each side, floor 0.02)? A genuine coverage ramp does, by construction; a
   genuinely thin ink band (two pixels wide, say) touches A on one side and B on the other but
   rarely straddles both. The doc comment gives the case this fixes: "The Vulcan salute's shadow
   strips, 2–3 px of a brown that is a mix of the palm and the outline, had interior 0.21 and were
   discarded as coverage; the palm then grew a radial gradient to explain them"
   (`color.rs:237-239`).

A candidate is finally discarded as coverage — not kept as an ink — only when it is a blend
**and** interior is below `BLEND_INTERIOR_FRACTION` **and** the straddle fraction is at least
`BLEND_STRADDLE_FRACTION` (`is_coverage`, `color/mdl.rs:394-399`).

### The escape rule — an overshoot rim is not an ink

`escape_needs_interior` (`color.rs:247-321`), applied by both walks (`is_thin_escape`,
`color/mdl.rs:401-406` and `native/palette.rs:489-493`): a candidate that sits inside
`max(merge_distance, reach)` of an accepted ink, that only the MDL escape let through, and that is
not a blend, is rejected when `interior < BLEND_INTERIOR_FRACTION`. `BlendEvidence::measure` takes
the interior of an escaped candidate for that (`color/mdl.rs:347-392`, `native/palette.rs:422-480`);
a candidate that did not escape is measured and decided exactly as before.

The case is the Studio's own sample, `studio/src-tauri/samples/crest-filigree.png`: one gold
(176,138,74) on a clear ground, drawn at 256 px and upscaled 2x with a premultiplied Lanczos filter.
The filter's negative lobes overshoot at every edge, and with alpha clipped at 1 the overshoot is an
*opaque* one-pixel rim of 1.067 × the gold: 3462 rim pixels of 12105 opaque ones. The two-ground
walk cannot call the rim a blend (a blend with the clear ground moves the colour over grey towards
grey; the rim is opaque), a non-blend was never tested for thinness, and at 0.0294 from the gold it
is inside the 0.035 radius, where the escape decided: `0.5 · 5725 · (0.0294 / 0.00196)² = 6.4e5`
against `3λ = 18.7`. The rim became faces along every edge. v0.2.5 wrote three fills (`#b08a4a`,
`#b08b4b`, `#bd944f`), 7 paths and no circles, with notched dots (dE00 0.1245 against the input).
The r2-palette research bisected it to 772771b ("native transparency by default", v0.1.4); the
opaque walk of v0.1.3 rejected the same pixels as coverage, because composited onto white they do
lie on the gold-white chord. With the rule the rim has interior 0 and is rejected: 14 circles and
the dots round again (dE00 0.1192, 2486 bytes against 6841).

Blends are exempt because a thin blend already has its shape test, the straddle test. The research
proposed asking every escaped candidate, blend or not; on the gate (v0.2.5 baseline, 2026-10-03)
that read **worse** at quality-128ss, dE00 +1.10 % (upper bound +2.07 % against a 1.38 % margin),
11 of 16 changed icons worse, all skin-tone shading bands in noto-emoji that the straddle test had
rightly kept (`emoji_u1f9dd_1f3fd_200d_2640` 0.478 -> 0.624). With blends exempt the rule is
byte-identical on all six gate conditions, changes one held_a icon (+0.005,
`noto-emoji/emoji_u1f9da_1f3fd_200d_2642`, the one the research predicted), and on the 2x
Lanczos screen set (`bench/resampled_eval.py`, ring2x) lowers dE00 2.4 % (95 % interval
-3.6 % .. -1.3 %), parameters 3.6 % and invented fills 7.5 %; soft2x is unchanged.

**Inspired by:** J. Yang, N. Vining, S. Kheradmand, N. Carr, L. Sigal, A. Sheffer (2023), "Subpixel
Deblurring of Anti-Aliased Raster Clip-Art", *Computer Graphics Forum* 42(2), doi:10.1111/cgf.14744,
whose candidate palette takes patch seeds only from same-colour patches at least 2 px wide; their
colour-outlier seeds are found after a network removes ringing, and on the raw raster an overshoot
pixel is exactly such an outlier. **See also:** VTracer / visioncortex `color_clusters`
(`patch_good`: `perimeter < area` of every layer candidate); L. Yang, P. V. Sander, J. Lawrence,
H. Hoppe (2011), "Antialiasing Recovery", *ACM TOG* 30(3), doi:10.1145/1966394.1966401, whose
two-colour edge model treats an overshoot pixel as one of the two extremes.

### Representation — small inks the share used to drop

`color/represent.rs`, called by both walks for a rare candidate after the cheaper gates
(`color/mdl.rs:249-268`, `native/palette.rs:334-355`). Until 2026-10-03 a candidate claiming less
than `MIN_INK_WEIGHT` (0.4 %) of the image was dropped: 65.5 px at 128², over a thousand at 512².
The share keeps anti-aliasing out, because a blend colour is individually rare, but it also
dropped small inks that are nothing like a blend. The r2-palette research found 44 thick artist
inks merged away on the 362 icons of screen + held_a, 75 % of them under the share (area quartiles
20 / 32 / 47 px), at a median 7.5 dE00 from the ink that painted them instead: pupils, a red
mouth, a yellow star. With the artist's palette supplied those merges fell from 34 to 2.

A pixel votes for a new ink only when the inks already accepted cannot explain it
(`represent::unexplained`):

1. the inks around a claimed pixel are the accepted ink nearest each of its eight neighbours that
   the candidate does not claim itself — the walk keeps that index, `nearest_ink`, beside the
   nearest-ink distance (`color/mdl.rs:291-326`, `native/palette.rs:373-405`; a distance moves
   only when strictly smaller, which is `f32::min` exactly, so the distances are unchanged) — the
   `MIX_INKS = 4` most frequent, plus the clear ground on the two-ground walk;
2. its residual is the distance to the nearest convex mixture of two or three of them
   (`native::mixture`, the model `regions::absorb_blend_slivers` uses), to the single ink when
   only one is around, or infinite when none is (a pixel surrounded by the candidate), in sRGB
   (six two-ground sRGB coordinates on the native walk);
3. it votes when that residual exceeds `max(3σ, 0.025)` — the absorption stages' tolerance — and
   it is not resampling overshoot of those inks either (`represent::overshoot_residual`: up to
   `OVERSHOOT = 0.15` of a chord beyond either end, or an ink scaled by up to 1.15);
4. the candidate is represented when its votes, scaled by the visiting stride, reach
   `max(MIN_VOTES, VOTE_SHARE · n)` = `max(8, n · 8 / 16384)`: 8 px at 128², 128 px at 512².

A represented candidate still meets every other gate. A candidate above the share is unaffected,
and so are the first ink and the two-ground walk's first visible ink (Wave A's `rarity_exempt`,
unchanged). The overshoot exception was added after measuring without it: on the 2x Lanczos screen
set the rule admitted the ringing rims as small inks (ring2x dE00 +0.88 %, invented fills +12.8 %,
worst `openmoji/1F1FF-1F1F2` +0.26); with it, ring2x reads +0.38 % (95 % interval -0.68 % ..
+1.29 %) and invented +8.4 %. The bound comes from measurement: Pillow's `LANCZOS` at 2x
overshoots a hard grey step by 10.0-10.5 % of the step, an anti-aliased one by 3.7-4.0 %, and a
premultiplied gold edge on clear leaves rim pixels at up to 1.108 times the gold.

Measured on gate v2 against v0.2.5 (with the escape rule and the fill snap below in the same
build, both of which are within noise on the gate): quality-128ss dE00 -1.99 % (95 % interval
-3.29 % .. -0.85 %, 24 icons better, 8 worse), quality-512ss -1.37 % (-2.41 % .. -0.35 %, 30 better,
12 worse), quality-512ssop -0.39 % (non-inferior), turning and parameters non-inferior everywhere,
Fast identical; held_a at 128 px 0.1365 -> 0.1335 (13 better, 4 worse; worst decile 0.4202 ->
0.4122); soft2x -0.32 %. Interleaved trace time against the build without it: 0.97x at 512 px.

**The frame.** Two other admission rules exist. `SAME_INK_DE00` (shipped) is a perceptual floor:
below 1.5 dE00 a candidate is the same ink whatever any count says. The two-level palette
(`e6fe352`, an experiment branch, opt-in, never merged) replaces candidate *generation*: level one
seeds inks from pixels with same-colour neighbours on both axes, level two runs the old walk seeded
with them. This keeps the walk and the floor and changes only what a rare candidate must show:
(a) the floor answers a different question — can anyone see the difference — that no count or
mixture can, and it runs before this test; (b) the two-level palette's seeds are a spatial
same-colour test, which the interior and escape tests already make inside the walk, and its
measured gain was on gradient icons, where the research's ramp oracle shows the palette is not the
lever (+0.009 dE00 with the artist's ramp colours supplied); (c) one walk keeps the opaque and
two-ground forks in step, which the two-level palette did for the opaque path only.

**Method from:** Y. Aksoy, T. O. Aydın, A. Smolić, M. Pollefeys (2017), "Unmixing-Based Soft Color
Segmentation for Image Manipulation", *ACM TOG* 36(2):19, doi:10.1145/3002176, section 5: the colour
model grows from votes of pixels the current model does not explain. Adapted: the model is local
(the inks around the pixel), the vote is binary against the noise, there is no gradient weight
(an edge pixel between accepted inks is already explained by their mixture), the residual is in
sRGB (linear light would let a dark ink beside black sit within 0.025 of the black-to-skin chord),
and the floor is in pixels as VTracer's `good_min_area` is, scaled with the image like the share it
replaces. **Inspired by:** L. Yang, P. V. Sander, J. Lawrence, H. Hoppe (2011), "Antialiasing
Recovery", *ACM TOG* 30(3), doi:10.1145/1966394.1966401 (an edge pixel as a mixture of its
neighbourhood's extremes). **Not from the literature:** the overshoot exception, because that
model takes an overshoot pixel for an extreme. **See also:** A. Delong, A. Osokin, H. N. Isack,
Y. Boykov (2012), "Fast Approximate Energy Minimization with Label Costs", *IJCV* 96(1):1-27,
doi:10.1007/s11263-011-0437-z, the joint palette-and-labels objective with a cost per label used,
which this greedy walk approximates.

### How it is computed: once per distinct colour

Every per-pixel quantity the loop reads (OKLab, sRGB, linear light, the distance to a candidate or
to the nearest accepted ink) is a function of the pixel's colour bits, and vector art repeats a
few colours many times (screen set: 232 distinct colours in 16 384 pixels at the median). So the
image is first reduced to its distinct colours (`ColourIds`), each quantity is computed per colour,
and pixel positions for the spatial tests come from each colour's list of sampled pixels
(`DistinctImage`). The module doc comment of `color/distinct.rs` (`color/distinct.rs:1-42`) names
the sources: "This is the unique-colour reduction of Celebi's weighted k-means colour quantiser"
(M. E. Celebi, "Improving the Performance of K-Means for Color Quantization", *Image and Vision
Computing* 29(4):260-271, 2011, doi:10.1016/j.imavis.2010.10.002), adapted so that the weights are
counts of *sampled* pixels and the means that become inks are still summed in pixel order; the
spatial tests are histogram backprojection (M. J. Swain and D. H. Ballard, "Color Indexing",
*IJCV* 7(1):11-32, 1991). A candidate's claimed set is the weighted bichromatic
reverse-nearest-neighbour set of `c` against the accepted inks (F. Korn and S. Muthukrishnan,
SIGMOD 2000, doi:10.1145/335191.335415; the weighted-client form R. C.-W. Wong et al., PVLDB 2(1),
2009), which does not change until a candidate is accepted, so it is computed once and shared by
the rarity count, the spread, the interior test, every straddle pair and the representation votes
(`DistinctImage::claimed_pixels`, `color/distinct.rs:397-407`). Not from the literature: each
claimed pixel's 3x3 neighbourhood is gathered once per candidate as colour ids. The result is
bit-identical to the per-pixel loop; the per-pixel loop is kept as a test-only oracle
(`color/reference_tests.rs`, `native/reference_tests.rs`). The walk itself is `color/mdl.rs`
(opaque) and `native/palette.rs` (two grounds).

### The transparent-image walk (`native::extract_palette`)

When transparency is traced natively, the palette is the same walk with every colour a two-ground
point (`Ink2`, `native.rs:76-120`): the colour over white and over a mid-grey second ground
(`SECOND_GROUND = 0.5`, `native.rs:122-128`), with distance the larger of the two OKLab distances,
so white paint and the clear ground are far apart even though they look identical over white. The
clear ground comes out as an ink of its own at opacity 0. The differences from the opaque walk are
listed on the function (`native.rs:295-304`):

- the clear ink (opacity at or below `CLEAR_INK_ALPHA = 0.02`, `native.rs:376-377`) does not count
  against `max_colors`; once the cap is full the scan continues only to find it
  (`native/palette.rs:186-200`);
- nor does it use up the **rarity exemption**: "the first ink that draws something is never rare,
  as the first ink is not, so a lone small shape on a clear canvas is an ink without having to be
  represented" (`native.rs:299-301`);
- a translucent candidate that is not a blend must have an interior
  (`BlendEvidence::measure`, `native/palette.rs:422-480`);
- the representation test measures in the six two-ground sRGB coordinates, with the clear ground
  always among the inks a pixel may be a mixture of (`native/palette.rs:334-355`);
- there is no `INKVEC_MERGE_DE00` experiment and the same-ink floor does not print.

The rarity exemption is `rarity_exempt` (`native/palette.rs:228-266`), applied at
`native/palette.rs:308-311`: a candidate is never rare when nothing is accepted yet, as in the
opaque walk, or when it is not clear (opacity above `CLEAR_INK_ALPHA`) and every accepted ink is.
Its doc comment gives the case. On a transparent canvas the walk's first ink is nearly always the
clear ground, the commonest colour, and the clear ground draws nothing. With only the opaque walk's
exemption, an image whose only paint covers less than 0.4 % of the canvas got a palette of the
clear ink alone: a lone 50 px² disc on a 128 px transparent canvas (0.31 %) was traced to an empty
SVG — the carve stage still cut the disc out as a face, but named it by the nearest palette entry
over white, the clear ink, so it was emitted at opacity 0. Every other gate still applies to the
exempt candidate, so a translucent anti-aliased rim without an interior is still rejected. A rare
colour beside an accepted visible ink is not exempt: it is an ink when it is represented (see
"Representation" above), and what stays under the representation floor the carve stage names by
the nearest visible ink (`name_carved_paint`, `CARVED_PAINT_ALPHA`, stage 05). The test-only
oracle got the same rules, so it still checks the per-point rewrite and nothing else
(`native/reference_tests.rs:252-300`). Tests: `a_lone_small_shape_on_the_clear_ground_is_an_ink`
(`native/tests.rs:1035-1058`),
`a_rare_colour_beside_a_visible_ink_is_an_ink_only_when_represented` (`native/tests.rs:1099-1129`),
`an_overshoot_rim_inside_the_merge_radius_is_not_an_ink` (`native/tests.rs:1060-1097`), and end to
end `a_lone_small_shape_on_a_transparent_canvas_is_drawn` (`inkvec-cli/tests/pipeline.rs:334-341`).

The code labels the rule (`native/palette.rs:257-262`): **Not from the literature** — "the rarity
gate and its exemption are rules of this walk, because the published quantisers have no clear ink
that draws nothing." **See also:** Heckbert, P. (1982), "Color image quantization for frame buffer
display", *ACM SIGGRAPH Computer Graphics* 16(3):297-307, doi:10.1145/965145.801294, "whose
popularity algorithm keeps the most frequent colours and drops rare ones. Rare colours that matter,
such as a small isolated shape, are the known weakness of that rule."

### `label_image`

`label_image(rgb, pal)` (`color.rs:1100-1119`) assigns every pixel to its nearest palette entry in
OKLab (ties to the lower index), independent of the extraction pass: a nearest-neighbour scan,
made once per distinct colour and read back through each pixel's colour id (`color/mdl.rs:559-573`).
It is hard labelling: "an anti-aliased pixel gets whichever ink is closest, often a third colour,
which is what `crate::regions::absorb_blend_slivers` later repairs" (`color.rs:1103-1105`).

### `split_alpha_inks`

`split_alpha_inks(labels, pal, alpha)` (`color.rs:964-1098`) runs after labelling, as a separate
pass, only when the caller asked for alpha inks and passed the source alpha
(`inkvec-trace/src/lib.rs:523-527`); it does not change how the palette itself was found, and the
native path does not use it (its palette carries opacity directly, `native.rs:291-293`).
Extraction always works on an opaque matte, which loses the distinction between "25% white over
nothing" and "the transparent ground itself" — both composite to the same colour and label as one
ink, so a translucent panel disappears into the background. For each ink with at least 16 pixels
(`color.rs:1015`), this function sorts the ink's true source alphas, cuts them into groups wherever
consecutive values jump by more than `LEVEL_GAP = 0.15`, and keeps a group as a level only if it is
tight (`spread <= LEVEL_SPREAD = 0.06`) and populous enough (`n/total >= MIN_SHARE = 0.02`)
(`color.rs:993-999`, `color.rs:1018-1044`); a level's mean below `CLEAR = 0.05` snaps to 0, and the
transparent group counts as a level of its own (`color.rs:1036-1039`). Only *flat* opacity is split —
"A face whose alpha varies across it is a glow, no single opacity describes it, and splitting it
would mint a band per level" (`color.rs:972-974`) — so a genuinely varying-alpha region is left
alone rather than being sliced into bands. One level just sets the ink's opacity. With two or more,
the most opaque level keeps the ink's original entry; each additional level mints a new palette
entry sharing the same colour, weight 0 and a different `alpha`, and every pixel of the ink moves to
the entry whose opacity is nearest its own (`color.rs:1053-1096`).

### One ink, one hex — snapping flat face fills (`color/snap.rs`)

The palette decides the inks; the fills are decided later, per face, after band merging and the
carve (stage 05). A flat face with an interior keeps the median of its own interior pixels, which
is right for a real plateau and leaves two faces of one ink a level or two apart: the crest's one
gold came out as `#b08a4a` and `#b18b4b`, and the r2-palette research counted 26 duplicate fills
under 1 dE00 in 21 of 362 clean icons and 683 in 147 icons of their 2x Lanczos resample.
`snap::snap_flat_fills` runs on both paths after the faces exist and before the boundary stages
(`inkvec-trace/src/lib.rs:711-724`, `native.rs:980-993`), so sub-pixel refinement and the boundary
solve place the edges against the colour that is written:

1. each ink's colour is the fill of its *representative face*: the flat face of that ink with the
   most strictly interior pixels (off the picture edge, four neighbours in the face) whose fill is
   within `REP_DE00 = 1.5` (the same-ink floor) of the palette entry, ties to the lower face
   index;
2. a flat face within `SNAP_DE00 = 0.5` of that colour is painted it. Gradient faces, the native
   path's fades and every face farther away keep their fills.

Not the palette entry's own colour: that is the mean of everything within the merge radius
(`refine_to_members`), rim included, and on the crest the entry is `#b28c4c` against the
interior's `#b08a4a` — painting the faces the entry cost dE00 0.1192 -> 0.1323. The research
proposed snapping within 1.5 dE00, and always for a face without an interior; both were measured
on the gate and both fail it. Within 1.5 or 1.0, quality-512ssop dE00 read +1.31 % / +1.30 %
(upper bound 2.99 % against 2.25 %): `noto-emoji/emoji_u2b05` went 0.0084 -> 0.1302, because the
white page and a `#fafafa` arrow are one palette ink under the same-ink floor and the page took the
arrow's colour. Painting interior-less faces whose colour is a blend of their ink read +0.97 % at
quality-128ss (48 icons changed, 30 worse): thin strokes of a colour of their own were repainted,
`#c1694f` as `#df1f32` (13.5 dE00) in `twemoji/1f646-1f3fb-200d-2640-fe0f`. At 0.5 without the
interior rule every gate condition is non-inferior (quality-128ss +0.01 %, 512ss -0.01 %, 512ssop
+0.28 %, all within noise; Fast identical), held_a +0.0001, and duplicate fills fall 48 % on ring2x
and 45 % on soft2x with dE00 +0.05 % (n.s.) and -0.12 %. The crest is one hex, `#b08a4a` (dE00
0.1214; the dots that had fitted `#b18b4b` cost 0.002).

**Inspired by:** Yang et al. (2023), doi:10.1111/cgf.14744: their labelling energy's
distinctiveness term penalises adjacent regions with similar but not identical colours, and their
palette colours come from patches, never from edge pixels. Here the same outcome is a post-fit
choice between near-equal fitted colours, because the labels are already fixed. Fast mode writes
every face in its ink by construction (`fast/palette.rs`).

## Measuring palette changes on resampled input (`bench/resampled_eval.py`)

The gate scores rasters rendered from the artist's SVG with a box filter, which never rings, so the
crest regression stayed invisible to it for seven releases. `bench/resampled_eval.py` is a local
benchmark, not a gate condition: it upscales the screen set's committed 128ss rasters 2x with Pillow
`LANCZOS` (ring2x: resized premultiplied, so every opaque edge on clear overshoots into an opaque
rim, and opaque edges ring by about 10 % of the step) and `BILINEAR` (soft2x: two-pixel ramps, no
overshoot), keeps the artist's SVGs as ground truth, and adds the crest sample itself. Per icon it
reports the gate's dE00 (judged at 1024 against the cached artist render) and parameter ratio, and
vocabulary-level palette counts: distinct flat fills, *invented* fills (farther than 2.5 dE00 from
every artist colour, gradient stop and translucent composite) and *duplicate* fills (pairs under
1.0 dE00 at one opacity). With two executables it pairs the rows and gives a bootstrap interval
of the dE00 change. Rows are cached per executable, kind, mode and set.

The branch, measured with it (Quality, screen set, 246 icons each):

| change | ring2x dE00 | ring2x invented | ring2x duplicates | soft2x dE00 | crest |
|---|---|---|---|---|---|
| v0.2.5 | 0.3509 | 1.68 / icon | 3.26 / icon | 0.3808 | 3 fills, 0 circles, dE00 0.1245, 6841 B |
| escape rule | -2.4 % (-3.6 .. -1.3) | -7.5 % | -3.0 % | identical | 2 hexes of one gold, 14 circles, 0.1192, 2486 B |
| fill snap | +0.05 % (n.s.) | -3.9 % | -48 % | -0.12 % | one hex, 0.1214 |
| representation | +0.38 % (n.s.) | +8.4 % | +1.2 % | -0.32 % | unchanged |

## `PaletteEvidence`, the guard constants, and their measured justifications

| constant | value | quoted derivation |
|---|---|---|
| `SAME_INK_DE00` | `1.5` (CIEDE2000) | `color.rs:330-347`. OKLab's lightness is cube-root-shaped, so a fixed OKLab radius is far too generous near black: "on a clean render of a one-ink black logo the palette accepted #020202, #040404 and #070707 as three more inks ... In CIEDE2000, which is what the bench scores with, those three sit at 0.31, 0.63 and 1.11 from black: differences no viewer can see." Swept on the screen set: `1.0` gave 0.4145→0.4140 (6 better, 5 worse) and still let `#070707` stand; `1.5` gave 0.4140→0.4124 (10 better, 6 worse, noto-emoji −0.005 dE00) and correctly split `abra_agency` back into two inks. "Mid-grey pairs 4 levels apart read 1.5, and a pair that close is not something the artwork is saying." |
| `SOFT_SAME_INK_DE00` | `5.0` | `color.rs:574-586`. Same judgement as `SOFT_NOISE_SIGMAS`, keyed to the same soft-intake trigger: on a resampled or oversampled intake, the ramp between two inks supplies "a whole family of intermediate colours that are not inks at all." Measured on a real brand mark upscaled 4x: 76 distinct fills where the drawing has five, `#030303` alone as 82 separate paths against one `#000000` at 1x. |
| `SOFT_NOISE_SIGMAS` | `3.0` | `color.rs:490-498`. Must stay gated: "Run unconditionally it costs **10.9 %** on the 246-icon screen set -- objective 0.4005 -> 0.4442, measured 2026-09-08 -- because on a clean intake two colours a whisker apart really are two inks and merging them throws away artwork." Switched on only by positive evidence the intake is not clean: wide edges (`SOFT_INTAKE_EDGE`), a lossy container, or measured ringing (`SOFT_RINGING`). |
| `NOISE_SIGMAS` | `0.0` | `color.rs:744-761`. The clean-intake value, deliberately zero, and read by name at both call sites (`inkvec-trace/src/lib.rs:357-361`, `native.rs:841-845`). The doc comment records that the guard *works* — on a logo upscaled with the packaged SR model's own 1.87-level error, it takes output "from 5 fills, 77 paths and 12.4 KB back to 1 fill, 3 paths and 1.0 KB" — but "it is not free on a clean intake -- the screen set goes from 0.4328 to 0.4451 -- because a region with a real gradient has a real spread, and the guard cannot tell that from noise without knowing which it is looking at." So "the caller decides, because the caller knows where its raster came from": upscaled, compressed or resampled input gets `SOFT_NOISE_SIGMAS` from the soft-intake gate. |
| `SOFT_INTAKE_EDGE` | `1.75` px | `color.rs:479-488`. Measured over all 980 corpus rasters (native, 8x-supersampled): median edge width 1.00, widest native 1.50 (a noto-emoji face with soft shading), then 1.43 and 1.26. A 2x Lanczos round trip reads 1.36–1.40, 4x reads 2.80, 8x reads 4.00. The threshold sits above everything native, catching "upscales of about 3x and more"; a 2x resample is indistinguishable from soft artwork by edge width alone and is what the SR pre-pass (`--sr auto`) exists for. |
| `SOFT_RINGING` | `0.12` | `color.rs:500-515`. Set from the false-positive side, because the guard it opens costs 2.8% on the screen set. Measured over 240 clean corpus rasters at tier 128ss: median 0.0000, p90 0.0062, p99 0.0333, max 0.1023 (`synthetic/rings_concentric`, a test pattern that genuinely oscillates); the highest real artwork is a noto-emoji at 0.0370. |
| `SOFT_RINGING_LARGE` | `0.05` | `color.rs:517-530`. The ring band is fixed in pixels, and at 128 px the ring of one glyph edge lands on the next, so clean art scores up to 0.1023. At 512 px the same artwork scores at most 0.0430 over 64 images, the compressed versions 0.086 median; a 0.05 gate has zero false positives and catches 88-89% of JPEG at qualities 85, 60 and 40, where the conservative gate catches 19-33%. |
| `RINGING_MIN_DIM` | `256` px | `color.rs:532-533`, used at `inkvec-trace/src/lib.rs:350-354`. The smallest side at which `SOFT_RINGING_LARGE` applies; below it the conservative number stands "and the screen set is bit-identical" (`color.rs:528-529`). No sweep cited for 256 itself. |
| `MEASURED_SIGMA_SCALE` | `1.0` | `color.rs:535-544`. How much of `regularize::residual_sigma` to believe when the noise is raised after labelling on a soft intake. Measured on 78 JPEG-re-encoded-as-PNG traces: at 1.0 the parameter count falls 31.5% and colour error 15.9%; the detector alone gives 22.0% and 22.2%; "The value here is the first: sigma taken at face value." (The comment used to call 1.0 "the swept optimum between them", though it is one of the two endpoints; no sweep between them is recorded.) |
| `MEASURED_SIGMA_CAP` | `8.0` levels | `color.rs:546-572`. A second ceiling on the measured noise, now non-binding (`residual_sigma` clamps itself to 8). On 791 traces across all classes a ceiling of 8 gives colour −16.9% and parameters −33.7%, diagrams +6% colour for −52% parameters. Smaller samples had pointed the wrong way (22 diagrams read +114% colour at the high ceiling, 106 diagrams +0.7%): "Nothing about this trade should be decided on fewer than several hundred paired traces". |
| `DEFAULT_MERGE_DISTANCE` | `0.035` (OKLab) | `color.rs:175-200`. Was `0.055`; an error-budget analysis on the 980-icon devset found that value merging inks the artwork keeps apart — "1.6 % of noto-emoji's interior pixels carrying half its interior error" turned out to be two flat colours (66% error reduction when fit as two flats) rather than a gradient (only 13% reduction as a ramp). Swept on the full set: `0.055→0.4960`, `0.040→0.4931`, `0.035→0.4922` (best), `0.030→0.4941`. At `0.035` all three axes improve together (dE00 0.2005→0.1991, DISTS 0.0296→0.0293, params-vs-artist 1.46→1.44). The doc comment explicitly warns not to tune this on the screen split alone — it prefers `0.030` there, and held-out set A prefers the old `0.055` outright; "Only the full set separates them." |
| `MIN_INK_WEIGHT` | `0.004` | `color.rs:202-211`. The share under which a candidate is *rare* and must be represented (until 2026-10-03: dropped). Qualitative: anti-aliased pixels are individually rare and spread across a ramp, so no single blend colour accumulates much weight, while flat regions accumulate thousands of pixels — no specific sweep cited for `0.004` itself. Never rare: the first ink accepted (`color/mdl.rs:219`), and on the transparent-image walk also the first ink that draws something (`native/palette.rs:228-266`). |
| `MIN_VOTES`, `VOTE_SHARE` | `8` px, `8 / 16384` | `color/represent.rs`. A rare candidate is represented with `max(MIN_VOTES, VOTE_SHARE · n)` unexplained pixels: 8 px at 128², 128 px at 512². The research measured the share alone at 0.001 and 0.0005 with the same result, so the floor sits at the lower. |
| `MIX_INKS` | `4` | `color/represent.rs`. Most inks around a pixel (by neighbour count) that its mixture is drawn from, plus the clear ground on the two-ground walk; at most ten pairs and ten triples. |
| mixture tolerance | `max(3σ, 0.025)` sRGB | `represent::mixture_tolerance`. The tolerance of `regions::absorb_blend_slivers` and `reassign_blend_pixels`, so a pixel those stages would hand back as anti-aliasing does not vote. |
| `OVERSHOOT` | `0.15` | `color/represent.rs`. Resampling overshoot a pixel may be explained as: up to 15 % of a chord beyond either end, or an ink scaled by up to 1.15. Measured with Pillow `LANCZOS` 2x: a hard step overshoots 10.0-10.5 % (64/192, 100/150, 30/220), an anti-aliased one 3.7-4.0 %, a premultiplied gold edge on clear 1.108x. |
| `SNAP_DE00` | `0.5` (CIEDE2000) | `color/snap.rs`. A flat face within this of its ink's colour is painted it. Measured on the gate: 1.5 and 1.0 read worse at quality-512ssop (+1.31 %, +1.30 %), 0.5 within noise everywhere. |
| `REP_DE00` | `1.5` (= `SAME_INK_DE00`) | `color/snap.rs`. A face counts as its palette entry's when the ink's colour is chosen if its fill is within this of the entry. |
| `BLEND_INTERIOR_FRACTION` | `0.25` | `color.rs:213-228`. Swept across two corpora with conflicting optima (real content wanted `0.015`, synthetic wanted `0.030`) before the discriminator itself was changed from abundance to shape (erosion/interior fraction). `0.25` is stated to sit below "a three-pixel ring [which] keeps about a third" of its pixels as interior — the concentric-rings case that motivated the fix. Also the escape rule's threshold (`escape_needs_interior`). |
| `BLEND_STRADDLE_FRACTION` | `0.5` | `color.rs:230-241`. Motivated by the Vulcan-salute shadow-strip case (interior 0.21, wrongly discarded as coverage under the interior test alone). No specific sweep is cited for `0.5` itself. |
| `STRADDLE_STEP` | `0.12` | `color.rs:242-245`. "Above quantisation noise for a pair of inks that differ by more than a few levels" — qualitative, no sweep cited. Clipped to half the room left on each side, floor `0.02` (`color/mdl.rs:443-446`). |
| `BLEND_TMIN` | `0.04` | `color.rs:588-590` (was `INKVEC_BLEND_TMIN`). Only interior mixtures count as a blend; `t` outside `[tmin, 1-tmin]` on the A–B axis "is a different colour, not a blend of these two" (`color.rs:665-667`). No derivation given for `0.04` specifically. |
| blend chord tolerance | `1.6 × merge_distance` | `color/mdl.rs:354` (and `native/palette.rs:440`). How far, in OKLab, a candidate may sit from the chord between two inks and still be a blend of them. No derivation given. |
| `JND_FLOOR` | `0.012` (OKLab) | `color.rs:738-742`. "Below this a viewer cannot tell the colours apart at all, so no amount of evidence makes them two inks rather than one measured twice." No numeric derivation shown; superseded in practice by `SAME_INK_DE00`'s perceptual floor for most cases, but still gates the MDL escape directly (`nearest > JND_FLOOR`, `color/mdl.rs:241`). |
| `PARAMS_PER_INK` | `3.0` | `color.rs:323-324`. One parameter per OKLab channel — a direct accounting fact, not a tuned constant. |
| `BINS` | `24` per OKLab axis | `color/mdl.rs:457-458`. The candidate grid; the entry is the mean of its pixels, so the grid only decides what is counted together. No derivation given. |
| `STAT_PIXELS` | `1 << 16` (65536) | `color.rs:783-796`. Bounds the cost of the per-candidate statistical passes (claim, spread, interior, straddle) so trace time does not grow with resolution beyond the corpus's 128px tuning point: "Every constant in this module was tuned on a 128 px corpus, and at or below the cap the stride is one and the arithmetic is bit-identical to visiting every pixel." The stride is the smallest `s ≥ ⌈n / STAT_PIXELS⌉` coprime with the width (`color.rs:807-824`). |
| `SPREAD_SAMPLES` | `8192` | `color/distinct.rs:196-198`. Most pixels the spread's median is taken over, "which bounds the sort at any image size". |
| `CLEAR_INK_ALPHA` | `0.02` | `native.rs:376-377`. Opacity at or below which an ink is the clear ground: it is not counted against `max_colors`, and it does not use up the rarity exemption (`native/palette.rs:263-266`). No derivation given. |

## OKLab, sRGB, dE00 — why three colour spaces

The module doc comment states the OKLab choice directly: "VTracer's `color_precision` truncates
significant bits per RGB channel, and RGB distance is not perceptual distance — so it
simultaneously splits colours a viewer cannot tell apart and merges ones they can. In OKLab,
Euclidean distance is approximately perceptually uniform by construction, so a single threshold
means the same thing everywhere in the space" (`color.rs:6-10`). OKLab is therefore the working
space for clustering, distance comparisons, and the palette's internal representation
(`Palette::colors: Vec<Oklab>`).

But OKLab's own lightness axis is a cube root, which is exactly the property that makes a *fixed*
OKLab distance untrustworthy near black: "the first sRGB level above black spans 0.067 of it,
thirty times the step at mid-grey" (`color.rs:333-335`). The `SAME_INK_DE00` derivation above is a
direct demonstration: a distance that is large in OKLab terms near black (`0.078`, over twice the
merge distance) is imperceptible in CIEDE2000 terms (`0.31`). So wherever the question is "can a
viewer actually tell these apart," the module converts to CIEDE2000 (`de00`, `color.rs:386-389`,
the Sharma/Wu/Dalal 2005 formulation in `de00_lab`, `color.rs:391-477`), held to Sharma, Wu and
Dalal's 34 published reference pairs (`color/tests.rs:10-78`) and to
`skimage.color.deltaE_ciede2000` values on sRGB pairs (`color.rs:1121-1136`), rather than trusting
OKLab distance directly. Compositing math (the blend test) is done in **linear-light sRGB**,
because alpha compositing is linear there and would be bent by OKLab's cube root or by
gamma-encoded sRGB (`color.rs:611-613`).

In short: OKLab for "is this the same cluster," sRGB (linear and gamma) for "what colour mixture
produces this pixel," CIEDE2000 for "can anyone actually see the difference" (the module's own
list, `color.rs:35-44`). Each answers a different question, and using OKLab for the last one is
exactly the bug `SAME_INK_DE00` exists to fix.

## Failure modes and edge cases

- **Merging too aggressively** loses real ink and manifests downstream as a spurious gradient —
  see the concentric-rings and the palm-shadow (Vulcan salute) cases above.
- **Merging too little** turns compression ringing or an anti-aliasing ramp into dozens of
  spurious inks — see `SOFT_SAME_INK_DE00`'s upscaled brand-mark case above.
- **Near-black colour-axis distortion.** OKLab's cube-root lightness inflates distances near
  black; `SAME_INK_DE00` is the fix, applied "before the MDL escape, not inside it" because "a
  description-length argument cannot rescue an ink nobody can distinguish" (`color.rs:340-342`).
- **The noise guard is not automatic on a clean-looking intake.** `coverage::estimate_noise`
  reads the floor on a mostly-empty icon however damaged it is — see `NOISE_SIGMAS`'s doc comment,
  quoted above — so the guard opens only on evidence the intake is degraded: edge width, container
  format, or ringing measured from the pixels (`coverage::ringing_score`). A damaged intake that
  shows none of the three is traced with the clean settings.
- **A rare small shape.** A shape under 0.4% of the canvas is an ink when at least 8 px (at 128²)
  of it are colours the inks around it cannot explain (representation, above); under that, the
  carve stage paints it its pixels' median colour (stage 05). On a transparent canvas whose paint
  is all rare, the first visible ink is exempt altogether (`rarity_exempt`, above) — before
  2026-10-02 a lone 50 px² disc on a clear 128 px canvas traced to an empty SVG.
- **Resampled input.** An upscaled raster rings: opaque rims brighter than the ink on clear
  edges, overshoot along the chord between two inks. Inside the merge radius the escape rule
  rejects such a rim; a rare one is not represented when it is within 15 % overshoot of its
  neighbours. A frequent rim farther than the merge radius from its ink is still admitted (see
  open questions).
- **Non-determinism from hash-map iteration order** was a real, measured bug (junction accuracy
  varying 0.054–0.134px across runs) and is fixed by carrying the bin key as an explicit
  tie-breaker (`color/mdl.rs:496`).
- **`INKVEC_PALDBG=1`** prints a per-candidate trace of every accept/reject decision
  (`color/mdl.rs:271-287`), "because a wrong palette does not look like a palette bug downstream
  — the green-circle case surfaced as a spurious radial gradient and twenty-seven junk paths" (the
  reason as recorded beside the same print in the test-only oracle,
  `color/reference_tests.rs:533-536`).

## Environment overrides

Since the settings cleanup (CHANGELOG, 0.2.0, *Changed*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

| variable | effect | default | source |
|---|---|---|---|
| `INKVEC_PALDBG` | prints per-candidate accept/reject diagnostics, the same-ink verdicts, the representation votes and the intake measurements to stderr | unset (silent) | `color/mdl.rs:138`, `color/mdl.rs:271-287`, `color.rs:950-958`, `native/palette.rs:184`, `native/palette.rs:359-369`, `inkvec-trace/src/lib.rs:387-392` |
| `INKVEC_MERGE_DE00` (*research build*) | replaces the OKLab merge radius with a CIEDE2000 radius | unset | `color/mdl.rs:139-144`, `color/mdl.rs:229-236` |
| `INKVEC_PALETTE_RGB` (*research build*) | clusters in plain sRGB instead of OKLab | off | `color.rs:91-105` |
| `INKVEC_NOISE_SIGMAS` (*removed*) | overrode `ev.noise_sigmas` | — | the soft-intake gate chooses `NOISE_SIGMAS` / `SOFT_NOISE_SIGMAS` |
| `INKVEC_SAME_INK_DE00` (*removed*) | overrode `ev.same_ink_de00` | — | the soft-intake gate chooses `SAME_INK_DE00` / `SOFT_SAME_INK_DE00` |
| `INKVEC_NO_INK_ESCAPE` (*removed*) | disabled the MDL `worth_it` escape | — | the escape is always on (`color/mdl.rs:239-248`) |
| `INKVEC_BLEND_TMIN` (*removed*) | overrode the interior-mixture window `tmin` in the blend test | — | now the constant `BLEND_TMIN` (`color.rs:588-590`) |

## Open questions

- **`BLEND_IMMUNE_WEIGHT` — resolved.** The constant, documented here before as dead code, is no
  longer declared anywhere in `color.rs`. `bench/sweep.py:29-33` records that it was dropped from
  the sweep on 2026-09-08 because nothing read it; every sweep entry is now proved live by
  `assert_live` before it is swept.
- **`NOISE_SIGMAS` as a named constant — resolved.** Both call sites now read
  `color::NOISE_SIGMAS` by name for the clean intake (`inkvec-trace/src/lib.rs:357-361`,
  `native.rs:841-845`).
- **Several shape-test constants have no numeric derivation**, only qualitative motivation:
  `BLEND_STRADDLE_FRACTION = 0.5`, `STRADDLE_STEP = 0.12`, `MIN_INK_WEIGHT = 0.004`,
  `JND_FLOOR = 0.012`, `BLEND_TMIN = 0.04`, the `1.6 × merge_distance` chord tolerance,
  `CLEAR_INK_ALPHA = 0.02` and `MIX_INKS = 4`. Each is tied to a real case that motivated it but
  not to a sweep that located its specific value, unlike `DEFAULT_MERGE_DISTANCE`,
  `SAME_INK_DE00`, `SOFT_NOISE_SIGMAS`, `SOFT_INTAKE_EDGE`, the ringing gates, `SNAP_DE00` or
  `OVERSHOOT`, all of which cite specific measurements.
- **Automatic detection of a noisy-but-clean-looking intake.** `NOISE_SIGMAS`'s doc comment still
  calls it "the open problem": "Making it free, by detecting the noise instead of being told about
  it" (`color.rs:757-760`). Part of it has since been answered from the pixels: the ringing score
  opens the gate on a JPEG re-saved as PNG, and on a soft intake the noise is then measured against
  the labels (`inkvec-trace/src/lib.rs:429-491`). Neither runs on an intake that shows no edge,
  container or ringing evidence, and the ringing score is built around a JPEG artefact: it "fires
  on only 21% of VAE output against 88% of JPEG" (`inkvec-trace/src/lib.rs:449-451`).
- **The opaque walk has no counterpart of the transparent walk's rarity exemption.** By design:
  on an opaque image every ink is paint, and a rare shape is an ink when it is represented or is
  drawn in its own colour by the carve stage, so the rule was not mirrored. The fork table in
  `native.rs:25-52` is the list to check when either walk changes.
- **Overshoot outside the merge radius.** The r2-palette research's P1(b) — a thin candidate on
  the *extension* of an accepted pair beyond one end, with that end's ink in its neighbourhood, is
  ringing — is not implemented. The representation test's overshoot exception covers rare rims;
  a frequent rim farther than the merge radius from its ink is still admitted, and ring2x still
  invents 1.6 fills per icon against 0.9 on soft2x. Fast mode's palette (`fast/palette.rs`) has
  neither the escape rule (it is immune to the crest: its leader clustering joins touching bins)
  nor the representation test.
- **Two radial dots on the crest.** With the rim no longer an ink, its pixels belong to the gold,
  and on two of the 12 dots the gradient stage explains them with a radial gradient ending in the
  rim colour (`#bc934e`). The input really is brighter there; whether a flat dot would be the better
  description is a question for the fill fit, not the palette.
