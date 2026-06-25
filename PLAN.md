# reMarkable Headless Browser — Research & Implementation Plan

A speed-first, viewer-first web browser for the reMarkable e-ink tablet, written in
Rust, driving the panel through `libremarkable` + `rm2fb`.

> Status: research + planning. No code yet. This document is the spec the
> implementation will be built against. Sections tagged **[COMPLEXITY: …]** flag
> work that is disproportionately hard relative to its size and should be
> scheduled/estimated with care.

---

## 1. Goals & priority order

The whole design is ordered by three emphases, in this strict priority:

1. **Speed through minimal resource use — CPU above all.** The RM2 is a 2× ARM
   Cortex-A7 @ ~1 GHz, 1 GB RAM, **no GPU**. Every architectural choice is judged
   first by CPU/RAM cost. Idle CPU must be ~0%; hot paths prefer integer/fixed-point
   and NEON over naive float.
2. **Responsiveness — input must always feel instantaneous.** The primary lever is
   switching to fast, low-quality e-ink draw modes during rapid redraws (scrolling,
   panning). Because fast mode degrades quality, it must engage **only past a
   measured redraw-rate threshold**, with hysteresis so it never triggers on
   incidental redraws, and must be **suppressed when only text is being redrawn**
   (a cheap bitonal update suffices there without dropping to animation mode).
3. **Quality — get the most out of the panel when not in fast mode.** Software
   dithering to the panel's ~16 grey levels, the best-fit waveform per content
   type, and a user-triggerable **hi-fi redraw** of the current screen. Higher
   fidelity / additional draw modes are researched here and flagged for complexity;
   some land later.

Non-goals (first pass): a complete web platform, banking-grade security, rich
editing, file-system/drag-drop interactions, and most interactive Web APIs. This is
a **viewer**; interaction exists only to enable more viewing.

---

## 2. Target hardware & low-level stack (established facts)

| Property | reMarkable 2 (primary target) | Paper Pro (RMPP, note only) |
|---|---|---|
| SoC | NXP **i.MX7D**, 2× Cortex-A7 @ ~1 GHz, **32-bit ARMv7-A** | quad Cortex-A53 @ 1.8 GHz, **64-bit** |
| GPU | **None** — 100% software rendering | None |
| RAM | **1 GB** LPDDR3 (shared with OS) | 2 GB |
| Storage | 8 GB eMMC | 64 GB |
| Display | 10.3" Carta mono, **1872×1404**, ~226 DPI, ~16 grey levels | 11.8" Gallery 3 **color**, 2160×1620 |
| FP/SIMD | VFPv4 + NEON (present, but low throughput) | ARMv8 NEON (strong) |

**The single most constraining fact: Cortex-A7 is 32-bit ARMv7.** This disqualifies
any engine that is aarch64-only (Ultralight, modern CEF/Chromium, Servo's realistic
tier). Build target is `armv7-unknown-linux-gnueabihf`.

**Display path (`rm2fb`).** On RM2 there is no kernel framebuffer for the panel; the
display is driven by a software TCON inside the `xochitl` process. `rm2fb`
(`ddvk/remarkable2-framebuffer`) recreates the RM1 programming model:

- A **server** (`librm2fb_server.so`) is `LD_PRELOAD`ed into `xochitl`; it owns the
  panel and hooks the SWTCON symbols (hardcoded per firmware build).
- A POSIX shared-memory buffer **`/swtfb.01`** sized `1404 × 1872 × u16` (**RGB565**)
  is the "fake framebuffer."
- Clients write pixels into the shm, then send an `mxcfb_update_data` envelope over a
  **SysV message queue** (`msgsnd`); the server performs the real EPDC update.
- `libremarkable` (Rust, 0.7.x, MSRV 1.80) speaks this protocol natively — it mmaps
  `/swtfb.01` and enqueues updates, exposing `partial_refresh`/`full_refresh`
  (returning an update marker) and `wait_refresh_complete(marker)`. It also decodes
  the three evdev inputs: **multitouch**, **Wacom pen** (pos/pressure/tilt), and
  **buttons/power**. We depend on the rm2fb **server** running (Toltec `display`
  package); we do **not** need the LD_PRELOAD client shim.

**EPDC waveform modes** (i.MX EPDC v1 — note: hardware dither/quant fields forced 0,
so we dither in software). Measured reMarkable latencies:

| Mode | ID | ~Latency | Depth | Use |
|---|---|---|---|---|
| INIT | 0x0 | ~780 ms | — | Full clear/flash; de-ghost |
| **DU** (Direct Update) | 0x1 | **~170 ms** | 1-bit | Any grey → pure B/W. Fast, non-flashy. **Text/UI/caret.** |
| **GC16** | 0x2 | ~460 ms | 4-bit (16) | Full-quality, flashing. **Page render, images. Quality default.** |
| GL16 | 0x3 | ~460 ms | 4-bit | Sparse anti-aliased text on white, reduced flash |
| GLR16 (REAGL/REGAL) | 0x4 | ~460 ms | 4-bit | Anti-aliased text, reduced flash/artifacts |
| GLD16 | 0x5 | ~460 ms | 4-bit | Text+graphics on white, reduced flash |
| **A2** | 0x6 | **~135 ms** | 1-bit | **Fastest.** Bitonal animation/scroll; ghosts → must flush |
| DU4 | 0x7 | ~300 ms | 2-bit (4) | Only non-flashing greyscale; menus |
| AUTO | — | — | — | Controller picks per region |

**Ghosting control:** fast bitonal modes (A2/DU) leave residue. Standard mitigation
is a **periodic full flash** — after N fast updates or on scroll-stop / page change,
issue a `UPDATE_MODE_FULL` GC16 (occasionally INIT) over the affected region. Feeding
a white frame around A2 transitions reduces its ghosting.

---

## 3. Engine selection — the central architectural decision

The hard requirement that drives this choice is **"basic compatibility with login
screens"** (Google / Amazon / Microsoft / Apple). A bare layout engine renders pages
but cannot complete modern SSO, which assumes a full web platform (DOM/CSSOM, `fetch`
with cookies, `crypto.subtle`, increasingly WebAuthn) and fingerprints aggressively.
That requirement, plus the 32-bit/no-GPU/1 GB constraints, narrows the field sharply.

| Engine | Runs on armv7? | No-GPU CPU render? | JS / login | RAM | Rust story | Verdict |
|---|---|---|---|---|---|---|
| **WPE WebKit** | ✅ | ✅ (Skia CPU default for embedded since 2.46) | **JavaScriptCore — full platform** | few×100 MB, tunable caps | FFI to `libwpe` (hand-rolled) | **Recommended engine** |
| **NetSurf** (fb frontend) | ✅ (**already ported to RM2**) | ✅ (own engine, tens of MB) | Duktape ES5, weak DOM — static logins only | very low | C, FFI-wrappable | **Bring-up / fallback** |
| **Blitz** (`blitz-dom` + `anyrender_vello_cpu`) | ✅ | ✅ (Vello CPU, NEON) | **None** (not on roadmap) | moderate | **Rust-native** | **Future / no-login viewer** |
| litehtml | ✅ | ✅ (you supply backend) | None | tiny | C++ FFI | Static docs only |
| Servo / Verso | armv7 least-tested | ✗ swgl not exposed; needs GL | SpiderMonkey (heavy) | 100s MB | unstable 0.0.x | Ruled out |
| Ultralight | ✗ no armv7 | ✅ | JSC | small | crate exists | Ruled out (32-bit) + paid |
| CEF / Chromium | ✗ armv7 abandoned | (OSR) | V8 | ~850 MB | — | Ruled out |

### Recommendation: **Rust application shell + WPE WebKit engine via FFI**, with
**NetSurf as the de-risking bring-up path** and **Blitz as the long-term Rust-native
migration target.**

Rationale: WPE is the *only* modern engine that (a) builds for 32-bit Cortex-A7,
(b) does CPU rendering to an offscreen buffer by design (headless view backend /
`WPE_USE_HEADLESS_VIEW_BACKEND`), (c) has hard RAM caps for a 1 GB box
(`WPE_RAM_SIZE`, `MemoryPressureSettings`, disable page cache), and (d) ships
JavaScriptCore + a real web platform — the only credible path to the login flows the
brief requires. Everything *around* the engine — the display pipeline, refresh-mode
logic, input, virtual buttons, link drawer, reader mode, bookmarks, zoom, UI — is
**pure Rust** and is where the project's value and most of its code live. WebKit is a
replaceable component behind a `RenderEngine` trait.

**Staging the risk.** WPE is a heavy cross-compile with no turnkey Rust binding.
Therefore Phase 0 brings the *whole device pipeline* up against **NetSurf** (proven on
RM2, trivial footprint) to validate display, input, refresh modes, and the UI shell
before the WPE integration is finished. NetSurf then remains a permanent "fast/static"
fallback. If, on review, login compatibility is **dropped** from scope, the
recommendation flips to **Blitz** (Rust-native, modern CSS, CPU/NEON rendering) and
the project becomes pure Rust end-to-end.

> **Decision to confirm (see §17):** WPE (keeps logins, heavy C++ dep) vs. Blitz
> (pure Rust, no logins). Default assumed below: **WPE**, abstracted behind a trait so
> the choice is reversible.

---

## 4. System architecture

Single multi-threaded Rust process. Engine is isolated behind a trait so it can be
swapped (WPE ⇄ NetSurf ⇄ Blitz) without touching the rest.

```
                         ┌──────────────────────────────────────────┐
   evdev (touch/pen/btn) │              rmbrowser (Rust)              │
        ──────────────▶  │                                            │
                         │  input ─▶ gesture/intent ─▶ app state      │
                         │                 │                          │
                         │   ┌─────────────┼───────────────────────┐  │
                         │   │  overlay (virtual buttons, drawers,  │  │
                         │   │  reader UI) — never enters DOM       │  │
                         │   └─────────────┬───────────────────────┘  │
                         │                 ▼                          │
                         │   RenderEngine trait  ◀── nav / zoom / JS  │
                         │     └─ WPE WebKit (FFI)  → RGBA/Y8 surface │
                         │                 │                          │
                         │                 ▼                          │
                         │   compositor: overlay ⊕ page  → dirty rects│
                         │                 ▼                          │
                         │   grayscale + dither (16 lvl)              │
                         │                 ▼                          │
   /swtfb.01 (RGB565) ◀──│   refresh controller  ── waveform FSM ──┐ │
   SysV msgq (mxcfb)  ◀──│   (mode select, ghost flush, hi-fi)     │ │
                         └──────────────────────────────────────────┘
```

### Threads
- **Input thread** — reads evdev via `libremarkable::input`; debounces; emits
  high-level `Intent`s (scroll, tap-link, button-press, pinch). Never blocks.
- **Engine thread(s)** — WebKit's own loop; produces painted surfaces + damage rects.
- **Composite/refresh thread** — owns `/swtfb.01`; merges page + overlay, diffs to
  dirty rects, dithers, writes shm, runs the **waveform FSM**, enqueues updates.
- Lock-free/SPSC channels between them; the composite thread is the only writer of the
  framebuffer. Target: 0% idle CPU, peaks only during real work.

### Workspace crate layout (Rust)
```
crates/
  rmbrowser            app shell, config, lifecycle
  rmb-display          /swtfb.01 + mxcfb wrapper over libremarkable, waveform FSM
  rmb-raster           grayscale convert + dithering (Bayer / Floyd–Steinberg)
  rmb-compositor       page⊕overlay merge, dirty-rect tracking
  rmb-input            evdev → gestures → Intents (scroll, pinch, tap, button)
  rmb-engine           RenderEngine trait + impls (wpe, netsurf, [blitz])
  rmb-engine-wpe       FFI bindings + headless backend glue to libwpe
  rmb-overlay          virtual buttons, link drawer, reader UI, TOC/landmark menu
  rmb-nav              link-drawer scoring, semantic fast-nav, zoom model
  rmb-reader           Readability-style extraction + reader stylesheet
  rmb-bookmarks        Netscape-HTML / Chromium-JSON / places.sqlite importers
  rmb-net              HTTPS/HTTP policy, TLS (rustls), cookie jar
```

---

## 5. Display pipeline & the refresh-mode state machine  *(Emphasis #1 & #2 — the core)*

This is where responsiveness is won or lost. The composite thread runs a small,
explicit state machine that picks the waveform per update from three inputs:
**redraw rate**, **content class of the dirty region**, and **current mode** (for
hysteresis).

### 5.1 Per-update inputs
- **Dirty rects.** Engine damage ∪ overlay damage, coalesced into a few rectangles.
  Refresh cost scales with area, so keep rects tight; never one update per glyph.
- **Content class** of the union region, cheaply classified during raster:
  - `TEXT` — only near-bitonal pixels (text/UI), no mid-greys/images.
  - `MIXED`/`IMAGE` — contains greyscale gradients/images.
- **Redraw rate** — exponential moving average of updates/sec over a sliding window.

### 5.2 Mode selection (the rules the brief asks for)

```
classify(region) + rate(ema) + mode(prev) ─▶ waveform

QUALITY (default, idle/low-rate):
    IMAGE/MIXED → GC16            (full 16-level, dithered)
    TEXT        → GL16 / GLR16    (sharp text, reduced flash)  or DU for tiny edits

FAST (high-rate, e.g. scroll/pan): engage ONLY when rate ≥ ENTER_FAST_HZ
                                   sustained for ENTER_FAST_FRAMES (hysteresis)
    TEXT-only   → DU              (~170 ms, bitonal, NO drop to animation mode)
    MIXED/IMAGE → A2              (~135 ms, fastest; accepts ghosting)

EXIT FAST: when rate < EXIT_FAST_HZ for SETTLE_MS  ─▶  one GC16 "settle" redraw
           of the touched region (restores grey fidelity, clears A2 ghosting)
```

Key behaviors mandated by the brief, made explicit:

- **Threshold + hysteresis to avoid accidental fast mode.** Entering fast mode
  requires the EMA rate to exceed `ENTER_FAST_HZ` for `ENTER_FAST_FRAMES` consecutive
  frames; a single burst (e.g. a tap that repaints twice) never trips it. Exit uses a
  lower threshold + `SETTLE_MS` dwell (classic Schmitt-trigger hysteresis) so it
  doesn't flap at the boundary. All four constants are config-tunable.
- **Text-aware suppression.** If the rapid region is `TEXT`-only, we use **DU**
  (fast bitonal) rather than **A2** (animation). DU keeps crisp edges and avoids the
  contrast loss/ghosting of A2 — so "only text is being redrawn" never pays the
  animation-mode quality penalty. A2 is reserved for genuine mixed/image motion.
- **Ghost budget.** A counter tracks consecutive fast (A2/DU) updates per region.
  At `GHOST_FLUSH_N` updates, or on scroll-stop / navigation, a full-region GC16 (or
  INIT on heavy accumulation) flush runs. White-frame padding wraps A2 transitions.
- **Marker discipline.** We generally fire-and-forget partial updates for latency;
  we `wait_refresh_complete` only before a dependent full flush, to serialize cleanly.

### 5.3 Tunable constants (initial guesses, to be measured on-device)
`ENTER_FAST_HZ ≈ 4`, `ENTER_FAST_FRAMES ≈ 3`, `EXIT_FAST_HZ ≈ 2`, `SETTLE_MS ≈ 350`,
`GHOST_FLUSH_N ≈ 12`. These are starting points; §16 includes an on-device tuning pass
to find the real knees. **[COMPLEXITY: MED]** — the logic is small but the constants
require empirical tuning against perceived latency and ghosting; getting "feels
instant" right is iterative.

---

## 6. Rendering quality: dithering, waveforms & hi-fi  *(Emphasis #3)*

The panel shows ~16 grey levels and the EPDC does **no** hardware dithering, so we
quantize+dither in software before GC16/GL16 updates.

### 6.1 Dithering strategy — two-tier
- **Ordered (Bayer 8×8) dither — default.** Per-pixel, no neighbor dependency →
  trivially SIMD/parallel, cache-friendly, deterministic (stable across partial
  redraws so a region doesn't "shimmer" when re-dithered). Cheapest CPU. Used for
  normal quality updates. **[COMPLEXITY: LOW]**
- **Floyd–Steinberg error diffusion — hi-fi only.** Better tonal gradients but
  serial (error propagates), so more CPU and not stable under partial repaint. Used
  only for the explicit hi-fi redraw / image-heavy full-page render. **[COMPLEXITY:
  MED]** — must be confined to full-region renders to avoid seam artifacts.

A grayscale LUT (sRGB→linear→16-level, gamma-aware) is precomputed once. Luminance is
the only channel that matters; color is mapped by perceptual luminance.

### 6.2 Waveform/draw modes to support out of the gate
Per the brief, these are researched now and prioritized:

| Mode | Phase | Purpose | Complexity |
|---|---|---|---|
| GC16 | P0 | Quality default; images/page | LOW |
| DU | P0 | Fast text/UI; caret | LOW |
| A2 | P1 | Scroll/animation fast mode | MED (ghosting mgmt) |
| GL16 / GLR16 (REAGL) | P1 | Sharp text, reduced flash | MED (naming/behavior varies; per-device test) |
| INIT | P0 | De-ghost full flash | LOW |
| DU4 | P2 | Non-flashing 4-level menus | MED |
| AUTO | P2 | Let EPDC pick (fallback) | LOW |

### 6.3 Hi-fi controls (brief: "switch to higher fidelity, or trigger a hi-fi redraw")
- **Hi-fi redraw of current screen** — a virtual-button/menu action that re-renders
  the visible page at full resolution with Floyd–Steinberg + GC16 full-region. Always
  available; cheap to implement on top of the pipeline. **[COMPLEXITY: LOW]**
- **Sustained hi-fi mode** — disables fast-mode switching entirely (rate threshold →
  ∞) and forces GC16/GL16. Trades responsiveness for fidelity. **[COMPLEXITY: LOW]**
- **Later / flagged:** content-adaptive per-tile mode selection (text tiles GL16,
  image tiles GC16 within one frame); waveform-LUT customization; gamma/contrast
  profiles per panel batch. **[COMPLEXITY: HIGH — deferred]** — requires per-tile
  classification + multi-region update orchestration; noted now, not in first pass.

---

## 7. Input & interaction model

Viewer-first, so the supported gesture set is deliberately tiny:

- **Touch scroll** (pan) — drives the fast-mode path in §5.
- **Pinch zoom** — page content zoom (see §12).
- **Tap** — activate a link / form control / virtual button.
- **Pen** — treated as a precise tap/pointer for link selection; no inking.
- **Power/physical button** — system, passed through.

Explicitly **dropped** (fail gracefully / never wired): drag-and-drop, file-system
interactions, multi-finger gestures beyond pinch, long-press context menus,
`contenteditable` rich editing (text input limited to login/review form fields),
text selection/clipboard beyond what a login form needs. See §14.

---

## 8. Virtual buttons subsystem  *(rmb-overlay)*

On-screen buttons rendered in the overlay layer **above** the page; events are
consumed by the overlay and **never pass into the DOM**.

- **Placement:** anywhere, by `(x%, y%)` of screen + size. Hit-tested before the page.
- **Action (swappable):** `Scroll{amount, dir}` (user-configurable amount — lines,
  screen-fraction, or px), plus future actions `NavNext/Prev` (via link drawer),
  `ZoomIn/Out`, `PageUp/Down`, `ReaderToggle`, `HiFiRedraw`, `OpenDrawer`. Action is a
  trait object so new actions slot in.
- **Toggle on/off** globally and per-button.
- **Orientation anchoring (two modes per button):**
  - `RotateWithScreen` — button rotates and repositions with screen orientation.
  - `FixedScreenPercent` — stays at the same vertical/horizontal % of the physical
    screen regardless of orientation (re-projected on rotation, not rotated).
- Rendered with large hit targets (≥44 px, §10) and high-contrast borders; drawn with
  DU so presses feel instant.

**[COMPLEXITY: MED]** — the orientation-anchoring math and the "consume-before-DOM"
hit-test ordering are the fiddly parts; the rest is straightforward overlay drawing.

---

## 9. Link drawer / semantic fast navigation  *(rmb-nav)*

An Opera-"Fast Forward/Rewind"-style panel that gives **direct navigation without
relying on styling or exact scroll position**. The engine exposes the DOM/links; we
score candidates for a fixed set of navigation **slots**:

`next · prev · up/parent · home/start · contents/index · top · close`

### Scoring (per slot, highest wins above a threshold)
```
score =  +100  rel matches slot      (link rel=next/prev/up/contents/start, <link> or <a>)
         + 40  URL = current URL with page-number incremented/decremented (next/prev)
         + 30  link text / title / aria-label matches slot regex
         + 15  contains directional glyph  » › →  /  « ‹ ←
         + 15  link sits inside <nav> / pagination landmark
         + 10  DOM position (bottom→next, top→prev/up)
         − 50  text matches comment|login|signup|share|tag|print
         − 30  off-origin link
```
- **rel signals (strongest, but thin):** Only `next`, `prev`, `canonical`, `bookmark`,
  `search` are standardized in HTML5/WHATWG; `up`, `contents`, `index`, `start`,
  `first`, `last`, `home` were either HTML4-only or never standardized and were
  **dropped** from the HTML5 registry, so they appear rarely in the wild. Treat
  `previous`→prev and `begin`/`start`→first as synonyms. Additionally, many sites
  dropped even `rel=prev/next` after Google deprecated them (2019). **Conclusion: rel
  alone is insufficient for most slots — the heuristics below are the primary signal
  for up/home/top/contents and a required fallback for next/prev.**
- **Text/glyph regex** (from Mozilla Readability, extend per locale):
  - next: `/(next|continue|weiter|older|>([^|]|$)|»([^|]|$))/i`
  - prev: `/(prev|previous|earl|old|new|<|«)/i`
- **URL number-delta:** same path differing only in a trailing/`?page=` integer.
- **ARIA landmarks** (`nav`, `main`, pagination containers) scope where heuristics
  run, cutting false positives.

The drawer lists the resolved slots as big tappable targets; mapping also powers
single-press virtual-button nav actions and Fast-Forward/Rewind. **[COMPLEXITY: MED]**
— scoring is simple; reliably extracting links + rel + landmarks from the engine
(WebKit DOM via injected JS / JSC) is the real work, and locale coverage of the text
heuristics is a known weak spot.

---

## 10. Reader mode & accessibility  *(rmb-reader, rmb-overlay)*

Reader mode is the single highest-value feature for slow e-ink (less DOM, reflow, and
repaint) and doubles as the accessibility backbone.

- **Extraction:** Mozilla **Readability** algorithm (clone DOM first; it mutates).
  Produces title/byline/content/text/lang/site. **Default *into* reader mode** when
  `isProbablyReaderable` is true (viewer-first). **[COMPLEXITY: MED]** — porting/
  embedding Readability (JS via JSC, or a Rust reimplementation) + handling messy
  real-world pages.
- **Reader stylesheet (WCAG-grounded):** black-on-white ≥7:1 (AAA), system serif/sans
  toggle, line-height ≥1.5, **ragged-right (never justify)**, measure ~66–80 ch,
  generous margins.
- **User controls:** font size stepper (to 200%), family, line spacing, margins,
  **invert** (default off — large black fills worsen ghosting), text-only (drop
  images) mode.
- **Jump-don't-scroll navigation** (scrolling is expensive on e-ink): a **heading TOC**
  (h1–h6) and **landmark menu** (`main`/`nav`/`aside`/`header`/`footer`/`search`) that
  anchor-jump. Shares machinery with the link drawer.
- **Large touch targets:** ≥44×44 px (WCAG 2.5.5) via padding; numbered-link mode for
  precise selection without mis-taps.
- **Honor media features:** `prefers-reduced-motion` (default reduce), `-color-scheme`,
  `-contrast`, `forced-colors`.
- **TTS:** out of scope first pass; capability-gate `speechSynthesis` and expose only
  if the device actually has voices. Flagged, not built.

### Accessibility-friendly features to *engage*
Reader extraction, reflow/resize, high-contrast defaults, heading/landmark jump,
numbered links + large targets, reduced-motion enforcement.

### Modern-web features to *drop* (no benefit to barebones e-ink, all cost)
Animations/transitions/scroll effects, autoplay video & animated GIF, heavy webfonts
(prefer system fonts), JS-heavy SPA churn (reader sidesteps), ads/trackers/analytics,
infinite-scroll/lazy-on-scroll (load article fully up front). These are disabled at
the engine/network layer where possible.

---

## 11. Bookmarks import  *(rmb-bookmarks)*

**There is no usable third-party cloud bookmark API** from Google, Mozilla, Microsoft,
or Apple (Chrome Sync is closed since 2021; Firefox Sync is E2E-encrypted behind a
partner-only key protocol; Edge/iCloud have none; Google Bookmarks shut down 2021). So
import is file-based, transferred to the device over USB/Wi-Fi.

Priority order:
1. **Netscape Bookmark HTML importer (primary).** The universal export format every
   browser writes ("Export bookmarks to HTML"). One tolerant parser covers
   Chrome/Firefox/Edge/Safari/Brave/IE. Walk the `<DL>` tree: `<DT><H3>` = folder
   (descend into following `<DL>`), `<DT><A HREF>` = bookmark, `<DD>` = description;
   `ADD_DATE`/`LAST_VISIT`/`LAST_MODIFIED` are **Unix seconds**; tolerate unclosed
   `<DT>`/`<DD>`. **[COMPLEXITY: LOW]**
2. **Chromium JSON reader (Chrome + Edge + Brave, one path).** Parse the profile
   `Bookmarks` JSON: roots `bookmark_bar`/`other`/`synced`, nodes `{type, name, url,
   children[]}`; timestamps are **microseconds since 1601** (FILETIME). Plaintext
   today; tolerate missing/encrypted file. **[COMPLEXITY: LOW]**
3. **Firefox `places.sqlite` reader (optional).** `SELECT b.title, h.url FROM
   moz_bookmarks b JOIN moz_places h ON h.id=b.fk`; reconstruct folders via
   `moz_bookmarks.parent`; timestamps **microseconds since 1970**; copy DB first (WAL
   lock). **[COMPLEXITY: MED]** — needs a SQLite dep (`rusqlite`).
4. **Safari `Bookmarks.plist`** — skip unless demanded (binary plist, macOS-only, low
   ROI).

No cloud sync. For "cloud" sources the supported equivalent is a user-run HTML export
(or Google Takeout) fed into path 1.

---

## 12. Browser-level zoom  *(rmb-nav)*

Independent of pinch zoom, to avoid re-pinching every page.

- **Persistent page zoom factor** (e.g. 50%–300%), applied via the engine's zoom/text
  factor (WebKit `webkit_web_view_set_zoom_level`), default global + optional per-host
  override, persisted in config.
- **Pinch** adjusts the same factor live (driving fast-mode during the gesture; GC16
  settle on release).
- Reader-mode font scaling (§10) is a separate, additive control.

**[COMPLEXITY: LOW]** — mostly plumbing the engine's existing zoom API + persistence.

---

## 13. Networking & security posture  *(rmb-net)*

First pass is explicitly **not** banking-grade and **blocks requests for advanced
security**, while supporting enough to sign in to Google/Amazon/Microsoft/Apple.

- **HTTPS by default, HTTP supported** for compatibility (per current standards;
  HTTP upgraded where possible, not blocked).
- **TLS** via the engine's stack or `rustls` for our own fetches; modern cipher suites
  only; standard CA bundle.
- **Authentication scope:** cookies, form POST, OAuth redirects, and basic WebCrypto
  needed for those flows. **Block / refuse** advanced mechanisms beyond that —
  WebAuthn/passkey hardware authenticators, client-certificate / smartcard auth,
  payment APIs — returning a graceful "not supported" rather than partial, risky
  implementations. Fail closed on anything we don't implement.
- **No** elaborate crypto subsystem, secure enclave emulation, DRM/EME, or WebRTC.
- Privacy/perf wins for free: block ads/trackers at the network layer (also a §10
  quality win).

**[COMPLEXITY: MED]** — the engine brings most TLS/cookie machinery; our work is the
**policy layer** (what to allow/deny/upgrade) and making refusals graceful.

---

## 14. Feature scope: included / dropped / graceful degradation

**Included (viewer essentials):** HTML/CSS rendering, modest JS (login/forms), links &
basic forms, scroll/pinch/tap/pen-tap, page zoom, reader mode, link drawer, virtual
buttons, bookmark import, HTTPS/HTTP.

**Dropped first pass (and the rule for each = *fail gracefully*):**
drag-and-drop, File System Access, most input APIs beyond basic form fields
(`contenteditable` rich edit, IME niceties), WebGL/WebGPU, WebRTC, push notifications,
service workers/PWA install, Web Bluetooth/USB/Serial/MIDI, geolocation, camera/mic,
DRM/EME, payment, WebAuthn/passkeys (§13), autoplay media, heavy animations.

**Graceful-degradation contract:** every dropped capability is **feature-detectable as
absent** (the JS API is undefined or returns a clean rejection/`NotSupportedError`),
never a crash or hang. A central capability registry defines, per API:
`{ Absent | Stub-reject | Static-fallback }`. Pages that probe-and-adapt continue to
work; pages that hard-require a dropped API show a clear, non-fatal notice.

---

## 15. Complexity & risk register

| Item | Complexity | Risk | Notes / mitigation |
|---|---|---|---|
| WPE WebKit cross-compile for armv7 + headless backend + Rust FFI | **HIGH** | **HIGH** | No turnkey binding; heavy build. Mitigate: NetSurf bring-up first; isolate behind `RenderEngine` trait. |
| RAM fit in 1 GB with WebKit + our buffers | HIGH | HIGH | Tune `WPE_RAM_SIZE`, MemoryPressureSettings, disable page cache; one tab; cap image decode. |
| Refresh-mode FSM "feels instant" tuning | MED | MED | Empirical on-device tuning pass (§16); hysteresis constants. |
| A2/DU ghosting management | MED | MED | Ghost budget + GC16/INIT flush + white-frame padding. |
| Floyd–Steinberg confined to full-region | MED | LOW | Ordered dither for partials; FS only on hi-fi/full. |
| Readability extraction on messy pages | MED | MED | Reuse Mozilla algorithm; default-in via isProbablyReaderable. |
| Link extraction (rel/landmarks/text) from engine | MED | MED | Injected JS / JSC DOM walk; locale-limited text heuristics. |
| Virtual-button orientation anchoring + event capture | MED | LOW | Clear hit-test ordering; re-projection math. |
| Bookmark importers | LOW–MED | LOW | HTML first; Chromium JSON; optional sqlite. |
| Login compatibility actually working (fingerprinting/passkeys) | — | **HIGH** | Even with JSC, big-provider SSO may resist; manage expectations; refuse advanced auth gracefully. |
| Content-adaptive per-tile waveform (deferred) | HIGH | — | Explicitly out of first pass. |

---

## 16. Phased roadmap

- **Phase 0 — Device pipeline bring-up (de-risk).**
  `rmb-display` over `libremarkable`/`rm2fb`; render test patterns; implement
  grayscale + ordered dither; wire **NetSurf** behind `RenderEngine` to get a real
  page on the panel; basic touch scroll. Implements GC16/DU/INIT. *Exit:* a page
  scrolls on-device.
- **Phase 1 — Responsiveness core.**
  Dirty-rect compositor; content classifier; **refresh-mode FSM** with A2/DU fast
  path, hysteresis, text-suppression; ghost-flush; on-device **tuning pass** for the
  §5.3 constants. GL16/GLR16. *Exit:* scrolling feels instant; no accidental fast-mode.
- **Phase 2 — WPE integration.**
  Cross-compile WPE for armv7; headless backend → RGBA surface → our compositor; RAM
  caps; cookies/TLS policy (`rmb-net`); zoom; basic forms/login. *Exit:* a real login
  page renders and submits.
- **Phase 3 — Viewer UX.**
  Virtual buttons (placement, actions, orientation anchoring, toggle); link drawer +
  fast-nav scoring; reader mode + reader stylesheet + controls; TOC/landmark jump.
- **Phase 4 — Bookmarks & polish.**
  Netscape HTML + Chromium JSON importers (+ optional places.sqlite); hi-fi redraw &
  sustained hi-fi mode; capability registry / graceful-degradation pass; config UI.
- **Later (flagged):** content-adaptive per-tile waveforms, sustained-hifi presets,
  TTS (if hardware supports), Paper Pro / color stack, additional locales for nav
  heuristics.

---

## 17. Open decisions (need confirmation before/at Phase 2)

1. **Engine: WPE WebKit vs. Blitz.** WPE keeps **login compatibility** at the cost of
   a heavy C++ dependency and a hard cross-compile; Blitz is **pure Rust** and modern
   CSS but has **no JS / no logins**. Plan defaults to WPE behind a trait. *If logins
   can be dropped, switch the default to Blitz and the project is end-to-end Rust.*
2. **Login scope realism.** Even with JavaScriptCore, Google/Microsoft/Apple SSO may
   resist a non-mainstream browser (fingerprinting, passkeys). Confirm we accept
   "best-effort, may fail for some providers."
3. **Refresh-mode default constants** — to be set by the Phase 1 on-device tuning pass;
   confirm the responsiveness/quality balance then.
4. **Bookmark transfer mechanism** to the device (USB mass-storage drop folder vs.
   Wi-Fi upload) — pick one for Phase 4.

---

## 18. Sources

Hardware / display stack: i.MX7D & RM2 specs (cnx-software, remarkable support,
NXP, Wikipedia i.MX); `libremarkable` (github canselcik, docs.rs);
`rm2fb`/SWTCON/`/swtfb.01` (github ddvk/remarkable2-framebuffer, timower/rM2-stuff);
waveform-mode latencies (NiLuJe/FBInk PR #41); E Ink waveform reference (waveshare).

Engines: WPE/WebKitGTK Skia CPU rendering (Igalia blog 2025, webkitgtk.org 2.46);
NetSurf on RM2 (alex0809/netsurf-reMarkable, akselmo.dev, HN); Blitz / anyrender /
vello_cpu (DioxusLabs/blitz, linebender); Servo status (Igalia, meta-servo); Ultralight
(ultralig.ht); litehtml; lightpanda.

Features: Netscape Bookmark format (Microsoft Learn, ArchiveTeam); Chrome JSON /
Firefox places.sqlite schemas; Chrome Sync cutoff & Google Bookmarks shutdown (9to5Google,
BleepingComputer); Opera Fast Forward (smyru/fast-forward, Opera forums); WHATWG link
types / MDN rel; Mozilla Readability regexes & algorithm (github mozilla/readability);
WCAG 1.4.3/1.4.6/1.4.8/1.4.10/1.4.12, 2.2.2, 2.5.5/2.5.8 (w3.org); MDN media features.

*(Full URLs captured in the research task transcripts backing this plan.)*
