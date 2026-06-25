//! Waveform modes and the refresh-mode state machine (PLAN.md §7).
//!
//! This is the heart of responsiveness (emphasis #1/#2): it picks an e-ink waveform per
//! update from the redraw rate, the content class of the dirty region, and the current
//! mode — switching to fast/low-quality modes *only* past a measured rate threshold with
//! hysteresis, and suppressing the animation mode when only text is redrawn.
//!
//! Pure logic, no hardware: the caller feeds timestamps and dirty-region metadata and
//! receives a [`Decision`]; an actual `DisplayBackend` (in `rmb-hal`) executes it.

use rmb_types::{PanelKind, Rect};

/// Mono (Carta) EPDC waveform modes, with their typical reMarkable latencies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveformMode {
    /// Full clear / de-ghost flash (~780 ms).
    Init,
    /// Direct Update: fast bitonal, ~170 ms. Text/UI fast path.
    Du,
    /// 16-level greyscale, flashing, ~460 ms. Quality default for images/page.
    Gc16,
    /// Sparse anti-aliased text, reduced flash, ~460 ms.
    Gl16,
    /// REAGL anti-aliased text, reduced flash, ~460 ms.
    Glr16,
    /// Fastest bitonal animation, ~135 ms. Scroll/pan; ghosts → must flush.
    A2,
    /// Non-flashing 4-level, ~300 ms. Menus.
    Du4,
}

/// Colour (Gallery 3) refresh classes (PLAN.md §4). Latencies are far higher than mono,
/// so colour is never used during motion — the mono partial path is the fast path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorClass {
    /// Mono / B-W partial update (~12–350 ms): the colour-panel fast path.
    MonoPartial,
    /// Quick colour paint (~500 ms).
    FastColor,
    /// Normal static colour paint (~750–1000 ms).
    StandardColor,
    /// Best colour, hi-fi only (~1500 ms).
    BestColor,
}

/// The waveform a [`Decision`] selects — one of the two panel families.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waveform {
    Mono(WaveformMode),
    Color(ColorClass),
}

impl Waveform {
    /// Whether this is a 1-bit/bitonal-class update that accrues ghosting.
    pub fn is_bitonal(self) -> bool {
        matches!(
            self,
            Waveform::Mono(WaveformMode::A2 | WaveformMode::Du)
                | Waveform::Color(ColorClass::MonoPartial)
        )
    }
}

/// Cheap classification of a dirty region, produced during raster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentClass {
    /// Near-bitonal: text / UI chrome. Never needs the animation mode.
    Text,
    /// Greyscale image or gradient.
    MonoGrey,
    /// Chromatic content (only meaningful on a colour panel).
    Color,
}

/// High-level refresh mode the controller is currently in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Quality waveforms (GC16 / colour). Default, idle / low rate.
    Quality,
    /// Fast waveforms (A2 / DU / mono-partial). Engaged only past the rate threshold.
    Fast,
}

/// Tunable thresholds (PLAN.md §7.3). Defaults are starting points to be measured on-device.
#[derive(Clone, Copy, Debug)]
pub struct FsmConfig {
    /// EMA updates/sec at/above which we begin counting toward entering fast mode.
    pub enter_fast_hz: f32,
    /// Consecutive above-threshold updates required to actually enter fast mode (hysteresis).
    pub enter_fast_frames: u32,
    /// EMA updates/sec below which fast mode is allowed to settle out.
    pub exit_fast_hz: f32,
    /// Idle dwell (ms) with no update before a settle (exit fast + quality flush).
    pub settle_ms: u64,
    /// Consecutive bitonal fast updates before a ghost-clearing full flush.
    pub ghost_flush_n: u32,
    /// EMA smoothing factor in [0,1]; higher reacts faster.
    pub ema_alpha: f32,
}

impl Default for FsmConfig {
    fn default() -> Self {
        Self {
            enter_fast_hz: 4.0,
            enter_fast_frames: 3,
            exit_fast_hz: 2.0,
            settle_ms: 350,
            ghost_flush_n: 12,
            ema_alpha: 0.4,
        }
    }
}

/// What the controller decided for one update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    /// Region to update.
    pub region: Rect,
    /// Waveform to drive it with.
    pub waveform: Waveform,
    /// If set, also issue a full-region quality flush (ghost budget hit) over this rect.
    pub flush: Option<Rect>,
    /// True if this update transitioned Quality → Fast.
    pub entered_fast: bool,
}

/// The refresh-mode state machine. One per display surface; driven by the composite thread.
#[derive(Clone, Debug)]
pub struct RefreshController {
    cfg: FsmConfig,
    panel: PanelKind,
    mode: Mode,
    rate_ema: f32,
    above_count: u32,
    ghost_count: u32,
    last_update_ms: Option<u64>,
    /// Union of fast-mode dirty regions since entering fast, for the settle flush.
    fast_region: Rect,
}

impl RefreshController {
    pub fn new(panel: PanelKind, cfg: FsmConfig) -> Self {
        Self {
            cfg,
            panel,
            mode: Mode::Quality,
            rate_ema: 0.0,
            above_count: 0,
            ghost_count: 0,
            last_update_ms: None,
            fast_region: Rect::new(0, 0, 0, 0),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn rate_hz(&self) -> f32 {
        self.rate_ema
    }

    /// Feed one update. `now_ms` is a monotonic clock in milliseconds.
    pub fn on_update(&mut self, now_ms: u64, region: Rect, content: ContentClass) -> Decision {
        // 1. Update the rate EMA from the inter-update interval.
        if let Some(prev) = self.last_update_ms {
            let dt = now_ms.saturating_sub(prev).max(1) as f32;
            let inst_hz = 1000.0 / dt;
            self.rate_ema = self.rate_ema * (1.0 - self.cfg.ema_alpha) + inst_hz * self.cfg.ema_alpha;
        }
        self.last_update_ms = Some(now_ms);

        // 2. Hysteresis: enter fast only after N consecutive above-threshold updates.
        let mut entered_fast = false;
        if self.mode == Mode::Quality {
            if self.rate_ema >= self.cfg.enter_fast_hz {
                self.above_count += 1;
                if self.above_count >= self.cfg.enter_fast_frames {
                    self.mode = Mode::Fast;
                    entered_fast = true;
                    self.ghost_count = 0;
                    self.fast_region = Rect::new(0, 0, 0, 0);
                }
            } else {
                self.above_count = 0;
            }
        }

        // 3. Pick the waveform for this update.
        let waveform = self.select_waveform(content);

        // 4. Ghost budget: count bitonal fast updates; flush when the budget is hit.
        let mut flush = None;
        if self.mode == Mode::Fast {
            self.fast_region = self.fast_region.union(&region);
            if waveform.is_bitonal() {
                self.ghost_count += 1;
                if self.ghost_count >= self.cfg.ghost_flush_n {
                    flush = Some(self.fast_region);
                    self.ghost_count = 0;
                }
            }
        }

        Decision {
            region,
            waveform,
            flush,
            entered_fast,
        }
    }

    /// Call when no update has occurred for a while (e.g. each idle tick). If we are in fast
    /// mode and have been idle past `settle_ms`, exit to quality and request a settle flush
    /// over the touched region to restore fidelity and clear ghosting.
    pub fn settle(&mut self, now_ms: u64) -> Option<Rect> {
        if self.mode != Mode::Fast {
            return None;
        }
        let idle = self
            .last_update_ms
            .map(|t| now_ms.saturating_sub(t))
            .unwrap_or(u64::MAX);
        if idle < self.cfg.settle_ms && self.rate_ema >= self.cfg.exit_fast_hz {
            return None;
        }
        self.mode = Mode::Quality;
        self.above_count = 0;
        self.rate_ema = 0.0;
        let region = self.fast_region;
        self.fast_region = Rect::new(0, 0, 0, 0);
        (!region.is_empty()).then_some(region)
    }

    fn select_waveform(&self, content: ContentClass) -> Waveform {
        match (self.panel, self.mode, content) {
            // ---- Mono panel ----
            // Fast: text → DU (bitonal, NOT A2); grey/image → A2.
            (PanelKind::Mono, Mode::Fast, ContentClass::Text) => Waveform::Mono(WaveformMode::Du),
            (PanelKind::Mono, Mode::Fast, _) => Waveform::Mono(WaveformMode::A2),
            // Quality: text → GL16; grey/image → GC16.
            (PanelKind::Mono, Mode::Quality, ContentClass::Text) => Waveform::Mono(WaveformMode::Gl16),
            (PanelKind::Mono, Mode::Quality, _) => Waveform::Mono(WaveformMode::Gc16),

            // ---- Colour panel ----
            // Fast: always the mono partial path; colour is never driven during motion.
            (PanelKind::Color, Mode::Fast, _) => Waveform::Color(ColorClass::MonoPartial),
            // Quality: text/grey → mono partial (fast B-W); colour → standard colour.
            (PanelKind::Color, Mode::Quality, ContentClass::Color) => {
                Waveform::Color(ColorClass::StandardColor)
            }
            (PanelKind::Color, Mode::Quality, _) => Waveform::Color(ColorClass::MonoPartial),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::new(0, 0, 100, 100)
    }

    /// A single burst of two fast updates must NOT trip fast mode (needs N consecutive).
    #[test]
    fn no_accidental_fast_mode_on_brief_burst() {
        let mut fsm = RefreshController::new(PanelKind::Mono, FsmConfig::default());
        // Two updates 10ms apart = ~100 Hz instantaneous, but only 2 frames < enter_fast_frames(3).
        let d0 = fsm.on_update(0, rect(), ContentClass::Text);
        let d1 = fsm.on_update(10, rect(), ContentClass::Text);
        assert_eq!(fsm.mode(), Mode::Quality);
        assert!(!d0.entered_fast && !d1.entered_fast);
    }

    /// Sustained high rate enters fast mode after the hysteresis frame count.
    #[test]
    fn sustained_high_rate_enters_fast() {
        let mut fsm = RefreshController::new(PanelKind::Mono, FsmConfig::default());
        let mut t = 0;
        let mut entered = false;
        for _ in 0..8 {
            let d = fsm.on_update(t, rect(), ContentClass::MonoGrey);
            entered |= d.entered_fast;
            t += 30; // ~33 Hz, well above enter_fast_hz
        }
        assert_eq!(fsm.mode(), Mode::Fast);
        assert!(entered);
    }

    /// Text-only rapid redraw uses DU, never the A2 animation mode.
    #[test]
    fn text_fast_path_uses_du_not_a2() {
        let mut fsm = RefreshController::new(PanelKind::Mono, FsmConfig::default());
        let mut t = 0;
        let mut last = None;
        for _ in 0..8 {
            last = Some(fsm.on_update(t, rect(), ContentClass::Text));
            t += 30;
        }
        assert_eq!(fsm.mode(), Mode::Fast);
        assert_eq!(last.unwrap().waveform, Waveform::Mono(WaveformMode::Du));
    }

    /// Greyscale motion in fast mode uses A2.
    #[test]
    fn grey_fast_path_uses_a2() {
        let mut fsm = RefreshController::new(PanelKind::Mono, FsmConfig::default());
        let mut t = 0;
        let mut last = None;
        for _ in 0..8 {
            last = Some(fsm.on_update(t, rect(), ContentClass::MonoGrey));
            t += 30;
        }
        assert_eq!(last.unwrap().waveform, Waveform::Mono(WaveformMode::A2));
    }

    /// A ghost-clearing flush is requested after ghost_flush_n bitonal fast updates.
    #[test]
    fn ghost_flush_after_budget() {
        let cfg = FsmConfig {
            ghost_flush_n: 5,
            ..FsmConfig::default()
        };
        let mut fsm = RefreshController::new(PanelKind::Mono, cfg);
        let mut t = 0;
        let mut flushes = 0;
        for _ in 0..30 {
            let d = fsm.on_update(t, rect(), ContentClass::MonoGrey);
            if d.flush.is_some() {
                flushes += 1;
            }
            t += 30;
        }
        assert!(flushes >= 1, "expected at least one ghost flush");
    }

    /// Going idle settles fast mode back to quality and asks for a flush.
    #[test]
    fn settle_exits_fast_and_flushes() {
        let mut fsm = RefreshController::new(PanelKind::Mono, FsmConfig::default());
        let mut t = 0;
        for _ in 0..8 {
            fsm.on_update(t, rect(), ContentClass::MonoGrey);
            t += 30;
        }
        assert_eq!(fsm.mode(), Mode::Fast);
        let flush = fsm.settle(t + 1000);
        assert_eq!(fsm.mode(), Mode::Quality);
        assert!(flush.is_some());
    }

    /// On a colour panel, fast mode never selects a colour waveform.
    #[test]
    fn color_panel_motion_stays_mono() {
        let mut fsm = RefreshController::new(PanelKind::Color, FsmConfig::default());
        let mut t = 0;
        let mut last = None;
        for _ in 0..8 {
            last = Some(fsm.on_update(t, rect(), ContentClass::Color));
            t += 30;
        }
        assert_eq!(fsm.mode(), Mode::Fast);
        assert_eq!(last.unwrap().waveform, Waveform::Color(ColorClass::MonoPartial));
    }

    /// On a colour panel at rest, chromatic content paints in standard colour.
    #[test]
    fn color_panel_static_paints_color() {
        let mut fsm = RefreshController::new(PanelKind::Color, FsmConfig::default());
        let d = fsm.on_update(0, rect(), ContentClass::Color);
        assert_eq!(d.waveform, Waveform::Color(ColorClass::StandardColor));
    }
}
