//! What the side panel opened lately: pages, artifacts, markdown and HTML
//! files, decision sheets. Most recent first, one entry per target, at most
//! [`MAX`], kept in `~/.config/kova/panel-history.json` so a relaunch keeps
//! it. The panel shows it as a page of its own (the header's clock button,
//! View > Panel History, `panel_history` key), and a click on an entry opens
//! it again. The list, its page and the file format are pure and tested; the
//! one store lives on the main thread.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::side_panel::PanelTarget;
use super::sidebar::tokens;

/// How many entries are kept.
pub const MAX: usize = 50;
/// The scheme of an entry's link on the history page: `kova-history:<index>`.
pub const LINK_SCHEME: &str = "kova-history";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "target", rename_all = "lowercase")]
pub enum Stored {
    Web(String),
    Markdown(PathBuf),
    Html(PathBuf),
    Sheet(String),
    File(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(flatten)]
    pub target: Stored,
    #[serde(default)]
    pub title: String,
    /// Epoch seconds of the last time it was opened.
    #[serde(default)]
    pub opened_at: u64,
}

impl Stored {
    /// What the panel keeps of a target; `None` for the history page itself.
    pub fn of(target: &PanelTarget) -> Option<Stored> {
        Some(match target {
            PanelTarget::Web(u) => Stored::Web(u.clone()),
            PanelTarget::Markdown(p) => Stored::Markdown(p.clone()),
            PanelTarget::Html(p) => Stored::Html(p.clone()),
            PanelTarget::File(p) => Stored::File(p.clone()),
            PanelTarget::Sheet(id) => Stored::Sheet(id.clone()),
            PanelTarget::History | PanelTarget::Question(_) => return None,
        })
    }

    pub fn target(&self) -> PanelTarget {
        match self {
            Stored::Web(u) => PanelTarget::Web(u.clone()),
            Stored::Markdown(p) => PanelTarget::Markdown(p.clone()),
            Stored::Html(p) => PanelTarget::Html(p.clone()),
            Stored::File(p) => PanelTarget::File(p.clone()),
            Stored::Sheet(id) => PanelTarget::Sheet(id.clone()),
        }
    }

    /// Read from disk: a file that anyone can edit, so only what the panel
    /// would open anyway (an http(s) URL, a plain question id).
    fn valid(&self) -> bool {
        match self {
            Stored::Web(u) => crate::ask::is_panel_url(u),
            Stored::Markdown(p) | Stored::Html(p) | Stored::File(p) => p.is_absolute(),
            Stored::Sheet(id) => crate::ask::is_safe_question_id(id),
        }
    }
}

/// The history, most recent first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    pub entries: Vec<Entry>,
}

impl History {
    /// `target` was opened at `now`: to the top, once, with `title` (kept
    /// from the last time when empty).
    pub fn record(&mut self, target: Stored, title: &str, now: u64) {
        let old = self.entries.iter().position(|e| e.target == target).map(|i| self.entries.remove(i));
        let title = if title.trim().is_empty() { old.map(|e| e.title).unwrap_or_default() } else { title.trim().to_string() };
        self.entries.insert(0, Entry { target, title, opened_at: now });
        self.entries.truncate(MAX);
    }

    /// The page told its name: the entry keeps it. True when it changed.
    pub fn retitle(&mut self, target: &Stored, title: &str) -> bool {
        let title = title.trim();
        match self.entries.iter_mut().find(|e| &e.target == target) {
            Some(e) if !title.is_empty() && e.title != title => {
                e.title = title.to_string();
                true
            }
            _ => false,
        }
    }

    pub fn from_json(raw: &str) -> History {
        let mut entries: Vec<Entry> = serde_json::from_str::<Vec<serde_json::Value>>(raw)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| serde_json::from_value::<Entry>(v).ok())
            .filter(|e| e.target.valid())
            .collect();
        entries.truncate(MAX);
        History { entries }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.entries).unwrap_or_else(|_| "[]".into())
    }
}

/// When it was opened, from `now`: `Just now`, `12 min ago`, `3 h ago`,
/// `Yesterday`, `4 days ago`.
pub fn ago(then: u64, now: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..=59 => "Just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        86_400..=172_799 => "Yesterday".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// The second line of an entry: the host of a page, the path of a file
/// (`~/` for the home folder), or what it is.
pub fn detail(target: &Stored, home: Option<&Path>) -> String {
    let short = |p: &Path| match home.and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    };
    match target {
        Stored::Web(u) => super::side_panel::header_title(None, Some(u)).unwrap_or_else(|| u.clone()),
        Stored::Markdown(p) | Stored::Html(p) | Stored::File(p) => short(p),
        Stored::Sheet(_) => "Decision sheet in Kova".into(),
    }
}

/// The name an entry shows: its title, else the file name or the URL.
fn name(e: &Entry) -> String {
    if !e.title.trim().is_empty() {
        return e.title.clone();
    }
    e.target.target().fallback_title()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// The history as a page: one row per entry (name, where it is, when),
/// each a link the panel catches (`kova-history:<index>`). No script.
pub fn render(history: &History, now: u64, home: Option<&Path>, palette: &tokens::Palette) -> String {
    let p = palette;
    let rows: String = if history.entries.is_empty() {
        "<p class=\"empty\">Nothing opened yet. Cmd+click a link or a .md file in a pane, or open a decision</p>".into()
    } else {
        history
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                format!(
                    "<a class=\"row\" href=\"{scheme}:{i}\"><span class=\"main\"><span class=\"name\">{name}</span><span class=\"detail\">{detail}</span></span><span class=\"when\">{when}</span></a>\n",
                    scheme = LINK_SCHEME,
                    name = esc(&name(e)),
                    detail = esc(&detail(&e.target, home)),
                    when = esc(&ago(e.opened_at, now)),
                )
            })
            .collect()
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>History</title><style>
:root {{ color-scheme: {scheme}; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; padding: 18px 14px 32px; background: {bg}; color: {t1}; font: 14px/1.45 "Inter", -apple-system, system-ui, sans-serif; }}
h1 {{ font-size: 11px; letter-spacing: .06em; text-transform: uppercase; color: {t3}; font-weight: 600; margin: 0 6px 8px; }}
.row {{ display: flex; align-items: center; gap: 12px; padding: 8px 10px; border-radius: 8px; text-decoration: none; color: inherit; }}
.row:hover {{ background: {hover}; }}
.main {{ display: grid; min-width: 0; flex: 1; }}
.name {{ font-weight: 500; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }}
.detail {{ color: {t3}; font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }}
.when {{ color: {t3}; font-size: 12px; white-space: nowrap; font-variant-numeric: tabular-nums; }}
.empty {{ color: {t2}; margin: 6px; }}
</style></head><body><h1>Recently opened</h1>
{rows}</body></html>"#,
        scheme = if p == &tokens::LIGHT { "light" } else { "dark" },
        bg = hex(p.ground),
        t1 = hex(p.text_primary),
        t2 = hex(p.text_secondary),
        t3 = hex(p.text_tertiary),
        hover = hex(p.tile_hover),
        rows = rows,
    )
}

// ---------------------------------------------------------------
// The store (main thread)
// ---------------------------------------------------------------

fn path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/kova/panel-history.json"))
}

thread_local! {
    static STORE: RefCell<Option<History>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut History) -> R) -> R {
    STORE.with(|s| {
        let mut s = s.borrow_mut();
        let h = s.get_or_insert_with(|| {
            path().and_then(|p| std::fs::read_to_string(p).ok()).map(|raw| History::from_json(&raw)).unwrap_or_default()
        });
        f(h)
    })
}

fn save(h: &History) {
    let Some(p) = path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, h.to_json()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// The panel opened `target`.
pub fn record(target: &PanelTarget, title: &str) {
    let Some(stored) = Stored::of(target) else { return };
    with(|h| {
        h.record(stored, title, crate::ask::now_epoch());
        save(h);
    });
}

/// The panel learnt `target`'s title.
pub fn retitle(target: &PanelTarget, title: &str) {
    let Some(stored) = Stored::of(target) else { return };
    with(|h| {
        if h.retitle(&stored, title) {
            save(h);
        }
    });
}

/// The entry the history page's link `index` names.
pub fn entry(index: usize) -> Option<PanelTarget> {
    with(|h| h.entries.get(index).map(|e| e.target.target()))
}

/// The history page, now.
pub fn page() -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    with(|h| render(h, crate::ask::now_epoch(), home.as_deref(), tokens::get()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web(u: &str) -> Stored {
        Stored::Web(u.into())
    }

    #[test]
    fn the_last_opened_comes_first_once() {
        let mut h = History::default();
        h.record(web("https://a.example/1"), "One", 10);
        h.record(Stored::Markdown("/x/plan.md".into()), "", 20);
        h.record(web("https://a.example/1"), "", 30);
        assert_eq!(h.entries.len(), 2);
        assert_eq!(h.entries[0].target, web("https://a.example/1"));
        assert_eq!(h.entries[0].title, "One", "an empty title keeps the old one");
        assert_eq!(h.entries[0].opened_at, 30);
        for i in 0..80 {
            h.record(web(&format!("https://a.example/n{i}")), "", 100 + i);
        }
        assert_eq!(h.entries.len(), MAX);
        assert!(h.retitle(&web("https://a.example/n79"), "Latest"));
        assert!(!h.retitle(&web("https://a.example/n79"), "Latest"));
        assert_eq!(h.entries[0].title, "Latest");
    }

    #[test]
    fn the_file_round_trips_and_drops_what_the_panel_would_not_open() {
        let mut h = History::default();
        h.record(Stored::Sheet("0db46adb-ddc5-465a-a073-6add251c9eee".into()), "Plan", 1);
        h.record(Stored::Html("/tmp/r.html".into()), "", 2);
        h.record(web("https://claude.ai/artifact/x"), "Sheet", 3);
        let back = History::from_json(&h.to_json());
        assert_eq!(back, h);
        let raw = r#"[{"kind":"web","target":"javascript:alert(1)"},{"kind":"markdown","target":"rel/x.md"},
            {"kind":"sheet","target":"../x"},{"kind":"web","target":"https://ok.example","title":"Ok","opened_at":5},{"bogus":1}]"#;
        let h = History::from_json(raw);
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].title, "Ok");
        assert_eq!(History::from_json("not json"), History::default());
    }

    #[test]
    fn the_page_lists_name_place_and_time_escaped() {
        let mut h = History::default();
        h.record(Stored::Markdown("/Users/me/notes/<plan>.md".into()), "", 0);
        h.record(web("https://claude.ai/artifact/x"), "Voice <b>typing</b>", 3580);
        let html = render(&h, 3600, Some(Path::new("/Users/me")), &tokens::DARK);
        assert!(html.contains("href=\"kova-history:0\""));
        assert!(html.contains("Voice &lt;b&gt;typing&lt;/b&gt;"));
        assert!(html.contains("claude.ai"));
        assert!(html.contains("~/notes/&lt;plan&gt;.md"));
        assert!(html.contains("Just now") && html.contains("1 h ago"));
        assert!(!html.contains("<script"));
        assert!(render(&History::default(), 0, None, &tokens::LIGHT).contains("Nothing opened yet"));
    }

    #[test]
    fn ago_reads_like_a_person() {
        assert_eq!(ago(100, 130), "Just now");
        assert_eq!(ago(0, 12 * 60), "12 min ago");
        assert_eq!(ago(0, 3 * 3600), "3 h ago");
        assert_eq!(ago(0, 30 * 3600), "Yesterday");
        assert_eq!(ago(0, 4 * 86_400), "4 days ago");
        assert_eq!(ago(50, 10), "Just now", "a clock that went back");
    }
}
