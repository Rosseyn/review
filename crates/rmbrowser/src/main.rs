//! rmbrowser — application shell (PLAN.md §6).
//!
//! Scaffold entry point targeting the **reMarkable 2 first** (mono Carta, rm2fb). It wires
//! the pure-logic crates into an end-to-end *host* render path (RGBA → mono dither → blit →
//! submit) using the in-memory display backend, and exercises the FSM, link scoring, button
//! anchoring, and bookmark import. Real device I/O (rm2fb, WPE) lands in Phases 0–2.

use rmb_display::{ContentClass, FsmConfig, RefreshController, Waveform};
use rmb_engine::{DomPosition, LinkInfo};
use rmb_hal::{mxcfb, DisplayBackend, MemoryBackend, UpdateRequest};
use rmb_nav::{pick_best, NavSlot};
use rmb_overlay::{resolve_rect, AnchorMode, ButtonAction, VirtualButton};
use rmb_pixel::{mono::MonoPipeline, PanelBuffer, PixelPipeline, RgbaSurface};
use rmb_types::{Device, Orientation, Rect};

fn main() {
    let device = Device::Rm2; // RM2-first; would be detected at runtime
    let fmt = device.framebuffer_format();
    println!("rmbrowser scaffold — target {device:?}");
    println!("  panel         : {:?}", device.panel());
    println!("  fb format     : {fmt:?}");
    println!("  needs rm2fb   : {}", device.needs_rm2fb());

    // --- end-to-end mono render: a grey ramp strip → dither → blit → submit ---
    // RM2 portrait framebuffer is 1404 x 1872 (rm2fb /swtfb.01, RGB565).
    let (fb_w, fb_h) = (1404u32, 1872u32);
    let mut backend = MemoryBackend::new(fb_w, fb_h, fmt);

    let (rw, rh) = (1404u32, 32u32);
    let mut ramp = Vec::with_capacity((rw * rh * 4) as usize);
    for _y in 0..rh {
        for x in 0..rw {
            let v = (x * 255 / (rw - 1)) as u8;
            ramp.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let src = RgbaSurface { width: rw, height: rh, pixels: &ramp };
    let region = Rect::new(0, 920, rw, rh); // a band near vertical centre
    let mut pipe = MonoPipeline::new(fmt);
    let mut dst = PanelBuffer::new(rw, rh, fmt);
    let written = pipe.convert(&src, Rect::new(0, 0, rw, rh), &mut dst);
    backend.blit(region, &dst.bytes);
    let marker = backend.submit(&UpdateRequest {
        region,
        waveform: Waveform::Mono(rmb_display::WaveformMode::Gc16),
        full: false,
    });
    let mid = backend.pixel(rw / 2, region.y as u32 + 16);
    println!("\nmono render (grey ramp, ordered dither):");
    println!("  converted     : {written:?}");
    println!("  midpoint px   : {mid:02X?} (RGB565 LE)");
    println!("  submit marker : {marker:?}");

    // --- refresh FSM: a scroll burst picks a fast waveform, resolved to an EPDC id ---
    let mut fsm = RefreshController::new(device.panel(), FsmConfig::default());
    let mut t = 0;
    let mut last = None;
    for _ in 0..10 {
        last = Some(fsm.on_update(t, region, ContentClass::Text));
        t += 30; // ~33 Hz scroll over text
    }
    let wf = last.unwrap().waveform;
    println!("\nrefresh FSM (33 Hz text scroll):");
    println!("  mode          : {:?}", fsm.mode());
    println!("  waveform      : {wf:?}");
    println!("  mxcfb id      : {:?}", mxcfb::resolve(wf, false));

    // --- link drawer ---
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
    println!("\nvirtual button (fixed-percent, scroll-down):");
    println!("  portrait      : {:?}", resolve_rect(&btn, (fb_w, fb_h), Orientation::Portrait));
    println!("  landscape     : {:?}", resolve_rect(&btn, (fb_w, fb_h), Orientation::LandscapeRight));

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
