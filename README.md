# rmbrowser

A speed-first, viewer-first web browser for the reMarkable e-ink lineup (Paper Pro,
Paper Pro Move, Paper Pure primary; reMarkable 2 as a degraded build), written in Rust.

See **[PLAN.md](PLAN.md)** for the full research and design — hardware targets, engine
selection, the refresh-mode state machine, the colour pipeline, and the feature roadmap.

## Status

Early scaffold. The architecture from PLAN.md §6 is laid out as a Cargo workspace, with
the **pure-logic, hardware-independent pieces implemented and unit-tested**. Device I/O
(the WPE engine, the EPDC/rm2fb display backends, the pixel renderers) are trait stubs to
be filled in Phases 0–3.

```
cargo test --workspace     # 21 tests across the implemented crates
cargo run -p rmbrowser      # smoke test: wires the seams together, no device needed
```

## Workspace layout

| Crate | Role | State |
|---|---|---|
| `rmb-types` | shared geometry + device/panel types | implemented |
| `rmb-display` | waveform modes + **refresh-mode FSM** (PLAN §7) | implemented + tested |
| `rmb-nav` | **link-drawer scoring** / fast-nav (PLAN §11) | implemented + tested |
| `rmb-overlay` | **virtual-button orientation anchoring** (PLAN §10) | implemented + tested |
| `rmb-bookmarks` | **Netscape HTML bookmark parser** (PLAN §13) | implemented + tested |
| `rmb-pixel` | `PixelPipeline` trait + mono/colour renderer modules (PLAN §8) | trait + scaffold |
| `rmb-hal` | `DisplayBackend` trait: mxcfb-direct / rm2fb (PLAN §4) | trait + scaffold |
| `rmb-engine` | `RenderEngine` trait: WPE / NetSurf / Blitz (PLAN §5) | trait + scaffold |
| `rmb-input` | evdev → high-level `Intent`s (PLAN §9) | types |
| `rmbrowser` | application shell / entry point | smoke test |

The three abstraction seams — `RenderEngine`, `DisplayBackend`, `PixelPipeline` — isolate
every engine/hardware difference so one aarch64 binary serves the three primary devices
and the RM2 build differs only at the seams (PLAN §3).

## License

Apache-2.0 (see [LICENSE](LICENSE)).
