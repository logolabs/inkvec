# Stage 01 — Intake

> Turns an arbitrary source file into the exact pixels the rest of the pipeline was tuned
> against: decoded in the format its own bytes announce, turned upright and converted to
> sRGB the way a viewer shows it, checked for lossy damage, undone if it is a disguised
> upscale, cleaned if it is genuinely degraded, priced in units of content rather than
> pixels, and written down as an opaque image, its alphas kept beside it, wherever it
> carries transparency.

**Source:** `crates/inkvec-trace/src/load.rs` (decode, orientation, the size cap and lossy
detection), `crates/inkvec-trace/src/load/icc.rs` (colour management),
`crates/inkvec-cli/src/lib.rs` (the driver: `run`, `intake`, `price_in_raster_units`,
`trace_prepared_priced`), `crates/inkvec-cli/src/units.rs` (content units),
`crates/inkvec-cli/src/alpha/unblock.rs` (unblock detection),
`crates/inkvec-cli/src/alpha.rs` (matting), `crates/inkvec-cli/src/pipeline.rs`
(`color_options`: the lossy flag and the time budget handed to the trace),
`crates/inkvec-sr` (the super-resolution pre-pass, as a crate of its own)
**Entry points:** `run()` (`crates/inkvec-cli/src/lib.rs:788`), which calls
`load_image_capped()` (`crates/inkvec-trace/src/load.rs:403`) then `resolve_lossy()`
(`crates/inkvec-cli/src/lib.rs:825`), then `trace_image_sized()` (`crates/inkvec-cli/src/lib.rs:210`),
which is `trace_prepared(intake(...))`. `intake()` (`lib.rs:250`) undoes an exact upscale,
applies `--intake-scale` and `--max-dim` and prices the pixel-denominated knobs in the
raster's own units; `trace_prepared()` (`lib.rs:461`) installs the curve cost model and hands
over to `trace_prepared_priced()` (`lib.rs:481`), which runs the restorer and SR pre-passes and
the matte, builds the fit configuration and calls one pipeline. Between them they do
essentially everything this stage covers.
**Pipeline position:** before the trace crate is invoked at all. `trace_prepared_priced` ends
by calling one of `run_strokes` / `run_bilevel` / `run_color`, and it is `run_color`
(`crates/inkvec-cli/src/pipeline.rs:114`, through `run_color_impl`) that calls
`inkvec_trace::trace_color_full_with_alpha` (`pipeline.rs:242`) — stage 03 onward. Intake
carries no stopwatch mark of its own; its cost is folded into whatever the caller measures
around `trace_image`, except for the two pre-passes, which time themselves for their stats
lines (see below).

## What problem this solves

A raster file is not a canonical representation of a drawing — it is one snapshot at one
size, possibly re-encoded, possibly resized by something that was not the artist, possibly
carrying transparency the palette has no dimension for. Every stage downstream of intake
was tuned against a specific kind of input (a native anti-aliased render, roughly 128 px on
its long side, opaque), and every one of those assumptions can be wrong on a file a user
actually hands the tool. Intake is not one problem; it is five, bundled because they all
have to be resolved before a single pixel reaches the palette:

1. **Decode.** Turn bytes of an unknown container format into straight RGBA floats — in
   the format the bytes say they are, whatever the file is named, the right way up, and in
   the colours a viewer would show.
2. **Know whether the file lies about its own cleanliness.** A JPEG or lossy WebP looks,
   pixel by pixel, like a slightly noisy PNG — except the noise clusters at edges in a way
   that fooled the first pixel-level detectors tried (see below), so the container format
   itself has to be asked.
3. **Undo what a viewer already did to the file.** Someone with a 96-px logo who wants a
   large SVG frequently resizes the PNG first, with nearest-neighbour resampling — many
   viewers do this without being asked. What arrives is not more detail, it is the same
   detail redrawn as a staircase of large flat blocks, and the tracer would faithfully
   vectorise the staircase.
4. **Price every pixel-denominated constant in the tracer per unit of content, not per
   pixel**, because the same logo exported at 128 px and at 512 px is the same drawing and
   should cost the same number of SVG parameters — see Resolution invariance, below.
5. **Reduce transparency to an opaque image**, because the projection at the heart of
   stage 02 (`coverage::bilevel_coverage`) and the palette's colour-mixture algebra both
   need opaque colours to unmix a boundary against. Under `--no-native-alpha` a fourth,
   alpha, dimension has nowhere to go in either, so the image is matted onto one chosen
   colour; under native alpha, the default, it is written down over white and its alphas
   travel beside it into every stage that unmixes.

## Inputs and outputs

**Input:** a file path (`load_image_capped`, `crates/inkvec-trace/src/load.rs:392-406`, which
the command line uses, or the uncapped `load_image`, `load.rs:98-102`), an in-memory byte
slice (`decode_image` / `decode_image_capped`, `load.rs:104-108`, `:475-488`), or raw RGBA8
pixels (`rgba8_capped`, `load.rs:490-519`, which has no format, profile or orientation to
read and only applies the cap). The decoders compiled in are PNG, JPEG, WebP, GIF, BMP and
TIFF (`crates/inkvec-trace/Cargo.toml:18`); the command line's usage line names the same six
(`crates/inkvec-cli/src/args.rs:310`). Every decoded image funnels through `from_dynamic` (`load.rs:298-342`), which produces straight
(unpremultiplied) floats in `[0, 1]`, alpha 1 where the image has none — the same `Rgba`
type stage 02 consumes. How it gets there depends on the layout the decoder handed over:

* **8-bit RGBA** is read straight from the decoder's own buffer, with no copy;
* **8-bit RGB**, the common opaque layout (every JPEG, most opaque PNGs), is widened straight
  to four floats with alpha 255, skipping the intermediate RGBA8 buffer;
* **every other layout** (grey, grey plus alpha, 16-bit, float) still goes through the
  library's `into_rgba8()`, whose colour conversion the loader does not reimplement.

Each byte then becomes a float by a table lookup, `UNIT[k] = k as f32 / 255.0`
(`load.rs:282-296`, built at compile time), in parallel chunks of 65,536 pixels
(`CONVERT_CHUNK_PIXELS`, `load.rs:278-280`) from 256 × 256 pixels on (`PARALLEL_MIN_PIXELS`,
`load.rs:274-276`); below that it runs on the calling thread (`widen`, `load.rs:344-373`). The
table holds exactly the quotients the old per-byte division computed, and each output float
depends on one input byte, so neither the table nor the split can change a value; the tests
`the_table_holds_the_quotients` and `from_dynamic_is_the_old_conversion` (`load.rs:623-643`)
check every layout bit for bit against the old `to_rgba8()`-then-divide path, which is kept as
a test oracle (`load.rs:521-572`). At 2048 px the intake had cost 20 ms, all serial: decode
4.9, the RGBA copy 4.0 and the float conversion 10.1 (`load.rs:28-30`). The copy is gone for
8-bit RGB and RGBA files, and the conversion is split over the cores. The method is an
engineering change, not a published one (the doc comment says "Not from the literature";
"See also" Ragan-Kelley et al., Halide, PLDI 2013, on fusing an image pipeline's stages,
`load.rs:319-325`).

**Output:** an `Intake` (`crates/inkvec-cli/src/lib.rs:218-236`): the `Rgba` the tracer will
actually trace at (which may differ from the file's own dimensions — see unblock,
`--intake-scale` and `--max-dim` below), an `Args` whose `lossy` field the command line has
already resolved from `auto` to `on` or `off` (a library caller that skips `resolve_lossy`
has `auto` read as off, `crates/inkvec-cli/src/pipeline.rs:154-156`) and whose `precision`,
`min_area` and `lambda_scale` are priced in this raster's own units, whether an upscale was
undone (`replicated`) or anything resampled the raster (`normalised`), and the
`(display_w, display_h)` the emitted SVG is retargeted to at the end whenever anything
resampled — so the file the user gets back claims the size the input file claimed
(upright), whatever size it was actually traced at; when the SR pre-pass ran on an image
that was not unblocked, it claims the size SR produced (`crates/inkvec-cli/src/lib.rs:630-638`). `intake` is public, and separate from `trace_prepared`, for one
caller: a browser that runs the restorer network itself between the two (`lib.rs:238-249`).

## How it works

### Decode and the lossy-container check

The loaders read the file once, decode it in the format its bytes announce, and apply what
a viewer applies before showing it. The steps below follow the passes the module
documentation lists, in its order (`load.rs:8-30`):

1. **Read once; the bytes decide the format.** `load_image_capped` reads the whole file
   into memory (`std::fs::read`, `load.rs:404`) and decodes both the header (the dimensions
   the `--max-dim` cap is decided from) and the pixels from those bytes
   (`load_file_bytes_capped`, `load.rs:408-433`); it used to open the file twice, once for the
   header and once for `image::open`, and the one-read change is filed as "Not from the
   literature: plumbing." The format is chosen by `sniff` (`load.rs:110-136`): the one the
   bytes' signature announces (`\x89PNG`, `\xFF\xD8\xFF`, `GIF8`, `RIFF....WEBP`, `BM`,
   `II*\0` / `MM\0*`, as the `image` crate's `guess_format` reads them), when it is one of the
   six formats the tracer reads; otherwise the one the path's extension names; and a file
   that names neither is handed to `image::open` itself, for its own error message. The
   extension used to decide alone, as `image::open` decides, so a JPEG saved as `.png`
   failed with "Invalid PNG signature" and a PNG saved as `.jpg` with "Illegal start bytes"
   (intake fuzz, 2026-10-02); a file whose extension is right decodes with the same decoder
   as before, so its pixels are unchanged. Citation, as the doc comment gives it: "Method
   from: magic-number content sniffing, the rule of the WHATWG MIME Sniffing Standard (§ 6.1,
   "Matching an image type pattern")", the patterns being "the `image` crate's
   `guess_format`". `decode_image_capped` (`load.rs:475-488`) sniffs the same way with no
   path; bytes with no signature the tracer reads get the library's own error
   (`image::load_from_memory`). `one_read_equals_image_open` (`load.rs:663-725`) checks PNG
   (RGBA and RGB), BMP, JPEG and TIFF files, a text file named `.png` and a missing file, at
   caps of 0, 64 and 2048, against the old two-open loader (the same pixels, or the same
   error text), and checks that a PNG named `.jpg`, a PNG with no extension and a JPEG named
   `.png` each decode pixel for pixel as the correctly named file. On the robustness
   branch's edge-case run the two mislabelled files that v0.2.4 refused traced
   byte-identical to the correctly named ones (impl2/robust report, 2026-10-03).
2. **Refuse an image with no pixels.** `decode_as` (`load.rs:138-162`) reads the header's
   dimensions and calls `has_pixels` (`load.rs:255-272`) before anything is decoded: a side
   of zero is a decode error, "the image has no pixels (W x H)". A decoder can hand back such
   an image without an error — a GIF whose logical-screen width is 0 decodes to 0 × 30000,
   while PNG refuses the same header itself — and every stage after the intake divides by a
   side or indexes row 0; the first to do so was the unblock test (`w / min(64, w)`, a
   division by zero, in Quality and Fast alike). The intake fuzz of 2026-10-02 found it in 3
   of 1,200 mutated files, all GIFs, which panicked on v0.2.4 and are now refused
   (`a_gif_with_a_zero_wide_screen_is_refused`, `load.rs:971-1005`). "Not from the
   literature: an input check." `pixel_grid` keeps a guard of its own for a caller that
   builds an `Rgba` itself (see the unblock pre-pass).
3. **A larger allocation allowance when the cap will apply.** The `image` crate refuses any
   decode whose output buffer would pass 512 MiB (`DEFAULT_MAX_ALLOC`, `load.rs:211-212`),
   its guard against decompression bombs, and that refused legitimate files too: a
   12000 × 12000 RGBA PNG is 576 MB decoded and failed with "Memory limit exceeded",
   although the trace would be capped at 2048 px and the full-size buffer lives only until
   the box filter has read it. When the header says the cap will apply (`target_dims`,
   `load.rs:375-390`), `capped_decode_limits` (`load.rs:225-253`) allows
   `CAPPED_EXTRA_ALLOC` = 768 MiB more on a 64-bit target, 1.25 GiB in all: room for
   16384 × 16384 RGBA at 8 bits (1 GiB) or 12000 × 12000 at 16 (1.15 GB), while the
   20000 × 20000 bomb of the intake fuzz (1.6 GB) and a 100000 × 100000 header (40 GB) are
   still refused before anything is allocated; on a 32-bit target (WebAssembly), whose whole
   address space is 4 GiB, the extra is 0 (`load.rs:214-223`). An image within the cap, or
   any decode with `max_dim = 0`, keeps the default limits exactly, and raising a limit
   changes no pixel of an image that decoded before. Citation: "Not from the literature: a
   resource limit. See also" the `image` crate's `Limits` documentation, and J. Cupitt,
   K. Martinez (1996), *VIPS: an image processing system for large images*, Proc. SPIE 2663,
   doi:10.1117/12.233043, for the strip-at-a-time alternative — "not taken, because the
   `image` crate decodes whole frames and a streaming PNG path would duplicate its colour
   conversions." On the edge-case run the 12000 × 12000 RGBA file, refused on v0.2.4, traced
   in both modes (620 MB peak), and the bombs were still refused at 5-8 MB (impl2/robust
   report, 2026-10-03).
4. **Colour-manage, then turn upright.** `decode_upright` (`load.rs:164-209`) is
   `ImageReader::decode` taken apart only to read, between building the decoder and reading
   its pixels, the EXIF orientation (`ImageDecoder::orientation`) and the embedded ICC
   profile (`ImageDecoder::icc_profile`): the same decoder, built with the same limits, whose
   output buffer is reserved from them first, exactly as `decode` does. An unreadable
   orientation counts as none and an unreadable profile as no profile, so neither fails a
   decode whose pixels are fine. Then, on the decoded 8-bit image:
   * **The ICC profile is converted to sRGB** (`icc::to_srgb`,
     `crates/inkvec-trace/src/load/icc.rs:72-149`). The tracer reads every pixel as sRGB,
     which a Display P3 screenshot, an Adobe RGB photo or a gamma-1.8 prepress file is not;
     read as sRGB, six gamma-1.8 images with the profile embedded traced at dE00 0.84 on
     average (worst 3.03) against 0.077 for the same art saved as sRGB (r2-inputs
     `formats_test.py`, 2026-10-02; `icc.rs:4-14`). The profile is parsed by `moxcms` (already
     the `image` crate's colour engine); an unreadable profile, or one whose colour space does
     not match the decoded pixels (a CMYK profile on a JPEG the decoder has already turned
     into RGB, a Lab profile), leaves the image as it was. Otherwise an 8-bit transform to sRGB
     is built with the relative colorimetric intent — an RGB profile converts RGBA to RGBA,
     alpha passed through; a grey profile on a grey image converts grey + alpha to RGBA.
     Before any pixel is converted, `moves_colours` (`icc.rs:151-197`) probes the transform on
     every level of the red, green, blue and grey ramps and a 9 × 9 × 9 lattice; when no probe
     moves by more than `SRGB_TOLERANCE` = 1 level in any channel (`icc.rs:66-67`) and no
     alpha changes, the profile is sRGB in all but name and the image is returned untouched,
     because "converting it through lookup tables would only add a level of rounding noise to
     pixels that were already right" (`icc.rs:30-34`). Otherwise the whole image is converted
     in parallel bands of `ROWS_PER_JOB` = 64 rows (`icc.rs:69-70`); each pixel is converted on
     its own, so the split cannot change a value (`the_row_split_does_not_change_a_value`).
     The conversion runs before the cap and before the floats exist, so the box filter
     averages sRGB values exactly as it would for a file saved in sRGB. Citation, as the
     module doc gives it (`icc.rs:48-56`): "Method from: the ICC colour-management
     architecture, Specification ICC.1:2010 (profile version 4.3.0.0)", International Color
     Consortium, "as implemented by the `moxcms` crate"; "Not from the literature: the
     identity probe of step 3". Measured before and after the change on one run: the six
     gamma-1.8 PNGs went from a mean dE00 of 0.85 to 0.12, and PNGs carrying three real embedded sRGB profiles (HP,
     Microsoft, PIL) traced byte-identical to the untagged PNG, 18 of 18 (impl2/robust
     report, 2026-10-03). No image in the benchmark sets carries a profile, so the traces
     judged there are unchanged (`icc.rs:44-46`). `moxcms` picks SIMD paths at run time, but
     its 8-bit transforms are fixed-point, so a given machine always converts the same way
     (`icc.rs:58-60`).
   * **The EXIF orientation is applied.** A camera stores the sensor's rows as they came and
     records how the picture is to be turned in the EXIF Orientation tag (274, values 1-8:
     identity, the two mirrors, the three rotations and the two transposes); every viewer
     applies it, and `decode` does not. Six JPEGs stored with Orientation = 6 traced
     sideways, dE00 2.9 to 17.6 against 0.03 to 0.42 for the same images without the tag
     (r2-inputs `formats_test.py`, 2026-10-02; `load.rs:168-173`); after the change they
     trace at 0.027-0.439, the untagged JPEGs' own figures (impl2/robust report,
     2026-10-03). The orientation comes from the decoder (the EXIF of a JPEG, WebP or PNG
     `eXIf` chunk, the TIFF tag) and is applied by `DynamicImage::apply_orientation` after
     the colour conversion and before the cap, so the cap and everything after it see the
     image as it is shown, and the arrival dimensions handed back are upright: a quarter
     turn swaps the sides (`load.rs:159-161`). Citation: "Method from: the Orientation tag
     (274) of the Exif standard (CIPA DC-008), as the `image` crate implements it:
     `image::metadata::Orientation` ... and `DynamicImage::apply_orientation`"
     (`load.rs:182-186`). `the_exif_orientation_is_applied` (`load.rs:884-947`) checks all
     eight values pixel by pixel on a 3 × 2 image, value 1 bit-identical to the untagged
     decode. `upright_dimensions` (`load.rs:449-473`) reads the same upright size from the
     header alone, for a caller that reports a file's size before tracing it; Inkvec Studio
     does (`studio/core/src/trace.rs:413`).
5. **Cap**, then **widen**: above `max_dim` on the longer side the 8-bit buffer is
   box-averaged down (`cap_decoded`, `load.rs:435-447`, through
   `coverage::box_downsample_rgba8`) before any float exists; otherwise `from_dynamic`
   widens it as described above.

The `--lossy auto` check below still opens the file separately for its first 32 bytes, and
only when `--lossy` is `auto` (`crates/inkvec-cli/src/lib.rs:805-814`).

The interesting function next to the loaders is `lossy_container` (`load.rs:58-96`):

```rust
pub fn lossy_container(bytes: &[u8]) -> Option<bool> {
    match image::guess_format(bytes).ok()? {
        image::ImageFormat::Jpeg => Some(true),
        image::ImageFormat::WebP => match bytes.get(12..16)? {
            b"VP8 " => Some(true),
            b"VP8L" => Some(false),
            _ => None,
        },
        image::ImageFormat::Png
        | image::ImageFormat::Gif
        | image::ImageFormat::Bmp
        | image::ImageFormat::Tiff => Some(false),
        _ => None,
    }
}
```

This is a fact about the container, not a statistic about the pixels, and the doc comment
(`load.rs:58-76`) is explicit about why that distinction matters: the palette has a guard for
exactly this kind of damage (`color::SOFT_NOISE_SIGMAS`, documented in `03-palette.md`), but
until 2026-09-08 it was switched only by `coverage::intake_scale` — the measured edge
*width* — which is blind to compression, because JPEG rings flat regions without widening
an edge. WebP is one container holding two different codecs, so the answer is read from the
RIFF sub-chunk tag at byte 12: `VP8 ` (with the trailing space) is the lossy codec, `VP8L`
is lossless, and `VP8X`, the extended container, is left `None` rather than guessed — its
sub-chunks would need walking to know for certain. Every other accepted format stores exact
samples and reads `Some(false)`. An unrecognised format reads `None`.

Four tests pin this down (`lossy_container_tests`, `load.rs:728-778`): a real JPEG and PNG
encode read `Some(true)` / `Some(false)` (`jpeg_is_lossy_and_png_is_not`); only the first 32
bytes of the JPEG are needed (`a_header_is_enough`) — which matters, because the caller only
reads a small head of the file, not the whole thing; the three WebP RIFF tags resolve as
described (`webp_is_read_from_the_riff_chunk`); and nonsense bytes resolve to `None`, not to
a guess (`nonsense_is_unknown_not_clean`).

### `resolve_lossy` and `--lossy auto|on|off`

`resolve_lossy` (`crates/inkvec-cli/src/lib.rs:819-838`) turns the CLI's three-way `--lossy`
flag (itself `inkvec_sr::Mode`, reused rather than a separate enum) into a definite yes or
no, once, before the trace crate ever sees a pixel:

```rust
pub fn resolve_lossy(args: &Args, head: impl FnOnce() -> Option<Vec<u8>>) -> Args {
    let mut out = args.clone();
    if out.lossy == inkvec_sr::Mode::Auto {
        let lossy = head()
            .and_then(|b| inkvec_trace::lossy_container(&b))
            .unwrap_or(false);
        out.lossy = if lossy {
            inkvec_sr::Mode::On
        } else {
            inkvec_sr::Mode::Off
        };
    }
    out
}
```

`head` is injected rather than read directly so the function is testable without touching
disk; `run()` (`lib.rs:805-814`) supplies it by reading the first 32 bytes of the input
file. An unreadable or unrecognised container resolves to `Off` — "not known to be lossy" —
because, as the doc comment states, the guard this feeds costs **10.9%** on the 246-icon
screen set when it runs on a clean intake (0.4005 → 0.4442, measured 2026-09-08) and must
not fire on a guess (`lib.rs:819-824`). `--lossy` defaults to `Auto`
(`crates/inkvec-cli/src/args.rs:252`), so container-format detection runs on every trace
unless a user overrides it; `On` is there for a file that no longer admits what was done to
it — "a PNG that was once a JPEG -- a re-saved screenshot, an export from a chat app"
(`args.rs:133-138`).

The resolved `args.lossy == Mode::On` becomes `ColorOptions::lossy_intake`
(`color_options`, `crates/inkvec-cli/src/pipeline.rs:154-156`), which the colour front end
reads alongside `coverage::intake_scale` and a third, pixel-level signal,
`coverage::ringing_score`, to decide whether the palette's soft-intake constants
(`SOFT_NOISE_SIGMAS`, `SOFT_SAME_INK_DE00`) switch on (`crates/inkvec-trace/src/lib.rs:342-362`).
`ringing_score` (`crates/inkvec-trace/src/coverage.rs:579-627`) reads *where* the Laplacian's
energy sits — a band 3 to 7 px out from strong edges, where clean vector art is flat, times
the sign-alternation rate there — and is "the only one of the three a re-encode cannot
launder": a JPEG re-saved as PNG defeats `lossy_container`. Its doc comment gives clean brand
logos firing at 3.8% and 82 to 91% of JPEG caught at the shipped threshold. All three are
documented fully in `03-palette.md`.

### The unblock pre-pass — undoing an exact upscale

`pixel_grid` (`crates/inkvec-cli/src/alpha/unblock.rs:8-89`) answers a narrow, exact
question: is this raster a nearest-neighbour replication of a smaller one? A `k`×
replication is exactly invertible — average each `k`×`k` block and the original pixels
return bit for bit — so the test is deliberately strict, not a tolerance-based heuristic.
The block test itself is `blocks_constant` (`unblock.rs:176-193`):

```rust
(0..h / k).all(|by| {
    (0..w / k).all(|bx| {
        let first = px(bx * k, by * k);
        (0..k).all(|dy| {
            (0..k).all(|dx| {
                let p = px(bx * k + dx, by * k + dy);
                (0..4).all(|c| (p[c] - first[c]).abs() < 1.0 / 512.0)
            })
        })
    })
})
```

Every pixel in every block must match its block's first pixel to within `1.0/512.0`
per channel, alpha included — no tolerance for "nearly", because a looser test would also
catch a genuine drawing of large flat squares and averaging that away would be a real loss
(`unblock.rs:17-25`). `k` is tried downwards from `MAX_FACTOR = 32` to `2` so an 8× upscale
is reported as 8×, not as 2×; the factor must divide both sides and leave at least
`smallest = 64` pixels on each, so a raster under 128 px on either side is never unblocked,
because "a 2x undo of a small icon leaves too few pixels for the boundary solve to work
with" (`unblock.rs:20-27, 69, 78-84`). An image with a zero side returns `None` before any
of this: a GIF with a zero-wide logical screen reached here as 0 × 30000 in the intake fuzz
of 2026-10-02 and divided by zero; the decoder now refuses such files, and the guard keeps
the function total for any caller that builds an `Rgba` itself (`unblock.rs:71-77`, test
`a_zero_side_is_not_an_upscale` in `alpha/intake_tests.rs`).

**Which factors get the block test: the gcd of the change positions.** The block test does
not run for every divisor of `w` and `h`. One early-exiting scan first computes

```text
g = gcd(w, h, every column x >= 1 where some pixel differs from its left neighbour by
               more than 1/256 in some channel,
               every row y >= 1 where some pixel differs that much from the one above)
```

(`change_gcd`, `unblock.rs:118-161`, with `SHARP = 1/256` at `:131-132`), stopping as soon as
`g` reaches 1. Then only the `k` that divide `g` get the block test, from `k_max` down
(`unblock.rs:85-88`). The answer is the old answer on any input, by the argument the doc
comment gives (`unblock.rs:43-51`): if a factor `k` passes the block test, two horizontally
adjacent pixels at `x − 1` and `x` with `k ∤ x` lie in one block, so each is within `1/512`
of the block's first pixel and they differ by less than `2 · 2⁻⁹ = 1/256`. Every position
where neighbours differ by more than `1/256` is therefore a multiple of `k`, and so are `w`
and `h`: `k` divides `g`. Skipping the `k ∤ g` never skips a passing factor, and the others
get the old test itself. For 8-bit input the two views coincide: distinct levels are at
least `1/255 > 1/256` apart, so a "sharp change" is any change, and the block test only
confirms.

It is fast because on anything that is not an upscale two edges at coprime positions appear
within the first rows of content, `g` falls to 1 and no block test runs. The old loop ran
the block test once per divisor from 32 down, each scanning until its first non-constant
block, most of it on the blank rows above the artwork: 7.1 ms at 2048 px
(`unblock.rs:53-60`). Blank rows are skipped cheaply: a row is first compared with the row
above, and with itself shifted by one pixel, bit for bit (`same_bits`, `unblock.rs:163-174`,
64 floats at a time), and only a row that differs is examined pixel by pixel. The gcd is
Stein's binary algorithm and divisibility is tested by multiplying back (`divides`, `gcd`,
`unblock.rs:91-116`), because wazero's arm64 compiler once miscompiled `i32.rem_u` in a hot
loop and the Go binding runs this crate as WebAssembly. Citation, as the doc comment gives
it (`unblock.rs:62-67`): "Not from the literature: the gcd of change positions as a
candidate filter for exact block replication, because the published resampling detectors
are statistical"; "See also" A. C. Popescu, H. Farid, "Exposing Digital Forgeries by
Detecting Traces of Resampling", IEEE Trans. Signal Processing 53(2):758–767, 2005, DOI
10.1109/TSP.2004.839932. The old serial loop is kept as an oracle in
`crates/inkvec-cli/src/alpha/intake_tests.rs`.

`intake` runs this first, before anything else looks at the image
(`crates/inkvec-cli/src/lib.rs:258-283`), and the doc comment on `pixel_grid` states the
measured cost of a naive tracer that skips it: a 96-px logo blown up to 768 traces its
pixel boundaries directly, the palette shattering from 3 inks to 13 and the boundary coming
back as 1568 straight lines walking round pixel corners (`unblock.rs:10-15`).
`--no-unblock` disables it (`crates/inkvec-cli/src/args.rs:366-371`). Detection is
exact-match only; a resampled or anti-aliased upscale — where block boundaries are not
perfectly constant — fails this test by design and is `--sr`'s problem instead
(`unblock.rs:29-30`).

### Why the SR pre-pass runs after unblock, not before

`inkvec-sr` (its own crate) exists for damage `pixel_grid` cannot undo exactly: JPEG
ringing, blur, a genuine photograph of a logo. It is documented in full below; the ordering
question is intake-specific and the comment at the unblock call
(`crates/inkvec-cli/src/lib.rs:258-266`) states the reason plainly:

> Before the pre-pass, not after, and that is the whole point of the order: run the
> upscaler on the blocky version and it treats the block edges as the artwork — measured on
> a 96-px logo blown up to 768, `--sr on` came back with 14 inks and 1846 segments, worse
> than doing nothing. On the recovered original it has something real to put detail back
> into.

In other words: unblock is exact and free of side effects when it applies, so it always
runs first; whatever the SR model sees next is the smallest raster that is honestly
representative of the drawing, and it has real edges to sharpen rather than staircase
artefacts to hallucinate detail onto. The rest of `intake` — `--intake-scale`, `--max-dim`
and the pricing of the knobs — also runs before the pre-passes, "so that a probe trace
either `auto` mode makes of an input it keeps is bounded exactly like the real trace"; they
used to run after the pre-passes' early returns, and `--restore auto` / `--sr auto` kept a
probe traced at full resolution (`lib.rs:285-289`).

### The SR pre-pass itself: `Mode::Auto | On | Off`

`--sr` (default `Off`, `crates/inkvec-cli/src/args.rs:253`) controls a network-based
upscale-then-halve cleanup, implemented in the `inkvec-sr` crate.
`inkvec_sr::Mode` (`crates/inkvec-sr/src/lib.rs:35-45`):

```rust
pub enum Mode {
    /// Trace, measure the fit, clean and retrace only if the fit is bad.
    #[default]
    Auto,
    /// Always clean first.
    On,
    /// Never clean. Identical to not having the mode.
    Off,
}
```

`Mode::On` calls `inkvec_sr::prepass` (`inkvec-sr/src/lib.rs:92-143`) unconditionally: it
zeroes the colour channels of every fully transparent pixel (alpha `<= 1e-4`, so they cannot
drag their upscaled neighbours toward an arbitrary stored colour), upscales by the
upscaler's factor, box-averages back down (`clean::box_downsample`) to the requested
`--sr-scale` (default 2, `args.rs:255`), and — unless `--sr-no-recolour` — refits each colour
channel of the result onto a bicubic upsample of the zeroed source over its flat pixels
(`clean::match_flats`) rather than trusting the network's colour reconstruction. The
upscaler always runs out of process: the network is not compiled in
(`crates/inkvec-sr/Cargo.toml:19-21`). `build_upscaler` (`crates/inkvec-cli/src/lib.rs:840-858`)
runs `--sr-command` when one is given, at a fixed scale of 4, and otherwise the packaged
Python pre-pass `tools/inkvec_sr` through `external::External::python`, found by
`sr_tools_dir` (`lib.rs:860-881`): `INKVEC_TOOLS_DIR` when it is set (and then only that
folder), otherwise a `tools/` folder beside the executable or in one of the three folders
above it, then the source checkout the binary was built from. The pass prints one stats line, "sr  cleaned to WxH in Ns, <upscaler>"
(`lib.rs:603-628`).

**The restorer comes first.** `trace_prepared_priced` runs the restorer pre-pass
(`--restore`, off by default, `args.rs:258`; `restore_prepass`, `lib.rs:932-1017`) before SR.
Its `auto` mode works exactly like SR's — one probe trace, kept when the input measures
clean, and "handed on so the two pre-passes never trace the same image twice"
(`lib.rs:932-933`); with SR off, a kept probe is returned as the trace (`lib.rs:504-520`), and
a restored image is traced with lossy intake forced on (`lib.rs:522-534`). The restorer
itself is outside this page.

`Mode::Auto` (`crates/inkvec-cli/src/lib.rs:536-601`) is the more interesting path: it takes
the restorer's probe when there is one, and otherwise traces the image once as it arrived
(`trace_once`, `lib.rs:1055-1084`), a stripped version of the ordinary pipeline — the same
`fit_config`, bilevel or colour, matte included (through the copying `alpha_source`), and
always in colour, because "a monochrome drawing disagrees with a colour input everywhere it
is not black or white, which would read as damage on every image"; under `--monochrome` the
probe decides and the trace is then made again as asked (`lib.rs:499-503`). It renders that
trace back to a raster with `resvg` (`inkvec_sr::detect::render_svg`,
`inkvec-sr/src/detect.rs:58-95`, the same renderer the benchmark harness scores with — so a
residual measured here cannot disagree with a score measured there), and asks
`inkvec_sr::decide` (`inkvec-sr/src/lib.rs:161-179`) whether the trace explains the input
where the trace itself claims to be flat:

```rust
pub fn decide(img: &Rgba, svg: &str, threshold: f64) -> Decision {
    let Ok(model) = detect::render_svg(svg, img.width, img.height) else {
        return Decision::Keep { residual: None };
    };
    match detect::interior_residual(img, &model) {
        Some(r) if r > threshold => Decision::Clean { residual: Some(r) },
        r => Decision::Keep { residual: r },
    }
}
```

`Keep` returns the probe as the trace, with the stats line "sr  residual R <= T, traced
directly" (or "could not measure the fit; traced directly"). `Clean` builds the upscaler,
once, and keeps it for the clean-up and the retrace (`crates/inkvec-cli/src/lib.rs:539-564`). **Without an
upscaler, `auto` traces directly** (impl2/bugs, merged 2026-10-03): "`auto` is a request to
clean *if it helps*, so no upscaler to be had -- no `tools/inkvec_sr` beside the binary, an
unusable `--sr-command` -- is not a reason to fail the trace: keep the probe, as a clean
input would, and say why in the stats. `on` asked for the clean-up outright and still fails
without one. This is `--restore auto`'s rule" (`lib.rs:565-574`). The stats line reads "sr
residual R > T, but no upscaler is available (<the error>); traced directly", and under
`--monochrome` the image is traced again uncleaned, as asked (`lib.rs:578-582`). The tests
`sr_auto_without_an_upscaler_traces_directly` and `sr_on_without_an_upscaler_is_an_error`
(`crates/inkvec-cli/tests/pipeline.rs:407-447`) check that the fallback SVG equals the plain
trace, in colour and in monochrome, and that `on` still errors. The fallback covers an
upscaler that cannot be built; a packaged tool that is found but fails while running still
fails the trace, under `auto` as under `on` (`prepass(...)?`, `crates/inkvec-cli/src/lib.rs:613`).

`interior_residual` (`inkvec-sr/src/detect.rs:97-155`) is the signal: both images are
composited onto white, a mask marks pixels where the *traced model* is flat (using the
composited colour, not the straight-alpha one, because straight-alpha channels are constant
across an anti-aliased edge while only alpha moves — a mask built on them would call every
boundary pixel flat, exactly where a trace and its input are expected to disagree), and the
RMS colour difference over that masked, genuinely-flat area is the residual. A face-model
disagreement in flat territory is not modelling error — a piecewise-flat model is flat by
construction — so it is degradation.

**The normalisation is deliberately `sum / 9n`, not the `sum / 3n` an RMS over three colour
channels would give** (`detect.rs:103-110`), so this residual reads `1/sqrt(3)` of the true
RMS. The doc comment calls this out explicitly as *not* a bug to fix: it is the formula
every threshold in the module was calibrated against, mirrored deliberately in the reference
Python implementation (`tools/inkvec_sr/clean.py`, which "divides by three twice for the same
reason"), and "correcting it without recalibrating [`DEGRADED_RESIDUAL`] would move every
clean icon across the threshold -- which is exactly what it did when this was first written
the "right" way." `interior_residual` returns `None` when fewer than 100 masked pixels
exist to judge on, or when the two images differ in size, and the caller treats "cannot
tell" as "keep" rather than "clean it", because cleaning a clean image is the expensive
mistake — three times worse than doing nothing, dE00 0.605 against 0.195, per the module
doc (`detect.rs:3-7`).

`DEGRADED_RESIDUAL = 0.5` (`detect.rs:35-36`, also the default for `--sr-threshold`,
`crates/inkvec-cli/src/args.rs:254`) is calibrated over 30 icons in five conditions
(`detect.rs:13-25`):

| condition | min | median | p95 | max |
|---|---|---|---|---|
| clean | 0.000 | 0.000 | 0.275 | 0.376 |
| jpeg-q80 | 0.454 | 0.907 | 1.441 | 2.708 |
| jpeg-q50 | 0.650 | 1.241 | 1.942 | 2.669 |
| blur-1.0 | 0.274 | 0.914 | 2.332 | 5.524 |
| noise-2 | 0.812 | 0.837 | 1.156 | 1.426 |

At `0.5`, no clean icon in this set is cleaned and 4.2% of damaged ones are missed; clean
and `blur-1.0` overlap, so no threshold cleanly separates every condition, but the case the
detector was built for — JPEG — is cleanly separated. A cheaper, pixel-only detector (an 8×8
block signature, needing no trace at all) was tried and refuted: it reads 2.175 on clean
against 2.133 on JPEG q50, no better than noise. `Auto` therefore pays for one full extra
trace, because nothing cheaper discriminates (`detect.rs:27-29`).

### `--intake-scale` (opt-in): resampling an oversampled input

`normalise_intake` (`crates/inkvec-cli/src/lib.rs:126-175`) is a separate, opt-in mechanism
from the unblock and SR pre-passes above: rather than undoing an exact replication or
cleaning genuine damage, it resamples an input that is native but carries more pixels per
unit of edge detail than the tracer needs — a photograph or a smoothly resized image, where
`coverage::intake_scale` (stage 02) reads a wide edge. It only runs when `--intake-scale` is
set, `replicated` is false (unblock did not already fire) and `args.sr == Mode::Off` (the SR
pre-pass, when it runs, is expected to have already normalised scale) (`lib.rs:290-298`):

```rust
fn normalise_intake(img: inkvec_trace::Rgba, quiet: bool) -> (inkvec_trace::Rgba, bool) {
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let scale = inkvec_trace::coverage::intake_scale(&rgb, img.width, img.height);
    if scale < INTAKE_SCALE_FLOOR {
        return (img, false);
    }
    let s = scale.min(INTAKE_SCALE_CAP);
    ...
    let out = inkvec_trace::coverage::downsample_to(&img, nw, nh);
    (out, true)
}
```

Each side is divided by `s` and rounded, at least 8 px, and an image that would not shrink
on both sides is returned untouched (`lib.rs:146-150`). `INTAKE_SCALE_FLOOR = 1.5`
(`lib.rs:119-121`) — below this the intake is left alone; "Every image in the corpus reads
exactly 1.00" per the constant's own comment. `INTAKE_SCALE_CAP = 8.0` (`lib.rs:122-124`)
bounds how much a single, possibly wrong, estimate is allowed to throw away at once.
`--intake-scale` defaults off (`args.rs:264`); the usage text states the trade explicitly
(`args.rs:508-521`): on 8×-upsampled input at 1024 px it is 24× faster with a twelfth of the
parameters and half the colour error, but **DINO** — the corpus's structural-similarity
measure — falls from 0.954 to 0.916, so it is offered for input too slow to trace at all,
not to make an already-tractable trace faster.

### `--max-dim` and `--time-budget`

`--max-dim` (default `2048`, `args.rs:232`) is a hard ceiling on the size that is traced,
applied in two places. The command line hands it to the decoder (`run`,
`crates/inkvec-cli/src/lib.rs:804`), which box-averages the 8-bit buffer down before the
floats exist (pass 5 above, with the larger allocation allowance of pass 3); and `intake`
applies the same ceiling again after unblock and `--intake-scale` (`lib.rs:300-318`), which is
where it acts for a caller that hands over a raster rather than a file (`trace_image`, the
bindings): each side divided and rounded, at least 8 px, by "the same exact area average the
intake normaliser uses, so edges stay edges". Either way the SVG is written at the original
size (`trace_image_sized`'s `display_size`, `lib.rs:203-209`). The stated reason is trace
time, which grows with pixel count; 2048 is chosen because it "keeps a typical logo under a
few seconds" (`args.rs:327-331`), a qualitative target rather than a swept figure.

`--time-budget` (default `0.0`, meaning no budget, `args.rs:233`) is advisory rather than a
hard deadline. It is spent in `color_options` (`crates/inkvec-cli/src/pipeline.rs:140-150`),
which the colour pipeline calls as it starts (`pipeline.rs:222`), so the clock runs from
there and each probe trace gets a clock of its own: 60% of the budget becomes
`ColorOptions::deadline`, after which gradient-band merging stops (the one stage whose cost
grows with the square of the region count — "an unmerged band is a correct fill, just a
separate one", `crates/inkvec-trace/src/lib.rs:162-165`); 25%, and at least 50 ms, becomes
`boundary_ms`, the boundary solve's wall-clock budget (`crates/inkvec-trace/src/lib.rs:174-177`).
The help text states the contract (`crates/inkvec-cli/src/args.rs:332-339`): "Gradient-band
merging stops at 60% of it, checked between fits, and the boundary solve gets 25%; the
output is still a correct trace, with more fills or a less polished outline. The palette and
the writer do not read it. A nonzero budget makes the output depend on the machine; 0 means
no budget and a reproducible output. Either way the merge is also bounded by a work cap set
by the pixel count". Without a budget no clock is read: the merge stops at a deterministic
work cap, `max(2²⁸, 32 per pixel)` units, which is on with a budget too (`MERGE_WORK_FLOOR`, `MERGE_WORK_PER_PIXEL`,
`crates/inkvec-trace/src/gradient/bands.rs:40-71`; [05-gradients.md](05-gradients.md)), and the
boundary solve stops on its iteration count, and is skipped outright when its band tables
would pass `max(256 MiB, 32 bytes per pixel)` (`TABLE_BUDGET_FLOOR`, `TABLE_BUDGET_PER_PIXEL`,
`crates/inkvec-trace/src/boundary_opt/band.rs:851-856`;
[08-boundary-solve.md](08-boundary-solve.md)). Both caps were set from what images spend,
"with room, so that it never binds on the gate's" (`bands.rs:42`). Fast mode lists
`--time-budget` among the options it ignores (`crates/inkvec-cli/src/fast.rs:71`). The two
fractions (`0.6`, `0.25`) and the 50 ms floor are not derived from a sweep in anything read
for this stage; see Open questions.

### Resolution invariance: `content_scale`, `REF_EXTENT`, `fit_config`, `in_content_units`

This is the subtlest mechanism in the stage, and it exists to fix a real, measured disease.
Every pixel-denominated constant in the tracer — the positional uncertainty
`DEFAULT_SIGMA_MODEL` (≈0.05 px, stage 02), the coordinate price `--precision` (0.1 px), the
speckle floor `--min-area` (2 px²) — was set against a corpus whose images are roughly
128 px on the long side. Handed a 512-px raster of the *same* logo, nothing about those
constants changes, but the geometry the fitter sees does: four times the boundary points,
each carrying four times the pixel residual for the same relative fit, while `lambda`
(`= ln(extent/precision)`, `FitConfig::from_precision`, `crates/inkvec-fit/src/lib.rs:147-168`)
has grown by only `ln 4 ≈ 1.39` nats, about a fifth of its value — nowhere near enough to
offset a chi-squared term that scales with the square of the residual times four times as
many points. The fitter therefore keeps buying segments it does not need, because each
additional segment looks cheap next to how expensive staying wrong has become. Measured on
real brand logos (`crates/inkvec-cli/src/units.rs:17-26`): **3.54× the artist's own
parameter count at 128 px, 6.02× at 256 px, 11.62× at 512 px, for content that had not
changed at all.** A brand mark has one complexity, and the doc comment's framing is exact: a
vectoriser should return it "whatever resolution the export happened to be."

`REF_EXTENT = 128.0` (`units.rs:17-31`) records the size the corpus was tuned at. The scale
`s` — pixels per unit of content — is measured by `content_scale` (`units.rs:33-78`) rather
than inferred from the extent:

```rust
pub(crate) fn content_scale(img: &inkvec_trace::Rgba, args: &Args) -> f64 {
    if !args.content_units {
        return 1.0;
    }
    let rgb = img.composited([1.0, 1.0, 1.0]);
    ...
    let edge = inkvec_trace::coverage::intake_scale(&rgb, img.width, img.height);
    let redundancy = inkvec_trace::coverage::oversample_factor(&rgb, img.width, img.height) as f64;
    edge.max(redundancy).max(1.0)
}
```

Two measurements, asking different questions: `intake_scale` (edge width) catches blur,
resampling and upscaling and reads 1.00 on anything with honest one-pixel boundaries;
`oversample_factor` (the downsampling round trip, [02-coverage.md](02-coverage.md)) sees a
simple drawing rendered larger than it needs — "a natively rendered icon reads 1, 1, 2 and 4
at 128, 256, 512 and 1024 while its edges stay exactly 1.00 px wide throughout"
(`units.rs:59-74`). Content that is genuinely resolved reads 1.0 and is left as it was found;
`s` is never below 1.0. It used to return `extent / REF_EXTENT`, which "asks a different
question -- how big is this?": a natively rendered icon reads an edge width of 1.00 at 128,
256, 512 and 1024 alike, where `extent / REF_EXTENT` claims 1x, 2x, 4x and 8x, so the old
scale loosened every tolerance eightfold on a 1024 px render that had lost no detail at all
(`units.rs:41-50`). The diagnostics module records the same disease: "`content_scale`
returned `extent / 128` -- 12.5 where the measured answer was 4"
(`crates/inkvec-trace/src/diag.rs:16`).

Two things then use `s`, and both reach the emitted trace:

**`in_content_units`** (`crates/inkvec-cli/src/units.rs:107-118`) multiplies a fitted
polyline's *positional sigma* — the per-point uncertainty from stage 02 — by `s`. This is
called on every boundary before fitting, in both `run_bilevel`
(`crates/inkvec-cli/src/pipeline.rs:71-75`) and the colour path's `fit_boundaries`
(`pipeline.rs:660-668`), whenever `--content-units` is on, regardless of which other path
produced the image. The fitter weighs each point's squared deviation by `1/sigma²` (its
chi-squared term, `crates/inkvec-fit/src/lib.rs:82-90`; the `tau * sigma` admissibility cone
the module header once described is no longer on the shipping path, `lib.rs:92-98`), so
inflating `sigma` by `s` divides each point's term by `s²`: the same *relative* deviation —
now `s` times larger in raw pixels, because the boundary itself is `s` times bigger — costs
per point what it would have cost at one pixel per unit of detail. The bilevel path deliberately leaves `--min-area` in pixels:
"noise does not scale with the raster", and scaled by `s²` it swallowed the 1-2 px gaps
between a scribble's strokes (abra_agency tile error 602 → 371 with the floor back in pixels;
60 real brands at 512 px, dE00 worst 1.33 → 0.96, 2026-09-05; `pipeline.rs:61-67`).

Fast mode's fitter reads neither these polylines nor their sigmas: it fits the map's edges
directly, in pixels, with its own tolerances (see `14-fast-mode.md`). So in Fast mode
`fit_boundaries` (`pipeline.rs:624-777`) builds the content-unit polylines, the per-edge λ
multipliers and the `content_scale` they need only when something reads them:
`--editability`, whose passes read the polylines, or the research build's structural
baseline (`INKVEC_STRUCTURAL`). The condition is
`need_polys = !fast || args.editability || structural` (`pipeline.rs:658-659`); otherwise
`Fits::polys` and `Fits::lambda_scales` are empty, and the fitted paths, which depend on
neither, are unchanged (0.47 ms of the fit stage at 2048 px, `pipeline.rs:641-648`). Quality
builds them, and then fits each boundary on every core through
`inkvec_fit::choice::describe` (`pipeline.rs:706-734`): the dynamic program on its
content-unit polyline, beside the whole-boundary primitive search, the cheaper of the two
kept; for the image frame the rectangle is tried first and the program skipped when the
rectangle is provably cheaper (`11-fitting.md`, *Curve or primitive*). Fast mode does still
call `content_scale` once, through `fit_config`, which every trace builds its configuration
with (below): it returns 1.0 at once unless `--content-units` is set, and under that flag,
which Fast lists among the options it ignores (`crates/inkvec-cli/src/fast.rs:68`), it still
runs the scale's two raster passes there.

**`fit_config`** (`crates/inkvec-cli/src/units.rs:80-105`) goes further: it also multiplies
`--precision` by `s` before deriving `lambda`, and then multiplies the resulting `lambda` by
`s` again:

```rust
pub(crate) fn fit_config(img: &inkvec_trace::Rgba, args: &Args) -> FitConfig {
    let extent = img.width.max(img.height) as f64;
    let s = content_scale(img, args);
    let mut cfg = FitConfig::from_precision(extent, args.precision * s, args.tau);
    cfg.lambda *= s;
    cfg
}
```

So `lambda = s · ln(extent / (s · precision))`. The doc comment gives both factors
(`units.rs:82-91`): asking for `precision * s` makes a coordinate cost "the nats of a
coordinate at the content's own resolution", and "the further factor of `s` on lambda is the
point count: the data term is a sum over boundary samples, there are `s` times as many of
them per unit of content, and pricing a parameter in the same currency means scaling its
cost by the same `s`. Together with sigma scaled by `s` at the fit ([`in_content_units`]),
the optimum is the one a raster at one pixel per unit of detail would reach -- from better
points." With `--content-units` off, `s = 1` and this is `FitConfig::from_precision(extent,
precision, tau)` unchanged.

**Both halves reach the emitted SVG.** `trace_prepared_priced` builds the configuration it
hands to `run_strokes`, `run_bilevel` and `run_color` through `fit_config`
(`crates/inkvec-cli/src/lib.rs:754-762`), and so does the probe trace (`trace_once`,
`lib.rs:1071`). The comment at the call says why:

> Through `fit_config`, not `FitConfig::from_precision` directly, so that
> `--content-units` applies the whole of its mechanism here and not half of it.
>
> It used to build its own configuration and skip the scaling, which left the flag
> scaling sigma (via `in_content_units`, below) while lambda stayed tied to raw pixel
> extent -- exactly the half that `fit_config`'s doc comment says must not be applied
> alone. `content_scale` returns 1.0 unless the flag is set, so this is the identity
> on the default path: `FitConfig::from_precision` with nothing scaled.

No test checks that identity, or that the flag now scales lambda; see Open questions.

`content_units` defaults to `false` (`crates/inkvec-cli/src/args.rs:231`); the
`INKVEC_CONTENT_SCALE` environment equivalent was removed in 0.1.7 (CHANGELOG, *Removed*).
The help text keeps it off as a trade, "it trades fidelity for parsimony" (`args.rs:396-399`).
It used to name two failures as the price (5-px squares fitted as circles, thin rings broken);
`content_scale`'s own doc comment traces those to the old `extent / REF_EXTENT` scale — "that
over-correction, described but never traced to its cause" (`units.rs:41-50`) — so the help no
longer names them, and no measurement of the measured scale against them is recorded. See Open
questions.

### The oversample block: pricing `--precision`, `--min-area` and lambda in the raster's own units

A separate mechanism, always on in Quality and independent of `--content-units`, addresses a
related but distinct disease: a raster that is native (not resized, not compressed) but
carries the drawing on more pixels than it needs — a super-resolution model's own output,
for instance, which returns genuinely sharp edges at high resolution and so is invisible to
`intake_scale` (edge width). `price_in_raster_units`
(`crates/inkvec-cli/src/lib.rs:331-457`) runs at the end of `intake` (`lib.rs:320`), after
unblock, `--intake-scale` and `--max-dim`. The comment gives the measured cost of leaving
this alone (`lib.rs:346-355`): on a real brand mark upscaled 4×, 301 paths and 11,960
coordinates against 16 paths and 456 coordinates for the same drawing at 1×; hand-scaling
`--precision` and `--min-area` brought it back to 21 paths and 441 coordinates.

Two measurements of the image composited over white decide it — none in Fast mode, which
reads none of the three knobs and for which "the two measurements are three full-image round
trips, the largest cost in a fast trace at 2048 px" (`lib.rs:363-369`):

```rust
let edge = inkvec_trace::coverage::intake_scale(&rgb, w, h);
...
let round_trip = inkvec_trace::coverage::oversample_factor(&rgb, w, h) as f64;
let up = if edge > inkvec_trace::color::SOFT_INTAKE_EDGE {
    round_trip
} else {
    1.0
};
(up, round_trip)
```

(`lib.rs:372-392`). With `r` the round-trip factor and `R` = `REF_EXTENT`, the doc comment
(`lib.rs:334-341`) states the rule:

* precision `× r` when the edge width exceeds the soft-intake threshold, else unchanged;
* min-area `× r²` when the longest side exceeds `R`, else unchanged;
* lambda `× r` when the longest side exceeds `R` and `r > 1`, else unchanged — through
  `args.lambda_scale`, which `fit_boundaries` multiplies into every boundary's lambda
  (`crates/inkvec-cli/src/pipeline.rs:669-682`) and the ring repair prices with
  (`pipeline.rs:885`).

The round trip is not 1 on every native render: it reads 1 for 212 of the 246 screen icons
at 128 px, and "at 512 px nearly every native render reads 2 to 8, which is the speckle floor
scaling that caller wants" (`crates/inkvec-trace/src/coverage/oversample.rs:113-117`), and
`price_in_raster_units`' comments say the same: "at 512 px its `r` is mostly 2 to 8"; "A
native intake at or below `REF_EXTENT` is left as it is (see below), so the 128 px benchmark
is untouched" (`crates/inkvec-cli/src/lib.rs:343, 358-359`). What keeps the 128 px tier
untouched is the `REF_EXTENT` guard below, with the edge-width gate for precision.

**Precision stays behind the edge-width gate.** `intake_scale` decides *whether* this is
native content — every corpus raster reads at most 1.50 against the `SOFT_INTAKE_EDGE = 1.75`
threshold (`crates/inkvec-trace/src/color.rs:398-407`, documented fully in `03-palette.md`) — a
job the round trip cannot safely do on its own, because "smooth native artwork survives
halving too, and would be rewritten for no reason". The round trip then measures *by how
much*, which `intake_scale` cannot: a super-resolution model's sharp output "reads 2.00 where
the answer is 4" (`crates/inkvec-cli/src/lib.rs:372-378`). Precision needs no more than that:
on ten corpus icons at 128, 256, 512 and 1024 the segment count grows only 1.33× across the
range, a search for the precision that reproduces the 128 px drawing picks the shipped 0.1 at
every tier, and `prim_ellipse` comes out as nine segments at all four with no correction
(`lib.rs:397-403`).

**The speckle floor follows the round trip alone**, because it is an area: an anti-aliasing
sliver is eight times longer and eight times wider at eight times the resolution, so it
carries sixty-four times the area, sails over the floor and becomes a face. Measured on
`mosaic_grid6`, same palette either way: 44 faces at 128 against 772 at 1024, where 131
segments become 2367; scaling the floor by the round trip squared brings that back to 94
faces and 293 segments (`lib.rs:405-411`). It is relative on purpose: a flat 9-px colour-mode
floor tried on 2026-09-03 measured worse on the full 980-icon set (objective 0.7885 against
0.7673, 508 icons worse), because it erased real dots as readily as confetti
(`lib.rs:413-418`). And only above `REF_EXTENT`: `min_area` was fitted against 128 px rasters
as they are, so scaling by their redundancy again counts it twice — measured, 128ss objective
0.4005 → 0.4112 without the guard (`lib.rs:419-425`).

**The round trip must keep the drawing in relative terms too.** Since impl2/bugs
(merged 2026-10-03) a factor passes only when its mean round-trip error is under
`OVERSAMPLE_TOL = 3` levels *and* at most `OVERSAMPLE_KEEP = 0.5` of `flat_error`, the error
of erasing everything but the per-channel median (`crates/inkvec-trace/src/coverage/oversample.rs:26-53`).
A near-empty 144 px raster with one 38 px² disc passed the absolute test at every factor,
"because the empty canvas dilutes the mean, read as 8x, and its disc fell under the 64-fold
speckle floor" (`crates/inkvec-cli/src/lib.rs:380-384`); it now reads 2× and the disc is
drawn. `oversample_factor`, its two tests and their margins are documented in
[02-coverage.md](02-coverage.md).

The lambda factor carries no measurement of its own in these comments; see Open questions.
When anything scales, the stats show it: "intake  oversampled xN: scaling precision xN,
min-area xM, lambda xL", or "intake  Nx more pixels than detail: scaling min-area xM and
lambda xL" when only the round trip moved (`lib.rs:437-448`). This happens independently of,
and before, the `--content-units` machinery above; both can be active on the same trace.

### Art that touches the canvas edge: the border pad

A transparent raster whose artwork reaches its outermost ring of pixels (a tightly cropped
logo, a sticker filling its canvas) used to trace worse than the same artwork with a little
room round it. Where the art meets the canvas edge the planar map has frame edges and
junction nodes pinned to the frame, and the boundaries that end there are fitted as open
curves between two frame junctions instead of the closed outline the artist drew: a rounded
square filling its canvas (`twemoji/1f7eb`, 512 px) came back with one corner replaced by an
arc spanning half a side, dE00 0.117, and comes back as one rounded rectangle with the pad,
dE00 0.0002. The r2-eval research (2026-10-02, `padtest.py`) found it with a metamorphic
test (a one-pixel translation of the input lowered the screen set's score by 5.2 %) and
measured the oracle: embedding the raster in a 2 px transparent margin, tracing, and
cropping the render back lowered the screen set's dE00 by 2.0 % at 512 px and 5.3 % at
128 px, by 9.7 % and 22.1 % on its 72 border-touching icons, while the rest moved by +0.1 %
and +1.7 % only because the padded raster is larger and the size-dependent defaults read the
larger size.

So `trace_prepared_priced`, after every resampling step and before the matte
(`crates/inkvec-cli/src/lib.rs:640-650`, through `trace_bordered`, `lib.rs:670-699`), traces
such a raster on a padded canvas and moves the document back
(`crates/inkvec-cli/src/border.rs`):

1. **Which rasters** (`touches_border`, `border.rs:57-82`): some pixel is not fully opaque
   (transparency is the raster's ground, so a transparent pad continues it) and some pixel of
   the outermost ring has alpha above zero. An opaque raster is never padded: its ground is
   whatever colour fills the border, and a transparent pad would send it down the alpha path.
   Of the 246 screen icons 81 qualify at 128 px and 78 at 512 px; the rest trace exactly as
   before (byte-identical, checked on all 168 non-touching icons at 512 px).
2. **When** (`border_pad_applies`, `lib.rs:712-723`): the Quality colour pipeline only; Fast
   mode, the bilevel, stroke and monochrome writers, `--use-symbols` (whose symbols are in
   their own frame) and `--uncertainty` (whose bands go to a separate file) trace as they
   always did.
3. **The pad** (`pad`, `border.rs:84-101`): the raster embedded at `(PAD, PAD)` in a canvas
   `PAD` = 2 px larger on every side, the new pixels `[0, 0, 0, 0]`.
4. **The defaults see the original size.** The knob pricing (`price_in_raster_units`) has
   already run on the original raster in `intake`, and the fit configuration is built for
   the original longest side (`fit_config_sized`, `crates/inkvec-cli/src/units.rs:96-105`,
   through `trace_matted`'s `extent`, `lib.rs:725-783`), so the price of a coordinate,
   `ln(extent / precision)`, and the 128 px reference-extent test do not move. The trace
   crate's own size-dependent priors (the gradient fits' `bic_lambda(w·h)`, `ln` of the
   pixel count) still read the padded size: at 128 px that is `ln 17424` against `ln 16384`,
   0.6 % on a log price.
5. **The crop** (`crop`, `border.rs:103-198`): every coordinate of the traced document moved
   by `(−PAD, −PAD)` and the root's `viewBox`, `width` and `height` written for the original
   size, so the document keeps the emitter's header and pixel coordinates that
   `crate::post` (the margin, the background knock-out) and the Studio overlays read. A
   translation is exact, and each number keeps the decimals it was written with, so the
   result is the exact decimal. What is translated is what the colour emitter writes: the
   absolute coordinates of path data (`shift_path_data`, `border.rs:331-434`; a relative
   command moves nothing except a path's opening `m`), `<rect>`, `<circle>`, `<ellipse>` and
   its `rotate(a cx cy)` centre, `<line>`, and `userSpaceOnUse` gradients, whose
   `gradientTransform`, when there is one, gets the translation in front of it
   (`prepend_translate`, `border.rs:293-314`). Anything else that carries coordinates
   (`<use>`, `<symbol>`, `<image>`, `<text>`, `<polygon>`, a shape inside `<defs>`, any other
   `transform`) makes `crop` refuse, and the raster is then traced again unpadded
   (`lib.rs:691-694`).

"Not from the literature: a boundary condition for our own planar map (zero padding, the
usual way image filtering gives a support that reaches past the image a known exterior,
applied here to the tracer as a whole)" (`border.rs:43-50`); the metamorphic relation that
found the defect is after T. Y. Chen, S. C. Cheung, S. M. Yiu (1998), *Metamorphic testing:
a new approach for generating next test cases*, HKUST-CS98-01,
<https://arxiv.org/abs/2002.12543>.

Measured on the gate (2026-10-04, the 246-icon screen set judged at 1024 px, paired against
the same build without the pad, the converged boundary solve of
[08-boundary-solve.md](08-boundary-solve.md) underneath): dE00 −6.21 % at 128 px and
−2.16 % at 512 px, parameters −2.29 % and −1.72 %, both "better"; on the touching icons
alone −20.0 % (81 icons) and −10.8 % (78 icons), and every other icon byte-identical. The
whole trace at 512 px took 1.17–1.19× v0.2.5 with the pad, against 1.13× without it (30
icons, two interleaved passes, under load; the finished branch 1.22–1.24× under heavier
load). One fitter interaction was seen on the way: with
a 128-iteration solve,
`simple-icons/debridlink` traced on the padded canvas came back with a closed ring whose
first segment skips the corner its seam sits on (a sliver 5 px wide, dE00 0.033 → 0.216);
with the shipped 64-iteration solve it does not occur, and the ring fit is stage 11's.

### `--hypotheses` (opt-in): structural alternatives chosen by description length

Three structural decisions of the colour tracer are right for most images and wrong, by a
lot, on a minority: blend absorption (a grey pixel between two black shapes read as their
anti-aliased blend closes a real sub-pixel gap; `simple-icons/ubiquiti`), native alpha
against a matte (some translucent overlays trace closer matted), and the merge distance
(0.035 in OKLab merges inks an artist kept apart). The r2-fidelity research (section 3.6)
measured each wrong on 14–34 % of the screen set, each worse when used always, and a
selector that keeps the variant whose render best explains the input raster within 0.2 % of
choosing with the artist's file in hand.

With `--hypotheses` (`Options::hypotheses`), `trace_prepared_priced` traces the image as the
settings ask and then once per applicable alternative (`select::hypotheses`,
`crates/inkvec-cli/src/select.rs:95-129`): blend absorption off (`Args::absorb_blends`, handed to
the trace crate as `ColorOptions::absorb_blends`), a matte instead of native alpha when the
raster has transparency, and merge 0.020; each through the border pad like the default
trace (`trace_bordered`, `crates/inkvec-cli/src/lib.rs:670-699`; the dispatch is
`lib.rs:640-650`). `select::choose` (`select.rs:131-193`) keeps the trace
with the shortest two-part description length,

```text
DL_i = RSS_i / (2·σ̂²) + 0.75 · k_i · ½·ln N,      σ̂² = min_i RSS_i / N,      N = w·h / e²
```

with `RSS_i` the squared sRGB error between trace `i` rendered at the input's resolution
(an 8x-to-1x supersampled render, at most 1024 px, box-averaged; `render_residual`) and the
input, both over white; `k_i` the geometry numbers of the document, counted as the benchmark
counts them (`count_params`); and `e` the measured edge width. Method from J. Rissanen
(1978), *Modeling by shortest data description*, Automatica 14(5):465–471,
<https://doi.org/10.1016/0005-1098(78)90005-5>, and S. C. Zhu, A. Yuille (1996), *Region
competition*, IEEE TPAMI 18(9):884–900, <https://doi.org/10.1109/34.537343>; the 0.75 is the
in-house measurement's (2026-09-16) price per number. Quality colour mode only, and not
with `--uncertainty`; the default path is unchanged (byte-identical on 165 sampled traces,
Quality at 128 px and 512 px opaque, and Fast).

Measured against the same build without it (2026-10-04, judged at 1024 px against the
artist's file): −6.0 % dE00 at 128 px on the screen set and −7.6 % on `held_a`, −0.9 % at
512 px with the worst tenth −8.9 %; 14 icons better and 15 worse by more than 0.01 at
512 px, the losses mostly lucide outlines the matte hypothesis paints as a filled shape with
a hole, which the renderer anti-aliases differently from the artist's stroke. Parameters
+6.9 % at 128 px (three icons, where blend absorption off keeps anti-aliased slivers as
faces) and −0.2 % at 512 px. 3.6 times the trace time at 512 px, which is why it is off by
default.

### Alpha handling and matting

Everything from `coverage::bilevel_coverage` (stage 02) onward reads an opaque image: the
coverage projection needs definite opaque colours to unmix a pixel against. So intake's last
step, after every resampling decision above and after the pre-passes, is to write the image
down opaque (`alpha_source_owned`, called at `crates/inkvec-cli/src/lib.rs:737-752`; see
"Applying the matte" below).

What "opaque" means depends on native alpha, on by default (`--no-native-alpha` or
`INKVEC_NATIVE_ALPHA=0` turn it off; `crates/inkvec-cli/src/args.rs:98-102, 231-232`).
Natively, nothing is chosen: the image is written over white, the cutout is on, and the
alphas travel beside it into the colour trace, whose palette finds inks with an opacity of
their own (`crates/inkvec-cli/src/alpha.rs:1-10, 648-658`). Under `--no-native-alpha` the
tracer proper is opaque and the palette has no fourth, alpha, dimension to cluster on, so the
matte matters, and `choose_matte` (`alpha.rs:343-541`) picks it by what it would *swallow*
rather than by a fixed choice. White was the historical default, and white is exactly wrong
for the input people bring most often: a white mark on a transparent ground composites to
one flat white and traces to nothing at all; a pale translucent panel disappears into a
white background it is drawn over. The function walks the image once, classifying drawn
pixels into three kinds — the silhouette and its anti-aliased rim, flat translucency (one
opacity, which the emitter can later carry out as `fill-opacity`), and a "glow" (opacity
that varies across the shape, which no single flattening colour can serve honestly and so
gets no vote) — then tries six candidate colours in order (white, black, magenta, green,
cyan, orange) and keeps the first one that would not composite `SWALLOWED = 0.33` or more
of the drawn-and-translucent mass into itself, measured by CIEDE2000 within `MARGIN = 10.0`
(`alpha.rs:324-329, 374-391, 400-418`); if none passes, the cheapest. Two cheaper rules were
tried and measured worse on the 246-icon screen set first — "the furthest candidate from
every colour in the image" swung a white highlight buried in an emoji onto a saturated matte
(0.4123 → 0.4735); voting on silhouette colours alone did the same for a white sock — which is
why the bar is a *share of drawn mass*, not a distance to any single ink (`alpha.rs:357-361`).
`SWALLOWED` is module-level because `alpha_source` used to restate it as a literal `0.33`
until 2026-09-08, "two numbers that had to agree, with nothing keeping them in agreement"
(`alpha.rs:324-328`).

This choice only reaches the traced faces under `--cutout` (default off): without it, the
image is matted to plain white, because transparency cannot reach the output anyway (a
transparent face is painted solid, not punched, and translucency is baked flat), so choosing
a content-aware matte would move every edge in the file for no gain it could carry. The doc
comment on `alpha_source` records exactly why this gate is not optional: the committed CI
screen-set gate rejected an always-on content-aware matte outright — dE00 0.15404 → 0.15654
against a limit of 0.15558 — because the corpus is scored over white and cannot see any of
the gain a better matte buys on other backgrounds (`alpha.rs:563-569`). Except where white
erases the artwork: when more than `LOST_TO_WHITE = 0.5` of the drawn silhouette, composited
over white, is paint the palette cannot tell from white (at least 0.9 in every channel and
within `SAME_INK_DE00` of it), the cutout is turned on for that image and the chosen matte
applies — the LogoLabs flask in white scored alpha error 0.89 either way and 0.0008 with the
cutout (`alpha.rs:571-578`). That bar is deliberately not `SWALLOWED`'s dE00-10 margin: at
that margin a near-white edge counts as lost although the tracer separates it from white
easily, and turning the cutout on for those images made the printer and bride emoji worse on
every ground (`noto-emoji/emoji_u1f5a8` dE00 0.53 → 0.76); at the same-ink margin white marks
on a transparent ground read 1.00 and no icon of the screen set reads above 0.32
(`alpha.rs:331-341`). `INKVEC_MATTE`, which used to force the matte from the environment, was
removed in 0.1.7 (CHANGELOG, *Removed*).

**Applying the matte: once, in place, in parallel.** The intake calls `alpha_source_owned`
(`alpha.rs:598-624`) rather than the copying `alpha_source` (`alpha.rs:560-596`), which only
the probe trace of `--sr auto` / `--restore auto` still uses (`crates/inkvec-cli/src/lib.rs:1076`).
The steps:

1. **Any transparency at all?** `has_transparency` (`crates/inkvec-cli/src/alpha.rs:831-852`)
   reads the fourth float of every whole pixel and stops at the first under 0.999 — the same
   set of floats the old strided scan visited. On an opaque image that is a read of every
   alpha, 3.3 ms serial at 2048 px; it is now split over rayon's workers from 256 × 256
   pixels on (`INTAKE_PARALLEL_MIN`, `alpha.rs:825-827`), in jobs of at least 16,384 pixels
   (`FLATTEN_CHUNK`, `:828-829`). `any` is a pure predicate, so the answer cannot depend on
   the split. An image with none goes on untouched (`Err(img)` hands it back).
2. **The matte is decided on the untouched image** (`MattePlan::of`, `alpha.rs:639-668`):
   white with the cutout on for the native-alpha path; otherwise `choose_matte` as above,
   with the cutout turned on when more than `LOST_TO_WHITE` of the silhouette would vanish
   into white, and the chosen matte applied only under the cutout.
3. **Every pixel is flattened over the input's own buffer** (`flatten_in_place`,
   `alpha.rs:792-823`) by `flatten_pixel` (`alpha.rs:778-790`): with `a' = clamp(a, 0, 1)`,
   `p ← [r·a' + M_r·(1 − a'), g·a' + M_g·(1 − a'), b·a' + M_b·(1 − a'), 1]`, the old push
   loop's arithmetic operand for operand. The copy `flatten_over` made was a fresh 64 MB
   buffer at 2048 px, whose page faults and release the shared-stage research measured at
   5.7 + 1.7 ms (`alpha.rs:602-605`), and it was a serial push loop, 26.7 ms of a 2048 px
   transparent trace (`alpha.rs:731-734`). The flatten is now parallel above the same
   threshold, and each output pixel depends on one input pixel, so the split cannot change
   a bit.

The notes printed to stderr (the matte or "native", the share of clear pixels, why the
cutout was turned on) are the old ones in the old order (`MattePlan::finish`,
`alpha.rs:670-700`). Oracles for all three rewrites (the strided scan, the copying flatten,
the gcd-free unblock loop) are kept in `crates/inkvec-cli/src/alpha/intake_tests.rs`.
Citations, as the doc comments give them: the flatten is "Method from" Porter & Duff,
"Compositing Digital Images", SIGGRAPH '84 (the "over" operator with an opaque background,
adapted to straight colour, `crates/inkvec-cli/src/alpha.rs:736-738`); writing it in place is
"Not from the literature: buffer reuse", with "See also" Leijen, Zorn & de Moura, "Mimalloc:
Free List Sharding in Action", APLAS 2019, the allocator-side answer to the same page-fault
cost (`alpha.rs:609-612`).

Every stage of both modes then reads the image composited over white, and that composite,
`Rgba::composited` (`crates/inkvec-trace/src/coverage.rs:198-232`), is parallel too from
256 × 256 pixels on (`COMPOSITE_PARALLEL_MIN`, `coverage.rs:235-236`): 11–18 ms serial at
2048 px before, the same "over" expression per pixel after, checked bit for bit against the
serial map (`the_parallel_composite_is_the_serial_one`, `coverage.rs:260`). Its citation is
Porter & Duff's "over", adapted to straight colour, and "Not from the literature: the
parallel split" (`coverage.rs:210-214`).

`crates/inkvec-trace/src/alpha.rs` is a related but distinct mechanism, run *after* the
trace rather than during intake: `decompose_with` (`crates/inkvec-trace/src/alpha.rs:380`,
beside `decompose` at `:370`) recovers a translucent layer — one shape at one opacity, seen
through several different backgrounds — from the flat face partition the trace already
produced. The module doc comment (`crates/inkvec-trace/src/alpha.rs:1-134`) states the
algebra plainly: a layer of colour `C` at opacity `a` over two *different* backgrounds `G1`
and `G2` produces two observed faces whose difference `c_F1 − c_F2 = (1−a)·(c_G1 − c_G2)` is
independent of the unknown `C` — a face seen against only one background can never be
distinguished from a flat region, so a layer with only one hypothesis is rejected, always.
Recovering the layer needs at least two hypotheses over well-separated backgrounds, a
quad-adjacency test that matches the actual geometry of a translucent edge crossing a
background edge, and a battery of conservative gates (opacity in `(0.05, 0.98)`, standard
error on the recovered opacity bounded, and more — `crates/inkvec-trace/src/alpha.rs:68-89`),
because a false layer is a visible error: it unions faces that are not one shape and paints
them a colour that appears nowhere in the source. This is exposed as `--layers` (default
off; `INKVEC_LAYERS` is *removed*), and the CLI's help records it firing on "about one real
icon in twenty" of the census used to tune it — two of forty
(`crates/inkvec-cli/src/args.rs:352-358`, `crates/inkvec-cli/src/alpha/layers.rs:49-54`). The CLI
side, `recover_layers` (`layers.rs:100`, split out of the CLI's `alpha.rs` on 2026-10-02),
holds the decomposition to three rules, each closing a way the layered document used to draw
something other than the image (`layers.rs:62-99`):

1. **sRGB only.** The document composites `fill-opacity` in sRGB (SVG 1.1 §11.7.1,
   `color-interpolation`), so only `inkvec_trace::alpha::Space::Srgb` is tried. It used to
   try linear light too and keep whichever found more layers: on
   `noto-emoji/emoji_u1f469_1f3fb_200d_1f52c` a black layer at 0.924 recovered in linear
   light repainted the dark grey hair near black, dE00 0.609 -> 2.92.
2. **Flat faces only.** A gradient face has no single ground colour to repaint, so it is
   given no area and takes no part.
3. **Reproduction.** The layered document is written only when the layer stack composited
   over every covered face's recovered (gamut-clamped) ground comes back within
   `LAYER_MAX_DE00 = 1.0` of the face's own colour (`layers_reproduce`, `layers.rs:203`).

The emitter then paints the recovered layers back to front: the decomposition peels the
frontmost layer first, and painting in peel order put the front layer underneath
(`synthetic/stack_overlap`, dE00 0.019 -> 2.90 with `--layers`; `write_layers`,
`crates/inkvec-cli/src/emit.rs:1270-1287`; see `13-emit.md`). Citations, as the code gives
them: "Method from" Porter & Duff 1984 (doi:10.1145/800031.808606) and SVG 1.1 §11.7.1; the
three rules "Not from the literature"; "See also" Richardt et al., EGSR 2014
(doi:10.1111/cgf.12408).

## Constants and thresholds

| name | value | controls | stated derivation |
|---|---|---|---|
| `REF_EXTENT` | `128.0` px | the intake size every pixel-denominated tracer constant was tuned at; above it `price_in_raster_units` scales `--min-area` and lambda | measured: 3.54×/6.02×/11.62× the artist's parameter count at 128/256/512 px for unchanged content (`crates/inkvec-cli/src/units.rs:17-31`); the guard measured: without it 128ss objective 0.4005 → 0.4112 (`crates/inkvec-cli/src/lib.rs:419-426`) |
| `INTAKE_SCALE_FLOOR` | `1.5` | below this, `--intake-scale` leaves the input alone | "Every image in the corpus reads exactly 1.00" (`lib.rs:119-121`) |
| `INTAKE_SCALE_CAP` | `8.0` | ceiling on how much `--intake-scale` will discard from one estimate | "A wrong estimate should cost detail slowly, not all at once" (`lib.rs:122-124`) |
| `--max-dim` default | `2048` px | ceiling on traced (not emitted) size | qualitative: "keeps a typical logo under a few seconds" (`crates/inkvec-cli/src/args.rs:232, 316-320`); no sweep cited |
| `--time-budget` split | `0.6` merge / `0.25` boundary-solve | how an advisory wall-clock budget is allotted between the two stages that read a clock | stated as a fixed split, no numeric derivation given (`args.rs:332-339`, `crates/inkvec-cli/src/pipeline.rs:140-150`) |
| boundary-solve budget floor | `50` ms | least wall-clock budget the boundary solve gets under any `--time-budget` | none (`pipeline.rs:146`) |
| `MAX_FACTOR` (`pixel_grid`) | `32` | largest replication factor the unblock pre-pass will try | tried downwards so an 8× upscale is undone as 8×, not 2×; no numeric derivation for the cap itself (`crates/inkvec-cli/src/alpha/unblock.rs:20-27, 69`) |
| `smallest` (`pixel_grid`) | `64` px | least side a factor must leave; a raster under 128 px on either side is never unblocked | "a 2x undo of a small icon leaves too few pixels for the boundary solve to work with" (`unblock.rs:78-81`); no swept value |
| block-constant tolerance (`blocks_constant`) | `1.0/512.0` per channel | how exactly a block must match to be called a replication | "under half an 8-bit level, so for 8-bit input it is exact equality and only float round-off is forgiven" (`unblock.rs:23-25, 188`); no swept value |
| `SHARP` (`change_gcd`) | `1.0/256.0` per channel | a neighbour difference above this counts as a change position for the gcd filter | derived: twice the block tolerance, so two pixels of one passing block never differ by more (`unblock.rs:43-51, 131-132`) |
| `INTAKE_PARALLEL_MIN` | `65,536` px (256 × 256) | below this the transparency scan and the flatten run on the calling thread | motivated: "at 128 px they take microseconds" (`crates/inkvec-cli/src/alpha.rs:825-827`); no sweep |
| `FLATTEN_CHUNK` | `16,384` px | pixels per parallel job of the flatten, and the smallest job of the transparency scan | none (`alpha.rs:828-829`) |
| `PARALLEL_MIN_PIXELS` (`load.rs`) | `65,536` px (256 × 256) | below this the byte-to-float conversion runs on the calling thread | motivated: "at 128 px it is a few microseconds, less than handing it to rayon" (`crates/inkvec-trace/src/load.rs:274-276`) |
| `CONVERT_CHUNK_PIXELS` | `65,536` px | pixels per parallel job of the byte-to-float conversion | motivated: "enough work to amortise the job and few enough jobs (64 at 2048 × 2048) for rayon to balance" (`load.rs:278-280`) |
| `UNIT` | `k / 255` for `k` in `0..=255` | the float every 8-bit sample becomes | derived: holds exactly the quotients the old per-byte division gave (`load.rs:282-296`) |
| `DEFAULT_MAX_ALLOC` | `512` MiB | the decode allocation limit every uncapped decode keeps | derived: the `image` crate's own default, checked by `the_capped_limit_extends_the_library_default` (`load.rs:211-212, 953-969`) |
| `CAPPED_EXTRA_ALLOC` | `768` MiB on 64-bit, `0` on 32-bit | extra allocation a decode that will be capped may make (1.25 GiB in all) | motivated: holds 16384 × 16384 RGBA at 8 bits or 12000 × 12000 at 16 and still refuses the 20000 × 20000 fuzz bomb; nothing on WebAssembly's 4 GiB address space (`load.rs:214-223`) |
| `SRGB_TOLERANCE` | `1` level | largest move of any probe colour for an embedded profile to count as sRGB and leave the image untouched | motivated: converting an sRGB profile "would only add a level of rounding noise" (`crates/inkvec-trace/src/load/icc.rs:30-34, 66-67`); no sweep |
| `ROWS_PER_JOB` | `64` rows | rows per parallel job of the ICC conversion | none (`icc.rs:69-70`) |
| `COMPOSITE_PARALLEL_MIN` | `65,536` px (256 × 256) | below this the composite over white runs on the calling thread | motivated: below it "the pass is a few microseconds" (`crates/inkvec-trace/src/coverage.rs:203-208, 235-236`) |
| `MARGIN` (`choose_matte`) | `10.0` (CIEDE2000) | how close a composited colour must land to a matte candidate to count as "swallowed" | no stated numeric derivation (`crates/inkvec-cli/src/alpha.rs:384-385, 416`) |
| `SWALLOWED` | `0.33` | share of drawn-and-translucent mass a matte candidate may swallow before rejection | motivated by the white-highlight and white-sock cases; the specific `0.33` itself is not swept (`alpha.rs:324-329, 351-361`) |
| `LOST_TO_WHITE` | `0.5` | share of the drawn silhouette lost to a white matte above which the cutout is turned on (without native alpha) | measured: white marks on a transparent ground read 1.00, no screen-set icon above 0.32; the dE00-10 margin instead cost `emoji_u1f5a8` 0.53 → 0.76 (`alpha.rs:331-341`) |
| `DRAWN` | `0.5` | alpha above which a pixel counts as part of the silhouette rather than a glow/translucency vote | "faint content is baked against the matte whatever it is ... letting it vote flipped two emoji with soft glows onto a black matte" — qualitative (`alpha.rs:409-417`) |
| `DRAWN_FLOOR` | `0.05` | alpha below which a pixel is ignored entirely | no stated numeric derivation (`alpha.rs:418`) |
| `SOFT_SHARE` | `0.05` | share of drawn+soft pixels that must be "glow" before white is kept outright without running the candidate ladder | no stated numeric derivation (`alpha.rs:438`) |
| `FLAT_ALPHA` | `0.02` | spread in a pixel's neighbour alphas below which its translucency counts as "flat" rather than a glow | no stated numeric derivation (`alpha.rs:439`) |
| `DEGRADED_RESIDUAL` / `--sr-threshold` default | `0.5` | interior-residual threshold above which `--sr auto` cleans | measured over 30 icons, five conditions: sits above the worst clean reading (`0.376`) and below the weakest damaged one (`0.454`, jpeg-q80); "4.2% of damaged ones are missed" at this value (`crates/inkvec-sr/src/detect.rs:13-36`) |
| `--sr-scale` default | `2` | output scale of the pre-pass relative to the input | not derived in what was read; stated as the default only (`crates/inkvec-cli/src/args.rs:255`) |
| interior-residual normalisation | `sum / 9n`, not `sum / 3n` | scales every residual reading (and therefore `DEGRADED_RESIDUAL`) to `1/sqrt(3)` of a true RMS | deliberate, matched to the reference Python implementation; explicitly "not a bug to fix" (`inkvec-sr/src/detect.rs:103-110`) |
| `SOFT_INTAKE_EDGE` | `1.75` px | gates whether the round trip scales `--precision` | fully documented in `03-palette.md`; reused here unmodified (`crates/inkvec-trace/src/color.rs:398-407`) |
| `OVERSAMPLE_TOL` | `3.0` | absolute round-trip error tolerance inside `oversample_factor` | fully documented in `02-coverage.md`; reused here unmodified (`crates/inkvec-trace/src/coverage/oversample.rs:10-24`) |
| `OVERSAMPLE_KEEP` | `0.5` | largest share of the image's detail a round trip may lose (relative test) | fully documented in `02-coverage.md`; reused here unmodified (`oversample.rs:26-53`) |

## Failure modes and edge cases

- **Unblocking after the SR pre-pass instead of before it destroys the SR model's input.**
  Measured directly: a 96-px logo blown up to 768 and cleaned before unblocking came back
  with 14 inks and 1846 segments, worse than tracing the blocky image untouched
  (`crates/inkvec-cli/src/lib.rs:262-266`).
- **A compressed logo that passes both pixel-level checks is traced as if it were clean.**
  This was a real, dated regression, fixed 2026-09-08 (`96c3b79`): a JPEG logo measured
  `intake_scale` at 1.15 px, under the 1.75 px `SOFT_INTAKE_EDGE` threshold, so the
  palette's soft-intake guard stayed off and the tracer fitted the encoder's ringing as
  artwork — **243 paths over 1301 faces, 89% of them covering 5% of the drawing.** Two
  pixel-level detectors were tried and refuted first: `sigma_noise` (a median Laplacian)
  reads its `0.5/255` floor on the damaged file, identical to a clean render, because
  ringing occupies a thin band beside the edge while most of the image stays flat and never
  moves the median; a flat-region Laplacian statistic separated clean (`0.0000`) from JPEG
  (`1.1`–`2.7`) cleanly — until it met a genuinely clean radial gradient, which scored
  *higher* "damage" than the compressed flat icon, because 8-bit ramp quantisation produces
  the same Laplacian spikes a lossy codec does. The fix — reading `lossy_container` instead
  of inferring damage from pixels — reduced the same file to **80 paths over 654 faces**,
  with the face, wrench, hand and lettering intact: it removed noise, not artwork (commit
  `96c3b79`; the limitation is pinned by `ringing_does_not_widen_an_edge`,
  `crates/inkvec-trace/src/coverage.rs:1264-1273`). A JPEG re-saved as PNG defeats the
  container check; `ringing_score`, which reads where the energy sits rather than how much
  of it there is, now opens the same guard for it (see `resolve_lossy` above).
- **A mislabelled file was refused.** Before the format came from the bytes, a JPEG named
  `.png` or a PNG named `.jpg` failed to decode ("Invalid PNG signature", "Illegal start
  bytes"); both now decode as what they are (`crates/inkvec-trace/src/load.rs:114-122`). A file
  with no recognisable signature still goes to the decoder its extension names, whose error
  then describes the damage.
- **An image with a zero side reached the tracer.** A GIF with a zero-wide logical screen
  decoded without an error to 0 × 30000 and the unblock test divided by zero; such an image
  is now a decode error, "the image has no pixels" (`load.rs:255-264`).
- **A legitimate large file was refused as a decompression bomb**: a 12000 × 12000 RGBA PNG
  (576 MB decoded) failed with "Memory limit exceeded" although it would be traced at
  2048 px. A decode that will be capped may now allocate 1.25 GiB; claims beyond that are
  still refused before anything is allocated, and WebAssembly keeps the default
  (`load.rs:225-241`).
- **A phone photo traced sideways**, dE00 2.9 to 17.6 against 0.03 to 0.42 without the
  orientation tag; the EXIF orientation is now applied (`load.rs:168-173`).
- **A file in another colour space traced in the wrong colours**: six gamma-1.8 images
  averaged dE00 0.84 against 0.077 for the same art in sRGB (`crates/inkvec-trace/src/load/icc.rs:4-14`).
  A profile that cannot be read, or that does not match the decoded pixels (a CMYK profile
  on a JPEG the decoder has already turned into RGB), is still ignored and the colours read
  as sRGB, with an `INKVEC_DIAG` line saying why (`icc.rs:18-29, 76-111`).
- **Resolution dependence, before the fix, was severe and monotonic**: 3.54×/6.02×/11.62×
  the artist's parameter count at 128/256/512 px for a logo that had not changed — the
  disease `price_in_raster_units` and, opt-in, `content_scale`, `REF_EXTENT` and
  `in_content_units` exist to treat (see How it works, above).
- **A sparse raster was read as heavily oversampled.** A 38 px² disc on a near-empty 144 px
  canvas read 8×, the speckle floor rose 64-fold and removed the disc; the relative
  round-trip test reads it as 2× (`crates/inkvec-cli/src/lib.rs:380-384`). The dilution is
  not gone in general: a small dot beside a large shape is still floored by the drawing's
  factor — a large disc with a 38 px² dot reads 4×, floor 32 px², so a 30 px² dot would go
  (impl2/bugs report, 2026-10-03).
- **The decode-time cap runs before unblock on the command line.** `run` passes `--max-dim`
  to the decoder (`lib.rs:804`), and `pixel_grid` runs afterwards in `intake`, so a
  nearest-neighbour upscale larger than the cap reaches the block test already box-averaged
  by the cap's factor; when that factor does not divide the replication, the blocks are no
  longer constant and the upscale is not undone. Read from the code; no case was measured.
- **`--content-units` is offered as a trade, not a free fix**: its help text says "it trades
  fidelity for parsimony" (`crates/inkvec-cli/src/args.rs:396-399`). The two failures it used
  to name (5-px squares fitted as circles, thin rings broken) are ones `content_scale`'s doc
  comment attributes to the scale it replaced (`crates/inkvec-cli/src/units.rs:41-50`).
- **`--intake-scale` costs structural accuracy for speed**, and says so in its own help
  text: 24× faster with a twelfth of the parameters and half the colour error on 8×-upsampled
  input at 1024 px, but DINO — the corpus's structural measure — falls from 0.954 to 0.916
  (`crates/inkvec-cli/src/args.rs:515-519`).
- **The SR pre-pass is three times worse than doing nothing on clean input** (dE00 0.605
  against 0.195, `crates/inkvec-sr/src/detect.rs:3-7`), which is why `Mode::Auto` pays for a
  full probe trace before deciding, rather than guessing from pixel statistics.
- **`--sr auto` without an upscaler failed the whole trace**; it now keeps the probe and says
  so in the stats, as `--restore auto` does, while `--sr on` still fails
  (`crates/inkvec-cli/src/lib.rs:565-574`). A packaged tool that is found but fails while
  running still fails the trace in either mode (`lib.rs:613`).
- **With a `--time-budget`, the output depends on the machine**, and the palette and the
  writer read no budget at all (`crates/inkvec-cli/src/args.rs:332-339`); without one, the
  merge and the boundary solve are bounded only by their work and memory caps, which apply
  either way, and the output is reproducible.
- **A soft glow is deliberately excluded from voting on the matte**, and the cost of getting
  that wrong is recorded directly: letting a candle's flame vote on `noto-emoji/emoji_u1f56f`
  chose a black matte that baked the flame dark, dE00 0.22 → 5.31
  (`crates/inkvec-cli/src/alpha.rs:409-415, 435-437`).
- **`pixel_grid`'s exact-match requirement is a deliberate blind spot, not an oversight.**
  A resampled or anti-aliased upscale — bilinear, Lanczos, or anything that blends across
  block edges — fails the constant-block test by design; that class of damage is `--sr`'s
  job (`crates/inkvec-cli/src/alpha/unblock.rs:29-30`).

## Environment overrides

Since the settings cleanup (CHANGELOG, 0.2.0, *Changed*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

| variable | effect | default | source |
|---|---|---|---|
| `INKVEC_CONTENT_SCALE=1` (*removed*) | was equivalent to `--content-units` | unset (off) | removed in 0.1.7 (CHANGELOG, *Removed*); `--content-units` only |
| `INKVEC_MATTE` (*removed*) | forced `choose_matte`'s answer, bypassing the swallowed-mass search | unset (`choose_matte` decides) | removed in 0.1.7 (CHANGELOG, *Removed*) |
| `INKVEC_LAYERS` (*removed*) | forced `--layers` on | unset (off, same as `--layers` unset) | removed; `--layers` only |
| `INKVEC_LAYER_SIGMA` (*removed*) | overrode the sRGB noise sigma used when fitting a translucent layer | `LAYER_SIGMA_SRGB` (`crates/inkvec-cli/src/alpha/layers.rs:14-22`) | removed |
| `INKVEC_NATIVE_ALPHA=0` | turns native alpha off, as `--no-native-alpha` does: the image is matted first | unset (native alpha on) | `crates/inkvec-cli/src/args.rs:240-241` |
| `INKVEC_TOOLS_DIR=<dir>` | the folder holding the packaged SR pre-pass (`inkvec_sr`); when set, the only folder looked in | unset (`tools/` beside the binary or up to three folders above it, then the source checkout) | `crates/inkvec-cli/src/lib.rs:860-870` |
| `INKVEC_PYTHON=<program>` | the interpreter the packaged SR pre-pass runs with | unset (`python` on Windows, `python3` elsewhere) | `crates/inkvec-sr/src/external.rs:59-68` |
| `INKVEC_DIAG=1\|json` | structured diagnostic lines on stderr; the intake's are the ICC outcome (`load/icc.rs`) and the soft-intake gate (edge width, ringing, lossy flag) | unset (silent) | `crates/inkvec-trace/src/diag.rs:47-60`; intake lines at `crates/inkvec-trace/src/load/icc.rs:79-147`, `crates/inkvec-trace/src/lib.rs:375-381` |
| `INKVEC_ALPHADBG` | prints per-face alpha diagnostics and the emitter's face dump to stderr | unset (silent) | `crates/inkvec-cli/src/alpha.rs:1029`, `crates/inkvec-cli/src/emit.rs:604-607` |

No `INKVEC_*` variable is read in `inkvec-trace/src/load.rs` or `inkvec-trace/src/load/icc.rs`:
sniffing, the zero-side check, the decode limits, the ICC conversion, the orientation,
`from_dynamic` and `lossy_container` have no environment knob. `icc.rs` only emits
`INKVEC_DIAG` lines through the crate's `diag!` macro, which reads the variable in `diag.rs`.
The `inkvec-sr` crate reads one variable, `INKVEC_PYTHON`, through `inkvec_core::env`
like every other engine read; the CLI reads
`INKVEC_TOOLS_DIR` to find the packaged tool. Every knob of the SR pre-pass's decision and
clean-up is a CLI flag. Other files of the trace crate (`lib.rs` above all) do read many
`INKVEC_*` variables for palette, boundary-solve and other downstream stages, but none of
those reads happen in the loader.

## Open questions

- **Nothing tests `fit_config`'s wiring.** The comment at the call
  (`crates/inkvec-cli/src/lib.rs:754-761`) says the configuration is the identity on the
  default path. It used to name a test, `content_units_changes_the_fit_cost`, as pinning
  that; no test of that name was ever in the workspace, so nothing visible checks either that
  `--content-units` now scales lambda in the emitted trace or that the default path is
  unchanged by building through `fit_config`.
- **`--content-units`' trade is unmeasured since the scale changed.** The help text used to
  warn of "5-px squares fitted as circles, thin rings broken" (`args.rs:396-399`), while
  `content_scale`'s doc comment says those failures were the over-correction of the old
  `extent / REF_EXTENT` scale (`units.rs:41-50`); the help now says only that the flag
  "trades fidelity for parsimony". No measurement of the measured scale against those cases,
  or of the flag's parameter ratio since the change, was found; whether the flag could now be
  on by default is unsettled.
- **`price_in_raster_units`' lambda factor carries no measurement in the comments its doc
  comment points to.** The doc says "the reasons for each factor, and the measurements behind
  them, are in the comments below" (`lib.rs:343-344`); the comments measure the precision
  (`lib.rs:397-403`) and the speckle floor (`lib.rs:405-425`), but not the `× r` on lambda above
  `REF_EXTENT`.
- **`choose_matte`'s `MARGIN`, `DRAWN_FLOOR`, `SOFT_SHARE` and `FLAT_ALPHA`** are each given
  a qualitative role in their surrounding comments but no measured sweep, unlike `SWALLOWED`,
  `LOST_TO_WHITE` and `DRAWN`, which are tied to specific before/after cases (the
  white-highlight, printer-emoji and candle-flame regressions).
- **`pixel_grid`'s `MAX_FACTOR = 32`, `smallest = 64`, and the `1/512` per-channel
  tolerance** are each qualitatively justified but not swept to a specific measured value.
- **`--time-budget`'s `0.6` / `0.25` split and its 50 ms floor** for the boundary solve
  (`crates/inkvec-cli/src/pipeline.rs:140-150`) are stated as a fixed allocation with no
  derivation shown; it is plausible the split was chosen by observing which of the two
  stages tends to dominate runtime, but that reasoning is not recorded where it was read.
- **`DEGRADED_RESIDUAL`'s calibration set is small** — 30 icons in five synthetic conditions
  — and the module's own table shows `clean` and `blur-1.0` overlapping in range even at the
  chosen threshold. The doc comment is honest about this ("no threshold separates every
  condition"), but the generalisation of `0.5` beyond this specific 30-icon set was not
  independently verified here.
