# rmbrowser

A speed-first, viewer-first web browser for the reMarkable e-ink lineup (Paper Pro,
Paper Pro Move, Paper Pure primary; reMarkable 2 as a degraded build), written in Rust.

See **[PLAN.md](PLAN.md)** for the full research and design — hardware targets, engine
selection, the refresh-mode state machine, the colour pipeline, and the feature roadmap.

## Status

Early scaffold, **targeting the reMarkable 2 first** (the available hardware): mono Carta
panel, armv7, rm2fb display path. The architecture from PLAN.md §6 is laid out as a Cargo
workspace; the pure-logic and host-testable pieces are implemented and unit-tested,
including an **end-to-end mono render path** (RGBA → dither → blit → submit). The **rm2fb
device backend is implemented** against `libremarkable` 0.7 behind the `device` feature and
type-checks for the RM2; final on-glass validation happens on hardware. See
**[DEVICE.md](DEVICE.md)** for building and deploying to the tablet. The WPE engine is the
remaining device-facing piece.

```
cargo test --workspace     # 29 tests across the implemented crates
cargo run -p rmbrowser      # RM2 smoke test on host: mono render + FSM + nav + bookmarks
cargo check -p rmb-hal --features device   # type-check the rm2fb backend vs libremarkable
```

## Workspace layout

| Crate | Role | State |
|---|---|---|
| `rmb-types` | shared geometry + device/panel types | implemented |
| `rmb-display` | waveform modes + **refresh-mode FSM** (PLAN §7) | implemented + tested |
| `rmb-nav` | **link-drawer scoring** / fast-nav (PLAN §11) | implemented + tested |
| `rmb-overlay` | **virtual-button orientation anchoring** (PLAN §10) | implemented + tested |
| `rmb-bookmarks` | **Netscape HTML bookmark parser** (PLAN §13) | implemented + tested |
| `rmb-pixel` | `PixelPipeline` trait + **mono renderer** (gamma LUT, ordered + Floyd–Steinberg dither, RGB565/Y8 pack) (PLAN §8.1) | mono done + tested; colour scaffold |
| `rmb-hal` | `DisplayBackend` trait + **MemoryBackend** + **mxcfb mapping** + **rm2fb backend** (libremarkable, `device` feature) (PLAN §4) | host + protocol tested; rm2fb impl type-checks vs libremarkable; mxcfb-direct scaffold |
| `rmb-engine` | `RenderEngine` trait: WPE / NetSurf / Blitz (PLAN §5) | trait + scaffold |
| `rmb-input` | evdev → high-level `Intent`s (PLAN §9) | types |
| `rmbrowser` | application shell / entry point | smoke test |

The three abstraction seams — `RenderEngine`, `DisplayBackend`, `PixelPipeline` — isolate
every engine/hardware difference so one aarch64 binary serves the three primary devices
and the RM2 build differs only at the seams (PLAN §3).

## License

Apache-2.0 (see [LICENSE](LICENSE)).
