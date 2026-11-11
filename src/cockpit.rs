//! The Cockpit: every action that waits on the user, on one page laid over
//! the panes (`docs/cockpit.md`). The attention queue's `now` and `point`
//! entries (`crate::attention`) and the live needs Kova already knows
//! (`window::needs`: permission prompts, open questions and decision sheets,
//! turn errors, failed routines), sorted by priority then age.
//!
//! Pure, like `decision_sheet`: the host (`window/cockpit_view.rs`) gathers
//! the items, this module orders, filters and renders them. Every text is
//! escaped into the HTML here; the page's script only posts clicks and the
//! text of an edited draft back (`kovaCockpit` messages), it never builds a
//! text node from data. Kova never sends anything: a draft's `Valider` or
//! `Corriger` goes to `act_command`, and whoever receives the click sends.

use serde::Serialize;
use std::path::Path;

use crate::attention::{Draft, Entry, Instructed, Level, Outcome, Scope};
use crate::window::needs::{NeedItem, NeedKind};

/// What the page's script posts under.
pub const HANDLER: &str = "kovaCockpit";
/// The chief of staff a scope's chat talks to: the pane whose agent session
/// (or peer name) is this one.
pub fn chief_session(scope: Scope) -> &'static str {
    match scope {
        Scope::Pro => "claap-agent",
        Scope::Perso => "perso-agent",
    }
}

/// The word the page shows for a scope.
pub fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::Pro => "Claap",
        Scope::Perso => "Perso",
    }
}

/// What the filter keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Only(Scope),
}

impl Filter {
    pub fn parse(word: &str) -> Option<Filter> {
        match word {
            "all" => Some(Filter::All),
            "pro" => Some(Filter::Only(Scope::Pro)),
            "perso" => Some(Filter::Only(Scope::Perso)),
            _ => None,
        }
    }

    fn keeps(self, item: &Item) -> bool {
        match self {
            Filter::All => true,
            Filter::Only(scope) => item.scope == scope,
        }
    }
}

/// What the list shows: the second filter, next to the scope's
/// (`docs/cockpit.md` § Filtres). `À moi` is what waits on the user, `En
/// cours` what the agents do for them (`crate::inflight`). A new filter is a
/// variant and a line in `SHOWS`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Show {
    #[default]
    Mine,
    /// Only what is still moving: clicks not done, agents at work.
    InFlight,
    /// The clicks done or withdrawn in the last 24 h.
    Done,
}

/// Every filter, in the order the page shows them: (value, word, label).
pub const SHOWS: [(Show, &str, &str); 3] =
    [(Show::Mine, "mine", "\u{c0} moi"), (Show::InFlight, "inflight", "En cours"), (Show::Done, "done", "Fait (24 h)")];

impl Show {
    pub fn parse(word: &str) -> Option<Show> {
        SHOWS.iter().find(|(_, w, _)| *w == word).map(|(s, _, _)| *s)
    }
}

/// Where an item comes from, which decides its buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An entry of the attention queue.
    Attention,
    /// A permission prompt on screen in a pane.
    Permission,
    /// An open `kova ask`.
    Ask,
    /// An open `kova ask` that is a decision sheet.
    Sheet,
    /// A turn that died on a provider error.
    Error,
    /// A routine whose last run failed.
    FailedRoutine,
}

/// Where a click stands, for the card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    Running,
    /// The click went through: the receiver has it.
    Sent,
    /// The go is sent by the command itself, not an agent: its own line.
    SentByCode(String),
    /// The click changed nothing, the command says why: its own line.
    Said(String),
    /// The click is recorded, the receiver does not have it yet.
    Noted,
    Failed(String),
}

/// One action, as the page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Stable across reads: `attention:<id>`, `pane:<id>`, `ask:<id>`,
    /// `routine:<name>`. What a click names.
    pub key: String,
    pub kind: Kind,
    /// 1 urgent, 2 today, 3 this week.
    pub prio: u8,
    /// Epoch seconds since it waits; 0 when unknown (sorts as the oldest).
    pub since: u64,
    pub title: String,
    /// What the user has to do.
    pub needs: String,
    /// Who or what it comes from.
    pub from: String,
    pub scope: Scope,
    pub draft: Option<Draft>,
    /// An attention entry whose action is a link or a file the panel shows.
    pub openable: bool,
    /// The waiting `kova ask` an attention entry stands for (its action is
    /// `kova-sheet:<id>` or `kova-ask:<id>`): its button opens that sheet,
    /// and the ask's own card is left out so it shows once.
    pub ask: Option<String>,
    /// An attention entry that waits on the user's go without a draft: a
    /// `would`, a `review`, or a `decision` with a prepared action. Its card
    /// carries `Refuser` and `Valider`.
    pub validates: bool,
    /// The card came back after its action failed: why. Shown at its head.
    pub failed: Option<String>,
    /// Nobody knows whether its message left: why. Shown at its head, and
    /// `Valider` is no longer the card's blue button.
    pub uncertain: Option<String>,
    /// The proof that its action took place, shown as a link when it is one.
    pub proof: Option<String>,
    /// An action plan: its card carries `No go`, distinct from the x.
    pub plan: bool,
    /// An attention entry's kind (`decision`, `test`, `blocked`…), which
    /// names its main button (`primary`); empty for a live need.
    pub entry_kind: String,
    /// Opened in place: what the card shows under itself. Filled by the
    /// host only for the cards it keeps open.
    pub detail: Option<Detail>,
    pub state: State,
    /// The last instruction typed on an attention card, filled by the host.
    pub instruction: Option<Instructed>,
    /// Screenshots shown in the card, as (label, absolute path): an entry's
    /// `before` and `after` (Robin, 09/11/2026).
    pub shots: Vec<(String, String)>,
    /// Its label, a colored tag next to its space (`attn labels`).
    pub label: Option<crate::attention::Label>,
}

/// What an opened card shows inside it, instead of a jump to another view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detail {
    /// A failed routine: the Scheduled row's body, its prompt and its
    /// actions (`RoutineAct`, the same paths as the sidebar).
    Routine {
        /// Label and value, in the Scheduled body's order.
        rows: Vec<(String, String)>,
        /// The last run failed: its value reads in the danger colour.
        failed: bool,
        prompt: String,
        acts: Vec<RoutineAct>,
        /// What the row is doing (`Running now…`) or why an action failed.
        note: Option<String>,
    },
    /// A permission prompt, as the pane shows it.
    Permission { text: String },
    /// A turn's error: its words and the last lines of the pane.
    Error { words: String, lines: String },
    /// A question: its full text, a button per answer, a field when it takes
    /// free text (the answer with `needs_text` posts it).
    Ask { message: String, answers: Vec<(String, bool)>, input: bool },
    /// An attention entry: what it says, its sources and its links. With
    /// nothing to say, `attn explain` writes a detail meanwhile.
    Entry { needs: String, body: String, detail: String, src: Vec<Source>, links: Vec<String>, explaining: bool },
}

/// A source of an attention entry as the card shows it: its words, and what
/// a click opens in the side panel when the source names a URL or a local
/// file the panel shows (`side_panel::open_target`). A source that opens is
/// never shown as a dead path (Robin, 10/11/2026).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub text: String,
    pub open: Option<String>,
}

/// The part of a source that opens: the first link `links::find` sees in
/// it (a URL, or a path, which may hold spaces and be followed by `l.3` or a
/// remark) that `opens` accepts.
pub fn source_target(text: &str, opens: &dyn Fn(&str) -> bool) -> Option<String> {
    let t = text.trim();
    // `file:///a/b.md` reads as its path.
    let t = &t.replace("file://", " ");
    crate::links::targets(t, opens).into_iter().find(|l| crate::links::is_web(l) || opens(l))
}

/// `source_target` against the real filesystem and `$HOME`.
fn opens_on_disk(link: &str) -> bool {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    crate::window::side_panel::open_target(link, home.as_deref(), &|p: &Path| p.is_file()).is_some()
}

/// A text of a card as HTML: escaped, its URLs, local files, issue ids
/// and pull request numbers as links (`links::linkify_refs`). A click
/// posts `open`, which goes to the side panel.
fn linked(s: &str) -> String {
    crate::links::linkify_refs(s)
}

/// What an opened attention card shows: its needs, body, detail (their
/// links inline), its sources, and under them its action plus the http(s)
/// links of its sources. `explaining` says
/// `attn explain` runs for it.
pub fn entry_detail(entry: &Entry, explaining: bool) -> Detail {
    entry_detail_with(entry, explaining, &opens_on_disk)
}

/// `entry_detail` with what the side panel opens given.
pub fn entry_detail_with(entry: &Entry, explaining: bool, opens: &dyn Fn(&str) -> bool) -> Detail {
    let text = |s: &Option<String>| s.as_deref().unwrap_or_default().to_string();
    let mut links: Vec<String> = Vec::new();
    if let Some(action) = entry.action.as_deref().filter(|a| ask_ref(a).is_none()) {
        links.push(action.to_string());
    }
    for link in crate::links::targets(&entry.src.join("\n"), &|_| false).into_iter().filter(|l| crate::links::is_web(l)) {
        if !links.contains(&link) {
            links.push(link);
        }
    }
    Detail::Entry {
        needs: text(&entry.needs),
        body: text(&entry.body),
        detail: text(&entry.detail),
        src: entry.src.iter().map(|s| Source { text: s.clone(), open: source_target(s, opens) }).collect(),
        links,
        explaining,
    }
}

/// Whether an entry says nothing a card could open on.
pub fn says_nothing(entry: &Entry) -> bool {
    entry.needs.is_none() && entry.body.is_none() && entry.detail.is_none()
}

/// A failed routine's actions inside its card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutineAct {
    Run,
    Resume,
    Log,
    Pause,
    Edit,
}

impl RoutineAct {
    pub fn word(self) -> &'static str {
        match self {
            RoutineAct::Run => "run",
            RoutineAct::Resume => "resume",
            RoutineAct::Log => "log",
            RoutineAct::Pause => "pause",
            RoutineAct::Edit => "edit",
        }
    }

    pub fn parse(word: &str) -> Option<RoutineAct> {
        [RoutineAct::Run, RoutineAct::Resume, RoutineAct::Log, RoutineAct::Pause, RoutineAct::Edit]
            .into_iter()
            .find(|a| a.word() == word)
    }

    fn label(self) -> &'static str {
        match self {
            RoutineAct::Run => "Lancer maintenant",
            RoutineAct::Resume => "Reprendre",
            RoutineAct::Log => "Voir le log",
            RoutineAct::Pause => "Mettre en pause",
            RoutineAct::Edit => "Modifier le prompt",
        }
    }
}

/// Perso when `cwd` is `perso_root` or under it, else Claap (pro). The root
/// is the config's `[attention] perso_dir`; without one, everything is pro.
pub fn scope_of_cwd(cwd: &str, perso_root: Option<&std::path::Path>) -> Scope {
    match perso_root {
        Some(root) if !root.as_os_str().is_empty() && std::path::Path::new(cwd.trim()).starts_with(root) => Scope::Perso,
        _ => Scope::Pro,
    }
}

/// The last `n` lines of a pane's text that are not blank, trimmed at the end.
pub fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The default priority of an attention entry by level; `None` for a memo,
/// which is not an action.
fn level_prio(level: Level) -> Option<u8> {
    match level {
        Level::Now => Some(2),
        Level::Point => Some(3),
        Level::Memo => None,
    }
}

/// An attention entry as an item: `now` and `point` only, its own priority
/// when it has one. An entry closed from here is gone until the next read.
pub fn from_attention(entry: &Entry, outcome: Option<&Outcome>, openable: bool) -> Option<Item> {
    let default = level_prio(entry.level)?;
    let state = match outcome {
        Some(Outcome::Closed) => return None,
        Some(Outcome::Running) => State::Running,
        Some(Outcome::Sent) => State::Sent,
        Some(Outcome::SentByCode(line)) => State::SentByCode(line.clone()),
        Some(Outcome::Said(line)) => State::Said(line.clone()),
        Some(Outcome::Noted) => State::Noted,
        Some(Outcome::Failed { words, .. }) => State::Failed(words.clone()),
        None => State::Idle,
    };
    Some(Item {
        key: format!("attention:{}", entry.id),
        kind: Kind::Attention,
        prio: entry.prio.unwrap_or(default),
        since: entry.since,
        title: entry.title.clone(),
        needs: entry.needs.clone().unwrap_or_default(),
        from: entry.from.clone(),
        scope: entry.scope,
        draft: entry.draft.clone(),
        openable,
        ask: None,
        validates: expects_validation(entry),
        failed: entry.failed.clone(),
        uncertain: entry.uncertain.clone(),
        proof: entry.proof.clone(),
        plan: entry.plan,
        entry_kind: entry.kind.clone(),
        detail: None,
        state,
        instruction: None,
        shots: [("Avant", &entry.before), ("Après", &entry.after)]
            .into_iter()
            .filter_map(|(label, path)| path.clone().map(|p| (label.to_string(), p)))
            .collect(),
        label: entry.label.clone(),
    })
}

/// An entry that waits on the user's go: a `would` or a `review`, or a
/// `decision` whose action is prepared (not a sheet, which takes its own
/// answers). A draft has its own `Valider`.
pub fn expects_validation(entry: &Entry) -> bool {
    let prepared = entry.action.as_deref().is_some_and(|a| !a.trim().is_empty() && ask_ref(a).is_none());
    entry.draft.is_none() && (matches!(entry.kind.as_str(), "would" | "review") || entry.kind == "decision" && prepared)
}

/// The ask id an attention entry's action names: `kova-sheet:<id>` or
/// `kova-ask:<id>`, the id possibly shortened to a prefix.
pub fn ask_ref(action: &str) -> Option<&str> {
    let a = action.trim();
    let id = a.strip_prefix(crate::decision_sheet::LINK_SCHEME).or_else(|| a.strip_prefix("kova-ask:"))?.trim();
    (id.len() >= 6 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')).then_some(id)
}

/// Point attention entries at the waiting asks they name (`ask_ref`, a
/// unique prefix match on `asks`) and drop those asks' own items.
pub fn link_asks(items: &mut Vec<Item>, actions: &[(String, String)], asks: &[String]) {
    let mut linked = std::collections::HashSet::new();
    for item in items.iter_mut().filter(|i| i.kind == Kind::Attention) {
        let Some((_, action)) = actions.iter().find(|(key, _)| *key == item.key) else { continue };
        let Some(r) = ask_ref(action) else { continue };
        let hits: Vec<&String> = asks.iter().filter(|id| id.starts_with(r)).collect();
        if let [id] = hits[..] {
            item.ask = Some(id.clone());
            linked.insert(format!("ask:{id}"));
        }
    }
    items.retain(|i| !(matches!(i.kind, Kind::Sheet | Kind::Ask) && linked.contains(&i.key)));
}

/// The attention entry that stands for the waiting ask `ask_id` (its action
/// names it, `ask_ref`, possibly by a prefix): the card the Cockpit shows
/// for that ask, which the chat shows too. Like `link_asks`, only a ref that
/// names one waiting ask (`waiting`) links.
pub fn entry_for_ask<'a>(entries: &'a [Entry], ask_id: &str, waiting: &[&str]) -> Option<&'a Entry> {
    entries.iter().find(|e| {
        e.action.as_deref().and_then(ask_ref).is_some_and(|r| ask_id.starts_with(r) && waiting.iter().filter(|id| id.starts_with(r)).count() == 1)
    })
}

/// `CARD_CSS` with every selector under `scope` (`#asks`): the chat page
/// takes the cards' look without it reaching its own buttons, and without
/// CSS nesting, which the oldest WebKit Kova runs on lacks.
pub fn scoped_card_css(scope: &str) -> String {
    let mut out = String::with_capacity(CARD_CSS.len() * 2);
    let mut rest = CARD_CSS;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        let selectors: Vec<String> =
            rest[..open].split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| format!("{scope} {s}")).collect();
        out.push_str(&selectors.join(","));
        out.push_str(&rest[open..open + close + 1]);
        out.push('\n');
        rest = &rest[open + close + 1..];
    }
    out
}

/// A live need as an item. `sheet` says an ask is a decision sheet. An
/// attention need is left out: the queue's own entry stands for it. Live
/// needs carry no scope: they count as work.
/// A waiting ask's tile title: the first line of its message, nothing of
/// what follows; a decision sheet's in its one form
/// (`decision_sheet::tile_title`). The host sets it on the item `from_need`
/// made, whose need only carries the message flattened for the sidebar.
pub fn ask_title(message: &str, sheet: bool) -> String {
    let first = message.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    if sheet { crate::decision_sheet::tile_title(first) } else { first.to_string() }
}

pub fn from_need(need: &NeedItem, sheet: bool) -> Option<Item> {
    let (kind, prio, title, needs, from) = match need.kind {
        NeedKind::Attention => return None,
        NeedKind::Permission => (
            Kind::Permission,
            1,
            format!("{} \u{b7} permission à donner", need.title),
            if need.detail.trim().is_empty() { "Autoriser ou refuser".to_string() } else { need.detail.clone() },
            format!("pane {}", need.title),
        ),
        NeedKind::Error => (
            Kind::Error,
            1,
            format!("{} \u{b7} tour arrêté", need.title),
            if need.detail.trim().is_empty() { "Regarder le pane".to_string() } else { need.detail.clone() },
            format!("pane {}", need.title),
        ),
        NeedKind::FailedRoutine => (
            Kind::FailedRoutine,
            2,
            format!("Routine {} \u{b7} en échec", need.title),
            "Regarder pourquoi elle a échoué".to_string(),
            "routines".to_string(),
        ),
        NeedKind::Ask => (
            if sheet { Kind::Sheet } else { Kind::Ask },
            2,
            need.detail.clone(),
            if sheet { "Répondre à la fiche" } else { "Répondre à la question" }.to_string(),
            need.title.clone(),
        ),
    };
    // A sheet: `<Subject> · <N> décisions` (older ones were posted with a
    // colon, and their card text follows on the same line).
    let title = match kind {
        Kind::Sheet => crate::decision_sheet::tile_title(title.split(" \u{2022} ").next().unwrap_or(&title)),
        _ => title,
    };
    Some(Item {
        key: need.key.clone(),
        kind,
        prio,
        since: need.since,
        title,
        needs,
        from,
        scope: Scope::Pro,
        draft: None,
        openable: false,
        ask: None,
        validates: false,
        failed: None,
        uncertain: None,
        proof: None,
        plan: false,
        entry_kind: String::new(),
        detail: None,
        state: State::Idle,
        instruction: None,
        shots: Vec::new(),
        label: None,
    })
}

/// Priority first, then the oldest, then the key: a total order, so two
/// reads of the same items render the same page.
pub fn sort(items: &mut [Item]) {
    items.sort_by(|a, b| (a.prio, a.since, &a.key).cmp(&(b.prio, b.since, &b.key)));
}

/// Whether one action is urgent: the sidebar entry's red dot.
pub fn any_urgent(items: &[Item]) -> bool {
    items.iter().any(|item| item.prio == 1)
}

/// A card's button. One rule for every kind (`docs/cockpit.md`, Buttons):
/// one blue button named by what the user does (`primary`), the rest plain
/// links, the attention icons top right. Nothing reads `Ouvrir`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Open or fold the card's detail in place: a click on the card's body.
    Expand,
    /// The entry's `action` in the side panel: a link, `Voir`.
    Open,
    /// A decision sheet in the side panel: `Décider`.
    OpenSheet,
    /// Open the card in place and give the keyboard to where the user
    /// answers: its `Consigne` (`Brief`, `Décider`, `Tester`) or the
    /// question's answer (`Répondre`).
    Brief,
    Decide,
    Test,
    Reply,
    /// A failed routine's log, the Scheduled tile's own path.
    Log,
    /// A turn's error: the pane, the page closed.
    Pane,
    Done,
    Deny,
    Allow,
    Correct,
    Validate,
    /// Turn down an entry that waits on a go: closed as dismissed, with the
    /// reason typed in the card's small field, if any.
    Refuse,
    /// The attention card's icons: out until a date, not relevant.
    Snooze,
    Dismiss,
    /// No go on an action plan: closed as `nogo`, which its agent learns as
    /// a plan turned down, not as a card that was not relevant.
    NoGo,
    /// The two ways out of a send nobody knows the end of: the user saw the
    /// message (`C'est parti`), or nothing left and it goes again (`Renvoyer`).
    Left,
    Resend,
}

impl Button {
    /// The word the page posts.
    pub fn act(self) -> &'static str {
        match self {
            Button::Expand | Button::Brief | Button::Decide | Button::Test | Button::Reply => "expand",
            Button::Log => "routine",
            Button::Pane => "pane",
            Button::Open | Button::OpenSheet => "open",
            Button::Done => "done",
            Button::Deny => "deny",
            Button::Allow => "allow",
            Button::Correct => "correct",
            Button::Validate => "validate",
            Button::Refuse => "refuse",
            Button::Snooze => "snooze",
            Button::Dismiss => "dismiss",
            Button::NoGo => "nogo",
            Button::Left => "left",
            Button::Resend => "resend",
        }
    }

    pub fn parse(word: &str) -> Option<Button> {
        match word {
            "nogo" => Some(Button::NoGo),
            "left" => Some(Button::Left),
            "resend" => Some(Button::Resend),
            "expand" => Some(Button::Expand),
            "open" => Some(Button::Open),
            "done" => Some(Button::Done),
            "deny" => Some(Button::Deny),
            "allow" => Some(Button::Allow),
            "correct" => Some(Button::Correct),
            "validate" => Some(Button::Validate),
            "refuse" => Some(Button::Refuse),
            "snooze" => Some(Button::Snooze),
            "dismiss" => Some(Button::Dismiss),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Button::Expand => "Détails",
            Button::Open => "Voir",
            Button::OpenSheet | Button::Decide => "Décider",
            Button::Brief => "Brief",
            Button::Test => "Tester",
            Button::Reply => "Répondre",
            Button::Log => "Voir le log",
            Button::Pane => "Voir le pane",
            Button::Done => "C'est fait",
            Button::Deny | Button::Refuse => "Refuser",
            Button::Allow => "Autoriser",
            Button::Correct => "Corriger",
            Button::Validate => "Valider",
            Button::Snooze => "Plus tard",
            Button::Dismiss => "Pas pertinent, retirer",
            Button::NoGo => "No go",
            Button::Left => "C'est parti",
            Button::Resend => "Renvoyer",
        }
    }

    /// Where an opening button puts the keyboard once the card is open.
    fn focus(self) -> Option<&'static str> {
        match self {
            // A draft's Corriger is a consigne to the card's agent, not an edit
            // of the text (07/11): it opens the card on its instruction field.
            Button::Brief | Button::Decide | Button::Test | Button::Correct => Some("ins"),
            Button::Reply => Some("ans"),
            _ => None,
        }
    }
}

/// A card's one blue button: what the user does on it (`docs/cockpit.md`,
/// Buttons). `None` for a card that only informs (its check icon is the
/// action) and once a go went through.
pub fn primary(item: &Item, can_act: bool) -> Option<Button> {
    // Maybe sent already: validating again is not what the card puts first.
    if item.uncertain.is_some() && (item.draft.is_some() || item.validates) {
        return None;
    }
    let sent = matches!(item.state, State::Sent | State::SentByCode(_) | State::Said(_) | State::Noted);
    match item.kind {
        Kind::Attention if item.draft.is_some() && can_act => (!sent).then_some(Button::Validate),
        Kind::Attention if item.ask.is_some() => Some(Button::OpenSheet),
        Kind::Attention if item.validates => (!sent && can_act).then_some(Button::Validate),
        Kind::Attention => match item.entry_kind.as_str() {
            "test" => Some(Button::Test),
            "blocked" | "ask" | "brief" => Some(Button::Brief),
            "decision" => Some(Button::Decide),
            _ => None,
        },
        Kind::Sheet => Some(Button::OpenSheet),
        Kind::Permission => Some(Button::Allow),
        Kind::Ask => Some(Button::Reply),
        Kind::Error => Some(Button::Pane),
        Kind::FailedRoutine => Some(Button::Log),
    }
}

/// A card's actions, left to right: the links, then the blue button last
/// (`primary`). A draft's verbs need `act_command`; a sent draft has none
/// left. `Refuser` needs the close command (`card_html`).
pub fn buttons(item: &Item, can_act: bool) -> Vec<Button> {
    let sent = matches!(item.state, State::Sent | State::SentByCode(_) | State::Said(_) | State::Noted);
    let links: Vec<Button> = match item.kind {
        Kind::Attention if item.draft.is_some() && can_act => match (sent, item.plan) {
            (true, _) => Vec::new(),
            (false, true) => vec![Button::NoGo, Button::Correct],
            (false, false) => vec![Button::Correct],
        },
        Kind::Attention if item.ask.is_some() => Vec::new(),
        Kind::Attention => {
            let open = item.openable.then_some(Button::Open);
            let refuse = (item.validates && !sent).then_some(Button::Refuse);
            // A plan is said no go to, not refused nor crossed out.
            let nogo = (item.plan && !sent).then_some(Button::NoGo);
            open.into_iter().chain(refuse).chain(nogo).collect()
        }
        Kind::Permission => vec![Button::Deny],
        Kind::Sheet | Kind::Ask | Kind::Error | Kind::FailedRoutine => Vec::new(),
    };
    // A send nobody knows the end of: the user settles it, nothing else is
    // offered on the draft (no blue button: neither answer is the default).
    if item.uncertain.is_some() && item.kind == Kind::Attention && (item.draft.is_some() || item.validates) {
        return if sent || !can_act { Vec::new() } else { vec![Button::Left, Button::Resend] };
    }
    links.into_iter().chain(primary(item, can_act)).collect()
}

/// A card's icons, top right: snooze, remove, done. On an attention card a
/// verb whose command is not configured is left out; a waiting ask has all
/// three; the other live kinds have none.
pub fn icons(item: &Item, can_close: bool, can_snooze: bool) -> Vec<Button> {
    match item.kind {
        // A waiting `kova ask` (sheet or question) closes and snoozes on the
        // board itself, no attention command needed (Robin, 09/11/2026).
        Kind::Sheet | Kind::Ask => return vec![Button::Snooze, Button::Dismiss, Button::Done],
        Kind::Attention => {}
        _ => return Vec::new(),
    }
    let mut out = Vec::new();
    if can_snooze {
        out.push(Button::Snooze);
    }
    if can_close && acted(item) {
        // Robin already gave a go or a consigne on it: clearing the card is
        // "done", never "not relevant", which would tell its agent to drop
        // the very thing he just asked for (10/11/2026).
        out.push(Button::Done);
    } else if can_close {
        out.extend([Button::Dismiss, Button::Done]);
    }
    out
}

/// The user already acted on the card: a go went through, or a consigne
/// was sent or is on its way.
pub fn acted(item: &Item) -> bool {
    matches!(item.state, State::Sent | State::SentByCode(_) | State::Said(_) | State::Noted)
        || matches!(&item.instruction, Some(Instructed { outcome: Outcome::Sent | Outcome::Noted | Outcome::Running, .. }))
}

/// `il y a 4 min`, `il y a 2 h`, `hier`, `il y a 3 j`; empty when unknown.
pub fn age(since: u64, now: u64) -> String {
    if since == 0 {
        return String::new();
    }
    let secs = now.saturating_sub(since);
    match secs {
        0..60 => "à l'instant".to_string(),
        60..3600 => format!("il y a {} min", secs / 60),
        3600..86_400 => format!("il y a {} h", secs / 3600),
        86_400..172_800 => "hier".to_string(),
        _ => format!("il y a {} j", secs / 86_400),
    }
}

/// `7 actions pour toi · 1 urgente`.
pub fn subtitle(items: &[Item]) -> (String, String) {
    let n = items.len();
    let urgent = items.iter().filter(|i| i.prio == 1).count();
    let lead = match n {
        0 => "Aucune action pour toi".to_string(),
        1 => "1 action pour toi".to_string(),
        n => format!("{n} actions pour toi"),
    };
    let tail = match urgent {
        0 => String::new(),
        1 => "1 urgente".to_string(),
        k => format!("{k} urgentes"),
    };
    (lead, tail)
}

/// The chat column, when it is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatColumn {
    pub chief: Scope,
    /// The chief's session runs in a pane of this window.
    pub found: bool,
}

/// Everything one render needs.
pub struct View<'a> {
    /// Sorted (`sort`), every scope.
    pub items: &'a [Item],
    pub filter: Filter,
    pub chat: Option<ChatColumn>,
    pub can_act: bool,
    pub can_close: bool,
    pub can_snooze: bool,
    /// How many cards the list shows; `Voir plus` adds `PAGE_SIZE`.
    pub limit: usize,
    pub now: u64,
    /// The catch-up band (`crate::catchup::band_html`), between the title
    /// and the filter; empty when nothing moved.
    pub band: String,
    /// The `Enregistrer` button and the note under the title once a
    /// recording stopped (`crate::record`).
    pub record: String,
    pub record_note: String,
    /// The theme button (`theme_button`), right of Chat.
    pub theme: String,
    /// The second filter (`Show`).
    pub show: Show,
    /// The `En cours` list (`crate::inflight::list_html`), shown in place of
    /// the cards under that filter, and what its button counts.
    pub inflight: String,
    pub inflight_n: usize,
    /// What `Fait (24 h)` counts.
    pub done_n: usize,
    /// The label the list keeps, `None` for every card.
    pub label: Option<String>,
    /// The day's journal (what the routines did, written to a file instead
    /// of cards), when it exists: a quiet link under the list.
    pub journal: Option<String>,
}

/// The cards the list shows first, and each `Voir plus` adds.
pub const PAGE_SIZE: usize = 10;

// ---------------------------------------------------------------
// The page
// ---------------------------------------------------------------

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

const ICON_CHEVRON: &str = r#"<svg class="i" width="13" height="13" viewBox="0 0 24 24"><polyline points="9 18 15 12 9 6"/></svg>"#;
const ICON_EDIT: &str = r#"<svg class="i" width="12" height="12" viewBox="0 0 24 24"><path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"/></svg>"#;
const ICON_CHECK: &str = r#"<svg class="i" width="13" height="13" viewBox="0 0 24 24"><polyline points="20 6 9 17 4 12"/></svg>"#;
const ICON_CHAT: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>"#;
const ICON_SORT: &str = r#"<svg class="i" width="11" height="11" viewBox="0 0 24 24"><path d="M3 6h18M6 12h12M10 18h4"/></svg>"#;
const ICON_X: &str = r#"<svg class="i" width="16" height="16" viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>"#;
// Feather `clock`, `x` and `check`, small, for the attention card's icons.
const ICON_CLOCK_S: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>"#;
const ICON_X_S: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>"#;
const ICON_CHECK_S: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><polyline points="20 6 9 17 4 12"/></svg>"#;

fn icons_html(list: &[Button], disabled: bool) -> String {
    if list.is_empty() {
        return String::new();
    }
    let one: String = list
        .iter()
        .map(|&b| {
            let icon = match b {
                Button::Snooze => ICON_CLOCK_S,
                Button::Dismiss => ICON_X_S,
                _ => ICON_CHECK_S,
            };
            format!(
                "<button type=\"button\" class=\"ico\" data-act=\"{}\" title=\"{}\" aria-label=\"{}\"{}>{icon}</button>",
                b.act(),
                b.label(),
                b.label(),
                if disabled { " disabled" } else { "" }
            )
        })
        .collect();
    format!("<div class=\"icons\">{one}</div>")
}

/// The blue button, or a plain link for the rest.
fn button_html(button: Button, disabled: bool, primary: bool) -> String {
    let dis = if disabled { " disabled" } else { "" };
    let mut data = format!("data-act=\"{}\"", button.act());
    if button == Button::Log {
        data.push_str(&format!(" data-arg=\"{}\"", RoutineAct::Log.word()));
    }
    if let Some(focus) = button.focus() {
        data.push_str(&format!(" data-focus=\"{focus}\""));
    }
    if !primary {
        return format!("<button type=\"button\" class=\"link\" {data}{dis}>{}</button>", button.label());
    }
    let icon = if button == Button::Validate { ICON_CHECK } else { "" };
    format!("<button type=\"button\" class=\"btn acc\" {data}{dis}>{icon}<span>{}</span></button>", button.label())
}

/// A label's dot in the filter menu, of its color.
fn label_dot(view: &View, name: &str) -> String {
    let color = view.items.iter().filter_map(|i| i.label.as_ref()).find(|l| l.name == name).map(|l| l.color.as_str()).unwrap_or("#94A3B8");
    format!("<i class=\"ld\" style=\"background:{}\"></i>", esc(color))
}

/// A card's label: a pill filled with its color, its name in the ink that
/// reads best on it (`ink_on`). The name says what the label is: nothing
/// rests on the color alone.
fn label_html(label: &Option<crate::attention::Label>) -> String {
    match label {
        Some(l) if crate::attention::hex_color(&l.color) => format!(
            "<span class=\"tag lb\" style=\"background:{};color:{}\">{}</span>",
            l.color,
            ink_on(&l.color),
            esc(&l.name)
        ),
        _ => String::new(),
    }
}

/// Near black or white, whichever contrasts more with a `#RRGGBB` fill
/// (WCAG relative luminance): the label's name stays readable on any color.
pub fn ink_on(fill: &str) -> &'static str {
    let channel = |i: usize| {
        let v = u8::from_str_radix(fill.get(i..i + 2).unwrap_or("80"), 16).unwrap_or(128) as f64 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let l = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
    // Contrast with white is 1.05 / (l + 0.05), with #111 about (l + 0.05) / 0.056.
    if 1.05 / (l + 0.05) >= (l + 0.05) / 0.056 { "#FFFFFF" } else { "#111111" }
}

/// A card's space (`Claap`, `Perso`): a pill filled with the space's color,
/// like a label, its name in the ink that reads on it. No dot: the name says
/// which space, nothing rests on the color alone.
fn tag_html(scope: Scope) -> String {
    format!(
        "<span class=\"tag sp {}\">{}</span>",
        if scope == Scope::Perso { "perso" } else { "pro" },
        scope_label(scope)
    )
}

fn source_html(item: &Item, now: u64) -> String {
    let words: Vec<String> = [item.from.trim().to_string(), age(item.since, now)].into_iter().filter(|w| !w.is_empty()).collect();
    format!("<div class=\"src\">{}{}<span>{}</span></div>", tag_html(item.scope), label_html(&item.label), esc(&words.join(" \u{b7} ")))
}

fn state_html(state: &State) -> String {
    match state {
        State::Idle => String::new(),
        State::Running => "<div class=\"state\">En cours</div>".to_string(),
        State::Sent => "<div class=\"state ok\">Transmis à l'agent</div>".to_string(),
        State::SentByCode(line) => format!("<div class=\"state ok\">{}</div>", esc(line)),
        // Neutral: neither a delivery (green) nor a failure (red).
        State::Said(line) => format!("<div class=\"state\">{}</div>", linked(line)),
        State::Noted => "<div class=\"state warn\">Noté, pas encore remis à l'agent</div>".to_string(),
        State::Failed(words) => format!("<div class=\"state crit\">Non transmis : {}</div>", esc(words)),
    }
}

/// One card.
pub fn card_html(item: &Item, can_act: bool, can_close: bool, can_snooze: bool, now: u64) -> String {
    // `Refuser` closes the entry: no close command, no `Refuser`.
    let list: Vec<Button> = buttons(item, can_act).into_iter().filter(|&b| can_close || b != Button::Refuse).collect();
    card_html_with(item, &list, &icons(item, can_close, can_snooze), can_act, now)
}

thread_local! {
    /// MOCK-UP ONLY (branch `reply-zone-mock`, not for main): the reply zone
    /// variant an opened attention card shows, `'a'` or `'b'`, and the cards
    /// shown as "asked" (key -> the question typed). Set by `kova render`.
    pub static REPLY_MOCK: std::cell::RefCell<(Option<char>, std::collections::HashMap<String, String>)> =
        std::cell::RefCell::new((None, std::collections::HashMap::new()));
}

const ICON_NOGO: &str = r#"<svg class="i" width="13" height="13" viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>"#;
const ICON_Q: &str = r#"<svg class="i" width="13" height="13" viewBox="0 0 24 24"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>"#;

/// MOCK-UP ONLY: the reply zone of an opened card. One field, and the
/// decision in the same gesture.
/// - `a`: three buttons. `No go` (outlined, a cross), `Demander` (a link,
///   a bubble), `Go` (filled, a check). Text and shape tell them apart, not
///   green and red.
/// - `b`: one choice then one send: a segmented `Go | No go | Question`,
///   then `Envoyer`.
fn reply_zone_html(item: &Item, variant: char, asked: Option<&str>) -> String {
    let go = if item.draft.is_some() { "Go, envoyer" } else { "Go" };
    let placeholder = if item.draft.is_some() { "Un commentaire pour l'agent (facultatif)" } else { "Ton commentaire (facultatif)" };
    let asked_html = asked
        .map(|q| format!("<div class=\"state warn\">Attend l'agent \u{b7} ta question est partie</div><p class=\"msg sent\">{}</p>\
             <button type=\"button\" class=\"link undo\">Annuler ma demande</button>", linked(q)))
        .unwrap_or_default();
    let buttons = match variant {
        'b' => format!(
            "<div class=\"rz-b\"><div class=\"seg\" role=\"radiogroup\"><button type=\"button\" class=\"on\">{ICON_CHECK}<span>Go</span></button>\
             <button type=\"button\">{ICON_NOGO}<span>No go</span></button><button type=\"button\">{ICON_Q}<span>Question</span></button></div>\
             <button type=\"button\" class=\"btn acc\"><span>Envoyer</span></button></div>"
        ),
        _ => format!(
            "<div class=\"rz-a\"><span class=\"hint\">Entrée : demander \u{b7} \u{2318}Entrée : go</span>\
             <button type=\"button\" class=\"btn out\">{ICON_NOGO}<span>No go</span></button>\
             <button type=\"button\" class=\"link\">{ICON_Q}<span>Demander</span></button>\
             <button type=\"button\" class=\"btn acc\">{ICON_CHECK}<span>{go}</span></button></div>"
        ),
    };
    format!(
        "<div class=\"instr rz\"><div class=\"lbl\">Ta réponse</div>{asked_html}\
         <textarea class=\"ins\" rows=\"2\" spellcheck=\"true\" placeholder=\"{placeholder}\"></textarea>{buttons}</div>"
    )
}

/// One card with the buttons and icons given: the chat's ask cards pick
/// their own (`chat::cards::ask_card_html`) on the Cockpit's markup.
pub fn card_html_with(item: &Item, list: &[Button], icon_list: &[Button], can_act: bool, now: u64) -> String {
    let running = item.state == State::Running;
    // The blue one is the card's `primary`, whoever picked the list.
    let main = primary(item, can_act);
    let acts: String = list.iter().map(|&b| button_html(b, running, Some(b) == main)).collect();
    let acts = format!("<div class=\"acts\">{acts}</div>");
    let icons = icons_html(icon_list, running);
    let mut detail = item.detail.as_ref().map(|d| detail_html(d, running)).unwrap_or_default();
    // Before and after, in the card itself: validated without opening anything.
    detail.insert_str(0, &shots_html(&item.shots, &shot_src));
    // `Refuser` opens this field first: Enter confirms, Escape cancels.
    if list.contains(&Button::Refuse) {
        detail.insert_str(0, &format!("<div class=\"why\" hidden><input type=\"text\" class=\"why\" placeholder=\"{REFUSE_PLACEHOLDER}\" maxlength=\"500\"></div>"));
    }
    // An opened attention card takes an instruction, action or not, under
    // the draft's actions when it has a draft.
    let (mock, asked) = REPLY_MOCK.with(|m| {
        let m = m.borrow();
        (m.0.filter(|_| item.kind == Kind::Attention && item.detail.is_some() && can_act), m.1.get(&item.key).cloned())
    });
    // MOCK-UP ONLY: the card's own verbs move into the reply zone; an asked
    // card keeps the clock only, the check and the cross are out of reach.
    let (acts, icons) = match mock {
        Some(_) => (
            format!("<div class=\"acts\">{}</div>", list.iter().filter(|b| matches!(b, Button::Open)).map(|&b| button_html(b, running, false)).collect::<String>()),
            // Asked: the clock only. Else the clock and a neutral `Vu`, with
            // its word, where the check and the cross were: it puts the card
            // away without saying go nor no go.
            if asked.is_some() {
                icons_html(&[Button::Snooze], running)
            } else {
                icons_html(&[Button::Snooze], running).replacen(
                    "</div>",
                    &format!("<button type=\"button\" class=\"link vu\" title=\"Ranger la carte, sans go ni no go\">{ICON_CHECK}<span>Vu</span></button></div>"),
                    1,
                )
            },
        ),
        None => (acts, icons),
    };
    if let Some(variant) = mock {
        detail.push_str(&reply_zone_html(item, variant, asked.as_deref()));
    } else if item.kind == Kind::Attention && item.detail.is_some() && can_act {
        detail.push_str(&instruct_html(item.instruction.as_ref()));
    }
    // Folded, a card is its title, its label, its source and its age: what
    // is expected reads in the title, and under it once the card is open.
    let needs = if item.needs.trim().is_empty() || item.detail.is_none() {
        String::new()
    } else {
        format!("<div class=\"need\">{ICON_CHEVRON}<span>{}</span></div>", linked(&item.needs))
    };
    // Back after its action failed: the reason first, the card as it was.
    let failed = item.failed.as_deref().map(|why| format!("<div class=\"state crit top\">Échec : {}</div>", linked(why))).unwrap_or_default();
    let unsure = item
        .uncertain
        .as_deref()
        .map(|why| format!("<div class=\"state warn top\">Peut-être parti : {}</div>", linked(why)))
        .unwrap_or_default();
    let failed = format!("{failed}{unsure}");
    let proof = item.proof.as_deref().map(|p| format!("<div class=\"state ok\">Preuve : {}</div>", linked(p))).unwrap_or_default();
    let head = format!("{failed}<div class=\"t\">{}</div>{needs}{proof}", linked(&item.title));
    let body = match (&item.draft, item.kind == Kind::Attention && can_act) {
        (Some(draft), true) => {
            let subject = draft
                .subject
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| format!("<span>Objet <b>{}</b></span>", esc(s)))
                .unwrap_or_default();
            let channel = if draft.channel.trim().is_empty() { String::new() } else { format!("<span>{}</span>", esc(&draft.channel)) };
            // The name a person reads, then where the message really goes.
            // In words when the queue gives them (`DM Sacha`), the raw id
            // then in the tooltip; else the raw id.
            let clean = |s: &Option<String>| s.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
            let target = match (clean(&draft.target_label), clean(&draft.target)) {
                (Some(label), raw) => format!(
                    " <span class=\"tg{}\" title=\"Cible de l'envoi{}\">{}</span>",
                    if draft.target_kind.as_deref() == Some("external") { " ext" } else { "" },
                    raw.map(|r| format!(" : {}", esc(&r))).unwrap_or_default(),
                    esc(&label)
                ),
                (None, Some(raw)) => format!(" <code class=\"tg\" title=\"Cible de l'envoi\">{}</code>", esc(&raw)),
                (None, None) => String::new(),
            };
            let to = if draft.to.trim().is_empty() && target.is_empty() {
                String::new()
            } else {
                format!("<span>À <b>{}</b>{target}</span>", esc(&draft.to))
            };
            // The newline after `<textarea>` is the one the parser drops, so
            // a text that starts with a newline keeps it.
            format!(
                "<div class=\"row\"><div class=\"txt\">{head}</div>{icons}</div>\
                 <div class=\"draft\"><div class=\"hd\">{to}{channel}{subject}<span class=\"edit\">{ICON_EDIT}Modifiable</span></div>\
                 <textarea class=\"bd\" spellcheck=\"true\" data-orig=\"{orig}\"{ro}>\n{text}</textarea></div>\
                 <div class=\"draft-foot\">{src}{acts}</div>{detail}{state}",
                orig = esc(&draft.text),
                text = esc(&draft.text),
                ro = if matches!(item.state, State::Sent | State::SentByCode(_) | State::Said(_) | State::Noted) || running { " readonly" } else { "" },
                src = source_html(item, now),
                state = state_html(&item.state),
            )
        }
        _ if !icons.is_empty() => format!(
            "<div class=\"row\"><div class=\"txt\">{head}{src}</div><div class=\"side\">{icons}{acts}</div></div>{detail}{state}",
            src = source_html(item, now),
            state = state_html(&item.state),
        ),
        _ => format!(
            "<div class=\"row\"><div class=\"txt\">{head}{src}</div>{acts}</div>{detail}{state}",
            src = source_html(item, now),
            state = state_html(&item.state),
        ),
    };
    format!(
        "<article class=\"card{}{}\" data-key=\"{}\">{body}</article>",
        if item.prio == 1 { " urgent" } else { "" },
        if item.detail.is_some() { " open" } else { "" },
        esc(&item.key),
    )
}

/// A card's screenshots side by side, each with its label; a click shows it
/// full size. One that cannot be read says so instead. `src` turns a path into
/// what an `<img>` loads.
fn shots_html(shots: &[(String, String)], src: &dyn Fn(&str) -> Result<String, String>) -> String {
    if shots.is_empty() {
        return String::new();
    }
    let figs: String = shots
        .iter()
        .map(|(label, path)| match src(path) {
            Ok(uri) => format!(
                "<figure class=\"shot\"><figcaption>{l}</figcaption><img src=\"{uri}\" alt=\"{l}\" title=\"Agrandir\" data-zoom=\"{l}\"></figure>",
                l = esc(label)
            ),
            Err(why) => format!("<figure class=\"shot\"><figcaption>{} : {}</figcaption></figure>", esc(label), esc(&why)),
        })
        .collect();
    format!("<div class=\"shots\">{figs}</div>")
}

/// The largest screenshot a card embeds.
const MAX_SHOT: u64 = 6 * 1024 * 1024;

/// A screenshot as a `data:` URI, read once per file version: the list is
/// rendered on every refresh.
fn shot_src(path: &str) -> Result<String, String> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<String, (std::time::SystemTime, String)>>> = Mutex::new(None);
    if !path.starts_with('/') || path.chars().any(char::is_control) {
        return Err("chemin non absolu".into());
    }
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => return Err("pas une image".into()),
    };
    let meta = std::fs::metadata(path).map_err(|_| "fichier introuvable".to_string())?;
    if !meta.is_file() {
        return Err("pas un fichier".into());
    }
    if meta.len() > MAX_SHOT {
        return Err("image trop lourde".into());
    }
    let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some((at, uri)) = cache.get(path)
        && *at == modified
    {
        return Ok(uri.clone());
    }
    let bytes = std::fs::read(path).map_err(|_| "illisible".to_string())?;
    if bytes.len() as u64 > MAX_SHOT {
        return Err("image trop lourde".into());
    }
    let uri = format!("data:{mime};base64,{}", crate::decision_sheet::base64(&bytes));
    cache.insert(path.to_string(), (modified, uri.clone()));
    Ok(uri)
}

/// The placeholder of the reason field `Refuser` opens.
pub const REFUSE_PLACEHOLDER: &str = "Pourquoi ? (facultatif)";

/// The placeholder of an opened card's instruction field.
pub const INSTRUCT_PLACEHOLDER: &str = "Une consigne pour cette carte";

/// The instruction field of an opened attention card, and what the last
/// instruction became. Enter sends it (`act_command ID instruct`), Shift+Enter
/// is a new line; distinct from a draft's correction.
fn instruct_html(last: Option<&Instructed>) -> String {
    let running = last.is_some_and(|i| i.outcome == Outcome::Running);
    let note = match last {
        None => String::new(),
        Some(Instructed { outcome: Outcome::Running, .. }) => "<div class=\"state\">En cours</div>".to_string(),
        Some(Instructed { text, outcome: Outcome::Failed { words, .. } }) => format!(
            "<div class=\"state crit\">Consigne non envoyée : {}</div><p class=\"msg sent\">{}</p>",
            esc(words),
            esc(text)
        ),
        Some(Instructed { text, outcome: Outcome::Noted }) => {
            format!("<div class=\"state warn\">Consigne notée, pas encore remise à l'agent</div><p class=\"msg sent\">{}</p>", linked(text))
        }
        Some(Instructed { text, .. }) => {
            format!("<div class=\"state ok\">Consigne remise à l'agent</div><p class=\"msg sent\">{}</p>", linked(text))
        }
    };
    format!(
        "<div class=\"instr\"><div class=\"lbl\">Consigne</div>\
         <textarea class=\"ins\" rows=\"2\" spellcheck=\"true\" placeholder=\"{INSTRUCT_PLACEHOLDER}\"{}></textarea>{note}</div>",
        if running { " readonly" } else { "" }
    )
}

/// What an opened card shows under its head.
pub fn detail_html(detail: &Detail, running: bool) -> String {
    let dis = if running { " disabled" } else { "" };
    let inner = match detail {
        Detail::Routine { rows, failed, prompt, acts, note } => {
            let kv: String = rows
                .iter()
                .enumerate()
                .map(|(i, (k, v))| {
                    // The third row is the last run: the one with a verdict.
                    let crit = if *failed && i == 2 { " class=\"crit\"" } else { "" };
                    format!("<dt>{}</dt><dd{crit}>{}</dd>", esc(k), esc(v))
                })
                .collect();
            let prompt = if prompt.trim().is_empty() {
                String::new()
            } else {
                format!("<details class=\"fold\"><summary>Voir le prompt</summary><pre class=\"scroll\">{}</pre></details>", esc(prompt))
            };
            // `Voir le log` is the card's blue button already.
            let buttons: String = acts
                .iter()
                .filter(|a| **a != RoutineAct::Log)
                .map(|a| format!("<button type=\"button\" class=\"link\" data-act=\"routine\" data-arg=\"{}\"{dis}>{}</button>", a.word(), a.label()))
                .collect();
            let note = note.as_deref().map(|n| format!("<div class=\"state\">{}</div>", esc(n))).unwrap_or_default();
            format!("<dl class=\"kv\">{kv}</dl>{prompt}<div class=\"dacts\">{buttons}</div>{note}")
        }
        Detail::Permission { text } => format!("<pre class=\"scroll\">{}</pre>", esc(text)),
        Detail::Error { words, lines } => {
            let words = if words.trim().is_empty() { String::new() } else { format!("<p class=\"words\">{}</p>", esc(words)) };
            let lines = if lines.trim().is_empty() { String::new() } else { format!("<pre class=\"scroll\">{}</pre>", esc(lines)) };
            // `Voir le pane` is the card's blue button.
            format!("{words}{lines}")
        }
        Detail::Ask { message, answers, input } => {
            let field = if *input || answers.iter().any(|(_, t)| *t) {
                "<textarea class=\"ans\" rows=\"2\" placeholder=\"Ta réponse\"></textarea>"
            } else {
                ""
            };
            let buttons: String = answers
                .iter()
                .map(|(label, text)| {
                    format!(
                        "<button type=\"button\" class=\"btn sec\" data-act=\"answer\" data-arg=\"{}\"{}{dis}><span>{}</span></button>",
                        esc(label),
                        if *text { " data-text=\"1\"" } else { "" },
                        esc(label)
                    )
                })
                .collect();
            format!("<p class=\"msg\">{}</p>{field}<div class=\"dacts\">{buttons}</div>", linked(message))
        }
        Detail::Entry { needs, body, detail, src, links, explaining } => {
            let para = |class: &str, t: &str| {
                if t.trim().is_empty() { String::new() } else { format!("<p class=\"{class}\">{}</p>", linked(t.trim())) }
            };
            let mut out = String::new();
            if needs.trim().is_empty() && body.trim().is_empty() && detail.trim().is_empty() {
                out.push_str(if *explaining {
                    "<p class=\"msg dim\">Je regarde de quoi il s'agit\u{2026}</p>"
                } else {
                    "<p class=\"msg dim\">Rien de plus sur cette entrée</p>"
                });
            }
            out.push_str(&para("msg need-t", needs));
            out.push_str(&para("msg", body));
            out.push_str(&para("msg", detail));
            if !src.is_empty() {
                let items: String = src
                    .iter()
                    .map(|s| match &s.open {
                        // The file's name opens it in the panel; what the
                        // source says around the path stays as words.
                        Some(open) => {
                            let around = s.text.replacen(open.as_str(), "", 1);
                            let around = around.trim();
                            let around = if around.is_empty() { String::new() } else { format!(" <span class=\"w\">{}</span>", linked(around)) };
                            format!(
                                "<li><button type=\"button\" class=\"link\" data-act=\"link\" data-arg=\"{}\" title=\"{}\">{}</button>{around}</li>",
                                esc(open),
                                esc(open),
                                esc(&crate::links::label(open, 70))
                            )
                        }
                        None => format!("<li>{}</li>", linked(&s.text)),
                    })
                    .collect();
                out.push_str(&format!("<div class=\"lbl\">Sources</div><ul class=\"srcs\">{items}</ul>"));
            }
            if !links.is_empty() {
                let items: String = links
                    .iter()
                    .map(|l| format!("<button type=\"button\" class=\"link\" data-act=\"link\" data-arg=\"{0}\" title=\"{0}\">{1}</button>", esc(l), esc(&crate::links::label(l, 70))))
                    .collect();
                out.push_str(&format!("<div class=\"links\">{items}</div>"));
            }
            out
        }
    };
    format!("<div class=\"detail\">{inner}</div>")
}

const SECTIONS: [(u8, &str, &str); 3] = [(1, "Urgent", "crit"), (2, "Aujourd'hui", "warn"), (3, "Cette semaine", "gray")];

impl View<'_> {
    /// A card the space and the label filters both keep.
    fn keeps(&self, item: &Item) -> bool {
        self.filter.keeps(item) && self.label.as_ref().is_none_or(|l| item.label.as_ref().is_some_and(|x| &x.name == l))
    }
}

/// The cards the filter keeps, under their section headers.
pub fn list_html(view: &View) -> String {
    if view.show != Show::Mine {
        return if view.inflight.is_empty() { "<p class=\"empty\">Lecture en cours\u{2026}</p>".to_string() } else { view.inflight.clone() };
    }
    let kept: Vec<&Item> = view.items.iter().filter(|i| view.keeps(i)).collect();
    if kept.is_empty() {
        return "<p class=\"empty\">Rien ne t'attend ici</p>".to_string();
    }
    // The first `limit` cards in the priority order; a section's count is
    // its full count.
    let limit = view.limit.max(1);
    let mut out = String::new();
    let mut shown = 0;
    for (prio, name, dot) in SECTIONS {
        let cards: Vec<&&Item> = kept.iter().filter(|i| i.prio.clamp(1, 3) == prio).collect();
        if cards.is_empty() || shown >= limit {
            continue;
        }
        out.push_str(&format!(
            "<div class=\"sec-h\"><span class=\"d {dot}\"></span>{name} <span class=\"k\">{}</span></div>",
            cards.len()
        ));
        for item in cards.iter().take(limit - shown) {
            out.push_str(&card_html(item, view.can_act, view.can_close, view.can_snooze, view.now));
            shown += 1;
        }
    }
    let left = kept.len() - shown;
    if left > 0 {
        out.push_str(&format!("<button type=\"button\" class=\"btn sec more\" data-more=\"1\"><span>Voir plus ({left})</span></button>"));
    }
    out
}

const ICON_SUN: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></svg>"#;
const ICON_MOON: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/></svg>"#;
const ICON_SYSTEM: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M12 3v18" /><path d="M12 3a9 9 0 0 1 0 18z" fill="currentColor"/></svg>"#;

/// A shortcut as macOS menus show it: `cmd+option+l` is `⌥⌘L`.
pub fn shortcut_label(keys: &str) -> String {
    let parts: Vec<String> = keys.split('+').map(|p| p.trim().to_ascii_lowercase()).collect();
    let has = |m: &[&str]| parts.iter().any(|p| m.contains(&p.as_str()));
    let mut out = String::new();
    for (names, glyph) in [(&["ctrl", "control"][..], "\u{2303}"), (&["option", "alt", "opt"][..], "\u{2325}"), (&["shift"][..], "\u{21e7}"), (&["cmd", "command", "super"][..], "\u{2318}")] {
        if has(names) {
            out.push_str(glyph);
        }
    }
    let modifiers = ["ctrl", "control", "option", "alt", "opt", "shift", "cmd", "command", "super"];
    if let Some(key) = parts.iter().find(|p| !modifiers.contains(&p.as_str())) {
        out.push_str(&key.to_uppercase());
    }
    out
}

/// The header's theme button: the icon of the current mode, a click goes to
/// the next one (Clair, Sombre, Système), as View > Theme and the shortcut do.
pub fn theme_button(mode: crate::theme::ThemeMode, shortcut: &str) -> String {
    use crate::theme::ThemeMode;
    let icon = match mode {
        ThemeMode::Light => ICON_SUN,
        ThemeMode::Dark => ICON_MOON,
        ThemeMode::System => ICON_SYSTEM,
    };
    let keys = shortcut_label(shortcut);
    let keys = if keys.is_empty() { String::new() } else { format!(" ({keys})") };
    format!(
        "<button type=\"button\" class=\"btn sec ico\" data-theme=\"cycle\" title=\"Thème : {}, clic pour {}{}\" aria-label=\"Thème {}\">{icon}</button>",
        mode.label(),
        mode.next().label(),
        esc(&keys),
        mode.label()
    )
}

/// The title block, the Chat button, the filter.
pub fn head_html(view: &View) -> String {
    let (lead, tail) = subtitle(view.items);
    let tail = if tail.is_empty() { String::new() } else { format!(" \u{b7} <b>{}</b>", esc(&tail)) };
    let sort = match view.show {
        Show::Mine => format!("{ICON_SORT}Par priorité"),
        Show::InFlight => "En direct".to_string(),
        Show::Done => "Dernières 24 h".to_string(),
    };
    format!(
        "<div class=\"phead\"><div><div class=\"ptitle\">Cockpit</div><div class=\"psub\">{}{tail}</div></div>\
         <div class=\"hbtns\">{}<button type=\"button\" class=\"btn {}\" data-chat=\"toggle\">{ICON_CHAT}<span>Chat</span></button>{}</div></div>\
         {}{}<div class=\"bar\">{}<span class=\"sortlbl\">{sort}</span></div>",
        esc(&lead),
        view.record,
        if view.chat.is_some() { "on" } else { "sec" },
        view.theme,
        view.record_note,
        view.band,
        filters_html(view),
    )
}

// Feather `filter`, the same size and stroke as the sort icon.
const ICON_FILTER: &str = r#"<svg class="i" width="11" height="11" viewBox="0 0 24 24"><polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3"/></svg>"#;
const ICON_PLUS: &str = r#"<svg class="i" width="11" height="11" viewBox="0 0 24 24"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>"#;

/// One option of the filter menu: (word, label, count, chosen).
struct FilterOption<'a> {
    attr: &'static str,
    word: &'a str,
    label: String,
    n: usize,
    on: bool,
}

/// The filters, Linear-like (Robin, 10/11/2026): no chip is the whole list
/// (`Tout`, `À moi`); `Filtrer` opens a menu of every dimension and its
/// values with their counts; a value chosen shows as a chip
/// `Espace : Claap` with its cross. Picking the chosen value again, or the
/// cross, removes it. The menu itself lives outside the head (`#fm`), filled
/// from `#fmsrc` here, so a push never closes it.
pub fn filters_html(view: &View) -> String {
    let count = |scope: Scope| view.items.iter().filter(|i| i.scope == scope).count();
    let space = [Scope::Pro, Scope::Perso].map(|scope| FilterOption {
        attr: "filter",
        word: if scope == Scope::Perso { "perso" } else { "pro" },
        label: scope_label(scope).to_string(),
        n: count(scope),
        on: view.filter == Filter::Only(scope),
    });
    let shows: Vec<FilterOption> = SHOWS
        .iter()
        .map(|(show, word, label)| FilterOption {
            attr: "show",
            word,
            label: label.to_string(),
            n: match show {
                Show::Mine => view.items.iter().filter(|i| view.keeps(i)).count(),
                Show::InFlight => view.inflight_n,
                Show::Done => view.done_n,
            },
            on: view.show == *show,
        })
        .collect();
    // The labels the cards carry, the most used first; a dot of its color.
    let mut names: Vec<(&str, usize)> = Vec::new();
    for l in view.items.iter().filter(|i| view.filter.keeps(i)).filter_map(|i| i.label.as_ref()) {
        match names.iter_mut().find(|(n, _)| *n == l.name) {
            Some((_, c)) => *c += 1,
            None => names.push((&l.name, 1)),
        }
    }
    names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    if let Some(on) = view.label.as_deref()
        && !names.iter().any(|(n, _)| *n == on)
    {
        // The chosen label stays removable when no card carries it any more.
        names.push((on, 0));
    }
    let labels: Vec<FilterOption> = names
        .iter()
        .map(|(l, n)| FilterOption { attr: "label", word: l, label: l.to_string(), n: *n, on: view.label.as_deref() == Some(*l) })
        .collect();
    // The default of each dimension: what removing its chip puts back.
    let groups: [(&str, &str, &str, &[FilterOption]); 3] =
        [("Espace", "filter", "all", &space[..]), ("Vue", "show", "mine", &shows[..]), ("Label", "label", "all", &labels[..])];
    let mut chips = String::new();
    let mut menu = String::new();
    for (name, attr, default, options) in groups {
        if options.is_empty() {
            continue;
        }
        menu.push_str(&format!("<div class=\"fm-h\">{name}</div>"));
        for o in options {
            // A chosen value: picked again, it goes back to the default.
            let word = if o.on { default } else { o.word };
            menu.push_str(&format!(
                "<button type=\"button\" class=\"fm-o{}\" data-{}=\"{word}\"><span class=\"ok\">{ICON_CHECK}</span><span>{}</span><span class=\"c\">{}</span></button>",
                if o.on { " on" } else { "" },
                o.attr,
                if o.attr == "label" { label_dot(view, &o.label) + &esc(&o.label) } else { esc(&o.label) },
                o.n
            ));
            if o.on && o.word != default {
                chips.push_str(&format!(
                    "<span class=\"chip\"><button type=\"button\" class=\"cb\" data-fopen=\"1\"><span class=\"ck\">{name} :</span> {}</button>\
                     <button type=\"button\" class=\"cx\" data-{attr}=\"{default}\" title=\"Retirer le filtre\" aria-label=\"Retirer le filtre {name}\">{ICON_X_S}</button></span>",
                    esc(&o.label)
                ));
            }
        }
    }
    let open = if chips.is_empty() {
        format!("<button type=\"button\" class=\"flt\" data-fopen=\"1\">{ICON_FILTER}<span>Filtrer</span></button>")
    } else {
        format!("<button type=\"button\" class=\"flt\" data-fopen=\"1\" title=\"Ajouter un filtre\" aria-label=\"Ajouter un filtre\">{ICON_PLUS}</button>")
    };
    format!("<div class=\"filters\">{chips}{open}</div><div id=\"fmsrc\" hidden>{menu}</div>")
}

/// The chat column's top bar and, when the chief has no pane, the one line
/// that says so. Its body is where the chat's own web view sits.
pub fn chat_html(chat: &ChatColumn) -> String {
    let seg = |scope: Scope| {
        format!(
            "<button type=\"button\" class=\"{}\" data-chief=\"{}\">{}</button>",
            if chat.chief == scope { "on" } else { "" },
            if scope == Scope::Perso { "perso" } else { "pro" },
            scope_label(scope)
        )
    };
    let name = chief_session(chat.chief);
    let body = if chat.found {
        String::new()
    } else {
        format!("<p class=\"none\">Pas de session {} dans cette fenêtre</p>", esc(name))
    };
    format!(
        "<div class=\"top\"><div class=\"seg\">{}{}</div><span class=\"who{}\"><i></i>{}</span>\
         <button type=\"button\" class=\"x\" data-chat=\"close\" title=\"Fermer le chat\">{ICON_X}</button></div><div class=\"cbody\">{body}</div>",
        seg(Scope::Pro),
        seg(Scope::Perso),
        if chat.found { "" } else { " off" },
        esc(name),
    )
}

/// What the page is pushed: each part only when it changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Update {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<String>,
    /// `Some("")` closes the chat column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    /// Give the keyboard to the page.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub focus: bool,
    /// Scroll to this card and light it, once the list is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reveal: Option<String>,
}

impl Update {
    pub fn is_empty(&self) -> bool {
        self.head.is_none() && self.list.is_none() && self.chat.is_none() && !self.focus && self.reveal.is_none()
    }
}

/// The three parts of a render, compared with what the page holds.
pub fn parts(view: &View) -> (String, String, String) {
    (head_html(view), list_html(view) + &journal_html(view), view.chat.as_ref().map(chat_html).unwrap_or_default())
}

/// The quiet link to the day's journal, at the bottom of the page: opened in
/// the side panel like any link to a file.
fn journal_html(view: &View) -> String {
    view.journal
        .as_deref()
        .map(|path| format!("<div class=\"jr\">{}Journal du jour</a></div>", crate::links::anchor(path, crate::links::Kind::Path)))
        .unwrap_or_default()
}

pub fn update_script(update: &Update) -> String {
    format!("window.kova && window.kova.update({})", serde_json::to_string(update).unwrap_or_else(|_| "{}".into()))
}

fn hex(c: [f32; 4]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// The design system's colours as CSS variables, light or dark.
pub fn vars(light: bool) -> String {
    use crate::window::ds_tokens as ds;
    macro_rules! set {
        ($m:ident, $t:ident) => {
            [
                ("bg0", hex(ds::$m::BG0)),
                ("bg1", hex(ds::$m::BG1)),
                ("bg2", hex(ds::$m::BG2)),
                ("bg-input", hex(ds::$m::BG_INPUT)),
                ("bg-elev", hex(ds::$m::BG_ELEV)),
                ("hover", hex(ds::$m::HOVER)),
                ("sel-bg", hex(ds::$m::SEL_BG)),
                ("sel-text", hex(ds::$m::SEL_TEXT)),
                ("accent", hex(ds::$m::ACCENT)),
                ("accent-hover", hex(ds::$m::ACCENT_HOVER)),
                ("btn2", hex(ds::$m::BTN2)),
                ("btn2-hover", hex(ds::$m::BTN2_HOVER)),
                ("b2", hex(ds::$m::B2)),
                ("b3", hex(ds::$m::B3)),
                ("t1", hex(ds::$m::T1)),
                ("t2", hex(ds::$m::T2)),
                ("t3", hex(ds::$m::T3)),
                ("info-dot", hex(ds::$m::INFO_DOT)),
                ("warn-dot", hex(ds::$m::WARN_DOT)),
                ("crit", hex(ds::$m::CRIT)),
                ("crit-dot", hex(ds::$m::CRIT_DOT)),
                ("ok-dot", hex(ds::$m::OK_DOT)),
                ("tag-blue", hex(ds::tag_dots::$t::BLUE)),
                ("tag-green", hex(ds::tag_dots::$t::GREEN)),
                ("tag-gray", hex(ds::tag_dots::$t::GRAY)),
            ]
        };
    }
    let set = if light { set!(light, light) } else { set!(dark, dark) };
    // The ink of a space's pill, read off its fill (`ink_on`).
    let inks: String = set
        .iter()
        .filter(|(k, _)| matches!(*k, "tag-blue" | "tag-green"))
        .map(|(k, v)| format!("--{k}-ink:{};", ink_on(v)))
        .collect();
    set.iter().map(|(k, v)| format!("--{k}:{v};")).collect::<String>() + &inks
}

/// The page's colours switched in place (`theme::restyle_script`).
pub fn restyle_script(light: bool) -> String {
    crate::theme::restyle_script(light, &vars(light))
}

/// The page, loaded once per open; `update_script` fills it.
/// The chat column in the page's frame (x, y up, width, height, in points):
/// the left third, under its `top` bar, its right border left to the page.
/// The side panel keeps the window's right.
pub fn chat_frame(page: (f64, f64, f64, f64), top: f64) -> (f64, f64, f64, f64) {
    let (x, y, w, h) = page;
    let col = (w / 3.0).round();
    (x, y, (col - 1.0).max(0.0), (h - top).max(0.0))
}

pub fn shell(light: bool, nonce: &str) -> String {
    PAGE.replace("{scheme}", if light { "light" } else { "dark" })
        .replace("{card_css}", CARD_CSS)
        .replace("{snooze_html}", SNOOZE_HTML)
        .replace("{snooze_js}", SNOOZE_JS)
        .replace("{vars}", &vars(light))
        .replace("{nonce}", nonce)
        .replace("{handler}", HANDLER)
}

/// The card's look, shared by the Cockpit and the chat's ask cards: the
/// design system's variables (`vars`) must be in scope where it applies.
pub const CARD_CSS: &str = r#".rt{color:var(--t3);font-weight:400}
.card a,.clk a,.ag a,.cu a{color:var(--accent);text-decoration:none;overflow-wrap:anywhere}.card a:hover,.clk a:hover,.ag a:hover,.cu a:hover{text-decoration:underline}
svg.i{fill:none;stroke:currentColor;stroke-width:2;stroke-linecap:round;stroke-linejoin:round;flex:none}
button{font:inherit;color:inherit;border:0;background:none;cursor:pointer}
button:disabled{opacity:.5;cursor:default}
.btn{height:28px;border-radius:6px;padding:0 12px;font-size:13px;font-weight:500;display:inline-flex;align-items:center;gap:6px;white-space:nowrap;color:var(--t1)}
.btn.acc{background:var(--accent);color:#fff}
.btn.acc:hover:not(:disabled){background:var(--accent-hover)}
.btn.sec{background:var(--btn2)}
.btn.sec:hover:not(:disabled){background:var(--btn2-hover)}
.card{background:var(--bg1);border:1px solid var(--b3);border-radius:8px;padding:10px 14px 10px 16px;margin-bottom:6px}
.card.urgent{border-color:color-mix(in srgb,var(--crit-dot) 45%,transparent)}
.card .row{display:flex;align-items:center;gap:16px}
.card .txt{flex:1;min-width:0}
.card .t{font-size:15px;line-height:20px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.card .need{font-size:13px;line-height:18px;margin-top:3px;display:flex;align-items:flex-start;gap:6px}
.card .need svg{color:var(--info-dot);margin-top:2px}
.card .need span{min-width:0;overflow-wrap:anywhere}
.src{font-size:12px;line-height:16px;color:var(--t3);margin-top:6px;display:flex;align-items:center;gap:8px;min-width:0}
.src span:last-child{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.tag{height:18px;border-radius:4px;padding:0 6px;font-size:11px;font-weight:500;display:inline-flex;align-items:center;gap:5px;background:var(--bg-elev);color:var(--t2);flex:none}
.tag i{width:6px;height:6px;border-radius:50%;display:block;background:var(--tag-blue)}
.tag i.perso{background:var(--tag-green)}.tag.sp{font-weight:600;padding:0 8px;border-radius:9px;background:var(--tag-blue);color:var(--tag-blue-ink)}.tag.sp.perso{background:var(--tag-green);color:var(--tag-green-ink)}.tag.lb{font-weight:600;padding:0 8px;border-radius:9px}.fm-o i.ld{width:7px;height:7px;border-radius:50%;display:inline-block;margin-right:7px;vertical-align:1px}
.acts{display:flex;align-items:center;gap:14px;flex:none}
.state{font-size:12px;color:var(--t3);margin-top:6px}
.detail{margin-top:10px;border-top:1px solid var(--b3);padding-top:10px;font-size:13px;line-height:19px}
.kv{display:grid;grid-template-columns:max-content 1fr;gap:4px 14px;margin:0}
.kv dt{color:var(--t3)}.kv dd{margin:0;min-width:0;overflow-wrap:anywhere}.kv dd.crit{color:var(--crit)}
pre.scroll{margin-top:8px;max-height:240px;overflow:auto;background:var(--bg-input);border:1px solid var(--b3);border-radius:6px;padding:8px 12px;font:12px/17px ui-monospace,SFMono-Regular,Menlo,monospace;white-space:pre-wrap;overflow-wrap:anywhere}
.dacts{display:flex;flex-wrap:wrap;align-items:center;gap:6px 14px;margin-top:10px}
.link{color:var(--accent);font-size:13px;font-weight:500;padding:0}
.link:hover{text-decoration:underline}
.words{color:var(--crit)}.msg{white-space:pre-wrap;overflow-wrap:anywhere}
textarea.ans{display:block;width:100%;margin-top:8px;background:var(--bg-input);border:1px solid var(--b2);border-radius:6px;color:var(--t1);font:inherit;font-size:13px;padding:6px 10px;resize:vertical}
textarea.ans:focus{border-color:var(--accent);outline:0}
.draft .hd code.tg{font:11.5px "SF Mono",Menlo,monospace;color:var(--t3);background:none;border:0;padding:0}
.draft .hd span.tg{color:var(--t3)}.draft .hd span.tg.ext{color:var(--warn-dot)}
.rz .rz-a,.rz .rz-b{display:flex;align-items:center;justify-content:flex-end;gap:12px;margin-top:8px}
.rz .undo{display:block;margin:2px 0 8px;font-size:12px;font-weight:400;color:var(--t3);text-decoration:underline}
.icons .vu{display:inline-flex;align-items:center;gap:5px;font-size:12.5px;margin-left:4px}
.rz .hint{margin-right:auto;font-size:11.5px;color:var(--t3)}
.rz .btn{display:inline-flex;align-items:center;gap:6px}.rz .link{display:inline-flex;align-items:center;gap:6px}
.rz .btn.out{background:none;border:1px solid var(--b2);color:var(--t1);height:28px;padding:0 12px;border-radius:6px;font-size:13px;font-weight:500;cursor:pointer}
.rz .seg{display:inline-flex;border:1px solid var(--b2);border-radius:7px;overflow:hidden;margin-right:auto}
.rz .seg button{display:inline-flex;align-items:center;gap:6px;height:28px;padding:0 12px;background:none;border:0;border-left:1px solid var(--b2);color:var(--t2);font-size:13px;cursor:pointer}
.rz .seg button:first-child{border-left:0}.rz .seg button.on{background:var(--bg-elev);color:var(--t1);font-weight:600}
.state.top{margin:0 0 6px}.jr{margin:18px 0 6px;font-size:12px;color:var(--t3)}.jr a{color:var(--t3);text-decoration:underline}
.state.ok{color:var(--ok-dot)}.state.warn{color:var(--warn-dot)}.state.crit{color:var(--crit)}
.instr{margin-top:10px;border-top:1px solid var(--b3);padding-top:4px;cursor:auto}
textarea.ins{display:block;width:100%;margin-top:4px;background:var(--bg-input);border:1px solid var(--b2);border-radius:6px;color:var(--t1);font:inherit;font-size:13px;line-height:19px;padding:6px 10px;resize:vertical}
textarea.ins:focus{border-color:var(--accent);outline:0}
.msg.sent{margin-top:4px;font-size:12px;color:var(--t2)}
div.why{margin-top:8px;cursor:auto}div.why[hidden]{display:none}
input.why{display:block;width:100%;height:28px;background:var(--bg-input);border:1px solid var(--b2);border-radius:6px;color:var(--t1);font:inherit;font-size:13px;padding:0 10px}
input.why:focus{border-color:var(--accent);outline:0}
.card{cursor:pointer}.card textarea,.card .detail{cursor:auto}
.shots{display:grid;grid-template-columns:repeat(auto-fit,minmax(300px,1fr));gap:10px;margin:10px 0 4px}
.shot{margin:0;min-width:0}
.shot figcaption{font-size:11px;color:var(--t3);margin-bottom:4px}
.shot img{display:block;width:100%;height:auto;border:1px solid var(--b3);border-radius:6px;cursor:zoom-in;background:var(--bg2)}
#zoom{position:fixed;inset:0;z-index:30;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:10px;padding:24px;background:rgba(0,0,0,.85);cursor:zoom-out}
#zoom[hidden]{display:none}
#zoom img{max-width:100%;max-height:calc(100vh - 70px);object-fit:contain;border-radius:6px}
#zoom span{color:#fff;font-size:13px}
.side{display:flex;flex-direction:column;align-items:flex-end;gap:6px;flex:none}
.icons{display:flex;gap:2px;flex:none}
.ico{width:24px;height:24px;border-radius:6px;display:flex;align-items:center;justify-content:center;color:var(--t3)}
.ico:hover:not(:disabled){background:var(--hover);color:var(--t1)}
.msg.dim{color:var(--t3)}.msg+.msg{margin-top:8px}.need-t{font-weight:500}
.lbl{margin-top:10px;font-size:12px;color:var(--t3)}
.srcs{margin:4px 0 0 18px;font-size:12px;color:var(--t2);overflow-wrap:anywhere}
.links{display:flex;flex-direction:column;align-items:flex-start;gap:4px;margin-top:10px}
.links .link{text-align:left;overflow-wrap:anywhere}
.snz{position:fixed;z-index:10;background:var(--bg-elev);border:1px solid var(--b2);border-radius:8px;padding:4px;min-width:180px;box-shadow:0 8px 24px rgba(0,0,0,.25);display:flex;flex-direction:column}
.snz[hidden]{display:none}
.snz button{height:28px;padding:0 10px;border-radius:6px;text-align:left;font-size:13px}
.snz button:hover{background:var(--hover)}
.snz .pick{display:flex;gap:6px;padding:4px}
.snz input{flex:1;min-width:0;background:var(--bg-input);border:1px solid var(--b2);border-radius:6px;color:var(--t1);font:inherit;font-size:12px;padding:3px 6px}"#;

/// The snooze menu of a card's clock icon, shared with the chat: fixed, so
/// it sits outside the list and a push never closes it.
pub const SNOOZE_HTML: &str = r#"<div class="snz" id="snz" hidden><button type="button" data-when="hour">Dans 1 h</button><button type="button" data-when="evening">Ce soir</button><button type="button" data-when="morning">Demain matin</button><button type="button" data-when="monday">Lundi prochain</button><button type="button" data-when="pick">Choisir…</button><div class="pick" id="snzPick" hidden><input type="datetime-local" id="snzAt"><button type="button" class="btn acc" data-when="at"><span>OK</span></button></div></div>"#;

/// The menu's dates: `when(word)` and `iso(date)`, the text a snooze posts
/// (`--until=<ISO 8601>`). Needs `$` (getElementById) in scope.
pub const SNOOZE_JS: &str = r#"function pad(n) { return String(n).padStart(2, '0'); }
function iso(d) {
  const off = -d.getTimezoneOffset(), a = Math.abs(off);
  return d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()) + 'T' + pad(d.getHours()) + ':' +
    pad(d.getMinutes()) + ':00' + (off < 0 ? '-' : '+') + pad(Math.floor(a / 60)) + ':' + pad(a % 60);
}
function when(word) {
  const d = new Date();
  if (word === 'hour') return new Date(d.getTime() + 3600e3);
  const at = (days, h) => { const x = new Date(d); x.setDate(x.getDate() + days); x.setHours(h, 0, 0, 0); return x; };
  if (word === 'evening') return d.getHours() < 19 ? at(0, 19) : at(1, 19);
  if (word === 'morning') return at(1, 9);
  if (word === 'monday') return at(((8 - d.getDay()) % 7) || 7, 9);
  if (word === 'at') { const v = $('snzAt').value; return v ? new Date(v) : null; }
  return null;
}"#;

const PAGE: &str = r#"<!doctype html><html lang="fr"><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:; script-src 'nonce-{nonce}'">
<title>Cockpit</title>
<style>
:root{color-scheme:{scheme};{vars}}
*{box-sizing:border-box;margin:0;padding:0}
html,body{height:100%;background:var(--bg0)}
body{font-family:Inter,"SF Pro Text",-apple-system,system-ui,sans-serif;color:var(--t1);-webkit-font-smoothing:antialiased;display:flex;overflow:hidden}
{card_css}
:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
.page{flex:1;min-width:0;overflow-y:auto}
.inner{max-width:880px;margin:0 auto;padding:26px 24px 40px}
body.chat-on .inner{max-width:none;margin:0 32px;padding-left:0;padding-right:0}
.phead{display:flex;align-items:flex-start;justify-content:space-between;gap:16px}
.ptitle{font-size:28px;line-height:34px;font-weight:600;letter-spacing:-.2px}
.psub{font-size:13px;color:var(--t3);margin-top:4px}
.psub b{color:var(--crit);font-weight:500}
.btn.on{background:var(--sel-bg);color:var(--sel-text)}
.bar{display:flex;align-items:center;justify-content:space-between;margin:16px 0 4px}
.seg{display:inline-flex;background:var(--bg2);border-radius:8px;padding:2px;gap:2px}
.seg button{height:26px;padding:0 14px;border-radius:6px;font-size:12px;font-weight:500;color:var(--t3);display:flex;align-items:center;gap:6px}
.seg button.on{background:var(--hover);color:var(--t1)}
.seg .c{font-size:11px;color:var(--t3);font-weight:400}
.sortlbl{font-size:12px;color:var(--t3);display:flex;align-items:center;gap:5px}
.sec-h{display:flex;align-items:center;gap:8px;height:24px;margin:12px 0 6px;font-size:12px;font-weight:600;color:var(--t2)}
.sec-h .d{width:8px;height:8px;border-radius:50%}
.d.crit{background:var(--crit-dot)}.d.warn{background:var(--warn-dot)}.d.gray{background:var(--tag-gray)}
.sec-h .k{color:var(--t3);font-weight:400}
.empty{color:var(--t3);font-size:13px;margin-top:24px}
.filters{display:flex;align-items:center;gap:6px;min-height:26px;flex-wrap:wrap}
.flt{height:24px;padding:0 6px;border-radius:6px;font-size:12px;color:var(--t3);display:inline-flex;align-items:center;gap:5px}
.flt:hover,.chip button:hover{background:var(--hover);color:var(--t1)}
.chip{display:inline-flex;align-items:center;height:24px;border:1px solid var(--b2);border-radius:6px;background:var(--bg1);font-size:12px;color:var(--t1);overflow:hidden}
.chip .cb{height:100%;padding:0 4px 0 8px;font-size:12px;color:var(--t1)}
.chip .ck{color:var(--t3)}
.chip .cx{height:100%;width:22px;display:flex;align-items:center;justify-content:center;color:var(--t3);border-left:1px solid var(--b3)}
.fm{min-width:200px}
.fm .fm-h{font-size:11px;font-weight:600;color:var(--t3);padding:6px 10px 2px}
.fm .fm-h:not(:first-child){margin-top:4px;border-top:1px solid var(--b3);padding-top:8px}
.fm .fm-o{display:flex;align-items:center;gap:8px}
.fm .fm-o .ok{width:13px;display:flex;color:var(--accent);visibility:hidden}
.fm .fm-o.on .ok{visibility:visible}
.fm .fm-o .c{margin-left:auto;font-size:11px;color:var(--t3)}
.d.info{background:var(--info-dot)}.d.ok{background:var(--ok-dot)}
.trk{display:flex;align-items:center;flex:none}
.st{display:flex;align-items:center;gap:6px;font-size:12px;color:var(--t3);white-space:nowrap}
.st i{width:8px;height:8px;border-radius:50%;display:block;border:1.5px solid var(--t3);box-sizing:border-box}
.st.ok{color:var(--t2)}.st.ok i{background:var(--ok-dot);border-color:var(--ok-dot)}
.st.run{color:var(--t1)}.st.run i{border-color:var(--info-dot);background:color-mix(in srgb,var(--info-dot) 35%,transparent)}
.st.warn{color:var(--warn-dot)}.st.warn i{background:var(--warn-dot);border-color:var(--warn-dot)}
.st em{font-style:normal;color:var(--t3);font-size:11px}
.sep{width:22px;height:1px;background:var(--b2);margin:0 8px}.sep.ok{background:var(--ok-dot)}
.clk{background:var(--bg1);border:1px solid var(--b3);border-radius:8px;padding:10px 14px 10px 16px;margin-bottom:6px}
.clk.warn{border-color:color-mix(in srgb,var(--warn-dot) 50%,transparent)}
.clk .row{display:flex;align-items:center;gap:16px}
.clk .txt{flex:1;min-width:0}
.clk .t{font-size:14px;line-height:19px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.clk .q{font-size:13px;line-height:18px;color:var(--t2);margin-top:3px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.clk .res{font-size:12px;color:var(--t3);margin-top:6px;display:flex;gap:8px;align-items:center;min-width:0}
.clk .res b{color:var(--t2);font-weight:500}
.clk .res .crit,.clk .res .crit b{color:var(--crit)}.clk .res .warn,.clk .res .warn b{color:var(--warn-dot)}.clk .res .ok b{color:var(--ok-dot)}.clk .res .dim,.clk .res .dim b{color:var(--t3)}
.st.crit{color:var(--crit)}.st.crit i{background:var(--crit);border-color:var(--crit)}
.verb{height:18px;border-radius:4px;padding:0 6px;font-size:11px;font-weight:600;display:inline-flex;align-items:center;background:var(--sel-bg);color:var(--sel-text);margin-right:8px;vertical-align:1px}
.verb.c{background:var(--bg-elev);color:var(--t1)}
.box{background:var(--bg1);border:1px solid var(--b3);border-radius:8px;padding:2px 0}
.ag{display:grid;grid-template-columns:200px 1fr auto;gap:14px;align-items:baseline;padding:9px 14px 9px 16px;border-top:1px solid var(--b3);font-size:13px;line-height:18px}
.ag:first-child{border-top:0}
.ag .p{font-weight:600;display:flex;align-items:center;gap:8px;white-space:nowrap;min-width:0}
.ag .p i{width:8px;height:8px;border-radius:50%;display:block;flex:none}
.ag .k{font-size:12px;color:var(--t3);margin-top:3px}.ag .k b{color:var(--t2);font-weight:500}
.ag .m{font-size:12px;color:var(--info-dot);white-space:nowrap}
.tk{padding:9px 14px 9px 16px;border-top:1px solid var(--b3);font-size:13px;line-height:18px;min-width:0}
.tk:first-child{border-top:0}.tk .t{font-weight:500;overflow-wrap:anywhere}
.tk .res{font-size:12px;color:var(--t3);margin-top:5px;display:flex;flex-wrap:wrap;gap:4px 10px;align-items:center;min-width:0}
.tk .res b{color:var(--t2);font-weight:500}.tk .res span{min-width:0;overflow-wrap:anywhere}.tk .dim{color:var(--t3)}
.tk .w{font-weight:600;color:var(--t2);white-space:nowrap}.tk .w.a_faire{color:var(--info-dot)}.tk .w.attend_robin{color:var(--warn-dot)}.tk .w.fait{color:var(--ok-dot)}
.tk a{color:var(--accent);text-decoration:none;overflow-wrap:anywhere}.tk a:hover{text-decoration:underline}
details.tks>summary{cursor:pointer;list-style:none}details.tks>summary::-webkit-details-marker{display:none}
details.tks>summary::after{content:"Afficher";font-weight:400;color:var(--accent);margin-left:6px}details.tks[open]>summary::after{content:"Masquer"}
.fold{font-size:12px;color:var(--t3);padding:9px 16px;border-top:1px solid var(--b3)}
.box .fold:first-child{border-top:0}
.draft{margin-top:10px;background:var(--bg-input);border:1px solid var(--b2);border-radius:6px}
.draft:focus-within{border-color:var(--accent)}
.draft .hd{display:flex;align-items:center;gap:14px;padding:8px 12px;border-bottom:1px solid var(--b3);font-size:12px;color:var(--t3);flex-wrap:wrap}
.draft .hd b{font-weight:500;color:var(--t1)}
.draft .hd .edit{margin-left:auto;display:flex;align-items:center;gap:5px}
textarea.bd{display:block;width:100%;resize:none;border:0;outline:0;background:transparent;color:var(--t1);font:inherit;font-size:13px;line-height:19px;padding:8px 12px;overflow:hidden;min-height:35px}
.draft-foot{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-top:8px}
.draft-foot .src{margin:0}
.fold{margin-top:8px}.fold summary{cursor:pointer;color:var(--t2);font-size:12px}
.more{margin:8px 0 0}
.chat{width:33.333%;flex:none;border-right:1px solid var(--b3);background:var(--bg1);display:none;flex-direction:column}
body.chat-on .chat{display:flex}
.chat .top{height:56px;flex:none;display:flex;align-items:center;justify-content:space-between;gap:8px;padding:0 14px 0 16px;border-bottom:1px solid var(--b3)}
.chat .who{font-size:11px;color:var(--t3);display:flex;align-items:center;gap:6px;white-space:nowrap;overflow:hidden}
.chat .who i{width:8px;height:8px;border-radius:50%;background:var(--ok-dot);display:block;flex:none}
.chat .who.off i{background:var(--tag-gray)}
.chat .x{color:var(--t3);width:26px;height:26px;display:flex;align-items:center;justify-content:center;border-radius:6px}
.chat .x:hover{background:var(--hover)}
.cbody{flex:1;padding:20px}
.hbtns{display:flex;gap:8px;flex:none}
.btn.ico{width:28px;padding:0;justify-content:center}
.btn.rec.on{background:color-mix(in srgb,var(--crit-dot) 22%,transparent);color:var(--crit)}
.btn.rec.on svg{fill:currentColor}
.btn.rec .tm{font-variant-numeric:tabular-nums;font-weight:400}
.recn{margin-top:16px;display:flex;align-items:flex-start;gap:10px;background:var(--bg1);border:1px solid var(--b3);border-radius:8px;padding:10px 10px 10px 16px;font-size:13px;line-height:19px}
.recn.ok{border-color:color-mix(in srgb,var(--ok-dot) 45%,transparent)}
.recn.crit{border-color:color-mix(in srgb,var(--crit-dot) 45%,transparent)}
.recn.crit .rt{color:var(--crit)}
.recn .rb{flex:1;min-width:0;display:flex;flex-direction:column;align-items:flex-start;gap:2px}
.recn .rt{font-weight:500}.recn .link{font-size:12px;text-align:left;overflow-wrap:anywhere}.recn .dim{font-size:12px;color:var(--t3)}
.cu{margin-top:16px;background:var(--bg1);border:1px solid var(--b3);border-radius:8px;padding:4px 0 6px}
.cu-h{display:flex;align-items:center;gap:8px;padding:6px 10px 6px 16px;font-size:13px;font-weight:600}
.cu-h svg{color:var(--info-dot)}
.cu-h .k{color:var(--t3);font-weight:400;font-size:12px}
.cu-h .r{margin-left:auto;display:flex;align-items:center;gap:4px}
.cu .ln{display:grid;grid-template-columns:minmax(0,150px) minmax(0,1fr) auto;gap:4px 12px;align-items:baseline;padding:7px 14px 7px 16px;border-top:1px solid var(--b3);font-size:13px;line-height:18px;cursor:pointer}
.cu .ln:hover{background:var(--hover)}.cu .ln.open,.cu .ln.open:hover{background:var(--bg2)}
.cu .p{font-weight:600;display:flex;align-items:center;gap:8px;min-width:0}
.cu .p span{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.cu .p i{width:6px;height:6px;border-radius:50%;background:var(--info-dot);display:block;flex:none}
.cu .m{min-width:0;overflow-wrap:anywhere}
.cu .w{color:var(--t3);font-size:12px;margin-top:2px}
.cu .w b{color:var(--warn-dot);font-weight:500}
.cu .ses{font-size:11px;color:var(--t3);display:flex;align-items:center;gap:4px;white-space:nowrap}
.cu .det{grid-column:2 / 4;border-top:1px solid var(--b3);padding-top:4px;margin-top:6px;cursor:auto}
.cu .det .lbl{margin-top:6px}
.chg{list-style:none;margin:4px 0 0;font-size:12px;color:var(--t2)}
.chg li{display:flex;flex-wrap:wrap;align-items:baseline;gap:4px 8px;padding:2px 0;overflow-wrap:anywhere}
.chg .tm{color:var(--t3);font-variant-numeric:tabular-nums}
.chg .dim{color:var(--t3)}
.chg .lks{display:inline-flex;flex-wrap:wrap;gap:4px 10px}
.chg .link{font-size:12px}
.cu-more{padding:6px 16px 2px;border-top:1px solid var(--b3)}
.card.flash{border-color:var(--accent);box-shadow:0 0 0 2px color-mix(in srgb,var(--accent) 35%,transparent);transition:box-shadow .3s}
.none{font-size:13px;color:var(--t3)}
</style></head><body>
<section class="chat" id="chat"></section>
<section class="page"><div class="inner"><div id="head"></div><div id="list"></div></div></section>
{snooze_html}
<div class="snz fm" id="fm" hidden></div>
<div id="zoom" hidden><img alt=""><span></span></div>
<script nonce="{nonce}">
const $ = id => document.getElementById(id);
const load = '{nonce}';
function post(msg) {
  try { window.webkit.messageHandlers.{handler}.postMessage(JSON.stringify(Object.assign({ load }, msg))); } catch (e) {}
}
function grow(t) { t.style.height = 'auto'; t.style.height = t.scrollHeight + 'px'; }
function edited(t) { return t.value !== t.dataset.orig; }
// A draft's buttons follow its text: once rewritten, Valider sends the
// correction and Corriger puts the draft back.
function sync(card) {
  const t = card.querySelector('textarea.bd');
  if (!t) return;
  const v = card.querySelector('[data-act=validate] span');
  const c = card.querySelector('[data-act=correct]');
  const changed = edited(t);
  if (v) v.textContent = changed ? 'Valider la correction' : 'Valider';
  if (c) c.textContent = changed ? 'Annuler' : 'Corriger';
}
// The list is replaced whole; a draft being rewritten keeps its text, its
// caret and the keyboard.
function setList(html) {
  const keep = {};
  document.querySelectorAll('textarea.bd').forEach(t => {
    if (edited(t)) keep[t.closest('.card').dataset.key] = { orig: t.dataset.orig, value: t.value };
  });
  const a = document.activeElement;
  const focusKey = a && a.classList && a.classList.contains('bd') ? a.closest('.card').dataset.key : null;
  const sel = focusKey ? [a.selectionStart, a.selectionEnd] : null;
  const top = document.querySelector('.page').scrollTop;
  // An opened card's own state: its folds, its scrolled boxes, a typed answer.
  const inner = {};
  document.querySelectorAll('.card').forEach(card => {
    inner[card.dataset.key] = {
      open: [...card.querySelectorAll('details')].map(d => d.open),
      scroll: [...card.querySelectorAll('pre.scroll')].map(p => p.scrollTop),
      ans: (card.querySelector('textarea.ans') || {}).value || '',
      ins: (card.querySelector('textarea.ins') || {}).value || '',
      why: card.querySelector('div.why:not([hidden]) input.why')
    };
  });
  const ansFocus = a && a.classList && a.classList.contains('ans') ? a.closest('.card').dataset.key : null;
  const insFocus = a && a.classList && a.classList.contains('ins') ? a.closest('.card').dataset.key : null;
  const insSel = insFocus ? [a.selectionStart, a.selectionEnd] : null;
  const whyFocus = a && a.classList && a.classList.contains('why') ? a.closest('.card').dataset.key : null;
  $('list').innerHTML = html;
  document.querySelectorAll('.card').forEach(card => {
    const t = card.querySelector('textarea.bd');
    if (!t) return;
    const k = keep[card.dataset.key];
    // A correction already sent keeps showing the text that went.
    if (k && k.orig === t.dataset.orig) t.value = k.value;
    grow(t); sync(card);
    if (card.dataset.key === focusKey && !t.readOnly) { t.focus(); t.setSelectionRange(sel[0], sel[1]); }
  });
  document.querySelectorAll('.card').forEach(card => {
    const k = inner[card.dataset.key];
    if (!k) return;
    card.querySelectorAll('details').forEach((d, i) => { if (k.open[i]) d.open = true; });
    card.querySelectorAll('pre.scroll').forEach((p, i) => { if (k.scroll[i]) p.scrollTop = k.scroll[i]; });
    const ans = card.querySelector('textarea.ans');
    if (ans) { ans.value = k.ans; if (card.dataset.key === ansFocus) ans.focus(); }
    const ins = card.querySelector('textarea.ins');
    if (ins) {
      ins.value = k.ins;
      if (card.dataset.key === insFocus) { ins.focus(); ins.setSelectionRange(insSel[0], insSel[1]); }
    }
    // A reason being typed for Refuser stays open with its text.
    const why = card.querySelector('input.why');
    if (why && k.why) {
      why.value = k.why.value; why.parentNode.hidden = false;
      if (card.dataset.key === whyFocus) why.focus();
    }
  });
  document.querySelector('.page').scrollTop = top;
  if (wantFocus) {
    const c = document.querySelector('.card.open[data-key="' + CSS.escape(wantFocus.key) + '"]');
    if (c) { focusIn(c, wantFocus.what); wantFocus = null; }
  }
}
// `Brief`, `Décider`, `Tester`, `Répondre` open the card, then give the
// keyboard to where the user answers, once the opened card is in.
let wantFocus = null;
function focusIn(card, what) {
  const el = card.querySelector(what === 'ans' ? 'textarea.ans, .dacts button' : 'textarea.ins');
  if (el) { el.focus(); el.scrollIntoView({ block: 'nearest' }); }
}
// A push replaces its part whole, which would drop a selection being made
// or about to be copied: while text is selected in a part, its push waits
// until the selection goes.
const held = {};
function selectedIn(id) {
  const sel = window.getSelection();
  return document.hasFocus() && sel && !sel.isCollapsed && String(sel).trim() !== '' && $(id).contains(sel.anchorNode);
}
function flush(force) {
  for (const id of Object.keys(held)) if (force || !selectedIn(id)) { const h = held[id]; delete held[id]; put(id, h); }
}
function put(id, html) {
  if (id === 'list') setList(html); else $(id).innerHTML = html;
}
function apply(id, html) {
  if (selectedIn(id)) { held[id] = html; return; }
  delete held[id];
  put(id, html);
}
document.addEventListener('selectionchange', () => flush(false));
// The page losing the keyboard (hidden, another view) lets every push in.
window.addEventListener('blur', () => flush(true));
window.kova = {
  update(u) {
    if (u.head !== undefined) { apply('head', u.head); fmFill(); }
    if (u.list !== undefined) {
      // A folded section the user opened (done, dropped) stays open.
      const open = [...document.querySelectorAll('details[data-tk][open]')].map(d => d.dataset.tk);
      apply('list', u.list);
      open.forEach(k => { const d = document.querySelector('details[data-tk="' + k + '"]'); if (d) d.open = true; });
    }
    if (u.chat !== undefined) { apply('chat', u.chat); document.body.classList.toggle('chat-on', u.chat !== ''); }
    if (u.focus && !(document.activeElement && /^(TEXTAREA|INPUT)$/.test(document.activeElement.tagName))) window.focus();
    if (u.reveal) {
      // The card to light must be in the list, held or not.
      if (held.list !== undefined) flush(true);
      const c = document.querySelector('.card[data-key="' + CSS.escape(u.reveal) + '"]');
      if (c) {
        c.scrollIntoView({ block: 'center', behavior: 'smooth' });
        c.classList.add('flash');
        setTimeout(() => c.classList.remove('flash'), 1600);
      }
    }
  }
};
// The snooze menu lives outside the list, so a push never closes it.
let snzKey = null;
{snooze_js}
function snzClose() { $('snz').hidden = true; $('snzPick').hidden = true; snzKey = null; }
// The filter menu, outside the head like the snooze one: a push refills it open.
function fmFill() { const m = $('fm'), src = $('fmsrc'); if (!m.hidden && src) m.innerHTML = src.innerHTML; }
function fmOpen(b) {
  const m = $('fm'), src = $('fmsrc');
  if (!src) return;
  m.innerHTML = src.innerHTML;
  m.hidden = false;
  const r = b.getBoundingClientRect();
  m.style.top = (r.bottom + 4) + 'px';
  m.style.left = Math.max(8, Math.min(r.left, window.innerWidth - m.offsetWidth - 8)) + 'px';
}
function hide(key) { const c = document.querySelector('.card[data-key="' + CSS.escape(key) + '"]'); if (c) c.style.display = 'none'; }
document.addEventListener('click', e => {
  // A link inside a text (a card, En cours, Fait, the band): the side
  // panel, never this page, and not the card or the row under it.
  const a = e.target.closest('a');
  if (a) {
    e.preventDefault();
    const href = a.dataset.path || a.getAttribute('href');
    if (href && href !== '#') post({ type: 'open', href });
    return;
  }
  // A card's screenshot opens full size over the page; a click closes it.
  const z = $('zoom');
  if (!z.hidden) { z.hidden = true; return; }
  const shot = e.target.closest('img[data-zoom]');
  if (shot) {
    e.stopPropagation();
    z.querySelector('img').src = shot.src;
    z.querySelector('span').textContent = shot.dataset.zoom;
    z.hidden = false;
    return;
  }
  const s = e.target.closest('#snz');
  if (s) {
    const b = e.target.closest('button');
    if (!b || !snzKey) return;
    if (b.dataset.when === 'pick') { $('snzPick').hidden = false; $('snzAt').focus(); return; }
    const d = when(b.dataset.when);
    if (!d || isNaN(d) || d <= new Date()) return;
    const key = snzKey;
    snzClose(); hide(key);
    post({ type: 'act', key, act: 'snooze', text: iso(d) });
    return;
  }
  if (!$('snz').hidden) snzClose();
  const fm = $('fm'), inMenu = e.target.closest('#fm');
  if (!fm.hidden && !inMenu) fm.hidden = true;
  const b = e.target.closest('button');
  if (b && b.dataset.fopen) { if (fm.hidden) fmOpen(b); else fm.hidden = true; return; }
  if (inMenu) {
    if (!b) return;
    fm.hidden = true;
    if (b.dataset.filter) return post({ type: 'filter', scope: b.dataset.filter });
    if (b.dataset.show) return post({ type: 'show', show: b.dataset.show });
    if (b.dataset.label) return post({ type: 'label', label: b.dataset.label });
    return;
  }
  // The catch-up band: its buttons, its links, a row that opens in place.
  if (b && b.dataset.rec) return post({ type: 'record', what: b.dataset.rec });
  if (b && b.dataset.band) return post({ type: 'band', what: b.dataset.band });
  if (b && b.dataset.goto) return post({ type: 'reveal', key: b.dataset.goto });
  const ln = e.target.closest('.cu .ln');
  if (b && b.dataset.blink && ln) return post({ type: 'band', what: 'link', proj: ln.dataset.proj, link: b.dataset.blink });
  if (ln && !b) {
    if (e.target.closest('.det') || String(window.getSelection()).trim()) return;
    return post({ type: 'band', what: 'row', proj: ln.dataset.proj });
  }
  // A click on a card's body, not on a button or a field: it opens or folds.
  if (!b) {
    const card = e.target.closest('.card');
    if (!card || e.target.closest('textarea, input, a, .detail, .instr, .why')) return;
    if (String(window.getSelection()).trim()) return;
    return post({ type: 'act', key: card.dataset.key, act: 'expand' });
  }
  if (b.disabled) return;
  if (b.dataset.more) return post({ type: 'more' });
  if (b.dataset.filter) return post({ type: 'filter', scope: b.dataset.filter });
  if (b.dataset.show) return post({ type: 'show', show: b.dataset.show });
  if (b.dataset.label) return post({ type: 'label', label: b.dataset.label });
  if (b.dataset.chief) return post({ type: 'chief', scope: b.dataset.chief });
  if (b.dataset.chat) return post({ type: 'chat', what: b.dataset.chat });
  if (b.dataset.theme) return post({ type: 'theme' });
  const card = b.closest('.card');
  if (!card || !b.dataset.act) return;
  const t = card.querySelector('textarea.bd');
  let act = b.dataset.act;
  // Corriger, text untouched: the card opens on its instruction field, sent to
  // the card's agent. Once the text is rewritten in place, it reads Annuler
  // and puts the draft back.
  if (act === 'correct' && t && edited(t)) { t.value = t.dataset.orig; grow(t); sync(card); t.focus(); return; }
  if (act === 'correct') act = 'expand';
  const msg = { type: 'act', key: card.dataset.key, act };
  if (b.dataset.arg) msg.arg = b.dataset.arg;
  if (act === 'answer' && b.dataset.text) {
    const ans = card.querySelector('textarea.ans');
    msg.text = ans ? ans.value : '';
    if (!msg.text.trim()) { if (ans) ans.focus(); return; }
  }
  if (act === 'expand' && b.dataset.focus) {
    if (card.classList.contains('open')) return focusIn(card, b.dataset.focus);
    wantFocus = { key: card.dataset.key, what: b.dataset.focus };
    return post(msg);
  }
  if (act === 'expand' || act === 'pane' || act === 'link') return post(msg);
  if (act === 'snooze') {
    snzKey = card.dataset.key;
    const r = b.getBoundingClientRect(), m = $('snz');
    m.hidden = false;
    m.style.top = (r.bottom + 4) + 'px';
    m.style.left = Math.max(8, Math.min(r.right - m.offsetWidth, window.innerWidth - m.offsetWidth - 8)) + 'px';
    const now = new Date(Date.now() + 3600e3);
    $('snzAt').value = iso(now).slice(0, 16);
    return;
  }
  if (act === 'dismiss' || act === 'done') { hide(card.dataset.key); return post(msg); }
  if (act === 'refuse') {
    const w = card.querySelector('div.why');
    if (w) { w.hidden = false; w.querySelector('input').focus(); }
    return;
  }
  if (act === 'validate' && t && edited(t)) { msg.act = 'correct'; msg.text = t.value; }
  card.querySelectorAll('.acts button, .dacts button').forEach(x => x.disabled = true);
  post(msg);
});
document.addEventListener('input', e => {
  const t = e.target;
  if (t.classList && t.classList.contains('bd')) { grow(t); sync(t.closest('.card')); }
});
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !$('zoom').hidden) { $('zoom').hidden = true; return; }
  const t = e.target;
  // An instruction: Enter sends it, Shift+Enter is a new line, nothing typed
  // sends nothing.
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && t.classList && t.classList.contains('ins')) {
    e.preventDefault();
    if (t.readOnly || !t.value.trim()) return;
    post({ type: 'act', key: t.closest('.card').dataset.key, act: 'instruct', text: t.value });
    t.value = '';
    return;
  }
  // Refuser's reason: Enter closes the entry with it (or none), Escape
  // folds the field and keeps the card.
  if (t.classList && t.classList.contains('why') && t.tagName === 'INPUT') {
    if (e.key === 'Enter' && !e.isComposing) {
      e.preventDefault();
      const key = t.closest('.card').dataset.key;
      hide(key);
      post({ type: 'act', key, act: 'refuse', text: t.value });
      return;
    }
    if (e.key === 'Escape') { e.preventDefault(); t.value = ''; t.parentNode.hidden = true; t.blur(); return; }
  }
  if (e.key !== 'Escape') return;
  e.preventDefault();
  if (!$('snz').hidden) return snzClose();
  if (!$('fm').hidden) { $('fm').hidden = true; return; }
  // An open reason field folds before the page closes.
  const why = document.querySelector('.card div.why:not([hidden])');
  if (why) { why.querySelector('input').value = ''; why.hidden = true; return; }
  if (document.activeElement && document.activeElement.tagName === 'TEXTAREA') document.activeElement.blur();
  else post({ type: 'close' });
});
window.addEventListener('resize', () => document.querySelectorAll('textarea.bd').forEach(grow));
post({ type: 'ready' });
</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, level: Level, prio: Option<u8>, since: u64) -> Entry {
        Entry {
            id: id.into(),
            since,
            from: "claap-agent".into(),
            level,
            kind: "decision".into(),
            title: format!("Title {id}"),
            needs: Some("Do it".into()),
            action: None,
            topic: None,
            scope: Scope::Pro,
            prio,
            draft: None,
            body: None,
            detail: None,
            src: Vec::new(),
            before: None,
            after: None,
            label: None,
            failed: None,
            uncertain: None,
            proof: None,
            plan: false,
        }
    }

    #[test]
    fn before_and_after_show_in_the_card_and_zoom_on_a_click() {
        let dir = std::env::temp_dir().join(format!("kova-shots-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let before = dir.join("before.png");
        std::fs::write(&before, b"png!").unwrap();
        let e = Entry {
            before: Some(before.to_string_lossy().into()),
            after: Some(dir.join("missing.png").to_string_lossy().into()),
            ..entry("a-1", Level::Now, None, 1)
        };
        let item = from_attention(&e, None, false).unwrap();
        assert_eq!(item.shots.len(), 2);
        let html = card_html(&item, true, true, true, 10);
        assert!(html.contains(r#"<figcaption>Avant</figcaption><img src="data:image/png;base64,cG5nIQ==" alt="Avant" title="Agrandir" data-zoom="Avant">"#), "{html}");
        assert!(html.contains("<figcaption>Après : fichier introuvable</figcaption>"), "{html}");
        let none = card_html(&from_attention(&entry("a-2", Level::Now, None, 1), None, false).unwrap(), true, true, true, 10);
        assert!(!none.contains("class=\"shots\""));
        let page = shell(false, "n");
        assert!(page.contains(r#"<div id="zoom" hidden>"#) && page.contains("img[data-zoom]"));
        assert!(page.contains("grid-template-columns:repeat(auto-fit,minmax(300px,1fr))"), "side by side when there is room");
        assert!(page.contains("img-src data:;"), "the page's policy lets the embedded images load");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_entry_naming_a_waiting_ask_opens_its_sheet_and_hides_the_ask_card() {
        assert_eq!(ask_ref("kova-sheet:19e0e20f"), Some("19e0e20f"));
        assert_eq!(ask_ref("kova-ask:19e0e20f-aaaa"), Some("19e0e20f-aaaa"));
        assert_eq!(ask_ref("https://x"), None);
        let mut entry_item = from_attention(&entry("a-1", Level::Now, None, 10), None, false).expect("item");
        entry_item.openable = false;
        let mut ask_item = from_need(&need(NeedKind::Ask, "ask:19e0e20f-1111", 5), true).expect("ask");
        ask_item.kind = Kind::Sheet;
        let mut items = vec![entry_item, ask_item];
        let actions = vec![("attention:a-1".to_string(), "kova-sheet:19e0e20f".to_string())];
        link_asks(&mut items, &actions, &["19e0e20f-1111".to_string(), "aaaa0000".to_string()]);
        assert_eq!(items.len(), 1, "the ask's own card is left out");
        assert_eq!(items[0].ask.as_deref(), Some("19e0e20f-1111"));
        assert_eq!(buttons(&items[0], true), [Button::OpenSheet]);
        // An ask that is not waiting any more: the entry keeps its own buttons.
        let mut items = vec![from_attention(&entry("a-1", Level::Now, None, 10), None, false).expect("item")];
        link_asks(&mut items, &actions, &[]);
        assert_eq!(items[0].ask, None);
        // The chat finds the same entry from the ask.
        let mut e = entry("a-1", Level::Now, None, 10);
        e.action = Some("kova-sheet:19e0e20f".into());
        let entries = [entry("a-0", Level::Now, None, 10), e];
        assert_eq!(entry_for_ask(&entries, "19e0e20f-1111", &["19e0e20f-1111", "aaaa0000"]).map(|e| e.id.as_str()), Some("a-1"));
        assert!(entry_for_ask(&entries, "aaaa0000", &["19e0e20f-1111", "aaaa0000"]).is_none());
        assert!(entry_for_ask(&entries, "19e0e20f-1111", &["19e0e20f-1111", "19e0e20f-2222"]).is_none(), "a ref naming two asks links none");
        // The chat's copy of the card CSS stays under its scope.
        let css = scoped_card_css("#asks");
        assert!(css.contains("#asks button{") && css.contains("#asks .card textarea,#asks .card .detail{cursor:auto}"), "{css}");
        assert!(css.lines().all(|l| l.starts_with("#asks ")));
    }

    #[test]
    fn a_failed_routine_shows_its_log_and_opens_in_place() {
        let mut item = from_need(&need(NeedKind::FailedRoutine, "routine:nightly", 5), false).expect("item");
        let closed = card_html(&item, true, true, true, 10);
        assert!(!closed.contains("class=\"detail\""));
        assert!(closed.contains("class=\"btn acc\" data-act=\"routine\" data-arg=\"log\"><span>Voir le log</span>"), "{closed}");
        item.detail = Some(Detail::Routine {
            rows: vec![
                ("Horaire".into(), "daily 21:00".into()),
                ("Prochain".into(), "21:00".into()),
                ("Dernier".into(), "Failed · exit 2".into()),
            ],
            failed: true,
            prompt: "Do <this>".into(),
            acts: vec![RoutineAct::Run, RoutineAct::Log, RoutineAct::Pause, RoutineAct::Edit],
            note: Some("Running now…".into()),
        });
        let open = card_html(&item, true, true, true, 10);
        assert!(open.contains("<dd class=\"crit\">Failed · exit 2</dd>"));
        assert!(open.contains("Do &lt;this&gt;") && open.contains("<details class=\"fold\">"));
        // The other actions are plain links; the log is the blue button only.
        for word in ["run", "pause", "edit"] {
            assert!(open.contains(&format!("class=\"link\" data-act=\"routine\" data-arg=\"{word}\"")), "{word}");
        }
        assert_eq!(open.matches("data-arg=\"log\"").count(), 1, "{open}");
        for word in ["run", "log", "pause", "edit"] {
            assert_eq!(RoutineAct::parse(word).map(|a| a.word()), Some(word));
        }
    }

    #[test]
    fn error_permission_and_question_details_are_escaped_and_answerable() {
        let error = detail_html(&Detail::Error { words: "<limit>".into(), lines: "a\n<b>".into() }, false);
        assert!(error.contains("&lt;limit&gt;") && error.contains("a\n&lt;b&gt;"));
        assert!(!error.contains("data-act=\"pane\""), "the card's blue button goes to the pane");
        let perm = detail_html(&Detail::Permission { text: "Bash command\nrm -rf <x>".into() }, false);
        assert!(perm.contains("rm -rf &lt;x&gt;"));
        let ask = detail_html(
            &Detail::Ask { message: "Ship?".into(), answers: vec![("Oui".into(), false), ("Autre \"x\"".into(), true)], input: false },
            true,
        );
        assert!(ask.contains("data-arg=\"Oui\" disabled"));
        assert!(ask.contains("data-arg=\"Autre &quot;x&quot;\" data-text=\"1\""));
        assert!(ask.contains("textarea class=\"ans\""), "an answer that needs text brings the field");
        let plain = detail_html(&Detail::Ask { message: "Ok?".into(), answers: vec![("Oui".into(), false)], input: false }, false);
        assert!(!plain.contains("textarea"));
    }

    #[test]
    fn the_scope_follows_the_cwd_and_the_pane_shows_its_last_lines() {
        let root = std::path::Path::new("/home/me/Perso");
        assert_eq!(scope_of_cwd("/home/me/Perso", Some(root)), Scope::Perso);
        assert_eq!(scope_of_cwd("/home/me/Perso/trail", Some(root)), Scope::Perso);
        assert_eq!(scope_of_cwd("/home/me/Personal", Some(root)), Scope::Pro, "a component, not a prefix");
        assert_eq!(scope_of_cwd("/home/me/Work", Some(root)), Scope::Pro);
        assert_eq!(scope_of_cwd("/home/me/Perso", None), Scope::Pro);
        assert_eq!(last_lines("a\n\nb  \nc\n\n", 2), "b\nc");
        assert_eq!(last_lines("", 15), "");
    }

    #[test]
    fn an_opened_entry_shows_what_it_says_its_sources_and_its_links() {
        let mut e = entry("a-1", Level::Point, None, 1);
        e.needs = Some("Choisir <vite>".into());
        e.body = Some("Ligne 1\nVoir https://linear.app/x/CLA-1, et (https://a.b/c).".into());
        e.src = vec!["https://a.b/c".into(), "s-42".into()];
        e.action = Some("~/notes/plan.md".into());
        let Detail::Entry { links, .. } = entry_detail(&e, false) else { panic!() };
        assert_eq!(links, ["~/notes/plan.md", "https://a.b/c"], "the action and the sources' links; the body's are in its text");
        let html = detail_html(&entry_detail(&e, false), false);
        assert!(html.contains("Choisir &lt;vite&gt;") && html.contains("Ligne 1\nVoir"));
        assert!(html.contains("<li>s-42</li>"));
        assert!(html.contains("Voir <a href=\"https://linear.app/x/CLA-1\">https://linear.app/x/CLA-1</a>, et (<a href=\"https://a.b/c\">https://a.b/c</a>)."), "{html}");
        assert!(html.contains("data-act=\"link\" data-arg=\"https://a.b/c\" title=\"https://a.b/c\">a.b/c</button>"));
        // The action, a local file, reads as its name.
        assert!(html.contains("data-arg=\"~/notes/plan.md\" title=\"~/notes/plan.md\">plan.md</button>"), "{html}");
        assert!(!html.contains("Je regarde"));
        // An entry that says nothing waits for `attn explain`.
        let bare = entry("a-2", Level::Point, None, 1);
        let bare = Entry { needs: None, ..bare };
        assert!(says_nothing(&bare));
        assert!(detail_html(&entry_detail(&bare, true), false).contains("Je regarde de quoi il s'agit"));
        assert!(detail_html(&entry_detail(&bare, false), false).contains("Rien de plus"));
        // A sheet's action is its button, not a link.
        let sheet = Entry { action: Some("kova-sheet:19e0e20f".into()), ..bare };
        assert!(matches!(entry_detail(&sheet, false), Detail::Entry { links, .. } if links.is_empty()));
    }

    #[test]
    fn a_label_is_a_tinted_tag_on_the_card_and_a_dimension_of_the_filter() {
        let lab = |n: &str, c: &str| Some(crate::attention::Label { name: n.into(), color: c.into() });
        let mut a = from_attention(&entry("a-1", Level::Now, None, 1), None, false).unwrap();
        a.label = lab("QA", "#F59E0B");
        let mut b = from_attention(&entry("a-2", Level::Now, None, 2), None, false).unwrap();
        b.label = lab("Dev", "#3B82F6");
        let mut c = from_attention(&entry("a-3", Level::Now, None, 3), None, false).unwrap();
        c.label = lab("QA", "#F59E0B");
        let bare = from_attention(&entry("a-4", Level::Now, None, 4), None, false).unwrap();
        let items = vec![a, b, c, bare];
        let view = |label: Option<&str>| View {
            items: &items, filter: Filter::All, chat: None, can_act: true, can_close: true, can_snooze: true,
            limit: PAGE_SIZE, now: 100, band: String::new(), record: String::new(), record_note: String::new(),
            theme: String::new(), show: Show::Mine, inflight: String::new(), inflight_n: 0, done_n: 0, journal: None,
            label: label.map(str::to_string),
        };
        let all = list_html(&view(None));
        assert!(all.contains("<span class=\"tag lb\" style=\"background:#F59E0B;color:#111111\">QA</span>"), "a filled pill, its name in the ink that reads on it: {all}");
        assert_eq!((ink_on("#F59E0B"), ink_on("#1E3A8A"), ink_on("#FFFFFF"), ink_on("#000000")), ("#111111", "#FFFFFF", "#111111", "#FFFFFF"));
        // The menu: a Label group, the most used first, each with its dot.
        let head = head_html(&view(None));
        let src = head.split("<div id=\"fmsrc\" hidden>").nth(1).unwrap();
        let (qa, dev) = (src.find("data-label=\"QA\"").unwrap(), src.find("data-label=\"Dev\"").unwrap());
        assert!(src.contains("<div class=\"fm-h\">Label</div>") && qa < dev, "{src}");
        assert!(src.contains("<i class=\"ld\" style=\"background:#F59E0B\"></i>QA</span><span class=\"c\">2</span>"));
        // Chosen: only its cards, a chip that removes it.
        let qa_only = list_html(&view(Some("QA")));
        assert!(qa_only.contains("attention:a-1") && qa_only.contains("attention:a-3") && !qa_only.contains("attention:a-2") && !qa_only.contains("attention:a-4"));
        let head = head_html(&view(Some("QA")));
        assert!(head.contains("<span class=\"ck\">Label :</span> QA</button>") && head.contains("class=\"cx\" data-label=\"all\""), "{head}");
        // No card carries a label: no Label group at all.
        let none = vec![from_attention(&entry("a-9", Level::Now, None, 1), None, false).unwrap()];
        let v = View { items: &none, ..view(None) };
        assert!(!head_html(&v).contains("fm-h\">Label"));
        assert!(!crate::attention::hex_color("red;x") && crate::attention::hex_color("#a1B2c3"));
    }

    #[test]
    fn a_local_file_source_is_a_link_to_the_panel_never_a_dead_path() {
        let opens = |p: &str| ["/Users/me/AI directory/Claap/qa list.md", "/tmp/shot.png"].contains(&p);
        let mut e = entry("a-1", Level::Point, None, 1);
        e.src = vec![
            "/Users/me/AI directory/Claap/qa list.md l.3".into(),
            "capture : /tmp/shot.png".into(),
            "/tmp/missing.md".into(),
            "git log packages/mobile".into(),
        ];
        let d = entry_detail_with(&e, false, &opens);
        let Detail::Entry { src, .. } = &d else { panic!() };
        assert_eq!(src[0].open.as_deref(), Some("/Users/me/AI directory/Claap/qa list.md"), "a path with spaces, a line after it");
        assert_eq!(src[1].open.as_deref(), Some("/tmp/shot.png"));
        assert_eq!(src[2].open, None, "a file that is not there stays words");
        assert_eq!(src[3].open, None);
        let html = detail_html(&d, false);
        assert!(html.contains("<li><button type=\"button\" class=\"link\" data-act=\"link\" data-arg=\"/Users/me/AI directory/Claap/qa list.md\" \
                               title=\"/Users/me/AI directory/Claap/qa list.md\">qa list.md</button> <span class=\"w\">l.3</span></li>"), "{html}");
        assert!(html.contains(">shot.png</button> <span class=\"w\">capture :</span></li>"), "{html}");
        assert!(html.contains("<li>git log packages/mobile</li>"));
        assert_eq!(source_target("voir https://a.b/c, merci", &|_| false).as_deref(), Some("https://a.b/c"));
        assert_eq!(source_target("file:///tmp/shot.png", &opens).as_deref(), Some("/tmp/shot.png"));
    }

    #[test]
    fn a_card_s_title_needs_and_body_link_files_ids_and_pull_requests() {
        let mut e = entry("a-1", Level::Now, None, 1);
        e.needs = Some("Say go after /tmp/plan.md and #13937 <ok>".into());
        e.title = "Review ~/notes/plan.html for CLA-10208".into();
        e.body = Some("Site https://x.io/a, [the plan](/tmp/plan.md). Not /v1/list.json".into());
        let mut item = from_attention(&e, None, true).expect("a card");
        let closed = card_html(&item, true, true, true, 10);
        assert!(closed.contains(r##"Review <a href="#" class="path" data-path="~/notes/plan.html">~/notes/plan.html</a> for <a href="https://linear.app/claap/issue/CLA-10208">CLA-10208</a>"##), "{closed}");
        // Folded: the title, the label, the source and the age, no line under the title.
        assert!(!closed.contains("class=\"need\"") && !closed.contains("Say go after"), "{closed}");
        item.detail = Some(entry_detail(&e, false));
        let open = card_html(&item, true, true, true, 10);
        assert!(open.contains(r##"<div class="need">"##) && open.contains(r##"after <a href="#" class="path" data-path="/tmp/plan.md">/tmp/plan.md</a> and <a href="https://github.com/claap-app/claap/pull/13937">#13937</a> &lt;ok&gt;"##), "{open}");
        assert!(open.contains(r##"Site <a href="https://x.io/a">https://x.io/a</a>, <a href="#" class="path" data-path="/tmp/plan.md">the plan</a>. Not /v1/list.json"##), "{open}");
        // The page sends a link's click to the side panel, not to the card under it.
        let page = shell(false, "n");
        assert!(page.contains("const a = e.target.closest('a');") && page.contains("post({ type: 'open', href })"));
    }

    #[test]
    fn a_draft_shows_where_it_really_goes_and_a_send_by_code_says_so() {
        let mut e = Entry { kind: "would".into(), ..entry("a-1", Level::Now, None, 1) };
        e.draft = Some(Draft { target_label: None, target_kind: None, seen: None, target: Some("U0BJ301TJNM/<17.5>".into()), to: "Sacha".into(), channel: "slack".into(), subject: None, text: "Hi".into() });
        let html = card_html(&from_attention(&e, None, false).unwrap(), true, true, true, 10);
        assert!(html.contains("<span>À <b>Sacha</b> <code class=\"tg\" title=\"Cible de l'envoi\">U0BJ301TJNM/&lt;17.5&gt;</code></span>"), "{html}");
        // No target: the name alone, as before.
        e.draft.as_mut().unwrap().target = None;
        let html = card_html(&from_attention(&e, None, false).unwrap(), true, true, true, 10);
        assert!(html.contains("<span>À <b>Sacha</b></span>") && !html.contains("class=\"tg\""), "{html}");
        // Sent by the command itself: its line, not "handed to the agent".
        let sent = card_html(&from_attention(&e, Some(&Outcome::SentByCode("envoi par le code (slack à <U1>)".into())), false).unwrap(), true, true, true, 10);
        assert!(sent.contains("<div class=\"state ok\">envoi par le code (slack à &lt;U1&gt;)</div>") && !sent.contains("Transmis à l'agent"), "{sent}");
        assert!(sent.contains("readonly") && !sent.contains("data-act=\"validate\""), "not offered again");
    }

    #[test]
    fn a_draft_names_its_target_in_words_and_an_uncertain_card_does_not_put_valider_first() {
        let mut e = Entry { kind: "would".into(), ..entry("a-1", Level::Now, None, 1) };
        e.draft = Some(Draft {
            target: Some("C0123/17.5".into()),
            target_label: Some("#it (canal privé, 4 membres), dans un fil".into()),
            target_kind: Some("private".into()),
            seen: Some("3fe43fde4beeba99".into()),
            to: "Sacha".into(),
            channel: "slack".into(),
            subject: None,
            text: "Hi".into(),
        });
        let item = from_attention(&e, None, false).unwrap();
        let html = card_html(&item, true, true, true, 10);
        assert!(html.contains("<span>À <b>Sacha</b> <span class=\"tg\" title=\"Cible de l'envoi : C0123/17.5\">#it (canal privé, 4 membres), dans un fil</span></span>"), "{html}");
        assert_eq!(item.draft.as_ref().and_then(|d| d.seen.as_deref()), Some("3fe43fde4beeba99"), "the version shown keeps its fingerprint");
        // Outside the workspace: said in amber.
        e.draft.as_mut().unwrap().target_kind = Some("external".into());
        assert!(card_html(&from_attention(&e, None, false).unwrap(), true, true, true, 10).contains("class=\"tg ext\""));
        // Maybe sent: said at the head, and Valider is there but not in blue.
        e.uncertain = Some("Slack n'a pas répondu <30 s>".into());
        let item = from_attention(&e, None, false).unwrap();
        assert_eq!(primary(&item, true), None);
        assert_eq!(buttons(&item, true), [Button::Left, Button::Resend], "the user settles it: it left, or send it again");
        let html = card_html(&item, true, true, true, 10);
        assert!(html.contains("<div class=\"state warn top\">Peut-être parti : Slack n&#39;a pas répondu &lt;30 s&gt;</div>"), "{html}");
        assert!(html.contains("class=\"link\" data-act=\"left\">C'est parti</button>") && html.contains("class=\"link\" data-act=\"resend\">Renvoyer</button>"), "{html}");
        assert!(!html.contains("data-act=\"validate\"") && !html.contains("btn acc"), "no blue button, no Valider: {html}");
        assert_eq!((Button::parse("left"), Button::parse("resend")), (Some(Button::Left), Some(Button::Resend)));
        // A click that changed nothing: the command's words, neutral, and the draft is not offered again.
        e.uncertain = None;
        let said = card_html(&from_attention(&e, Some(&Outcome::Said("déjà remis à claap-agent à 14:02 pour cette version".into())), false).unwrap(), true, true, true, 10);
        assert!(said.contains("<div class=\"state\">déjà remis à claap-agent à 14:02 pour cette version</div>"), "{said}");
        assert!(!said.contains("Transmis à l'agent") && !said.contains("Non transmis") && !said.contains("data-act=\"validate\""), "{said}");
    }

    #[test]
    fn a_card_back_after_a_failure_says_why_and_a_plan_takes_a_no_go() {
        // Back after its action failed: the reason first, the buttons as before.
        let mut e = Entry { kind: "would".into(), ..entry("a-1", Level::Now, None, 1) };
        e.failed = Some("Slack refused the message <403>, see /tmp/send.md".into());
        let html = card_html(&from_attention(&e, None, false).unwrap(), true, true, true, 10);
        let (fail, title) = (html.find("class=\"state crit top\">Échec : Slack refused the message &lt;403&gt;").expect("the reason"), html.find("class=\"t\"").unwrap());
        assert!(fail < title, "at the card's head: {html}");
        assert!(html.contains(r##"<a href="#" class="path" data-path="/tmp/send.md">"##) && html.contains("data-act=\"validate\""), "{html}");
        // A proof is a link when it is one.
        e.failed = None;
        e.proof = Some("https://x.slack.com/archives/C1/p17".into());
        assert!(card_html(&from_attention(&e, None, false).unwrap(), true, true, true, 10).contains(r#"Preuve : <a href="https://x.slack.com/archives/C1/p17">"#));
        // An action plan: `No go`, distinct from the x, with or without a draft.
        let plan = Entry { kind: "decision".into(), plan: true, body: Some("Plan : 1) write 2) send".into()), ..entry("a-2", Level::Point, None, 1) };
        let item = from_attention(&plan, None, false).unwrap();
        assert!(buttons(&item, true).contains(&Button::NoGo));
        let html = card_html(&item, true, true, true, 10);
        assert!(html.contains("data-act=\"nogo\"") && html.contains(">No go<") && html.contains("data-act=\"dismiss\""), "both: {html}");
        let drafted = Entry { draft: Some(Draft { target: None, target_label: None, target_kind: None, seen: None, to: "A".into(), channel: "slack".into(), subject: None, text: "x".into() }), ..plan };
        assert_eq!(buttons(&from_attention(&drafted, None, false).unwrap(), true), [Button::NoGo, Button::Correct, Button::Validate]);
        assert!(!buttons(&from_attention(&entry("a-3", Level::Now, None, 1), None, false).unwrap(), true).contains(&Button::NoGo), "not on another card");
        assert_eq!(Button::parse("nogo"), Some(Button::NoGo));
        // The day's journal: a quiet link under the list, when there is one.
        let items: Vec<Item> = Vec::new();
        let view = |journal: Option<&str>| View {
            items: &items, filter: Filter::All, chat: None, can_act: true, can_close: true, can_snooze: true, limit: PAGE_SIZE, now: 10,
            band: String::new(), record: String::new(), record_note: String::new(), theme: String::new(), show: Show::Mine,
            inflight: String::new(), inflight_n: 0, done_n: 0, journal: journal.map(str::to_string), label: None,
        };
        let (_, list, _) = parts(&view(Some("~/state/recap-2026-11-10.md")));
        assert!(list.ends_with(r##"<div class="jr"><a href="#" class="path" data-path="~/state/recap-2026-11-10.md">Journal du jour</a></div>"##), "{list}");
        assert!(!parts(&view(None)).1.contains("class=\"jr\""));
    }

    #[test]
    fn an_attention_card_carries_its_three_icons_top_right() {
        let item = from_attention(&entry("a-1", Level::Now, None, 1), None, true).unwrap();
        let html = card_html(&item, true, true, true, 10);
        let side = html.split("<div class=\"side\">").nth(1).expect("icons and buttons on the right");
        assert!(side.starts_with("<div class=\"icons\"><button type=\"button\" class=\"ico\" data-act=\"snooze\""));
        assert!(side.contains("title=\"Pas pertinent, retirer\"") && side.contains("data-act=\"done\" title=\"C'est fait\""));
        assert!(!html.contains("<span>C'est fait</span>"), "no text button any more");
        let perm = from_need(&need(NeedKind::Permission, "pane:1", 1), false).unwrap();
        assert!(!card_html(&perm, true, true, true, 10).contains("class=\"icons\""));
    }

    #[test]
    fn the_list_shows_ten_cards_then_voir_plus() {
        let items: Vec<Item> =
            (0..23).map(|i| from_attention(&entry(&format!("a-{i}"), Level::Now, Some(if i < 4 { 1 } else { 2 }), i), None, false).unwrap()).collect();
        let view = |limit| View { items: &items, filter: Filter::All, chat: None, can_act: true, can_close: true, can_snooze: true, limit, now: 100, band: String::new(), record: String::new(), record_note: String::new(), theme: String::new(), show: Show::Mine, inflight: String::new(), inflight_n: 0, done_n: 0, journal: None, label: None };
        let html = list_html(&view(PAGE_SIZE));
        assert_eq!(html.matches("<article").count(), 10);
        assert!(html.contains("data-more=\"1\"><span>Voir plus (13)</span>"));
        assert!(html.contains("Aujourd'hui <span class=\"k\">19</span>"), "a section counts all its cards");
        assert!(head_html(&view(PAGE_SIZE)).contains("<span>Claap</span><span class=\"c\">23</span>"), "the menu counts every card");
        let html = list_html(&view(30));
        assert_eq!(html.matches("<article").count(), 23);
        assert!(!html.contains("data-more"));
    }

    fn need(kind: NeedKind, key: &str, since: u64) -> NeedItem {
        NeedItem { kind, key: key.into(), pane_id: Some(1), since, title: "build".into(), detail: "git push".into() }
    }

    #[test]
    fn memos_are_not_actions_and_levels_give_the_default_priority() {
        let now = from_attention(&entry("a-1", Level::Now, None, 10), None, false).expect("now is an action");
        assert_eq!(now.prio, 2);
        let point = from_attention(&entry("a-2", Level::Point, None, 10), None, false).expect("point is an action");
        assert_eq!(point.prio, 3);
        assert!(from_attention(&entry("a-3", Level::Memo, None, 10), None, false).is_none());
        // The entry's own priority wins.
        assert_eq!(from_attention(&entry("a-4", Level::Point, Some(1), 10), None, false).map(|i| i.prio), Some(1));
        // Closed from here: gone until the queue drops it.
        assert!(from_attention(&entry("a-5", Level::Now, None, 10), Some(&Outcome::Closed), false).is_none());
    }

    #[test]
    fn live_needs_get_their_priority_and_the_attention_need_is_left_to_the_queue() {
        assert_eq!(from_need(&need(NeedKind::Permission, "pane:1", 5), false).map(|i| (i.kind, i.prio)), Some((Kind::Permission, 1)));
        assert_eq!(from_need(&need(NeedKind::Error, "pane:2", 5), false).map(|i| (i.kind, i.prio)), Some((Kind::Error, 1)));
        assert_eq!(from_need(&need(NeedKind::Ask, "ask:q", 5), false).map(|i| (i.kind, i.prio)), Some((Kind::Ask, 2)));
        assert_eq!(from_need(&need(NeedKind::Ask, "ask:q", 5), true).map(|i| i.kind), Some(Kind::Sheet));
        assert_eq!(from_need(&need(NeedKind::FailedRoutine, "routine:n", 5), false).map(|i| i.prio), Some(2));
        assert!(from_need(&need(NeedKind::Attention, "attention:a-1", 5), false).is_none());
        let perm = from_need(&need(NeedKind::Permission, "pane:1", 5), false).expect("an item");
        assert_eq!(perm.title, "build \u{b7} permission à donner");
        // A sheet's tile: one form, one count, nothing of the first question.
        let mut sheet_need = need(NeedKind::Ask, "ask:q", 5);
        sheet_need.detail = "Lem context API : 2 décisions (P6 et P7) : 2 décisions \u{2022} Les serveurs écrivent-ils ?".into();
        assert_eq!(ask_title("Lem context API : 2 décisions (P6 et P7) : 2 décisions\n\u{2022} Les serveurs écrivent-ils ?", true), "Lem context API (P6 et P7) \u{b7} 2 décisions");
        assert_eq!(ask_title("\nPeux-tu relire ?\nle détail", false), "Peux-tu relire ?");
        let folded = card_html(&from_need(&sheet_need, true).unwrap(), true, true, true, 10);
        assert!(!folded.contains("Répondre à la fiche") && !folded.contains("class=\"need\""), "{folded}");
        assert_eq!(perm.needs, "git push");
        assert_eq!(perm.scope, Scope::Pro);
    }

    #[test]
    fn items_sort_by_priority_then_oldest_first() {
        let mut items: Vec<Item> = vec![
            from_attention(&entry("a-1", Level::Point, None, 1), None, false).unwrap(),
            from_attention(&entry("a-2", Level::Now, None, 50), None, false).unwrap(),
            from_attention(&entry("a-3", Level::Now, None, 20), None, false).unwrap(),
            from_need(&need(NeedKind::Permission, "pane:9", 99), false).unwrap(),
        ];
        sort(&mut items);
        let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["pane:9", "attention:a-3", "attention:a-2", "attention:a-1"]);
        assert!(any_urgent(&items));
        assert!(!any_urgent(&items[1..]));
    }

    /// The one blue button of a card, as rendered, and how many there are.
    fn blue(html: &str) -> (usize, String) {
        let n = html.matches("class=\"btn acc\"").count();
        let label = html
            .split("class=\"btn acc\"")
            .nth(1)
            .and_then(|rest| rest.split("<span>").nth(1))
            .and_then(|rest| rest.split("</span>").next())
            .unwrap_or_default()
            .to_string();
        (n, label)
    }

    #[test]
    fn every_kind_has_one_blue_button_named_by_its_action_and_never_ouvrir() {
        let with = |kind: &str, action: Option<&str>, openable: bool| {
            let e = Entry { kind: kind.into(), action: action.map(Into::into), ..entry("a-1", Level::Now, None, 1) };
            from_attention(&e, None, openable).unwrap()
        };
        let draft = {
            let e = Entry { kind: "would".into(), draft: Some(Draft { target: None, target_label: None, target_kind: None, seen: None, to: "A".into(), channel: "email".into(), subject: None, text: "Hi".into() }), ..entry("a-1", Level::Now, None, 1) };
            from_attention(&e, None, false).unwrap()
        };
        let mut sheet_entry = with("decision", Some("kova-sheet:430b8a55"), false);
        sheet_entry.ask = Some("430b8a55-1".into());
        let live = |kind: NeedKind, key: &str, sheet: bool| from_need(&need(kind, key, 1), sheet).unwrap();
        let cases: Vec<(&str, Item, Option<&str>)> = vec![
            ("would", with("would", Some("/tmp/prep.md"), true), Some("Valider")),
            ("review", with("review", Some("https://x.test/pr"), true), Some("Valider")),
            ("decision prepared", with("decision", Some("/tmp/prep.md"), true), Some("Valider")),
            ("draft", draft, Some("Valider")),
            ("decision sheet entry", sheet_entry, Some("Décider")),
            ("decision", with("decision", None, false), Some("Décider")),
            ("test", with("test", Some("/tmp/plan.md"), true), Some("Tester")),
            ("blocked", with("blocked", None, false), Some("Brief")),
            ("ask", with("ask", None, false), Some("Brief")),
            ("info", with("info", Some("https://x.test"), true), None),
            ("auto", with("auto", None, false), None),
            ("permission", live(NeedKind::Permission, "pane:1", false), Some("Autoriser")),
            ("question", live(NeedKind::Ask, "ask:q1", false), Some("Répondre")),
            ("sheet", live(NeedKind::Ask, "ask:q2", true), Some("Décider")),
            ("error", live(NeedKind::Error, "pane:2", false), Some("Voir le pane")),
            ("routine", live(NeedKind::FailedRoutine, "routine:nightly", false), Some("Voir le log")),
        ];
        for (name, item, want) in cases {
            let html = card_html(&item, true, true, true, 10);
            let (n, label) = blue(&html);
            match want {
                Some(want) => assert_eq!((n, label.as_str()), (1, want), "{name}: {html}"),
                None => assert_eq!(n, 0, "{name}: {html}"),
            }
            assert!(!html.contains(">Ouvrir<") && !html.contains("Ouvrir la fiche"), "{name}: {html}");
            // Everything else is a plain link, never a grey button.
            assert!(!html.contains("btn sec"), "{name}: {html}");
            // The attention cards keep their three icons top right.
            if item.kind == Kind::Attention {
                assert_eq!(html.matches("class=\"ico\"").count(), 3, "{name}");
            }
        }
        // The material of an entry is a secondary link, `Voir`.
        let html = card_html(&with("test", Some("/tmp/plan.md"), true), true, true, true, 10);
        assert!(html.contains("<button type=\"button\" class=\"link\" data-act=\"open\">Voir</button>"), "{html}");
        // Brief, Décider, Tester open the card on its Consigne; Répondre on the answer.
        assert!(html.contains("data-act=\"expand\" data-focus=\"ins\""), "{html}");
        let q = card_html(&live(NeedKind::Ask, "ask:q1", false), true, true, true, 10);
        assert!(q.contains("data-act=\"expand\" data-focus=\"ans\""), "{q}");
        // A draft's Corriger is a link next to its blue Valider; it opens the
        // card on its consigne field.
        let e = Entry { kind: "would".into(), draft: Some(Draft { target: None, target_label: None, target_kind: None, seen: None, to: "A".into(), channel: "email".into(), subject: None, text: "Hi".into() }), ..entry("a-1", Level::Now, None, 1) };
        let d = card_html(&from_attention(&e, None, false).unwrap(), true, true, true, 10);
        assert!(d.contains("<button type=\"button\" class=\"link\" data-act=\"correct\" data-focus=\"ins\">Corriger</button>"), "{d}");
    }

    #[test]
    fn the_page_keeps_a_selection_and_routes_the_opening_buttons() {
        let page = shell(false, "n0");
        // Nothing in the page turns selection off.
        assert!(!page.contains("user-select:none") && !page.contains("user-select: none"));
        // A push waits while text is selected in its part.
        assert!(page.contains("if (selectedIn(id)) { held[id] = html; return; }"));
        assert!(page.contains("document.addEventListener('selectionchange'"));
        // A click that ends a drag-select does not fold or open the card.
        assert!(page.contains("if (String(window.getSelection()).trim()) return;\n    return post({ type: 'act', key: card.dataset.key, act: 'expand' });"));
        assert!(page.contains("if (card.classList.contains('open')) return focusIn(card, b.dataset.focus);"));
    }

    #[test]
    fn buttons_follow_the_kind() {
        let info = Entry { kind: "info".into(), ..entry("a-1", Level::Now, None, 1) };
        let mut item = from_attention(&info, None, true).unwrap();
        assert_eq!(buttons(&item, true), [Button::Open]);
        // `C'est fait` is an icon now, with snooze and remove.
        assert_eq!(icons(&item, true, true), [Button::Snooze, Button::Dismiss, Button::Done]);
        assert_eq!(icons(&item, true, false), [Button::Dismiss, Button::Done]);
        assert_eq!(icons(&item, false, false), []);
        item.openable = false;
        assert_eq!(buttons(&item, true), []);
        item.draft = Some(Draft { target: None, target_label: None, target_kind: None, seen: None, to: "A".into(), channel: "Email".into(), subject: None, text: "Hi".into() });
        assert_eq!(buttons(&item, true), [Button::Correct, Button::Validate]);
        item.state = State::Sent;
        assert_eq!(buttons(&item, true), []);
        let sheet = from_need(&need(NeedKind::Ask, "ask:q", 1), true).unwrap();
        // A pending sheet is answered on the sheet, never marked done here.
        assert_eq!(buttons(&sheet, true), [Button::OpenSheet]);
        let perm = from_need(&need(NeedKind::Permission, "pane:1", 1), false).unwrap();
        assert_eq!(buttons(&perm, true), [Button::Deny, Button::Allow]);
        let err = from_need(&need(NeedKind::Error, "pane:1", 1), false).unwrap();
        assert_eq!(buttons(&err, true), [Button::Pane]);
        assert!(icons(&perm, true, true).is_empty(), "a live need keeps its own buttons");
        // Once a go went through, or a consigne, the card is cleared as done,
        // never removed as not relevant.
        item.state = State::Sent;
        assert_eq!(icons(&item, true, true), [Button::Snooze, Button::Done]);
        item.state = State::Idle;
        item.instruction = Some(Instructed { text: "il a répondu, mets à jour".into(), outcome: Outcome::Sent });
        assert_eq!(icons(&item, true, false), [Button::Done]);
        item.instruction = Some(Instructed { text: "x".into(), outcome: Outcome::Failed { words: "boom".into(), until_ms: 0 } });
        assert_eq!(icons(&item, true, false), [Button::Dismiss, Button::Done], "a consigne that failed is no act");
        item.instruction = None;
        let ask = from_need(&need(NeedKind::Ask, "ask:q1", 5), false).unwrap();
        assert_eq!(icons(&ask, false, false), [Button::Snooze, Button::Dismiss, Button::Done], "an ask closes on its board");
        for b in [Button::Snooze, Button::Dismiss, Button::Expand, Button::Done, Button::Deny, Button::Allow, Button::Correct, Button::Validate, Button::Refuse, Button::Open] {
            assert_eq!(Button::parse(b.act()), Some(b));
        }
    }

    #[test]
    fn an_entry_waiting_on_a_go_carries_refuser_and_valider() {
        let with = |kind: &str, action: Option<&str>| Entry { kind: kind.into(), action: action.map(Into::into), ..entry("a-1", Level::Point, None, 1) };
        assert!(expects_validation(&with("would", None)));
        assert!(expects_validation(&with("review", Some("https://notion.so/x"))));
        assert!(expects_validation(&with("decision", Some("/tmp/prep.md#P1"))));
        assert!(!expects_validation(&with("decision", None)), "a decision with nothing prepared");
        assert!(!expects_validation(&with("decision", Some("kova-sheet:430b8a55"))), "a sheet takes its own answers");
        assert!(!expects_validation(&with("test", Some("/tmp/x.md"))));
        let drafted = Entry { draft: Some(Draft { target: None, target_label: None, target_kind: None, seen: None, to: "A".into(), channel: "email".into(), subject: None, text: "Hi".into() }), ..with("would", None) };
        assert!(!expects_validation(&drafted), "a draft keeps its own Valider");
        let mut item = from_attention(&with("would", Some("/tmp/prep.md")), None, true).unwrap();
        assert_eq!(buttons(&item, true), [Button::Open, Button::Refuse, Button::Validate]);
        assert_eq!(buttons(&item, false), [Button::Open, Button::Refuse], "no act command, no go to give");
        let html = card_html(&item, true, true, true, 10);
        // Valider is the one blue button, Voir and Refuser are links, the
        // icons stay, the reason field waits hidden.
        assert!(html.contains("<button type=\"button\" class=\"link\" data-act=\"open\">Voir</button>"), "{html}");
        assert!(html.contains("<button type=\"button\" class=\"link\" data-act=\"refuse\">Refuser</button>"), "{html}");
        assert!(html.contains("class=\"btn acc\" data-act=\"validate\">"), "{html}");
        assert!(html.contains("data-act=\"snooze\"") && html.contains("data-act=\"dismiss\"") && html.contains("data-act=\"done\""));
        assert!(html.contains("<div class=\"why\" hidden><input type=\"text\" class=\"why\" placeholder=\"Pourquoi ? (facultatif)\""), "{html}");
        // Refuser closes the entry: without a close command it is left out.
        let no_close = card_html(&item, true, false, true, 10);
        assert!(!no_close.contains("data-act=\"refuse\"") && !no_close.contains("class=\"why\""), "{no_close}");
        // The go went through: the card says so and offers it no more.
        item.state = State::Sent;
        assert_eq!(buttons(&item, true), [Button::Open]);
        assert!(card_html(&item, true, true, true, 10).contains("Transmis"));
        // An opened card keeps its instruction field under the reason field.
        item.state = State::Idle;
        item.detail = Some(entry_detail(&with("would", None), false));
        let opened = card_html(&item, true, true, true, 10);
        assert!(opened.find("class=\"why\"").unwrap() < opened.find("Consigne").unwrap(), "{opened}");
    }

    #[test]
    fn the_filter_keeps_one_scope_and_the_counts_cover_every_scope() {
        let mut perso = entry("a-2", Level::Now, None, 2);
        perso.scope = Scope::Perso;
        let items = vec![
            from_attention(&entry("a-1", Level::Now, None, 1), None, false).unwrap(),
            from_attention(&perso, None, false).unwrap(),
        ];
        let view = |filter| View { items: &items, filter, chat: None, can_act: true, can_close: true, can_snooze: true, limit: PAGE_SIZE, now: 100, band: String::new(), record: String::new(), record_note: String::new(), theme: String::new(), show: Show::Mine, inflight: String::new(), inflight_n: 0, done_n: 0, journal: None, label: None };
        assert!(list_html(&view(Filter::All)).contains("attention:a-1"));
        let only = list_html(&view(Filter::Only(Scope::Perso)));
        assert!(only.contains("attention:a-2") && !only.contains("attention:a-1"));
        let head = head_html(&view(Filter::Only(Scope::Perso)));
        // The menu counts every scope; the chosen one, picked again, goes back to all.
        assert!(head.contains("data-filter=\"pro\"><span class=\"ok\">"), "{head}");
        assert!(head.contains("<button type=\"button\" class=\"fm-o on\" data-filter=\"all\">"), "{head}");
        assert!(head.contains("<span class=\"ck\">Espace :</span> Perso</button>"), "{head}");
        assert_eq!(Filter::parse("pro"), Some(Filter::Only(Scope::Pro)));
        assert_eq!(Filter::parse("nope"), None);
    }

    #[test]
    fn the_filters_are_one_control_a_menu_and_removable_chips() {
        let mut perso = entry("a-2", Level::Now, None, 2);
        perso.scope = Scope::Perso;
        let items = vec![from_attention(&entry("a-1", Level::Now, None, 1), None, false).unwrap(), from_attention(&perso, None, false).unwrap()];
        let view = |filter, show, inflight: &str| View {
            items: &items, filter, chat: None, can_act: true, can_close: true, can_snooze: true,
            limit: PAGE_SIZE, now: 100, band: String::new(), record: String::new(), record_note: String::new(),
            theme: String::new(), show, inflight: inflight.to_string(), inflight_n: 4, done_n: 7, journal: None, label: None,
        };
        // Default: no chip, one `Filtrer` control, the sort label on the right.
        let head = head_html(&view(Filter::All, Show::Mine, ""));
        assert!(head.contains("<div class=\"filters\"><button type=\"button\" class=\"flt\" data-fopen=\"1\">"), "{head}");
        assert!(head.contains("<span>Filtrer</span>") && !head.contains("class=\"chip\"") && head.contains("Par priorité"));
        assert!(!head.contains("class=\"seg\""), "no more segmented toggles");
        // The menu: every dimension, its values and counts, nothing chosen.
        let src = head.split("<div id=\"fmsrc\" hidden>").nth(1).unwrap();
        assert!(src.starts_with("<div class=\"fm-h\">Espace</div>"), "{src}");
        assert!(src.contains("data-filter=\"pro\"><span class=\"ok\">") && src.contains("<span>Claap</span><span class=\"c\">1</span>"));
        assert!(src.contains("<div class=\"fm-h\">Vue</div>"));
        assert!(src.contains("<button type=\"button\" class=\"fm-o on\" data-show=\"mine\">"), "À moi is the default, picked again it stays");
        assert!(src.contains("data-show=\"inflight\"><span class=\"ok\">") && src.contains("<span>En cours</span><span class=\"c\">4</span>"));
        assert!(src.contains("data-show=\"done\"><span class=\"ok\">") && src.contains("<span>Fait (24 h)</span><span class=\"c\">7</span>"));
        assert!(list_html(&view(Filter::All, Show::Mine, "")).contains("attention:a-1"));
        // Two filters chosen: two chips with their cross, the control turns into `+`.
        let flight = view(Filter::Only(Scope::Pro), Show::InFlight, "<div class=\"clk\">X</div>");
        let head = head_html(&flight);
        assert!(head.contains("<span class=\"ck\">Espace :</span> Claap</button>"), "{head}");
        assert!(head.contains("<span class=\"ck\">Vue :</span> En cours</button>"));
        assert!(head.contains("class=\"cx\" data-filter=\"all\"") && head.contains("class=\"cx\" data-show=\"mine\""));
        assert!(head.contains("title=\"Ajouter un filtre\"") && !head.contains("<span>Filtrer</span>"));
        assert!(head.contains("En direct"));
        assert_eq!(list_html(&flight), "<div class=\"clk\">X</div>");
        assert!(list_html(&view(Filter::All, Show::InFlight, "")).contains("Lecture en cours"));
        assert_eq!(Show::parse("inflight"), Some(Show::InFlight));
        assert_eq!(Show::parse("done"), Some(Show::Done));
        assert_eq!(list_html(&view(Filter::All, Show::Done, "<div class=\"clk\">D</div>")), "<div class=\"clk\">D</div>");
        assert_eq!(Show::parse("x"), None);
        // The page: the menu outside the head, refilled by a push, picked items posted.
        let page = shell(false, "n");
        assert!(page.contains("<div class=\"snz fm\" id=\"fm\" hidden></div>"));
        assert!(page.contains("if (u.head !== undefined) { apply('head', u.head); fmFill(); }"));
        assert!(page.contains("if (b.dataset.show) return post({ type: 'show', show: b.dataset.show });"));
    }

    #[test]
    fn sections_and_subtitle_name_the_priorities() {
        let items = vec![
            from_need(&need(NeedKind::Permission, "pane:1", 1), false).unwrap(),
            from_attention(&entry("a-1", Level::Point, None, 1), None, false).unwrap(),
        ];
        let html = list_html(&View { items: &items, filter: Filter::All, chat: None, can_act: true, can_close: true, can_snooze: true, limit: PAGE_SIZE, now: 100, band: String::new(), record: String::new(), record_note: String::new(), theme: String::new(), show: Show::Mine, inflight: String::new(), inflight_n: 0, done_n: 0, journal: None, label: None });
        assert!(html.contains("Urgent <span class=\"k\">1</span>"));
        assert!(html.contains("Cette semaine <span class=\"k\">1</span>"));
        assert!(!html.contains("Aujourd'hui"));
        assert_eq!(subtitle(&items), ("2 actions pour toi".to_string(), "1 urgente".to_string()));
        assert_eq!(subtitle(&[]).0, "Aucune action pour toi");
        assert!(list_html(&View { items: &[], filter: Filter::All, chat: None, can_act: true, can_close: true, can_snooze: true, limit: PAGE_SIZE, now: 0, band: String::new(), record: String::new(), record_note: String::new(), theme: String::new(), show: Show::Mine, inflight: String::new(), inflight_n: 0, done_n: 0, journal: None, label: None }).contains("Rien ne t'attend"));
    }

    #[test]
    fn ages_read_in_french() {
        assert_eq!(age(0, 100), "");
        assert_eq!(age(100, 130), "à l'instant");
        assert_eq!(age(100, 100 + 4 * 60), "il y a 4 min");
        assert_eq!(age(100, 100 + 2 * 3600), "il y a 2 h");
        assert_eq!(age(100, 100 + 30 * 3600), "hier");
        assert_eq!(age(100, 100 + 3 * 86_400), "il y a 3 j");
    }

    #[test]
    fn every_text_is_escaped_and_the_draft_keeps_its_leading_newline() {
        let mut e = entry("a-1", Level::Now, None, 1);
        e.title = "<img src=x onerror=alert(1)>".into();
        e.from = "\"a\" & 'b'".into();
        e.needs = Some("</textarea><script>".into());
        e.draft = Some(Draft {
            target: None,
            target_label: None,
            target_kind: None,
            seen: None,
            to: "<b>x</b>".into(),
            channel: "Email".into(),
            subject: Some("S\"".into()),
            text: "\nLine</textarea><script>alert(1)</script>".into(),
        });
        let item = from_attention(&e, None, false).unwrap();
        let html = card_html(&item, true, true, true, 10);
        assert!(!html.contains("<img"), "{html}");
        assert!(!html.contains("<script"), "{html}");
        assert_eq!(html.matches("</textarea>").count(), 1, "only the field's own end tag");
        assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(html.contains("&quot;a&quot; &amp; &#39;b&#39;"));
        // The parser drops one newline after `<textarea>`: the text's own
        // leading newline survives.
        assert!(html.contains(">\n\nLine&lt;/textarea&gt;"));
        assert!(html.contains("À <b>&lt;b&gt;x&lt;/b&gt;</b>"));
        assert!(html.contains("Objet <b>S&quot;</b>"));
        // Without act_command, a draft shows as a plain card.
        assert!(!card_html(&item, false, true, true, 10).contains("<textarea"));
    }

    #[test]
    fn an_opened_attention_card_takes_an_instruction_even_without_a_button() {
        // A card with no action, no draft, no button: once opened, a field.
        let bare = Entry { needs: None, kind: "info".into(), ..entry("a-1", Level::Point, None, 1) };
        let mut item = from_attention(&bare, None, false).unwrap();
        assert!(buttons(&item, true).is_empty());
        let field = format!("<textarea class=\"ins\" rows=\"2\" spellcheck=\"true\" placeholder=\"{INSTRUCT_PLACEHOLDER}\">");
        assert!(!card_html(&item, true, true, true, 10).contains("class=\"ins\""), "folded: no field");
        item.detail = Some(entry_detail(&bare, false));
        let html = card_html(&item, true, true, true, 10);
        assert!(html.contains(&field), "{html}");
        assert!(html.contains("<div class=\"lbl\">Consigne</div>"));
        // Without act_command there is nowhere to send it.
        assert!(!card_html(&item, false, true, true, 10).contains("class=\"ins\""));

        // Sent: the card says so with the text that went, escaped.
        item.instruction = Some(Instructed { text: "Relance <Fidsud> \"vite\"\nmerci".into(), outcome: Outcome::Sent });
        let html = card_html(&item, true, true, true, 10);
        assert!(html.contains("Consigne remise à l'agent</div><p class=\"msg sent\">Relance &lt;Fidsud&gt; &quot;vite&quot;\nmerci</p>"), "{html}");
        // Recorded, not handed to the agent yet: said as such, not as a failure.
        item.instruction = Some(Instructed { text: "plus tard".into(), outcome: Outcome::Noted });
        let noted = card_html(&item, true, true, true, 10);
        assert!(noted.contains("<div class=\"state warn\">Consigne notée, pas encore remise à l'agent</div><p class=\"msg sent\">plus tard</p>"), "{noted}");
        item.instruction = Some(Instructed { text: "Relance <Fidsud> \"vite\"\nmerci".into(), outcome: Outcome::Sent });
        assert!(html.contains(&field), "the field stays for the next one");
        // In flight: the field waits.
        item.instruction = Some(Instructed { text: "x".into(), outcome: Outcome::Running });
        let html = card_html(&item, true, true, true, 10);
        assert!(html.contains("placeholder=\"Une consigne pour cette carte\" readonly>") && html.contains("En cours"));
        item.instruction = Some(Instructed { text: "x".into(), outcome: Outcome::Failed { words: "boom".into(), until_ms: 0 } });
        assert!(card_html(&item, true, true, true, 10).contains("Consigne non envoyée : boom"));

        // A draft keeps its own correction field; the instruction comes
        // under the draft's actions, once.
        let mut e = entry("a-2", Level::Now, None, 1);
        e.draft = Some(Draft { target: None, target_label: None, target_kind: None, seen: None, to: "A".into(), channel: "Email".into(), subject: None, text: "Hi".into() });
        let mut item = from_attention(&e, None, false).unwrap();
        item.detail = Some(entry_detail(&e, false));
        item.instruction = Some(Instructed { text: "Plus court".into(), outcome: Outcome::Sent });
        let html = card_html(&item, true, true, true, 10);
        assert_eq!(html.matches("class=\"bd\"").count(), 1);
        assert_eq!(html.matches("class=\"ins\"").count(), 1);
        assert!(html.find("data-act=\"validate\"").unwrap() < html.find("class=\"ins\"").unwrap());
        // An instruction never reads as a sent draft: Valider stays.
        assert_eq!(buttons(&item, true), [Button::Correct, Button::Validate]);
        assert!(!html.contains("Transmis"));

        // A live card has no attention id: no field.
        let mut live = from_need(&need(NeedKind::Error, "pane:1", 5), false).unwrap();
        live.detail = Some(Detail::Error { words: "w".into(), lines: String::new() });
        assert!(!card_html(&live, true, true, true, 10).contains("class=\"ins\""));
    }

    #[test]
    fn a_sent_draft_says_so_and_a_running_click_disables_the_buttons() {
        let mut e = entry("a-1", Level::Now, None, 1);
        e.draft = Some(Draft { target: None, target_label: None, target_kind: None, seen: None, to: "A".into(), channel: "Email".into(), subject: None, text: "Hi".into() });
        let sent = from_attention(&e, Some(&Outcome::Sent), false).unwrap();
        let html = card_html(&sent, true, true, true, 10);
        assert!(html.contains("Transmis à l'agent") && html.contains("readonly") && !html.contains("data-act=\"validate\""));
        // The three ends of a click: delivered, noted, failed with its reason.
        let noted = card_html(&from_attention(&e, Some(&Outcome::Noted), false).unwrap(), true, true, true, 10);
        assert!(noted.contains("<div class=\"state warn\">Noté, pas encore remis à l'agent</div>") && noted.contains("readonly"), "{noted}");
        assert!(!noted.contains("data-act=\"validate\""), "not offered again: the click is recorded");
        let failed = card_html(&from_attention(&e, Some(&Outcome::Failed { words: "session <gone>".into(), until_ms: 0 }), false).unwrap(), true, true, true, 10);
        assert!(failed.contains("<div class=\"state crit\">Non transmis : session &lt;gone&gt;</div>"), "{failed}");
        assert!(failed.contains("data-act=\"validate\""), "the card stays, with its buttons");
        // The icons stay: the entry waits for its receiver to close it.
        assert!(html.contains("class=\"icons\""));
        let running = from_attention(&e, Some(&Outcome::Running), false).unwrap();
        assert!(card_html(&running, true, true, true, 10).contains("data-act=\"validate\" disabled"));
        let failed = from_attention(&e, Some(&Outcome::Failed { words: "<boom>".into(), until_ms: 0 }), false).unwrap();
        assert!(card_html(&failed, true, true, true, 10).contains("&lt;boom&gt;"));
    }

    #[test]
    fn the_chat_column_names_its_chief_or_says_it_has_no_pane() {
        let html = chat_html(&ChatColumn { chief: Scope::Perso, found: false });
        assert!(html.contains("Pas de session perso-agent dans cette fenêtre"));
        assert!(html.contains("class=\"on\" data-chief=\"perso\""));
        let html = chat_html(&ChatColumn { chief: Scope::Pro, found: true });
        assert!(html.contains("claap-agent") && !html.contains("Pas de session"));
        assert_eq!(chief_session(Scope::Pro), "claap-agent");
    }

    #[test]
    fn the_page_has_no_slot_left_and_runs_only_its_own_script() {
        let page = shell(false, "abc123");
        for slot in ["{scheme}", "{vars}", "{nonce}", "{handler}"] {
            assert!(!page.contains(slot), "{slot} left");
        }
        assert!(page.contains("script-src 'nonce-abc123'"));
        assert!(page.contains("<script nonce=\"abc123\">"));
        assert!(page.contains("messageHandlers.kovaCockpit"));
        assert!(shell(true, "n").contains("color-scheme:light"));
    }

    #[test]
    fn the_chat_column_sits_left_of_the_action_list() {
        let page = shell(false, "n");
        let (chat, list) = (page.find(r#"<section class="chat""#).unwrap(), page.find(r#"<section class="page""#).unwrap());
        assert!(chat < list, "the chat comes first in the flex row");
        assert!(page.contains(".chat{width:33.333%;flex:none;border-right:"));
        // It takes its width; only the side panel floats (Robin, 09/11).
        assert!(!page.contains("position:fixed;left:0;top:0;bottom:0;z-index:5"));
        assert_eq!(chat_frame((200.0, 10.0, 900.0, 600.0), 56.0), (200.0, 10.0, 299.0, 544.0), "the left third, under its bar");
        assert!(!PAGE.contains('\u{2014}'), "no em dash in the UI");
        let u = Update { list: Some("<p>x</p>".into()), ..Default::default() };
        assert_eq!(update_script(&u), "window.kova && window.kova.update({\"list\":\"<p>x</p>\"})");
        assert!(Update::default().is_empty());
    }

    #[test]
    fn the_theme_button_shows_the_mode_and_its_shortcut() {
        use crate::theme::ThemeMode;
        assert_eq!(shortcut_label("cmd+option+l"), "\u{2325}\u{2318}L");
        assert_eq!(shortcut_label(""), "");
        let dark = theme_button(ThemeMode::Dark, "cmd+option+l");
        assert!(dark.contains("data-theme=\"cycle\"") && dark.contains(ICON_MOON));
        assert!(dark.contains("title=\"Thème : Sombre, clic pour Système (\u{2325}\u{2318}L)\""), "{}", dark);
        assert!(theme_button(ThemeMode::Light, "").contains(ICON_SUN));
        assert!(theme_button(ThemeMode::System, "x").contains(ICON_SYSTEM));
    }

    #[test]
    fn the_catch_up_band_sits_between_the_title_and_the_filter() {
        let items: Vec<Item> = Vec::new();
        let view = |band: &str| View {
            items: &items, filter: Filter::All, chat: None, can_act: true, can_close: true, can_snooze: true,
            limit: PAGE_SIZE, now: 0, band: band.to_string(), record: "<button data-rec=\"start\">R</button>".into(),
            record_note: "<div class=\"recn\">N</div>".into(),
            theme: theme_button(crate::theme::ThemeMode::Dark, "cmd+option+l"),
            show: Show::Mine, inflight: String::new(), inflight_n: 0, done_n: 0, journal: None, label: None,
        };
        let head = head_html(&view("<div class=\"cu\">B</div>"));
        let (title, band, bar) = (head.find("ptitle").unwrap(), head.find("class=\"cu\"").unwrap(), head.find("class=\"bar\"").unwrap());
        assert!(title < band && band < bar);
        // The record button sits left of Chat; its note above the band.
        assert!(head.find("data-rec=\"start\"").unwrap() < head.find("data-chat").unwrap());
        assert!(head.find("class=\"recn\"").unwrap() < band);
        assert!(!head_html(&view("")).contains("class=\"cu\""), "no band, nothing between");
        // A reveal alone is a push.
        assert!(!Update { reveal: Some("attention:a-1".into()), ..Default::default() }.is_empty());
    }
}

#[cfg(test)]
mod shot_demo {
    /// `KOVA_SHOT_DEMO=<out.html> cargo test shot_demo`: a page with one card
    /// showing the two given screenshots, to look at outside Kova.
    #[test]
    fn write_demo() {
        let Ok(out) = std::env::var("KOVA_SHOT_DEMO") else { return };
        let (b, a) = (std::env::var("KOVA_SHOT_B").unwrap(), std::env::var("KOVA_SHOT_A").unwrap());
        let e = crate::attention::Entry {
            id: "a-demo".into(), since: 1, from: "groom-priorisation".into(), level: crate::attention::Level::Now,
            kind: "test".into(), title: "CLA-10067 (skeleton du desktop) : avant / après".into(),
            needs: Some("Valider le visuel".into()), action: None, topic: None, scope: crate::attention::Scope::Pro,
            prio: Some(2), draft: None, body: None, detail: None, src: Vec::new(), before: Some(b), after: Some(a), label: None,
            failed: None, uncertain: None, proof: None, plan: false,
        };
        let item = super::from_attention(&e, None, false).unwrap();
        let page = super::shell(false, "n").replace("<div id=\"list\"></div>", &format!("<div id=\"list\">{}</div>", super::card_html(&item, true, true, true, 10)));
        std::fs::write(out, page).unwrap();
    }
}
