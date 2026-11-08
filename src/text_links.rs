//! Links inside an ask's text (card, decision sheet, long question): bare URLs
//! and markdown `[label](url)`, cut into spans so each surface makes them
//! clickable its own way. Pure and tested.

/// The schemes a link may use: the web, and the apps Robin's agents link to.
/// Anything else (`javascript:`, `file:`) stays plain text.
const SCHEMES: [&str; 5] = ["https://", "http://", "slack://", "linear://", "notion://"];

/// A piece of text: plain, or a link with its label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Link { label: String, url: String },
}

fn scheme_at(s: &str) -> bool {
    SCHEMES.iter().any(|p| s.len() >= p.len() && s.is_char_boundary(p.len()) && s[..p.len()].eq_ignore_ascii_case(p))
}

/// Whether `url` is one a span may carry.
pub fn allowed(url: &str) -> bool {
    scheme_at(url) && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The length in bytes of the bare URL starting `s`: up to a space or a quote,
/// without the punctuation that ends a sentence, and without a closing
/// bracket that the URL did not open.
fn url_len(s: &str) -> usize {
    let end = s
        .char_indices()
        .find(|&(_, c)| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '`' | '«' | '»'))
        .map_or(s.len(), |(i, _)| i);
    let mut url = &s[..end];
    loop {
        let Some(last) = url.chars().last() else { break };
        let unopened = |open: char| url.matches(open).count() < url.matches(last).count();
        let trim = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' => true,
            ')' => unopened('('),
            ']' => unopened('['),
            '}' => unopened('{'),
            _ => false,
        };
        if !trim {
            break;
        }
        url = &url[..url.len() - last.len_utf8()];
    }
    url.len()
}

/// `[label](url)` at the start of `s`: the label, the url, the bytes taken.
fn markdown_at(s: &str) -> Option<(String, String, usize)> {
    let rest = s.strip_prefix('[')?;
    let close = rest.find("](")?;
    let label = &rest[..close];
    if label.is_empty() || label.contains('\n') || label.contains('[') {
        return None;
    }
    let after = &rest[close + 2..];
    let end = after.find(')')?;
    let url = after[..end].trim();
    allowed(url).then(|| (label.to_string(), url.to_string(), 1 + close + 2 + end + 1))
}

/// `text` cut into plain text and links. Concatenating the plain texts and
/// the labels gives back what the user reads.
pub fn spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if let Some((label, url, taken)) = markdown_at(rest) {
            flush(&mut out, &mut plain);
            out.push(Span::Link { label, url });
            i += taken;
            continue;
        }
        let starts_word = text[..i].chars().last().is_none_or(|c| !c.is_alphanumeric());
        if starts_word && scheme_at(rest) {
            let len = url_len(rest);
            let url = &rest[..len];
            if SCHEMES.iter().all(|p| url.len() > p.len()) {
                flush(&mut out, &mut plain);
                out.push(Span::Link { label: url.to_string(), url: url.to_string() });
                i += len;
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        plain.push(c);
        i += c.len_utf8();
    }
    flush(&mut out, &mut plain);
    out
}

fn flush(out: &mut Vec<Span>, plain: &mut String) {
    if !plain.is_empty() {
        out.push(Span::Text(std::mem::take(plain)));
    }
}

/// Whether a clicked link opens in Kova's side panel (the web) rather than in
/// the app its scheme names (Slack, Linear, Notion).
pub fn opens_in_panel(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(label: &str, url: &str) -> Span {
        Span::Link { label: label.into(), url: url.into() }
    }
    fn text(t: &str) -> Span {
        Span::Text(t.into())
    }

    #[test]
    fn bare_urls_stop_before_the_sentence_punctuation() {
        assert_eq!(
            spans("See https://linear.app/claap/issue/CLA-12. Then (https://x.com/a_(b)) ok"),
            vec![
                text("See "),
                link("https://linear.app/claap/issue/CLA-12", "https://linear.app/claap/issue/CLA-12"),
                text(". Then ("),
                link("https://x.com/a_(b)", "https://x.com/a_(b)"),
                text(") ok"),
            ]
        );
        assert_eq!(
            spans("Fil : https://claapio.slack.com/archives/C0/p17\n- next"),
            vec![text("Fil : "), link("https://claapio.slack.com/archives/C0/p17", "https://claapio.slack.com/archives/C0/p17"), text("\n- next")]
        );
    }

    #[test]
    fn markdown_links_keep_their_label() {
        assert_eq!(
            spans("Ticket [CLA-12](https://linear.app/x) and [app](slack://channel?id=C1)"),
            vec![text("Ticket "), link("CLA-12", "https://linear.app/x"), text(" and "), link("app", "slack://channel?id=C1")]
        );
    }

    #[test]
    fn only_known_schemes_become_links() {
        assert_eq!(spans("[x](javascript:alert(1)) file:///etc"), vec![text("[x](javascript:alert(1)) file:///etc")]);
        assert_eq!(spans("nothttps://a.b"), vec![text("nothttps://a.b")]);
        assert_eq!(spans("https:// alone"), vec![text("https:// alone")]);
        assert_eq!(spans("é https://a.fr/é»"), vec![text("é "), link("https://a.fr/é", "https://a.fr/é"), text("»")]);
    }

    #[test]
    fn web_links_open_in_the_panel_app_links_in_their_app() {
        assert!(opens_in_panel("https://linear.app/x"));
        assert!(opens_in_panel("HTTP://a.b"));
        assert!(!opens_in_panel("slack://open"));
        assert!(!opens_in_panel("notion://www.notion.so/x"));
    }
}
