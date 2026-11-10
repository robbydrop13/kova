//! The catch-up band at the top of the Cockpit, "Depuis ton passage"
//! (`docs/cockpit.md` § Catch-up): what moved in each project since the
//! user last left the page, as `attn catchup --json` prints it.
//!
//! Read like the attention queue (`crate::attention`): cached, refreshed on a
//! background thread when the page opens and at most every `MAX_AGE_SECS`,
//! never on the tick. Leaving the page, or `Tout lu`, runs `attn seen`: the
//! next visit starts from then. Every text is escaped into the HTML here.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::Value;

/// How long a read stays fresh while the page is open.
pub const MAX_AGE_SECS: u64 = 60;
/// The project rows the band shows before `Voir plus`.
pub const ROWS: usize = 5;
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
const SEEN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// What moved since the last visit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catchup {
    /// Epoch seconds of the last visit; 0 when it cannot be read.
    pub since: u64,
    pub projects: Vec<Project>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Project {
    pub project: String,
    pub headline: String,
    /// The session that works on it, when one does.
    pub session: String,
    pub changes: Vec<Change>,
    pub waiting: Vec<Waiting>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    /// Epoch seconds; 0 when unknown.
    pub ts: u64,
    pub what: String,
    pub id: String,
    /// Its `action`, then its `src`, in order, once each.
    pub links: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Waiting {
    pub id: String,
    pub title: String,
    pub needs: String,
}

#[derive(Deserialize)]
struct RawCatchup {
    #[serde(default)]
    since: String,
    #[serde(default)]
    projects: Vec<Value>,
}

#[derive(Deserialize)]
struct RawProject {
    project: String,
    #[serde(default)]
    headline: Option<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    changes: Vec<Value>,
    #[serde(default)]
    waiting: Vec<Value>,
}

#[derive(Deserialize)]
struct RawChange {
    #[serde(default)]
    ts: String,
    #[serde(default)]
    what: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    src: Option<Value>,
}

#[derive(Deserialize)]
struct RawWaiting {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    needs: Option<String>,
}

/// Parse the CLI's answer. `None` when it is not the expected object; a
/// project, a change or a waiting item that does not read is skipped alone.
pub fn parse(text: &str) -> Option<Catchup> {
    let value: Value = serde_json::from_str(text).ok()?;
    if !value.is_object() {
        return None;
    }
    let raw: RawCatchup = serde_json::from_value(value).ok()?;
    let projects = raw
        .projects
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RawProject>(v).ok())
        .filter(|p| !p.project.trim().is_empty())
        .map(|p| Project {
            project: p.project,
            headline: p.headline.unwrap_or_default(),
            session: p.session.unwrap_or_default(),
            changes: p
                .changes
                .into_iter()
                .filter_map(|v| serde_json::from_value::<RawChange>(v).ok())
                .map(|c| {
                    let mut links: Vec<String> = Vec::new();
                    let src = match c.src {
                        Some(Value::String(s)) => vec![s],
                        Some(Value::Array(list)) => list.into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
                        _ => Vec::new(),
                    };
                    for l in c.action.into_iter().chain(src) {
                        let l = l.trim().to_string();
                        if !l.is_empty() && !links.contains(&l) {
                            links.push(l);
                        }
                    }
                    Change { ts: crate::attention::epoch_of(&c.ts).unwrap_or(0), what: c.what, id: c.id, links }
                })
                .collect(),
            waiting: p
                .waiting
                .into_iter()
                .filter_map(|v| serde_json::from_value::<RawWaiting>(v).ok())
                .filter(|w| !w.title.trim().is_empty())
                .map(|w| Waiting { id: w.id, title: w.title, needs: w.needs.unwrap_or_default() })
                .collect(),
        })
        .collect();
    Some(Catchup { since: crate::attention::epoch_of(&raw.since).unwrap_or(0), projects })
}

/// `11h40`, for epoch seconds shifted by the zone's offset.
pub fn clock(epoch: u64, offset_secs: i64) -> String {
    let local = (epoch as i64 + offset_secs).rem_euclid(86_400);
    format!("{:02}h{:02}", local / 3600, (local % 3600) / 60)
}

/// `clock` in this Mac's zone.
pub fn local_clock(epoch: u64) -> String {
    let t = epoch as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    clock(epoch, tm.tm_gmtoff)
}

/// `2026-11-10`, the day of `epoch` in this Mac's zone.
pub fn local_date(epoch: u64) -> String {
    let t = epoch as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

/// Whether leaving the page marks the band read: only when a read landed
/// during the visit, so a CLI that failed never hides what the user did not
/// see.
pub fn seen_on_leave(was_open: bool, read_landed: bool) -> bool {
    was_open && read_landed
}

// ---------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------

/// Everything one render of the band needs.
pub struct Band<'a> {
    pub data: &'a Catchup,
    pub folded: bool,
    /// The project rows opened in place.
    pub opened: &'a HashSet<String>,
    /// `Voir plus` was clicked: every row shows.
    pub all: bool,
    /// Whether `attention:<id>` is a card of the list.
    pub is_card: &'a dyn Fn(&str) -> bool,
    /// Whether a link opens in the side panel.
    pub opens: &'a dyn Fn(&str) -> bool,
    pub clock: &'a dyn Fn(u64) -> String,
}

fn esc(s: &str) -> String {
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

// Feather `rotate-ccw`, `terminal`, `chevron-up`, `chevron-down`.
const ICON_ROTATE: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>"#;
const ICON_TERM: &str = r#"<svg class="i" width="11" height="11" viewBox="0 0 24 24"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>"#;
const ICON_UP: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><polyline points="18 15 12 9 6 15"/></svg>"#;
const ICON_DOWN: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><polyline points="6 9 12 15 18 9"/></svg>"#;

/// A text of the band as HTML, its links clickable (`links::linkify_refs`).
fn linked(s: &str) -> String {
    crate::links::linkify_refs(s)
}

fn row_html(p: &Project, band: &Band) -> String {
    let open = band.opened.contains(&p.project);
    let waiting = p
        .waiting
        .first()
        .map(|w| format!("<div class=\"w\"><b>À toi :</b> {}</div>", linked(&w.title)))
        .unwrap_or_default();
    let session = if p.session.trim().is_empty() {
        "<span></span>".to_string()
    } else {
        format!("<span class=\"ses\">{ICON_TERM}{}</span>", esc(&p.session))
    };
    let det = if open { detail_html(p, band) } else { String::new() };
    format!(
        "<div class=\"ln{}\" data-proj=\"{}\"><div class=\"p\"><i></i><span>{}</span></div><div class=\"m\"><div>{}</div>{waiting}</div>{session}{det}</div>",
        if open { " open" } else { "" },
        esc(&p.project),
        esc(&p.project),
        linked(&p.headline),
    )
}

fn detail_html(p: &Project, band: &Band) -> String {
    let mut out = String::new();
    if !p.changes.is_empty() {
        let items: String = p
            .changes
            .iter()
            .map(|c| {
                let tm = if c.ts > 0 { format!("<span class=\"tm\">{}</span>", esc(&(band.clock)(c.ts))) } else { String::new() };
                let links: String = c
                    .links
                    .iter()
                    .filter(|l| (band.opens)(l))
                    .map(|l| {
                        format!(
                            "<button type=\"button\" class=\"link\" data-blink=\"{}\" title=\"{}\">{}</button>",
                            esc(l),
                            esc(l),
                            esc(&crate::links::label(l, 60))
                        )
                    })
                    .collect();
                let links = if links.is_empty() { links } else { format!("<span class=\"lks\">{links}</span>") };
                format!("<li>{tm}<span>{}</span>{links}</li>", linked(&c.what))
            })
            .collect();
        out.push_str(&format!("<div class=\"lbl\">Changements</div><ul class=\"chg\">{items}</ul>"));
    }
    if !p.waiting.is_empty() {
        let items: String = p
            .waiting
            .iter()
            .map(|w| {
                let needs = if w.needs.trim().is_empty() || w.needs.trim() == w.title.trim() {
                    String::new()
                } else {
                    format!("<span class=\"dim\"> \u{b7} {}</span>", linked(&w.needs))
                };
                // A waiting item that is a card: the card has the actions.
                let key = format!("attention:{}", w.id);
                let goto = if (band.is_card)(&key) {
                    format!("<button type=\"button\" class=\"link\" data-goto=\"{}\">Voir la carte</button>", esc(&key))
                } else {
                    String::new()
                };
                format!("<li><span>{}</span>{needs}{goto}</li>", linked(&w.title))
            })
            .collect();
        out.push_str(&format!("<div class=\"lbl\">À toi</div><ul class=\"chg\">{items}</ul>"));
    }
    format!("<div class=\"det\">{out}</div>")
}

/// The band, or nothing when no project moved.
pub fn band_html(band: &Band) -> String {
    let n = band.data.projects.len();
    if n == 0 {
        return String::new();
    }
    let count = if n == 1 { "1 projet a bougé".to_string() } else { format!("{n} projets ont bougé") };
    let since = if band.data.since > 0 { format!(" de {}", esc(&(band.clock)(band.data.since))) } else { String::new() };
    let (icon, tip) = if band.folded { (ICON_DOWN, "Déplier") } else { (ICON_UP, "Replier") };
    let mut out = format!(
        "<div class=\"cu\"><div class=\"cu-h\">{ICON_ROTATE}<span>Depuis ton passage{since}</span><span class=\"k\">{count}</span>\
         <span class=\"r\"><button type=\"button\" class=\"btn sec\" data-band=\"seen\"><span>Tout lu</span></button>\
         <button type=\"button\" class=\"ico\" data-band=\"fold\" title=\"{tip}\" aria-label=\"{tip}\">{icon}</button></span></div>"
    );
    if !band.folded {
        let shown = if band.all { n } else { n.min(ROWS) };
        for p in &band.data.projects[..shown] {
            out.push_str(&row_html(p, band));
        }
        if shown < n {
            out.push_str(&format!(
                "<div class=\"cu-more\"><button type=\"button\" class=\"link\" data-band=\"all\">Voir plus ({})</button></div>",
                n - shown
            ));
        }
    }
    out.push_str("</div>");
    out
}

/// Whether `link` is one the band shows for `project`: a click opens only
/// what the page was given.
pub fn shows_link(data: &Catchup, project: &str, link: &str) -> bool {
    data.projects.iter().any(|p| p.project == project && p.changes.iter().any(|c| c.links.iter().any(|l| l == link)))
}

// ---------------------------------------------------------------
// Reading and marking read
// ---------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commands {
    /// Prints the band as JSON (`attn catchup --json`).
    pub read: Vec<String>,
    /// Marks it read now (`attn seen`).
    pub seen: Vec<String>,
}

static COMMANDS: RwLock<Commands> = RwLock::new(Commands { read: Vec::new(), seen: Vec::new() });
static CACHE: RwLock<Option<Arc<Catchup>>> = RwLock::new(None);
static FILLED_AT: AtomicU64 = AtomicU64::new(0);
static REFRESHING: AtomicBool = AtomicBool::new(false);
/// Bumped by every `attn seen`: a read begun before it is stale.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// An `attn seen` runs: no read starts until it is done.
static SEEING: AtomicBool = AtomicBool::new(false);

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Adopt the configured commands; an empty `read` turns the band off.
pub fn set_commands(commands: &Commands) {
    if *COMMANDS.read() == *commands {
        return;
    }
    *COMMANDS.write() = commands.clone();
    *CACHE.write() = None;
    GENERATION.fetch_add(1, Ordering::Relaxed);
    FILLED_AT.store(0, Ordering::Relaxed);
}

/// The band as last read, and a read in the background once it aged out.
/// `None` while off, before the first read lands, or while `attn seen` runs.
pub fn snapshot() -> Option<Arc<Catchup>> {
    let argv = COMMANDS.read().read.clone();
    if argv.is_empty() {
        return None;
    }
    let age = now_secs().saturating_sub(FILLED_AT.load(Ordering::Relaxed));
    if age >= MAX_AGE_SECS && !SEEING.load(Ordering::Acquire) && !REFRESHING.swap(true, Ordering::AcqRel) {
        let generation = GENERATION.load(Ordering::Relaxed);
        std::thread::spawn(move || {
            let read = read(&argv);
            if GENERATION.load(Ordering::Relaxed) == generation {
                if let Some(data) = read {
                    *CACHE.write() = Some(Arc::new(data));
                }
                FILLED_AT.store(now_secs(), Ordering::Relaxed);
            }
            REFRESHING.store(false, Ordering::Release);
        });
    }
    CACHE.read().clone()
}

/// Read again on the next `snapshot`: the page opened.
pub fn refresh() {
    FILLED_AT.store(0, Ordering::Relaxed);
}

/// Run `attn seen`: the band empties now, and the next read starts from
/// then. `blocking` waits for it (the app is quitting).
pub fn mark_seen(blocking: bool) {
    let argv = COMMANDS.read().seen.clone();
    let Some((bin, args)) = argv.split_first().map(|(b, a)| (b.clone(), a.to_vec())) else { return };
    SEEING.store(true, Ordering::Release);
    GENERATION.fetch_add(1, Ordering::Relaxed);
    *CACHE.write() = None;
    let run = move || {
        match crate::exec::run_with_deadline(std::process::Command::new(&bin).args(&args), SEEN_DEADLINE) {
            Ok(out) if out.status.success() => {}
            Ok(out) => log::warn!("catchup: {bin} seen exited with {}", out.status),
            Err(e) => log::warn!("catchup: {e}"),
        }
        FILLED_AT.store(0, Ordering::Relaxed);
        SEEING.store(false, Ordering::Release);
    };
    if blocking {
        run();
    } else {
        std::thread::spawn(run);
    }
}

fn read(argv: &[String]) -> Option<Catchup> {
    let (bin, args) = argv.split_first()?;
    if !Path::new(bin).is_file() {
        log::warn!("catchup: {bin} is not a file");
        return None;
    }
    let out = match crate::exec::run_with_deadline(std::process::Command::new(bin).args(args), READ_DEADLINE) {
        Ok(out) => out,
        Err(e) => {
            log::warn!("catchup: {e}");
            return None;
        }
    };
    if !out.status.success() {
        log::warn!("catchup: {bin} exited with {}", out.status);
        return None;
    }
    let data = parse(&String::from_utf8_lossy(&out.stdout));
    if data.is_none() {
        log::warn!("catchup: cannot read the band");
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
 "since": "2026-11-06T11:40:00+01:00",
 "projects": [
  {"project": "integration-lemlist", "headline": "Kevin a validé le scope <V1>", "state": "x", "session": "claap-agent",
   "changes": [
    {"ts": "2026-11-06T12:18:47+01:00", "what": "fiche projet mise à jour", "id": "p.md", "src": ["/tmp/p.md", "https://slack.com/x"]},
    {"ts": "2026-11-06T12:09:34+01:00", "what": "Brouillon \"Kevin\"", "id": "a-77eb07", "src": null, "action": "https://linear.app/t/CLA-1"},
    {"ts": "2026-11-06T12:00:00+01:00", "what": "seul", "id": "a-1", "src": "/tmp/one.md", "action": "/tmp/one.md"}
   ],
   "waiting": [{"id": "a-77eb07", "title": "Valider la réponse à Kevin", "needs": "Valider le brouillon", "level": "point"},
               {"id": "a-999999", "title": "Relire la fiche", "needs": "", "level": "now"}],
   "last": "2026-11-06T12:18:47+01:00"},
  {"project": "voice-typing", "headline": "PR approuvée", "session": null, "changes": [], "waiting": []},
  {"headline": "no project field"},
  {"project": "c", "headline": "c"}, {"project": "d", "headline": "d"}, {"project": "e", "headline": "e"},
  {"project": "f", "headline": "f"}
 ]
}"#;

    fn band<'a>(data: &'a Catchup, opened: &'a HashSet<String>, all: bool, folded: bool) -> Band<'a> {
        Band { data, folded, opened, all, is_card: &|k| k == "attention:a-77eb07", opens: &|_| true, clock: &|t| clock(t, 3600) }
    }

    #[test]
    fn the_cli_answer_parses_with_null_string_or_list_sources() {
        let data = parse(SAMPLE).expect("parses");
        assert_eq!(data.since, crate::attention::epoch_of("2026-11-06T11:40:00+01:00").unwrap());
        assert_eq!(data.projects.len(), 6, "a project without a name is skipped alone");
        let p = &data.projects[0];
        assert_eq!(p.changes[0].links, ["/tmp/p.md", "https://slack.com/x"]);
        assert_eq!(p.changes[1].links, ["https://linear.app/t/CLA-1"], "a null src is no link");
        assert_eq!(p.changes[2].links, ["/tmp/one.md"], "action first, once");
        assert_eq!(data.projects[1].session, "");
        assert_eq!(parse("[]"), None);
        assert_eq!(parse("not json"), None);
    }

    #[test]
    fn the_clock_reads_hours_and_minutes_in_the_zone() {
        let t = crate::attention::epoch_of("2026-11-06T11:40:00+01:00").unwrap();
        assert_eq!(clock(t, 3600), "11h40");
        assert_eq!(clock(t, 0), "10h40");
        assert_eq!(clock(t, -11 * 3600), "23h40");
    }

    #[test]
    fn the_band_is_hidden_when_nothing_moved() {
        let none = HashSet::new();
        assert_eq!(band_html(&band(&Catchup::default(), &none, false, false)), "");
    }

    #[test]
    fn the_band_shows_five_rows_then_voir_plus() {
        let data = parse(SAMPLE).unwrap();
        let none = HashSet::new();
        let html = band_html(&band(&data, &none, false, false));
        assert!(html.contains("Depuis ton passage de 11h40") && html.contains("6 projets ont bougé"));
        assert!(html.contains("data-band=\"seen\"") && html.contains("Tout lu"));
        assert_eq!(html.matches("class=\"ln").count(), 5);
        assert!(html.contains("Voir plus (1)") && !html.contains("data-proj=\"f\""));
        let all = band_html(&band(&data, &none, true, false));
        assert_eq!(all.matches("class=\"ln").count(), 6);
        assert!(!all.contains("Voir plus"));
        // Folded: the header alone, with the chevron that unfolds.
        let folded = band_html(&band(&data, &none, false, true));
        assert_eq!(folded.matches("class=\"ln").count(), 0);
        assert!(folded.contains("Déplier"));
        let one = Catchup { since: 0, projects: vec![data.projects[1].clone()] };
        let html = band_html(&band(&one, &none, false, false));
        assert!(html.contains("1 projet a bougé") && html.contains("Depuis ton passage<"), "no time when since is unknown");
    }

    #[test]
    fn a_row_shows_its_first_waiting_item_and_session_escaped() {
        let data = parse(SAMPLE).unwrap();
        let none = HashSet::new();
        let html = band_html(&band(&data, &none, false, false));
        assert!(html.contains("Kevin a validé le scope &lt;V1&gt;"));
        assert!(html.contains("<b>À toi :</b> Valider la réponse à Kevin</div>"));
        assert!(!html.contains("Relire la fiche"), "only the first waiting item on the row");
        assert!(html.contains("claap-agent</span>"));
        // No waiting item: no line; no session: no terminal icon.
        let vt = html.split("data-proj=\"voice-typing\"").nth(1).unwrap().split("data-proj=").next().unwrap();
        assert!(!vt.contains("À toi") && !vt.contains("class=\"ses\""));
        assert!(!html.contains("class=\"det\""), "rows are closed until clicked");
    }

    #[test]
    fn an_opened_row_lists_changes_with_links_and_points_waiting_cards_at_the_list() {
        let data = parse(SAMPLE).unwrap();
        let opened: HashSet<String> = ["integration-lemlist".to_string()].into();
        let b = Band { opens: &|l: &str| !l.starts_with("/tmp/p"), ..band(&data, &opened, false, false) };
        let html = band_html(&b);
        assert!(html.contains("class=\"ln open\""));
        assert!(html.contains("<span class=\"tm\">12h18</span><span>fiche projet mise à jour</span>"));
        assert!(html.contains("Brouillon &quot;Kevin&quot;"));
        assert!(html.contains("data-blink=\"https://slack.com/x\"") && html.contains(">slack.com/x</button>"));
        assert!(html.contains("data-blink=\"/tmp/one.md\"") && html.contains(">one.md</button>"));
        assert!(!html.contains("data-blink=\"/tmp/p.md\""), "a link the panel cannot show is left out");
        // The waiting item that is a card points at it, without its actions;
        // the other one is plain text.
        assert!(html.contains("data-goto=\"attention:a-77eb07\""));
        assert!(!html.contains("attention:a-999999"));
        assert!(html.contains("Relire la fiche"));
        assert!(shows_link(&data, "integration-lemlist", "https://slack.com/x"));
        assert!(!shows_link(&data, "voice-typing", "https://slack.com/x"));
    }

    #[test]
    fn leaving_marks_read_only_after_a_read_landed() {
        assert!(seen_on_leave(true, true));
        assert!(!seen_on_leave(true, false), "a CLI that never answered hides nothing");
        assert!(!seen_on_leave(false, true), "a page that was not open is not left");
    }

    #[test]
    fn the_band_reads_the_configured_command_and_seen_empties_it() {
        let dir = std::env::temp_dir().join(format!("kova-catchup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("catchup.json");
        std::fs::write(&file, SAMPLE).unwrap();
        let argv: Vec<String> = ["/bin/sh", "-c", &format!("cat '{}'", file.display())].iter().map(|s| s.to_string()).collect();
        assert_eq!(read(&argv).map(|c| c.projects.len()), Some(6));
        assert_eq!(read(&["/nonexistent/attn".to_string()]), None);
        let failing: Vec<String> = ["/bin/sh", "-c", "exit 3"].iter().map(|s| s.to_string()).collect();
        assert_eq!(read(&failing), None);
        // `attn seen` runs with its words and the cache is gone.
        let mark = dir.join("seen");
        set_commands(&Commands {
            read: argv,
            seen: ["/bin/sh", "-c", &format!("touch '{}'", mark.display())].iter().map(|s| s.to_string()).collect(),
        });
        *CACHE.write() = Some(Arc::new(parse(SAMPLE).unwrap()));
        FILLED_AT.store(now_secs(), Ordering::Relaxed);
        mark_seen(true);
        assert!(mark.exists());
        assert!(CACHE.read().is_none());
        set_commands(&Commands::default());
        assert!(snapshot().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
