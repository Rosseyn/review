//! reMarkable 2 display backend over rm2fb, via `libremarkable` 0.7 (PLAN.md §4).
//!
//! `libremarkable::Framebuffer::new()` autodetects the device and, on the RM2, talks to the
//! rm2fb server natively over the `/dev/shm/swtfb.01` shared memory + SysV message queue —
//! no LD_PRELOAD client shim needed (the rm2fb *server*, from Toltec's `display` package,
//! must be running; see DEVICE.md). We map our format-agnostic `DisplayBackend` onto it:
//! `blit` → `restore_region` (RGB565 byte block into a rect), `submit` → `partial_refresh`,
//! `wait` → `wait_refresh_complete`.

use libremarkable::framebuffer::common::{
    dither_mode, display_temp, mxcfb_rect, waveform_mode, DRAWING_QUANT_BIT,
};
use libremarkable::framebuffer::core::Framebuffer;
use libremarkable::framebuffer::{FramebufferIO, FramebufferRefresh, PartialRefreshMode};

use rmb_display::{Waveform, WaveformMode};
use rmb_types::{PanelFormat, Rect};

use crate::{DisplayBackend, PanelInfo, UpdateMarker, UpdateRequest};

/// The RM2 rm2fb-backed display.
pub struct Rm2Backend {
    fb: Framebuffer,
    info: PanelInfo,
}

impl Rm2Backend {
    /// Open the panel. Panics (per libremarkable) if the framebuffer can't be acquired —
    /// e.g. the rm2fb server isn't running.
    pub fn new() -> Self {
        let fb = Framebuffer::new();
        let info = PanelInfo {
            width: fb.var_screen_info.xres,
            height: fb.var_screen_info.yres,
            format: PanelFormat::Rgb565,
        };
        Self { fb, info }
    }
}

impl Default for Rm2Backend {
    fn default() -> Self {
        Self::new()
    }
}

fn to_mxcfb_rect(r: Rect) -> mxcfb_rect {
    mxcfb_rect {
        top: r.y.max(0) as u32,
        left: r.x.max(0) as u32,
        width: r.w,
        height: r.h,
    }
}

/// Map our waveform to a libremarkable `waveform_mode` plus its `(dither, quant_bit)`.
///
/// Only the constants confirmed present in libremarkable 0.7 are referenced. `A2` is not
/// exposed as a named constant there, so it falls back to `DU` (the safe fast bitonal mode)
/// until validated on-device; `Du4` maps to `GC16_FAST`.
fn map_waveform(w: Waveform) -> (waveform_mode, dither_mode, i32) {
    let mono = match w {
        Waveform::Mono(m) => m,
        // Colour never occurs on the RM2 (mono panel); pick a safe quality mode.
        Waveform::Color(_) => WaveformMode::Gc16,
    };
    match mono {
        WaveformMode::Init => (
            waveform_mode::WAVEFORM_MODE_INIT,
            dither_mode::EPDC_FLAG_USE_DITHERING_PASSTHROUGH,
            0,
        ),
        WaveformMode::Du | WaveformMode::A2 => (
            waveform_mode::WAVEFORM_MODE_DU,
            dither_mode::EPDC_FLAG_EXP1,
            DRAWING_QUANT_BIT,
        ),
        WaveformMode::Gc16 => (
            waveform_mode::WAVEFORM_MODE_GC16,
            dither_mode::EPDC_FLAG_USE_DITHERING_PASSTHROUGH,
            0,
        ),
        WaveformMode::Gl16 | WaveformMode::Du4 => (
            waveform_mode::WAVEFORM_MODE_GC16_FAST,
            dither_mode::EPDC_FLAG_USE_DITHERING_PASSTHROUGH,
            0,
        ),
        WaveformMode::Glr16 => (
            waveform_mode::WAVEFORM_MODE_GLR16,
            dither_mode::EPDC_FLAG_USE_DITHERING_PASSTHROUGH,
            0,
        ),
    }
}

impl DisplayBackend for Rm2Backend {
    fn panel_info(&self) -> &PanelInfo {
        &self.info
    }

    fn blit(&mut self, region: Rect, bytes: &[u8]) {
        // `restore_region` writes a width*height*2 RGB565-LE block into the rect, honoring
        // the framebuffer's line_length internally.
        let _ = self.fb.restore_region(to_mxcfb_rect(region), bytes);
    }

    fn submit(&mut self, req: &UpdateRequest) -> UpdateMarker {
        let rect = to_mxcfb_rect(req.region);
        let (wf, dither, quant) = map_waveform(req.waveform);
        let marker = self.fb.partial_refresh(
            &rect,
            PartialRefreshMode::Async,
            wf,
            display_temp::TEMP_USE_REMARKABLE_DRAW,
            dither,
            quant,
            req.full, // force_full_refresh
        );
        UpdateMarker(marker)
    }

    fn wait(&mut self, marker: UpdateMarker) {
        self.fb.wait_refresh_complete(marker.0);
    }
}
