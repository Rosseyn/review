//! Display backend: how panel-ready bytes reach the glass (PLAN.md §4, §6).
//!
//! Two real implementations sit behind [`DisplayBackend`]: `mxcfb`-direct (Pro/Move/Pure —
//! a real kernel EPDC framebuffer driven by `MXCFB_SEND_UPDATE`) and `rm2fb` (RM2 — shared
//! memory `/swtfb.01` + SysV message queue). Both require the device, so a host-testable
//! [`MemoryBackend`] is provided for development and tests. The protocol mapping in
//! [`mxcfb`] (waveform → mode id) is pure and unit-tested here.

use rmb_display::{Waveform, WaveformMode};
use rmb_types::{PanelFormat, Rect};

/// One panel update request handed to a [`DisplayBackend`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateRequest {
    pub region: Rect,
    pub waveform: Waveform,
    /// Full (clearing) vs partial update.
    pub full: bool,
}

/// An opaque marker for an in-flight update, used to wait for completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateMarker(pub u32);

/// Detected panel geometry/format, probed at startup.
#[derive(Clone, Copy, Debug)]
pub struct PanelInfo {
    pub width: u32,
    pub height: u32,
    pub format: PanelFormat,
}

impl PanelInfo {
    pub fn stride(&self) -> usize {
        self.width as usize * self.format.bytes_per_pixel()
    }
}

/// The seam to the panel. Implementors own the framebuffer and the update transport.
pub trait DisplayBackend {
    fn panel_info(&self) -> &PanelInfo;

    /// Write already-converted region bytes into the backing buffer at `region`.
    fn blit(&mut self, region: Rect, bytes: &[u8]);

    /// Submit an update; returns a marker the caller may later wait on.
    fn submit(&mut self, req: &UpdateRequest) -> UpdateMarker;

    /// Block until the given update has completed (used only before dependent flushes).
    fn wait(&mut self, marker: UpdateMarker);
}

/// A host-side backend that keeps the framebuffer in memory. Used for development and
/// tests; mirrors the blit/submit semantics of the device backends without any I/O.
pub struct MemoryBackend {
    info: PanelInfo,
    fb: Vec<u8>,
    next_marker: u32,
    pub submitted: Vec<UpdateRequest>,
}

impl MemoryBackend {
    pub fn new(width: u32, height: u32, format: PanelFormat) -> Self {
        let info = PanelInfo { width, height, format };
        let fb = vec![0u8; info.stride() * height as usize];
        Self {
            info,
            fb,
            next_marker: 0,
            submitted: Vec::new(),
        }
    }

    pub fn framebuffer(&self) -> &[u8] {
        &self.fb
    }

    /// Raw bytes of pixel (x,y) in the framebuffer.
    pub fn pixel(&self, x: u32, y: u32) -> &[u8] {
        let bpp = self.info.format.bytes_per_pixel();
        let i = y as usize * self.info.stride() + x as usize * bpp;
        &self.fb[i..i + bpp]
    }
}

impl DisplayBackend for MemoryBackend {
    fn panel_info(&self) -> &PanelInfo {
        &self.info
    }

    fn blit(&mut self, region: Rect, bytes: &[u8]) {
        let bpp = self.info.format.bytes_per_pixel();
        let fb_stride = self.info.stride();
        let row_bytes = region.w as usize * bpp;
        for ry in 0..region.h {
            let dst_y = region.y as usize + ry as usize;
            if dst_y >= self.info.height as usize {
                break;
            }
            let dst = dst_y * fb_stride + region.x as usize * bpp;
            let src = ry as usize * row_bytes;
            // Clip to the framebuffer width.
            let copy = row_bytes.min(fb_stride.saturating_sub(region.x as usize * bpp));
            if src + copy <= bytes.len() && dst + copy <= self.fb.len() {
                self.fb[dst..dst + copy].copy_from_slice(&bytes[src..src + copy]);
            }
        }
    }

    fn submit(&mut self, req: &UpdateRequest) -> UpdateMarker {
        self.submitted.push(*req);
        let m = self.next_marker;
        self.next_marker = self.next_marker.wrapping_add(1);
        UpdateMarker(m)
    }

    fn wait(&mut self, _marker: UpdateMarker) {}
}

/// mxcfb protocol mapping (PLAN.md §4). Pure and testable; the device backends use it to
/// build the `mxcfb_update_data` they ioctl/enqueue.
pub mod mxcfb {
    use super::{Waveform, WaveformMode};

    /// `update_mode` field: partial vs full (clearing) update.
    pub const UPDATE_MODE_PARTIAL: u32 = 0;
    pub const UPDATE_MODE_FULL: u32 = 1;

    /// EPDC waveform mode id for a mono waveform.
    pub const fn mono_mode_id(m: WaveformMode) -> u32 {
        match m {
            WaveformMode::Init => 0,
            WaveformMode::Du => 1,
            WaveformMode::Gc16 => 2,
            WaveformMode::Gl16 => 3,
            WaveformMode::Glr16 => 4,
            WaveformMode::A2 => 6,
            WaveformMode::Du4 => 7,
        }
    }

    /// Resolve a [`Waveform`] to an EPDC `(waveform_mode, update_mode)` pair.
    ///
    /// Returns `None` for colour (Gallery 3) waveforms — those mode ids are not yet
    /// reverse-engineered and are handled by the (Phase 3) colour backend, not here.
    pub fn resolve(waveform: Waveform, full: bool) -> Option<(u32, u32)> {
        let update_mode = if full { UPDATE_MODE_FULL } else { UPDATE_MODE_PARTIAL };
        match waveform {
            Waveform::Mono(m) => Some((mono_mode_id(m), update_mode)),
            Waveform::Color(_) => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use rmb_display::ColorClass;

        #[test]
        fn mono_ids_match_plan_table() {
            assert_eq!(mono_mode_id(WaveformMode::Du), 1);
            assert_eq!(mono_mode_id(WaveformMode::Gc16), 2);
            assert_eq!(mono_mode_id(WaveformMode::A2), 6);
        }

        #[test]
        fn resolve_sets_update_mode() {
            assert_eq!(resolve(Waveform::Mono(WaveformMode::Gc16), true), Some((2, UPDATE_MODE_FULL)));
            assert_eq!(resolve(Waveform::Mono(WaveformMode::Du), false), Some((1, UPDATE_MODE_PARTIAL)));
        }

        #[test]
        fn color_is_unresolved_here() {
            assert_eq!(resolve(Waveform::Color(ColorClass::StandardColor), false), None);
        }
    }
}

/// reMarkable 2 framebuffer backend over rm2fb (`/dev/shm/swtfb.01` + SysV message queue).
/// Implemented against `libremarkable` 0.7; only built under the `device` feature so the
/// host workspace stays free of the device dependency (PLAN.md §4, DEVICE.md).
#[cfg(feature = "device")]
pub mod rm2fb;

pub mod mxcfb_direct {
    //! Direct kernel EPDC framebuffer backend (Pro/Move/Pure). Scaffold — primary devices.
}
