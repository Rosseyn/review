//! Input intents (PLAN.md §9).
//!
//! The input thread decodes evdev (touch, pen, buttons) into a tiny, viewer-first set of
//! high-level intents. Anything outside this set (drag-drop, multi-finger gestures beyond
//! pinch, rich editing) is intentionally absent and fails gracefully.

use rmb_types::Point;

/// A high-level, debounced user intent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Intent {
    /// Pan/scroll by a delta (touch drag).
    Scroll { dx: i32, dy: i32 },
    /// Pinch zoom; `scale` is a multiplicative factor around `center`.
    Pinch { center: Point, scale: f32 },
    /// Activate whatever is under the point (link, control, or virtual button).
    Tap { at: Point },
    /// Precise pen tap (treated like a pointer; no inking).
    PenTap { at: Point },
    /// A hardware/power button event.
    Button(ButtonId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonId {
    Power,
}
