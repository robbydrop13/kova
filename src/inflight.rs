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
    /// The tasks, in the queue's order (by state, then the latest first):
    /// the open ones whatever their age, the done and dropped ones of the
    /// last 24 h. `None` when the queue does not give them (an older one,
    /// or its legacy reading): `En cours` then shows as before.
    pub tasks: Option<Vec<Task>>,
}

/// Where a task stands: the queue's five states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    /// `a_faire`: an agent has to act.
    ToDo,
    /// `attend_robin`: it waits on the user.
    WaitsUser,
    /// `attend_autre`: it waits on someone else.
    WaitsOther,
    /// `fait`: done, with its proof.
    Done,
    /// `abandonne`: dropped, with its reason.
    Dropped,
}

impl TaskState {
    /// The five, in the order `En cours` shows them.
    pub const ALL: [TaskState; 5] = [TaskState::ToDo, TaskState::WaitsUser, TaskState::WaitsOther, TaskState::Done, TaskState::Dropped];

    fn parse(word: &str) -> Option<TaskState> {
        match word {
            "a_faire" => Some(TaskState::ToDo),
            "attend_robin" => Some(TaskState::WaitsUser),
            "attend_autre" => Some(TaskState::WaitsOther),
            "fait" => Some(TaskState::Done),
            "abandonne" => Some(TaskState::Dropped),
            _ => None,
        }
    }

    /// The state in words, always written: nothing rests on a color.
    pub fn word(self) -> &'static str {
        match self {
            TaskState::ToDo => "À faire",
            TaskState::WaitsUser => "Attend Robin",
            TaskState::WaitsOther => "Attend un autre",
            TaskState::Done => "Fait",
            TaskState::Dropped => "Abandonné",
        }
    }

    /// Its section's key and dot class.
    fn key(self) -> (&'static str, &'static str) {
        match self {
            TaskState::ToDo => ("a_faire", "info"),
            TaskState::WaitsUser => ("attend_robin", "warn"),
            TaskState::WaitsOther => ("attend_autre", "gray"),
            TaskState::Done => ("fait", "ok"),
            TaskState::Dropped => ("abandonne", "gray"),
        }
    }

    /// Open: still to act on, shown unfolded whatever its age.
    pub fn open(self) -> bool {
        matches!(self, TaskState::ToDo | TaskState::WaitsUser | TaskState::WaitsOther)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub state: TaskState,
    pub scope: Scope,
    pub title: String,
    /// The session that has to act.
    pub owner: String,
    /// `a_faire`: the next action.
    pub next: String,
    /// `attend_*`: who and what.
    pub waiting_on: String,
    /// `fait`: the proof (a link, a message id, a commit).
    pub proof: String,
    /// `abandonne`: `dismissed`, `nogo`, `superseded`, `retiree`, `ttl`,
    /// `drop`, or the agent's own words; `why` says more.
    pub reason: String,
    pub why: String,
    /// The card is put away, the task waits for its proof.
    pub hidden: bool,
    /// Epoch seconds of its last change; 0 when unknown.
    pub updated: u64,
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
    /// Not 0 only once the action is proven (`attn close --proof`).
    pub done: u64,
    /// `preuve : X`, `échec : why`, `carte fermée sans preuve`, or other words.
    pub result: String,
    /// The click is a go: the agent acts on it, and owes a proof.
    pub go: bool,
    /// What became of it, in the queue's own word: `proof`, `superseded`,
    /// `failed`, `no_proof`, `done`, `dismissed`. Empty when the queue does
    /// not say (an older one): `fate` then reads `result`.
    pub outcome: String,
}

/// What a click's `result` says, in the queue's own words (`attn inflight`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fate<'a> {
    /// Nothing yet.
    Open,
    /// The action took place: its proof (a message id, a link, a commit, a file).
    Proof(&'a str),
    /// The action failed: why. Its card is back in the list.
    Failed(&'a str),
    /// The card was closed and nothing proves the action took place.
    NoProof,
    /// Nobody knows whether the message left (a send that timed out): what
    /// to check. Not over: the user settles it on the card.
    Uncertain(&'a str),
    /// The command is sending the message itself, no end yet.
    Sending,
    /// The action became pointless (the user did it himself, the subject
    /// went away): why. Over, but neither done nor proven.
    Superseded(&'a str),
    /// Other words (a card updated, a withdrawal passed on).
    Words(&'a str),
}

pub fn fate(c: &Click) -> Fate<'_> {
    let r = c.result.trim();
    let after = |prefix: &str| r.strip_prefix(prefix).map(|rest| rest.trim_start().trim_start_matches(':').trim());
    // The queue's `outcome` decides; `result` only gives the words.
    match c.outcome.as_str() {
        "proof" => return Fate::Proof(after("preuve").unwrap_or(r)),
        "superseded" => return Fate::Superseded(after("sans objet").unwrap_or(r)),
        "failed" => return Fate::Failed(after("échec").unwrap_or(r)),
        "no_proof" => return Fate::NoProof,
        "uncertain" => return Fate::Uncertain(after("peut-être parti").unwrap_or(r)),
        "sending" => return Fate::Sending,
        "done" | "dismissed" => return if r.is_empty() { Fate::Open } else { Fate::Words(r) },
        // Absent, or a word this build does not know: read the text.
        _ => {}
    }
    if r.is_empty() {
        Fate::Open
    } else if let Some(why) = after("sans objet") {
        Fate::Superseded(why)
    } else if let Some(proof) = after("preuve") {
        Fate::Proof(proof)
    } else if let Some(why) = after("échec") {
        Fate::Failed(why)
    } else if r == "carte fermée sans preuve" {
        Fate::NoProof
    } else {
        Fate::Words(r)
    }
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
    #[serde(default)]
    tasks: Option<Vec<Value>>,
}

#[derive(Deserialize)]
struct RawTask {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    next: Option<String>,
    #[serde(default)]
    waiting_on: Option<String>,
    #[serde(default)]
    proof: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    hidden: Option<bool>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
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
    #[serde(default)]
    go: Option<Value>,
    #[serde(default)]
    outcome: Option<String>,
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
            // `true`, or how the go was read (`"explicit"`, `"validated"`).
            go: matches!(&c.go, Some(Value::Bool(true))) || matches!(&c.go, Some(Value::String(s)) if !s.is_empty()),
            outcome: c.outcome.unwrap_or_default().trim().to_string(),
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
    // A task without a state this build knows, or without a title, is
    // skipped alone; every other field has a default.
    let tasks = raw.tasks.map(|list| {
        list.into_iter()
            .filter_map(|v| serde_json::from_value::<RawTask>(v).ok())
            .filter_map(|t| {
                let text = |s: Option<String>| s.unwrap_or_default().trim().to_string();
                let title = text(t.title);
                Some(Task {
                    state: TaskState::parse(t.state.as_deref()?)?,
                    id: text(t.id),
                    scope: if t.scope.as_deref() == Some("perso") { Scope::Perso } else { Scope::Pro },
                    owner: text(t.owner),
                    next: text(t.next),
                    waiting_on: text(t.waiting_on),
                    proof: text(t.proof),
                    reason: text(t.reason),
                    why: text(t.why),
                    hidden: t.hidden == Some(true),
                    updated: match epoch(&t.updated_at) {
                        0 => epoch(&t.created_at),
                        at => at,
                    },
                    title,
                })
                .filter(|t| !t.title.is_empty())
            })
            .collect()
    });
    Some(InFlight { clicks, agents, waiting_others: raw.waiting_others, tasks })
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
    // With tasks: the open ones, each once (a click's card is a task too).
    if let Some(tasks) = &data.tasks {
        return tasks.iter().filter(|t| t.state.open() && scope.is_none_or(|s| t.scope == s)).count();
    }
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
fn verb(button: &str, go: bool) -> (&'static str, &'static str) {
    match button.to_lowercase().as_str() {
        // An instruction that says to act is a go.
        "instruct" if go => ("Go", ""),
        "nogo" => ("No go", " c"),
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
    let third = match (st, fate(c)) {
        // Over without having been done: never `Fait`.
        (_, Fate::Superseded(_)) => "<span class=\"st\"><i></i>Sans objet</span>".to_string(),
        (Step::Done, _) => format!("<span class=\"st ok\"><i></i>Fait{}</span>", at(c.done)),
        // Not done: the action failed, or nothing proves it.
        (_, Fate::Failed(_)) => "<span class=\"st crit\"><i></i>Échec</span>".to_string(),
        (_, Fate::NoProof) => "<span class=\"st warn\"><i></i>Sans preuve</span>".to_string(),
        (_, Fate::Uncertain(_)) => "<span class=\"st warn\"><i></i>Peut-être parti</span>".to_string(),
        (_, Fate::Sending) => "<span class=\"st run\"><i></i>Envoi en cours</span>".to_string(),
        (Step::Received, _) => "<span class=\"st run\"><i></i>En cours</span>".to_string(),
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
    let (word, class) = verb(&c.button, c.go);
    let quote = if c.text.trim().is_empty() {
        String::new()
    } else {
        format!("<div class=\"q\">\u{ab} {} \u{bb}</div>", linked(&one_line(&c.text, 220)))
    };
    // The result, in En cours as in Fait: a proof is a link when it is one
    // (`links`), a failure is red, a close without proof amber.
    let res = match (fate(c), step(c, now)) {
        (Fate::Proof(proof), _) => format!("<span class=\"ok\"><b>Preuve :</b> {}</span>", linked(&one_line(proof, 200))),
        (Fate::Failed(why), _) => format!("<span class=\"crit\"><b>Échec :</b> {}</span>", linked(&one_line(why, 200))),
        (Fate::Superseded(why), _) => format!("<span class=\"dim\"><b>Sans objet :</b> {}</span>", linked(&one_line(why, 200))),
        (Fate::Uncertain(why), _) => format!("<span class=\"warn\"><b>Peut-être parti :</b> {}</span>", linked(&one_line(why, 220))),
        (Fate::NoProof, _) => "<span class=\"warn\"><b>Sans preuve :</b> la carte a été fermée, rien ne prouve que l'action a eu lieu</span>".to_string(),
        (Fate::Words(words), Step::Done) => format!("<span><b>Résultat :</b> {}</span>", linked(&one_line(words, 160))),
        (_, Step::Late) => "<span>Pas encore remis : il le sera à son prochain outil, ou réveillé s'il dort</span>".to_string(),
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

/// Why a task was dropped, in words: the queue's reason, then its `why`.
fn dropped_words(t: &Task) -> String {
    let reason = match t.reason.as_str() {
        "dismissed" => "pas pertinent",
        "nogo" => "no go",
        "superseded" => "sans objet",
        "retiree" => "retirée",
        "ttl" => "expirée",
        "drop" => "lâchée",
        other => other,
    };
    match (reason.is_empty(), t.why.is_empty()) {
        (true, true) => String::new(),
        (false, true) => reason.to_string(),
        (true, false) => t.why.clone(),
        (false, false) => format!("{reason} : {}", t.why),
    }
}

/// One task: its title, who has to act, its state in words and what the
/// state has to say (the next action, who it waits on, the proof, why it was
/// dropped).
fn task_html(t: &Task, now: u64) -> String {
    let detail = match t.state {
        TaskState::ToDo if !t.next.is_empty() => format!("<b>Prochaine action :</b> {}", linked(&one_line(&t.next, 220))),
        TaskState::WaitsUser | TaskState::WaitsOther if !t.waiting_on.is_empty() => linked(&one_line(&t.waiting_on, 220)),
        TaskState::Done if !t.proof.is_empty() => format!("<b>Preuve :</b> {}", linked(&one_line(&t.proof, 220))),
        TaskState::Done => "<b>Preuve :</b> aucune".to_string(),
        TaskState::Dropped => {
            let words = dropped_words(t);
            if words.is_empty() { String::new() } else { format!("<b>Raison :</b> {}", linked(&one_line(&words, 220))) }
        }
        _ => String::new(),
    };
    let put_away = if t.hidden { "<span class=\"dim\">carte rangée, attend sa preuve</span>" } else { "" };
    let owner = if t.owner.is_empty() { String::new() } else { format!("<span class=\"tag\"><i></i>{}</span>", esc(&t.owner)) };
    let age = crate::cockpit::age(t.updated, now);
    let age = if age.is_empty() { String::new() } else { format!("<span class=\"dim\">{}</span>", esc(&age)) };
    format!(
        "<div class=\"tk\"><div class=\"t\">{}</div><div class=\"res\">{owner}<span class=\"w {}\">{}</span>{}{put_away}{age}</div></div>",
        linked(&one_line(&t.title, 200)),
        t.state.key().0,
        t.state.word(),
        if detail.is_empty() { String::new() } else { format!("<span>{detail}</span>") },
    )
}

/// The five sections of the tasks, in the queue's order, each with its
/// state in words and its count; an empty one says so. The open ones are
/// unfolded whatever their age; done and dropped (the last 24 h) are there,
/// folded, a click away.
fn tasks_html(tasks: &[Task], scope: Option<Scope>, now: u64) -> String {
    let mut out = String::new();
    for state in TaskState::ALL {
        let rows: Vec<&Task> = tasks.iter().filter(|t| t.state == state && scope.is_none_or(|s| t.scope == s)).collect();
        let (key, dot) = state.key();
        let day = if state.open() { "" } else { " (24 h)" };
        let head = format!("<span class=\"d {dot}\"></span>{}{day} <span class=\"k\">{}</span>", state.word(), rows.len());
        let body: String = if rows.is_empty() {
            "<div class=\"fold\">Aucune</div>".to_string()
        } else {
            rows.iter().map(|t| task_html(t, now)).collect()
        };
        if state.open() {
            out.push_str(&format!("<div class=\"sec-h\">{head}</div><div class=\"box\">{body}</div>"));
        } else {
            out.push_str(&format!(
                "<details class=\"tks\" data-tk=\"{key}\"><summary class=\"sec-h\">{head}</summary><div class=\"box\">{body}</div></details>"
            ));
        }
    }
    out
}

/// The `En cours` list. With the queue's tasks: their five states, then the
/// user's clicks not over yet and the agents at work. Without them (an
/// older queue): the clicks and the agents, as before. `scope` keeps one
/// side's tasks and clicks (the agents have no side).
pub fn list_html(data: &InFlight, scope: Option<Scope>, panes: &[Pane], now: u64, clock: &dyn Fn(u64) -> String) -> String {
    let clicks = kept(data, scope, false, now);
    let (listed, idle) = agents(data, panes, now);
    if data.tasks.is_none() && clicks.is_empty() && listed.is_empty() {
        return "<p class=\"empty\">Rien en cours pour toi</p>".to_string();
    }
    let mut out = String::new();
    if let Some(tasks) = &data.tasks {
        out.push_str(&tasks_html(tasks, scope, now));
    }
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
    fn a_go_shows_its_proof_its_failure_or_that_nothing_proves_it() {
        let data = parse(
            r#"{"clicks":[
              {"act_id":"c-1","id":"a-1","scope":"pro","button":"Valider","title":"Send the plan","to":"pm","go":true,
               "sent":"2026-11-10T10:00:00+01:00","received":"2026-11-10T10:00:05+01:00","done":"2026-11-10T10:05:00+01:00","result":"preuve : https://x.slack.com/archives/C1/p17"},
              {"act_id":"c-2","id":"a-2","scope":"pro","button":"Valider","title":"Send the mail","to":"pm","go":true,
               "sent":"2026-11-10T10:00:00+01:00","received":"2026-11-10T10:00:05+01:00","done":null,"result":"échec : mailbox <full>"},
              {"act_id":"c-3","id":"a-3","scope":"pro","button":"instruct","title":"Post it","text":"envoie direct","to":"pm","go":"explicit",
               "sent":"2026-11-10T10:00:00+01:00","received":"2026-11-10T10:00:05+01:00","done":null,"result":"carte fermée sans preuve"},
              {"act_id":"c-4","id":"a-4","scope":"pro","button":"instruct","title":"Change it","text":"plus court","to":"pm","go":false,
               "sent":"2026-11-10T10:00:00+01:00","received":"2026-11-10T10:00:05+01:00","done":"2026-11-10T10:05:00+01:00","result":"carte mise à jour"}
            ]}"#,
        )
        .expect("inflight json");
        assert_eq!(data.clicks.iter().map(|c| c.go).collect::<Vec<_>>(), [true, true, true, false]);
        assert_eq!(fate(&data.clicks[0]), Fate::Proof("https://x.slack.com/archives/C1/p17"));
        assert_eq!(fate(&data.clicks[1]), Fate::Failed("mailbox <full>"));
        assert_eq!(fate(&data.clicks[2]), Fate::NoProof);
        assert_eq!(fate(&data.clicks[3]), Fate::Words("carte mise à jour"));
        let now = crate::attention::epoch_of("2026-11-10T10:06:00+01:00").unwrap();
        // Fait: only what is proven or over.
        let done = done_html(&data, None, now, &|_| String::new());
        assert!(done.contains(r#"<span class="ok"><b>Preuve :</b> <a href="https://x.slack.com/archives/C1/p17">https://x.slack.com/archives/C1/p17</a></span>"#), "{done}");
        assert!(done.contains("<b>Résultat :</b> carte mise à jour") && !done.contains("Échec") && !done.contains("Sans preuve"), "{done}");
        // En cours: the failure in red, the close without proof in amber, both said.
        let list = list_html(&data, None, &[], now, &|_| String::new());
        assert!(list.contains(r#"<span class="crit"><b>Échec :</b> mailbox &lt;full&gt;</span>"#) && list.contains(r#"<span class="st crit"><i></i>Échec</span>"#), "{list}");
        assert!(list.contains(r#"<span class="warn"><b>Sans preuve :</b>"#) && list.contains(r#"<span class="st warn"><i></i>Sans preuve</span>"#), "{list}");
        assert!(list.contains(r#"<span class="verb">Go</span>"#), "an instruction that is a go: {list}");
    }

    /// The six ends the queue names (`outcome`), real shapes of 10/11.
    #[test]
    fn the_queue_s_outcome_decides_and_a_superseded_click_is_neither_done_nor_proven() {
        let click = |n: u8, outcome: &str, done: bool, result: &str| {
            format!(
                r#"{{"act_id":"c-{n}","id":"a-{n}","scope":"pro","button":"Valider","title":"T{n}","to":"pm","go":true,"sent":"2026-11-10T10:00:00+01:00","received":"2026-11-10T10:00:05+01:00","done":{},"result":{},"outcome":"{outcome}"}}"#,
                if done { "\"2026-11-10T10:05:00+01:00\"" } else { "null" },
                serde_json::to_string(result).unwrap()
            )
        };
        let raw = format!(
            r#"{{"clicks":[{},{},{},{},{},{}]}}"#,
            click(1, "proof", true, "preuve : Slack DM https://x.slack.com/archives/D1/p17"),
            click(2, "superseded", true, "sans objet : Robin lui a répondu lui-même (https://x.slack.com/archives/D1/p18)"),
            click(3, "failed", false, "échec : canal archivé"),
            click(4, "no_proof", false, "carte fermée sans preuve"),
            click(5, "done", true, "carte mise à jour"),
            click(6, "dismissed", true, "retrait transmis"),
        );
        let data = parse(&raw).expect("inflight json");
        assert_eq!(fate(&data.clicks[0]), Fate::Proof("Slack DM https://x.slack.com/archives/D1/p17"));
        assert_eq!(fate(&data.clicks[1]), Fate::Superseded("Robin lui a répondu lui-même (https://x.slack.com/archives/D1/p18)"));
        assert_eq!(fate(&data.clicks[2]), Fate::Failed("canal archivé"));
        assert_eq!(fate(&data.clicks[3]), Fate::NoProof);
        assert_eq!((fate(&data.clicks[4]), fate(&data.clicks[5])), (Fate::Words("carte mise à jour"), Fate::Words("retrait transmis")));
        // A send by the command itself: on its way, then maybe gone.
        let mut code = data.clicks[3].clone();
        code.outcome = "uncertain".into();
        code.result = "peut-être parti : timeout ; vérification dans Slack, sinon C'est parti ou Renvoyer".into();
        assert_eq!(fate(&code), Fate::Uncertain("timeout ; vérification dans Slack, sinon C'est parti ou Renvoyer"));
        code.outcome = "sending".into();
        assert_eq!(fate(&code), Fate::Sending);
        // The field wins over the text; without it, the text is read as before.
        let mut odd = data.clicks[0].clone();
        odd.outcome = "superseded".into();
        assert!(matches!(fate(&odd), Fate::Superseded(_)), "the queue's word, whatever the text starts with");
        odd.outcome = String::new();
        odd.result = "sans objet : plus besoin".into();
        assert_eq!(fate(&odd), Fate::Superseded("plus besoin"), "an older queue: the text");
        let now = crate::attention::epoch_of("2026-11-10T10:06:00+01:00").unwrap();
        let done = done_html(&data, None, now, &|_| String::new());
        // Sans objet: in Fait, grey, its reason linked, never `Fait` nor `Preuve`.
        let row = done.split("<div class=\"clk").find(|r| r.contains("T2")).expect("the superseded click is in Fait");
        assert!(row.contains(r#"<span class="dim"><b>Sans objet :</b> Robin lui a répondu lui-même (<a href="https://x.slack.com/archives/D1/p18">"#), "{row}");
        assert!(row.contains(r#"<span class="st"><i></i>Sans objet</span>"#) && !row.contains("Fait") && !row.contains("Preuve"), "{row}");
        assert!(done.contains("<b>Preuve :</b> Slack DM <a href=") && done.contains("carte mise à jour") && done.contains("retrait transmis"));
        let list = list_html(&data, None, &[], now, &|_| String::new());
        assert!(list.contains("<b>Échec :</b> canal archivé") && list.contains("<b>Sans preuve :</b>") && !list.contains("Sans objet"), "{list}");
    }

    #[test]
    fn the_five_task_states_show_in_their_sections_with_their_word_and_count() {
        let task = |id: &str, state: &str, extra: &str| {
            format!(r#"{{"id":"{id}","state":"{state}","scope":"pro","title":"Task {id} <x>","owner":"pm","updated_at":"2026-11-11T06:00:00+01:00"{extra}}}"#)
        };
        let raw = format!(
            r#"{{"source":"tasks","clicks":[],"agents":[],"task_counts":{{}},"tasks":[{},{},{},{},{},{},{},{{"id":"t-x","state":"later","title":"unknown state"}},{{"id":"t-y","state":"fait"}}]}}"#,
            task("t-1", "a_faire", r#","next":"send the validated draft, see /tmp/plan.md","created_at":"2026-10-01T06:00:00+01:00""#),
            task("t-2", "a_faire", r#","next":null"#),
            task("t-3", "attend_robin", r#","waiting_on":"Robin : go or not on CLA-12","hidden":true"#),
            task("t-4", "fait", r#","proof":"https://x.slack.com/archives/D1/p17""#),
            task("t-5", "fait", ""),
            task("t-6", "abandonne", r#","reason":"superseded","why":"Robin answered himself""#),
            task("t-7", "abandonne", r#","reason":"Doublon de k-1","scope":"perso""#),
        );
        let data = parse(&raw).expect("inflight json");
        let tasks = data.tasks.as_ref().expect("tasks");
        assert_eq!(tasks.len(), 7, "an unknown state and a task without a title are skipped alone");
        let now = crate::attention::epoch_of("2026-11-11T07:00:00+01:00").unwrap();
        let html = list_html(&data, None, &[], now, &|_| String::new());
        // Five sections in order, each with its word and its count, the empty one too.
        let heads: Vec<&str> = ["À faire <span class=\"k\">2</span>", "Attend Robin <span class=\"k\">1</span>", "Attend un autre <span class=\"k\">0</span>", "Fait (24 h) <span class=\"k\">2</span>", "Abandonné (24 h) <span class=\"k\">2</span>"].to_vec();
        let mut at = 0;
        for h in &heads {
            at += html[at..].find(h).unwrap_or_else(|| panic!("{h} missing or out of order: {html}"));
        }
        assert!(html.contains("<div class=\"fold\">Aucune</div>"), "{html}");
        // The open ones unfolded, done and dropped folded but there.
        assert!(html.contains("<details class=\"tks\" data-tk=\"fait\">") && html.contains("<details class=\"tks\" data-tk=\"abandonne\">"));
        assert!(!html.contains("data-tk=\"a_faire\""), "open states are not folded");
        // A row: title escaped, owner, the state in words, what the state says.
        assert!(html.contains("Task t-1 &lt;x&gt;") && html.contains("<span class=\"w a_faire\">À faire</span>"), "{html}");
        assert!(html.contains(r##"<b>Prochaine action :</b> send the validated draft, see <a href="#" class="path" data-path="/tmp/plan.md">"##), "{html}");
        assert!(html.contains("<span class=\"w attend_robin\">Attend Robin</span><span>Robin : go or not on <a href=\"https://linear.app/claap/issue/CLA-12\">CLA-12</a></span><span class=\"dim\">carte rangée, attend sa preuve</span>"), "{html}");
        assert!(html.contains(r#"<b>Preuve :</b> <a href="https://x.slack.com/archives/D1/p17">"#) && html.contains("<b>Preuve :</b> aucune"), "{html}");
        assert!(html.contains("<b>Raison :</b> sans objet : Robin answered himself") && html.contains("<b>Raison :</b> Doublon de k-1"), "{html}");
        // No age limit on what is to do: the queue decides, Kova filters nothing.
        assert_eq!(count(&data, None, &[], now), 3, "the open tasks");
        // One side only.
        let pro = list_html(&data, Some(Scope::Pro), &[], now, &|_| String::new());
        assert!(pro.contains("Abandonné (24 h) <span class=\"k\">1</span>") && !pro.contains("Doublon de k-1"), "{pro}");
        // Without `tasks` (an older queue, or its legacy reading): as before.
        let legacy = parse(r#"{"clicks":[],"agents":[]}"#).unwrap();
        assert!(legacy.tasks.is_none());
        assert!(list_html(&legacy, None, &[], now, &|_| String::new()).contains("Rien en cours pour toi"));
    }

    #[test]
    fn nothing_in_flight_says_so() {
        let empty = InFlight::default();
        assert!(list_html(&empty, None, &[], 0, &|_| String::new()).contains("Rien en cours pour toi"));
    }
}
