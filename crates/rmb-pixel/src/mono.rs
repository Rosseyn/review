//! Greyscale renderer for Carta panels (Pure, RM2) — PLAN.md §8.1.
//!
//! Pipeline: RGBA → gamma-correct linear luminance → dither to the panel's ~16 grey
//! levels → pack to the framebuffer format. Two tiers: ordered (Bayer 8×8) by default —
//! O(1)/pixel, NEON-friendly, and *phase-stable* across partial redraws so re-dithering a
//! region doesn't shimmer; Floyd–Steinberg for hi-fi full-region renders.

use crate::{PanelBuffer, PixelPipeline, RgbaSurface};
use rmb_types::{PanelFormat, PanelKind, Rect};

/// Number of grey levels the panel resolves.
const LEVELS: i32 = 16;

/// Standard Bayer 8×8 ordered-dither matrix, values 0..=63.
#[rustfmt::skip]
const BAYER8: [[u8; 8]; 8] = [
    [ 0, 48, 12, 60,  3, 51, 15, 63],
    [32, 16, 44, 28, 35, 19, 47, 31],
    [ 8, 56,  4, 52, 11, 59,  7, 55],
    [40, 24, 36, 20, 43, 27, 39, 23],
    [ 2, 50, 14, 62,  1, 49, 13, 61],
    [34, 18, 46, 30, 33, 17, 45, 29],
    [10, 58,  6, 54,  9, 57,  5, 53],
    [42, 26, 38, 22, 41, 25, 37, 21],
];

/// Which dither tier to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DitherMode {
    /// Ordered Bayer — cheap, phase-stable. Default for normal updates.
    Ordered,
    /// Floyd–Steinberg error diffusion — hi-fi, full-region only.
    FloydSteinberg,
}

/// The mono pixel pipeline.
pub struct MonoPipeline {
    format: PanelFormat,
    dither: DitherMode,
    /// sRGB byte → linear-light byte, precomputed once.
    srgb_to_linear: [u8; 256],
}

impl MonoPipeline {
    pub fn new(format: PanelFormat) -> Self {
        let mut srgb_to_linear = [0u8; 256];
        for (i, slot) in srgb_to_linear.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            let lin = if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
            *slot = (lin * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        Self {
            format,
            dither: DitherMode::Ordered,
            srgb_to_linear,
        }
    }

    /// Switch dither tier (e.g. `FloydSteinberg` for a hi-fi redraw).
    pub fn with_dither(mut self, dither: DitherMode) -> Self {
        self.dither = dither;
        self
    }

    pub fn dither(&self) -> DitherMode {
        self.dither
    }

    /// Rec.709 luminance in linear light, 0..=255.
    pub fn luminance(&self, rgba: [u8; 4]) -> u8 {
        let r = self.srgb_to_linear[rgba[0] as usize] as u32;
        let g = self.srgb_to_linear[rgba[1] as usize] as u32;
        let b = self.srgb_to_linear[rgba[2] as usize] as u32;
        // 0.2126, 0.7152, 0.0722 scaled by 256 → 54, 183, 19 (sum 256).
        ((54 * r + 183 * g + 19 * b) >> 8) as u8
    }

    #[inline]
    fn grey_for_level(level: i32) -> u8 {
        // 0..15 → 0..255 (17*15 = 255).
        (level * 17).clamp(0, 255) as u8
    }

    /// Ordered quantisation of a linear value to a grey byte, using absolute screen coords
    /// for the Bayer phase (stable across partial redraws).
    #[inline]
    fn quantize_ordered(&self, lin: u8, sx: u32, sy: u32) -> u8 {
        let num = lin as i32 * (LEVELS - 1); // 0..3825
        let low = num / 255; // floor level
        let frac = num - low * 255; // 0..254
        let thr = BAYER8[(sy & 7) as usize][(sx & 7) as usize] as i32 * 255 / 64;
        let level = if frac > thr { low + 1 } else { low };
        Self::grey_for_level(level.clamp(0, LEVELS - 1))
    }

    fn convert_ordered(&self, src: &RgbaSurface<'_>, region: Rect, dst: &mut PanelBuffer) {
        let bpp = self.format.bytes_per_pixel();
        for ry in 0..region.h {
            for rx in 0..region.w {
                let sx = region.x as u32 + rx;
                let sy = region.y as u32 + ry;
                let lin = self.luminance(src.rgba(sx, sy));
                let grey = self.quantize_ordered(lin, sx, sy);
                let di = ((ry * region.w + rx) as usize) * bpp;
                let packed = self.format.pack_grey(grey);
                dst.bytes[di..di + bpp].copy_from_slice(&packed[..bpp]);
            }
        }
    }

    fn convert_floyd(&self, src: &RgbaSurface<'_>, region: Rect, dst: &mut PanelBuffer) {
        let bpp = self.format.bytes_per_pixel();
        let w = region.w as usize;
        let h = region.h as usize;
        // High-precision (i32) working buffer of linear luminance to avoid banding.
        let mut buf = vec![0i32; w * h];
        for ry in 0..h {
            for rx in 0..w {
                let lin = self.luminance(src.rgba(region.x as u32 + rx as u32, region.y as u32 + ry as u32));
                buf[ry * w + rx] = lin as i32;
            }
        }
        for ry in 0..h {
            // Serpentine scan reduces directional artefacts.
            let l2r = ry % 2 == 0;
            for k in 0..w {
                let rx = if l2r { k } else { w - 1 - k };
                let old = buf[ry * w + rx].clamp(0, 255);
                let level = (old * (LEVELS - 1) + 127) / 255;
                let grey = Self::grey_for_level(level);
                let err = old - grey as i32;
                // Distribute error to neighbours (7/16, 3/16, 5/16, 1/16).
                let fwd = if l2r { 1i32 } else { -1i32 };
                let mut spread = |dx: i32, dy: usize, num: i32| {
                    let nx = rx as i32 + dx * fwd;
                    let ny = ry + dy;
                    if nx >= 0 && (nx as usize) < w && ny < h {
                        buf[ny * w + nx as usize] += err * num / 16;
                    }
                };
                spread(1, 0, 7);
                spread(-1, 1, 3);
                spread(0, 1, 5);
                spread(1, 1, 1);

                let di = (ry * w + rx) * bpp;
                let packed = self.format.pack_grey(grey);
                dst.bytes[di..di + bpp].copy_from_slice(&packed[..bpp]);
            }
        }
    }
}

impl PixelPipeline for MonoPipeline {
    fn panel(&self) -> PanelKind {
        PanelKind::Mono
    }

    fn convert(&mut self, src: &RgbaSurface<'_>, region: Rect, dst: &mut PanelBuffer) -> Rect {
        debug_assert_eq!(dst.width, region.w);
        debug_assert_eq!(dst.height, region.h);
        match self.dither {
            DitherMode::Ordered => self.convert_ordered(src, region, dst),
            DitherMode::FloydSteinberg => self.convert_floyd(src, region, dst),
        }
        // Region actually written, clamped to the source bounds.
        let x1 = (region.x as u32 + region.w).min(src.width);
        let y1 = (region.y as u32 + region.h).min(src.height);
        Rect::new(
            region.x,
            region.y,
            x1.saturating_sub(region.x as u32),
            y1.saturating_sub(region.y as u32),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            v.extend_from_slice(&rgba);
        }
        v
    }

    fn convert_full(pipe: &mut MonoPipeline, w: u32, h: u32, pixels: &[u8]) -> PanelBuffer {
        let src = RgbaSurface { width: w, height: h, pixels };
        let region = Rect::new(0, 0, w, h);
        let mut dst = PanelBuffer::new(w, h, pipe.format);
        pipe.convert(&src, region, &mut dst);
        dst
    }

    #[test]
    fn white_and_black_are_extremes_rgb565() {
        let mut pipe = MonoPipeline::new(PanelFormat::Rgb565);
        let white = convert_full(&mut pipe, 4, 4, &solid(4, 4, [255, 255, 255, 255]));
        let black = convert_full(&mut pipe, 4, 4, &solid(4, 4, [0, 0, 0, 255]));
        assert!(white.bytes.iter().all(|&b| b == 0xFF));
        assert!(black.bytes.iter().all(|&b| b == 0x00));
    }

    #[test]
    fn y8_white_is_255_black_is_0() {
        let mut pipe = MonoPipeline::new(PanelFormat::Y8);
        let white = convert_full(&mut pipe, 2, 2, &solid(2, 2, [255, 255, 255, 255]));
        let black = convert_full(&mut pipe, 2, 2, &solid(2, 2, [0, 0, 0, 255]));
        assert!(white.bytes.iter().all(|&b| b == 255));
        assert!(black.bytes.iter().all(|&b| b == 0));
    }

    #[test]
    fn ordered_dither_uses_multiple_levels_on_gradient() {
        let mut pipe = MonoPipeline::new(PanelFormat::Y8);
        let (w, h) = (64u32, 8u32);
        // Horizontal sRGB ramp.
        let mut px = Vec::new();
        for _y in 0..h {
            for x in 0..w {
                let v = (x * 255 / (w - 1)) as u8;
                px.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let out = convert_full(&mut pipe, w, h, &px);
        let mut levels: Vec<u8> = out.bytes.clone();
        levels.sort_unstable();
        levels.dedup();
        assert!(levels.len() >= 4, "expected several grey levels, got {}", levels.len());
    }

    #[test]
    fn ordered_is_deterministic() {
        let mut pipe = MonoPipeline::new(PanelFormat::Y8);
        let px = solid(8, 8, [120, 120, 120, 255]);
        let a = convert_full(&mut pipe, 8, 8, &px);
        let b = convert_full(&mut pipe, 8, 8, &px);
        assert_eq!(a.bytes, b.bytes);
    }

    #[test]
    fn floyd_preserves_average() {
        let mut pipe = MonoPipeline::new(PanelFormat::Y8).with_dither(DitherMode::FloydSteinberg);
        let (w, h) = (32u32, 32u32);
        let px = solid(w, h, [128, 128, 128, 255]);
        let expected = pipe.luminance([128, 128, 128, 255]) as f64;
        let out = convert_full(&mut pipe, w, h, &px);
        let avg = out.bytes.iter().map(|&b| b as f64).sum::<f64>() / out.bytes.len() as f64;
        assert!((avg - expected).abs() < 6.0, "avg {avg} vs expected {expected}");
    }
}
