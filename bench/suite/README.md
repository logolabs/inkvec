# Benchmark suite

One command, three vectorisers, two families of metric, and a degradation battery.

```sh
python bench/suite/run.py --split screen --limit 40                  # quality, clean
python bench/suite/run.py --split full --engines inkvec vtracer      # the settling run
python bench/suite/run.py --degrade clean jpeg-q50 blur-1.0 noise-2  # robustness
python bench/suite/run.py --svgenius <path-to-svgenius-checkout> --tier hard # against the literature
```

Writes `bench/data/suite_<label>.json` and prints the table below.

## What it measures, and why

**Research-standard**, so a number here can sit beside a published one: `MSE`, `SSIM`,
`LPIPS(vgg)`, `DINOScore`. This is the set SVG-Bench established and that SVGenius and
VectorArk report. When they disagree, trust DINOScore: pixel metrics on vector art are
dominated by flat background and punish a sub-pixel offset far more than a viewer does,
which is the reason SVG-Bench introduced it.

**Production**, which no paper reports and every deployment needs: `dE00` (CIEDE2000
against the artist's own file, stricter than any of the above), `params` against the
artist's parameter count, output `KB`, trace time as `p50`/`p95`/`max` and a count over the
five-second budget, and a `fail` column. A tracer with a latency budget is judged by its
tail, and an engine that crashes on a tenth of the corpus must not look like one that
merely traced it badly.

## Engines

| engine | status | note |
|---|---|---|
| inkvec | runs | this project |
| VTracer | runs | the open-source colour tracer, at the config this project has always compared against |
| Potrace | runs | the classical baseline, bilevel by construction, so it is only asked about two-colour families |
| Vectorizer.AI | opt-in only | charges per image and uploads the corpus; never part of a default run |
| Vector Magic, Adobe Image Trace | not automatable here | |
| StarVector, VectorArk | weights unreleased | quoted in `published.py` with citations, never as a row of our own table |

## Results, 40 icons of the screen split, judged at 512 px

| input | engine | dE00 | LPIPS | DINO | SSIM | params | KB | p95 s |
|---|---|---|---|---|---|---|---|---|
| clean | **inkvec** | **0.132** | **0.0045** | **0.984** | **0.994** | **3.29** | 1.0 | 0.93 |
| clean | vtracer | 3.244 | 0.0620 | 0.896 | 0.894 | 23.27 | 3.9 | 0.00 |
| clean | potrace | 0.954 | 0.0308 | 0.890 | 0.954 | 7.95 | 1.7 | 0.04 |
| jpeg-q50 | inkvec | **0.464** | 0.0504 | 0.711 | 0.950 | 35.92 | 13.3 | 0.81 |
| jpeg-q50 | vtracer | 2.526 | 0.1009 | 0.646 | 0.774 | 158.81 | 27.5 | 0.01 |
| jpeg-q50 | potrace | 0.931 | **0.0313** | **0.887** | **0.954** | **7.87** | 1.7 | 0.04 |
| blur-1.0 | inkvec | 1.278 | 0.1209 | 0.707 | 0.910 | 49.06 | 11.3 | 1.17 |
| blur-1.0 | vtracer | 4.088 | 0.1114 | 0.706 | 0.695 | 83.72 | 13.5 | 0.01 |
| blur-1.0 | potrace | **1.057** | **0.0370** | **0.861** | **0.950** | **7.94** | 1.7 | 0.04 |
| noise-2 | inkvec | **0.882** | 0.0494 | 0.702 | 0.956 | 36.53 | 10.5 | 0.84 |
| noise-2 | vtracer | 2.820 | 0.1312 | 0.591 | 0.814 | 342.77 | 51.3 | 0.03 |
| noise-2 | potrace | 0.948 | **0.0307** | **0.889** | 0.954 | **7.90** | 1.7 | 0.04 |

No engine failed on any of the 480 traces.

**Read the clean row and the degraded rows as two different verdicts.** On clean vector art
inkvec wins every column: twenty-five times lower colour error than VTracer at a seventh of
the parameters. Under degradation it stays ahead of VTracer, whose parameter count reaches
343x the artist on noise, but **Potrace beats it** on the perceptual metrics — LPIPS 0.031
against 0.121 under blur, DINO 0.887 against 0.711 under JPEG — and does so while its own
numbers barely move at all.

That is not an accident and it is worth stating plainly: Potrace thresholds to two levels
before it does anything else, which is a hard denoiser, and it has no colour model to
shatter. Our pipeline tries to preserve everything it is given, so it preserves the damage
too, and pays in parameters: 3.29x the artist on clean input becomes 35.9x through JPEG.

## Adding an engine

Write a function returning `Result(svg, seconds)` in `engines.py` and register it in
`available()`. Add it to `BILEVEL_ONLY` if it cannot represent colour, and it will be
scored only on two-colour families rather than being handed a question it cannot answer.
