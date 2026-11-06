//! Links in the chat's text: http(s) URLs, and absolute or `~/` paths to
//! local `.md` and `.html` files, the targets the side panel shows
//! (`window::side_panel::target_for`). A click opens them there. Any other
//! path stays text.
//!
//! A path may hold spaces (`~/AI directory/x.md`): one is linked only when
//! the file exists, so a sentence that happens to start with `/` is never
//! taken for one.

use super::page::escape;

/// A link found in a text: its bytes in the text, and what it opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Url,
    Path,
}

/// The longest path with spaces looked for, in bytes.
const SPACED_PATH_MAX: usize = 240;

/// Whether a link destination is a path the panel shows: absolute or `~/`,
/// ending in `.md` or `.html`.
pub fn is_panel_path(raw: &str) -> bool {
    (raw.starts_with('/') || raw.starts_with("~/")) && (raw.ends_with(".md") || raw.ends_with(".html"))
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
    links.sort_by_key(|l| l.start);
    links
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
            out.push(Link { start: at, end: at + url.len(), kind: Kind::Url });
            from = at + url.len();
        }
    }
    out
}

fn paths(text: &str, urls: &[Link], exists: &dyn Fn(&str) -> bool) -> Vec<Link> {
    let mut out: Vec<Link> = Vec::new();
    for ext in [".md", ".html"] {
        let mut from = 0;
        while let Some(rel) = text[from..].find(ext) {
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
                !candidate.contains(char::is_whitespace) || exists(candidate)
            });
            // Never across a URL nor another path.
            if let Some(start) = found
                && !out.iter().chain(urls).any(|l| l.start < end && start < l.end)
            {
                out.push(Link { start, end, kind: Kind::Path });
            }
        }
    }
    out
}

/// `text` as HTML: escaped, its links as anchors the page opens in the side
/// panel (`data-path` for a file, `href` for a URL).
pub fn linkify_with(text: &str, exists: &dyn Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for link in find(text, exists) {
        if link.start < at {
            continue;
        }
        out.push_str(&escape(&text[at..link.start]));
        let target = &text[link.start..link.end];
        out.push_str(&anchor(target, link.kind));
        out.push_str(&escape(target));
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
    }
}

/// `linkify_with` against the real filesystem.
pub fn linkify(text: &str) -> String {
    linkify_with(text, &on_disk)
}

fn on_disk(path: &str) -> bool {
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => std::path::PathBuf::from(path),
    };
    expanded.is_file()
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
    fn linkify_escapes_and_tags_each_kind() {
        let html = linkify_with("<b> /x/y.md & https://a.b/?q=1&r=2", &none);
        assert_eq!(
            html,
            r##"&lt;b&gt; <a href="#" class="path" data-path="/x/y.md">/x/y.md</a> &amp; <a href="https://a.b/?q=1&amp;r=2">https://a.b/?q=1&amp;r=2</a>"##
        );
    }
}
