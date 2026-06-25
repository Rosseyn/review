# reMarkable Headless Browser — Research & Implementation Plan

A speed-first, viewer-first web browser for the **current reMarkable e-ink lineup**,
written in Rust, driving the panel through `libremarkable` and the kernel EPDC
framebuffer.

> Status: research + planning. No code yet. This document is the spec the
> implementation will be built against. Sections tagged **[COMPLEXITY: …]** flag work
> that is disproportionately hard relative to its size.
>
> **Targeting decision (this revision):** the **64-bit Paper Pro family — Paper Pro,
> Paper Pro Move, Paper Pure — are the primary targets.** The 32-bit reMarkable 2 is a
> now-discontinued outlier; it is accommodated only via a separate, degraded build
> behind hardware-abstraction seams, and can be dropped at zero cost to the primary
> build (see §3).

---

## 1. Goals & priority order

Design is ordered by three emphases, in strict priority:

1. **Speed through minimal resource use — CPU above all.** Every target is
   **software-rendering-only** (no usable web-class GPU exists anywhere in the lineup —
   §2), on modest aarch64 cores with 2 GB RAM. Choices are judged first by CPU/RAM
   cost; idle CPU ~0%; hot paths prefer integer/fixed-point and NEON.
2. **Responsiveness — input must feel instantaneous.** Primary lever: switch to fast,
   low-quality e-ink draw modes during rapid redraws, but **only past a measured
   redraw-rate threshold**, with hysteresis so it never trips accidentally, and
   **suppressed when only text is redrawn** (a cheap bitonal/mono update suffices). On
   color panels the fast path is the **mono partial-update** mode, not slow color.
3. **Quality — get the most out of the panel when not in fast mode.** Software
   dithering / palette-aware quantization to the panel, the best-fit waveform per
   content type, and a user-triggerable **hi-fi redraw**. On color (Gallery 3) panels
   this includes color, treated as a final quantization stage — see the
   double-dithering caution in §8.

Non-goals (first pass): a complete web platform, banking-grade security, rich editing,
file-system/drag-drop, and most interactive Web APIs. This is a **viewer**; interaction
exists only to enable more viewing.

---

## 2. Target device matrix (established facts, June 2026)

Four devices exist; the original reMarkable 1 is excluded. Codenames are confirmed from
reMarkable's own kernel defconfigs (`reMarkable/linux-imx-rm`).

| | **Paper Pro** (`ferrari`) | **Paper Pro Move** (`chiappa`) | **Paper Pure** (`tatsu`) | reMarkable 2 (`rm2`) |
|---|---|---|---|---|
| Role | **Primary** (flagship) | **Primary** | **Primary** (budget) | **Outlier / discontinued** |
| Launch | Sep 2024 | 2025 | May 2026 | 2020 |
| SoC | NXP **i.MX8M Mini** | NXP **i.MX93** | NXP **i.MX93** | NXP **i.MX7D** |
| CPU | 4× Cortex-A53 @1.8 GHz | 2× Cortex-A55 @1.7 GHz | 2× Cortex-A55 @1.7 GHz | 2× Cortex-A7 @1.2 GHz |
| **Arch** | **aarch64** | **aarch64** | **aarch64** | **armv7 (32-bit)** |
| **GPU (3D)** | Vivante GC NanoUltra — **GLES 2.0 only** (web-unusable) | **none** (2D PXP only) | **none** (2D PXP only) | none |
| RAM | 2 GB LPDDR4 | 2 GB LPDDR4x | 2 GB LPDDR4 | 1 GB LPDDR3 |
| Storage | 64 GB | 64 GB | 32 GB | 8 GB |
| Panel | **Gallery 3 color** (ACeP) | **Gallery 3 color** (ACeP) | Carta 1300 **mono** | Carta **mono** |
| Color | ~20k colors + grey | ~20k colors + grey | 16 grey | 16 grey |
| Res / DPI | 2160×1620 / 229 | 1696×954 / 264 | 1872×1404 / 226 | 1872×1404 / 226 |
| Frontlight | yes | yes | no | no |
| Display path | real `/dev/fb0` + **mxcfb ioctl** | real `/dev/fb0` + **mxcfb ioctl** | real `/dev/fb0` + **mxcfb ioctl** | **rm2fb shim** (no kernel fb) |

**Three facts dominate the design:**

1. **All primary devices are 64-bit aarch64.** Build target `aarch64-unknown-linux-gnu`.
   RM2 alone needs `armv7-unknown-linux-gnueabihf`.
2. **No device has a usable web-class GPU.** Move/Pure have *no* 3D GPU; the Pro's GC
   NanoUltra is **GLES 2.0 only**, and WebRender/modern compositors require GLES 3.0 —
   so the GPU is unusable for web rendering even where present, and driver availability
   is itself unconfirmed. **Everything is CPU/software rendering.** (This is what keeps
   Servo disqualified — see §5 — regardless of bitness.)
3. **Two display-access stacks.** Primary devices expose a genuine kernel EPDC
   framebuffer driven by the classic Kindle-style `mxcfb` `MXCFB_SEND_UPDATE` ioctl
   (`mxcfb_update_data`: region, waveform_mode, update_mode, marker, temp, flags).
   **RM2 has no kernel framebuffer** (its EPDC work is done in closed software / SWTCON),
   which is the sole reason `rm2fb` exists (shm `/swtfb.01` + SysV msg queue + LD_PRELOAD
   server). **rm2fb is RM2-only**; the primary build never touches it.

There are also **two axes that vary within the primary tier**: **color** (Pro, Move =
Gallery 3) vs **mono** (Pure), and **core count** (quad A53 on Pro vs dual A55 on
Move/Pure). The renderer must handle both color and mono cleanly, and must not assume 4
cores.

---

## 3. Cross-device strategy & RM2 accommodation (the core of this revision)

The whole point of the plan is one codebase that serves the three primary devices well,
with RM2 isolated so it is **optional and droppable**. This is achieved with three
hardware-abstraction seams (detailed in §6):

- **`DisplayBackend`** — how pixels reach the panel. `mxcfb`-direct (primary) vs `rm2fb`
  (RM2). `libremarkable` already abstracts much of this.
- **`PixelPipeline`** — `Color` (Gallery 3: Pro/Move) vs `Mono` (Carta: Pure/RM2).
- **`RenderEngine`** — the web engine (WPE / Ultralight / Blitz / NetSurf), swappable.

Given those seams, device support reduces to a **build matrix**:

| Build | Targets | Arch | Display | Pixel | Engine | Notes |
|---|---|---|---|---|---|---|
| **Primary** | Pro, Move, Pure | aarch64 | mxcfb-direct | Mono *and* Color (separate renderers; panel-detected at runtime) | WPE (default) | 2 GB, the real product |
| **RM2 (kept)** | RM2 | armv7 | rm2fb | Mono | **WPE (armv7)** | 1 GB, degraded but same engine — Option C |

A single aarch64 binary serves all three primary devices; the **mono** and **color**
renderers are separate implementations (§8) selected at runtime from the detected panel.
RM2 is a *separate compile* (different target triple, `rm2fb` dependency, tighter memory
budget) gated behind a cargo feature so it never burdens the primary build.

### RM2 disposition — DECIDED: keep RM2 via Option C

RM2 is **kept** as a supported, if degraded, target. Of the three dispositions the
`RenderEngine` seam allows, we commit to **Option C**:

- **Option C — Same WPE engine on armv7 + rm2fb (CHOSEN).** WPE builds for both arm32 and
  arm64, so RM2 reuses the *same engine and almost all of the codebase* as the primary
  build — only the target triple, the `rm2fb` `DisplayBackend`, the mono `PixelPipeline`,
  and tighter memory tuning differ. This keeps **login compatibility on RM2** and avoids a
  second engine integration. The cost is real and accepted: WPE in 1 GB on a dual-A7
  behind the rm2fb shim means heavier memory pressure and higher latency, so RM2 leans
  hardest on aggressive `WPE_RAM_SIZE`/MemoryPressureSettings caps, single-tab operation,
  reader mode by default, and the mono fast-refresh path. We validate the 1 GB budget
  early (see §17/§18) and fall back to Option B only if WPE proves unviable there.
- **Option B — NetSurf + rm2fb (fallback only).** NetSurf is tiny and already ported to
  RM2, but its weak Duktape JS means **no modern logins**. Kept as a contingency behind the
  same `RenderEngine` trait (crate `rmb-engine-net`) if Option C can't fit 1 GB.
- **Option A — Drop RM2.** No longer the plan; retained only as the ultimate fallback if
  neither C nor B is acceptable.

Because RM2 shares the engine and all the application layers, keeping it costs the primary
build little — the deltas are confined to the HAL seams.

---

## 4. Display & input stack per tier

**Primary (Pro/Move/Pure) — mxcfb-direct.** Standard NXP i.MX EPDC. We mmap the kernel
framebuffer and issue `MXCFB_SEND_UPDATE` with `mxcfb_update_data`, waiting on update
markers via `MXCFB_WAIT_FOR_UPDATE_COMPLETE` only when serializing a dependent flush.
`libremarkable` (0.7.x, MSRV 1.80) lists Paper Pro support and wraps the partial/full
refresh + marker API; its **color (Gallery 3) waveform handling is the least-mature part
of the third-party stack** and must be validated on-device (see risks, §17). Frontlight
(Pro/Move) is controlled via a sysfs backlight node (e.g. `/sys/class/backlight/…`),
confirmed at runtime.

**RM2 — rm2fb.** No kernel fb; the rm2fb **server** (`LD_PRELOAD`ed into `xochitl`) owns
the panel and exposes shm `/swtfb.01` (1404×1872×u16, RGB565) + a SysV message queue.
`libremarkable` speaks this protocol natively when built for RM2.

**Input (all devices).** Standard Linux **evdev**: a Wacom EMR pen digitizer (4096
pressure, tilt on the Pro-family Markers) and a capacitive multitouch layer (plus
power/buttons). `libremarkable::input` decodes these. Protocol is functionally uniform
across devices; differences are mechanical.

### Mono waveform modes (Carta — Pure, RM2)
i.MX EPDC v1, no hardware dither (we dither in software). Measured reMarkable latencies:

| Mode | ID | ~Latency | Depth | Use |
|---|---|---|---|---|
| INIT | 0x0 | ~780 ms | — | Full clear / de-ghost |
| **DU** | 0x1 | **~170 ms** | 1-bit | Fast text/UI, caret (the mono fast path) |
| **GC16** | 0x2 | ~460 ms | 16 | Quality default; images/page |
| GL16 / GLR16 (REAGL) | 0x3/0x4 | ~460 ms | 16 | Sharp text, reduced flash |
| **A2** | 0x6 | **~135 ms** | 1-bit | Fastest; scroll/animation, ghosts → flush |
| DU4 | 0x7 | ~300 ms | 4 | Non-flashing menus |

### Color refresh classes (Gallery 3 ACeP — Pro, Move)
ACeP synthesizes full color per pixel from CMY+W pigment (no CFA, no resolution
penalty), but color refreshes are **much slower** than mono:

| Class | ~Latency | Use |
|---|---|---|
| Mono/B-W partial | ~12–350 ms | **Text, scroll, interaction fast path** |
| Fast color | ~500 ms | Quick color paint |
| Standard color | ~750–1000 ms | Normal static page paint |
| Best color | ~1500 ms | Hi-fi redraw |

**Implication:** on color panels, drive the **mono partial-update fast path** for
scrolling/typing/interaction and reserve slow color refreshes for static page paint and
hi-fi. Color full-refreshes are far too slow to use during motion.

---

## 5. Engine selection — re-ranked for 64-bit, software-rendered, color e-ink

The hard requirement remains **basic login compatibility** (Google/Amazon/Microsoft/
Apple), which needs a real web platform (DOM/CSSOM, `fetch`+cookies, WebCrypto). Dropping
the 32-bit constraint and confirming "no usable GPU anywhere" re-orders the field:

| Engine | aarch64 | CPU render → buffer | JS / login | RAM (2 GB fit) | License | Verdict vs prior (32-bit) |
|---|---|---|---|---|---|---|
| **WPE WebKit** | ✅ | ✅ Skia CPU is the *default & recommended* on weak-GPU embedded (since 2.46; Cairo removed by 2.53) | **JSC — full platform** | ~180 MB web proc, comfortable | LGPL/BSD (free) | **Promoted to clear default** |
| **Ultralight** | ✅ (ARM64 since 1.4) | ✅ CPU→**BGRA** buffer (textbook e-ink model) | JSC | small | **closed, ~$3k/yr/app; free tier forbids static linking** | **Was disqualified (32-bit) → now a top-2 technical fit** |
| **Blitz** (`anyrender_vello_cpu`) | ✅ | ✅ vello_cpu, purpose-built for no/weak GPU, NEON | **none** (no logins) | moderate | Rust (free) | **Strategic Rust-native future; still alpha** |
| **CEF / Chromium** | ✅ | ✅ software OSR (`--disable-gpu`) | V8 — best compat | 150–500 MB/page, ~1 core | BSD | Was "no" (RAM) → **"possible but against priorities"** |
| **Servo / Verso** | ✅ tier | ✗ WebRender needs **GLES 3.0**; swgl unexposed in libservo | SpiderMonkey | 100s MB | MPL | **Still ruled out** — blocked by GPU/swgl, not bitness |
| **NetSurf** | ✅ | ✅ own engine, tiny | Duktape ES5 (weak) | very low | free | **RM2/degraded fallback** |

### Recommendation: **Rust application shell + WPE WebKit engine via FFI** (primary),
behind a `RenderEngine` trait, with **Ultralight as a licensed alternative**, **Blitz as
the Rust-native migration target**, and **NetSurf as the RM2/degraded fallback.**

Why WPE stays the default even with the 32-bit constraint gone: it is the only **open-
source** engine that (a) defaults to **CPU rendering** and Igalia found CPU *beats* weak
embedded GPUs — exactly the GC NanoUltra situation; (b) fits 2 GB with room (~180 MB web
process, tunable via `WPE_RAM_SIZE`/MemoryPressureSettings); (c) ships **JavaScriptCore**
+ a real web platform for logins; (d) runs on both arm32 and arm64, so it could even
serve RM2 (Option C). Everything around it is pure Rust and is where the project's value
lives; WebKit is a replaceable component.

**Ultralight** is now genuinely competitive — its CPU→BGRA model is the ideal e-ink fit
and it has JSC for logins — but it is **closed-source and paid** (~$3k/yr/app; free tier
forbids static linking, awkward for a single-binary device deploy). Keep it as a
drop-in `RenderEngine` alternative; pick it only if a commercial license is acceptable
and benchmarks beat WPE on-device.

**Blitz** is the long-term Rust-native bet (Stylo+Taffy+Parley+`vello_cpu`, the right CPU
renderer for this hardware) but is **alpha and has no JS** (no logins). Prototype now;
adopt as it matures. If login support is ever dropped from scope, Blitz becomes the
primary and the project is **end-to-end Rust**.

> **Decision to confirm (see §19):** WPE (open, logins, heavy C++ dep) vs. Ultralight
> (best e-ink fit, paid/closed) vs. Blitz (pure Rust, no logins). Default below: **WPE**,
> abstracted so the choice is reversible per-build.

---

## 6. System architecture

Single multi-threaded Rust process; three abstraction seams isolate every hardware/engine
difference so the primary build is uniform and RM2 is optional.

```
                         ┌──────────────────────────────────────────────┐
   evdev (touch/pen/btn) │                 rmbrowser (Rust)               │
        ─────────────▶   │  input ─▶ gesture/intent ─▶ app state          │
                         │   ┌──────────────────────────────────────────┐│
                         │   │ overlay (virtual buttons, drawers, reader)││  never enters DOM
                         │   └───────────────┬──────────────────────────┘│
                         │                   ▼                            │
                         │   RenderEngine ◀── nav / zoom / JS             │
                         │   (WPE | Ultralight | Blitz | NetSurf) → RGBA  │
                         │                   ▼                            │
                         │   compositor: overlay ⊕ page → dirty rects     │
                         │                   ▼                            │
                         │   PixelPipeline (one trait, two renderers):    │
                         │     ├─ Mono(Carta): grey quantize + SW dither  │
                         │     ├─ Color(Gallery3): gamut+auto-contrast →   │
                         │     │    SW dither (ordered); panel = synthesis │
                         │     │    only; PxP offload on Move if exposed   │
                         │     └─ waveform FSM (mode select, ghost flush) │
                         │                   ▼                            │
                         │   DisplayBackend: mxcfb-direct | rm2fb          │
                         └──────────────────────────────────────────────┘
```

### Threads
- **Input** — evdev → debounced high-level `Intent`s (scroll, tap, pinch, button). Never blocks.
- **Engine** — the web engine's own loop; emits painted surfaces + damage rects.
- **Composite/refresh** — sole framebuffer writer; merges page+overlay, diffs dirty rects,
  runs `PixelPipeline` + the waveform FSM, dispatches updates via `DisplayBackend`.
- SPSC/lock-free channels between them. Target ~0% idle CPU; do not assume >2 cores.

### Workspace crate layout (Rust)
```
crates/
  rmbrowser        app shell, config, device detection, lifecycle
  rmb-hal          DisplayBackend (mxcfb-direct | rm2fb), panel/frontlight probe
  rmb-pixel-core   shared PixelPipeline trait, RGBA surface, dirty-rect & LUT helpers
  rmb-pixel-mono   Mono renderer (Carta): greyscale quantize + dither   (Pure, RM2)
  rmb-pixel-color  Color renderer (Gallery 3): gamut/auto-contrast + SW dither; opt. PxP (Pro, Move)
  rmb-display      waveform FSM (mono modes + color refresh classes), update dispatch
  rmb-compositor   page⊕overlay merge, dirty-rect tracking
  rmb-input        evdev → gestures → Intents
  rmb-engine       RenderEngine trait + impls
  rmb-engine-wpe   FFI to libwpe + headless backend  (feature: engine-wpe; all devices incl. RM2)
  rmb-engine-net   NetSurf FFI — RM2 fallback only    (feature: engine-netsurf)
  rmb-overlay      virtual buttons, link drawer, reader UI, TOC/landmark menu
  rmb-nav          link-drawer scoring, semantic fast-nav, zoom model
  rmb-reader       Readability-style extraction + reader stylesheet
  rmb-bookmarks    Netscape-HTML / Chromium-JSON / places.sqlite importers
  rmb-net          HTTPS/HTTP policy, TLS (rustls), cookie jar
```
Build features select arch/display/engine: primary = `aarch64 + mxcfb + engine-wpe` with
both `pixel-mono` and `pixel-color`; RM2 = `armv7 + rm2fb + engine-wpe` with `pixel-mono`
only (`engine-netsurf` is the fallback if WPE can't fit 1 GB).

---

## 7. Display pipeline & the refresh-mode state machine  *(Emphasis #1 & #2 — core)*

The composite thread runs a small, explicit state machine picking the waveform per
update from three inputs — **redraw rate**, **content class**, **current mode** — now
extended with a **color/mono axis**.

### 7.1 Per-update inputs
- **Dirty rects** — engine ∪ overlay damage, coalesced. Cost scales with area; keep tight.
- **Content class** — cheap classification during raster: `TEXT` (near-bitonal),
  `MONO_GREY` (greyscale image/gradient), `COLOR` (chromatic content, color panels only).
- **Redraw rate** — EMA of updates/sec over a sliding window.

### 7.2 Mode selection

```
classify(region) + rate(ema) + mode(prev) + panel(color?) ─▶ waveform

QUALITY (default, idle / low-rate):
    Mono panel:  TEXT → GL16/GLR16 (or DU for tiny edits);  GREY/IMAGE → GC16
    Color panel: TEXT/GREY → mono partial (~350 ms);  COLOR → Standard color (~750 ms)

FAST (high-rate: scroll/pan) — engage ONLY when rate ≥ ENTER_FAST_HZ for
                              ENTER_FAST_FRAMES frames (hysteresis):
    TEXT-only          → DU / mono-partial   (bitonal; NO drop to A2)
    GREY/IMAGE (mono)  → A2 (~135 ms)
    COLOR (color panel)→ mono partial fast path (NEVER slow color during motion)

EXIT FAST: rate < EXIT_FAST_HZ for SETTLE_MS ─▶ one quality redraw of the touched
           region (GC16 on mono; Standard/Best color on color) to restore fidelity
           and clear ghosting.
```

Behaviors mandated by the brief, made explicit:

- **Threshold + hysteresis prevent accidental fast mode.** Entering requires the EMA to
  exceed `ENTER_FAST_HZ` for `ENTER_FAST_FRAMES` consecutive frames; a single burst never
  trips it. Exit uses a lower threshold + `SETTLE_MS` dwell (Schmitt-trigger). All
  config-tunable.
- **Text-aware suppression.** A `TEXT`-only rapid region uses **DU / mono-partial**, not
  A2 — crisp edges, no animation-mode contrast loss. A2 is reserved for genuine
  mono image/gradient motion.
- **Color never appears during motion.** On Gallery 3, full color refreshes (500–1500 ms)
  are disqualifying for scrolling; the fast path is always the mono partial mode. Color is
  painted only on settle / static page / hi-fi.
- **Ghost budget.** Per-region counter of consecutive fast (A2/DU) updates; at
  `GHOST_FLUSH_N`, or on scroll-stop / navigation, a full-region quality flush runs (GC16
  or INIT on mono; full color on color). White-frame padding wraps A2.
- **Marker discipline.** Fire-and-forget partials for latency; `wait_refresh_complete`
  only before a dependent full flush.

### 7.3 Tunable constants (initial guesses, measured on-device per §18)
`ENTER_FAST_HZ ≈ 4`, `ENTER_FAST_FRAMES ≈ 3`, `EXIT_FAST_HZ ≈ 2`, `SETTLE_MS ≈ 350`,
`GHOST_FLUSH_N ≈ 12`. **[COMPLEXITY: MED]** — small logic, but the constants and the
color/mono thresholds need empirical tuning; "feels instant" is iterative.

---

## 8. Rendering quality: dithering, color quantization, waveforms & hi-fi  *(Emphasis #3)*

The engine renders full RGBA; a `PixelPipeline` maps it to the panel. **Mono and color
are two substantially different renderers, not one parameterized path** — they differ in
bit depth, dithering strategy, waveform classes, and (crucially) in how much work is done
in software vs. handed to the panel. They share only the `rmb-pixel-core` scaffolding
(RGBA surface, dirty-rect plumbing, gamma LUT helpers, the `PixelPipeline` trait).

**Both are launch priorities, developed sequentially:** ship the **mono renderer first**
(it also covers Pure and RM2 and is the simpler, fully-documented case), then build the
**color renderer reusing as much of the mono scaffolding and the §7 FSM as possible**.
Mono is therefore both a product target and the proving ground for the shared core that
color builds on.

### 8.1 Mono panels (Pure, RM2) — grayscale dithering
~16 grey levels, no hardware dither. Two tiers:
- **Ordered (Bayer 8×8) — default.** No neighbor dependency → SIMD/parallel,
  cache-friendly, **stable across partial redraws** (no shimmer on re-dither). Cheapest
  CPU. **[COMPLEXITY: LOW]**
- **Floyd–Steinberg — hi-fi only.** Better gradients but serial and unstable under
  partial repaint, so confined to full-region/hi-fi renders. **[COMPLEXITY: MED]**

A gamma-aware sRGB→linear→16-level LUT is precomputed once.

### 8.2 Color panels (Pro, Move) — Gallery 3 pipeline  **[COMPLEXITY: HIGH]**

**Correction to an earlier assumption (important).** There is **no free hardware dithering
offload** on these devices. What the panel/TCON does for free is **waveform generation +
CMYW pigment color *synthesis*** — landing the four-particle (White/Cyan/Magenta/Yellow)
ink to realize a pixel's target color. That is the part we must **not** replicate. But
**spatial dithering is *not* free and is the host's job:** each Gallery 3 pixel resolves to
only **8 solid colors** (K, R, G, B, C, M, Y, W); the ~20,000 perceived shades exist
**only via spatial dithering across pixels**, and on the i.MX8M Mini (Paper Pro) there is
**no EPDC peripheral at all** to do it. reMarkable's own stack (closed, inside
`QsgEpaperPlugin`/xochitl) does it in software: **auto-contrast → quantize to a fixed
palette → dither**, confirmed by the wavexx reverse-engineering of the Paper Pro. So the
muddy/washed-out reputation comes from naive sRGB→panel mapping into a narrow,
low-saturation gamut — not from "double dithering" per se, though double-dithering is a
real, separate hazard to avoid.

**Division of labor — what the host (us) MUST compute (no offload on Paper Pro):**
1. **Gamut + gamma map** sRGB → the panel's measured primaries, **perceptual intent** to
   avoid washout. Reuse/refine the wavexx-measured ICC primaries as the target (a small 3D
   LUT or matrix+gamma). This is the single biggest correctness lever.
2. **Global auto-contrast / tone map** (cheap level stretch) — the biggest *visual* win per
   CPU cycle against the muddy-color problem.
3. **Quantize + dither to the 8-color palette.** Two-tier, mirroring mono (§8.1) and reusing
   the same `rmb-pixel-core` dither code:
   - **Ordered / blue-noise — default.** O(1)/pixel, NEON-vectorizable, stable across
     partial redraws. Right for text/UI/static color and the CPU budget.
   - **Error-diffusion (serpentine Floyd–Steinberg/Atkinson) — high-value images only.**
     Sequential and costly; accumulate error in a **≥8-bit working buffer** to avoid banding.
4. **Pack** to the panel's expected indexed/bit-depth format.

**What the panel does for free (do NOT redo):** waveform + CMYW color synthesis only.

**Opportunistic hardware offload — Move (i.MX93) ONLY:** the i.MX93 has a **PxP** engine
with an EPDC path that *can* do colorspace conversion, rotation, and **ORDERED** quantize
as a DMA-class pass — offloading work off the dual A55 **if reMarkable exposes the ioctl**.
Treat as opportunistic, probed at runtime, never assumed. **Paper Pro cannot do this (no
EPDC); error-diffusion is never hardware-offloaded anywhere** (it's the mode HW omits).

**Pitfalls:** (a) **double-dithering** — if reMarkable's compositor or any driver stage
applies its own dither, ours must be the only one (request a passthrough/no-second-dither
path); (b) **banding** from quantizing a low-bit buffer — always diffuse in high precision;
(c) keep the **mono/grey fast-path** for text/scroll, engaging color only on static paint
(color waveforms are 500–1500 ms, §4).

**On-device unknowns to resolve (risk, §17):** exact color framebuffer pixel format
reMarkable's compositor accepts; whether it applies its own dither (double-dither check);
whether Move exposes PxP dithering ioctls; the panel's true measured primaries (start from
the wavexx profile).

### 8.3 Draw modes to support out of the gate
Mono: GC16 (P0), DU (P0), INIT (P0), A2 (P1), GL16/GLR16 (P1), DU4 (P2), AUTO (P2).
Color: mono-partial fast path (P1), Standard color (P1), Fast color (P2), Best color
(P2, hi-fi). Per-tile adaptive color/mono mode selection is **deferred [HIGH]**.

### 8.4 Hi-fi controls (brief: "higher fidelity, or trigger a hi-fi redraw")
- **Hi-fi redraw of current screen** — re-render visible page at full quality: mono =
  Floyd–Steinberg + GC16; color = best-color waveform + careful quantization. Always
  available. **[COMPLEXITY: LOW once pipeline exists]**
- **Sustained hi-fi mode** — disables fast-mode switching (rate threshold → ∞), forces
  quality waveforms. Trades responsiveness for fidelity. **[COMPLEXITY: LOW]**
- **Deferred [HIGH]:** content-adaptive per-tile mode selection (text tiles mono/GL16,
  image tiles GC16/color within one frame); waveform-LUT customization; per-panel color
  profiles.

---

## 9. Input & interaction model

Viewer-first; deliberately tiny gesture set: **touch scroll/pan**, **pinch zoom**, **tap**
(activate link/control/virtual button), **pen** (precise tap/pointer; no inking),
**power/buttons** (system). Explicitly **dropped / fail-gracefully**: drag-and-drop,
file-system interactions, multi-finger gestures beyond pinch, long-press menus,
`contenteditable` rich editing (text limited to login/review fields), selection/clipboard
beyond login needs. See §16.

---

## 10. Virtual buttons subsystem  *(rmb-overlay)*

On-screen buttons in the overlay layer **above** the page; events consumed by the overlay
and **never passed into the DOM**.

- **Placement:** anywhere, by `(x%, y%)` + size; hit-tested before the page.
- **Action (swappable trait):** `Scroll{amount, dir}` (configurable amount — lines /
  screen-fraction / px), plus `NavNext/Prev` (via link drawer), `ZoomIn/Out`,
  `PageUp/Down`, `ReaderToggle`, `HiFiRedraw`, `OpenDrawer`, `Frontlight±` (Pro/Move).
- **Toggle** globally and per-button.
- **Orientation anchoring (two modes per button):** `RotateWithScreen` (rotates &
  repositions with orientation) vs `FixedScreenPercent` (stays at the same vertical/
  horizontal % of the physical screen regardless of orientation; re-projected, not
  rotated).
- Large hit targets (§12), high-contrast borders; drawn with DU/mono-partial so presses
  feel instant. **[COMPLEXITY: MED]** — orientation math + consume-before-DOM ordering.

---

## 11. Link drawer / semantic fast navigation  *(rmb-nav)*

An Opera-"Fast Forward/Rewind"-style panel for **direct navigation that doesn't rely on
styling or exact scroll position**. The engine exposes DOM/links; we score candidates for
fixed **slots**: `next · prev · up/parent · home/start · contents/index · top · close`.

### Scoring (per slot; highest above threshold wins)
```
+100  rel matches slot
+ 40  URL = current URL with page-number incremented/decremented (next/prev)
+ 30  link text / title / aria-label matches slot regex
+ 15  directional glyph  » › →  /  « ‹ ←
+ 15  inside <nav> / pagination landmark
+ 10  DOM position (bottom→next, top→prev/up)
− 50  text matches comment|login|signup|share|tag|print
− 30  off-origin
```
- **rel signals (strongest, but thin):** only `next`, `prev`, `canonical`, `bookmark`,
  `search` are standardized in HTML5/WHATWG; `up`, `contents`, `index`, `start`, `first`,
  `last`, `home` were HTML4-only or never standardized and were **dropped** — so they're
  rare. Treat `previous`→prev and `begin`/`start`→first as synonyms. Many sites also
  dropped `rel=prev/next` after Google's 2019 deprecation. **So rel is the primary signal
  only for next/prev when present; heuristics are primary for up/home/top/contents and a
  required fallback elsewhere.**
- **Text/glyph regex** (from Mozilla Readability, extend per locale):
  next `/(next|continue|weiter|older|>([^|]|$)|»([^|]|$))/i`;
  prev `/(prev|previous|earl|old|new|<|«)/i`.
- **ARIA landmarks** (`nav`, `main`, pagination containers) scope where heuristics run.

Powers the drawer, single-press virtual-button nav, and Fast-Forward/Rewind.
**[COMPLEXITY: MED]** — scoring is simple; extracting links/rel/landmarks from the engine
(injected JS / JSC DOM walk) is the work; locale coverage of text heuristics is the weak
spot.

---

## 12. Reader mode & accessibility  *(rmb-reader, rmb-overlay)*

Highest-value feature for slow e-ink (less DOM/reflow/repaint) and the accessibility
backbone; also the core of the RM2 degraded build (Option B).

- **Extraction:** Mozilla **Readability** (clone DOM first; it mutates). **Default *into*
  reader mode** when `isProbablyReaderable` is true. **[COMPLEXITY: MED]**
- **Reader stylesheet (WCAG):** black-on-white ≥7:1 (AAA), system serif/sans toggle,
  line-height ≥1.5, **ragged-right (never justify)**, ~66–80 ch measure, generous margins.
- **User controls:** font size (to 200%), family, line spacing, margins, **invert**
  (default off — large black fills worsen ghosting), text-only (drop images).
- **Jump-don't-scroll nav:** heading **TOC** (h1–h6) and **landmark menu**
  (`main`/`nav`/`aside`/`header`/`footer`/`search`) that anchor-jump (shares §11 machinery).
- **Large touch targets:** ≥44×44 px (WCAG 2.5.5) via padding; numbered-link mode.
- **Honor media features:** `prefers-reduced-motion` (default reduce), `-color-scheme`,
  `-contrast`, `forced-colors`.
- **Color note:** on Gallery 3, reader mode can optionally render in grayscale to keep the
  fast mono path and avoid slow color refresh while reading.
- **TTS:** out of scope first pass; capability-gate `speechSynthesis`, expose only if real
  voices exist. Flagged, not built.

**Engage:** reader extraction, reflow/resize, high-contrast defaults, heading/landmark
jump, numbered links + large targets, reduced-motion enforcement.
**Drop (no benefit, all cost):** animations/transitions/scroll effects, autoplay
video/animated GIF, heavy webfonts (prefer system fonts), JS-heavy SPA churn (reader
sidesteps), ads/trackers/analytics, infinite-scroll/lazy-on-scroll.

---

## 13. Bookmarks import  *(rmb-bookmarks)*

**No usable third-party cloud bookmark API** exists (Chrome Sync closed since 2021;
Firefox Sync is E2E-encrypted behind a partner-only protocol; Edge/iCloud have none;
Google Bookmarks shut down 2021). Import is file-based, transferred to the device over
USB/Wi-Fi. Priority:

1. **Netscape Bookmark HTML (primary).** Universal export format for every browser; one
   tolerant parser covers all. Walk the `<DL>` tree (`<DT><H3>`=folder, `<DT><A HREF>`=
   bookmark, `<DD>`=desc); `ADD_DATE`/`LAST_VISIT` are Unix **seconds**; tolerate unclosed
   tags. **[COMPLEXITY: LOW]**
2. **Chromium JSON (Chrome+Edge+Brave, one path).** Roots `bookmark_bar`/`other`/`synced`;
   timestamps **µs since 1601**; tolerate missing/encrypted file. **[COMPLEXITY: LOW]**
3. **Firefox `places.sqlite` (optional).** `SELECT b.title,h.url FROM moz_bookmarks b JOIN
   moz_places h ON h.id=b.fk`; folders via `parent`; µs since 1970; copy DB first (WAL).
   **[COMPLEXITY: MED]** (`rusqlite`).
4. **Safari `Bookmarks.plist`** — skip unless demanded.

No cloud sync; the supported "cloud" equivalent is a user-run HTML export / Google Takeout
fed into path 1.

---

## 14. Browser-level zoom  *(rmb-nav)*

Independent of pinch, to avoid re-pinching every page. **Persistent page zoom** (≈50–300%)
via the engine zoom API (WebKit `webkit_web_view_set_zoom_level`), global default + optional
per-host override, persisted. Pinch adjusts the same factor live (driving fast-mode during
the gesture; quality settle on release). Reader font scaling (§12) is separate/additive.
**[COMPLEXITY: LOW]**

---

## 15. Networking & security posture  *(rmb-net)*

First pass is explicitly **not** banking-grade and **blocks requests for advanced
security**, while supporting enough for Google/Amazon/Microsoft/Apple sign-in.

- **HTTPS default, HTTP supported** for compatibility (upgrade where possible, don't block).
- **TLS** via engine stack or `rustls`; modern suites; standard CA bundle.
- **Auth scope:** cookies, form POST, OAuth redirects, basic WebCrypto for those flows.
  **Block / gracefully refuse** advanced mechanisms beyond that — WebAuthn/passkey hardware
  authenticators, client-cert/smartcard, payment APIs — returning a clean "not supported"
  rather than partial, risky crypto. Fail closed on the unimplemented.
- **No** elaborate crypto subsystem, secure-enclave emulation, DRM/EME, WebRTC.
- Free privacy/perf win: block ads/trackers at the network layer.

**[COMPLEXITY: MED]** — engine brings TLS/cookies; our work is the **policy layer** and
graceful refusals.

---

## 16. Feature scope: included / dropped / graceful degradation

**Included:** HTML/CSS rendering (color where available), modest JS (login/forms), links &
basic forms, scroll/pinch/tap/pen-tap, page zoom, reader mode, link drawer, virtual
buttons, bookmark import, frontlight control (Pro/Move), HTTPS/HTTP.

**Dropped first pass (rule = *fail gracefully*):** drag-and-drop, File System Access, rich
input/IME beyond basic fields, WebGL/WebGPU (no GPU anyway), WebRTC, push notifications,
service workers/PWA install, Web Bluetooth/USB/Serial/MIDI, geolocation, camera/mic,
DRM/EME, payment, WebAuthn/passkeys (§15), autoplay media, heavy animations.

**Graceful-degradation contract:** every dropped capability is **feature-detectable as
absent** (API undefined or clean `NotSupportedError`), never a crash/hang. A central
capability registry defines per API `{ Absent | Stub-reject | Static-fallback }`.

---

## 17. Complexity & risk register

| Item | Complexity | Risk | Mitigation |
|---|---|---|---|
| WPE WebKit aarch64 cross-compile + headless backend + Rust FFI | HIGH | MED | No turnkey binding; isolate behind `RenderEngine`; 64-bit + 2 GB eases vs RM2 |
| **Gallery 3 color path** (no HW dither offload on Paper Pro — i.MX8M Mini has no EPDC; waveform IDs, pixel format, true primaries undocumented) | **HIGH** | **HIGH** | Host does gamut-map + auto-contrast + dither on CPU (NEON); reuse wavexx ICC primaries; reverse-engineer on-device; ship mono first, color second |
| **Muddy/washed-out color** (narrow gamut) + double-dithering | MED | **HIGH** | Perceptual gamut map + global auto-contrast; ensure ours is the only dither (passthrough); grayscale reader option |
| **PxP offload on Move not guaranteed** | LOW | LOW | Probe i.MX93 PxP ioctl at runtime; fall back to CPU ordered dither; Paper Pro is CPU-only regardless |
| Refresh-mode FSM "feels instant" (now with color axis) | MED | MED | On-device tuning pass (§18); hysteresis |
| A2/DU ghosting management | MED | MED | Ghost budget + GC16/INIT flush + white-frame padding |
| **RM2 = WPE on armv7 + rm2fb in 1 GB** (Option C) | MED | **HIGH** | Validate the 1 GB memory budget early; aggressive WPE caps, single-tab, reader-default, mono-only; NetSurf (Option B) as contingency |
| Readability extraction on messy pages | MED | MED | Reuse Mozilla algorithm; default-in |
| Link extraction (rel/landmarks/text) | MED | MED | Injected JS / JSC DOM walk; locale-limited heuristics |
| Virtual-button orientation anchoring + event capture | MED | LOW | Clear hit-test ordering; re-projection math |
| Bookmark importers | LOW–MED | LOW | HTML first; Chromium JSON; optional sqlite |
| Login compatibility actually working (fingerprinting/passkeys) | — | HIGH | Even JSC may face friction; manage expectations; refuse advanced auth gracefully |
| No-GPU everywhere | — | LOW (now expected) | Software render is the baseline assumption; no GPU code path to maintain |

---

## 18. Phased roadmap

- **Phase 0 — Device pipeline bring-up (de-risk).** `rmb-hal` mxcfb-direct over
  `libremarkable`; render test patterns on a **Pure (mono)** first; ordered dither;
  wire an initial engine (NetSurf or WPE) behind `RenderEngine`; basic touch scroll.
  *Exit:* a page scrolls on a primary device.
- **Phase 1 — Responsiveness core.** Dirty-rect compositor; content classifier; refresh
  FSM with mono fast path, hysteresis, text-suppression, ghost-flush; on-device **tuning
  pass** for §7.3 constants. *Exit:* scrolling feels instant; no accidental fast-mode.
- **Phase 2 — WPE integration + logins.** aarch64 WPE build; headless backend → compositor;
  RAM caps; cookies/TLS policy (`rmb-net`); zoom; basic forms/login. *Exit:* a real login
  page renders and submits.
- **Phase 3 — Color renderer (Gallery 3), reusing the mono core.** Build `rmb-pixel-color`
  on the shared scaffolding + §7 FSM; reverse-engineer/validate color waveforms and the
  framebuffer format on Pro/Move; host-side gamut map + auto-contrast + efficient ordered
  dither (reusing mono's dither core), with opportunistic PxP offload on Move; color in the
  FSM (static-paint only). *Exit:* color pages render cleanly
  without muddiness; scroll stays mono-fast. (Launch priority alongside mono, built second.)
- **Phase 4 — Viewer UX.** Virtual buttons (placement, actions, orientation anchoring,
  toggle); link drawer + fast-nav; reader mode + stylesheet + controls; TOC/landmark jump;
  frontlight control.
- **Phase 5 — Bookmarks & polish.** Netscape-HTML + Chromium-JSON importers (+ optional
  places.sqlite); hi-fi redraw & sustained hi-fi; capability registry / graceful-degradation
  pass; config UI.
- **RM2 bring-up (Option C, run early as a memory spike then finished late):** armv7 + rm2fb
  + WPE + mono behind features. Do a Phase-0/1 spike to confirm WPE fits 1 GB (gating the
  Option C vs B decision), then complete and harden alongside Phases 4–5.
- **Later (flagged):** content-adaptive per-tile waveforms; per-panel color profiles; TTS
  if hardware supports; additional locales for nav heuristics; GPU offload *only if* a
  future reMarkable ships an ES3.1+ GPU.

---

## 19. Open decisions (confirm at/before the relevant phase)

1. **Engine:** WPE (open, logins, heavy C++) vs Ultralight (best e-ink fit, paid/closed
   ~$3k/yr, no static-link on free tier) vs Blitz (pure Rust, no logins). Default: WPE,
   behind a trait. *If logins are dropped, Blitz makes it end-to-end Rust.* — Phase 2.
2. **RM2 disposition:** *DECIDED — keep via Option C* (WPE on armv7 + rm2fb, mono). Only
   open sub-question: the early 1 GB spike result; fall back to Option B (NetSurf) if WPE
   can't fit. — Phase 0/1 spike.
3. **Color scope:** *DECIDED — color is a launch priority*, but the color renderer is built
   **second**, reusing the mono core (Phase 3). Mono ships first as both a product target
   (Pure, RM2) and the shared foundation.
4. **Login realism:** accept "best-effort, may fail for some providers (fingerprinting/
   passkeys)." — Phase 2.
5. **Refresh constants** — set by the Phase 1 on-device tuning pass.
6. **Bookmark transfer mechanism** (USB drop folder vs Wi-Fi upload). — Phase 5.

---

## 20. Sources

Hardware / lineup: reMarkable support pages (Paper Pro, Pro Move, Pure); `reMarkable/
linux-imx-rm` kernel (codenames `ferrari`/`chiappa`/`tatsu`, `ARCH=arm64`); NXP i.MX8M Mini
(GC NanoUltra, GLES 2.0) & i.MX93 (2D PXP only, no 3D GPU); Engadget/Notebookcheck (Pure);
Good e-Reader & CNX (Gallery 3 / ACeP, refresh times); E Ink Gallery 3 brand page.
Display stack: `libremarkable` (RMPP support), `ddvk/remarkable2-framebuffer` (rm2fb, RM2-
only), remarkablewiki SWTCON, remarkable.guide display, FBInk PR #41 (mono waveform
latencies). Color dithering / Gallery 3: FBInk `mxcfb-kobo.h` (`mxcfb_dithering_mode` enum,
v2 struct) & `mxc_epdc_v2_fb.c` (PxP/CPU dithering, not EPDC-core silicon); NXP community
"i.MX8M Mini has no EPDC"; NXP i.MX93 RM / Linux release notes (PxP-for-EPDC); E Ink
Gallery 3 brand page (CMYW, 8 colors, dithering for shades); **wavexx "Remarkable Pro
Colors"** (host-side auto-contrast→quantize→dither, measured ICC primaries); Modos
Labs Glider/Caster (host-RGB→controller-dither reference architecture). Engines: WPE/WebKitGTK Skia CPU default (Igalia, wpewebkit.org 2.46);
Ultralight ARM64 1.4 + pricing (ultralig.ht); Servo WebRender GLES3 requirement & libservo
software-backend proposals (servo/webrender #3701, servo #18597/#35083); Blitz / anyrender
/ vello_cpu (DioxusLabs, Linebender); CEF OSR software rendering. Features: Netscape
Bookmark format (MS Learn, ArchiveTeam); Chrome JSON / Firefox places.sqlite schemas; Chrome
Sync cutoff & Google Bookmarks shutdown; Opera Fast Forward (smyru/fast-forward, Opera
forums); WHATWG link types / HTML4 vs HTML5 rel (w3.org, WHATWG blog, microformats); Mozilla
Readability; WCAG 1.4.x / 2.5.x; MDN media features.

*(Full URLs captured in the research task transcripts backing this plan.)*
