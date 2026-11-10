//! Ticket and pull request ids in a text (`CLA-123`, `#12345`), and their
//! titles from a cache file, so a page can show what an id is about next to
//! it instead of asking the agent to write it.
//!
//! The cache is a flat JSON object written by an outside tool:
//! `{"CLA-10229": {"title": "…", "url": "…" or null}, "#13951": {…}}`
//! (`[links] refs_file`). Read again when the file changes; a missing or
//! broken file is an empty cache, never an error.
//!
//! An id already told in its text gets nothing: `told` reads the forms a
//! reader takes for a subject, the ones the harness's form check accepts
//! (`claude-config/bin/formcheck`, `check_subject`): `CLA-1 (subject)`,
//! `CLA-1 : subject`, `#13573, which keeps the screen on`, `| #13908 |
//! subject |`, `subject (CLA-1)`, and any other mention of the same id that
//! carries one. Nothing here changes a text: a title is only drawn.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Longest title drawn after an id, in chars.
pub const TITLE_MAX: usize = 90;
/// How often the cache file's date is looked at.
const STAT_EVERY: Duration = Duration::from_secs(2);

/// What the cache knows of an id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub title: String,
    pub url: Option<String>,
}

#[derive(Default)]
struct Cache {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    checked: Option<Instant>,
    refs: HashMap<String, Ref>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// `[links] refs_file`, set at load and on a config reload (`~/` expanded
/// by the caller). `None`: no cache, no title.
pub fn set_file(path: Option<&Path>) {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(Cache { path: path.map(Path::to_path_buf), ..Cache::default() });
    }
}

/// The cache's entry for `key` (`CLA-123`, `#12345`).
pub fn lookup(key: &str) -> Option<Ref> {
    let mut guard = CACHE.lock().ok()?;
    let cache = guard.as_mut()?;
    let path = cache.path.clone()?;
    if cache.checked.is_none_or(|t| t.elapsed() >= STAT_EVERY) {
        cache.checked = Some(Instant::now());
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if modified != cache.modified || modified.is_none() {
            cache.modified = modified;
            cache.refs = std::fs::read_to_string(&path).map(|raw| parse(&raw)).unwrap_or_default();
        }
    }
    cache.refs.get(key).cloned()
}

/// The cache file's content: entries without a title are dropped, a `url`
/// that is not http(s) is no url.
pub fn parse(raw: &str) -> HashMap<String, Ref> {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) else { return HashMap::new() };
    map.into_iter()
        .filter_map(|(key, v)| {
            let title = v.get("title")?.as_str()?.split_whitespace().collect::<Vec<_>>().join(" ");
            let url = v.get("url").and_then(|u| u.as_str()).filter(|u| crate::links::is_web(u) && !u.contains(char::is_whitespace));
            (!title.is_empty()).then(|| (key, Ref { title, url: url.map(str::to_string) }))
        })
        .collect()
}

/// A title as drawn after its id: `TITLE_MAX` chars at most.
pub fn short_title(title: &str) -> String {
    if title.chars().count() <= TITLE_MAX {
        return title.to_string();
    }
    let mut out: String = title.chars().take(TITLE_MAX).collect();
    out.push('\u{2026}');
    out
}

/// One id in a text: its bytes and its cache key (`PR #13937`, `PR13937`
/// and `#13937` are the same pull request, `#13937`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    pub start: usize,
    pub end: usize,
    pub key: String,
}

fn letter(c: char) -> bool {
    c.is_ascii_alphabetic() || ('\u{C0}'..='\u{FF}').contains(&c)
}

/// The id starting at byte `i` of `text`, if one does: its end and key.
fn id_at(text: &str, i: usize) -> Option<(usize, String)> {
    let bytes = text.as_bytes();
    let digits = |from: usize| bytes[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    // A letter of any alphabet before or after glues the id to a word.
    let prev = text[..i].chars().next_back();
    let free = prev.is_none_or(|c| !c.is_alphanumeric() && c != '_');
    let ends = |end: usize| text[end..].chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '_');
    if bytes[i] == b'#' {
        let n = digits(i + 1);
        let glued = !free || prev == Some('/');
        return ((4..=6).contains(&n) && !glued && ends(i + 1 + n)).then(|| (i + 1 + n, text[i..i + 1 + n].to_string()));
    }
    if !free {
        return None;
    }
    for prefix in ["CLAR-", "CLAD-", "CLA-"] {
        if text[i..].starts_with(prefix) {
            let n = digits(i + prefix.len());
            let end = i + prefix.len() + n;
            return (n > 0 && ends(end)).then(|| (end, text[i..end].to_string()));
        }
    }
    if text[i..].starts_with("PR") {
        let mut at = i + 2;
        if bytes.get(at) == Some(&b' ') {
            at += 1;
        }
        if bytes.get(at) == Some(&b'#') {
            at += 1;
        }
        let n = digits(at);
        return ((3..=6).contains(&n) && ends(at + n)).then(|| (at + n, format!("#{}", &text[at..at + n])));
    }
    None
}

/// Every id of `text`, in order.
pub fn mentions(text: &str) -> Vec<Mention> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text.is_char_boundary(i)
            && let Some((end, key)) = id_at(text, i)
        {
            out.push(Mention { start: i, end, key });
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// `text` as a reader takes it for this check: a markdown link reads as its
/// label, a URL and a code span as nothing.
fn readable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        // `[label](http…|/…|~…)`
        if let Some(inner) = rest.strip_prefix('[')
            && let Some(close) = inner.find("](")
            && !inner[..close].contains([']', '\n'])
            && (1..=200).contains(&close)
        {
            let dest = &inner[close + 2..];
            if (dest.starts_with("http://") || dest.starts_with("https://") || dest.starts_with('/') || dest.starts_with('~'))
                && let Some(end) = dest.find(|c: char| c == ')' || c.is_whitespace()).filter(|&e| dest[e..].starts_with(')'))
            {
                out.push_str(&inner[..close]);
                rest = &dest[end + 1..];
                continue;
            }
        }
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            out.push(' ');
            rest = &rest[end..];
            continue;
        }
        if let Some(code) = rest.strip_prefix('`')
            && let Some(end) = code.find('`')
        {
            out.push(' ');
            rest = &code[end + 1..];
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Three letters in a row.
fn has_word(s: &str) -> bool {
    let mut run = 0;
    s.chars().any(|c| {
        run = if letter(c) { run + 1 } else { 0 };
        run >= 3
    })
}

/// `s` with its ids blanked: `#13734 (CLA-10107)` tells nothing.
fn without_ids(s: &str) -> String {
    let mut out = s.to_string();
    for m in mentions(s).iter().rev() {
        out.replace_range(m.start..m.end, " ");
    }
    out
}

/// A subject in parentheses after the id, at most three words away:
/// `CLA-1 (subject)`, `#13535 du matin (session live, CLA-9909 (…`.
fn subject_after(after: &str) -> bool {
    // On the id's line: the next line's parenthesis is about something else.
    let mut rest = after.split('\n').next().unwrap_or("").trim_start();
    for _ in 0..=3 {
        if let Some(inner) = rest.strip_prefix('(') {
            let end = inner.find(['(', ')']).unwrap_or(inner.len());
            return inner[end..].starts_with(['(', ')']) && has_word(&without_ids(&inner[..end]));
        }
        let word = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if word == 0 || word == rest.len() || rest[..word].contains(['(', ')', ',', '.', ';', ':']) {
            return false;
        }
        rest = rest[word..].trim_start();
    }
    false
}

/// A subject in plain words right after the id: `CLA-1 : subject`,
/// `#13573, which keeps the screen on`, `| #13908 | subject |`. Its first
/// word is a word, not another id: `CLA-1, CLA-2` stays a list.
fn subject_inline(after: &str) -> bool {
    let rest = after.trim_start_matches('*').trim_start();
    let comma = rest.starts_with(',');
    let Some(rest) = rest.strip_prefix([':', ',', '|']) else { return false };
    let rest = rest.trim_start().trim_start_matches('*').trim_start();
    if rest.is_empty() || id_at(rest, 0).is_some() {
        return false;
    }
    // `#13900, et son ticket …`: a comma then a conjunction goes on with the
    // sentence, it does not say what the id is (stricter than the form check).
    let first: String = rest.chars().take_while(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
    if comma && matches!(first.as_str(), "et" | "and" | "ou" | "or" | "puis" | "then" | "mais" | "but" | "donc" | "so") {
        return false;
    }
    let line = rest.split(['|', '\n']).next().unwrap_or("");
    // A word that starts within the first 30 chars.
    let head: String = line.chars().take(33).collect();
    has_word(&head)
}

/// The ids of `text` whose subject the text gives, at any of their
/// mentions: these need no title.
pub fn told(text: &str) -> HashSet<String> {
    let t = readable(text);
    let all = mentions(&t);
    let mut out = HashSet::new();
    let mut i = 0;
    while i < all.len() {
        // A run: `CLA-1, CLA-2 et #3` shares what follows it.
        let mut j = i;
        while j + 1 < all.len() && matches!(t[all[j].end..all[j + 1].start].trim(), "," | "et" | "and" | "/" | "+") {
            j += 1;
        }
        let before = &t[..all[i].start];
        let after = &t[all[j].end..];
        // Inside an open parenthesis: a subject or another id frames it.
        let framed = before.matches('(').count() > before.matches(')').count();
        if framed || subject_after(after) || subject_inline(after) {
            out.extend(all[i..=j].iter().map(|m| m.key.clone()));
        }
        i = j + 1;
    }
    out
}

/// The ids of `text` that get their title drawn: mentioned, not told by the
/// text, and known to `lookup` with a title. Each once.
pub fn untold(text: &str, lookup: &dyn Fn(&str) -> Option<Ref>) -> HashSet<String> {
    let told = told(text);
    mentions(&readable(text)).into_iter().map(|m| m.key).filter(|k| !told.contains(k) && lookup(k).is_some()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<String> {
        mentions(text).into_iter().map(|m| m.key).collect()
    }

    fn told_of(text: &str) -> Vec<String> {
        let mut v: Vec<String> = told(text).into_iter().collect();
        v.sort();
        v
    }

    #[test]
    fn ids_are_found_with_one_key_per_pull_request() {
        assert_eq!(keys("CLA-12, CLAR-3 (CLAD-4), #13544, PR #13937, PR13938 and PR 139."), ["CLA-12", "CLAR-3", "CLAD-4", "#13544", "#13937", "#13938", "#139"]);
        assert!(keys("XCLA-1 CLA- CLA-12a a#12345 x/#12345 #123 #1234567 PR 12 étéCLA-1").is_empty(), "{:?}", keys("XCLA-1 CLA- CLA-12a a#12345 x/#12345 #123 #1234567 PR 12"));
        assert_eq!(keys("été CLA-2 — #12345"), ["CLA-2", "#12345"], "after accented text, no cut inside a char");
    }

    /// The forms `formcheck`'s `check_subject` accepts, on its own examples.
    #[test]
    fn a_subject_is_read_in_the_forms_the_form_check_accepts() {
        assert_eq!(told_of("CLA-1 (login fix) is ready"), ["CLA-1"]);
        assert_eq!(told_of("#13535 (session live, CLA-9909 (phrases)) attend"), ["#13535", "CLA-9909"], "the inner id is framed by the parenthesis");
        assert_eq!(told_of("dictée desktop (CLA-10105) livrée"), ["CLA-10105"], "the id after its subject");
        assert_eq!(told_of("CLA-1 : le test du déplacement"), ["CLA-1"]);
        assert_eq!(told_of("#13573, qui garde l'écran allumé"), ["#13573"]);
        assert_eq!(told_of("| #13908 | source Zoom gardée |"), ["#13908"]);
        assert_eq!(told_of("**CLA-7** : notes perso"), ["CLA-7"]);
        assert_eq!(told_of("[CLA-10194](https://linear.app/x/CLA-10194) (Start now) and [CLA-2](https://l.app/CLA-2) : le test"), ["CLA-10194", "CLA-2"], "after a link");
        assert_eq!(told_of("CLA-1, CLA-2 et #13544 (les trois parties de la détection)"), ["#13544", "CLA-1", "CLA-2"], "a run shares its subject");
        assert_eq!(told_of("#13905 (live transcript) first. Otherwise #13905 merges"), ["#13905"]);
        // Not a subject.
        assert!(told_of("CLA-10040 et CLA-9905 attendent ta relecture.").is_empty());
        assert!(told_of("#13734 (CLA-10107)").iter().all(|k| k != "#13734"), "another id tells nothing");
        assert!(told_of("CLA-1, CLA-2").is_empty(), "a list");
        assert!(told_of("Le dernier est #13900, et son ticket est passé en release.").is_empty(), "a comma then a conjunction: the sentence goes on");
        assert_eq!(told_of("- CLA-5 too\n- #13937 (sign out fix)"), ["#13937"], "a subject on the next line is the next id's");
        assert!(told_of("Je merge #13573. Ensuite on verra (peut-être).").is_empty(), "a parenthesis more than three words away");
        assert!(told_of("see `CLA-9 (x)` and https://x.io/CLA-8 (page)").is_empty(), "code and URLs are not read");
    }

    #[test]
    fn only_an_id_the_cache_knows_and_the_text_does_not_tell_gets_a_title() {
        let lookup = |k: &str| matches!(k, "CLA-10040" | "CLA-9914" | "#13937").then(|| Ref { title: "t".into(), url: None });
        let text = "Les 4 sous-tickets de CLA-9914 (vidéo de réunion envoyée pendant l'appel) sont prêts. CLA-10040 et CLA-9905 attendent. PR #13937 aussi, et CLA-10040 encore.";
        let mut got: Vec<String> = untold(text, &lookup).into_iter().collect();
        got.sort();
        assert_eq!(got, ["#13937", "CLA-10040"], "told: no; unknown to the cache: no");
    }

    #[test]
    fn the_cache_file_reads_titles_and_safe_urls() {
        let refs = parse(r##"{"CLA-1": {"title": " Fix  the\nlogin ", "url": "https://linear.app/x/CLA-1", "src": "linear", "ts": "x"},
            "#12": {"title": "feat: x", "url": null}, "CLA-2": {"title": "", "url": "https://x"}, "CLA-3": {"url": "https://x"},
            "CLA-4": {"title": "t", "url": "javascript:alert(1)"}, "CLA-5": "nope"}"##);
        assert_eq!(refs.get("CLA-1"), Some(&Ref { title: "Fix the login".into(), url: Some("https://linear.app/x/CLA-1".into()) }));
        assert_eq!(refs.get("#12"), Some(&Ref { title: "feat: x".into(), url: None }));
        assert_eq!(refs.get("CLA-4").map(|r| r.url.clone()), Some(None), "only an http(s) url");
        assert_eq!(refs.len(), 3, "no title, no entry");
        assert!(parse("[1]").is_empty() && parse("not json").is_empty());
        assert_eq!(short_title(&"é".repeat(100)).chars().count(), TITLE_MAX + 1);
    }
}
