//! The links of every text Kova shows as HTML: the chat, the Cockpit's
//! cards, En cours and Fait, the catch-up band, the decision sheets. One
//! detector (`find`), one HTML renderer (`linkify`), one opener (`open`),
//! which always goes to the side panel.
//!
//! What is a link:
//! - an http(s) URL;
//! - the path of a local file the side panel shows
//!   (`window::side_panel::FILE_EXTENSIONS`: markdown, HTML, images, PDF,
//!   text, JSON). An absolute or `~/` path to a `.md` or `.html` file as
//!   written; every other one only when the file exists: another extension,
//!   a path with spaces (`~/AI directory/x.md`), a path relative to the
//!   base folder or one of its parents (`artifacts/plan.md`), so a sentence
//!   that happens to start with `/` is never taken for one;
//! - a markdown link `[label](destination)` to one of those;
//! - with `linkify_refs`, a bare Linear id (`CLA-123`, `CLAR-`, `CLAD-`), to
//!   its issue, and a bare `#12345`, to that pull request of the Claap repo.
//!
//! Existence is the opener's own test (`side_panel::target_on_disk`): a
//! link drawn is a link that opens.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::window::side_panel::{self, FILE_EXTENSIONS as PANEL_EXTENSIONS, PanelTarget};

/// `s` as HTML text.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

thread_local! {
    /// The folder of the pane whose chat is rendered: what a relative path
    /// is read against (`on_disk`).
    static BASE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Set the folder relative paths are read against, before rendering a
/// pane's chat (the main thread renders every chat).
pub fn set_base(cwd: Option<&Path>) {
    BASE.with(|b| *b.borrow_mut() = cwd.map(Path::to_path_buf));
}

/// A link found in a text: its bytes in the text, and what it opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
    /// A markdown link `[label](destination)`: the bytes of its label and of
    /// its destination. `None`: the text between `start` and `end` is both.
    pub md: Option<((usize, usize), (usize, usize))>,
}

impl Link {
    /// What the link opens.
    pub fn target<'a>(&self, text: &'a str) -> &'a str {
        match self.md {
            Some((_, (a, b))) => &text[a..b],
            None => &text[self.start..self.end],
        }
    }

    /// What the link reads as.
    pub fn label<'a>(&self, text: &'a str) -> &'a str {
        match self.md {
            Some(((a, b), _)) => &text[a..b],
            None => &text[self.start..self.end],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Url,
    Path,
    /// A Linear issue id, `CLA-123`.
    Issue,
    /// A pull request number, `#12345`.
    Pull,
}

const LINEAR_ISSUE: &str = "https://linear.app/claap/issue/";
const GITHUB_PULL: &str = "https://github.com/claap-app/claap/pull/";

/// The longest path with spaces looked for, in bytes.
const SPACED_PATH_MAX: usize = 240;

/// Whether a link destination is a path the panel shows: absolute or `~/`,
/// ending in `.md` or `.html`.
pub fn is_panel_path(raw: &str) -> bool {
    (raw.starts_with('/') || raw.starts_with("~/")) && (raw.ends_with(".md") || raw.ends_with(".html"))
}

/// `raw` ends with an extension the panel shows.
fn has_panel_extension(raw: &str) -> bool {
    raw.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && PANEL_EXTENSIONS.contains(&ext))
}

/// Whether the whole of `raw` (a code span, a markdown link's destination)
/// names a file the panel shows: a path, absolute, `~/` or relative, to a
/// file that exists.
pub fn is_file_path(raw: &str, exists: &dyn Fn(&str) -> bool) -> bool {
    !raw.is_empty()
        && raw.len() <= SPACED_PATH_MAX
        && raw.contains('/')
        && !raw.contains("://")
        && !raw.contains(char::is_control)
        && has_panel_extension(raw)
        && exists(raw)
}


/// Whether `raw` is an http(s) URL.
pub fn is_web(raw: &str) -> bool {
    let lower = raw.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The links in `text`, in order, without overlap. `exists` says whether a
/// path with spaces names a file.
pub fn find(text: &str, exists: &dyn Fn(&str) -> bool) -> Vec<Link> {
    let mut links = urls(text);
    for path in paths(text, &links, exists) {
        links.push(path);
    }
    // A markdown link is one link: what was found inside it goes.
    let md = markdown_links(text, exists);
    links.retain(|l| !md.iter().any(|m| m.start < l.end && l.start < m.end));
    links.extend(md);
    links.sort_by_key(|l| l.start);
    links
}

/// `[label](destination)` on one line, to a URL or a file the panel shows.
fn markdown_links(text: &str, exists: &dyn Fn(&str) -> bool) -> Vec<Link> {
    let mut out: Vec<Link> = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find("](") {
        let mid = from + rel;
        from = mid + 2;
        let line_start = text[..mid].rfind('\n').map_or(0, |i| i + 1);
        let Some(open) = text[line_start..mid].rfind('[').map(|i| line_start + i) else { continue };
        let Some(close) = text[mid + 2..].find([')', '\n']).map(|i| mid + 2 + i).filter(|&i| text[i..].starts_with(')')) else { continue };
        let (label, dest) = (&text[open + 1..mid], text[mid + 2..close].trim());
        if label.trim().is_empty() || out.last().is_some_and(|l| open < l.end) {
            continue;
        }
        let kind = if is_web(dest) && !dest.contains(char::is_whitespace) {
            Kind::Url
        } else if is_panel_path(dest) || is_file_path(dest, exists) {
            Kind::Path
        } else {
            continue;
        };
        let at = mid + 2 + (text[mid + 2..close].len() - text[mid + 2..close].trim_start().len());
        out.push(Link { start: open, end: close + 1, kind, md: Some(((open + 1, mid), (at, at + dest.len()))) });
        from = close + 1;
    }
    out
}

/// What the links of `text` open, in order, once each.
pub fn targets(text: &str, exists: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for link in find(text, exists) {
        let target = link.target(text).to_string();
        if !out.contains(&target) {
            out.push(target);
        }
    }
    out
}

/// What a link reads as when it stands alone (a button, a list): a file's
/// name, a URL without its scheme, `max` chars at most.
pub fn label(link: &str, max: usize) -> String {
    if let Some(rest) = link.strip_prefix("https://").or_else(|| link.strip_prefix("http://")) {
        let mut out: String = rest.chars().take(max).collect();
        if rest.chars().count() > max {
            out.push('\u{2026}');
        }
        return out;
    }
    let file = link.split('#').next().unwrap_or(link).trim_end_matches('/');
    Path::new(file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| link.to_string())
}

/// A local path as a `file:` URL, for a page whose links are navigations
/// (a decision sheet in the side panel) or an attributed string (the
/// floating ask card): the path resolved as `open` would, percent-encoded.
/// `None` when it names nothing the panel shows.
pub fn file_url(path: &str, cwd: Option<&Path>) -> Option<String> {
    let file = match open(path, cwd)? {
        PanelTarget::Markdown(p) | PanelTarget::Html(p) | PanelTarget::File(p) => p,
        _ => return None,
    };
    let mut out = String::from("file://");
    for b in file.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Some(out)
}

/// The path a `file_url` names.
pub fn path_of_file_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() && let Ok(b) = u8::from_str_radix(rest.get(i + 1..i + 3)?, 16) {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The paths (as `file:` URLs), issue ids and pull request numbers of a
/// text whose URLs and markdown links were already taken out
/// (`text_links::spans`): each piece, and what it opens when it is a link.
pub fn pieces(text: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut at = 0;
    for link in find_with_refs(text, &on_disk) {
        if link.start < at {
            continue;
        }
        let target = link.target(text);
        let url = match link.kind {
            Kind::Url => Some(target.to_string()),
            Kind::Path => file_url(target, None),
            Kind::Issue => Some(format!("{LINEAR_ISSUE}{target}")),
            Kind::Pull => Some(format!("{GITHUB_PULL}{}", target.trim_start_matches('#'))),
        };
        let Some(url) = url else { continue };
        if link.start > at {
            out.push((text[at..link.start].to_string(), None));
        }
        out.push((link.label(text).to_string(), Some(url)));
        at = link.end;
    }
    if at < text.len() {
        out.push((text[at..].to_string(), None));
    }
    out
}

/// What a clicked link shows in the side panel, if anything: the one way a
/// link of any page opens. `cwd` is the folder a relative path is read
/// against (the pane's, when the page has one).
pub fn open(href: &str, cwd: Option<&Path>) -> Option<PanelTarget> {
    side_panel::target_on_disk(href, cwd)
}

/// `find`, plus the bare issue ids and pull request numbers (`refs`).
pub fn find_with_refs(text: &str, exists: &dyn Fn(&str) -> bool) -> Vec<Link> {
    let mut links = find(text, exists);
    let found = refs(text, &links);
    links.extend(found);
    links.sort_by_key(|l| l.start);
    links
}

/// `CLA-123`, `CLAR-123`, `CLAD-123` and `#12345` standing alone: not glued
/// to a word, a path or a URL, nor inside a link already found.
fn refs(text: &str, taken: &[Link]) -> Vec<Link> {
    let bytes = text.as_bytes();
    let word = |i: usize| bytes[i].is_ascii_alphanumeric() || b"_/-#".contains(&bytes[i]);
    let digits = |from: usize| bytes[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let free = i == 0 || !word(i - 1);
        let found = if !free {
            None
        } else if bytes[i] == b'#' {
            let n = digits(i + 1);
            (n == 5).then(|| (i + 1 + n, Kind::Pull))
        } else {
            ["CLA-", "CLAR-", "CLAD-"].iter().find(|p| bytes[i..].starts_with(p.as_bytes())).and_then(|p| {
                let n = digits(i + p.len());
                (n > 0).then(|| (i + p.len() + n, Kind::Issue))
            })
        };
        match found {
            Some((end, kind))
                if (end == bytes.len() || !bytes[end].is_ascii_alphanumeric() && bytes[end] != b'_')
                    && !taken.iter().any(|l| l.start < end && i < l.end) =>
            {
                out.push(Link { start: i, end, kind, md: None });
                i = end;
            }
            _ => i += 1,
        }
    }
    out
}

fn starts_token(text: &str, at: usize) -> bool {
    text[..at].chars().next_back().is_none_or(|c| c.is_whitespace() || "([{<\"'`*_".contains(c))
}

fn urls(text: &str) -> Vec<Link> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find("http") {
        let at = from + rel;
        from = at + 4;
        let rest = &text[at..];
        if !(rest.starts_with("http://") || rest.starts_with("https://")) || !starts_token(text, at) {
            continue;
        }
        let len = rest.find(|c: char| c.is_whitespace() || "<>\"'`".contains(c)).unwrap_or(rest.len());
        let mut url = &rest[..len];
        // Trailing punctuation is the sentence's, and an unmatched closing
        // bracket the parenthesis around the link.
        loop {
            let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?']);
            let trimmed = match trimmed.chars().last() {
                Some(')') if trimmed.matches('(').count() < trimmed.matches(')').count() => &trimmed[..trimmed.len() - 1],
                Some(']') if trimmed.matches('[').count() < trimmed.matches(']').count() => &trimmed[..trimmed.len() - 1],
                _ => trimmed,
            };
            if trimmed.len() == url.len() {
                break;
            }
            url = trimmed;
        }
        if url.len() > "https://".len() {
            out.push(Link { start: at, end: at + url.len(), kind: Kind::Url, md: None });
            from = at + url.len();
        }
    }
    out
}

fn paths(text: &str, urls: &[Link], exists: &dyn Fn(&str) -> bool) -> Vec<Link> {
    let mut out: Vec<Link> = Vec::new();
    for ext in PANEL_EXTENSIONS {
        let ext = &format!(".{ext}");
        // As written for these two; the file must be there for the others.
        let as_written = ext == ".md" || ext == ".html";
        let mut from = 0;
        while let Some(rel) = text[from..].find(ext.as_str()) {
            let end = from + rel + ext.len();
            from = end;
            let boundary = text[end..].chars().next().is_none_or(|c| c.is_whitespace() || ".,;:!?)]}>\"'`*_".contains(c));
            if !boundary || urls.iter().any(|u| u.start < end && end <= u.end) {
                continue;
            }
            let line_start = text[..end].rfind('\n').map_or(0, |i| i + 1);
            let low = line_start.max(end.saturating_sub(SPACED_PATH_MAX));
            let low = (low..end).find(|&i| text.is_char_boundary(i)).unwrap_or(end);
            // The nearest start of a path on the line; one with spaces only
            // when the file is there.
            let starts = text[low..end]
                .char_indices()
                .map(|(i, _)| low + i)
                .filter(|&i| (text[i..].starts_with('/') || text[i..].starts_with("~/")) && starts_token(text, i));
            let found = starts.collect::<Vec<_>>().into_iter().rev().find(|&i| {
                let candidate = &text[i..end];
                (as_written && !candidate.contains(char::is_whitespace)) || exists(candidate)
            });
            // A path relative to the pane's folder (`docs/plan.md`): the
            // word it ends, when it names a file.
            let found = found.or_else(|| {
                let word = text[low..end].rfind(|c: char| c.is_whitespace() || "([{<\"'`*".contains(c)).map_or(low, |i| low + i + 1);
                let candidate = &text[word..end];
                (word == low || starts_token(text, word)).then_some(word).filter(|_| {
                    candidate.contains('/') && !candidate.starts_with('/') && !candidate.starts_with('~') && !candidate.contains("://") && exists(candidate)
                })
            });
            // Never across a URL nor another path.
            if let Some(start) = found
                && !out.iter().chain(urls).any(|l| l.start < end && start < l.end)
            {
                out.push(Link { start, end, kind: Kind::Path, md: None });
            }
        }
    }
    out
}

/// `text` as HTML: escaped, its links as anchors the page opens in the side
/// panel (`data-path` for a file, `href` for a URL).
pub fn linkify_with(text: &str, exists: &dyn Fn(&str) -> bool) -> String {
    render(text, find(text, exists))
}

/// `linkify_with`, the bare issue ids and pull request numbers linked too.
pub fn linkify_refs_with(text: &str, exists: &dyn Fn(&str) -> bool) -> String {
    render(text, find_with_refs(text, exists))
}

fn render(text: &str, links: Vec<Link>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for link in links {
        if link.start < at {
            continue;
        }
        out.push_str(&escape(&text[at..link.start]));
        out.push_str(&anchor(link.target(text), link.kind));
        out.push_str(&escape(link.label(text)));
        out.push_str("</a>");
        at = link.end;
    }
    out.push_str(&escape(&text[at..]));
    out
}

/// The opening tag of a link to `target`.
pub fn anchor(target: &str, kind: Kind) -> String {
    match kind {
        Kind::Url => format!(r#"<a href="{}">"#, escape(target)),
        Kind::Path => format!(r##"<a href="#" class="path" data-path="{}">"##, escape(target)),
        Kind::Issue => format!(r#"<a href="{LINEAR_ISSUE}{}">"#, escape(target)),
        Kind::Pull => format!(r#"<a href="{GITHUB_PULL}{}">"#, escape(target.trim_start_matches('#'))),
    }
}

/// `linkify_with` against the real filesystem.
pub fn linkify(text: &str) -> String {
    linkify_with(text, &on_disk)
}

/// `linkify_refs_with` against the real filesystem.
pub fn linkify_refs(text: &str) -> String {
    linkify_refs_with(text, &on_disk)
}

/// `path` names a file the panel would open on a click: the same rule as
/// the click itself (`side_panel::target_on_disk`), so a link is never dead.
/// A relative path is read against the pane's folder and its parents.
pub fn on_disk(path: &str) -> bool {
    BASE.with(|b| matches!(side_panel::target_on_disk(path, b.borrow().as_deref()), Some(t) if !matches!(t, PanelTarget::Web(_))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(text: &str, exists: &dyn Fn(&str) -> bool) -> Vec<(String, Kind)> {
        find(text, exists).into_iter().map(|l| (text[l.start..l.end].to_string(), l.kind)).collect()
    }

    fn none(_: &str) -> bool {
        false
    }

    #[test]
    fn urls_are_found_without_the_sentence_s_punctuation() {
        let text = "See https://linear.app/claap/issue/CLA-1, and (https://github.com/a/b/pull/2). Also http://x.io/a_(b)";
        assert_eq!(
            targets(text, &none),
            [
                ("https://linear.app/claap/issue/CLA-1".to_string(), Kind::Url),
                ("https://github.com/a/b/pull/2".to_string(), Kind::Url),
                ("http://x.io/a_(b)".to_string(), Kind::Url),
            ]
        );
        assert!(targets("nothttps://x.io and https://", &none).is_empty(), "inside a word, or nothing after the scheme");
    }

    #[test]
    fn md_and_html_paths_are_found_other_paths_are_not() {
        let text = "Wrote /tmp/report.html and ~/notes/plan.md: read /etc/hosts or ./rel/x.md or src/main.rs, /a/b.mdx";
        assert_eq!(
            targets(text, &none),
            [("/tmp/report.html".to_string(), Kind::Path), ("~/notes/plan.md".to_string(), Kind::Path)]
        );
        assert_eq!(targets("https://x.io/doc.md", &none), [("https://x.io/doc.md".to_string(), Kind::Url)], "a URL is one link");
        assert_eq!(
            targets("see https://x.io/a/b.md?x=1 /c.md", &none),
            [("https://x.io/a/b.md?x=1".to_string(), Kind::Url), ("/c.md".to_string(), Kind::Path)],
            "a path inside a URL is not a second link"
        );
    }

    #[test]
    fn a_path_with_spaces_is_linked_only_when_the_file_exists() {
        let text = "see ~/AI directory/Claap/context/now.md for it";
        let exists = |p: &str| p == "~/AI directory/Claap/context/now.md";
        assert_eq!(targets(text, &exists), [("~/AI directory/Claap/context/now.md".to_string(), Kind::Path)]);
        assert!(targets(text, &none).is_empty());
        assert!(targets("I edited /a/b and c/d.md", &none).is_empty(), "not a path made of a sentence");
        let long = format!("{} /tmp/x.md", "é".repeat(200));
        assert_eq!(targets(&long, &none), [("/tmp/x.md".to_string(), Kind::Path)], "a long accented line: no cut inside a char");
    }

    #[test]
    fn every_file_the_panel_shows_is_linked_when_it_exists() {
        let there = ["/tmp/shot.png", "~/out/run.log", "docs/plan.md", "a b/report.pdf", "~/AI directory/x/data.json"];
        let exists = |p: &str| there.contains(&p);
        let text = "Saved /tmp/shot.png and ~/out/run.log, see docs/plan.md (and ~/AI directory/x/data.json). Not /v1/list.json nor lib/x.txt nor a.md";
        assert_eq!(
            targets(text, &exists),
            [
                ("/tmp/shot.png".to_string(), Kind::Path),
                ("~/out/run.log".to_string(), Kind::Path),
                ("docs/plan.md".to_string(), Kind::Path),
                ("~/AI directory/x/data.json".to_string(), Kind::Path),
            ],
            "an API route, a relative path to nothing and a bare file name stay text"
        );
        assert!(targets("Saved /tmp/shot.png and docs/plan.md", &none).is_empty(), "only when the file is there");
        assert_eq!(targets("https://x.io/docs/plan.md", &exists).len(), 1, "inside a URL: the URL only");
        assert!(is_file_path("docs/plan.md", &exists) && is_file_path("~/AI directory/x/data.json", &exists) && is_file_path("a b/report.pdf", &exists));
        assert!(!is_file_path("docs/plan.md", &none) && !is_file_path("plan.md", &|_| true) && !is_file_path("https://x.io/a.md", &|_| true));
        assert!(!is_file_path("src/main.rs", &|_| true), "not a file the panel shows");
    }

    #[test]
    fn a_markdown_link_is_one_link_named_by_its_label() {
        let exists = |p: &str| p == "~/AI directory/x/plan.md" || p == "docs/a.pdf";
        let html = linkify_refs_with("See [the plan](~/AI directory/x/plan.md), [site](https://x.io/a_b) and [doc](docs/a.pdf). Not [x](javascript:1) nor [y](src/main.rs) nor [z] (a).", &exists);
        assert_eq!(
            html,
            r##"See <a href="#" class="path" data-path="~/AI directory/x/plan.md">the plan</a>, <a href="https://x.io/a_b">site</a> and <a href="#" class="path" data-path="docs/a.pdf">doc</a>. Not [x](javascript:1) nor [y](src/main.rs) nor [z] (a)."##
        );
        assert_eq!(
            super::targets("a [p](~/AI directory/x/plan.md) b https://x.io c https://x.io", &exists),
            ["~/AI directory/x/plan.md", "https://x.io"],
            "what opens, once each"
        );
        assert_eq!(linkify_refs_with("[CLA-12](https://linear.app/claap/issue/CLA-12) (title)", &none), r#"<a href="https://linear.app/claap/issue/CLA-12">CLA-12</a> (title)"#, "the id inside a link is not linked twice");
    }

    #[test]
    fn a_path_travels_as_a_file_url_and_comes_back() {
        let dir = std::env::temp_dir().join(format!("kova links é {}", crate::ask::new_question_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("plan b.md");
        std::fs::write(&file, "x").unwrap();
        let path = file.to_string_lossy().into_owned();
        let url = file_url(&path, None).expect("a file the panel shows");
        assert!(url.starts_with("file:///") && !url.contains(' ') && url.contains("plan%20b.md"), "{url}");
        assert_eq!(path_of_file_url(&url).as_deref(), Some(path.as_str()));
        assert_eq!(file_url(&dir.join("none.md").to_string_lossy(), None), None, "nothing there, no link");
        let text = format!("Read {path} for CLA-7 and #12345.");
        let got = pieces(&text);
        assert_eq!(got[0], ("Read ".to_string(), None));
        assert_eq!(got[1], (path.clone(), Some(url)));
        assert_eq!(got[3], ("CLA-7".to_string(), Some("https://linear.app/claap/issue/CLA-7".to_string())));
        assert_eq!(got[5].1.as_deref(), Some("https://github.com/claap-app/claap/pull/12345"));
        assert_eq!(got.iter().map(|(t, _)| t.as_str()).collect::<String>(), text, "the pieces give the text back");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_lone_link_reads_as_a_file_name_or_a_short_url() {
        assert_eq!(label("~/AI directory/x/plan.md#top", 70), "plan.md");
        assert_eq!(label("https://linear.app/x/CLA-1", 70), "linear.app/x/CLA-1");
        assert_eq!(label("https://linear.app/x/CLA-1", 6), "linear\u{2026}");
    }

    #[test]
    fn linkify_escapes_and_tags_each_kind() {
        let html = linkify_with("<b> /x/y.md & https://a.b/?q=1&r=2", &none);
        assert_eq!(
            html,
            r##"&lt;b&gt; <a href="#" class="path" data-path="/x/y.md">/x/y.md</a> &amp; <a href="https://a.b/?q=1&amp;r=2">https://a.b/?q=1&amp;r=2</a>"##
        );
    }

    #[test]
    fn bare_issue_ids_and_pull_numbers_link_to_linear_and_github() {
        let html = linkify_refs_with("Fixed CLA-9513 and CLAR-12 (CLAD-3), see #13544.", &none);
        assert_eq!(
            html,
            r#"Fixed <a href="https://linear.app/claap/issue/CLA-9513">CLA-9513</a> and <a href="https://linear.app/claap/issue/CLAR-12">CLAR-12</a> (<a href="https://linear.app/claap/issue/CLAD-3">CLAD-3</a>), see <a href="https://github.com/claap-app/claap/pull/13544">#13544</a>."#
        );
        assert_eq!(linkify_with("CLA-1 #13544", &none), "CLA-1 #13544", "only answers get them");
    }

    #[test]
    fn ids_inside_a_url_a_word_or_too_short_stay_text() {
        let refs_of = |t: &str| find_with_refs(t, &none).into_iter().filter(|l| matches!(l.kind, Kind::Issue | Kind::Pull)).count();
        assert_eq!(refs_of("https://linear.app/claap/issue/CLA-9513"), 0);
        assert_eq!(refs_of("https://github.com/claap-app/claap/pull/1#13544 x"), 0);
        assert_eq!(refs_of("linear.app/claap/issue/CLA-1 and a#12345"), 0);
        assert_eq!(refs_of("#1234, #123456, CLA-, XCLA-1, CLA-12a"), 0);
        assert_eq!(refs_of("CLA-1."), 1);
        assert_eq!(refs_of("été CLA-2 — #12345"), 2, "after accented text, no cut inside a char");
    }
}
