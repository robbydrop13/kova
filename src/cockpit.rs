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
    /// Opened with `Ouvrir`: what the card shows under itself. Filled by the
    /// host only for the cards it keeps open.
    pub detail: Option<Detail>,
    pub state: State,
    /// The last instruction typed on an attention card, filled by the host.
    pub instruction: Option<Instructed>,
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
    Entry { needs: String, body: String, detail: String, src: Vec<String>, links: Vec<String>, explaining: bool },
}

/// The http(s) links in a text, in order, without the punctuation that
/// closes a sentence around them.
pub fn links_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || c == '<' || c == '>') {
        let Some(at) = word.find("http://").or_else(|| word.find("https://")) else { continue };
        let link = word[at..].trim_end_matches(|c: char| ".,;:!?)]}\"'`".contains(c));
        if link.len() > "https://".len() && !out.iter().any(|l| l == link) {
            out.push(link.to_string());
        }
    }
    out
}

/// What an opened attention card shows: its needs, body, detail, sources,
/// and its action plus every http(s) link in the rest. `explaining` says
/// `attn explain` runs for it.
pub fn entry_detail(entry: &Entry, explaining: bool) -> Detail {
    let text = |s: &Option<String>| s.as_deref().unwrap_or_default().to_string();
    let mut links: Vec<String> = Vec::new();
    if let Some(action) = entry.action.as_deref().filter(|a| ask_ref(a).is_none()) {
        links.push(action.to_string());
    }
    let rest = [text(&entry.body), text(&entry.detail), entry.src.join("\n")].join("\n");
    for link in links_in(&rest) {
        if !links.contains(&link) {
            links.push(link);
        }
    }
    Detail::Entry {
        needs: text(&entry.needs),
        body: text(&entry.body),
        detail: text(&entry.detail),
        src: entry.src.clone(),
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
        detail: None,
        state,
        instruction: None,
    })
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

/// A live need as an item. `sheet` says an ask is a decision sheet. An
/// attention need is left out: the queue's own entry stands for it. Live
/// needs carry no scope: they count as work.
pub fn from_need(need: &NeedItem, sheet: bool) -> Option<Item> {
    let (kind, prio, title, needs, from) = match need.kind {
        NeedKind::Attention => return None,
        NeedKind::Permission => (
            Kind::Permission,
            1,
            format!("{} attend une permission", need.title),
            if need.detail.trim().is_empty() { "Autoriser ou refuser".to_string() } else { need.detail.clone() },
            format!("pane {}", need.title),
        ),
        NeedKind::Error => (
            Kind::Error,
            1,
            format!("{} : le tour s'est arrêté", need.title),
            if need.detail.trim().is_empty() { "Regarder le pane".to_string() } else { need.detail.clone() },
            format!("pane {}", need.title),
        ),
        NeedKind::FailedRoutine => (
            Kind::FailedRoutine,
            2,
            format!("Routine {} en échec", need.title),
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
        detail: None,
        state: State::Idle,
        instruction: None,
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

/// A card's button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Open or fold the card's detail in place (`Ouvrir` / `Masquer`).
    Expand,
    Open,
    OpenSheet,
    Done,
    Deny,
    Allow,
    Correct,
    Validate,
    /// The attention card's icons: out until a date, not relevant.
    Snooze,
    Dismiss,
}

impl Button {
    /// The word the page posts.
    pub fn act(self) -> &'static str {
        match self {
            Button::Expand => "expand",
            Button::Open | Button::OpenSheet => "open",
            Button::Done => "done",
            Button::Deny => "deny",
            Button::Allow => "allow",
            Button::Correct => "correct",
            Button::Validate => "validate",
            Button::Snooze => "snooze",
            Button::Dismiss => "dismiss",
        }
    }

    pub fn parse(word: &str) -> Option<Button> {
        match word {
            "expand" => Some(Button::Expand),
            "open" => Some(Button::Open),
            "done" => Some(Button::Done),
            "deny" => Some(Button::Deny),
            "allow" => Some(Button::Allow),
            "correct" => Some(Button::Correct),
            "validate" => Some(Button::Validate),
            "snooze" => Some(Button::Snooze),
            "dismiss" => Some(Button::Dismiss),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Button::Expand | Button::Open => "Ouvrir",
            Button::OpenSheet => "Ouvrir la fiche",
            Button::Done => "C'est fait",
            Button::Deny => "Refuser",
            Button::Allow => "Autoriser",
            Button::Correct => "Corriger",
            Button::Validate => "Valider",
            Button::Snooze => "Plus tard",
            Button::Dismiss => "Pas pertinent, retirer",
        }
    }

    fn primary(self) -> bool {
        matches!(self, Button::Open | Button::OpenSheet | Button::Allow | Button::Validate)
    }
}

/// A card's buttons, left to right. A draft's verbs need `act_command`; a
/// sent draft has none left.
pub fn buttons(item: &Item, can_act: bool) -> Vec<Button> {
    match item.kind {
        Kind::Attention if item.draft.is_some() && can_act => {
            if item.state == State::Sent {
                Vec::new()
            } else {
                vec![Button::Correct, Button::Validate]
            }
        }
        Kind::Attention if item.ask.is_some() => vec![Button::OpenSheet],
        // `C'est fait` is one of the card's icons (`icons`).
        Kind::Attention if item.openable => vec![Button::Open],
        Kind::Attention => Vec::new(),
        Kind::Sheet => vec![Button::OpenSheet],
        // The live kinds open in place: nothing jumps to another view.
        Kind::Permission => vec![Button::Expand, Button::Deny, Button::Allow],
        Kind::Ask | Kind::Error | Kind::FailedRoutine => vec![Button::Expand],
    }
}

/// An attention card's icons, top right: snooze, remove, done. A verb whose
/// command is not configured is left out; the live kinds have none.
pub fn icons(item: &Item, can_close: bool, can_snooze: bool) -> Vec<Button> {
    if item.kind != Kind::Attention {
        return Vec::new();
    }
    let mut out = Vec::new();
    if can_snooze {
        out.push(Button::Snooze);
    }
    if can_close {
        out.extend([Button::Dismiss, Button::Done]);
    }
    out
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

fn button_html(button: Button, disabled: bool) -> String {
    button_html_as(button, disabled, button.primary(), button.label())
}

fn button_html_as(button: Button, disabled: bool, primary: bool, label: &str) -> String {
    let icon = if button == Button::Validate { ICON_CHECK } else { "" };
    let tip = "";
    format!(
        "<button type=\"button\" class=\"btn {}\" data-act=\"{}\"{tip}{}>{}<span>{}</span></button>",
        if primary { "acc" } else { "sec" },
        button.act(),
        if disabled { " disabled" } else { "" },
        icon,
        label,
    )
}

fn tag_html(scope: Scope) -> String {
    format!(
        "<span class=\"tag\"><i class=\"{}\"></i>{}</span>",
        if scope == Scope::Perso { "perso" } else { "pro" },
        scope_label(scope)
    )
}

fn source_html(item: &Item, now: u64) -> String {
    let words: Vec<String> = [item.from.trim().to_string(), age(item.since, now)].into_iter().filter(|w| !w.is_empty()).collect();
    format!("<div class=\"src\">{}<span>{}</span></div>", tag_html(item.scope), esc(&words.join(" \u{b7} ")))
}

fn state_html(state: &State) -> String {
    match state {
        State::Idle => String::new(),
        State::Running => "<div class=\"state\">En cours</div>".to_string(),
        State::Sent => "<div class=\"state ok\">Transmis</div>".to_string(),
        State::Failed(words) => format!("<div class=\"state crit\">{}</div>", esc(words)),
    }
}

/// One card.
pub fn card_html(item: &Item, can_act: bool, can_close: bool, can_snooze: bool, now: u64) -> String {
    let running = item.state == State::Running;
    let list = buttons(item, can_act);
    let last = list.len().saturating_sub(1);
    let acts: String = list
        .iter()
        .enumerate()
        .map(|(i, &b)| match b {
            // `Ouvrir` is the card's main button when it stands alone, and
            // reads `Masquer` while the detail shows.
            Button::Expand => button_html_as(b, false, i == last, if item.detail.is_some() { "Masquer" } else { "Ouvrir" }),
            _ => button_html(b, running),
        })
        .collect();
    let acts = format!("<div class=\"acts\">{acts}</div>");
    let icons = icons_html(&icons(item, can_close, can_snooze), running);
    let mut detail = item.detail.as_ref().map(|d| detail_html(d, running)).unwrap_or_default();
    // An opened attention card takes an instruction, action or not, under
    // the draft's actions when it has a draft.
    if item.kind == Kind::Attention && item.detail.is_some() && can_act {
        detail.push_str(&instruct_html(item.instruction.as_ref()));
    }
    let needs = if item.needs.trim().is_empty() {
        String::new()
    } else {
        format!("<div class=\"need\">{ICON_CHEVRON}<span>{}</span></div>", esc(&item.needs))
    };
    let head = format!("<div class=\"t\">{}</div>{needs}", esc(&item.title));
    let body = match (&item.draft, item.kind == Kind::Attention && can_act) {
        (Some(draft), true) => {
            let subject = draft
                .subject
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| format!("<span>Objet <b>{}</b></span>", esc(s)))
                .unwrap_or_default();
            let channel = if draft.channel.trim().is_empty() { String::new() } else { format!("<span>{}</span>", esc(&draft.channel)) };
            let to = if draft.to.trim().is_empty() { String::new() } else { format!("<span>À <b>{}</b></span>", esc(&draft.to)) };
            // The newline after `<textarea>` is the one the parser drops, so
            // a text that starts with a newline keeps it.
            format!(
                "<div class=\"row\"><div class=\"txt\">{head}</div>{icons}</div>\
                 <div class=\"draft\"><div class=\"hd\">{to}{channel}{subject}<span class=\"edit\">{ICON_EDIT}Modifiable</span></div>\
                 <textarea class=\"bd\" spellcheck=\"true\" data-orig=\"{orig}\"{ro}>\n{text}</textarea></div>\
                 <div class=\"draft-foot\">{src}{acts}</div>{detail}{state}",
                orig = esc(&draft.text),
                text = esc(&draft.text),
                ro = if item.state == State::Sent || running { " readonly" } else { "" },
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
        Some(Instructed { text, .. }) => {
            format!("<div class=\"state ok\">Consigne envoyée</div><p class=\"msg sent\">{}</p>", esc(text))
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
            let buttons: String = acts
                .iter()
                .map(|a| format!("<button type=\"button\" class=\"btn sec\" data-act=\"routine\" data-arg=\"{}\"{dis}><span>{}</span></button>", a.word(), a.label()))
                .collect();
            let note = note.as_deref().map(|n| format!("<div class=\"state\">{}</div>", esc(n))).unwrap_or_default();
            format!("<dl class=\"kv\">{kv}</dl>{prompt}<div class=\"dacts\">{buttons}</div>{note}")
        }
        Detail::Permission { text } => format!("<pre class=\"scroll\">{}</pre>", esc(text)),
        Detail::Error { words, lines } => {
            let words = if words.trim().is_empty() { String::new() } else { format!("<p class=\"words\">{}</p>", esc(words)) };
            let lines = if lines.trim().is_empty() { String::new() } else { format!("<pre class=\"scroll\">{}</pre>", esc(lines)) };
            format!("{words}{lines}<div class=\"dacts\"><button type=\"button\" class=\"link\" data-act=\"pane\">Voir le pane</button></div>")
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
            format!("<p class=\"msg\">{}</p>{field}<div class=\"dacts\">{buttons}</div>", esc(message))
        }
        Detail::Entry { needs, body, detail, src, links, explaining } => {
            let para = |class: &str, t: &str| {
                if t.trim().is_empty() { String::new() } else { format!("<p class=\"{class}\">{}</p>", esc(t.trim())) }
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
                let items: String = src.iter().map(|s| format!("<li>{}</li>", esc(s))).collect();
                out.push_str(&format!("<div class=\"lbl\">Sources</div><ul class=\"srcs\">{items}</ul>"));
            }
            if !links.is_empty() {
                let items: String = links
                    .iter()
                    .map(|l| format!("<button type=\"button\" class=\"link\" data-act=\"link\" data-arg=\"{0}\">{0}</button>", esc(l)))
                    .collect();
                out.push_str(&format!("<div class=\"links\">{items}</div>"));
            }
            out
        }
    };
    format!("<div class=\"detail\">{inner}</div>")
}

const SECTIONS: [(u8, &str, &str); 3] = [(1, "Urgent", "crit"), (2, "Aujourd'hui", "warn"), (3, "Cette semaine", "gray")];

/// The cards the filter keeps, under their section headers.
pub fn list_html(view: &View) -> String {
    let kept: Vec<&Item> = view.items.iter().filter(|i| view.filter.keeps(i)).collect();
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

/// The title block, the Chat button, the filter.
pub fn head_html(view: &View) -> String {
    let (lead, tail) = subtitle(view.items);
    let tail = if tail.is_empty() { String::new() } else { format!(" \u{b7} <b>{}</b>", esc(&tail)) };
    let count = |scope: Option<Scope>| view.items.iter().filter(|i| scope.is_none_or(|s| i.scope == s)).count();
    let seg = |filter: Filter, word: &str, label: &str, n: usize| {
        format!(
            "<button type=\"button\" class=\"{}\" data-filter=\"{word}\">{label} <span class=\"c\">{n}</span></button>",
            if view.filter == filter { "on" } else { "" }
        )
    };
    format!(
        "<div class=\"phead\"><div><div class=\"ptitle\">Cockpit</div><div class=\"psub\">{}{tail}</div></div>\
         <div class=\"hbtns\">{}<button type=\"button\" class=\"btn {}\" data-chat=\"toggle\">{ICON_CHAT}<span>Chat</span></button></div></div>\
         {}{}<div class=\"bar\"><div class=\"seg\">{}{}{}</div><span class=\"sortlbl\">{ICON_SORT}Par priorité</span></div>",
        esc(&lead),
        view.record,
        if view.chat.is_some() { "on" } else { "sec" },
        view.record_note,
        view.band,
        seg(Filter::All, "all", "Tout", count(None)),
        seg(Filter::Only(Scope::Pro), "pro", scope_label(Scope::Pro), count(Some(Scope::Pro))),
        seg(Filter::Only(Scope::Perso), "perso", scope_label(Scope::Perso), count(Some(Scope::Perso))),
    )
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
    (head_html(view), list_html(view), view.chat.as_ref().map(chat_html).unwrap_or_default())
}

pub fn update_script(update: &Update) -> String {
    format!("window.kova && window.kova.update({})", serde_json::to_string(update).unwrap_or_else(|_| "{}".into()))
}

fn hex(c: [f32; 4]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// The design system's colours as CSS variables, light or dark.
fn vars(light: bool) -> String {
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
    set.iter().map(|(k, v)| format!("--{k}:{v};")).collect()
}

/// The page, loaded once per open; `update_script` fills it.
pub fn shell(light: bool, nonce: &str) -> String {
    PAGE.replace("{scheme}", if light { "light" } else { "dark" })
        .replace("{vars}", &vars(light))
        .replace("{nonce}", nonce)
        .replace("{handler}", HANDLER)
}

const PAGE: &str = r#"<!doctype html><html lang="fr"><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-{nonce}'">
<title>Cockpit</title>
<style>
:root{color-scheme:{scheme};{vars}}
*{box-sizing:border-box;margin:0;padding:0}
html,body{height:100%;background:var(--bg0)}
body{font-family:Inter,"SF Pro Text",-apple-system,system-ui,sans-serif;color:var(--t1);-webkit-font-smoothing:antialiased;display:flex;overflow:hidden}
svg.i{fill:none;stroke:currentColor;stroke-width:2;stroke-linecap:round;stroke-linejoin:round;flex:none}
button{font:inherit;color:inherit;border:0;background:none;cursor:pointer}
button:disabled{opacity:.5;cursor:default}
:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
.page{flex:1;min-width:0;overflow-y:auto}
.inner{max-width:880px;margin:0 auto;padding:26px 24px 40px}
body.chat-on .inner{max-width:none;margin:0 32px;padding-left:0;padding-right:0}
.phead{display:flex;align-items:flex-start;justify-content:space-between;gap:16px}
.ptitle{font-size:28px;line-height:34px;font-weight:600;letter-spacing:-.2px}
.psub{font-size:13px;color:var(--t3);margin-top:4px}
.psub b{color:var(--crit);font-weight:500}
.btn{height:28px;border-radius:6px;padding:0 12px;font-size:13px;font-weight:500;display:inline-flex;align-items:center;gap:6px;white-space:nowrap;color:var(--t1)}
.btn.acc{background:var(--accent);color:#fff}
.btn.acc:hover:not(:disabled){background:var(--accent-hover)}
.btn.sec{background:var(--btn2)}
.btn.sec:hover:not(:disabled){background:var(--btn2-hover)}
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
.tag i.perso{background:var(--tag-green)}
.acts{display:flex;gap:6px;flex:none}
.draft{margin-top:10px;background:var(--bg-input);border:1px solid var(--b2);border-radius:6px}
.draft:focus-within{border-color:var(--accent)}
.draft .hd{display:flex;align-items:center;gap:14px;padding:8px 12px;border-bottom:1px solid var(--b3);font-size:12px;color:var(--t3);flex-wrap:wrap}
.draft .hd b{font-weight:500;color:var(--t1)}
.draft .hd .edit{margin-left:auto;display:flex;align-items:center;gap:5px}
textarea.bd{display:block;width:100%;resize:none;border:0;outline:0;background:transparent;color:var(--t1);font:inherit;font-size:13px;line-height:19px;padding:8px 12px;overflow:hidden;min-height:35px}
.draft-foot{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-top:8px}
.draft-foot .src{margin:0}
.state{font-size:12px;color:var(--t3);margin-top:6px}
.detail{margin-top:10px;border-top:1px solid var(--b3);padding-top:10px;font-size:13px;line-height:19px}
.kv{display:grid;grid-template-columns:max-content 1fr;gap:4px 14px;margin:0}
.kv dt{color:var(--t3)}.kv dd{margin:0;min-width:0;overflow-wrap:anywhere}.kv dd.crit{color:var(--crit)}
pre.scroll{margin-top:8px;max-height:240px;overflow:auto;background:var(--bg-input);border:1px solid var(--b3);border-radius:6px;padding:8px 12px;font:12px/17px ui-monospace,SFMono-Regular,Menlo,monospace;white-space:pre-wrap;overflow-wrap:anywhere}
.fold{margin-top:8px}.fold summary{cursor:pointer;color:var(--t2);font-size:12px}
.dacts{display:flex;flex-wrap:wrap;gap:6px;margin-top:10px}
.link{color:var(--accent);font-size:13px;font-weight:500;padding:0}
.link:hover{text-decoration:underline}
.words{color:var(--crit)}.msg{white-space:pre-wrap;overflow-wrap:anywhere}
textarea.ans{display:block;width:100%;margin-top:8px;background:var(--bg-input);border:1px solid var(--b2);border-radius:6px;color:var(--t1);font:inherit;font-size:13px;padding:6px 10px;resize:vertical}
textarea.ans:focus{border-color:var(--accent);outline:0}
.state.ok{color:var(--ok-dot)}.state.crit{color:var(--crit)}
.instr{margin-top:10px;border-top:1px solid var(--b3);padding-top:4px;cursor:auto}
textarea.ins{display:block;width:100%;margin-top:4px;background:var(--bg-input);border:1px solid var(--b2);border-radius:6px;color:var(--t1);font:inherit;font-size:13px;line-height:19px;padding:6px 10px;resize:vertical}
textarea.ins:focus{border-color:var(--accent);outline:0}
.msg.sent{margin-top:4px;font-size:12px;color:var(--t2)}
.card{cursor:pointer}.card textarea,.card .detail{cursor:auto}
.side{display:flex;flex-direction:column;align-items:flex-end;gap:6px;flex:none}
.icons{display:flex;gap:2px;flex:none}
.ico{width:24px;height:24px;border-radius:6px;display:flex;align-items:center;justify-content:center;color:var(--t3)}
.ico:hover:not(:disabled){background:var(--hover);color:var(--t1)}
.msg.dim{color:var(--t3)}.msg+.msg{margin-top:8px}.need-t{font-weight:500}
.lbl{margin-top:10px;font-size:12px;color:var(--t3)}
.srcs{margin:4px 0 0 18px;font-size:12px;color:var(--t2);overflow-wrap:anywhere}
.links{display:flex;flex-direction:column;align-items:flex-start;gap:4px;margin-top:10px}
.links .link{text-align:left;overflow-wrap:anywhere}
.more{margin:8px 0 0}
.snz{position:fixed;z-index:10;background:var(--bg-elev);border:1px solid var(--b2);border-radius:8px;padding:4px;min-width:180px;box-shadow:0 8px 24px rgba(0,0,0,.25);display:flex;flex-direction:column}
.snz[hidden]{display:none}
.snz button{height:28px;padding:0 10px;border-radius:6px;text-align:left;font-size:13px}
.snz button:hover{background:var(--hover)}
.snz .pick{display:flex;gap:6px;padding:4px}
.snz input{flex:1;min-width:0;background:var(--bg-input);border:1px solid var(--b2);border-radius:6px;color:var(--t1);font:inherit;font-size:12px;padding:3px 6px}
.chat{width:33.333%;flex:none;border-left:1px solid var(--b3);background:var(--bg1);display:none;flex-direction:column}
body.chat-on .chat{display:flex}
.chat .top{height:56px;flex:none;display:flex;align-items:center;justify-content:space-between;gap:8px;padding:0 14px 0 16px;border-bottom:1px solid var(--b3)}
.chat .who{font-size:11px;color:var(--t3);display:flex;align-items:center;gap:6px;white-space:nowrap;overflow:hidden}
.chat .who i{width:8px;height:8px;border-radius:50%;background:var(--ok-dot);display:block;flex:none}
.chat .who.off i{background:var(--tag-gray)}
.chat .x{color:var(--t3);width:26px;height:26px;display:flex;align-items:center;justify-content:center;border-radius:6px}
.chat .x:hover{background:var(--hover)}
.cbody{flex:1;padding:20px}
.hbtns{display:flex;gap:8px;flex:none}
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
<section class="page"><div class="inner"><div id="head"></div><div id="list"></div></div></section>
<section class="chat" id="chat"></section>
<div class="snz" id="snz" hidden><button type="button" data-when="hour">Dans 1 h</button><button type="button" data-when="evening">Ce soir</button><button type="button" data-when="morning">Demain matin</button><button type="button" data-when="monday">Lundi prochain</button><button type="button" data-when="pick">Choisir…</button><div class="pick" id="snzPick" hidden><input type="datetime-local" id="snzAt"><button type="button" class="btn acc" data-when="at"><span>OK</span></button></div></div>
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
  const c = card.querySelector('[data-act=correct] span');
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
      ins: (card.querySelector('textarea.ins') || {}).value || ''
    };
  });
  const ansFocus = a && a.classList && a.classList.contains('ans') ? a.closest('.card').dataset.key : null;
  const insFocus = a && a.classList && a.classList.contains('ins') ? a.closest('.card').dataset.key : null;
  const insSel = insFocus ? [a.selectionStart, a.selectionEnd] : null;
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
  });
  document.querySelector('.page').scrollTop = top;
}
window.kova = {
  update(u) {
    if (u.head !== undefined) $('head').innerHTML = u.head;
    if (u.list !== undefined) setList(u.list);
    if (u.chat !== undefined) { $('chat').innerHTML = u.chat; document.body.classList.toggle('chat-on', u.chat !== ''); }
    if (u.focus && !(document.activeElement && document.activeElement.tagName === 'TEXTAREA')) window.focus();
    if (u.reveal) {
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
function pad(n) { return String(n).padStart(2, '0'); }
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
}
function snzClose() { $('snz').hidden = true; $('snzPick').hidden = true; snzKey = null; }
function hide(key) { const c = document.querySelector('.card[data-key="' + CSS.escape(key) + '"]'); if (c) c.style.display = 'none'; }
document.addEventListener('click', e => {
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
  const b = e.target.closest('button');
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
    if (!card || e.target.closest('textarea, input, a, .detail, .instr')) return;
    if (String(window.getSelection()).trim()) return;
    return post({ type: 'act', key: card.dataset.key, act: 'expand' });
  }
  if (b.disabled) return;
  if (b.dataset.more) return post({ type: 'more' });
  if (b.dataset.filter) return post({ type: 'filter', scope: b.dataset.filter });
  if (b.dataset.chief) return post({ type: 'chief', scope: b.dataset.chief });
  if (b.dataset.chat) return post({ type: 'chat', what: b.dataset.chat });
  const card = b.closest('.card');
  if (!card || !b.dataset.act) return;
  const t = card.querySelector('textarea.bd');
  let act = b.dataset.act;
  if (act === 'correct' && t) {
    if (edited(t)) { t.value = t.dataset.orig; grow(t); sync(card); }
    t.focus();
    return;
  }
  const msg = { type: 'act', key: card.dataset.key, act };
  if (b.dataset.arg) msg.arg = b.dataset.arg;
  if (act === 'answer' && b.dataset.text) {
    const ans = card.querySelector('textarea.ans');
    msg.text = ans ? ans.value : '';
    if (!msg.text.trim()) { if (ans) ans.focus(); return; }
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
  if (act === 'validate' && t && edited(t)) { msg.act = 'correct'; msg.text = t.value; }
  card.querySelectorAll('.acts button, .dacts button').forEach(x => x.disabled = true);
  post(msg);
});
document.addEventListener('input', e => {
  const t = e.target;
  if (t.classList && t.classList.contains('bd')) { grow(t); sync(t.closest('.card')); }
});
document.addEventListener('keydown', e => {
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
  if (e.key !== 'Escape') return;
  e.preventDefault();
  if (!$('snz').hidden) return snzClose();
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
        }
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
    }

    #[test]
    fn a_live_card_opens_in_place_and_says_masquer_while_open() {
        let mut item = from_need(&need(NeedKind::FailedRoutine, "routine:nightly", 5), false).expect("item");
        let closed = card_html(&item, true, true, true, 10);
        assert!(closed.contains("data-act=\"expand\"") && closed.contains("Ouvrir") && !closed.contains("class=\"detail\""));
        // Alone, `Ouvrir` is the card's main button.
        assert!(closed.contains("btn acc\" data-act=\"expand\""));
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
        assert!(open.contains("Masquer"));
        assert!(open.contains("<dd class=\"crit\">Failed · exit 2</dd>"));
        assert!(open.contains("Do &lt;this&gt;") && open.contains("<details class=\"fold\">"));
        for word in ["run", "log", "pause", "edit"] {
            assert!(open.contains(&format!("data-act=\"routine\" data-arg=\"{word}\"")), "{word}");
            assert_eq!(RoutineAct::parse(word).map(|a| a.word()), Some(word));
        }
        // Next to Refuser / Autoriser, `Ouvrir` is a secondary button.
        let perm = from_need(&need(NeedKind::Permission, "pane:1", 5), false).expect("item");
        assert!(card_html(&perm, true, true, true, 10).contains("btn sec\" data-act=\"expand\""));
    }

    #[test]
    fn error_permission_and_question_details_are_escaped_and_answerable() {
        let error = detail_html(&Detail::Error { words: "<limit>".into(), lines: "a\n<b>".into() }, false);
        assert!(error.contains("&lt;limit&gt;") && error.contains("a\n&lt;b&gt;") && error.contains("data-act=\"pane\""));
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
        assert_eq!(links, ["~/notes/plan.md", "https://linear.app/x/CLA-1", "https://a.b/c"]);
        let html = detail_html(&entry_detail(&e, false), false);
        assert!(html.contains("Choisir &lt;vite&gt;") && html.contains("Ligne 1\nVoir"));
        assert!(html.contains("<li>s-42</li>"));
        assert!(html.contains("data-act=\"link\" data-arg=\"https://linear.app/x/CLA-1\""));
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
        assert_eq!(links_in("x http://"), Vec::<String>::new());
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
        let view = |limit| View { items: &items, filter: Filter::All, chat: None, can_act: true, can_close: true, can_snooze: true, limit, now: 100, band: String::new(), record: String::new(), record_note: String::new() };
        let html = list_html(&view(PAGE_SIZE));
        assert_eq!(html.matches("<article").count(), 10);
        assert!(html.contains("data-more=\"1\"><span>Voir plus (13)</span>"));
        assert!(html.contains("Aujourd'hui <span class=\"k\">19</span>"), "a section counts all its cards");
        assert!(head_html(&view(PAGE_SIZE)).contains("Tout <span class=\"c\">23</span>"));
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
        assert_eq!(perm.title, "build attend une permission");
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

    #[test]
    fn buttons_follow_the_kind() {
        let mut item = from_attention(&entry("a-1", Level::Now, None, 1), None, true).unwrap();
        assert_eq!(buttons(&item, true), [Button::Open]);
        // `C'est fait` is an icon now, with snooze and remove.
        assert_eq!(icons(&item, true, true), [Button::Snooze, Button::Dismiss, Button::Done]);
        assert_eq!(icons(&item, true, false), [Button::Dismiss, Button::Done]);
        assert_eq!(icons(&item, false, false), []);
        item.openable = false;
        assert_eq!(buttons(&item, true), []);
        item.draft = Some(Draft { to: "A".into(), channel: "Email".into(), subject: None, text: "Hi".into() });
        assert_eq!(buttons(&item, true), [Button::Correct, Button::Validate]);
        item.state = State::Sent;
        assert_eq!(buttons(&item, true), []);
        let sheet = from_need(&need(NeedKind::Ask, "ask:q", 1), true).unwrap();
        // A pending sheet is answered on the sheet, never marked done here.
        assert_eq!(buttons(&sheet, true), [Button::OpenSheet]);
        let perm = from_need(&need(NeedKind::Permission, "pane:1", 1), false).unwrap();
        assert_eq!(buttons(&perm, true), [Button::Expand, Button::Deny, Button::Allow]);
        let err = from_need(&need(NeedKind::Error, "pane:1", 1), false).unwrap();
        assert_eq!(buttons(&err, true), [Button::Expand]);
        assert!(icons(&perm, true, true).is_empty(), "a live need keeps its own buttons");
        for b in [Button::Snooze, Button::Dismiss, Button::Expand, Button::Done, Button::Deny, Button::Allow, Button::Correct, Button::Validate, Button::Open] {
            assert_eq!(Button::parse(b.act()), Some(b));
        }
    }

    #[test]
    fn the_filter_keeps_one_scope_and_the_counts_cover_every_scope() {
        let mut perso = entry("a-2", Level::Now, None, 2);
        perso.scope = Scope::Perso;
        let items = vec![
            from_attention(&entry("a-1", Level::Now, None, 1), None, false).unwrap(),
            from_attention(&perso, None, false).unwrap(),
        ];
        let view = |filter| View { items: &items, filter, chat: None, can_act: true, can_close: true, can_snooze: true, limit: PAGE_SIZE, now: 100, band: String::new(), record: String::new(), record_note: String::new() };
        assert!(list_html(&view(Filter::All)).contains("attention:a-1"));
        let only = list_html(&view(Filter::Only(Scope::Perso)));
        assert!(only.contains("attention:a-2") && !only.contains("attention:a-1"));
        let head = head_html(&view(Filter::Only(Scope::Perso)));
        assert!(head.contains("Tout <span class=\"c\">2</span>"));
        assert!(head.contains("class=\"on\" data-filter=\"perso\""));
        assert_eq!(Filter::parse("pro"), Some(Filter::Only(Scope::Pro)));
        assert_eq!(Filter::parse("nope"), None);
    }

    #[test]
    fn sections_and_subtitle_name_the_priorities() {
        let items = vec![
            from_need(&need(NeedKind::Permission, "pane:1", 1), false).unwrap(),
            from_attention(&entry("a-1", Level::Point, None, 1), None, false).unwrap(),
        ];
        let html = list_html(&View { items: &items, filter: Filter::All, chat: None, can_act: true, can_close: true, can_snooze: true, limit: PAGE_SIZE, now: 100, band: String::new(), record: String::new(), record_note: String::new() });
        assert!(html.contains("Urgent <span class=\"k\">1</span>"));
        assert!(html.contains("Cette semaine <span class=\"k\">1</span>"));
        assert!(!html.contains("Aujourd'hui"));
        assert_eq!(subtitle(&items), ("2 actions pour toi".to_string(), "1 urgente".to_string()));
        assert_eq!(subtitle(&[]).0, "Aucune action pour toi");
        assert!(list_html(&View { items: &[], filter: Filter::All, chat: None, can_act: true, can_close: true, can_snooze: true, limit: PAGE_SIZE, now: 0, band: String::new(), record: String::new(), record_note: String::new() }).contains("Rien ne t'attend"));
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
        let bare = Entry { needs: None, ..entry("a-1", Level::Point, None, 1) };
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
        assert!(html.contains("Consigne envoyée</div><p class=\"msg sent\">Relance &lt;Fidsud&gt; &quot;vite&quot;\nmerci</p>"), "{html}");
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
        e.draft = Some(Draft { to: "A".into(), channel: "Email".into(), subject: None, text: "Hi".into() });
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
        e.draft = Some(Draft { to: "A".into(), channel: "Email".into(), subject: None, text: "Hi".into() });
        let sent = from_attention(&e, Some(&Outcome::Sent), false).unwrap();
        let html = card_html(&sent, true, true, true, 10);
        assert!(html.contains("Transmis") && html.contains("readonly") && !html.contains("data-act=\"validate\""));
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
        assert!(!PAGE.contains('\u{2014}'), "no em dash in the UI");
        let u = Update { list: Some("<p>x</p>".into()), ..Default::default() };
        assert_eq!(update_script(&u), "window.kova && window.kova.update({\"list\":\"<p>x</p>\"})");
        assert!(Update::default().is_empty());
    }

    #[test]
    fn the_catch_up_band_sits_between_the_title_and_the_filter() {
        let items: Vec<Item> = Vec::new();
        let view = |band: &str| View {
            items: &items, filter: Filter::All, chat: None, can_act: true, can_close: true, can_snooze: true,
            limit: PAGE_SIZE, now: 0, band: band.to_string(), record: "<button data-rec=\"start\">R</button>".into(),
            record_note: "<div class=\"recn\">N</div>".into(),
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
