//! Bookmark import (PLAN.md §13).
//!
//! Primary path: the universal Netscape Bookmark HTML format every browser exports. This
//! crate implements a lenient parser (the format omits close tags by spec) producing a
//! folder tree. Chromium-JSON and Firefox `places.sqlite` readers are planned additions.

/// A node in the bookmark tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Bookmark(Bookmark),
    Folder(Folder),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Folder {
    pub name: String,
    pub children: Vec<Node>,
}

impl Folder {
    /// Total number of bookmarks (recursively).
    pub fn bookmark_count(&self) -> usize {
        self.children
            .iter()
            .map(|n| match n {
                Node::Bookmark(_) => 1,
                Node::Folder(f) => f.bookmark_count(),
            })
            .sum()
    }
}

/// Parse a Netscape Bookmark HTML document into a root folder.
///
/// Lenient by necessity: treats `<H3>` as "open folder, descend", `<A HREF>` as a leaf,
/// and `</DL>` as "close current folder". Unclosed `<DT>`/`<DD>` and attribute-case
/// variation are tolerated.
pub fn parse_netscape(html: &str) -> Folder {
    let mut stack: Vec<Folder> = vec![Folder::default()];
    let bytes = html.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Advance to the next tag.
        let Some(lt) = find_from(bytes, i, b'<') else { break };
        let Some(gt) = find_from(bytes, lt + 1, b'>') else { break };
        let tag = &html[lt + 1..gt];
        let tag_lower = tag.trim_start().to_ascii_lowercase();

        // Text content following the tag, up to the next '<'.
        let text_start = gt + 1;
        let text_end = find_from(bytes, text_start, b'<').unwrap_or(bytes.len());
        let text = unescape(html[text_start..text_end].trim());

        if tag_lower.starts_with("h3") {
            stack.push(Folder {
                name: text,
                children: Vec::new(),
            });
        } else if tag_lower == "a" || tag_lower.starts_with("a ") || tag_lower.starts_with("a\t") {
            if let Some(url) = extract_attr(tag, "href") {
                if let Some(top) = stack.last_mut() {
                    top.children.push(Node::Bookmark(Bookmark { title: text, url }));
                }
            }
        } else if tag_lower.starts_with("/dl") && stack.len() > 1 {
            let f = stack.pop().unwrap();
            stack.last_mut().unwrap().children.push(Node::Folder(f));
        }

        i = text_end;
    }

    // Fold any unclosed folders back into the root (defensive against malformed files).
    while stack.len() > 1 {
        let f = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(Node::Folder(f));
    }
    stack.pop().unwrap_or_default()
}

fn find_from(bytes: &[u8], from: usize, b: u8) -> Option<usize> {
    bytes[from.min(bytes.len())..]
        .iter()
        .position(|&c| c == b)
        .map(|p| p + from)
}

/// Extract a quoted attribute value (case-insensitive name) from a tag's inner text.
fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let key = format!("{name}=");
    let kpos = lower.find(&key)?;
    let after = &tag[kpos + key.len()..];
    let after = after.trim_start();
    let mut chars = after.char_indices();
    let (_, first) = chars.next()?;
    if first == '"' || first == '\'' {
        let rest = &after[1..];
        let end = rest.find(first)?;
        Some(unescape(&rest[..end]))
    } else {
        // Unquoted: read until whitespace or '>'.
        let end = after.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(after.len());
        Some(unescape(&after[..end]))
    }
}

/// Minimal HTML entity unescaping for the entities bookmark exports actually use.
fn unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">
<TITLE>Bookmarks</TITLE>
<H1>Bookmarks</H1>
<DL><p>
    <DT><H3 PERSONAL_TOOLBAR_FOLDER="true">Bookmarks Bar</H3>
    <DL><p>
        <DT><A HREF="https://example.com/" ADD_DATE="1698384000">Example &amp; Co</A>
        <DD>desc
        <DT><H3>Sub</H3>
        <DL><p>
            <DT><A HREF="https://mozilla.org/">Mozilla</A>
        </DL><p>
    </DL><p>
</DL><p>"#;

    #[test]
    fn parses_tree_structure() {
        let root = parse_netscape(SAMPLE);
        assert_eq!(root.children.len(), 1);
        let bar = match &root.children[0] {
            Node::Folder(f) => f,
            _ => panic!("expected a folder"),
        };
        assert_eq!(bar.name, "Bookmarks Bar");
        // Example bookmark + Sub folder.
        assert_eq!(bar.children.len(), 2);
    }

    #[test]
    fn extracts_href_and_unescapes_title() {
        let root = parse_netscape(SAMPLE);
        let bar = match &root.children[0] {
            Node::Folder(f) => f,
            _ => unreachable!(),
        };
        match &bar.children[0] {
            Node::Bookmark(b) => {
                assert_eq!(b.url, "https://example.com/");
                assert_eq!(b.title, "Example & Co");
            }
            _ => panic!("expected a bookmark"),
        }
    }

    #[test]
    fn counts_nested_bookmarks() {
        let root = parse_netscape(SAMPLE);
        assert_eq!(root.bookmark_count(), 2);
    }
}
