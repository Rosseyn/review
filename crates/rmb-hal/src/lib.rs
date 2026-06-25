//! Display backend: how panel-ready bytes reach the glass (PLAN.md §4, §6).
//!
//! Two implementations sit behind this trait: `mxcfb-direct` (Pro/Move/Pure — a real
//! kernel EPDC framebuffer driven by the `MXCFB_SEND_UPDATE` ioctl) and `rm2fb` (RM2 —
//! shared memory `/swtfb.01` + SysV message queue). The rest of the app never knows which.

use rmb_display::Waveform;
use rmb_types::Rect;

/// One panel update request handed to a [`DisplayBackend`].
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
pub struct PanelInfo {
    pub width: u32,
    pub height: u32,
    /// Bytes per pixel of the backing framebuffer (e.g. 2 for RGB565, 1 for Y8).
    pub bytes_per_pixel: u8,
}

/// The seam to the panel. Implementors own the framebuffer and the update transport.
pub trait DisplayBackend {
    fn panel_info(&self) -> &PanelInfo;

    /// Write already-converted bytes into the backing buffer at `region`.
    fn blit(&mut self, region: Rect, bytes: &[u8]);

    /// Submit an update; returns a marker the caller may later wait on.
    fn submit(&mut self, req: &UpdateRequest) -> UpdateMarker;

    /// Block until the given update has completed (used only before dependent flushes).
    fn wait(&mut self, marker: UpdateMarker);
}

pub mod mxcfb {
    //! Direct kernel EPDC framebuffer backend (Pro/Move/Pure). Scaffold — Phase 0.
}

pub mod rm2fb {
    //! reMarkable 2 framebuffer backend over `/swtfb.01` + SysV message queue. Scaffold.
}
