# Stage 03 — Palette

> Decides how many inks a raster contains, and what colour each one is — by minimum
> description length, not by clustering or a fixed distance threshold.

**Source:** `crates/inkvec-trace/src/color.rs` (the decisions and their constants), with the walk
in `color/mdl.rs` and the per-colour index in `color/distinct.rs`; the transparent-image mirror is
`native.rs` and `native/palette.rs`
**Entry point:** `extract_palette_mdl()` (`color.rs:766-833`), which runs `mdl::extract`
(`color/mdl.rs:117-172`)
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
(`native.rs:285-344`, called at `native.rs:852-867`). Fast mode does not run this stage; it has its
own histogram palette (`fast/front.rs:7-10`, stage 14).

## What problem this solves

A raster does not come labelled with "this many inks." Two failure directions are both real
and both visible in the final SVG:

- **Too few inks** merges colours the artist kept apart, and the region that should have been
  two flat fills gets painted as one flat colour — or, worse, the merged residual gets explained
  away as a spurious gradient. The doc comment on `extract_palette_mdl` gives a concrete case:
  "Ten concentric rings of ten distinct hues came back as six colours, because the five pale rings
  fell inside the threshold of each other — and the five that vanished then made their regions
  look like gradients, which is where a DISTS@4x of 0.197 came from" (`color.rs:779-782`).
- **Too many inks** turns measurement noise, JPEG ringing, or one anti-aliased ramp into
  dozens of spurious colours, each of which becomes its own face, its own boundary, and its own
  path. `SOFT_SAME_INK_DE00`'s doc comment gives the concrete cost: "76 distinct fills where the
  drawing has five, `#030303` alone emitted as 82 separate paths against a single `#000000` at
  1x" (`color.rs:500-501`).

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
- `ev: PaletteEvidence` (`color.rs:745-764`) — see below.

**Output:** `Palette` (`color.rs:614-629`):

| field | type | meaning |
|---|---|---|
| `colors` | `Vec<Oklab>` | the recovered inks, in OKLab, in acceptance order |
| `rgb` | `Vec<[f32; 3]>` | the same, converted to sRGB |
| `weight` | `Vec<f32>` | each ink's share of the image: the pixels whose nearest ink it is, within `merge_distance` (`color/mdl.rs:454-494`) |
| `alpha` | `Vec<f32>` | opacity of each entry; all `1.0` from this walk until `split_alpha_inks` runs |

The result always has at least one ink: with nothing accepted it is the most frequent mode, and
an empty image gives white with weight 0 (`color.rs:820-822`).

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

(`color.rs:745-764`). The doc comment on the struct explains why it is a named struct rather than
trailing arguments: "These three arrived as trailing numbers and were easy to transpose — two
`f64` and an `f32`, all plausible in any order, and a swap would have quietly changed how many
inks the image was found to have. Naming them makes that impossible" (`color.rs:747-749`).

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
  where the container lies and the edges are still one pixel wide" (`color.rs:430-433`). See
  `SOFT_NOISE_SIGMAS` and `SOFT_SAME_INK_DE00` below for what each value is and why it must stay
  gated rather than always on.

### The extraction loop

`extract_palette_mdl` (`color.rs:766-833`) documents the procedure in four stages
(`color.rs:791-810`); the walk itself is `mdl::extract` (`color/mdl.rs:117-172`):

**1. Number the distinct colours.** Every distinct colour is converted once to OKLab, to sRGB
(from the OKLab value) and to linear light (`ClassicView`, `color/mdl.rs:26-86`), and every later
per-pixel quantity is read through the pixel's colour id (see "How it is computed" below).

**2. Mode-finding by frequency, not binning.** Colours are bucketed into a coarse `24^3` grid over
OKLab (`BINS = 24`; `L` over `[0, 1]`, `a` and `b` over `[−0.4, 0.4]`, clamped; cell
`round(x · 23)` per axis) purely to make counting tractable — the candidate itself is the *mean*
OKLab colour of the pixels that fall into that cell, summed in `f64` in pixel order, not the cell
centre, so the recovered colour is not snapped to a grid point (`frequency_modes`,
`color/mdl.rs:389-435`). Modes are sorted by descending pixel count with the cell key as a
tie-breaker (`color/mdl.rs:433`). The tie-break is not cosmetic: sorting by count alone left
equal-frequency colours in hash-map order, and "the junction accuracy test measured 0.054-0.134 px
across ten consecutive runs of one binary" (recorded on the test-only oracle,
`color/reference_tests.rs:591-595`).

**3. For each mode, in frequency order, decide whether it survives as its own ink.** This is
where the model-selection logic lives (`Walk::accepts`, `color/mdl.rs:195-264`), and it runs the
following gauntlet per candidate `c`, in this order, stopping once `max_colors` inks are accepted
(`color/mdl.rs:146-148`):

- **Rarity floor.** The candidate's *claim* counts how many pixels would actually choose `c` —
  not the coarse bin count `n`, but the pixels strictly nearer to `c` than to every ink accepted
  so far, read from the per-colour nearest-ink distance (`DistinctImage::claim`,
  `color/distinct.rs:301-340`; on images over `STAT_PIXELS` it counts every `s`-th pixel and
  scales back up by `s`). If the claimed share is below `MIN_INK_WEIGHT` (0.4%) and at least one
  ink is already accepted, the candidate is dropped outright (`color/mdl.rs:214-216`). So the
  first ink accepted is exempt, and the palette is never empty. This is the opaque walk, and it
  is unchanged: on an opaque image every ink is paint, and a shape too rare to be an ink is left
  to the carve stage (stage 05), which paints it its pixels' median colour
  (`inkvec-cli/tests/pipeline.rs:355-356`). The transparent-image walk exempts one more candidate,
  the first ink that draws anything; see below.
- **Perceptual floor (`SAME_INK_DE00` / `SOFT_SAME_INK_DE00`).** Converted to sRGB and compared
  by CIEDE2000 against the accepted ink nearest to it in OKLab (`same_ink_as_accepted`,
  `color.rs:850-877`, called at `color/mdl.rs:223-225`). Below the floor, the candidate is folded
  in regardless of pixel count or evidence: "A description-length argument cannot rescue an ink
  nobody can distinguish, so this floor is applied before the MDL escape, not inside it"
  (`color.rs:261-263`).
- **Fixed-threshold / noise-gated separation (`nearest <= merge_distance.max(reach)`).** If the
  candidate sits within `merge_distance` — or within `reach = noise_sigmas * spread`, whichever
  is larger — of an existing ink, it is folded in *unless* it can buy its way out with the MDL
  test below (`color/mdl.rs:222`, `color/mdl.rs:226-245`). `spread` is the lower median distance
  from `c` of the pixels it claims within `merge_distance` (`DistinctImage::spread`,
  `color/distinct.rs:342-359`), measured only when `noise_sigmas` is non-zero
  (`color/mdl.rs:207-213`). The doc comment on `spread` records why it is members, not
  territory ("a spread over that rejected every colour after the first (screen set 0.4328 ->
  1.2461 before it was restricted to `tol`)"), and the median, not the mean ("a mean is pulled up
  by them (2 % on the screen set)").
- **Blend test.** If none of the above disqualifies it, the candidate is checked against whether
  it is explained as a coverage-weighted mixture of two already-accepted inks, and whether it is
  thin and straddling (`BlendEvidence`, `color/mdl.rs:284-344`) — see below.

**4. Refit.** Once the palette is decided, every entry is moved to the mean of the pixels that
chose it, where a pixel chooses its nearest ink only if that ink is within `merge_distance`, so
anti-aliased pixels far from every entry do not pull the means (`refine_to_members`,
`color/mdl.rs:437-494`). All entries leave the walk with `alpha = 1.0` (`color/mdl.rs:165`) —
extraction always runs on an opaque-matted image, because unmixing a boundary needs two opaque
colours (`inkvec-trace/src/lib.rs:255-256`).

### The MDL escape — `worth_it`

The core of the "worth keeping" decision is `color/mdl.rs:236-241`:

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
(`color.rs:244-245`, one per OKLab channel) per ink. The doc comment is explicit about the units:
`nearest` is an OKLab distance and `sigma_noise` an sRGB one, "both scales run over about
`[0, 1]`, and the test treats the ratio as a number of standard deviations" (`color.rs:814-818`).
"A mode with thousands of pixels and a separation far above the noise is therefore kept however
close the fixed threshold would call it, while a handful of pixels a hair away from an existing
ink is folded in — which is exactly the behaviour wanted from both" (`color.rs:787-789`). This is
the same `cost = 0.5*chi2 + lambda*params` objective that governs curve fitting and gradient
selection elsewhere in the pipeline — the palette is not a special case, it is the same rule
applied to "how many colours" instead of "how many segments."

### The blend test — telling an anti-aliased ramp from real ink

A colour that lies on the segment between two already-accepted inks might be a real third ink
(a pastel between white and red, say) or it might just be a coverage-weighted blend pixel from
the boundary between those two inks. Both look identical in colour space near the middle of the
segment, so colour alone cannot decide. Three tests run in sequence, each only when the one
before it leaves the verdict open (`BlendEvidence::measure`, `color/mdl.rs:296-336`):

1. **`blend_pairs_cached`** (`color.rs:522-612`) checks whether `c` lands on the segment between
   any two accepted inks: `t = ((p − A) · (B − A)) / |B − A|²` must lie in
   `[BLEND_TMIN, 1 − BLEND_TMIN]` and the residual to the nearest point of the chord, measured
   back in OKLab, must be at most `1.6 × merge_distance` (`color/mdl.rs:299`). The check runs in
   **linear light** (compositing is linear there) and also in sRGB (because some pipelines
   composite in gamma space anyway) (`color.rs:530-532`), trying every pair and keeping every
   match — "A pale pink is within tolerance of the white–grey axis as well as the white–red one,
   and only the pair its pixels actually lie between can say whether it straddles them; the
   caller tries them all" (`color.rs:534-537`).
2. **`interior`** (`DistinctImage::interior`, `color/distinct.rs:361-395`), measured only for a
   blend, is one step of 4-neighbour erosion of the pixels `c` claims: the fraction whose four
   in-image neighbours are also claimed (a neighbour outside the image counts as claimed). This
   has to be measured against the pixels `c` would actually take from the current palette state,
   not a fixed-radius ball around `c`: "A ball is the obvious choice and it is wrong: an
   anti-aliased colour sits close to one end of its ramp, so a ball around it swallows the solid
   region as well as the band, and the band then measures as solid (tried: the green-circle case
   went from 29 faces to 40)" (`color/distinct.rs:367-371`). Anti-aliasing is a one-pixel band
   with almost no interior; a real ink covers area and is almost entirely interior.
3. **`straddle`** (`color/mdl.rs:346-387`, counted by `Neighbourhoods::straddle`,
   `color/distinct.rs:493-516`), run only when `interior < BLEND_INTERIOR_FRACTION`
   (`color/mdl.rs:311`) and taken as the largest over the matching pairs. This asks something
   erosion cannot: does the candidate's pixel neighbourhood actually straddle the two inks it
   supposedly blends — does a claimed pixel's 3x3 neighbourhood hold a pixel further towards ink
   A *and* one further towards ink B along their axis, by `STRADDLE_STEP` (clipped to half the
   room left on each side, floor 0.02)? A genuine coverage ramp does, by construction; a
   genuinely thin ink band (two pixels wide, say) touches A on one side and B on the other but
   rarely straddles both. The doc comment gives the case this fixes: "The Vulcan salute's shadow
   strips, 2–3 px of a brown that is a mix of the palm and the outline, had interior 0.21 and were
   discarded as coverage; the palm then grew a radial gradient to explain them"
   (`color.rs:234-236`).

A candidate is finally discarded as coverage — not kept as an ink — only when it is a blend
**and** interior is below `BLEND_INTERIOR_FRACTION` **and** the straddle fraction is at least
`BLEND_STRADDLE_FRACTION` (`is_coverage`, `color/mdl.rs:338-343`).

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
the rarity count, the spread, the interior test and every straddle pair. Not from the literature:
each claimed pixel's 3x3 neighbourhood is gathered once per candidate as colour ids. The result is
bit-identical to the per-pixel loop; the per-pixel loop is kept as a test-only oracle
(`color/reference_tests.rs`, `native/reference_tests.rs`). The walk itself is `color/mdl.rs`
(opaque) and `native/palette.rs` (two grounds).

### The transparent-image walk (`native::extract_palette`)

When transparency is traced natively, the palette is the same walk with every colour a two-ground
point (`Ink2`, `native.rs:76-120`): the colour over white and over a mid-grey second ground
(`SECOND_GROUND = 0.5`, `native.rs:122-128`), with distance the larger of the two OKLab distances,
so white paint and the clear ground are far apart even though they look identical over white. The
clear ground comes out as an ink of its own at opacity 0. The differences from the opaque walk are
listed on the function (`native.rs:293-302`):

- the clear ink (opacity at or below `CLEAR_INK_ALPHA = 0.02`, `native.rs:374-375`) does not count
  against `max_colors`; once the cap is full the scan continues only to find it
  (`native/palette.rs:185-199`);
- nor does it use up the **rarity exemption**: "the first ink that draws something skips the
  `MIN_INK_WEIGHT` gate as the first ink does, so a lone small shape on a clear canvas is an ink"
  (`native.rs:297-299`);
- a translucent candidate that is not a blend must have an interior
  (`BlendEvidence::measure`, `native/palette.rs:366-418`);
- there is no `INKVEC_MERGE_DE00` experiment and the same-ink floor does not print.

The rarity exemption is `rarity_exempt` (`native/palette.rs:227-261`), applied at
`native/palette.rs:301-303`: a candidate skips the `MIN_INK_WEIGHT` gate when nothing is accepted
yet, as in the opaque walk, or when it is not clear (opacity above `CLEAR_INK_ALPHA`) and every
accepted ink is. Its doc comment gives the case. On a transparent canvas the walk's first ink is
nearly always the clear ground, the commonest colour, and the clear ground draws nothing. With
only the opaque walk's exemption, "an image whose only paint covers less than 0.4 % of the canvas
got a palette of the clear ink alone: a lone 50 px² disc on a 128 px transparent canvas (0.31 %)
was traced to an empty SVG" — the carve stage still cut the disc out as a face, but named it by
the nearest palette entry over white, the clear ink, so it was emitted at opacity 0. Every other
gate still applies to the exempt candidate, so a translucent anti-aliased rim without an interior
is still rejected, and the exemption changes a palette only when a rare visible candidate comes up
while no visible ink has been accepted: "a transparent canvas whose paint is all rare". A rare
colour beside an accepted visible ink is still rare; on that path the carve stage names such a
feature by the nearest visible ink (`name_carved_paint`, `CARVED_PAINT_ALPHA`, stage 05). The
test-only oracle got the same rule, so it still checks the per-point rewrite and nothing else
(`native/reference_tests.rs:252-258`). Tests: `a_lone_small_shape_on_the_clear_ground_is_an_ink`
(`native/tests.rs:1035-1058`), `a_rare_colour_beside_a_visible_ink_is_still_rare`
(`native/tests.rs:1060-1077`), and end to end `a_lone_small_shape_on_a_transparent_canvas_is_drawn`
(`inkvec-cli/tests/pipeline.rs:334-341`).

The code labels the rule (`native/palette.rs:252-257`): **Not from the literature** — "the rarity
gate and its exemption are rules of this walk, because the published quantisers have no clear ink
that draws nothing." **See also:** Heckbert, P. (1982), "Color image quantization for frame buffer
display", *ACM SIGGRAPH Computer Graphics* 16(3):297-307, doi:10.1145/965145.801294, "whose
popularity algorithm keeps the most frequent colours and drops rare ones. Rare colours that matter,
such as a small isolated shape, are the known weakness of that rule."

### `label_image`

`label_image(rgb, pal)` (`color.rs:1015-1034`) assigns every pixel to its nearest palette entry in
OKLab (ties to the lower index), independent of the extraction pass: a nearest-neighbour scan,
made once per distinct colour and read back through each pixel's colour id (`color/mdl.rs:496-510`).
It is hard labelling: "an anti-aliased pixel gets whichever ink is closest, often a third colour,
which is what `crate::regions::absorb_blend_slivers` later repairs" (`color.rs:1018-1020`).

### `split_alpha_inks`

`split_alpha_inks(labels, pal, alpha)` (`color.rs:879-1013`) runs after labelling, as a separate
pass, only when the caller asked for alpha inks and passed the source alpha
(`inkvec-trace/src/lib.rs:523-527`); it does not change how the palette itself was found, and the
native path does not use it (its palette carries opacity directly, `native.rs:289-291`).
Extraction always works on an opaque matte, which loses the distinction between "25% white over
nothing" and "the transparent ground itself" — both composite to the same colour and label as one
ink, so a translucent panel disappears into the background. For each ink with at least 16 pixels
(`color.rs:930`), this function sorts the ink's true source alphas, cuts them into groups wherever
consecutive values jump by more than `LEVEL_GAP = 0.15`, and keeps a group as a level only if it is
tight (`spread <= LEVEL_SPREAD = 0.06`) and populous enough (`n/total >= MIN_SHARE = 0.02`)
(`color.rs:908-914`, `color.rs:933-959`); a level's mean below `CLEAR = 0.05` snaps to 0, and the
transparent group counts as a level of its own (`color.rs:951-954`). Only *flat* opacity is split —
"A face whose alpha varies across it is a glow, no single opacity describes it, and splitting it
would mint a band per level" (`color.rs:887-889`) — so a genuinely varying-alpha region is left
alone rather than being sliced into bands. One level just sets the ink's opacity. With two or more,
the most opaque level keeps the ink's original entry; each additional level mints a new palette
entry sharing the same colour, weight 0 and a different `alpha`, and every pixel of the ink moves to
the entry whose opacity is nearest its own (`color.rs:968-1011`).

## `PaletteEvidence`, the guard constants, and their measured justifications

| constant | value | quoted derivation |
|---|---|---|
| `SAME_INK_DE00` | `1.5` (CIEDE2000) | `color.rs:251-268`. OKLab's lightness is cube-root-shaped, so a fixed OKLab radius is far too generous near black: "on a clean render of a one-ink black logo the palette accepted #020202, #040404 and #070707 as three more inks ... In CIEDE2000, which is what the bench scores with, those three sit at 0.31, 0.63 and 1.11 from black: differences no viewer can see." Swept on the screen set: `1.0` gave 0.4145→0.4140 (6 better, 5 worse) and still let `#070707` stand; `1.5` gave 0.4140→0.4124 (10 better, 6 worse, noto-emoji −0.005 dE00) and correctly split `abra_agency` back into two inks. "Mid-grey pairs 4 levels apart read 1.5, and a pair that close is not something the artwork is saying." |
| `SOFT_SAME_INK_DE00` | `5.0` | `color.rs:493-505`. Same judgement as `SOFT_NOISE_SIGMAS`, keyed to the same soft-intake trigger: on a resampled or oversampled intake, the ramp between two inks supplies "a whole family of intermediate colours that are not inks at all." Measured on a real brand mark upscaled 4x: 76 distinct fills where the drawing has five, `#030303` alone as 82 separate paths against one `#000000` at 1x. |
| `SOFT_NOISE_SIGMAS` | `3.0` | `color.rs:409-417`. Must stay gated: "Run unconditionally it costs **10.9 %** on the 246-icon screen set -- objective 0.4005 -> 0.4442, measured 2026-09-08 -- because on a clean intake two colours a whisker apart really are two inks and merging them throws away artwork." Switched on only by positive evidence the intake is not clean: wide edges (`SOFT_INTAKE_EDGE`), a lossy container, or measured ringing (`SOFT_RINGING`). |
| `NOISE_SIGMAS` | `0.0` | `color.rs:663-680`. The clean-intake value, deliberately zero, and read by name at both call sites (`inkvec-trace/src/lib.rs:357-361`, `native.rs:837-841`). The doc comment records that the guard *works* — on a logo upscaled with the packaged SR model's own 1.87-level error, it takes output "from 5 fills, 77 paths and 12.4 KB back to 1 fill, 3 paths and 1.0 KB" — but "it is not free on a clean intake -- the screen set goes from 0.4328 to 0.4451 -- because a region with a real gradient has a real spread, and the guard cannot tell that from noise without knowing which it is looking at." So "the caller decides, because the caller knows where its raster came from": upscaled, compressed or resampled input gets `SOFT_NOISE_SIGMAS` from the soft-intake gate. |
| `SOFT_INTAKE_EDGE` | `1.75` px | `color.rs:398-407`. Measured over all 980 corpus rasters (native, 8x-supersampled): median edge width 1.00, widest native 1.50 (a noto-emoji face with soft shading), then 1.43 and 1.26. A 2x Lanczos round trip reads 1.36–1.40, 4x reads 2.80, 8x reads 4.00. The threshold sits above everything native, catching "upscales of about 3x and more"; a 2x resample is indistinguishable from soft artwork by edge width alone and is what the SR pre-pass (`--sr auto`) exists for. |
| `SOFT_RINGING` | `0.12` | `color.rs:419-434`. Set from the false-positive side, because the guard it opens costs 2.8% on the screen set. Measured over 240 clean corpus rasters at tier 128ss: median 0.0000, p90 0.0062, p99 0.0333, max 0.1023 (`synthetic/rings_concentric`, a test pattern that genuinely oscillates); the highest real artwork is a noto-emoji at 0.0370. |
| `SOFT_RINGING_LARGE` | `0.05` | `color.rs:436-449`. The ring band is fixed in pixels, and at 128 px the ring of one glyph edge lands on the next, so clean art scores up to 0.1023. At 512 px the same artwork scores at most 0.0430 over 64 images, the compressed versions 0.086 median; a 0.05 gate has zero false positives and catches 88-89% of JPEG at qualities 85, 60 and 40, where the conservative gate catches 19-33%. |
| `RINGING_MIN_DIM` | `256` px | `color.rs:451-452`, used at `inkvec-trace/src/lib.rs:350-354`. The smallest side at which `SOFT_RINGING_LARGE` applies; below it the conservative number stands "and the screen set is bit-identical" (`color.rs:447-448`). No sweep cited for 256 itself. |
| `MEASURED_SIGMA_SCALE` | `1.0` | `color.rs:454-463`. How much of `regularize::residual_sigma` to believe when the noise is raised after labelling on a soft intake. Measured on 78 JPEG-re-encoded-as-PNG traces: at 1.0 the parameter count falls 31.5% and colour error 15.9%; the detector alone gives 22.0% and 22.2%; "The value here is the swept optimum between them." |
| `MEASURED_SIGMA_CAP` | `8.0` levels | `color.rs:465-491`. A second ceiling on the measured noise, now non-binding (`residual_sigma` clamps itself to 8). On 791 traces across all classes a ceiling of 8 gives colour −16.9% and parameters −33.7%, diagrams +6% colour for −52% parameters. Smaller samples had pointed the wrong way (22 diagrams read +114% colour at the high ceiling, 106 diagrams +0.7%): "Nothing about this trade should be decided on fewer than several hundred paired traces". |
| `DEFAULT_MERGE_DISTANCE` | `0.035` (OKLab) | `color.rs:175-200`. Was `0.055`; an error-budget analysis on the 980-icon devset found that value merging inks the artwork keeps apart — "1.6 % of noto-emoji's interior pixels carrying half its interior error" turned out to be two flat colours (66% error reduction when fit as two flats) rather than a gradient (only 13% reduction as a ramp). Swept on the full set: `0.055→0.4960`, `0.040→0.4931`, `0.035→0.4922` (best), `0.030→0.4941`. At `0.035` all three axes improve together (dE00 0.2005→0.1991, DISTS 0.0296→0.0293, params-vs-artist 1.46→1.44). The doc comment explicitly warns not to tune this on the screen split alone — it prefers `0.030` there, and held-out set A prefers the old `0.055` outright; "Only the full set separates them." |
| `MIN_INK_WEIGHT` | `0.004` | `color.rs:202-208`. Qualitative: anti-aliased pixels are individually rare and spread across a ramp, so no single blend colour accumulates much weight, while flat regions accumulate thousands of pixels — no specific sweep cited for `0.004` itself. Exempt: the first ink accepted (`color/mdl.rs:214`), and on the transparent-image walk also the first ink that draws something (`native/palette.rs:227-261`). |
| `BLEND_INTERIOR_FRACTION` | `0.25` | `color.rs:210-225`. Swept across two corpora with conflicting optima (real content wanted `0.015`, synthetic wanted `0.030`) before the discriminator itself was changed from abundance to shape (erosion/interior fraction). `0.25` is stated to sit below "a three-pixel ring [which] keeps about a third" of its pixels as interior — the concentric-rings case that motivated the fix. |
| `BLEND_STRADDLE_FRACTION` | `0.5` | `color.rs:227-238`. Motivated by the Vulcan-salute shadow-strip case (interior 0.21, wrongly discarded as coverage under the interior test alone). No specific sweep is cited for `0.5` itself. |
| `STRADDLE_STEP` | `0.12` | `color.rs:239-242`. "Above quantisation noise for a pair of inks that differ by more than a few levels" — qualitative, no sweep cited. Clipped to half the room left on each side, floor `0.02` (`color/mdl.rs:380-383`). |
| `BLEND_TMIN` | `0.04` | `color.rs:507-509` (was `INKVEC_BLEND_TMIN`). Only interior mixtures count as a blend; `t` outside `[tmin, 1-tmin]` on the A–B axis "is a different colour, not a blend of these two" (`color.rs:584-586`). No derivation given for `0.04` specifically. |
| blend chord tolerance | `1.6 × merge_distance` | `color/mdl.rs:299` (and `native/palette.rs:379`). How far, in OKLab, a candidate may sit from the chord between two inks and still be a blend of them. No derivation given. |
| `JND_FLOOR` | `0.012` (OKLab) | `color.rs:657-661`. "Below this a viewer cannot tell the colours apart at all, so no amount of evidence makes them two inks rather than one measured twice." No numeric derivation shown; superseded in practice by `SAME_INK_DE00`'s perceptual floor for most cases, but still gates the MDL escape directly (`nearest > JND_FLOOR`, `color/mdl.rs:238`). |
| `PARAMS_PER_INK` | `3.0` | `color.rs:244-245`. One parameter per OKLab channel — a direct accounting fact, not a tuned constant. |
| `BINS` | `24` per OKLab axis | `color/mdl.rs:394-395`. The candidate grid; the entry is the mean of its pixels, so the grid only decides what is counted together. No derivation given. |
| `STAT_PIXELS` | `1 << 16` (65536) | `color.rs:702-715`. Bounds the cost of the per-candidate statistical passes (claim, spread, interior, straddle) so trace time does not grow with resolution beyond the corpus's 128px tuning point: "Every constant in this module was tuned on a 128 px corpus, and at or below the cap the stride is one and the arithmetic is bit-identical to visiting every pixel." The stride is the smallest `s ≥ ⌈n / STAT_PIXELS⌉` coprime with the width (`color.rs:726-743`). |
| `SPREAD_SAMPLES` | `8192` | `color/distinct.rs:196-198`. Most pixels the spread's median is taken over, "which bounds the sort at any image size". |
| `CLEAR_INK_ALPHA` | `0.02` | `native.rs:374-375`. Opacity at or below which an ink is the clear ground: it is not counted against `max_colors`, and it does not use up the rarity exemption (`native/palette.rs:258-261`). No derivation given. |

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
thirty times the step at mid-grey" (`color.rs:254-256`). The `SAME_INK_DE00` derivation above is a
direct demonstration: a distance that is large in OKLab terms near black (`0.078`, over twice the
merge distance) is imperceptible in CIEDE2000 terms (`0.31`). So wherever the question is "can a
viewer actually tell these apart," the module converts to CIEDE2000 (`de00`, `color.rs:305-308`,
the Sharma/Wu/Dalal 2005 formulation in `de00_lab`, `color.rs:310-396`), held to Sharma, Wu and
Dalal's 34 published reference pairs (`color/tests.rs:10-78`) and to
`skimage.color.deltaE_ciede2000` values on sRGB pairs (`color.rs:1036-1051`), rather than trusting
OKLab distance directly. Compositing math (the blend test) is done in **linear-light sRGB**,
because alpha compositing is linear there and would be bent by OKLab's cube root or by
gamma-encoded sRGB (`color.rs:530-532`).

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
  description-length argument cannot rescue an ink nobody can distinguish" (`color.rs:261-263`).
- **The noise guard is not automatic on a clean-looking intake.** `coverage::estimate_noise`
  reads the floor on a mostly-empty icon however damaged it is — see `NOISE_SIGMAS`'s doc comment,
  quoted above — so the guard opens only on evidence the intake is degraded: edge width, container
  format, or ringing measured from the pixels (`coverage::ringing_score`). A damaged intake that
  shows none of the three is traced with the clean settings.
- **A rare small shape.** On an opaque image a shape under 0.4% of the canvas is not an ink; the
  carve stage paints it its pixels' median colour (stage 05). On a transparent canvas whose paint
  is all rare, the first visible ink is exempt from the rarity floor (`rarity_exempt`, above) —
  before 2026-10-02 a lone 50 px² disc on a clear 128 px canvas traced to an empty SVG. A second
  rare colour beside a visible ink is still not an ink.
- **Non-determinism from hash-map iteration order** was a real, measured bug (junction accuracy
  varying 0.054–0.134px across runs) and is fixed by carrying the bin key as an explicit
  tie-breaker (`color/mdl.rs:433`).
- **`INKVEC_PALDBG=1`** prints a per-candidate trace of every accept/reject decision
  (`color/mdl.rs:247-262`), "because a wrong palette does not look like a palette bug downstream
  — the green-circle case surfaced as a spurious radial gradient and twenty-seven junk paths" (the
  reason as recorded beside the same print in the test-only oracle,
  `color/reference_tests.rs:462-465`).

## Environment overrides

Since the settings cleanup (CHANGELOG, *Unreleased*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

| variable | effect | default | source |
|---|---|---|---|
| `INKVEC_PALDBG` | prints per-candidate accept/reject diagnostics, the same-ink verdicts and the intake measurements to stderr | unset (silent) | `color/mdl.rs:137`, `color/mdl.rs:247-262`, `color.rs:865-873`, `native/palette.rs:183`, `native/palette.rs:326-336`, `inkvec-trace/src/lib.rs:387-392` |
| `INKVEC_MERGE_DE00` (*research build*) | replaces the OKLab merge radius with a CIEDE2000 radius | unset | `color/mdl.rs:138-143`, `color/mdl.rs:226-233` |
| `INKVEC_PALETTE_RGB` (*research build*) | clusters in plain sRGB instead of OKLab | off | `color.rs:91-105` |
| `INKVEC_NOISE_SIGMAS` (*removed*) | overrode `ev.noise_sigmas` | — | the soft-intake gate chooses `NOISE_SIGMAS` / `SOFT_NOISE_SIGMAS` |
| `INKVEC_SAME_INK_DE00` (*removed*) | overrode `ev.same_ink_de00` | — | the soft-intake gate chooses `SAME_INK_DE00` / `SOFT_SAME_INK_DE00` |
| `INKVEC_NO_INK_ESCAPE` (*removed*) | disabled the MDL `worth_it` escape | — | the escape is always on (`color/mdl.rs:236-245`) |
| `INKVEC_BLEND_TMIN` (*removed*) | overrode the interior-mixture window `tmin` in the blend test | — | now the constant `BLEND_TMIN` (`color.rs:507-509`) |

## Open questions

- **`BLEND_IMMUNE_WEIGHT` — resolved.** The constant, documented here before as dead code, is no
  longer declared anywhere in `color.rs`. `bench/sweep.py:29-33` records that it was dropped from
  the sweep on 2026-09-08 because nothing read it; every sweep entry is now proved live by
  `assert_live` before it is swept.
- **`NOISE_SIGMAS` as a named constant — resolved.** Both call sites now read
  `color::NOISE_SIGMAS` by name for the clean intake (`inkvec-trace/src/lib.rs:357-361`,
  `native.rs:837-841`).
- **Several shape-test constants have no numeric derivation**, only qualitative motivation:
  `BLEND_STRADDLE_FRACTION = 0.5`, `STRADDLE_STEP = 0.12`, `MIN_INK_WEIGHT = 0.004`,
  `JND_FLOOR = 0.012`, `BLEND_TMIN = 0.04`, the `1.6 × merge_distance` chord tolerance and
  `CLEAR_INK_ALPHA = 0.02`. Each is tied to a real case that motivated it but not to a sweep that
  located its specific value, unlike `DEFAULT_MERGE_DISTANCE`, `SAME_INK_DE00`,
  `SOFT_NOISE_SIGMAS`, `SOFT_INTAKE_EDGE` or the ringing gates, all of which cite specific
  before/after numbers.
- **Automatic detection of a noisy-but-clean-looking intake.** `NOISE_SIGMAS`'s doc comment still
  calls it "the open problem": "Making it free, by detecting the noise instead of being told about
  it" (`color.rs:676-679`). Part of it has since been answered from the pixels: the ringing score
  opens the gate on a JPEG re-saved as PNG, and on a soft intake the noise is then measured against
  the labels (`inkvec-trace/src/lib.rs:429-491`). Neither runs on an intake that shows no edge,
  container or ringing evidence, and the ringing score is built around a JPEG artefact: it "fires
  on only 21% of VAE output against 88% of JPEG" (`inkvec-trace/src/lib.rs:449-451`).
- **The opaque walk has no counterpart of the transparent walk's rarity exemption.** By design:
  on an opaque image every ink is paint and the carve stage draws a rare shape in its own colour,
  so the rule was not mirrored. The fork table in `native.rs:25-52` is the list to check when either
  walk changes.
