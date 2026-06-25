//! rmbrowser — application shell (PLAN.md §6).
//!
//! This is a scaffold entry point: it wires the pure-logic crates together as a smoke test
//! of the architecture (no engine/display I/O yet — those land in Phases 0–3). It prints
//! the resolved device profile and exercises the refresh FSM, link scoring, button
//! anchoring, and bookmark import so the seams demonstrably compose.

use rmb_display::{ContentClass, FsmConfig, RefreshController};
use rmb_engine::{DomPosition, LinkInfo};
use rmb_nav::{pick_best, NavSlot};
use rmb_overlay::{resolve_rect, AnchorMode, ButtonAction, VirtualButton};
use rmb_types::{Device, Orientation, Rect};

fn main() {
    let device = Device::PaperPro; // would be detected at runtime
    println!("rmbrowser scaffold — device profile");
    println!("  device        : {device:?}");
    println!("  panel         : {:?}", device.panel());
    println!("  needs rm2fb   : {}", device.needs_rm2fb());

    // --- refresh FSM: simulate a scroll burst, observe the fast-mode switch ---
    let mut fsm = RefreshController::new(device.panel(), FsmConfig::default());
    let region = Rect::new(0, 0, 1404, 200);
    let mut t = 0;
    let mut entered_at = None;
    for frame in 0..10 {
        let d = fsm.on_update(t, region, ContentClass::MonoGrey);
        if d.entered_fast {
            entered_at = Some(frame);
        }
        t += 30; // ~33 Hz scroll
    }
    println!("\nrefresh FSM (33 Hz scroll):");
    println!("  mode          : {:?}", fsm.mode());
    println!("  rate (Hz)     : {:.1}", fsm.rate_hz());
    println!("  entered fast  : frame {entered_at:?}");
    if let Some(flush) = fsm.settle(t + 1000) {
        println!("  settle flush  : {flush:?}");
    }

    // --- link drawer: resolve the "next" slot ---
    let links = vec![
        LinkInfo {
            href: "https://blog.test/page/2".into(),
            text: "Older posts »".into(),
            rel: vec!["next".into()],
            in_nav: true,
            same_origin: true,
            position: DomPosition::Bottom,
        },
        LinkInfo {
            href: "https://blog.test/login".into(),
            text: "Sign in".into(),
            rel: vec![],
            in_nav: false,
            same_origin: true,
            position: DomPosition::Top,
        },
    ];
    let next = pick_best(NavSlot::Next, &links, "https://blog.test/page/1", 1);
    println!("\nlink drawer:");
    println!("  next target   : {:?}", next.map(|l| l.href.as_str()));

    // --- virtual button anchoring ---
    let btn = VirtualButton {
        pos_pct: (0.92, 0.5),
        size_px: (120, 120),
        action: ButtonAction::Scroll { dx_frac: 0.0, dy_frac: 0.85 },
        anchor: AnchorMode::FixedScreenPercent,
        enabled: true,
    };
    let phys = (1404, 1872);
    println!("\nvirtual button (fixed-percent, scroll-down):");
    println!("  portrait      : {:?}", resolve_rect(&btn, phys, Orientation::Portrait));
    println!("  landscape     : {:?}", resolve_rect(&btn, phys, Orientation::LandscapeRight));

    // --- bookmark import ---
    let html = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
  <DT><H3>Bar</H3>
  <DL><p>
    <DT><A HREF="https://example.com/">Example</A>
  </DL><p>
</DL><p>"#;
    let root = rmb_bookmarks::parse_netscape(html);
    println!("\nbookmark import:");
    println!("  imported      : {} bookmark(s)", root.bookmark_count());
}
