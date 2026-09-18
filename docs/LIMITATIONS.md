# Known Limitations & Honest Technical Boundaries

**Project:** Inkvec / LogoLabs (`logolabs/inkvec`)  
**Scope:** Core Deterministic Vectorizer, Neural Modules & CLI Pipeline  
**Last Updated:** September 2026  

---

## 1. Executive Summary

Every software system is built around explicit design assumptions. Inkvec is engineered to produce **publication-grade, exact, mathematically compact vector graphics from raster artwork** (logos, icons, technical diagrams, emoji, and graphic illustrations).

To achieve sub-pixel boundary fidelity ($< 0.05\text{px}$) and an exact planar topology with no seams and no overdraw in the internal representation ($1.000\times$), Inkvec relies on physical and information-theoretic models. When inputs violate those underlying assumptions, the system degrades predictably.

This document outlines Inkvec's known limitations, failure modes, performance trade-offs, and out-of-scope use cases with complete engineering transparency.

---

## 2. Core Geometric & Topological Boundaries

### 2.1 Continuous-Tone Photographs
- **Assumption:** The image is composed of a finite set of discrete inks (colors), flat regions, and smooth linear or radial gradients separated by well-defined boundaries.
- **Limitation:** Natural photographic images (portraits, landscapes, clouds, foliage) possess millions of continuous color transitions, sensor noise, and textured grain.
- **Failure Mode:** Attempting to trace a photograph forces the Minimum Description Length (MDL) palette extractor to allocate dozens of inks or group natural transitions into posterized, stepped contour bands. Tracing times escalate significantly, and the resulting SVG will be bloated (often tens of thousands of coordinates) and visually inferior to modern raster formats (AVIF/WebP) or specialized diffusion curve renderers.
- **Guidance:** **Do not use Inkvec for photographic images.**

### 2.2 Text, Typography & OCR
- **Assumption:** Inkvec treats all raster pixels as generic geometric shapes.
- **Limitation:** Inkvec does **not** include an Optical Character Recognition (OCR) engine, font identification model, or text-layout parser.
- **Failure Mode:** Lettering, wordmarks, and body text are vectorized purely as raw Bézier path outlines (`<path d="...">`). It does **not** emit SVG `<text>` elements, cannot reflow text, does not preserve font semantics or kerning tables, and cannot fix distorted or low-resolution character shapes beyond their measured pixel coverage.
- **Guidance:** For rasterized text documents, run a dedicated OCR engine (such as Tesseract) to recover textual semantics.

### 2.3 Variable-Width & Sketch Line Art
- **Assumption:** Inkvec's `--strokes` engine recovers centerlines via medial-axis analysis on paths with approximately uniform stroke width.
- **Limitation:** Freehand pencil sketches, cross-hatching, variable-pressure stylus drawings, and calligraphic brush strokes do not have constant stroke thickness.
- **Failure Mode:** The stroke detector will decline centerline mode and fall back to dual-boundary filled outlines (tracing the left and right contours of each pencil stroke as filled loops). While geometrically faithful to the pixels, the resulting SVG paths cannot be manipulated with a single stroke-width slider in vector editors like Figma or Illustrator.

### 2.4 Sub-Pixel Gap Merging & Optical Blurring
- **Assumption:** Anti-aliasing between two distinct shapes provides enough separable signal to invert the point spread function (PSF).
- **Limitation:** If two adjacent strokes or shapes are separated by less than $\approx 0.8$ to $1.0$ pixels in the raster, their anti-aliasing ramps overlap completely in optical RGB space.
- **Failure Mode:** The linear unmixing pass cannot distinguish a sub-pixel background gap from an intermediate blend color. As a result, the two shapes will either be topologically merged into a single connected component, or linked by a spurious diagonal saddle connection.
- **Guidance:** For tiny graphics with ultra-fine line work, utilize the `--sr on` super-resolution pre-pass to synthesize sub-pixel separation before contouring.

### 2.5 Occlusion & Hidden Layer Inpainting (Amodal Vectorization)
- **Assumption:** Inkvec traces visible surface boundaries that project light onto the camera / sensor grid.
- **Limitation:** Inkvec generates a 2D planar partition (DCEL) or recovers layer ordering strictly through boundary containment and alpha-channel heuristics (`--layers`).
- **Failure Mode:** Inkvec **cannot guess or complete occluded geometry**. If a red circle sits partially behind a blue square, Inkvec vectorizes the visible circular crescent and the square; it does not hallucinate the hidden backside of the circle into a complete 360° disc.

---

## 3. Color, Shading & Filter Limitations

### 3.1 Exotic Gradients (Conical, Mesh, Freeform)
- **Supported:** Flat fills, 2-stop / multi-stop linear gradients (`<linearGradient>`), and concentric radial gradients (`<radialGradient>`).
- **Limitation:** Freeform gradient meshes, conical (angular/sweep) gradients, and multi-point bilinear color patches are not supported by standard SVG 1.1 primitives or Inkvec's analytical fitting models.
- **Failure Mode:** Complex non-linear shading is segmented and quantised into stepped discrete palette bands under the MDL description-length cost.

### 3.2 Complex Filter Effects (Drop Shadows, Glows, Blurs)
- **Limitation:** Inkvec does not synthesize SVG filter graphs (`<filter>`, `<feGaussianBlur>`, `<feDropShadow>`).
- **Failure Mode:** Soft Gaussian drop shadows and blurred halos are treated as physical color gradients or banded fills rather than procedural raster filters.

---

## 4. Performance & Resource Trade-offs

### 4.1 Processing Throughput (Speed vs. Quality)
- **Trade-off:** Inkvec prioritizes maximum geometric fidelity, exact topology, and minimal coordinate count over raw throughput.
- **Benchmarked Timing:** Tracing an icon or logo typically takes **1.0 to 1.8 seconds** on a modern multi-core x86_64 or Apple Silicon CPU. In contrast, polygon-based tracers like Potrace execute in $< 0.01\text{s}$, and VTracer executes in $\approx 0.04\text{s}$.
- **Limitation:** Inkvec is **not suitable for real-time applications** (e.g., 60 FPS interactive screen tracing or live video vectorization). It is engineered as an offline asset preparation tool and CI-grade build artifact generator.

### 4.2 Large Image Memory Footprint
- **Scaling:** The Doubly Connected Edge List (DCEL), per-pixel label arrays, coverage gradient fields, and dynamic programming memory matrices scale linearly with pixel dimensions $\mathcal{O}(W \times H)$.
- **Limitation:** Processing ultra-high-resolution images (e.g., 8,000 × 8,000 pixels or larger) requires several gigabytes of RAM during the global nonlinear conjugate gradient boundary solve and DP curve fitting.
- **Mitigation:** Inkvec's intake normalizer automatically detects oversampled artwork and downsamples to $\le 8.0\times$ detail resolution unless overridden.

---

## 5. Neural & AI Runtime Model Limitations

### 5.1 Restorer Deblocking (`restorer.onnx`)
- **Inductive Bias:** The denoiser was trained on synthetic degradation kernels representing discrete cosine transform (DCT) artifacts, WebP compression, and VAE decode haze.
- **Limitation:** It is an image-to-image convolutional regression model. When presented with intentional high-frequency patterns—such as fine halftone dots, comic-book stippling, dithering, or paper textures—it may misclassify them as compression noise and smooth them out.
- **Mitigation:** The restorer is disabled by default (`--restore off`) and must be explicitly enabled when processing damaged assets.

### 5.2 Geometric Graph Neural Network (`TraceRefineGNN20M` — Research Stage)
- **Inductive Bias:** The experimental GNN post-processor predicts continuous coordinate offsets ($\Delta \mathbf{x}$) and CAD regularity constraints (parallelism, perpendicularity, tangency) based on learned geometric priors.
- **Limitation:** On organic, hand-lettered, or deliberately imperfect artistic vector art, the model's CAD snapping prior may over-regularize subtle hand-drawn curves into rigid geometric primitives.
- **Status:** Maintained as an opt-in research pipeline in Python tooling; not bundled in the standalone deterministic Rust CLI.

---

## 6. Summary Matrix

| Capability / Scenario | Support Level | Expected Behavior / Failure Mode |
|---|:---:|---|
| Clean Vector Logos & Icons | ⭐⭐⭐⭐⭐ **Native** | Optimal; $< 0.05\text{px}$ boundary fit, exact planar map (boundaries stored once, no internal overdraw). |
| Linear & Radial Gradients | ⭐⭐⭐⭐⭐ **Native** | Merged across bands; emitted as native SVG gradients. |
| Flat Line Art (Uniform Width) | ⭐⭐⭐⭐ **High** | Recovered as single-path centerlines via `--strokes`. |
| Lossy JPEGs / WebP | ⭐⭐⭐⭐ **High** | Requires `--restore on` for deblocking; high noise guard. |
| Tiny Favicons (16px to 48px) | ⭐⭐⭐ **Moderate** | Requires `--sr on` (MambaIR) to prevent gap collapse. |
| Variable-Width Calligraphy | ⭐⭐ **Partial** | Emitted as dual-boundary filled outlines, not centerlines. |
| Complex Drop Shadows / Blurs | ⭐⭐ **Partial** | Quantised into stepped color bands; no `<filter>` tags. |
| Continuous-Tone Photos | ❌ **Unsupported** | Severe over-quantization, high coordinate count, slow. |
| Text OCR / Font Matching | ❌ **Unsupported** | Emitted purely as `<path>` outlines; no `<text>` tags. |
| Occluded Shape Inpainting | ❌ **Unsupported** | Visible surfaces only; no amodal shape reconstruction. |
| Real-Time / 60 FPS Video | ❌ **Unsupported** | Heavy optimization passes; $\approx 1.2\text{s}$ per graphic. |
