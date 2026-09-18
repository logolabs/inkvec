# Stage 04 — Regions

> Turns a per-pixel palette label into a set of connected, single-ink faces: speckle gone, anti-aliasing folded into its neighbours, disconnected same-colour shapes told apart, and pixel-corner ambiguities resolved by the image.

**Source:** `crates/inkvec-trace/src/lib.rs`
**Entry point:** `trace_color_full_with_alpha()` (`lib.rs:256`), stages within it
**Pipeline position:** after `color::label_image` (mark `"labels"`, `lib.rs:321`), before `gradient::merge_gradient_bands_with_ink` (mark `"merge_bands"`, `lib.rs:373`) and `planar::build` (mark `"build_map"`, `lib.rs:453`)

This stage is four sub-stages run in sequence, with stopwatch marks:

| mark | line | function(s) |
|---|---|---|
| `despeckle` | `lib.rs:324` | `despeckle` |
| `blend_absorb` | `lib.rs:345` | `absorb_blend_slivers`, `reassign_blend_pixels` |
| `split` | `lib.rs:409` | `split_components` |
| `saddles` | `lib.rs:451` | `merge_saddle_faces` (calls into `planar::build`'s own corner logic downstream) |

## What problem this solves

`color::label_image` assigns every pixel the palette entry nearest its measured colour. That is a per-pixel decision with no notion of shape: it does not know that a pixel is noise, that a run of pixels is only anti-aliasing between two real inks, that two blobs of the same colour on opposite sides of the canvas are different objects, or that four pixels meeting at one corner might belong to a single connected mark. Each of those is a distinct failure mode, and each gets its own pass:

- **Speckle** — an isolated pixel or tiny cluster that landed on the wrong palette entry (JPEG ringing, a stray anti-aliased pixel) becomes its own face with its own boundary unless removed first.
- **Blend slivers** — anti-aliased pixels between two real inks are frequently nearest a *third* palette entry, not either of the two they are blending. Left alone they form thin sliver faces along every boundary in the image. Measured on `mosaic_grid6` (36 flat cells, truth = 37 regions): unabsorbed, the trace produced 128 faces and 368 edges (`lib.rs:331`).
- **Disconnected same colour** — palette labelling only records colour, not connectivity. Two separate shapes of one ink must become two faces, not one, or every per-face measurement downstream (including gradient fitting) is corrupted.
- **Bowtie corners** — where two arms of one shape (or two separate shapes) meet at exactly one pixel corner, marching-squares-style label extraction cannot tell, from the labels alone, whether that corner is one connected mark or two shapes touching at a point.

## Inputs and outputs

Working state through this stage is `labels: Vec<u16>`, one palette index per pixel (`w * h` long), plus the palette `pal: color::Palette`, the composited sRGB image `rgb: &[[f32; 3]]`, the true alpha channel (`source_alpha` or `img.data[..][3]`), and `sigma_noise: f64` (from `coverage::estimate_noise`).

- `despeckle` and `absorb_blend_slivers`/`reassign_blend_pixels` **relabel pixels in place** — the array stays `w * h` long, only the label at each pixel changes.
- `split_components` **changes the indexing scheme**: it turns the palette-indexed `labels` into face-indexed `labels` (a `u16` per pixel that is now a connected-component id, not a palette index), plus `face_color: Vec<usize>` mapping each new face id back to the palette entry it came from.
- `merge_saddle_faces` **may reduce the face count** by unioning face ids at resolved corners, leaving `labels`, `face_fill`, `face_color`, and `n_faces` updated (vacated ids are left empty, not renumbered — see below).

## How it works

### Despeckle (`despeckle`, `lib.rs:1159`)

4-connected component labelling (stack-based flood fill) over the label array; any component smaller than `min_size` pixels is reassigned, whole, to whichever neighbouring label owns the most contacts along its border, with a deterministic tie-break on the label value itself (`std::cmp::Reverse(lab)` — without it a speckle with two equally common neighbours would be absorbed based on hash-map iteration order, which differs between runs, `lib.rs:1221-1226`). This is Potrace's `turdsize` generalised from contour area to component pixel count (`lib.rs:1154`).

`min_size` is `opts.min_region`, described below.

### Blend absorption (`blend_absorb`, `lib.rs:332-344`)

Two passes, both gated by `INKVEC_NO_ABSORB` being unset:

**`absorb_blend_slivers` (`lib.rs:862`, 175 lines).** Runs up to two rounds. In each round:
1. Recompute 4-connected components of the current labelling.
2. A component **qualifies** as a candidate sliver only if it is thin (fewer than a fifth of its pixels are interior — every 4-neighbour also in the component) and its foreign contacts are dominated by up to three neighbouring labels (the top three account for at least 4/5 of all foreign contacts, `lib.rs:960-979`). Three, not two, because a sliver at a junction or under a shape that pokes out from underneath another is a mixture of *three* inks, not two.
3. Build the candidate ink list from those neighbours' palette colours; if any pixel in the component is translucent (`alpha[p] < 0.99`) and the backdrop colour (pure white, `BACKDROP = [1.0, 1.0, 1.0]`, `lib.rs:759`) is not already among them, add it — a translucent edge pixel is by construction mixed with whatever it was composited onto.
4. Per pixel, call `mixture()` (`lib.rs:779`) — explain the pixel's colour as the best-fit convex combination of exactly two or exactly three of the candidate inks, and return the ink with the largest weight in that mixture, not the nearest colour by simple distance. A pixel 70% dark outline / 30% mid-brown is nearer mid-brown in plain colour distance but belongs to the outline (`lib.rs:774-778`, from the `270c` hand test case).
5. The component is only absorbed if at least 4/5 of its pixels pass the mixture test within tolerance `tol = max(3·sigma_noise, 0.025)` (`lib.rs:872`). Debug output for each rejection reason is available under `INKVEC_ABSDBG`.

**`reassign_blend_pixels` (`lib.rs:1055`).** Catches slivers the component-level pass misses: on a colour mosaic, anti-aliasing between two cells can land on a third cell's *own* colour and fuse into a large component with it, which fails the thinness test outright. This pass instead works per pixel, for `ROUNDS = 4` erosion-like rounds:
- Every pixel looks at the labels present in its 3×3 neighbourhood (up to 4, own label first).
- If at least two labels are present, it is offered every pair as a blend hypothesis via `mixture()`, again with the backdrop added when translucent.
- The pixel moves to the ink with the largest mixture weight only if that model's residual is within tolerance **and** beats the pixel's own label's residual by at least half (`r <= tol && r < 0.5 * resid_own`, `lib.rs:1131`).
- All decisions in a round are taken against a snapshot (`snap = labels.to_vec()`) and computed in parallel with `rayon`, then applied afterwards — the result is independent of scan order and identical to a single-threaded pass (`lib.rs:1070-1073`).

If either pass changed anything, `despeckle` is run again (`lib.rs:341-343`) to clean up whatever the reassignment left behind.

### Split into connected faces (`split`, `split_components`, `lib.rs:706`)

Flood-fills the palette-indexed label array into 4-connected components, in raster order of first pixel, producing a face id per pixel and a `face_color: Vec<usize>` mapping face id back to palette index. This is the step that turns "every pixel of this colour" into "this one connected shape". Skipping it merges *every* disconnected same-coloured shape in the image into a single region — which is wrong both as a matter of editing (an editor could no longer move one shape independently) and as a matter of measurement: concentric rings quantised to six palette entries put several non-adjacent annuli of slightly different colour into one "region", whose flat residual was then a thousand times what noise could explain — the fill fitter dutifully explained the difference with a radial gradient it should never have been offered (`lib.rs:398-407`). Component ids are capped at `u16::MAX`; overflow pixels fall back to palette label 0 (`lib.rs:748-754`).

### Saddle resolution (`saddles`, `merge_saddle_faces`, `lib.rs:544`)

**Off by default** — gated by `INKVEC_SADDLE` being set to something other than `"0"` (`lib.rs:561`). See Environment overrides.

For every interior pixel corner `(i, j)` where `1 <= i < w`, `1 <= j < h`, look at the four pixels meeting there: `nw = (i-1,j-1)`, `ne = (i,j-1)`, `sw = (i-1,j)`, `se = (i,j)`.

1. Skip unless all four **dual edges exist** — i.e. `nw != ne && nw != sw && ne != se && sw != se` (`lib.rs:587`). This is the four-way-junction case marching squares cannot resolve from labels alone.
2. Skip unless the two **diagonals** carry exactly two distinct **inks** (palette indices via `face_color`), one ink per diagonal: `face_color[nw] == face_color[se]` and `face_color[ne] == face_color[sw]`, and the two inks differ (`lib.rs:594`). This is the marching-squares ambiguous configuration — an "X" of two inks touching only at the corner — which is exactly what could be either one connected shape (bowtie welded shut) or two shapes correctly touching at a point.
3. Compute the fraction of the NW/SE ink covering each of the four pixels by projecting its colour onto the segment between the two palette colours (`cover()`, `lib.rs:607-613`) — the same unmixing the boundary refinement stage uses.
4. Evaluate the **bilinear interpolant of that coverage field at the corner itself**:
   ```
   den = cnw + cse - cne - csw
   num = cnw*cse - cne*csw
   at_corner = num / den      (or the plain mean (cnw+cne+csw+cse)/4 if |den| < 1e-9, the degenerate case)
   ```
   This is the **asymptotic decider** of Nielson & Hamann (1991) for the marching-squares saddle ambiguity: which side of a bilinear-interpolated field a saddle point's value actually falls on. `at_corner > 0.5` means the NW/SE diagonal owns the corner; otherwise NE/SW does.
5. Propagate the pixel noise `sigma_noise` through this computation to a `sigma_corner` (a closed-form gradient-propagation for the degenerate case, and the full Jacobian `g` of `at_corner` with respect to the four coverages otherwise, `lib.rs:639-651`).
6. The reading is **decisive** only if `|at_corner - 0.5| > SADDLE_SIGMAS * sigma_corner` with `SADDLE_SIGMAS = 3.0` (`lib.rs:515`). Below that threshold the two readings are statistically indistinguishable and the corner is left exactly as found — this matters concretely for a checkerboard pattern (a QR code, a chequered flag), whose corners sit at 0.5 to four decimal places because the two squares really do meet at a point and *neither* reading is truer; forcing a decision there on the sign of a rounding error moved one icon's colour error by 0.09 on its own (`lib.rs:624-638`).
7. A decisive corner unions the two faces on the winning diagonal via union-find (`find`/`parent`, `lib.rs:565-578`).

After the scan, every pixel's label is rewritten to its union-find root (`lib.rs:691-693`). **Vacated face ids are left empty rather than renumbered** — the id owns no pixels, no edges, and emits nothing, and renumbering to close the gap would perturb every other face's id for no benefit (`lib.rs:682-690`).

`planar::build`, called immediately afterward (`lib.rs:453`), is what actually turns a merged corner into two boundary points instead of one junction: it splits a shared corner into a copy per face only when exactly one diagonal is a single face — precisely the state `merge_saddle_faces` produces at a corner it joins. This mechanical rule is covered by `crates/inkvec-trace/src/planar.rs`'s `saddle_tests` module (`planar.rs:1526-1594`): `four_inks_meeting_at_a_corner_stay_a_junction` confirms an unreadable four-way corner (no diagonal reducible to one face) keeps its junction, and `a_corner_two_shapes_share_becomes_two_points` / `splitting_gives_each_shape_its_own_copy_of_the_corner` confirm that once one diagonal is a single face, the corner becomes two independent points, one per touching shape, and every edge still separates exactly the two faces it always did.

## Constants and thresholds

| name | value | controls | stated derivation |
|---|---|---|---|
| `ColorOptions::min_region` (default) | `4` (library default, `lib.rs:210`) but the CLI always overrides it with `args.min_area.max(1.0) as usize`, and `args.min_area` defaults to `2.0` (`args.rs:64`) — so the shipped default is **2 pixels** | `despeckle`'s `min_size`, and (via `min_region.max(2)`) the carve pass's minimum feature size | See long comment at `crates/inkvec-cli/src/lib.rs:805-816`, quoted below |
| `SADDLE_SIGMAS` | `3.0` | how many propagated-noise sigma a corner's coverage value must clear the 0.5 mark by before a saddle is resolved | `lib.rs:512-514`: "three sigma of the propagated coverage noise. Below that the two readings are indistinguishable, and the corner keeps the junction it has always had." No further derivation than "three sigma" as a standard significance bar; not tuned against a benchmark in the comment. |
| absorption thinness gate | interior pixels `< area / 5` | which components are candidate slivers | `lib.rs:851` doc comment: "fewer than a fifth of its pixels are interior"; no numeric derivation beyond the description |
| absorption dominant-neighbour gate | top-3 neighbours cover `>= 4/5` of foreign contacts | which components are candidate slivers | `lib.rs:851-852`: "two neighbour labels account for at least 80% of its foreign contacts" (code allows up to three, doc comment says two — see Open Questions) |
| absorption pass threshold | `>= 4/5` of pixels within tolerance | whether a qualifying component is actually absorbed | `lib.rs:853`: "at least 80% of its pixels lie within tolerance of the straight line between those two neighbours' palette colours" |
| `tol` (both blend passes) | `max(3 * sigma_noise, 0.025)` sRGB | how close a pixel's colour must be to a mixture hypothesis to count as a match | derived from measured noise, floored at 0.025 (~6.4/255) with no stated reason for that specific floor |
| `reassign_blend_pixels::ROUNDS` | `4` | how many erosion-like rounds the per-pixel reassignment runs | no stated derivation |
| reassign "beats own label" margin | residual `< 0.5 * resid_own` | how much better a blend hypothesis must be than the pixel's own label before it moves | no stated derivation beyond "beats... by at least half" (`lib.rs:1049`) |
| **9-pixel colour-mode floor (rejected)** | tried, not shipped | would have been `min_region` in colour mode | `cli/lib.rs:810-816` (quoted below): tried 2026-09-03, measured *worse* on the 980-icon set — objective `0.7885` vs `0.7673` with the 2px floor, 508 icons worse / 171 better in dE00. It erased real dots and thin details more often than it removed confetti; the shipped default stays at `--min-area` (2 px). |

The `min_region` decision, quoted directly (`crates/inkvec-cli/src/lib.rs:805-816`):

> A 2px floor is appropriate for the continuous bilevel path, but in colour mode it lets anti-aliased edge samples become their own tiny faces. Those faces split a smooth shared boundary into a literal pixel staircase before fitting. Nine pixels removes that confetti on the grid canaries while long thin artwork remains intact; an explicit --min-area always wins for deliberately tiny art. A 9-pixel colour-mode floor was tried (2026-09-03) to absorb anti-aliased stair-step faces and measured worse on the full 980-icon set: objective 0.7885 vs 0.7673 with the 2 px floor, 508 icons worse / 171 better in dE00 — it erased real dots and thin details more often than it removed confetti. Blend absorption and residual carving handle the stair-steps at the label level instead. The default therefore stays at the --min-area value (2 px).

The comment reads as written in the order the idea was tried and then reverted; the operative fact is the last sentence — the shipped default is 2 px, and the 9-pixel floor is a recorded negative result, not a live option gated by any flag. Blend absorption (this stage) and residual carving (stage 05) are what handle the stair-step confetti instead.

## Failure modes and edge cases

- **`mosaic_grid6` regression** (`lib.rs:326-331`): unabsorbed blend slivers on a 36-cell flat mosaic produced 128 faces and 368 edges against a 37-region truth — this is the canonical example of why blend absorption exists at all.
- **`thin_features` canary** (`lib.rs:859-861`): a genuine thin feature — the doc comment's example is a black hairline between two whites — must have a colour *off* the segment between its neighbours' palette colours, so it fails the blend test outright and keeps its own label. `absorb_blend_slivers` is explicitly verified against this canary never being touched.
- **`270c` hand** (`lib.rs:768-772`): an outline drawn over a shading path that extends past it by a sub-pixel margin makes every outer-edge pixel a *triple* mixture (outline + shading + backdrop). Testing pairs only, none of them registers as a blend, each pixel keeps the shading label, and the outline's outer boundary arrives at the curve fitter fragmented into three-pixel pieces. This is why `mixture()` solves triples as well as pairs, and why the backdrop colour is added as a candidate ink whenever any pixel in the group is translucent.
- **Non-deterministic tie-breaks**: both `despeckle` and (implicitly, via sorted/deterministic containers) the blend passes are careful to break ties on the label value itself rather than iteration order — the comment at `lib.rs:1221-1223` records this was a real source of run-to-run variance before the fix.
- **Two men holding hands** (`lib.rs:685-690`): the saddle merge was measured on this icon: joining faces made no difference to the corner geometry itself, but when the *absorbed* face happened to be the background, the emitter's containment tree (which stacks faces and punches holes by containment) was rewritten well beyond the corner, costing 0.06 dE00. This is recorded as the remaining, unfixed cost of the saddle pass.
- **Saddle pass net effect**: the module comment (`lib.rs:557-560`) states the pass does not yet pay for itself on the screen set — 231 of 246 icons untouched, objective moved `0.4005 -> 0.4010` (i.e. slightly worse), six icons better and nine worse, with the nine attributed to the containment-tree issue above rather than to a wrong saddle reading. This is why it ships off by default.

## Environment overrides

| variable | effect |
|---|---|
| `INKVEC_NO_ABSORB` | if set (any value), skips both `absorb_blend_slivers` and `reassign_blend_pixels` entirely (`lib.rs:510`). It is the only environment variable of this kind in the current `inkvec-trace` source (confirmed by `grep -rn "NO_ABSORB" crates/ --include=*.rs`, one hit) — this project was previously named svgify, but no old `SVGIFY_*` env var names survive in the current source. |
| `INKVEC_ABSDBG` | prints per-component rejection reasons from `absorb_blend_slivers` to stderr for components over 15 pixels (`lib.rs:953-978`) |
| `INKVEC_SADDLE` | must be set to a value other than `"0"` to turn the saddle pass **on**; it is off by default (`lib.rs:561`) |
| `INKVEC_SADDLEDBG` | prints, per resolved or tied corner, the two ink ids, the corner value, its uncertainty and the verdict (`lib.rs:654-664`) |
| `INKVEC_DUMP_LABELS` | (path) writes the post-absorption label image as a binary PPM using palette colours, for visual inspection (`lib.rs:346-348`, `dump_labels` at `lib.rs:1263`) |
| `INKVEC_TIMING` | enables the `Stopwatch` (`lib.rs:1241-1260`), printing each stage mark's wall-clock time |

## Open questions

- The doc comment on `absorb_blend_slivers` (`lib.rs:851-852`) says "two neighbour labels account for at least 80% of its foreign contacts", but the code takes up to **three** dominant neighbours (`tally.truncate(3)`, `lib.rs:970`) with the stated reason being junction/pokeout mixtures. The doc comment has not been updated to match — a reader trusting the prose alone would expect a two-ink test.
- The `tol` floor of `0.025` sRGB (roughly 6.4/255) in both blend passes (`regions.rs:278`, `regions.rs:449`) has no stated derivation in the surrounding code beyond being a floor on `3 * sigma_noise` — **Unverified:** why this specific value rather than, say, `2/255` or `1/255`.
- `reassign_blend_pixels`'s `ROUNDS = 4` and the "beats own label by at least half" (`0.5 * resid_own`) margin both carry no stated derivation in the source; they read as chosen values rather than measured ones.
- `SADDLE_SIGMAS = 3.0` (`regions.rs:13`) is justified in the comment only as a standard three-sigma significance bar ("three sigma of the propagated coverage noise"); unlike several other constants in this stage, the comment does not report a swept or rejected alternative value — **Unverified:** whether it was swept.
- The saddle pass's net effect on the 246-icon screen set (`0.4005 -> 0.4010`, six better / nine worse) means it currently ships **off**, with the nine regressions attributed to the emitter's containment-tree behaviour on a background-absorbing merge rather than to the saddle reading itself. That containment-tree interaction is an acknowledged unresolved cost, not a bug in this stage's logic — fixing it is a prerequisite for turning the pass on by default.
- `split_components` and `despeckle` each have a direct unit test calling them by name, in `regions.rs`'s own `#[cfg(test)] mod tests` (`test_split_components_disjoint`, `regions.rs:625`; `test_despeckle_removes_single_pixel`, `regions.rs:635`); `crates/inkvec-trace/tests/components.rs` additionally exercises `split_components`'s behaviour end-to-end through the public `trace_color_full` API (e.g. `concentric_rings_are_separate_faces_and_each_is_flat`), without calling it by name. `absorb_blend_slivers` and `reassign_blend_pixels` have no test calling them by name anywhere in the repository — confirmed via `grep -rn "absorb_blend_slivers\|reassign_blend_pixels" crates/ bench/`, which returns only their definitions (`regions.rs:268`, `regions.rs:438`) and their production call sites (`lib.rs:517`, `lib.rs:526`). The `thin_features` and `mosaic_grid6` canaries referenced in the doc comments are corpus-level regression fixtures, not unit tests in this crate.
