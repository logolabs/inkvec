"""Generate branded SVG infographics for the Inkvec pipeline documentation.
Adheres strictly to the LogoLabs Design System:
- Palette: #0c0a09 (void), #141210 (abyss), #1a1816 (primary), #211f1c (elevated card), #2a2724 (surface), #35322e (highlight)
- Text: #faf8f5 (primary warm cream), rgba(250, 248, 245, 0.75) (secondary), rgba(250, 248, 245, 0.45) (muted)
- Accents: #c9754a (copper brand accent), #d4896a (hover), #b8976c (gold)
- Typography: Playfair Display (display serif), Inter (body sans), JetBrains Mono (monospace)
"""
import os
import xml.etree.ElementTree as ET

os.makedirs("docs/assets", exist_ok=True)

# -----------------------------------------------------------------------------
# GRAPHIC 1: Pipeline Architecture & 13-Stage Step-by-Step Overview
# -----------------------------------------------------------------------------
svg1 = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1400 1020" width="1400" height="1020" style="background:#0c0a09; font-family:'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;">
  <defs>
    <style>
      @import url('https://fonts.googleapis.com/css2?family=Playfair+Display:ital,wght@0,600;0,700;1,400&amp;family=Inter:wght@300;400;500;600;700&amp;family=JetBrains+Mono:wght@400;500;600&amp;display=swap');
      .font-display { font-family: 'Playfair Display', Georgia, serif; }
      .font-mono { font-family: 'JetBrains Mono', monospace; }
      .font-sans { font-family: 'Inter', sans-serif; }
    </style>
    <linearGradient id="copperGrad" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="#d4896a"/>
      <stop offset="100%" stop-color="#c9754a"/>
    </linearGradient>
    <linearGradient id="goldGrad" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="#d8ba8e"/>
      <stop offset="100%" stop-color="#b8976c"/>
    </linearGradient>
    <linearGradient id="cardGrad" x1="0%" y1="0%" x2="0%" y2="100%">
      <stop offset="0%" stop-color="#211f1c"/>
      <stop offset="100%" stop-color="#181614"/>
    </linearGradient>
    <filter id="subtleGlow" x="-10%" y="-10%" width="120%" height="120%">
      <feDropShadow dx="0" dy="4" stdDeviation="8" flood-color="#000000" flood-opacity="0.5"/>
    </filter>
  </defs>

  <!-- Background Canvas -->
  <rect width="1400" height="1020" fill="#0c0a09"/>
  <!-- Subtle Grid lines -->
  <g opacity="0.04" stroke="#faf8f5" stroke-width="1">
    <line x1="0" y1="120" x2="1400" y2="120"/>
    <line x1="0" y1="340" x2="1400" y2="340"/>
    <line x1="0" y1="560" x2="1400" y2="560"/>
    <line x1="0" y1="780" x2="1400" y2="780"/>
    <line x1="350" y1="0" x2="350" y2="1020"/>
    <line x1="700" y1="0" x2="700" y2="1020"/>
    <line x1="1050" y1="0" x2="1050" y2="1020"/>
  </g>

  <!-- Header Section -->
  <g transform="translate(60, 48)">
    <!-- Brand Emblem Lockup -->
    <g transform="translate(0, 4)">
      <rect x="0" y="0" width="36" height="36" rx="8" fill="#211f1c" stroke="#35322e" stroke-width="1"/>
      <path d="M10 26 L18 10 L26 26 Z" fill="none" stroke="#c9754a" stroke-width="2.5" stroke-linejoin="round"/>
      <circle cx="18" cy="20" r="2.5" fill="#b8976c"/>
    </g>
    <text x="50" y="20" class="font-sans" font-size="12" font-weight="600" letter-spacing="0.18em" fill="#c9754a">LOGOLABS · RESEARCH &amp; ARCHITECTURE</text>
    <text x="50" y="44" class="font-display" font-size="28" font-weight="700" fill="#faf8f5" letter-spacing="-0.01em">Inkvec: Exact End-to-End Tracing Pipeline</text>
    
    <!-- Header Right: The Core Objective Equation -->
    <g transform="translate(760, 0)">
      <rect x="0" y="-4" width="520" height="52" rx="8" fill="#141210" stroke="#35322e" stroke-width="1"/>
      <text x="20" y="18" class="font-sans" font-size="10" font-weight="600" fill="#b8976c" letter-spacing="0.14em">GOVERNING MDL OBJECTIVE</text>
      <text x="20" y="38" class="font-mono" font-size="13" font-weight="500" fill="#faf8f5">min <tspan fill="#d4896a">D*</tspan> = 0.5·<tspan fill="#34d399">χ²</tspan>(pixels) + <tspan fill="#b8976c">λ</tspan>·<tspan fill="#d4896a">K_params</tspan> <tspan font-size="11" fill="rgba(250,248,245,0.5)">[λ = ln(extent/precision)]</tspan></text>
    </g>
  </g>

  <!-- Flow Connector Bar -->
  <path d="M 60 125 L 1340 125" stroke="#2a2724" stroke-width="2" stroke-dasharray="4 4"/>

  <!-- COLUMN 1: Intake & Physical Coverage (Stages 01–02) -->
  <g transform="translate(60, 145)">
    <!-- Column Header -->
    <rect x="0" y="0" width="300" height="32" rx="6" fill="#1a1816" stroke="#35322e"/>
    <circle cx="16" cy="16" r="4" fill="#c9754a"/>
    <text x="28" y="20" class="font-sans" font-size="11" font-weight="700" letter-spacing="0.12em" fill="#faf8f5">PHASE I: IMAGE FORMATION</text>

    <!-- Stage 01 Card -->
    <g transform="translate(0, 44)" filter="url(#subtleGlow)">
      <rect x="0" y="0" width="300" height="195" rx="8" fill="url(#cardGrad)" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="300" height="3" rx="1.5" fill="#c9754a"/>
      <text x="16" y="24" class="font-mono" font-size="11" font-weight="600" fill="#c9754a">STAGE 01</text>
      <text x="16" y="44" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">Intake &amp; Pre-Pass</text>
      <text x="16" y="66" class="font-sans" font-size="12" fill="rgba(250,248,245,0.7)" line-height="1.4">
        <tspan x="16" dy="0">• Container inspect: JPEG/WebP lossy guard</tspan>
        <tspan x="16" dy="18">• Unblock: gcd run-length upscale detector</tspan>
        <tspan x="16" dy="18">• MambaIRv2: state-space artifact denoiser</tspan>
        <tspan x="16" dy="18">• Continuous 2D area box downsampler</tspan>
        <tspan x="16" dy="18">• Resolution invariance: scale min_area ∝ s²</tspan>
      </text>
      <rect x="16" y="160" width="268" height="24" rx="4" fill="#141210" stroke="#2a2724"/>
      <text x="24" y="176" class="font-mono" font-size="10" fill="#b8976c">out: Rgba straight f32 [0,1]</text>
    </g>

    <!-- Arrow down -->
    <path d="M 150 245 L 150 262" stroke="#c9754a" stroke-width="1.5" marker-end="url(#arrow)"/>

    <!-- Stage 02 Card -->
    <g transform="translate(0, 265)" filter="url(#subtleGlow)">
      <rect x="0" y="0" width="300" height="225" rx="8" fill="url(#cardGrad)" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="300" height="3" rx="1.5" fill="#c9754a"/>
      <text x="16" y="24" class="font-mono" font-size="11" font-weight="600" fill="#c9754a">STAGE 02</text>
      <text x="16" y="44" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">Coverage &amp; Uncertainty</text>
      <text x="16" y="66" class="font-sans" font-size="12" fill="rgba(250,248,245,0.7)">
        <tspan x="16" dy="0">• Physical unmixing: P = α·F + (1-α)·B</tspan>
        <tspan x="16" dy="18">• Inversion: α = (P-B)·(F-B) / |F-B|²</tspan>
        <tspan x="16" dy="18">• Noise propagation: σα = σ_pixel / |F-B|</tspan>
        <tspan x="16" dy="18">• Spatial uncertainty: σ_pos = σα / |∇α|</tspan>
        <tspan x="16" dy="18">• Discretization floor: σ ≥ 1/√12 ≈ 0.289px</tspan>
      </text>
      <rect x="16" y="172" width="268" height="42" rx="4" fill="#141210" stroke="#2a2724"/>
      <text x="24" y="188" class="font-mono" font-size="10" fill="#34d399">σ = σ_pixel / (|F-B| · |∇α|)</text>
      <text x="24" y="204" class="font-mono" font-size="9" fill="rgba(250,248,245,0.5)">Faint edge → large σ → adaptive simplify</text>
    </g>

    <!-- Callout Box -->
    <g transform="translate(0, 505)">
      <rect x="0" y="0" width="300" height="90" rx="8" fill="#141210" stroke="#c9754a" stroke-dasharray="3 3"/>
      <text x="14" y="22" class="font-sans" font-size="10" font-weight="700" fill="#c9754a" letter-spacing="0.1em">KEY BREAKTHROUGH §1</text>
      <text x="14" y="40" class="font-sans" font-size="11" fill="#faf8f5" font-weight="500">AA is not blur — it is area measurement.</text>
      <text x="14" y="58" class="font-sans" font-size="10" fill="rgba(250,248,245,0.65)">Propagates true posterior variance directly to downstream DP curve fitters.</text>
    </g>
  </g>

  <!-- COLUMN 2: Topology & Color (Stages 03–06) -->
  <g transform="translate(390, 145)">
    <!-- Column Header -->
    <rect x="0" y="0" width="300" height="32" rx="6" fill="#1a1816" stroke="#35322e"/>
    <circle cx="16" cy="16" r="4" fill="#b8976c"/>
    <text x="28" y="20" class="font-sans" font-size="11" font-weight="700" letter-spacing="0.12em" fill="#faf8f5">PHASE II: TOPOLOGY &amp; INKS</text>

    <!-- Stage 03 Card -->
    <g transform="translate(0, 44)" filter="url(#subtleGlow)">
      <rect x="0" y="0" width="300" height="150" rx="8" fill="url(#cardGrad)" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="300" height="3" rx="1.5" fill="#b8976c"/>
      <text x="16" y="24" class="font-mono" font-size="11" font-weight="600" fill="#b8976c">STAGE 03</text>
      <text x="16" y="44" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">MDL Palette in OKLab</text>
      <text x="16" y="66" class="font-sans" font-size="12" fill="rgba(250,248,245,0.7)">
        <tspan x="16" dy="0">• Perceptual clustering in OKLab (Ottosson)</tspan>
        <tspan x="16" dy="18">• ΔE00 perceptual distance floor</tspan>
        <tspan x="16" dy="18">• MDL accept test: cost = 0.5·χ² + λ_bic·K</tspan>
        <tspan x="16" dy="18">• Eliminates spurious gradient bands</tspan>
      </text>
      <rect x="16" y="118" width="268" height="22" rx="4" fill="#141210" stroke="#2a2724"/>
      <text x="24" y="133" class="font-mono" font-size="10" fill="#b8976c">out: Palette { colors, rgb, weight }</text>
    </g>

    <!-- Stage 04 & 05 Combined Compact Card -->
    <g transform="translate(0, 204)" filter="url(#subtleGlow)">
      <rect x="0" y="0" width="300" height="160" rx="8" fill="url(#cardGrad)" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="300" height="3" rx="1.5" fill="#b8976c"/>
      <text x="16" y="22" class="font-mono" font-size="11" font-weight="600" fill="#b8976c">STAGES 04 &amp; 05</text>
      <text x="16" y="40" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">Regions &amp; Gradient Fills</text>
      <text x="16" y="62" class="font-sans" font-size="12" fill="rgba(250,248,245,0.7)">
        <tspan x="16" dy="0">• Blend sliver absorption at AA boundaries</tspan>
        <tspan x="16" dy="18">• Saddle disambiguation (4-pixel diagonal)</tspan>
        <tspan x="16" dy="18">• Fill model selection: Flat / Linear / Radial</tspan>
        <tspan x="16" dy="18">• Du et al. / Chakraborty et al. gradient fits</tspan>
      </text>
      <rect x="16" y="128" width="268" height="22" rx="4" fill="#141210" stroke="#2a2724"/>
      <text x="24" y="143" class="font-mono" font-size="10" fill="#b8976c">out: Vec&lt;u16&gt; connected labels + Fills</text>
    </g>

    <!-- Stage 06 Planar Map Card -->
    <g transform="translate(0, 374)" filter="url(#subtleGlow)">
      <rect x="0" y="0" width="300" height="220" rx="8" fill="url(#cardGrad)" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="300" height="3" rx="1.5" fill="#b8976c"/>
      <text x="16" y="24" class="font-mono" font-size="11" font-weight="600" fill="#b8976c">STAGE 06</text>
      <text x="16" y="44" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">Planar Map (Half-Edge DCEL)</text>
      <text x="16" y="66" class="font-sans" font-size="12" fill="rgba(250,248,245,0.7)">
        <tspan x="16" dy="0">• Dual grid segment extraction from labels</tspan>
        <tspan x="16" dy="18">• Shared edge between exactly two faces</tspan>
        <tspan x="16" dy="18">• Shewchuk exact adaptive predicates</tspan>
        <tspan x="16" dy="18">• Overdraw = 1.000 (0 seams by invariant)</tspan>
      </text>
      <rect x="16" y="150" width="268" height="58" rx="4" fill="#141210" stroke="#2a2724"/>
      <text x="24" y="168" class="font-mono" font-size="10" fill="#34d399">struct Edge { pts, sigma, left, right }</text>
      <text x="24" y="186" class="font-mono" font-size="9" fill="rgba(250,248,245,0.5)">Shewchuk exact orient2d / incircle</text>
      <text x="24" y="200" class="font-mono" font-size="9" fill="#c9754a">Seams unrepresentable</text>
    </g>
  </g>

  <!-- COLUMN 3: Subpixel Optimization (Stages 07–10) -->
  <g transform="translate(720, 145)">
    <!-- Column Header -->
    <rect x="0" y="0" width="300" height="32" rx="6" fill="#1a1816" stroke="#35322e"/>
    <circle cx="16" cy="16" r="4" fill="#34d399"/>
    <text x="28" y="20" class="font-sans" font-size="11" font-weight="700" letter-spacing="0.12em" fill="#faf8f5">PHASE III: SPATIAL SOLVE</text>

    <!-- Stage 07 & 09 Card -->
    <g transform="translate(0, 44)" filter="url(#subtleGlow)">
      <rect x="0" y="0" width="300" height="150" rx="8" fill="url(#cardGrad)" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="300" height="3" rx="1.5" fill="#34d399"/>
      <text x="16" y="24" class="font-mono" font-size="11" font-weight="600" fill="#34d399">STAGES 07 &amp; 09</text>
      <text x="16" y="44" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">Subpixel Normals &amp; Decode</text>
      <text x="16" y="66" class="font-sans" font-size="12" fill="rgba(250,248,245,0.7)">
        <tspan x="16" dy="0">• Slide boundary points along local normal</tspan>
        <tspan x="16" dy="18">• Level set crossing α = 0.5</tspan>
        <tspan x="16" dy="18">• Junction settlement (triple/Y-junctions)</tspan>
        <tspan x="16" dy="18">• Order-first decode: recover &lt;1px features</tspan>
      </text>
      <rect x="16" y="118" width="268" height="22" rx="4" fill="#141210" stroke="#2a2724"/>
      <text x="24" y="133" class="font-mono" font-size="10" fill="#34d399">Points centered at 0.5 coverage</text>
    </g>

    <!-- Stage 08 Global Boundary Solve -->
    <g transform="translate(0, 204)" filter="url(#subtleGlow)">
      <rect x="0" y="0" width="300" height="220" rx="8" fill="url(#cardGrad)" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="300" height="3" rx="1.5" fill="#34d399"/>
      <text x="16" y="24" class="font-mono" font-size="11" font-weight="600" fill="#34d399">STAGE 08</text>
      <text x="16" y="44" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">Analysis-by-Synthesis Solve</text>
      <text x="16" y="66" class="font-sans" font-size="12" fill="rgba(250,248,245,0.7)">
        <tspan x="16" dy="0">• Move ALL boundary points simultaneously</tspan>
        <tspan x="16" dy="18">• Exact clipped polygon pixel area rendering</tspan>
        <tspan x="16" dy="18">• Shoelace formula: A = 0.5·Σ (x_i·y_i+1 - ...)</tspan>
        <tspan x="16" dy="18">• Analytic Jacobian via Prov tracking enum</tspan>
        <tspan x="16" dy="18">• Levenberg-Marquardt energy minimization</tspan>
      </text>
      <rect x="16" y="160" width="268" height="48" rx="4" fill="#141210" stroke="#2a2724"/>
      <text x="24" y="178" class="font-mono" font-size="10" fill="#34d399">E = ||a·c_L + (1-a)·c_R - target||²</text>
      <text x="24" y="196" class="font-mono" font-size="9" fill="rgba(250,248,245,0.5)">+ w_kink·|Δ²p| + w_anchor·|p - p₀|²</text>
    </g>

    <!-- Stage 10 Symmetry -->
    <g transform="translate(0, 434)" filter="url(#subtleGlow)">
      <rect x="0" y="0" width="300" height="160" rx="8" fill="url(#cardGrad)" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="300" height="3" rx="1.5" fill="#34d399"/>
      <text x="16" y="24" class="font-mono" font-size="11" font-weight="600" fill="#34d399">STAGE 10</text>
      <text x="16" y="44" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">Symmetry Group Discovery</text>
      <text x="16" y="66" class="font-sans" font-size="12" fill="rgba(250,248,245,0.7)">
        <tspan x="16" dy="0">• Reflection axes &amp; k-fold rotational groups</tspan>
        <tspan x="16" dy="18">• Detected on exact integer lattice</tspan>
        <tspan x="16" dy="18">• Enforced as geometric equality constraint</tspan>
        <tspan x="16" dy="18">• Emits &lt;defs&gt; and &lt;use&gt; transforms</tspan>
      </text>
      <rect x="16" y="124" width="268" height="22" rx="4" fill="#141210" stroke="#2a2724"/>
      <text x="24" y="139" class="font-mono" font-size="10" fill="#34d399">Enforces exact mathematical mirror/rot</text>
    </g>
  </g>

  <!-- COLUMN 4: Curve Fitting, Repair & Emit (Stages 11–13) -->
  <g transform="translate(1050, 145)">
    <!-- Column Header -->
    <rect x="0" y="0" width="290" height="32" rx="6" fill="#1a1816" stroke="#35322e"/>
    <circle cx="16" cy="16" r="4" fill="#d4896a"/>
    <text x="28" y="20" class="font-sans" font-size="11" font-weight="700" letter-spacing="0.12em" fill="#faf8f5">PHASE IV: FITTING &amp; EMIT</text>

    <!-- Stage 11 Dynamic Programming Curve Fit -->
    <g transform="translate(0, 44)" filter="url(#subtleGlow)">
      <rect x="0" y="0" width="290" height="255" rx="8" fill="url(#cardGrad)" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="290" height="3" rx="1.5" fill="#d4896a"/>
      <text x="16" y="24" class="font-mono" font-size="11" font-weight="600" fill="#d4896a">STAGE 11</text>
      <text x="16" y="44" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">Multi-Model DP Curve Fit</text>
      <text x="16" y="66" class="font-sans" font-size="12" fill="rgba(250,248,245,0.7)">
        <tspan x="16" dy="0">• Generalized Potrace DP over full alphabet:</tspan>
        <tspan x="26" dy="16">- Line (2 params), Axis line (1 param)</tspan>
        <tspan x="26" dy="16">- Circular arc (5 params via Kåsa O(1))</tspan>
        <tspan x="26" dy="16">- G1 Cubic (6) via Raph Levien quartic</tspan>
        <tspan x="26" dy="16">- Primitives: &lt;circle&gt;(3), &lt;rect&gt;(4/6)</tspan>
        <tspan x="16" dy="18">• Ahn Orthogonal Distance (no curvature bias)</tspan>
        <tspan x="16" dy="18">• Bow penalty detects systematic arc drift</tspan>
      </text>
      <rect x="16" y="195" width="258" height="46" rx="4" fill="#141210" stroke="#2a2724"/>
      <text x="24" y="212" class="font-mono" font-size="10" fill="#d4896a">d = σ·√(2·λ·k / n)</text>
      <text x="24" y="228" class="font-mono" font-size="9" fill="rgba(250,248,245,0.5)">Levien moment quartic avoids 3 minima</text>
    </g>

    <!-- Stage 12 Repair -->
    <g transform="translate(0, 309)" filter="url(#subtleGlow)">
      <rect x="0" y="0" width="290" height="135" rx="8" fill="url(#cardGrad)" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="290" height="3" rx="1.5" fill="#d4896a"/>
      <text x="16" y="24" class="font-mono" font-size="11" font-weight="600" fill="#d4896a">STAGE 12</text>
      <text x="16" y="44" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">Topological Ring Repair</text>
      <text x="16" y="66" class="font-sans" font-size="12" fill="rgba(250,248,245,0.7)">
        <tspan x="16" dy="0">• Assembled ring self-intersection test</tspan>
        <tspan x="16" dy="18">• Prevents even-odd fill parity inversion</tspan>
        <tspan x="16" dy="18">• Tightens span cap: max_span → 1</tspan>
        <tspan x="16" dy="18">• Guaranteed termination to simple boundary</tspan>
      </text>
    </g>

    <!-- Stage 13 Emit & Post -->
    <g transform="translate(0, 454)" filter="url(#subtleGlow)">
      <rect x="0" y="0" width="290" height="140" rx="8" fill="url(#cardGrad)" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="290" height="3" rx="1.5" fill="#d4896a"/>
      <text x="16" y="24" class="font-mono" font-size="11" font-weight="600" fill="#d4896a">STAGE 13</text>
      <text x="16" y="44" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">High-Fidelity SVG Emission</text>
      <text x="16" y="66" class="font-sans" font-size="12" fill="rgba(250,248,245,0.7)">
        <tspan x="16" dy="0">• Stacked vs cutout mosaic export</tspan>
        <tspan x="16" dy="18">• Emit coordinate precision (2 decimals)</tspan>
        <tspan x="16" dy="18">• Shared sibling even-odd path merger</tspan>
        <tspan x="16" dy="18">• Minification &amp; viewBox retargeting</tspan>
      </text>
    </g>
  </g>

  <!-- Bottom Comparison Bar / Key Findings Banner -->
  <g transform="translate(60, 775)">
    <rect x="0" y="0" width="1280" height="195" rx="12" fill="#141210" stroke="#35322e" stroke-width="1"/>
    <!-- Banner Header -->
    <text x="30" y="32" class="font-sans" font-size="11" font-weight="700" letter-spacing="0.14em" fill="#c9754a">WHY INKVEC DOMINATES PREVIOUS ENGINES (VTRACER, POTRACE, AUTOTRACE)</text>
    
    <!-- Comparison 3-Column Layout -->
    <g transform="translate(30, 48)">
      <!-- Item 1 -->
      <g transform="translate(0, 0)">
        <rect x="0" y="0" width="380" height="120" rx="8" fill="#1a1816" stroke="#2a2724"/>
        <text x="16" y="26" class="font-sans" font-size="13" font-weight="600" fill="#faf8f5">1. Subpixel Precision vs Bitmaps</text>
        <text x="16" y="48" class="font-sans" font-size="11" fill="rgba(250,248,245,0.7)">
          <tspan x="16" dy="0">Conventional: Thresholds pixels to integer raster grid.</tspan>
          <tspan x="16" dy="18" fill="#c9754a" font-weight="500">Inkvec: Inverts continuous physical coverage mixture,</tspan>
          <tspan x="16" dy="16" fill="#c9754a" font-weight="500">localizing edges to ~0.02 px with honest uncertainty σ.</tspan>
        </text>
      </g>

      <!-- Item 2 -->
      <g transform="translate(410, 0)">
        <rect x="0" y="0" width="380" height="120" rx="8" fill="#1a1816" stroke="#2a2724"/>
        <text x="16" y="26" class="font-sans" font-size="13" font-weight="600" fill="#faf8f5">2. Planar Map DCEL vs Independent Paths</text>
        <text x="16" y="48" class="font-sans" font-size="11" fill="rgba(250,248,245,0.7)">
          <tspan x="16" dy="0">Conventional: 1.64× overdraw or unsightly gaps/seams.</tspan>
          <tspan x="16" dy="18" fill="#b8976c" font-weight="500">Inkvec: Stores shared edge ONCE in planar subdivision.</tspan>
          <tspan x="16" dy="16" fill="#b8976c" font-weight="500">Overdraw = 1.000 by invariant; Shewchuk exact predicates.</tspan>
        </text>
      </g>

      <!-- Item 3 -->
      <g transform="translate(820, 0)">
        <rect x="0" y="0" width="390" height="120" rx="8" fill="#1a1816" stroke="#2a2724"/>
        <text x="16" y="26" class="font-sans" font-size="13" font-weight="600" fill="#faf8f5">3. Multi-Model Alphabet vs Cubic-Only</text>
        <text x="16" y="48" class="font-sans" font-size="11" fill="rgba(250,248,245,0.7)">
          <tspan x="16" dy="0">Conventional: 36 chords or 4 cubics (24 floats) per circle.</tspan>
          <tspan x="16" dy="18" fill="#34d399" font-weight="500">Inkvec: MDL DP selects &lt;circle&gt; (3 floats), arcs (5),</tspan>
          <tspan x="16" dy="16" fill="#34d399" font-weight="500">or Levien quartic G1 cubics. 3.3× fewer coordinates.</tspan>
        </text>
      </g>
    </g>
  </g>

  <!-- Flow Direction Arrows -->
  <defs>
    <marker id="arrow" viewBox="0 0 10 10" refX="5" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
      <path d="M 0 1 L 8 5 L 0 9 z" fill="#c9754a"/>
    </marker>
  </defs>
  <!-- Inter-column connector arrows -->
  <path d="M 360 280 L 390 280" stroke="#c9754a" stroke-width="2" marker-end="url(#arrow)"/>
  <path d="M 690 280 L 720 280" stroke="#b8976c" stroke-width="2" marker-end="url(#arrow)"/>
  <path d="M 1020 280 L 1050 280" stroke="#34d399" stroke-width="2" marker-end="url(#arrow)"/>
</svg>
"""

# -----------------------------------------------------------------------------
# GRAPHIC 2: Subpixel Coverage Mechanics & Uncertainty Propagation
# -----------------------------------------------------------------------------
svg2 = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1200 680" width="1200" height="680" style="background:#0c0a09; font-family:'Inter', sans-serif;">
  <defs>
    <style>
      @import url('https://fonts.googleapis.com/css2?family=Playfair+Display:wght@600;700&amp;family=Inter:wght@400;500;600;700&amp;family=JetBrains+Mono:wght@400;500;600&amp;display=swap');
      .font-display { font-family: 'Playfair Display', Georgia, serif; }
      .font-mono { font-family: 'JetBrains Mono', monospace; }
      .font-sans { font-family: 'Inter', sans-serif; }
    </style>
    <linearGradient id="cardBg" x1="0%" y1="0%" x2="0%" y2="100%">
      <stop offset="0%" stop-color="#1f1d1a"/>
      <stop offset="100%" stop-color="#141210"/>
    </linearGradient>
  </defs>

  <rect width="1200" height="680" fill="#0c0a09"/>
  
  <!-- Header -->
  <g transform="translate(60, 40)">
    <text x="0" y="16" class="font-sans" font-size="11" font-weight="700" letter-spacing="0.16em" fill="#c9754a">MATHEMATICAL FOUNDATION · STAGE 02</text>
    <text x="0" y="44" class="font-display" font-size="26" font-weight="700" fill="#faf8f5">Subpixel Anti-Aliasing Inversion &amp; Uncertainty Propagation</text>
  </g>

  <!-- Panel 1: Physical Continuous Coverage inside a Pixel Grid -->
  <g transform="translate(60, 110)">
    <rect x="0" y="0" width="340" height="510" rx="10" fill="url(#cardBg)" stroke="#35322e" stroke-width="1"/>
    <text x="24" y="32" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">1. Physical Pixel Integration</text>
    <text x="24" y="52" class="font-sans" font-size="12" fill="rgba(250,248,245,0.65)">Continuous vector edge cuts the square sensor cell</text>

    <!-- Visual of pixel cell -->
    <g transform="translate(50, 80)">
      <!-- Pixel Cell Boundary -->
      <rect x="0" y="0" width="240" height="240" fill="#1a1816" stroke="#35322e" stroke-width="2"/>
      <!-- Background Fill (1-alpha) -->
      <polygon points="0,0 240,0 240,160 0,60" fill="#2a2724"/>
      <!-- Foreground Fill alpha -->
      <polygon points="0,60 240,160 240,240 0,240" fill="#c9754a" fill-opacity="0.85"/>
      <!-- True Boundary Vector -->
      <line x1="0" y1="60" x2="240" y2="160" stroke="#faf8f5" stroke-width="3"/>
      <!-- Normal vector -->
      <line x1="120" y1="110" x2="85" y2="194" stroke="#b8976c" stroke-width="2" stroke-dasharray="3 3"/>
      <circle cx="120" cy="110" r="4" fill="#faf8f5"/>
      <text x="130" y="105" class="font-mono" font-size="11" fill="#faf8f5">Edge Level-Set α=0.5</text>
      <text x="100" y="215" class="font-mono" font-size="11" fill="#b8976c">∇α (normal)</text>

      <!-- Label Area -->
      <text x="40" y="40" class="font-mono" font-size="12" fill="#faf8f5">Area = (1 - α)</text>
      <text x="40" y="56" class="font-sans" font-size="10" fill="rgba(250,248,245,0.6)">Background (B)</text>

      <text x="40" y="190" class="font-mono" font-size="12" fill="#faf8f5">Area = α (62%)</text>
      <text x="40" y="206" class="font-sans" font-size="10" fill="rgba(250,248,245,0.85)">Foreground (F)</text>
    </g>

    <!-- Formula block -->
    <g transform="translate(20, 360)">
      <rect x="0" y="0" width="300" height="125" rx="6" fill="#141210" stroke="#2a2724"/>
      <text x="16" y="24" class="font-mono" font-size="12" fill="#d4896a">Forward Image Formation:</text>
      <text x="16" y="46" class="font-mono" font-size="14" fill="#faf8f5">P = α·F + (1 - α)·B</text>
      <text x="16" y="70" class="font-sans" font-size="11" fill="rgba(250,248,245,0.6)">P is the recorded pixel value in RGB space.</text>
      <text x="16" y="88" class="font-sans" font-size="11" fill="rgba(250,248,245,0.6)">α is scalar area fraction [0, 1].</text>
      <text x="16" y="106" class="font-sans" font-size="11" fill="#34d399">Preserves subpixel edge to ~0.02 px.</text>
    </g>
  </g>

  <!-- Panel 2: Vector Projection in 3D Color Space -->
  <g transform="translate(430, 110)">
    <rect x="0" y="0" width="340" height="510" rx="10" fill="url(#cardBg)" stroke="#35322e" stroke-width="1"/>
    <text x="24" y="32" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">2. 3D Color Space Inversion</text>
    <text x="24" y="52" class="font-sans" font-size="12" fill="rgba(250,248,245,0.65)">Least-squares unmixing along (F - B) axis</text>

    <!-- 3D Vector Diagram -->
    <g transform="translate(30, 90)">
      <!-- Axis lines -->
      <line x1="30" y1="200" x2="250" y2="200" stroke="#35322e" stroke-width="1.5"/>
      <line x1="30" y1="200" x2="30" y2="20" stroke="#35322e" stroke-width="1.5"/>
      <line x1="30" y1="200" x2="100" y2="250" stroke="#35322e" stroke-width="1.5"/>
      <text x="255" y="205" class="font-mono" font-size="10" fill="rgba(250,248,245,0.4)">Red</text>
      <text x="25" y="15" class="font-mono" font-size="10" fill="rgba(250,248,245,0.4)">Green</text>
      <text x="105" y="255" class="font-mono" font-size="10" fill="rgba(250,248,245,0.4)">Blue</text>

      <!-- Vector F - B -->
      <line x1="50" y1="170" x2="220" y2="60" stroke="#c9754a" stroke-width="3"/>
      <!-- Background Node B -->
      <circle cx="50" cy="170" r="6" fill="#2a2724" stroke="#faf8f5" stroke-width="2"/>
      <text x="35" y="195" class="font-mono" font-size="12" font-weight="600" fill="#faf8f5">B (α=0)</text>

      <!-- Foreground Node F -->
      <circle cx="220" cy="60" r="6" fill="#c9754a" stroke="#faf8f5" stroke-width="2"/>
      <text x="210" y="45" class="font-mono" font-size="12" font-weight="600" fill="#c9754a">F (α=1)</text>

      <!-- Observed Pixel P with noise projection -->
      <circle cx="150" cy="95" r="5" fill="#34d399"/>
      <text x="160" y="90" class="font-mono" font-size="11" fill="#34d399">P (Observed)</text>
      <!-- Projection line -->
      <line x1="150" y1="95" x2="160" y2="99" stroke="#34d399" stroke-width="1.5" stroke-dasharray="2 2"/>
      <circle cx="160" cy="99" r="4" fill="#faf8f5"/>
      <text x="170" y="115" class="font-mono" font-size="10" fill="#faf8f5">α Projection</text>

      <!-- Vector Difference annotation -->
      <text x="110" y="150" class="font-mono" font-size="11" fill="#b8976c">Vector (F - B)</text>
    </g>

    <!-- Formula block -->
    <g transform="translate(20, 360)">
      <rect x="0" y="0" width="300" height="125" rx="6" fill="#141210" stroke="#2a2724"/>
      <text x="16" y="24" class="font-mono" font-size="12" fill="#d4896a">Least-Squares Projection:</text>
      <text x="16" y="46" class="font-mono" font-size="13" fill="#34d399">α = (P - B) · (F - B) / |F - B|²</text>
      <text x="16" y="70" class="font-sans" font-size="11" fill="rgba(250,248,245,0.6)">Uses all three RGB channels simultaneously,</text>
      <text x="16" y="88" class="font-sans" font-size="11" fill="rgba(250,248,245,0.6)">unlike luminance-only thresholding.</text>
      <text x="16" y="106" class="font-sans" font-size="11" fill="#b8976c">Robust against single-channel noise.</text>
    </g>
  </g>

  <!-- Panel 3: Rigorous Uncertainty Propagation -->
  <g transform="translate(800, 110)">
    <rect x="0" y="0" width="340" height="510" rx="10" fill="url(#cardBg)" stroke="#35322e" stroke-width="1"/>
    <text x="24" y="32" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">3. Error Propagation Chain</text>
    <text x="24" y="52" class="font-sans" font-size="12" fill="rgba(250,248,245,0.65)">Pixel noise → Coverage noise → Positional σ</text>

    <!-- Visual comparison of Sharp vs Faint -->
    <g transform="translate(20, 80)">
      <!-- Sharp Edge Box -->
      <rect x="0" y="0" width="300" height="110" rx="6" fill="#141210" stroke="#34d399" stroke-width="1"/>
      <text x="14" y="22" class="font-sans" font-size="11" font-weight="700" fill="#34d399">SHARP / HIGH CONTRAST (|F-B| large)</text>
      <text x="14" y="42" class="font-mono" font-size="11" fill="#faf8f5">|∇α| ≈ 1.0 px⁻¹  ·  σα ≈ 0.02</text>
      <text x="14" y="62" class="font-mono" font-size="14" font-weight="600" fill="#34d399">σ_pos = 0.028 px (tight tolerance!)</text>
      <text x="14" y="86" class="font-sans" font-size="10" fill="rgba(250,248,245,0.6)">Fitter strictly respects every contour point.</text>

      <!-- Faint Edge Box -->
      <rect x="0" y="130" width="300" height="110" rx="6" fill="#141210" stroke="#e09263" stroke-width="1"/>
      <text x="14" y="152" class="font-sans" font-size="11" font-weight="700" fill="#e09263">FAINT / LOW CONTRAST (|F-B| small)</text>
      <text x="14" y="172" class="font-mono" font-size="11" fill="#faf8f5">|∇α| ≈ 0.2 px⁻¹  ·  σα ≈ 0.25</text>
      <text x="14" y="192" class="font-mono" font-size="14" font-weight="600" fill="#e09263">σ_pos = 1.250 px (wide tolerance!)</text>
      <text x="14" y="216" class="font-sans" font-size="10" fill="rgba(250,248,245,0.6)">Fitter simplifies hard — naturally noise-immune.</text>
    </g>

    <!-- Formula block -->
    <g transform="translate(20, 360)">
      <rect x="0" y="0" width="300" height="125" rx="6" fill="#141210" stroke="#2a2724"/>
      <text x="16" y="24" class="font-mono" font-size="12" fill="#d4896a">Unified Uncertainty Formula:</text>
      <text x="16" y="48" class="font-mono" font-size="13" font-weight="600" fill="#faf8f5">σ_pos = σ_pixel / (|F - B| · |∇α|)</text>
      <text x="16" y="72" class="font-sans" font-size="11" fill="rgba(250,248,245,0.7)">Quantization Variance Floor (Uniform noise):</text>
      <text x="16" y="92" class="font-mono" font-size="12" fill="#b8976c">σ_floor ≥ 1 / √12 ≈ 0.2887 px</text>
      <text x="16" y="110" class="font-sans" font-size="10" fill="#34d399">Prevents over-segmentation on 16-level alpha!</text>
    </g>
  </g>
</svg>
"""

# -----------------------------------------------------------------------------
# GRAPHIC 3: Planar Map DCEL vs Independent Closed Paths
# -----------------------------------------------------------------------------
svg3 = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1200 680" width="1200" height="680" style="background:#0c0a09; font-family:'Inter', sans-serif;">
  <defs>
    <style>
      @import url('https://fonts.googleapis.com/css2?family=Playfair+Display:wght@600;700&amp;family=Inter:wght@400;500;600;700&amp;family=JetBrains+Mono:wght@400;500;600&amp;display=swap');
      .font-display { font-family: 'Playfair Display', Georgia, serif; }
      .font-mono { font-family: 'JetBrains Mono', monospace; }
      .font-sans { font-family: 'Inter', sans-serif; }
    </style>
  </defs>

  <rect width="1200" height="680" fill="#0c0a09"/>

  <!-- Header -->
  <g transform="translate(60, 40)">
    <text x="0" y="16" class="font-sans" font-size="11" font-weight="700" letter-spacing="0.16em" fill="#b8976c">TOPOLOGY &amp; DATA STRUCTURE · STAGE 06</text>
    <text x="0" y="44" class="font-display" font-size="26" font-weight="700" fill="#faf8f5">Planar Map Subdivision vs Conventional Independent Paths</text>
  </g>

  <!-- Left: Conventional Approach -->
  <g transform="translate(60, 110)">
    <rect x="0" y="0" width="510" height="510" rx="10" fill="#141210" stroke="#f87171" stroke-opacity="0.5" stroke-width="1.5"/>
    <text x="24" y="32" class="font-display" font-size="18" font-weight="600" fill="#f87171">Conventional Tracers (VTracer, Potrace, SVG)</text>
    <text x="24" y="54" class="font-sans" font-size="12" fill="rgba(250,248,245,0.65)">Independent closed paths per region: The Seam vs Overdraw Dilemma</text>

    <!-- Failure illustration -->
    <g transform="translate(40, 80)">
      <!-- Shape A -->
      <path d="M 20 20 L 180 20 L 180 180 L 20 180 Z" fill="#2a2724" stroke="#faf8f5" stroke-width="1.5"/>
      <text x="60" y="105" class="font-sans" font-size="14" font-weight="600" fill="#faf8f5">Face A</text>
      <!-- Duplicated boundary with jitter/gap -->
      <path d="M 184 20 L 340 20 L 340 180 L 184 180 Z" fill="#35322e" stroke="#faf8f5" stroke-width="1.5"/>
      <text x="230" y="105" class="font-sans" font-size="14" font-weight="600" fill="#faf8f5">Face B</text>

      <!-- Zoom callout to the seam -->
      <line x1="182" y1="80" x2="182" y2="120" stroke="#f87171" stroke-width="3"/>
      
      <!-- Seam Magnified Circle -->
      <g transform="translate(260, 190)">
        <circle cx="60" cy="60" r="55" fill="#1a1816" stroke="#f87171" stroke-width="2"/>
        <line x1="45" y1="15" x2="45" y2="105" stroke="#faf8f5" stroke-width="3"/>
        <line x1="75" y1="15" x2="75" y2="105" stroke="#faf8f5" stroke-width="3"/>
        <!-- Gap -->
        <rect x="47" y="15" width="26" height="90" fill="#f87171" fill-opacity="0.3"/>
        <text x="18" y="130" class="font-mono" font-size="10" fill="#f87171">Hairline Seam / Gap</text>
      </g>
    </g>

    <!-- Failure metrics box -->
    <g transform="translate(24, 340)">
      <rect x="0" y="0" width="462" height="145" rx="8" fill="#1a1816" stroke="#2a2724"/>
      <text x="18" y="24" class="font-mono" font-size="12" font-weight="600" fill="#f87171">MEASURED BENCHMARK FAILURE:</text>
      <text x="18" y="46" class="font-sans" font-size="12" fill="rgba(250,248,245,0.8)">
        <tspan x="18" dy="0">• <tspan font-weight="700" fill="#faf8f5">Overdraw: 1.64×</tspan> (64% of boundary geometry is drawn twice)</tspan>
        <tspan x="18" dy="20">• Edge rounding errors diverge at 4× zoom: <tspan fill="#f87171">seams widen 2.4×</tspan></tspan>
        <tspan x="18" dy="20">• Moving an edge requires synchronizing two separate bezier paths</tspan>
        <tspan x="18" dy="20">• Under noise perturbation: anchor count explodes by <tspan fill="#f87171">+106.7%</tspan></tspan>
      </text>
    </g>
  </g>

  <!-- Right: Inkvec DCEL Planar Map -->
  <g transform="translate(630, 110)">
    <rect x="0" y="0" width="510" height="510" rx="10" fill="#141210" stroke="#34d399" stroke-opacity="0.7" stroke-width="1.5"/>
    <text x="24" y="32" class="font-display" font-size="18" font-weight="600" fill="#34d399">Inkvec Half-Edge Planar Subdivision (DCEL)</text>
    <text x="24" y="54" class="font-sans" font-size="12" fill="rgba(250,248,245,0.65)">Every shared edge stored exactly once with exact Shewchuk predicates</text>

    <!-- Planar Map Diagram -->
    <g transform="translate(40, 80)">
      <!-- Face L -->
      <polygon points="20,20 180,20 180,180 20,180" fill="#211f1c" stroke="#35322e" stroke-width="1"/>
      <text x="60" y="105" class="font-sans" font-size="14" font-weight="600" fill="#faf8f5">Face L</text>
      <!-- Face R -->
      <polygon points="180,20 340,20 340,180 180,180" fill="#2a2724" stroke="#35322e" stroke-width="1"/>
      <text x="230" y="105" class="font-sans" font-size="14" font-weight="600" fill="#faf8f5">Face R</text>

      <!-- Shared Edge E_k (highlighted) -->
      <line x1="180" y1="20" x2="180" y2="180" stroke="#c9754a" stroke-width="4"/>
      <!-- Nodes / Junctions -->
      <circle cx="180" cy="20" r="6" fill="#34d399" stroke="#faf8f5" stroke-width="2"/>
      <circle cx="180" cy="180" r="6" fill="#34d399" stroke="#faf8f5" stroke-width="2"/>
      <text x="190" y="25" class="font-mono" font-size="11" fill="#34d399">Node v₁</text>
      <text x="190" y="185" class="font-mono" font-size="11" fill="#34d399">Node v₂</text>

      <!-- Direction Arrow on Shared Edge -->
      <path d="M 175 90 L 180 105 L 185 90" fill="#faf8f5"/>
      <text x="100" y="145" class="font-mono" font-size="11" fill="#c9754a">Shared Edge e_k</text>
      <text x="100" y="160" class="font-mono" font-size="9" fill="rgba(250,248,245,0.6)">left: L, right: R</text>
    </g>

    <!-- Mathematical Invariant Box -->
    <g transform="translate(24, 340)">
      <rect x="0" y="0" width="462" height="145" rx="8" fill="#1a1816" stroke="#2a2724"/>
      <text x="18" y="24" class="font-mono" font-size="12" font-weight="600" fill="#34d399">TOPOLOGICAL GUARANTEES (BY INVARIANT):</text>
      <text x="18" y="46" class="font-sans" font-size="12" fill="rgba(250,248,245,0.8)">
        <tspan x="18" dy="0">• <tspan font-weight="700" fill="#faf8f5">Overdraw: 1.000×</tspan> (Exactly zero duplicated geometry)</tspan>
        <tspan x="18" dy="20">• <tspan font-weight="700" fill="#34d399">0 Seams:</tspan> Hairline gaps are unrepresentable in the data structure</tspan>
        <tspan x="18" dy="20">• <tspan font-weight="700" fill="#b8976c">Shewchuk robust predicates:</tspan> orient2d/incircle evaluated exactly</tspan>
        <tspan x="18" dy="20">• Moving edge e_k moves boundary for both faces identically</tspan>
      </text>
    </g>
  </g>
</svg>
"""

# -----------------------------------------------------------------------------
# GRAPHIC 4: Multi-Model Dynamic Programming Curve Fitting & Primitives
# -----------------------------------------------------------------------------
svg4 = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1200 720" width="1200" height="720" style="background:#0c0a09; font-family:'Inter', sans-serif;">
  <defs>
    <style>
      @import url('https://fonts.googleapis.com/css2?family=Playfair+Display:wght@600;700&amp;family=Inter:wght@400;500;600;700&amp;family=JetBrains+Mono:wght@400;500;600&amp;display=swap');
      .font-display { font-family: 'Playfair Display', Georgia, serif; }
      .font-mono { font-family: 'JetBrains Mono', monospace; }
      .font-sans { font-family: 'Inter', sans-serif; }
    </style>
  </defs>

  <rect width="1200" height="720" fill="#0c0a09"/>

  <!-- Header -->
  <g transform="translate(60, 36)">
    <text x="0" y="16" class="font-sans" font-size="11" font-weight="700" letter-spacing="0.16em" fill="#c9754a">ALGORITHM &amp; OPTIMIZATION · STAGE 11</text>
    <text x="0" y="44" class="font-display" font-size="26" font-weight="700" fill="#faf8f5">Multi-Model Dynamic Programming &amp; Primitive Fitting</text>
  </g>

  <!-- Top: Dynamic Programming Graph Representation -->
  <g transform="translate(60, 105)">
    <rect x="0" y="0" width="1080" height="155" rx="10" fill="#141210" stroke="#35322e" stroke-width="1"/>
    <text x="24" y="28" class="font-sans" font-size="12" font-weight="700" fill="#b8976c" letter-spacing="0.12em">THE GLOBAL DYNAMIC PROGRAMMING RECURRENCE</text>
    
    <!-- State and transitions visual -->
    <g transform="translate(40, 50)">
      <!-- Vertices on the polyline -->
      <circle cx="0" cy="50" r="6" fill="#c9754a"/>
      <text x="-8" y="72" class="font-mono" font-size="11" fill="#c9754a">v₀</text>

      <circle cx="160" cy="50" r="6" fill="#faf8f5"/>
      <text x="154" y="72" class="font-mono" font-size="11" fill="#faf8f5">v_i</text>

      <circle cx="360" cy="50" r="6" fill="#faf8f5"/>
      <text x="354" y="72" class="font-mono" font-size="11" fill="#faf8f5">v_j</text>

      <circle cx="560" cy="50" r="6" fill="#34d399"/>
      <text x="552" y="72" class="font-mono" font-size="11" fill="#34d399">v_n</text>

      <!-- Polyline path -->
      <line x1="0" y1="50" x2="160" y2="50" stroke="#35322e" stroke-width="2"/>
      <line x1="160" y1="50" x2="360" y2="50" stroke="#35322e" stroke-width="2"/>
      <line x1="360" y1="50" x2="560" y2="50" stroke="#35322e" stroke-width="2"/>

      <!-- Competing candidate transitions over span (i, j) -->
      <!-- Candidate 1: Line -->
      <path d="M 160 50 Q 260 20 360 50" fill="none" stroke="#faf8f5" stroke-width="1.5" stroke-dasharray="3 3"/>
      <text x="230" y="30" class="font-mono" font-size="10" fill="#faf8f5">Line (2 params)</text>

      <!-- Candidate 2: Circular Arc -->
      <path d="M 160 50 Q 260 0 360 50" fill="none" stroke="#b8976c" stroke-width="2"/>
      <text x="210" y="0" class="font-mono" font-size="10" fill="#b8976c">Circular Arc (5 params, Kåsa O(1))</text>

      <!-- Candidate 3: G1 Cubic -->
      <path d="M 160 50 Q 260 -25 360 50" fill="none" stroke="#c9754a" stroke-width="2.5"/>
      <text x="190" y="-28" class="font-mono" font-size="10" fill="#c9754a">Levien G1 Cubic (6 params, Quartic root)</text>

      <!-- Recurrence Formula on Right -->
      <g transform="translate(620, -10)">
        <text x="0" y="20" class="font-mono" font-size="12" fill="#d4896a">best[j] = min_{i &lt; j, kind} [ base(i) + segcost(kind, i, j) ]</text>
        <text x="0" y="44" class="font-mono" font-size="11" fill="rgba(250,248,245,0.7)">segcost = 0.5·χ²(kind, i, j) + λ·PARAMS(kind) + bow_penalty</text>
        <text x="0" y="66" class="font-sans" font-size="11" fill="#34d399">Verified strictly optimal against exhaustive search.</text>
      </g>
    </g>
  </g>

  <!-- Bottom 3 Pillars: Specific Mathematical Models -->
  <g transform="translate(60, 280)">
    <!-- Card 1: Raph Levien's Quartic Closed-Form Fit -->
    <g transform="translate(0, 0)">
      <rect x="0" y="0" width="340" height="400" rx="10" fill="#141210" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="340" height="3" rx="1.5" fill="#c9754a"/>
      <text x="20" y="28" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">Raph Levien G1 Cubic Fit</text>
      <text x="20" y="48" class="font-sans" font-size="11" fill="#c9754a">Green's Theorem Moment Matching</text>

      <g transform="translate(20, 68)">
        <rect x="0" y="0" width="300" height="150" rx="6" fill="#1a1816" stroke="#2a2724"/>
        <text x="14" y="24" class="font-sans" font-size="12" fill="rgba(250,248,245,0.85)">
          <tspan x="14" dy="0">• Matches signed area via Green's:</tspan>
          <tspan x="24" dy="18" class="font-mono" fill="#b8976c">∮ (x dy - y dx) = 2·Area</tspan>
          <tspan x="14" dy="22">• Matches first x-moment:</tspan>
          <tspan x="24" dy="18" class="font-mono" fill="#b8976c">∮ x (x dy - y dx) = 3·M_x</tspan>
          <tspan x="14" dy="22">• Reduces 2D arm space to a <tspan fill="#34d399">single quartic</tspan></tspan>
          <tspan x="24" dy="18" fill="rgba(250,248,245,0.6)">solvable analytically in closed form!</tspan>
        </text>
      </g>

      <text x="20" y="248" class="font-sans" font-size="12" font-weight="600" fill="#faf8f5">Why it beats Schneider (Graphics Gems):</text>
      <text x="20" y="270" class="font-sans" font-size="11" fill="rgba(250,248,245,0.7)">
        <tspan x="20" dy="0">C-shaped cubics possess <tspan fill="#f87171">three local minima</tspan>.</tspan>
        <tspan x="20" dy="18">Schneider's gradient descent routinely gets trapped</tspan>
        <tspan x="20" dy="18">in suboptimal arm configurations. Levien's quartic</tspan>
        <tspan x="20" dy="18" fill="#34d399">inspects all 4 roots, finding the global minimum.</tspan>
      </text>
    </g>

    <!-- Card 2: Ahn Orthogonal Distance vs Taubin Algebraic Bias -->
    <g transform="translate(370, 0)">
      <rect x="0" y="0" width="340" height="400" rx="10" fill="#141210" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="340" height="3" rx="1.5" fill="#b8976c"/>
      <text x="20" y="28" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">Ahn Orthogonal Distance</text>
      <text x="20" y="48" class="font-sans" font-size="11" fill="#b8976c">Geometric vs Algebraic Fitting</text>

      <g transform="translate(20, 68)">
        <rect x="0" y="0" width="300" height="150" rx="6" fill="#1a1816" stroke="#2a2724"/>
        <text x="14" y="24" class="font-sans" font-size="12" fill="rgba(250,248,245,0.85)">
          <tspan x="14" dy="0">• Taubin algebraic fit: fast initializer,</tspan>
          <tspan x="24" dy="18" fill="#f87171">but carries documented high-curvature bias</tspan>
          <tspan x="24" dy="18" fill="#f87171">(shrinks tight radii and fillets!)</tspan>
          <tspan x="14" dy="24">• Ahn Orthogonal Distance Fitting (ODF):</tspan>
          <tspan x="24" dy="18" class="font-mono" fill="#34d399">d_ortho = ||p_k - proj(p_k, curve)||</tspan>
          <tspan x="24" dy="18" fill="rgba(250,248,245,0.6)">True geometric distance to ellipse/arc.</tspan>
        </text>
      </g>

      <text x="20" y="248" class="font-sans" font-size="12" font-weight="600" fill="#faf8f5">Why it matters for brand logos:</text>
      <text x="20" y="270" class="font-sans" font-size="11" fill="rgba(250,248,245,0.7)">
        <tspan x="20" dy="0">Logo design lives at high curvature — corner fillets,</tspan>
        <tspan x="20" dy="18">counter-spaces, and tight serif brackets.</tspan>
        <tspan x="20" dy="18" fill="#34d399">Ahn ODF preserves intended corner radii</tspan>
        <tspan x="20" dy="18">without clipping or flattening.</tspan>
      </text>
    </g>

    <!-- Card 3: Exact Primitives in the Alphabet -->
    <g transform="translate(740, 0)">
      <rect x="0" y="0" width="340" height="400" rx="10" fill="#141210" stroke="#35322e" stroke-width="1"/>
      <rect x="0" y="0" width="340" height="3" rx="1.5" fill="#34d399"/>
      <text x="20" y="28" class="font-display" font-size="16" font-weight="600" fill="#faf8f5">First-Class Primitives</text>
      <text x="20" y="48" class="font-sans" font-size="11" fill="#34d399">Parametric Primitives inside the DP</text>

      <g transform="translate(20, 68)">
        <rect x="0" y="0" width="300" height="150" rx="6" fill="#1a1816" stroke="#2a2724"/>
        <text x="14" y="24" class="font-mono" font-size="11" fill="#faf8f5">
          <tspan x="14" dy="0">• &lt;circle cx cy r&gt;       : <tspan fill="#34d399">3 params</tspan></tspan>
          <tspan x="14" dy="20">• &lt;ellipse cx cy rx ry φ&gt; : <tspan fill="#34d399">5 params</tspan></tspan>
          <tspan x="14" dy="20">• &lt;rect x y w h rx ry&gt;    : <tspan fill="#34d399">6 params</tspan></tspan>
          <tspan x="14" dy="20">• Four cubics (circle)   : <tspan fill="#f87171">24 params</tspan></tspan>
          <tspan x="14" dy="20">• Thirty-six chords       : <tspan fill="#f87171">72 params</tspan></tspan>
        </text>
      </g>

      <text x="20" y="248" class="font-sans" font-size="12" font-weight="600" fill="#faf8f5">MDL Compactness Rewards Editability:</text>
      <text x="20" y="270" class="font-sans" font-size="11" fill="rgba(250,248,245,0.7)">
        <tspan x="20" dy="0">Because description length rewards compactness,</tspan>
        <tspan x="20" dy="18">a circle written as &lt;circle&gt; saves 21 parameters!</tspan>
        <tspan x="20" dy="18" fill="#34d399">A designer opening the SVG can grab the radius</tspan>
        <tspan x="20" dy="18">handle in Figma or Illustrator and edit it cleanly.</tspan>
      </text>
    </g>
  </g>
</svg>
"""

# -----------------------------------------------------------------------------
# GRAPHIC 5: Global Boundary Solve (Stage 08 Analysis-by-Synthesis)
# -----------------------------------------------------------------------------
svg5 = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1200 680" width="1200" height="680" style="background:#0c0a09; font-family:'Inter', sans-serif;">
  <defs>
    <style>
      @import url('https://fonts.googleapis.com/css2?family=Playfair+Display:wght@600;700&amp;family=Inter:wght@400;500;600;700&amp;family=JetBrains+Mono:wght@400;500;600&amp;display=swap');
      .font-display { font-family: 'Playfair Display', Georgia, serif; }
      .font-mono { font-family: 'JetBrains Mono', monospace; }
      .font-sans { font-family: 'Inter', sans-serif; }
    </style>
  </defs>

  <rect width="1200" height="680" fill="#0c0a09"/>

  <!-- Header -->
  <g transform="translate(60, 40)">
    <text x="0" y="16" class="font-sans" font-size="11" font-weight="700" letter-spacing="0.16em" fill="#34d399">ANALYSIS-BY-SYNTHESIS · STAGE 08</text>
    <text x="0" y="44" class="font-display" font-size="26" font-weight="700" fill="#faf8f5">Global Boundary Solve: Analytic Area Jacobian</text>
  </g>

  <!-- Left: Geometric Polygon Clipping inside Pixel Grid -->
  <g transform="translate(60, 110)">
    <rect x="0" y="0" width="520" height="510" rx="10" fill="#141210" stroke="#35322e" stroke-width="1"/>
    <text x="24" y="32" class="font-display" font-size="18" font-weight="600" fill="#faf8f5">Pixel Clipping &amp; Shoelace Area Integration</text>
    <text x="24" y="52" class="font-sans" font-size="12" fill="rgba(250,248,245,0.65)">Exact continuous coverage computed by boundary chain polygon clipping</text>

    <!-- Visual Pixel Grid with Polygon Clipping -->
    <g transform="translate(60, 75)">
      <!-- Pixel Square (280x280) -->
      <rect x="0" y="0" width="280" height="280" fill="#1a1816" stroke="#35322e" stroke-width="2"/>
      
      <!-- Clipped Polygon on the left side of the boundary -->
      <polygon points="0,0 280,0 280,70 200,160 80,210 0,280" fill="#c9754a" fill-opacity="0.25"/>
      <polygon points="0,0 280,0 280,70 200,160 80,210 0,280" fill="none" stroke="#c9754a" stroke-width="2"/>

      <!-- Boundary points inside pixel -->
      <circle cx="200" cy="160" r="6" fill="#34d399" stroke="#faf8f5" stroke-width="2"/>
      <text x="212" y="165" class="font-mono" font-size="11" fill="#34d399">v₁ (Prov::Vertex)</text>

      <circle cx="80" cy="210" r="6" fill="#34d399" stroke="#faf8f5" stroke-width="2"/>
      <text x="92" y="215" class="font-mono" font-size="11" fill="#34d399">v₂ (Prov::Vertex)</text>

      <!-- Gridline Crossing Points -->
      <circle cx="280" cy="70" r="5" fill="#b8976c"/>
      <text x="210" y="60" class="font-mono" font-size="10" fill="#b8976c">Prov::CrossV</text>

      <circle cx="0" cy="280" r="5" fill="#b8976c"/>
      <text x="12" y="270" class="font-mono" font-size="10" fill="#b8976c">Prov::CrossH</text>

      <!-- Fixed Pixel Corners -->
      <circle cx="0" cy="0" r="4" fill="rgba(250,248,245,0.4)"/>
      <circle cx="280" cy="0" r="4" fill="rgba(250,248,245,0.4)"/>
      <text x="10" y="20" class="font-mono" font-size="9" fill="rgba(250,248,245,0.4)">Prov::Corner</text>
    </g>

    <g transform="translate(24, 385)">
      <rect x="0" y="0" width="472" height="105" rx="6" fill="#1a1816" stroke="#2a2724"/>
      <text x="16" y="24" class="font-mono" font-size="12" fill="#d4896a">Shoelace Formula for Polygon Area:</text>
      <text x="16" y="46" class="font-mono" font-size="13" fill="#faf8f5">A = 0.5 · Σ (x_i · y_{i+1} - x_{i+1} · y_i)</text>
      <text x="16" y="70" class="font-sans" font-size="11" fill="rgba(250,248,245,0.7)">Gradient w.r.t vertex: <tspan class="font-mono" fill="#34d399">∂A/∂x_i = -0.5(y_{i+1} - y_{i-1})</tspan></text>
      <text x="16" y="90" class="font-sans" font-size="11" fill="rgba(250,248,245,0.7)">No numerical finite differences needed!</text>
    </g>
  </g>

  <!-- Right: Analytic Jacobian Tracking & LM Energy Minimization -->
  <g transform="translate(620, 110)">
    <rect x="0" y="0" width="520" height="510" rx="10" fill="#141210" stroke="#35322e" stroke-width="1"/>
    <text x="24" y="32" class="font-display" font-size="18" font-weight="600" fill="#faf8f5">Levenberg-Marquardt &amp; Prov Tracking</text>
    <text x="24" y="52" class="font-sans" font-size="12" fill="rgba(250,248,245,0.65)">Propagating pixel area derivatives directly to boundary coordinate unknowns</text>

    <!-- Prov enum explanation box -->
    <g transform="translate(24, 75)">
      <rect x="0" y="0" width="472" height="190" rx="8" fill="#1a1816" stroke="#2a2724"/>
      <text x="16" y="24" class="font-mono" font-size="12" font-weight="600" fill="#34d399">enum Prov { Vertex, CrossV, CrossH, Corner }</text>
      <text x="16" y="48" class="font-sans" font-size="12" fill="rgba(250,248,245,0.8)">
        <tspan x="16" dy="0">• <tspan class="font-mono" fill="#faf8f5">Prov::Vertex(v):</tspan> Boundary point. ∂A/∂v passes straight through.</tspan>
        <tspan x="16" dy="24">• <tspan class="font-mono" fill="#b8976c">Prov::CrossV { line, a, b }:</tspan> Crosses vertical gridline.</tspan>
        <tspan x="26" dy="18" class="font-mono" font-size="11" fill="rgba(250,248,245,0.6)">t = (line - a.x)/(b.x - a.x); propagates to unknowns a &amp; b.</tspan>
        <tspan x="16" dy="24">• <tspan class="font-mono" fill="#b8976c">Prov::CrossH { line, a, b }:</tspan> Crosses horizontal gridline.</tspan>
        <tspan x="16" dy="24">• <tspan class="font-mono" fill="rgba(250,248,245,0.4)">Prov::Corner:</tspan> Fixed pixel corner; zero contribution.</tspan>
      </text>
    </g>

    <!-- Energy minimization box -->
    <g transform="translate(24, 285)">
      <rect x="0" y="0" width="472" height="205" rx="8" fill="#1a1816" stroke="#2a2724"/>
      <text x="16" y="24" class="font-mono" font-size="12" font-weight="600" fill="#c9754a">JOINT NON-LINEAR ENERGY FUNCTION:</text>
      
      <text x="16" y="52" class="font-mono" font-size="12" fill="#faf8f5">E = Σ || a·c_left + (1-a)·c_right - target ||²</text>
      <text x="36" y="72" class="font-mono" font-size="11" fill="#b8976c">+ w_kink · Σ |p_{i-1} - 2·p_i + p_{i+1}|</text>
      <text x="36" y="90" class="font-mono" font-size="11" fill="#34d399">+ w_anchor · Σ |p_i - p_i⁰|²</text>

      <text x="16" y="118" class="font-sans" font-size="11" fill="rgba(250,248,245,0.75)">
        <tspan x="16" dy="0">• Data term evaluates true rendered pixel coverage</tspan>
        <tspan x="16" dy="18">• Kink regularizer discourages sharp boundary oscillations</tspan>
        <tspan x="16" dy="18">• Anchor regularizer stabilizes motion along boundary tangents</tspan>
        <tspan x="16" dy="18">• Self-intersection fold guard rejects any move that flips topology</tspan>
      </text>
    </g>
  </g>
</svg>
"""

# Write all SVGs
files = {
    "docs/assets/pipeline-step-by-step.svg": svg1,
    "docs/assets/subpixel-coverage-mechanics.svg": svg2,
    "docs/assets/planar-map-topology.svg": svg3,
    "docs/assets/curve-fitting-dp-models.svg": svg4,
    "docs/assets/boundary-solve-optimization.svg": svg5,
}

for path, content in files.items():
    with open(path, "w", encoding="utf-8") as f:
        f.write(content.strip())
    # Verify XML validity
    try:
        ET.fromstring(content.strip())
        print(f"Verified valid XML: {path}")
    except Exception as e:
        print(f"Error parsing {path}: {e}")
