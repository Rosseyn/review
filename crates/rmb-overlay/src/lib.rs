//! Overlay layer: virtual buttons and their orientation anchoring (PLAN.md §10).
//!
//! Buttons render above the page and never pass events into the DOM. Each can either
//! rotate with the screen or stay fixed at a physical-glass percentage regardless of
//! orientation. This crate implements the (pure, testable) anchoring geometry; the actual
//! draw + hit-test ordering lives in the compositor.

use rmb_types::{Orientation, Rect};

/// How a button's position behaves under screen rotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorMode {
    /// Position is a fraction of the *logical* viewport — rotates and reflows with content.
    RotateWithScreen,
    /// Position is a fixed point on the *physical* glass — stays put as content rotates.
    FixedScreenPercent,
}

/// A swappable button action (PLAN.md §10). Extensible; scroll amount is user-configurable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ButtonAction {
    /// Scroll by a fraction of the viewport (e.g. -0.85 for "page up").
    Scroll { dx_frac: f32, dy_frac: f32 },
    NavNext,
    NavPrev,
    ZoomIn,
    ZoomOut,
    ReaderToggle,
    HiFiRedraw,
    OpenDrawer,
    /// Frontlight step (Pro/Move only); ignored elsewhere (fails gracefully).
    Frontlight(i32),
}

/// A user-placed virtual button. `pos_pct` is the button *centre* as (x,y) fractions.
#[derive(Clone, Copy, Debug)]
pub struct VirtualButton {
    pub pos_pct: (f32, f32),
    pub size_px: (u32, u32),
    pub action: ButtonAction,
    pub anchor: AnchorMode,
    pub enabled: bool,
}

/// Logical viewport size for a physical panel under an orientation (axes swap in landscape).
pub fn logical_size(phys: (u32, u32), o: Orientation) -> (u32, u32) {
    if o.is_landscape() {
        (phys.1, phys.0)
    } else {
        phys
    }
}

/// Map a point on the physical glass to logical-viewport coordinates under `o`.
fn phys_to_logical(p: (f32, f32), phys: (f32, f32), o: Orientation) -> (f32, f32) {
    let (pw, ph) = phys;
    let (px, py) = p;
    match o {
        Orientation::Portrait => (px, py),
        Orientation::LandscapeRight => (py, pw - px),
        Orientation::PortraitFlipped => (pw - px, ph - py),
        Orientation::LandscapeLeft => (ph - py, px),
    }
}

/// Resolve a button's on-screen rectangle in logical-viewport coordinates.
pub fn resolve_rect(btn: &VirtualButton, phys: (u32, u32), o: Orientation) -> Rect {
    let phys_f = (phys.0 as f32, phys.1 as f32);
    let (lw, lh) = logical_size(phys, o);

    let center = match btn.anchor {
        AnchorMode::RotateWithScreen => (btn.pos_pct.0 * lw as f32, btn.pos_pct.1 * lh as f32),
        AnchorMode::FixedScreenPercent => {
            let glass = (btn.pos_pct.0 * phys_f.0, btn.pos_pct.1 * phys_f.1);
            phys_to_logical(glass, phys_f, o)
        }
    };

    let (w, h) = btn.size_px;
    let x = (center.0 - w as f32 / 2.0).round().max(0.0) as i32;
    let y = (center.1 - h as f32 / 2.0).round().max(0.0) as i32;
    Rect::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btn(anchor: AnchorMode, pos: (f32, f32)) -> VirtualButton {
        VirtualButton {
            pos_pct: pos,
            size_px: (100, 100),
            action: ButtonAction::Scroll { dx_frac: 0.0, dy_frac: -0.85 },
            anchor,
            enabled: true,
        }
    }

    fn center_of(r: Rect) -> (i32, i32) {
        (r.x + r.w as i32 / 2, r.y + r.h as i32 / 2)
    }

    #[test]
    fn logical_axes_swap_in_landscape() {
        assert_eq!(logical_size((1404, 1872), Orientation::Portrait), (1404, 1872));
        assert_eq!(logical_size((1404, 1872), Orientation::LandscapeRight), (1872, 1404));
    }

    #[test]
    fn center_is_invariant_for_both_anchors() {
        let phys = (1404, 1872);
        for anchor in [AnchorMode::RotateWithScreen, AnchorMode::FixedScreenPercent] {
            let b = btn(anchor, (0.5, 0.5));
            for o in [
                Orientation::Portrait,
                Orientation::LandscapeRight,
                Orientation::PortraitFlipped,
                Orientation::LandscapeLeft,
            ] {
                let (lw, lh) = logical_size(phys, o);
                let c = center_of(resolve_rect(&b, phys, o));
                // Centre of the screen stays at the centre of the logical viewport.
                assert!((c.0 - lw as i32 / 2).abs() <= 1);
                assert!((c.1 - lh as i32 / 2).abs() <= 1);
            }
        }
    }

    #[test]
    fn fixed_anchor_tracks_physical_corner_under_rotation() {
        let phys = (1404, 1872);
        // A button pinned near the physical top-left corner.
        let b = btn(AnchorMode::FixedScreenPercent, (0.0, 0.0));
        let portrait = center_of(resolve_rect(&b, phys, Orientation::Portrait));
        let landscape = center_of(resolve_rect(&b, phys, Orientation::LandscapeRight));
        // Under FixedScreenPercent the logical coordinates must differ across orientations
        // (the physical point maps to a different logical corner once content rotates).
        assert_ne!(portrait, landscape);
    }

    #[test]
    fn rotate_anchor_stays_at_logical_origin() {
        let phys = (1404, 1872);
        let b = btn(AnchorMode::RotateWithScreen, (0.0, 0.0));
        // RotateWithScreen keeps the button at logical (0,0)-ish in every orientation.
        for o in [Orientation::Portrait, Orientation::LandscapeRight] {
            let r = resolve_rect(&b, phys, o);
            assert_eq!((r.x, r.y), (0, 0));
        }
    }
}
