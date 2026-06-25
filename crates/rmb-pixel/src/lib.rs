//! Pixel pipeline: the seam between the engine's RGBA output and the panel (PLAN.md §8).
//!
//! Mono and colour are deliberately separate renderers (`mono` / `color` modules), sharing
//! only this trait and the surface/format helpers — they differ in bit depth, dithering,
//! and how much work is done in software. Mono ships first (Pure, RM2) and is the proving
//! ground; colour reuses the scaffolding (PLAN.md §3, §8.2).

use rmb_types::{PanelFormat, PanelKind, Rect};

pub mod mono;

/// A borrowed RGBA8 source surface produced by the render engine.
pub struct RgbaSurface<'a> {
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA8, length == width*height*4.
    pub pixels: &'a [u8],
}

impl<'a> RgbaSurface<'a> {
    /// Read the RGBA bytes of pixel (x,y); returns black if out of bounds.
    #[inline]
    pub fn rgba(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 255];
        }
        let i = ((y * self.width + x) * 4) as usize;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]]
    }
}

/// A panel-ready output buffer for a single dirty *region* (not the whole screen).
///
/// `width`/`height` are the region's size; `bytes` is row-major in `format`.
pub struct PanelBuffer {
    pub width: u32,
    pub height: u32,
    pub format: PanelFormat,
    pub bytes: Vec<u8>,
}

impl PanelBuffer {
    /// Allocate a zeroed region buffer of the given size and format.
    pub fn new(width: u32, height: u32, format: PanelFormat) -> Self {
        let len = width as usize * height as usize * format.bytes_per_pixel();
        Self {
            width,
            height,
            format,
            bytes: vec![0; len],
        }
    }
}

/// Converts engine RGBA into panel-ready bytes for a dirty region.
///
/// Implementors: [`mono::MonoPipeline`] (greyscale quantise + dither) and (Phase 3) a
/// `color::ColorPipeline` (gamut/auto-contrast + dither; optional PxP on i.MX93).
pub trait PixelPipeline {
    /// Which panel family this pipeline targets.
    fn panel(&self) -> PanelKind;

    /// Convert the `region` of `src` into `dst` (sized to `region`), returning the region
    /// actually written (clamped to the source bounds).
    fn convert(&mut self, src: &RgbaSurface<'_>, region: Rect, dst: &mut PanelBuffer) -> Rect;
}

pub mod color {
    //! Gallery 3 renderer (Pro, Move). Host does gamut/gamma map to measured primaries,
    //! global auto-contrast, then quantise+dither to the 8-colour palette; the panel does
    //! colour *synthesis* only. Optional PxP offload on i.MX93 (PLAN.md §8.2).
    //!
    //! Scaffold only — to be implemented in Phase 3 (deferred until colour hardware is on
    //! hand; RM2-first development targets the mono path).
}
