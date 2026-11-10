//! The Cockpit's `En cours` filter (`docs/cockpit.md` § Filtres): what the
//! agents do for the user, as `attn inflight --json` prints it. Every click
//! the user gave (a go, a correction, a consigne, a removal) followed through
//! `Envoyé`, `Reçu`, `Fait`, and the sessions at work with their open
//! consignes.
//!
//! Read like the catch-up band (`crate::catchup`): cached, refreshed on a
//! background thread at most every `MAX_AGE_SECS` while the page shows it,
//! never on the tick. Every text is escaped into the HTML here.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::Value;

use crate::attention::Scope;

/// How long a read stays fresh while the filter is on.
pub const MAX_AGE_SECS: u64 = 15;
/// A click received that long ago and still not done is not late; one not
/// received after that is.
pub const LATE_SECS: u64 = 180;
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InFlight {
    pub clicks: Vec<Click>,
    pub agents: Vec<Agent>,
    /// Sessions that wait on someone else (a review, a merge), not listed.
    pub waiting_others: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Click {
    pub id: String,
    pub scope: Scope,
    /// The button as the queue records it: `Valider`, `Corriger`,
    /// `instruct`, `dismissed`.
    pub button: String,
    pub title: String,
    pub text: String,
    /// The session the click goes to.
    pub to: String,
    /// Epoch seconds; 0 when not yet.
    pub sent: u64,
    pub received: u64,
    pub done: u64,
    pub result: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
    pub state: String,
    pub doing: String,
    /// When the session last wrote its status, epoch seconds.
    pub since: u64,
    pub consignes: Vec<String>,
}

#[derive(Deserialize)]
struct RawInFlight {
    #[serde(default)]
    clicks: Vec<Value>,
    #[serde(default)]
    agents: Vec<Value>,
    #[serde(default)]
    waiting_others: usize,
}

#[derive(Deserialize)]
struct RawClick {
    #[serde(default)]
    id: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    button: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    sent: Option<String>,
    #[serde(default)]
    received: Option<String>,
    #[serde(default)]
    done: Option<String>,
    #[serde(default)]
    result: Option<String>,
}

#[derive(Deserialize)]
struct RawAgent {
    name: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    doing: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    consignes: Vec<Value>,
}

fn epoch(ts: &Option<String>) -> u64 {
    ts.as_deref().and_then(crate::attention::epoch_of).unwrap_or(0)
}

/// Parse the CLI's answer. `None` when it is not the expected object; a click
/// or an agent that does not read is skipped alone.
pub fn parse(text: &str) -> Option<InFlight> {
    let value: Value = serde_json::from_str(text).ok()?;
    if !value.is_object() {
        return None;
    }
    let raw: RawInFlight = serde_json::from_value(value).ok()?;
    let clicks = raw
        .clicks
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RawClick>(v).ok())
        .filter(|c| !c.button.trim().is_empty())
        .map(|c| Click {
            sent: epoch(&c.sent),
            received: epoch(&c.received),
            done: epoch(&c.done),
            id: c.id,
            scope: if c.scope.as_deref() == Some("perso") { Scope::Perso } else { Scope::Pro },
            button: c.button,
            title: c.title.unwrap_or_default(),
            text: c.text.unwrap_or_default(),
            to: c.to.unwrap_or_default(),
            result: c.result.unwrap_or_default(),
        })
        .collect();
    let agents = raw
        .agents
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RawAgent>(v).ok())
        .filter(|a| !a.name.trim().is_empty())
        .map(|a| Agent {
            name: a.name,
            state: a.state.unwrap_or_default(),
            doing: a.doing.unwrap_or_default(),
            since: epoch(&a.since),
            consignes: a
                .consignes
                .into_iter()
                .filter_map(|c| c.get("text").and_then(Value::as_str).map(str::to_string))
                .filter(|t| !t.trim().is_empty())
                .collect(),
        })
        .collect();
    Some(InFlight { clicks, agents, waiting_others: raw.waiting_others })
}

// ---------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------

/// Where a click stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Sent,
    /// Sent `LATE_SECS` ago or more and still not received.
    Late,
    Received,
    Done,
}

pub fn step(c: &Click, now: u64) -> Step {
    if c.done > 0 {
        Step::Done
    } else if c.received > 0 {
        Step::Received
    } else if c.sent > 0 && now.saturating_sub(c.sent) >= LATE_SECS {
        Step::Late
    } else {
        Step::Sent
    }
}

/// A click that is over: done, or a card the user withdrew (`Retiré`),
/// which never shows as in flight (Robin, 10/11/2026).
pub fn finished(c: &Click, now: u64) -> bool {
    step(c, now) == Step::Done || c.button.eq_ignore_ascii_case("dismissed")
}

/// The clicks one view keeps: `done` the finished ones, else the others.
fn kept<'a>(data: &'a InFlight, scope: Option<Scope>, done: bool, now: u64) -> Vec<&'a Click> {
    data.clicks.iter().filter(|c| scope.is_none_or(|s| c.scope == s) && finished(c, now) == done).collect()
}

/// What `En cours` counts: the clicks not over yet and the agents at work.
pub fn count(data: &InFlight, scope: Option<Scope>, panes: &[Pane], now: u64) -> usize {
    kept(data, scope, false, now).len() + agents(data, panes, now).0.len()
}

/// What `Fait (24 h)` counts.
pub fn done_count(data: &InFlight, scope: Option<Scope>, now: u64) -> usize {
    kept(data, scope, true, now).len()
}

/// A session a Kova pane holds: its name, and whether the pane works now.
pub type Pane = (String, bool);

/// A `working` status a session wrote longer ago than this, with no pane in
/// Kova to vouch for it, is stale.
pub const STATUS_FRESH_SECS: u64 = 1800;

/// A session `En cours` lists.
#[derive(Debug, PartialEq, Eq)]
pub struct Live<'a> {
    pub name: &'a str,
    pub doing: &'a str,
    pub consignes: &'a [String],
}

/// The sessions at work now, and the open consignes held by the others.
/// Liveness is read, not reported: a session in a Kova pane is at work when
/// its pane works, whatever its last status says (Robin, 10/11/2026). A
/// session with no pane counts on a fresh `working` status. Its status text
/// shows only while that status says `working`; holding a consigne alone
/// never lists a session.
fn agents<'a>(data: &'a InFlight, panes: &'a [Pane], now: u64) -> (Vec<Live<'a>>, usize) {
    let status = |n: &str| data.agents.iter().find(|a| a.name == n);
    let mut out: Vec<Live> = Vec::new();
    for (name, working) in panes {
        if *working && !out.iter().any(|l| l.name == name) {
            let a = status(name);
            out.push(Live {
                name,
                doing: a.filter(|a| a.state == "working").map(|a| a.doing.as_str()).unwrap_or(""),
                consignes: a.map(|a| a.consignes.as_slice()).unwrap_or(&[]),
            });
        }
    }
    for a in &data.agents {
        let in_pane = panes.iter().any(|(n, _)| *n == a.name);
        if !in_pane && a.state == "working" && now.saturating_sub(a.since) < STATUS_FRESH_SECS {
            out.push(Live { name: &a.name, doing: &a.doing, consignes: &a.consignes });
        }
    }
    out.sort_by(|x, y| x.name.cmp(y.name));
    let idle = data.agents.iter().filter(|a| !out.iter().any(|l| l.name == a.name)).map(|a| a.consignes.len()).sum();
    (out, idle)
}

/// What the page calls a button.
fn verb(button: &str) -> (&'static str, &'static str) {
    match button.to_lowercase().as_str() {
        "valider" | "envoyer" | "send" => ("Valider", ""),
        "corriger" => ("Corriger", ""),
        "instruct" => ("Consigne", " c"),
        "dismissed" => ("Retiré", " c"),
        "done" => ("Fait", " c"),
        _ => ("Clic", " c"),
    }
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

/// A text of the list as HTML, its links clickable (`links::linkify_refs`).
fn linked(s: &str) -> String {
    crate::links::linkify_refs(s)
}

fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max).collect();
    cut.push('\u{2026}');
    cut
}

fn tracker(c: &Click, now: u64, clock: &dyn Fn(u64) -> String) -> String {
    let at = |t: u64| if t > 0 { format!(" <em>{}</em>", esc(&clock(t))) } else { String::new() };
    let st = step(c, now);
    let second = match st {
        Step::Late => {
            let mins = now.saturating_sub(c.sent) / 60;
            format!("<span class=\"st warn\"><i></i>Pas encore reçu <em>{mins} min</em></span>")
        }
        Step::Sent => "<span class=\"st\"><i></i>Reçu</span>".to_string(),
        _ => format!("<span class=\"st ok\"><i></i>Reçu{}</span>", at(c.received)),
    };
    let third = match st {
        Step::Done => format!("<span class=\"st ok\"><i></i>Fait{}</span>", at(c.done)),
        Step::Received => "<span class=\"st run\"><i></i>En cours</span>".to_string(),
        _ => "<span class=\"st\"><i></i>Fait</span>".to_string(),
    };
    let line = |ok: bool| format!("<span class=\"sep{}\"></span>", if ok { " ok" } else { "" });
    format!(
        "<div class=\"trk\"><span class=\"st ok\"><i></i>Envoyé{}</span>{}{second}{}{third}</div>",
        at(c.sent),
        line(c.received > 0),
        line(c.done > 0)
    )
}

fn click_html(c: &Click, now: u64, clock: &dyn Fn(u64) -> String) -> String {
    let (word, class) = verb(&c.button);
    let quote = if c.text.trim().is_empty() {
        String::new()
    } else {
        format!("<div class=\"q\">\u{ab} {} \u{bb}</div>", linked(&one_line(&c.text, 220)))
    };
    let res = match step(c, now) {
        Step::Done if !c.result.trim().is_empty() => format!("<span><b>Résultat :</b> {}</span>", linked(&one_line(&c.result, 160))),
        Step::Late => "<span>Pas encore remis : il le sera à son prochain outil, ou réveillé s'il dort</span>".to_string(),
        _ => String::new(),
    };
    format!(
        "<div class=\"clk{}\" data-act-card=\"{}\"><div class=\"row\"><div class=\"txt\"><div class=\"t\"><span class=\"verb{class}\">{word}</span>{}</div>{quote}\
         <div class=\"res\"><span class=\"tag\"><i></i>{}</span>{res}</div></div>{}</div></div>",
        if step(c, now) == Step::Late { " warn" } else { "" },
        esc(&c.id),
        linked(&c.title),
        esc(&c.to),
        tracker(c, now, clock),
    )
}

fn agent_html(a: &Live) -> String {
    let consignes = match a.consignes.len() {
        0 => String::new(),
        n => format!(
            "<div class=\"k\"><b>{}</b> {}</div>",
            if n == 1 { "1 consigne :".to_string() } else { format!("{n} consignes, dont :") },
            linked(&one_line(&a.consignes[0], 160))
        ),
    };
    let doing = if a.doing.trim().is_empty() { String::new() } else { format!("<div>{}</div>", linked(&one_line(a.doing, 160))) };
    format!(
        "<div class=\"ag\"><div class=\"p\"><i style=\"background:var(--info-dot)\"></i><span>{}</span></div><div>{doing}{consignes}</div><span class=\"m\">au travail</span></div>",
        esc(a.name)
    )
}

/// The `Fait (24 h)` list: the clicks over, the latest first.
pub fn done_html(data: &InFlight, scope: Option<Scope>, now: u64, clock: &dyn Fn(u64) -> String) -> String {
    let clicks = kept(data, scope, true, now);
    if clicks.is_empty() {
        return "<p class=\"empty\">Rien de fait ces dernières 24 h</p>".to_string();
    }
    let mut out = format!("<div class=\"sec-h\"><span class=\"d ok\"></span>Fait <span class=\"k\">{}</span></div>", clicks.len());
    for c in clicks {
        out.push_str(&click_html(c, now, clock));
    }
    out
}

/// The `En cours` list: the user's clicks not over yet, then the agents at
/// work. `scope` keeps one side's clicks (the agents have no side). What is
/// over leaves it at once for `Fait (24 h)`.
pub fn list_html(data: &InFlight, scope: Option<Scope>, panes: &[Pane], now: u64, clock: &dyn Fn(u64) -> String) -> String {
    let clicks = kept(data, scope, false, now);
    let (listed, idle) = agents(data, panes, now);
    if clicks.is_empty() && listed.is_empty() {
        return "<p class=\"empty\">Rien en cours pour toi</p>".to_string();
    }
    let mut out = String::new();
    if !clicks.is_empty() {
        out.push_str(&format!(
            "<div class=\"sec-h\"><span class=\"d info\"></span>Tes clics <span class=\"k\">{}</span></div>",
            clicks.len()
        ));
        for c in clicks {
            out.push_str(&click_html(c, now, clock));
        }
    }
    if !listed.is_empty() {
        out.push_str(&format!(
            "<div class=\"sec-h\"><span class=\"d info\"></span>Agents au travail pour toi <span class=\"k\">{}</span></div><div class=\"box\">",
            listed.len()
        ));
        for a in &listed {
            out.push_str(&agent_html(a));
        }
        if idle > 0 {
            out.push_str(&format!(
                "<div class=\"fold\">{idle} consigne{} chez des sessions au repos</div>",
                if idle > 1 { "s" } else { "" }
            ));
        }
        out.push_str("</div>");
    }
    out
}

// ---------------------------------------------------------------
// Reading
// ---------------------------------------------------------------

static COMMAND: RwLock<Vec<String>> = RwLock::new(Vec::new());
static CACHE: RwLock<Option<Arc<InFlight>>> = RwLock::new(None);
static FILLED_AT: AtomicU64 = AtomicU64::new(0);
static REFRESHING: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Adopt the configured command; an empty one turns the filter's list off.
pub fn set_command(argv: &[String]) {
    if *COMMAND.read() == argv {
        return;
    }
    *COMMAND.write() = argv.to_vec();
    *CACHE.write() = None;
    GENERATION.fetch_add(1, Ordering::Relaxed);
    FILLED_AT.store(0, Ordering::Relaxed);
}

/// The list as last read, and a read in the background once it aged out.
/// `None` while off or before the first read lands.
pub fn snapshot() -> Option<Arc<InFlight>> {
    let argv = COMMAND.read().clone();
    if argv.is_empty() {
        return None;
    }
    let age = now_secs().saturating_sub(FILLED_AT.load(Ordering::Relaxed));
    if age >= MAX_AGE_SECS && !REFRESHING.swap(true, Ordering::AcqRel) {
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

/// Read again on the next `snapshot`: the filter was switched on.
pub fn refresh() {
    FILLED_AT.store(0, Ordering::Relaxed);
}

fn read(argv: &[String]) -> Option<InFlight> {
    let (bin, args) = argv.split_first()?;
    if !Path::new(bin).is_file() {
        log::warn!("inflight: {bin} is not a file");
        return None;
    }
    let out = match crate::exec::run_with_deadline(std::process::Command::new(bin).args(args), READ_DEADLINE) {
        Ok(out) => out,
        Err(e) => {
            log::warn!("inflight: {e}");
            return None;
        }
    };
    if !out.status.success() {
        log::warn!("inflight: {bin} exited with {}", out.status);
        return None;
    }
    let data = parse(&String::from_utf8_lossy(&out.stdout));
    if data.is_none() {
        log::warn!("inflight: cannot read the list");
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
 "clicks": [
  {"act_id": "c-1", "id": "a-1", "scope": "pro", "button": "instruct", "title": "Réponse à <Alex>",
   "text": "il a répondu depuis,\n mets à jour", "to": "claap-agent",
   "sent": "2026-11-10T14:46:35+01:00", "received": "2026-11-10T14:47:00+01:00", "done": "2026-11-10T14:49:54+01:00",
   "result": "carte mise à jour"},
  {"act_id": "c-2", "id": "a-2", "scope": "pro", "button": "Valider", "title": "Mail à Sam", "text": null,
   "to": "claap-agent", "sent": "2026-11-10T14:48:01+01:00", "received": "2026-11-10T14:48:02+01:00", "done": null},
  {"act_id": "c-3", "id": "a-3", "scope": "perso", "button": "instruct", "title": "Montre", "text": "teste",
   "to": "watch-app", "sent": "2026-11-10T15:02:00+01:00", "received": null, "done": null},
  {"act_id": "c-4", "title": "no button"}
 ],
 "agents": [
  {"name": "builder", "state": "done", "doing": "x", "consignes": [{"id": "k-1", "text": "Un filtre de plus"}]},
  {"name": "claap-agent", "state": null, "doing": null, "consignes": []},
  {"name": "idle", "state": "awaiting-you", "doing": "rien", "consignes": []}
 ],
 "waiting_others": 11
}"#;

    fn at(ts: &str) -> u64 {
        crate::attention::epoch_of(ts).unwrap()
    }

    #[test]
    fn the_cli_answer_parses_and_a_click_without_button_is_skipped() {
        let data = parse(SAMPLE).expect("parses");
        assert_eq!(data.clicks.len(), 3);
        assert_eq!(data.clicks[2].scope, Scope::Perso);
        assert_eq!(data.clicks[1].done, 0);
        assert_eq!(data.agents[0].consignes, ["Un filtre de plus"]);
        assert_eq!(data.waiting_others, 11);
        assert_eq!(parse("[]"), None);
    }

    #[test]
    fn a_click_steps_from_sent_to_done_and_turns_late_when_not_received() {
        let data = parse(SAMPLE).unwrap();
        let now = at("2026-11-10T15:10:00+01:00");
        assert_eq!(step(&data.clicks[0], now), Step::Done);
        assert_eq!(step(&data.clicks[1], now), Step::Received);
        assert_eq!(step(&data.clicks[2], now), Step::Late);
        assert_eq!(step(&data.clicks[2], at("2026-11-10T15:02:30+01:00")), Step::Sent);
    }

    #[test]
    fn the_list_shows_the_tracker_escaped_and_the_scope_filter_keeps_one_side() {
        let data = parse(SAMPLE).unwrap();
        let now = at("2026-11-10T15:10:00+01:00");
        let clock = |t: u64| crate::catchup::clock(t, 3600);
        // Both views together: the done click sits in `Fait (24 h)`.
        let html = list_html(&data, None, &[], now, &clock) + &done_html(&data, None, now, &clock);
        assert!(html.contains("Réponse à &lt;Alex&gt;"), "{html}");
        assert!(html.contains("\u{ab} il a répondu depuis, mets à jour \u{bb}"), "{html}");
        assert!(html.contains("<span class=\"st ok\"><i></i>Fait <em>14h49</em></span>"), "{html}");
        assert!(html.contains("<b>Résultat :</b> carte mise à jour"));
        assert!(html.contains("<span class=\"st run\"><i></i>En cours</span>"));
        assert!(html.contains("class=\"clk warn\"") && html.contains("Pas encore reçu <em>8 min</em>"), "{html}");
        let pro = list_html(&data, Some(Scope::Pro), &[], now, &clock);
        assert!(!pro.contains("Montre") && pro.contains("Mail à Sam"));
    }

    #[test]
    fn the_agents_at_work_are_the_working_panes_never_a_stale_status() {
        let mut data = parse(SAMPLE).unwrap();
        let now = at("2026-11-10T15:10:00+01:00");
        let clock = |t: u64| crate::catchup::clock(t, 3600);
        data.agents.push(Agent { name: "headless".into(), state: "working".into(), doing: "nuit".into(), since: now - 60, consignes: vec![] });
        data.agents.push(Agent { name: "old".into(), state: "working".into(), doing: "vieux".into(), since: now - 7200, consignes: vec![] });
        // builder's pane works: listed, its stale `done` text hidden, its consigne kept;
        // night-qa is only a pane: listed; idle's pane rests: not listed, whatever it wrote.
        let panes = vec![("builder".to_string(), true), ("night-qa".to_string(), true), ("idle".to_string(), false)];
        let html = list_html(&data, None, &panes, now, &clock);
        assert!(html.contains("<span>builder</span></div><div><div class=\"k\"><b>1 consigne :</b> Un filtre de plus</div></div>"), "{html}");
        assert!(html.contains("<span>night-qa</span>") && html.contains("<span>headless</span>") && html.contains("nuit"));
        assert!(!html.contains("<span>idle</span>") && !html.contains("<span>old</span>") && !html.contains("vieux"), "{html}");
        assert!(html.contains("Agents au travail pour toi <span class=\"k\">3</span>"));
        // No pane works: builder's consigne alone lists nobody, it goes to the fold.
        let rest = list_html(&data, None, &[("builder".to_string(), false)], now, &clock);
        assert!(!rest.contains("<span>builder</span>") && rest.contains("1 consigne chez des sessions au repos") == rest.contains("Agents au travail"), "{rest}");
        assert_eq!(count(&data, None, &panes, now), 2 + 3);
    }

    #[test]
    fn en_cours_keeps_only_what_moves_fait_keeps_the_rest() {
        let mut data = parse(SAMPLE).unwrap();
        let now = at("2026-11-10T15:10:00+01:00");
        let clock = |t: u64| crate::catchup::clock(t, 3600);
        let mut withdrawn = data.clicks[1].clone();
        withdrawn.id = "c-x".into();
        withdrawn.button = "dismissed".into();
        withdrawn.title = "Carte retirée".into();
        data.clicks.push(withdrawn);
        let live = list_html(&data, None, &[], now, &clock);
        assert!(!live.contains("Alex"), "a done click leaves En cours: {live}");
        assert!(!live.contains("Carte retirée"), "Retiré never shows in En cours");
        assert!(live.contains("Tes clics <span class=\"k\">2</span>"), "{live}");
        let done = done_html(&data, None, now, &clock);
        assert!(done.contains("Alex") && done.contains("Carte retirée"), "{done}");
        assert!(!done.contains("Agents au travail"));
        assert_eq!((count(&data, None, &[], now), done_count(&data, None, now)), (2, 2));
    }

    #[test]
    fn a_click_s_title_instruction_and_result_and_an_agent_s_lines_carry_their_links() {
        let data = parse(
            r#"{"clicks":[{"act_id":"c-1","id":"a-1","scope":"pro","button":"instruct","title":"Read /tmp/plan.md for CLA-12","from":"pm",
                "text":"see https://x.io/a and #12345","to":"pm","sent":"2026-11-10T10:00:00+01:00","received":"2026-11-10T10:00:05+01:00",
                "done":"2026-11-10T10:05:00+01:00","result":"written to ~/out/report.html"}],
               "agents":[{"name":"pm","state":"working","doing":"plan in /tmp/plan.md","since":"2026-11-10T10:00:00+01:00",
                "consignes":[{"id":"k-1","text":"ticket CLA-7 <first>","ts":"2026-11-10T10:00:00+01:00"}]}]}"#,
        )
        .expect("inflight json");
        let now = crate::attention::epoch_of("2026-11-10T10:06:00+01:00").unwrap();
        let done = done_html(&data, None, now, &|_| String::new());
        assert!(done.contains(r##"Read <a href="#" class="path" data-path="/tmp/plan.md">/tmp/plan.md</a> for <a href="https://linear.app/claap/issue/CLA-12">CLA-12</a>"##), "{done}");
        assert!(done.contains(r#"see <a href="https://x.io/a">https://x.io/a</a> and <a href="https://github.com/claap-app/claap/pull/12345">#12345</a>"#), "{done}");
        assert!(done.contains(r##"written to <a href="#" class="path" data-path="~/out/report.html">~/out/report.html</a>"##), "{done}");
        let list = list_html(&data, None, &[("pm".to_string(), true)], now, &|_| String::new());
        assert!(list.contains(r##"plan in <a href="#" class="path" data-path="/tmp/plan.md">"##), "{list}");
        assert!(list.contains(r#"ticket <a href="https://linear.app/claap/issue/CLA-7">CLA-7</a> &lt;first&gt;"#), "still escaped: {list}");
    }

    #[test]
    fn nothing_in_flight_says_so() {
        let empty = InFlight::default();
        assert!(list_html(&empty, None, &[], 0, &|_| String::new()).contains("Rien en cours pour toi"));
    }
}
