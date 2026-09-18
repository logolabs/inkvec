# Alpha, and what "first class" would mean

Where the tracer stands with transparency, what it cannot do, and the change that would fix
it. Every number here was measured on 2026-09-07.

## Where it stands

An RGBA input is **matted** before anything looks at it: composited onto one opaque colour,
alpha set to 1, the original alphas kept aside (`alpha_source` in `crates/inkvec-cli`). The
whole pipeline downstream — palette, labels, unmix, fills — is RGB and never sees alpha.

`--cutout` then puts the transparency back at the end:

- the matte is chosen against the artwork rather than fixed at white, so a white mark on a
  transparent ground survives at all (with white it composites to one flat colour and traces
  to nothing);
- a face whose source pixels are transparent becomes a hole punched out of the faces above
  it, rather than a patch of matte colour;
- a face whose *interior* alpha is one value comes back with `fill-opacity` and its colour
  un-matted.

That covers the common cases: logos on transparency, holes, flat translucent panels. It
costs 0.4123 → 0.4218 on the 246-icon screen objective, which is why it is a flag: the
corpus is scored over white, where none of the gain is visible and the seams the cutout
opens along shared edges are.

## What it cannot do

1. **Alpha that varies within a face.** A glow, a soft shadow, a feathered edge: one
   `fill-opacity` cannot describe it, so it is baked against the matte. The guard for this
   is a standard-deviation test on the face's interior alpha, and faces that fail it are
   written opaque.
2. **Choose a matte that suits everything at once.** The matte must differ from the ink it
   meets or the silhouette dissolves; it must not differ from what a glow will be composited
   against or the glow bakes wrong. On a white candle with a soft flame those two demands
   point opposite ways, and the current rule resolves it by keeping white whenever soft
   translucency is more than 5% of the drawn mass — correct for the corpus, arbitrary in
   general.
3. **Translucency over another shape.** A 60% blue over a red disc is one observed colour;
   the tracer emits it as that colour, which renders identically and is not what the artist
   drew. `crates/inkvec-trace/src/alpha.rs` solves exactly this — a layer seen against two
   different grounds is recoverable, and the module is written and tested — but it is not
   wired into the pipeline.

## The change

Make the ink four-dimensional: **an ink is a colour and an opacity**, and the fully
transparent ground is an ink like any other. Then no matte is needed anywhere, because the
thing a boundary pixel is a blend of is two RGBA inks, and that blend is linear in
premultiplied space exactly as the current one is in RGB.

Concretely, in dependency order:

1. **`Palette` carries alpha.** `colors: Vec<Oklab>` and `rgb: Vec<[f32; 3]>` gain an
   `alpha: Vec<f32>`. Extraction clusters colour as it does today on pixels solid enough to
   have one, then splits each cluster by alpha level under the same MDL test that decides
   colour count, and adds one clear ink for the ground. Fully transparent pixels carry no
   colour and must not vote on one — the present code learns this the hard way in
   `choose_matte`.
2. **Labelling and unmixing in premultiplied RGBA.** `label_image` assigns to the nearest
   RGBA ink; `refine_subpixel`'s two-ink unmix runs on premultiplied values, where an
   anti-aliased pixel between an opaque ink and the ground is a straight-line blend and the
   existing coverage arithmetic applies unchanged.
3. **Fills.** `fit_flat` returns colour and opacity; the emitter already writes
   `fill-opacity` and already punches holes, so the last stage needs no new concepts —
   `--cutout`'s two halves become the default behaviour of a pipeline that has alpha in it.
4. **Alpha gradients.** A face whose alpha varies linearly is a `<linearGradient>` with
   `stop-opacity`, fitted by the same machinery that fits colour ramps. This is what closes
   gap 1, and it should be judged by the same ΔJ test as any other fill model.
5. **Layer recovery.** With alpha in the palette, `alpha.rs` has somewhere to put its answer:
   a translucent layer over two grounds becomes one face with an opacity rather than two
   faces with baked colours. This closes gap 3 and is the largest single reduction in face
   count available on emoji art, which is 27% opacity-using by the module's own census.

## How it must be measured

The corpus cannot referee this. It is rendered over white, so every one of these changes is
invisible to dE00 except through the seams and the parameter count — the same blindness that
keeps `--cutout` and `--simplify-faint` off by default. Two things are needed before the work
starts:

- **An alpha-aware score.** Render both the trace and the artist's file over *two* grounds
  (white and a dark one) and score both, or score the RGBA buffers directly. Half a day of
  work on `bench/`, and without it the whole change is unfalsifiable.
- **A transparency corpus.** The existing rasters are transparent-background but their
  artwork is opaque; the cases that matter are glows, soft shadows and stacked translucency.
  Noto and Twemoji contain them, so a subset can be selected by measuring alpha variance
  inside faces of the artist's own file.

## What has landed since

Stages 1 and 4 are in, and stage 5 is connected but not paying yet.

- **Inks carry an opacity** (`Palette::alpha`, `split_alpha_inks`). A region drawn at one
  alpha is its own ink instead of merging with whatever it resembles once matted, which is
  what a white matte did to a 25% white wash.
- **A fade is a gradient.** A face whose source alpha varies linearly is written as a
  linear gradient of `stop-opacity` — the element an editor would use — rather than baked
  against the matte. Deliberately not restricted to flat-filled faces, because over a white
  matte a fade *looks* like a colour ramp towards white and only the alpha channel says
  which it is. On the ramp case the alpha error falls 0.1552 to 0.0008.
- **Layers are connected, and made to pay** (`--layers`). `alpha::decompose` recovers
  `#3082cf at 0.849` against a truth of 0.85 on a stack of translucent discs. The faces the
  layer covers are relabelled onto the ground beneath them, which makes the cuts the layer
  made *interior* — an interior edge belongs to no ring, so it stops being written, and the
  geometry is otherwise untouched because boundary points and their fits are per edge. The
  layer's own pieces are unioned the same way, so its outline is drawn once too.

  Then the rule: **the layer form is kept only when it is smaller** — fewer shapes and no
  more coordinates. The image is identical either way, so the whole decision is a
  description length, exactly as it is for a cubic against a line. On the stack that is
  7 shapes / 1696 bytes against 5 / 1523, and the layer is kept. On forty real icons the
  hypothesis arises three times and the rule refuses all three, which is the answer to
  "does this complicate the common case": it cannot.

What is left: the premultiplied unmix that removes the matte entirely (stage 2), and fills
that carry opacity natively rather than through the emitter (stage 3).

`bench/cases/alpha.py` holds the five properties still — an anti-aliased rim is geometry, a
counter stays transparent, a wash keeps its opacity and colour, a fade is one shape — so
none of the remaining work can regress the common case unnoticed.
