//! Render engine seam (PLAN.md §5, §6).
//!
//! The web engine is a replaceable component behind this trait: WPE WebKit (default, all
//! devices incl. RM2), with NetSurf as the RM2 fallback and Blitz as the Rust-native
//! future. The rest of the app drives navigation/zoom and consumes painted surfaces +
//! damage + an extracted link model, never the engine's internals.

use rmb_types::Rect;

/// A link discovered in the current document, with the signals the link drawer scores
/// (PLAN.md §11). Engine impls populate this from the DOM (injected JS / JSC walk).
#[derive(Clone, Debug)]
pub struct LinkInfo {
    pub href: String,
    pub text: String,
    /// `rel` tokens on the `<a>`/`<link>` (lower-cased), e.g. ["next"].
    pub rel: Vec<String>,
    /// True if the link sits inside a `<nav>` / pagination landmark.
    pub in_nav: bool,
    /// True if same-origin as the current document.
    pub same_origin: bool,
    /// Vertical position bucket within the document.
    pub position: DomPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomPosition {
    Top,
    Middle,
    Bottom,
}

/// A heading entry for the reader TOC / jump menu (PLAN.md §12).
#[derive(Clone, Debug)]
pub struct Heading {
    /// 1–6.
    pub level: u8,
    pub text: String,
    pub anchor: String,
}

/// Result of painting: the damaged region the compositor should re-pull.
#[derive(Clone, Copy, Debug)]
pub struct PaintDamage {
    pub region: Rect,
}

/// The replaceable web engine.
pub trait RenderEngine {
    fn load(&mut self, url: &str);
    fn current_url(&self) -> &str;

    fn set_zoom(&mut self, factor: f32);
    fn scroll_by(&mut self, dx: i32, dy: i32);

    /// Links in the current document (for the link drawer / fast-nav).
    fn links(&self) -> Vec<LinkInfo>;
    /// Headings in the current document (for the reader TOC).
    fn headings(&self) -> Vec<Heading>;

    /// Pull the latest painted RGBA into `out` (row-major RGBA8); returns the damage.
    fn paint(&mut self, out: &mut Vec<u8>) -> PaintDamage;
}

pub mod wpe {
    //! WPE WebKit engine via FFI to libwpe + a headless view backend. Scaffold — Phase 2.
}

pub mod netsurf {
    //! NetSurf engine (RM2 fallback). Scaffold.
}
