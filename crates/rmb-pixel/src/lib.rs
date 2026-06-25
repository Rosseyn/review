//! Pixel pipeline: the seam between the engine's RGBA output and the panel (PLAN.md §8).
//!
//! Mono and colour are deliberately separate renderers (`mono` / `color` modules), sharing
//! only this trait and the surface/LUT helpers — they differ in bit depth, dithering, and
//! how much work is done in software. Mono ships first and is the proving ground; colour
//! reuses the scaffolding (PLAN.md §3, §8.2).

use rmb_types::{PanelKind, Rect};

/// A borrowed RGBA8 source surface produced by the render engine.
pub struct RgbaSurface<'a> {
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA8, length == width*height*4.
    pub pixels: &'a [u8],
}

/// A panel-ready output buffer (format owned by the concrete pipeline: packed Y or indexed).
pub struct PanelBuffer {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

/// Converts engine RGBA into panel-ready bytes for a dirty region.
///
/// Implementors: `mono::MonoPipeline` (greyscale quantise + dither) and
/// `color::ColorPipeline` (gamut/auto-contrast + dither; optional PxP on i.MX93).
pub trait PixelPipeline {
    /// Which panel family this pipeline targets.
    fn panel(&self) -> PanelKind;

    /// Convert `src` within `dirty` into `dst`, returning the region actually written.
    fn convert(&mut self, src: &RgbaSurface<'_>, dirty: Rect, dst: &mut PanelBuffer) -> Rect;
}

pub mod mono {
    //! Greyscale renderer for Carta panels (Pure, RM2). Ordered (Bayer) dither by default;
    //! Floyd–Steinberg reserved for hi-fi full-region renders (PLAN.md §8.1).
    //!
    //! Scaffold only — quantisation/dither to be implemented in Phase 0/1.
}

pub mod color {
    //! Gallery 3 renderer (Pro, Move). Host does gamut/gamma map to measured primaries,
    //! global auto-contrast, then quantise+dither to the 8-colour palette; the panel does
    //! colour *synthesis* only. Optional PxP offload on i.MX93 (PLAN.md §8.2).
    //!
    //! Scaffold only — to be implemented in Phase 3.
}
