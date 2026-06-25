//! Shared geometry and device types used across the rmbrowser crates.
//!
//! Kept dependency-free so every other crate can build on it cheaply.

/// An integer pixel point in some coordinate space (physical glass or logical viewport).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }

    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Smallest rectangle covering both inputs (treating empties as absent).
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.w as i32).max(other.x + other.w as i32);
        let y1 = (self.y + self.h as i32).max(other.y + other.h as i32);
        Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }
}

/// Which physical panel family the device has. Selected at runtime from the detected
/// panel; drives the choice of mono vs. color `PixelPipeline` and waveform set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKind {
    /// E Ink Carta greyscale (Paper Pure, reMarkable 2).
    Mono,
    /// E Ink Gallery 3 colour / ACeP (Paper Pro, Paper Pro Move).
    Color,
}

/// Screen orientation. `Portrait` is the device's native upright orientation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    LandscapeRight,
    PortraitFlipped,
    LandscapeLeft,
}

impl Orientation {
    /// True for the two landscape orientations (logical viewport has swapped axes).
    pub const fn is_landscape(self) -> bool {
        matches!(self, Orientation::LandscapeRight | Orientation::LandscapeLeft)
    }
}

/// A target device in the supported lineup. Used for build/runtime tuning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// Paper Pro (`ferrari`): i.MX8M Mini, aarch64, 2 GB, Gallery 3 colour.
    PaperPro,
    /// Paper Pro Move (`chiappa`): i.MX93, aarch64, 2 GB, Gallery 3 colour, PxP.
    PaperProMove,
    /// Paper Pure (`tatsu`): i.MX93, aarch64, 2 GB, Carta mono.
    PaperPure,
    /// reMarkable 2 (`rm2`): i.MX7D, armv7, 1 GB, Carta mono, rm2fb shim.
    Rm2,
}

impl Device {
    pub const fn panel(self) -> PanelKind {
        match self {
            Device::PaperPro | Device::PaperProMove => PanelKind::Color,
            Device::PaperPure | Device::Rm2 => PanelKind::Mono,
        }
    }

    /// Whether reaching the panel requires the rm2fb shim (true only for RM2).
    pub const fn needs_rm2fb(self) -> bool {
        matches!(self, Device::Rm2)
    }

    /// The backing framebuffer pixel format. RM2's rm2fb shm is RGB565; the primary
    /// devices' EPDC framebuffer format is to be confirmed on-device (assume RGB565).
    pub const fn framebuffer_format(self) -> PanelFormat {
        PanelFormat::Rgb565
    }
}

/// Pixel format of the backing framebuffer we write into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelFormat {
    /// 8-bit greyscale, one byte per pixel.
    Y8,
    /// 16-bit RGB565, little-endian, two bytes per pixel (rm2fb `/swtfb.01`).
    Rgb565,
}

impl PanelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            PanelFormat::Y8 => 1,
            PanelFormat::Rgb565 => 2,
        }
    }

    /// Pack an 8-bit grey value into this format's bytes (little-endian).
    pub fn pack_grey(self, grey: u8) -> [u8; 2] {
        match self {
            PanelFormat::Y8 => [grey, 0],
            PanelFormat::Rgb565 => {
                let r = (grey >> 3) as u16;
                let g = (grey >> 2) as u16;
                let b = (grey >> 3) as u16;
                let v = (r << 11) | (g << 5) | b;
                v.to_le_bytes()
            }
        }
    }
}
