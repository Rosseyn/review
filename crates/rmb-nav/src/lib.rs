//! Link drawer / semantic fast-navigation scoring (PLAN.md §11).
//!
//! Scores candidate links for a fixed set of navigation slots using rel signals, URL
//! page-number deltas, link text/glyph heuristics, landmark membership, position, and
//! negative/off-origin penalties. Pure and dependency-free (substring/glyph matching in
//! place of a regex crate for the skeleton).

use rmb_engine::{DomPosition, LinkInfo};

/// A navigation slot the drawer resolves and exposes as a direct-jump target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavSlot {
    Next,
    Prev,
    Up,
    Home,
    Contents,
    Top,
    Close,
}

impl NavSlot {
    /// Recognised rel tokens for this slot (incl. legacy synonyms).
    fn rel_tokens(self) -> &'static [&'static str] {
        match self {
            NavSlot::Next => &["next"],
            NavSlot::Prev => &["prev", "previous"],
            NavSlot::Up => &["up", "parent"],
            NavSlot::Home => &["home", "start", "begin"],
            NavSlot::Contents => &["contents", "index", "toc"],
            NavSlot::Top => &[],
            NavSlot::Close => &[],
        }
    }

    /// Positive text keywords (lower-case substrings) for this slot.
    fn text_words(self) -> &'static [&'static str] {
        match self {
            NavSlot::Next => &["next", "continue", "weiter", "older", "more"],
            NavSlot::Prev => &["prev", "previous", "earlier", "newer", "back"],
            NavSlot::Up => &["up", "parent"],
            NavSlot::Home => &["home", "start", "index"],
            NavSlot::Contents => &["contents", "table of contents", "toc", "index"],
            NavSlot::Top => &["top", "back to top"],
            NavSlot::Close => &["close", "dismiss"],
        }
    }

    /// Directional glyphs hinting this slot.
    fn glyphs(self) -> &'static [char] {
        match self {
            NavSlot::Next => &['»', '›', '→', '>'],
            NavSlot::Prev => &['«', '‹', '←', '<'],
            _ => &[],
        }
    }
}

const NEGATIVE_WORDS: &[&str] = &["comment", "login", "sign in", "signup", "sign up", "share", "tag", "print"];

/// Score weights (PLAN.md §11).
const W_REL: i32 = 100;
const W_URL_DELTA: i32 = 40;
const W_TEXT: i32 = 30;
const W_GLYPH: i32 = 15;
const W_LANDMARK: i32 = 15;
const W_POSITION: i32 = 10;
const W_NEGATIVE: i32 = 50;
const W_OFFHOST: i32 = 30;

/// Extract a page number from a URL: `?page=N`, `/page/N`, or a trailing path integer.
pub fn page_number(url: &str) -> Option<i64> {
    let lower = url.to_ascii_lowercase();
    for key in ["page=", "/page/", "p="] {
        if let Some(pos) = lower.find(key) {
            let rest = &url[pos + key.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<i64>() {
                return Some(n);
            }
        }
    }
    // Trailing integer path segment, e.g. /articles/12
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let seg = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()) {
        return seg.parse::<i64>().ok();
    }
    None
}

fn text_matches(words: &[&str], text_lower: &str) -> bool {
    words.iter().any(|w| text_lower.contains(w))
}

/// Score one candidate link for a slot, given the current document URL.
pub fn score(slot: NavSlot, link: &LinkInfo, current_url: &str) -> i32 {
    let text_lower = link.text.to_ascii_lowercase();
    let mut s = 0;

    // rel (decisive when present)
    if link.rel.iter().any(|r| slot.rel_tokens().contains(&r.as_str())) {
        s += W_REL;
    }

    // URL page-number delta for next/prev
    if matches!(slot, NavSlot::Next | NavSlot::Prev) {
        if let (Some(cur), Some(cand)) = (page_number(current_url), page_number(&link.href)) {
            let want = if slot == NavSlot::Next { cur + 1 } else { cur - 1 };
            if cand == want {
                s += W_URL_DELTA;
            }
        }
    }

    // text + glyph heuristics
    if text_matches(slot.text_words(), &text_lower) {
        s += W_TEXT;
    }
    if link.text.chars().any(|c| slot.glyphs().contains(&c)) {
        s += W_GLYPH;
    }

    // landmark + position
    if link.in_nav {
        s += W_LANDMARK;
    }
    match (slot, link.position) {
        (NavSlot::Next, DomPosition::Bottom) => s += W_POSITION,
        (NavSlot::Prev | NavSlot::Up, DomPosition::Top) => s += W_POSITION,
        _ => {}
    }

    // penalties
    if text_matches(NEGATIVE_WORDS, &text_lower) {
        s -= W_NEGATIVE;
    }
    if !link.same_origin {
        s -= W_OFFHOST;
    }
    s
}

/// Pick the best candidate for a slot above a minimum confidence threshold.
pub fn pick_best<'a>(
    slot: NavSlot,
    links: &'a [LinkInfo],
    current_url: &str,
    threshold: i32,
) -> Option<&'a LinkInfo> {
    links
        .iter()
        .map(|l| (l, score(slot, l, current_url)))
        .filter(|(_, s)| *s >= threshold)
        .max_by_key(|(_, s)| *s)
        .map(|(l, _)| l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(href: &str, text: &str, rel: &[&str]) -> LinkInfo {
        LinkInfo {
            href: href.into(),
            text: text.into(),
            rel: rel.iter().map(|s| s.to_string()).collect(),
            in_nav: false,
            same_origin: true,
            position: DomPosition::Middle,
        }
    }

    #[test]
    fn rel_next_is_decisive() {
        let l = link("https://x.test/2", "click", &["next"]);
        assert!(score(NavSlot::Next, &l, "https://x.test/1") >= W_REL);
    }

    #[test]
    fn glyph_and_text_contribute() {
        let l = link("https://x.test/p2", "Next »", &[]);
        let s = score(NavSlot::Next, &l, "https://x.test/p1");
        assert!(s >= W_TEXT + W_GLYPH);
    }

    #[test]
    fn url_page_delta_detected() {
        let l = link("https://x.test/articles/13", "go", &[]);
        let s = score(NavSlot::Next, &l, "https://x.test/articles/12");
        assert!(s >= W_URL_DELTA);
    }

    #[test]
    fn negative_words_penalised() {
        let l = link("https://x.test/c", "next comment", &[]);
        let pos = link("https://x.test/2", "next", &["next"]);
        assert!(score(NavSlot::Next, &l, "https://x.test/1") < score(NavSlot::Next, &pos, "https://x.test/1"));
    }

    #[test]
    fn off_origin_penalised() {
        let same = link("https://x.test/2", "next", &[]);
        let mut off = same.clone();
        off.href = "https://other.test/2".into();
        off.same_origin = false;
        // Identical but cross-origin must score exactly W_OFFHOST lower.
        assert_eq!(
            score(NavSlot::Next, &same, "https://x.test/1") - score(NavSlot::Next, &off, "https://x.test/1"),
            W_OFFHOST
        );
    }

    #[test]
    fn pick_best_chooses_highest() {
        let links = vec![
            link("https://x.test/spam", "share", &[]),
            link("https://x.test/2", "Next »", &["next"]),
            link("https://x.test/0", "Prev", &["prev"]),
        ];
        let best = pick_best(NavSlot::Next, &links, "https://x.test/1", 1).unwrap();
        assert_eq!(best.href, "https://x.test/2");
    }
}
