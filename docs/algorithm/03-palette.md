# Stage 03 — Palette

> Decides how many inks a raster contains, and what colour each one is — by minimum
> description length, not by clustering or a fixed distance threshold.

**Source:** `crates/inkvec-trace/src/color.rs`
**Entry point:** `extract_palette_mdl()` (`color.rs:798`, 289 lines to `color.rs:1086`)
**Pipeline position:** first stage of the colour front end. Stopwatch mark `palette`
(`lib.rs:311`), called from `trace_color_full_with_alpha` (`lib.rs:298-310`), immediately after
noise estimation (`coverage::estimate_noise`, `lib.rs:269`, stage 02) and edge-width measurement
(`coverage::intake_scale`, `lib.rs:282`, stage 02). Followed by `label_image` (mark `labels`,
`lib.rs:321`) and despeckling (mark `despeckle`, `lib.rs:324`).

## What problem this solves

A raster does not come labelled with "this many inks." Two failure directions are both real
and both visible in the final SVG:

- **Too few inks** merges colours the artist kept apart, and the region that should have been
  two flat fills gets painted as one flat colour — or, worse, the merged residual gets explained
  away as a spurious gradient. The module doc comment gives a concrete case: "Ten concentric
  rings of ten distinct hues came back as six colours, because the five pale rings fell inside
  the threshold of each other — and the five that vanished then made their regions look like
  gradients, which is where a DISTS@4x of 0.197 came from" (`color.rs:658-661`).
- **Too many inks** turns measurement noise, JPEG ringing, or one anti-aliased ramp into
  dozens of spurious colours, each of which becomes its own face, its own boundary, and its own
  path. `SOFT_SAME_INK_DE00`'s doc comment gives the concrete cost: "76 distinct fills where the
  drawing has five, `#030303` alone emitted as 82 separate paths against a single `#000000` at
  1x" (`color.rs:434-435`).

Both failures come from the same root cause: **a fixed distance threshold cannot be right
everywhere in a perceptual colour space.** Saturated inks sit far apart and survive any
reasonable threshold; pale ones cluster close together and do not, even when the artist meant
them as distinct. The module's answer is to stop asking "is this colour close enough to an
existing one?" and start asking "does keeping this colour separate pay for itself?" — a model
selection question, with an explicit cost function, rather than a clustering question with an
implicit one.

## Inputs and outputs

**Input:**
- `rgb: &[[f32; 3]]` — sRGB pixels, row-major.
- `width`, `height`.
- `merge_distance: f32` — the fixed OKLab-distance threshold two colours must clear before either
  is even considered separate (`DEFAULT_MERGE_DISTANCE`, see below).
- `max_colors: usize` — hard cap on palette size.
- `ev: PaletteEvidence` (`color.rs:782-796`) — see below.

**Output:** `Palette` (`color.rs:568-581`):

| field | type | meaning |
|---|---|---|
| `colors` | `Vec<Oklab>` | the recovered inks, in OKLab |
| `rgb` | `Vec<[f32; 3]>` | the same, converted to sRGB |
| `weight` | `Vec<f32>` | fraction of pixels assigned to each entry |
| `alpha` | `Vec<f32>` | opacity of each entry; all `1.0` until `split_alpha_inks` runs |

## How it works

### `PaletteEvidence` — what the palette needs to know about the measurement

```rust
pub struct PaletteEvidence {
    pub sigma_noise: f64,     // per-channel pixel noise, sRGB units, from coverage::estimate_noise
    pub lambda: f64,          // nats per parameter — gradient::bic_lambda(width*height)
    pub noise_sigmas: f32,    // how many measured sigmas apart before two modes count as two inks
    pub same_ink_de00: f32,   // perceptual floor below which two colours are one ink regardless
}
```

(`color.rs:782-796`). The doc comment on the struct explains why it is a named struct rather than
three trailing arguments: "These three arrived as trailing numbers and were easy to transpose —
two `f64` and an `f32`, all plausible in any order, and a swap would have quietly changed how
many inks the image was found to have. Naming them makes that impossible" (`color.rs:779-781`).

Where each field comes from, concretely, at the call site (`lib.rs:298-309`):

- `sigma_noise` — `coverage::estimate_noise` on the whole image's luminance (stage 02).
- `lambda` — `gradient::bic_lambda(img.width * img.height)` = `0.5 * ln(n)` (`gradient.rs:297-299`),
  the Bayesian information criterion choice: "it grows slowly with region size, so a large region
  has to earn a gradient with proportionally more evidence than a small one does"
  (`gradient.rs:294-296`). This is the same `lambda` used throughout the pipeline's cost function,
  `cost = 0.5*chi2 + lambda*params`.
- `noise_sigmas` and `same_ink_de00` — switched between a clean-intake value and a soft-intake
  value by two upstream signals (`lib.rs:282-297`): `coverage::intake_scale` (edge width) and
  `ColorOptions::lossy_intake` (container format, from `lossy_container`, `lib.rs:94-113`). See
  `SOFT_NOISE_SIGMAS` and `SOFT_SAME_INK_DE00` below for what each value is and why it must stay
  gated rather than always on.

### The extraction loop

`extract_palette_mdl` (`color.rs:798-1086`) proceeds in five phases:

**1. Mode-finding by frequency, not binning.** Every pixel is converted to OKLab
(`color.rs:809`). Colours are bucketed into a coarse `24^3` grid (`BINS = 24`, `color.rs:807`)
purely to make counting tractable — the palette entry itself is the *weighted mean* of the
pixels that fall into that bucket, not the bucket centre (`color.rs:870-884`), so the recovered
colour is not snapped to a grid point. Modes are then sorted by descending pixel count, with the
bin key as a tie-breaker (`color.rs:885`) — the doc comment explains this is not cosmetic:
without a deterministic tie-break, "the junction accuracy test measured 0.054-0.134px across ten
consecutive runs of one binary" because equal-frequency colours were ordered by hash-map
iteration order (`color.rs:864-869`).

**2. For each mode, in frequency order, decide whether it survives as its own ink.** This is
where the model-selection logic lives (`color.rs:892-1027`), and it runs the following gauntlet
per candidate `c`:

- **Rarity floor.** `claim_spread` (`color.rs:729-775`) counts how many pixels would actually
  choose `c` — not the coarse bin count `n`, but a real nearest-ink pass against `nearest_px`,
  the running per-pixel distance to the nearest ink accepted so far. If the claimed share is
  below `MIN_INK_WEIGHT` and at least one ink is already accepted, the candidate is dropped
  outright (`color.rs:909-911`).
- **Perceptual floor (`SAME_INK_DE00` / `SOFT_SAME_INK_DE00`).** Converted to sRGB and compared
  by CIEDE2000 against the nearest already-accepted ink. Below the floor, the candidate is folded
  in regardless of pixel count or evidence — "Decided perceptually, before any description-length
  argument, because the argument counts pixels and pixels are exactly what an anti-aliasing ramp
  near an ink has plenty of" (`color.rs:927-929`).
- **Fixed-threshold / noise-gated escape (`nearest <= merge_distance.max(reach)`).** If the
  candidate sits within `merge_distance` — or within `reach = noise_sigmas * spread`, whichever
  is larger — of an existing ink, it is folded in *unless* it can buy its way out with the MDL
  test below (`color.rs:952-965`).
- **Blend test.** If none of the above disqualifies it, the candidate is checked against whether
  it is explained as a coverage-weighted mixture of two already-accepted inks (`blend_pairs`,
  `interior_fraction`, `straddle_fraction`) — see below.

**3. Refit.** Once the palette is decided, every entry is refit to the mean of the pixels that
actually chose it, excluding anti-aliased pixels that sit far from every entry
(`color.rs:1036-1076`).

**4. Convert to sRGB and set opacity.** All entries start at `alpha = 1.0`
(`color.rs:1079`) — extraction always runs on an opaque-matted image, because unmixing a
boundary needs two opaque colours (`lib.rs:250-255`).

### The MDL escape — `worth_it`

The core of the "worth keeping" decision is `color.rs:956-961`:

```rust
let worth_it = sigma_noise > 0.0
    && std::env::var_os("INKVEC_NO_INK_ESCAPE").is_none()
    && nearest > JND_FLOOR
    && nearest > reach
    && 0.5 * (claim as f64) * ((nearest as f64 / sigma_noise).powi(2))
        > lambda * PARAMS_PER_INK;
```

Read as a sentence: a candidate inside the fixed merge distance is kept anyway only if (a) a
noise estimate actually exists, (b) it has not been globally disabled, (c) it clears the
just-noticeable-difference floor, (d) it clears the noise-scaled `reach`, and (e) — the
substantive test — folding it into the nearest ink would cost more in residual than minting it
costs in description length:

```text
0.5 * claim * (nearest / sigma_noise)^2   >   lambda * PARAMS_PER_INK
```

The left side is the Gaussian negative log-likelihood of explaining `claim` pixels, each `nearest`
OKLab-units from where they'd be forced to sit, given measurement noise `sigma_noise`. The right
side is what a new ink costs: `lambda` nats per parameter, `PARAMS_PER_INK = 3.0` parameters
(`color.rs:279`, one per OKLab channel) per ink. "A mode with thousands of pixels and a
separation far above the noise is therefore kept however close the fixed threshold would call it,
while a handful of pixels a hair away from an existing ink is folded in — which is exactly the
behaviour wanted from both" (`color.rs:667-669`). This is the same `cost = 0.5*chi2 + lambda*params`
objective that governs curve fitting and gradient selection elsewhere in the pipeline — the
palette is not a special case, it is the same rule applied to "how many colours" instead of "how
many segments."

### The blend test — telling an anti-aliased ramp from real ink

A colour that lies on the segment between two already-accepted inks might be a real third ink
(a pastel between white and red, say) or it might just be a coverage-weighted blend pixel from
the boundary between those two inks. Both look identical in colour space near the middle of the
segment, so colour alone cannot decide. Three tests run in sequence:

1. **`blend_pairs`** (`color.rs:506-566`) checks whether `c` lands on the segment between any two
   accepted inks, within tolerance, doing the check in **linear light** (compositing is linear
   there) and also in sRGB (because some pipelines composite in gamma space anyway), trying every
   pair and keeping every match — a pale pink can be near both the white-red axis and the
   white-grey axis, and only shape evidence can then decide which, if either, actually applies
   (`color.rs:502-505`).
2. **`interior_fraction`** (`color.rs:441-488`) measures what fraction of the pixels `c` would
   claim are *interior* — all four orthogonal neighbours also claimed by `c`. This has to be
   measured against the pixels `c` would actually take from the current palette state (nearer to
   `c` than to anything already accepted), not a fixed-radius ball around `c` — "an
   anti-aliased colour sits close to one end of its ramp, so a ball around it swallows the solid
   region as well as the band... Tried that way, the green-circle case got worse rather than
   better, 29 faces to 40" (`color.rs:445-448`). Anti-aliasing is a one-pixel band with almost no
   interior; a real ink covers area and is almost entirely interior.
3. **`straddle_fraction`** (`color.rs:183-276`), run only when `interior < BLEND_INTERIOR_FRACTION`
   (a candidate too thin for the interior test to see on its own). This asks something erosion
   cannot: does the candidate's pixel neighbourhood actually straddle the two inks it supposedly
   blends — does it have, within one step, a neighbour nearer ink A *and* a neighbour nearer ink
   B? A genuine coverage ramp does, by construction; a genuinely thin ink band (two pixels wide,
   say) touches A on one side and B on the other but rarely straddles both. The doc comment gives
   the case this fixes: "The Vulcan salute's shadow strips, 2–3 px of a brown that is a mix of
   the palm and the outline, had interior 0.21 and were discarded as coverage; the palm then grew
   a radial gradient to explain them" (`color.rs:172-175`).

A candidate is finally discarded as coverage — not kept as an ink — only when it is a blend
**and** interior is below the threshold **and** the straddle fraction clears its own threshold
(`color.rs:1019-1021`).

### `label_image`

`label_image(rgb, pal)` (`color.rs:1211-1215`) assigns every pixel to its nearest palette entry
in OKLab, independent of the extraction pass — a straightforward nearest-neighbour scan.

### `split_alpha_inks`

`split_alpha_inks(labels, pal, alpha)` (`color.rs:1102-1208`) runs after labelling, as a separate
pass, and does not change how the palette itself was found. Extraction always works on an opaque
matte, which loses the distinction between "25% white over nothing" and "the transparent ground
itself" — both composite to the same colour and label as one ink, so a translucent panel
disappears into the background. This function walks each ink's true source alphas, cuts them into
groups wherever consecutive sorted values jump by more than `LEVEL_GAP = 0.15`, and keeps a group
only if it is tight (`spread <= LEVEL_SPREAD = 0.06`) and populous enough
(`n/total >= MIN_SHARE = 0.02`) (`color.rs:1103-1109, 1150-1153`). Only *flat* opacity is split —
"A face whose alpha varies across it is a glow, no single opacity describes it, and splitting it
would mint a band per level" (`color.rs:1096-1097`) — so a genuinely varying-alpha region is left
alone rather than being sliced into bands. The most opaque level keeps the ink's original entry;
each additional level mints a new palette entry sharing the same colour but a different `alpha`
(`color.rs:1170-1178`).

## `PaletteEvidence`, the guard constants, and their measured justifications

| constant | value | quoted derivation |
|---|---|---|
| `SAME_INK_DE00` | `1.5` (CIEDE2000) | `color.rs:285-302`. OKLab's lightness is cube-root-shaped, so a fixed OKLab radius is far too generous near black: "a clean render of a one-ink black logo the palette accepted #020202, #040404 and #070707 as three more inks... In CIEDE2000, which is what the bench scores with, those three sit at 0.31, 0.63 and 1.11 from black: differences no viewer can see." Swept on the screen set: `1.0` gave 0.4145→0.4140 (6 better, 5 worse) and still let `#070707` stand; `1.5` gave 0.4140→0.4124 (10 better, 6 worse, noto-emoji −0.005 dE00) and correctly split `abra_agency` back into two inks. "Mid-grey pairs 4 levels apart read 1.5, and a pair that close is not something the artwork is saying." |
| `SOFT_SAME_INK_DE00` | `5.0` | `color.rs:427-439`. Same judgement as `SOFT_NOISE_SIGMAS`, keyed to the same soft-intake trigger: on a resampled or oversampled intake, the ramp between two inks supplies "a whole family of intermediate colours that are not inks at all." Measured on the incorpo mark upscaled 4x: 76 distinct fills where the drawing has five, `#030303` alone as 82 separate paths against one `#000000` at native resolution. |
| `SOFT_NOISE_SIGMAS` | `3.0` | `color.rs:417-425`. Must stay gated: "Run unconditionally it costs **10.9%** on the 246-icon screen set — objective 0.4005 → 0.4442, measured 2026-09-08 — because on a clean intake two colours a whisker apart really are two inks and merging them throws away artwork." Switched on only by positive evidence the intake is not clean: wide edges (`SOFT_INTAKE_EDGE`) or a lossy container. |
| `NOISE_SIGMAS` | `0.0` | `color.rs:617-634`. The clean-intake default, deliberately zero. The doc comment records that the guard *works* — on a logo upscaled with the packaged SR model's own 1.87-level error, it takes output "from 5 fills, 77 paths and 12.4 KB back to 1 fill, 3 paths and 1.0 KB" — but "it is not free on a clean intake — the screen set goes from 0.4328 to 0.4451 — because a region with a real gradient has a real spread, and the guard cannot tell that from noise without knowing which it is looking at." So the *caller* decides via `INKVEC_NOISE_SIGMAS` or the soft-intake switch, rather than this being detected automatically; automatic detection is called out as "the open problem" because `coverage::estimate_noise`'s median "cannot see it, because an icon is mostly empty and its median Laplacian is zero however noisy the artwork is." |
| `SOFT_INTAKE_EDGE` | `1.75` px | `color.rs:406-415`. Measured over all 980 corpus rasters (native, 8x-supersampled): median edge width 1.00, widest native 1.50 (a noto-emoji face with soft shading). A 2x Lanczos round trip reads 1.36–1.40, 4x reads 2.80, 8x reads 4.00. The threshold sits above everything native, catching "upscales of about 3x and more"; a 2x resample is explicitly called out as indistinguishable from soft native artwork by edge width alone, and is the SR pre-pass's job instead. |
| `DEFAULT_MERGE_DISTANCE` | `0.035` (OKLab) | `color.rs:100-125`. Was `0.055`; an error-budget analysis on the 980-icon devset found that value merging inks the artwork keeps apart — "1.6% of noto-emoji's interior pixels carrying half its interior error" turned out to be two flat colours (66% error reduction when fit as two flats) rather than a gradient (only 13% reduction as a ramp). Swept on the full set: `0.055→0.4960`, `0.040→0.4931`, `0.035→0.4922` (best), `0.030→0.4941`. At `0.035` all three axes improve together (dE00 0.2005→0.1991, DISTS 0.0296→0.0293, params-vs-artist 1.46→1.44). The doc comment explicitly warns not to tune this on the screen split alone — it prefers `0.030` there, and a held-out set prefers the old `0.055` outright; "Only the full set separates them." |
| `MIN_INK_WEIGHT` | `0.004` | `color.rs:127-133`. Qualitative: anti-aliased pixels are individually rare and spread across a ramp, so no single blend colour accumulates much weight, while flat regions accumulate thousands of pixels — no specific sweep cited for `0.004` itself. |
| `BLEND_IMMUNE_WEIGHT` | `0.03` | `color.rs:135-147`. Documented rationale: above this image share, a colour is never dismissed as anti-aliasing, because "ten concentric rings lost their five pale hues to this test, one of which was 8.4% of the image." **This constant is not read anywhere in the current `extract_palette_mdl` logic** — see Open questions. |
| `BLEND_INTERIOR_FRACTION` | `0.25` | `color.rs:149-164`. Swept across two corpora with conflicting optima (real content wanted `0.015`, synthetic wanted `0.030`) before the discriminator itself was changed from abundance to shape (erosion/interior fraction). `0.25` is stated to sit below "a three-pixel ring [which] keeps about a third" of its pixels as interior — the concentric-rings case that motivated the fix. |
| `BLEND_STRADDLE_FRACTION` | `0.5` | `color.rs:166-177`. Motivated by the Vulcan-salute shadow-strip case (interior 0.21, wrongly discarded as coverage under the interior test alone). No specific sweep is cited for `0.5` itself. |
| `STRADDLE_STEP` | `0.12` | `color.rs:178-181`. "Above quantisation noise for a pair of inks that differ by more than a few levels" — qualitative, no sweep cited. |
| `JND_FLOOR` | `0.012` (OKLab) | `color.rs:611-615`. "Below this a viewer cannot tell the colours apart at all, so no amount of evidence makes them two inks rather than one measured twice." No numeric derivation shown; superseded in practice by `SAME_INK_DE00`'s perceptual floor for most cases, but still gates the MDL escape directly (`nearest > JND_FLOOR`, `color.rs:958`). |
| `PARAMS_PER_INK` | `3.0` | `color.rs:279`. One parameter per OKLab channel — a direct accounting fact, not a tuned constant. |
| `STAT_PIXELS` | `1 << 16` (65536) | `color.rs:692-705`. Bounds the cost of the per-candidate statistical passes (claim, spread, interior, straddle) so trace time does not grow with resolution beyond the corpus's 128px tuning point: "Every constant in this module was tuned on a 128 px corpus, and at or below the cap the stride is one and the arithmetic is bit-identical to visiting every pixel." |
| `INKVEC_BLEND_TMIN` (env, default `0.04`) | `color.rs:535-538` | Only interior mixtures count as a blend; `t` outside `[tmin, 1-tmin]` on the A–B axis is a different colour, not a mixture of these two. No derivation given for `0.04` specifically. |

## OKLab, sRGB, dE00 — why three colour spaces

The module doc comment states the OKLab choice directly: "VTracer's `color_precision` truncates
significant bits per RGB channel, and RGB distance is not perceptual distance — so it
simultaneously splits colours a viewer cannot tell apart and merges ones they can. In OKLab,
Euclidean distance is approximately perceptually uniform by construction, so a single threshold
means the same thing everywhere in the space" (`color.rs:6-10`). OKLab is therefore the working
space for clustering, distance comparisons, and the palette's internal representation
(`Palette::colors: Vec<Oklab>`).

But OKLab's own lightness axis is a cube root, which is exactly the property that makes a *fixed*
OKLab distance untrustworthy near black — the `SAME_INK_DE00` derivation above is a direct
demonstration of this: a distance that is enormous in OKLab terms near black (`0.078`, over twice
the merge distance) is imperceptible in CIEDE2000 terms (`0.31`). So wherever the question is "can
a viewer actually tell these apart," the module converts to CIEDE2000 (`de00`, `color.rs:330-403`,
the Sharma/Wu/Dalal 2005 formulation, verified against `skimage.color.deltaE_ciede2000` reference
values in `color.rs:1223-1231`) rather than trusting OKLab distance directly. Compositing math
(the blend test) is done in **linear-light sRGB**, because alpha compositing is linear there and
would be bent by OKLab's cube root or by gamma-encoded sRGB (`color.rs:498-501`).

In short: OKLab for "is this the same cluster," sRGB (linear and gamma) for "what colour mixture
produces this pixel," CIEDE2000 for "can anyone actually see the difference." Each answers a
different question, and using OKLab for the last one is exactly the bug `SAME_INK_DE00` exists to
fix.

## Failure modes and edge cases

- **Merging too aggressively** loses real ink and manifests downstream as a spurious gradient —
  see the concentric-rings and the palm-shadow (Vulcan salute) cases above.
- **Merging too little** turns compression ringing or an anti-aliasing ramp into dozens of
  spurious inks — see `SOFT_SAME_INK_DE00`'s incorpo-mark case above.
- **Near-black colour-axis distortion.** OKLab's cube-root lightness inflates distances near
  black; `SAME_INK_DE00` is the fix, applied "before the MDL escape, not inside it" because "a
  description-length argument cannot rescue an ink nobody can distinguish" (`color.rs:295-296`).
- **The noise guard is not automatic.** `coverage::estimate_noise`'s global median is blind on a
  mostly-empty icon — see `NOISE_SIGMAS`'s doc comment, quoted above — so the caller must supply
  evidence the intake is degraded (edge width, or container format) rather than the palette
  inferring it from pixel statistics alone.
- **Non-determinism from hash-map iteration order** was a real, measured bug (junction accuracy
  varying 0.054–0.134px across runs) and is fixed by carrying the bin key as an explicit
  tie-breaker (`color.rs:864-869`).
- **`INKVEC_PALDBG=1`** prints a per-candidate trace of every accept/reject decision, "because a
  wrong palette does not look like a palette bug downstream — the green-circle case surfaced as a
  spurious radial gradient and twenty-seven junk paths" (`color.rs:1000-1003`).

## Environment overrides

| variable | effect | default | source |
|---|---|---|---|
| `INKVEC_NOISE_SIGMAS` | overrides `ev.noise_sigmas` | `PaletteEvidence.noise_sigmas` (0.0 clean / 3.0 soft) | `color.rs:922-925` |
| `INKVEC_SAME_INK_DE00` | overrides `ev.same_ink_de00` | `PaletteEvidence.same_ink_de00` (1.5 clean / 5.0 soft) | `color.rs:935-938` |
| `INKVEC_NO_INK_ESCAPE` | disables the MDL `worth_it` escape entirely, when set | unset (escape active) | `color.rs:957` |
| `INKVEC_BLEND_TMIN` | overrides the interior-mixture window `tmin` in `blend_pairs` | `0.04` | `color.rs:535-538` |
| `INKVEC_PALDBG` | prints per-candidate accept/reject diagnostics to stderr | unset (silent) | `color.rs:940, 1004, 1006` |

## Open questions

- **`BLEND_IMMUNE_WEIGHT` is dead code.** It is declared and carries a full measured
  justification (`color.rs:135-147`, the ten-concentric-rings 8.4%-of-image case), and it is
  referenced from `bench/sweep.py:30` and `evolve/cells/palette.md:15` as if it is an active
  tuning knob — but a repo-wide search confirms it is **never read** inside
  `extract_palette_mdl` or anywhere else in `color.rs`. The abundance-based immunity it describes
  appears to have been superseded by the interior/straddle shape tests, and the constant, its
  doc comment, and the external tooling that references it were not updated to match. This is
  worth flagging explicitly: any sweep run against it via `bench/sweep.py` is currently a no-op.
- **`sqrt(6)` in `estimate_noise`** (stage 02, but consumed here as `sigma_noise`) is asserted
  rather than derived — see `02-coverage.md`'s Open questions.
- **Several shape-test constants have no numeric derivation**, only qualitative motivation:
  `BLEND_STRADDLE_FRACTION = 0.5`, `STRADDLE_STEP = 0.12`, `MIN_INK_WEIGHT = 0.004`,
  `JND_FLOOR = 0.012`, and `INKVEC_BLEND_TMIN`'s default of `0.04`. Each is tied to a real case
  that motivated it but not to a sweep that located its specific value, unlike
  `DEFAULT_MERGE_DISTANCE`, `SAME_INK_DE00`, `SOFT_NOISE_SIGMAS`, or `SOFT_INTAKE_EDGE`, all of
  which cite specific before/after numbers.
- **`NOISE_SIGMAS` as a named constant is arguably vestigial.** The actual pipeline (`lib.rs:284`)
  computes `noise_sigmas` inline as `0.0` or `color::SOFT_NOISE_SIGMAS`, and never references
  `color::NOISE_SIGMAS` by name; the constant exists to document the clean-intake value and to be
  overridden via `INKVEC_NOISE_SIGMAS`, but nothing in the traced code path reads it directly.
  This is a milder version of the `BLEND_IMMUNE_WEIGHT` problem and worth checking during any
  future refactor of this file.
- **Automatic detection of a noisy-but-clean-looking intake remains unsolved**, and the module
  says so itself: "Making it free, by detecting the noise instead of being told about it, is the
  open problem" (`color.rs:631-633`).
