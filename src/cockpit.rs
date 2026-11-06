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

use crate::attention::{Draft, Entry, Level, Outcome, Scope};
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
    pub state: State,
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
        state,
    })
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
        state: State::Idle,
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
    Open,
    OpenSheet,
    Done,
    Deny,
    Allow,
    Correct,
    Validate,
}

impl Button {
    /// The word the page posts.
    pub fn act(self) -> &'static str {
        match self {
            Button::Open | Button::OpenSheet => "open",
            Button::Done => "done",
            Button::Deny => "deny",
            Button::Allow => "allow",
            Button::Correct => "correct",
            Button::Validate => "validate",
        }
    }

    pub fn parse(word: &str) -> Option<Button> {
        match word {
            "open" => Some(Button::Open),
            "done" => Some(Button::Done),
            "deny" => Some(Button::Deny),
            "allow" => Some(Button::Allow),
            "correct" => Some(Button::Correct),
            "validate" => Some(Button::Validate),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Button::Open => "Ouvrir",
            Button::OpenSheet => "Ouvrir la fiche",
            Button::Done => "C'est fait",
            Button::Deny => "Refuser",
            Button::Allow => "Autoriser",
            Button::Correct => "Corriger",
            Button::Validate => "Valider",
        }
    }

    fn primary(self) -> bool {
        matches!(self, Button::Open | Button::OpenSheet | Button::Allow | Button::Validate)
    }
}

/// A card's buttons, left to right. A verb whose command is not configured
/// is left out; a sent draft has none left.
pub fn buttons(item: &Item, can_act: bool, can_close: bool) -> Vec<Button> {
    match item.kind {
        Kind::Attention if item.draft.is_some() && can_act => {
            if item.state == State::Sent {
                Vec::new()
            } else {
                vec![Button::Correct, Button::Validate]
            }
        }
        Kind::Attention => {
            let mut out = Vec::new();
            if can_close {
                out.push(Button::Done);
            }
            if item.openable {
                out.push(Button::Open);
            }
            out
        }
        Kind::Sheet => vec![Button::OpenSheet],
        Kind::Permission => vec![Button::Deny, Button::Allow],
        Kind::Ask | Kind::Error | Kind::FailedRoutine => vec![Button::Open],
    }
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
    pub now: u64,
}

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

fn button_html(button: Button, disabled: bool) -> String {
    let icon = if button == Button::Validate { ICON_CHECK } else { "" };
    format!(
        "<button type=\"button\" class=\"btn {}\" data-act=\"{}\"{}>{}<span>{}</span></button>",
        if button.primary() { "acc" } else { "sec" },
        button.act(),
        if disabled { " disabled" } else { "" },
        icon,
        button.label(),
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
pub fn card_html(item: &Item, can_act: bool, can_close: bool, now: u64) -> String {
    let running = item.state == State::Running;
    let acts: String = buttons(item, can_act, can_close).into_iter().map(|b| button_html(b, running)).collect();
    let acts = format!("<div class=\"acts\">{acts}</div>");
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
                "<div class=\"row\"><div class=\"txt\">{head}</div></div>\
                 <div class=\"draft\"><div class=\"hd\">{to}{channel}{subject}<span class=\"edit\">{ICON_EDIT}Modifiable</span></div>\
                 <textarea class=\"bd\" spellcheck=\"true\" data-orig=\"{orig}\"{ro}>\n{text}</textarea></div>\
                 <div class=\"draft-foot\">{src}{acts}</div>{state}",
                orig = esc(&draft.text),
                text = esc(&draft.text),
                ro = if item.state == State::Sent || running { " readonly" } else { "" },
                src = source_html(item, now),
                state = state_html(&item.state),
            )
        }
        _ => format!(
            "<div class=\"row\"><div class=\"txt\">{head}{src}</div>{acts}</div>{state}",
            src = source_html(item, now),
            state = state_html(&item.state),
        ),
    };
    format!(
        "<article class=\"card{}\" data-key=\"{}\">{body}</article>",
        if item.prio == 1 { " urgent" } else { "" },
        esc(&item.key),
    )
}

const SECTIONS: [(u8, &str, &str); 3] = [(1, "Urgent", "crit"), (2, "Aujourd'hui", "warn"), (3, "Cette semaine", "gray")];

/// The cards the filter keeps, under their section headers.
pub fn list_html(view: &View) -> String {
    let kept: Vec<&Item> = view.items.iter().filter(|i| view.filter.keeps(i)).collect();
    if kept.is_empty() {
        return "<p class=\"empty\">Rien ne t'attend ici</p>".to_string();
    }
    let mut out = String::new();
    for (prio, name, dot) in SECTIONS {
        let cards: Vec<&&Item> = kept.iter().filter(|i| i.prio.clamp(1, 3) == prio).collect();
        if cards.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "<div class=\"sec-h\"><span class=\"d {dot}\"></span>{name} <span class=\"k\">{}</span></div>",
            cards.len()
        ));
        for item in cards {
            out.push_str(&card_html(item, view.can_act, view.can_close, view.now));
        }
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
         <button type=\"button\" class=\"btn {}\" data-chat=\"toggle\">{ICON_CHAT}<span>Chat</span></button></div>\
         <div class=\"bar\"><div class=\"seg\">{}{}{}</div><span class=\"sortlbl\">{ICON_SORT}Par priorité</span></div>",
        esc(&lead),
        if view.chat.is_some() { "on" } else { "sec" },
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
}

impl Update {
    pub fn is_empty(&self) -> bool {
        self.head.is_none() && self.list.is_none() && self.chat.is_none() && !self.focus
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
.state.ok{color:var(--ok-dot)}.state.crit{color:var(--crit)}
.chat{width:33.333%;flex:none;border-left:1px solid var(--b3);background:var(--bg1);display:none;flex-direction:column}
body.chat-on .chat{display:flex}
.chat .top{height:56px;flex:none;display:flex;align-items:center;justify-content:space-between;gap:8px;padding:0 14px 0 16px;border-bottom:1px solid var(--b3)}
.chat .who{font-size:11px;color:var(--t3);display:flex;align-items:center;gap:6px;white-space:nowrap;overflow:hidden}
.chat .who i{width:8px;height:8px;border-radius:50%;background:var(--ok-dot);display:block;flex:none}
.chat .who.off i{background:var(--tag-gray)}
.chat .x{color:var(--t3);width:26px;height:26px;display:flex;align-items:center;justify-content:center;border-radius:6px}
.chat .x:hover{background:var(--hover)}
.cbody{flex:1;padding:20px}
.none{font-size:13px;color:var(--t3)}
</style></head><body>
<section class="page"><div class="inner"><div id="head"></div><div id="list"></div></div></section>
<section class="chat" id="chat"></section>
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
  const focusKey = a && a.tagName === 'TEXTAREA' ? a.closest('.card').dataset.key : null;
  const sel = focusKey ? [a.selectionStart, a.selectionEnd] : null;
  const top = document.querySelector('.page').scrollTop;
  $('list').innerHTML = html;
  document.querySelectorAll('.card').forEach(card => {
    const t = card.querySelector('textarea.bd');
    if (!t) return;
    const k = keep[card.dataset.key];
    if (k && k.orig === t.dataset.orig && !t.readOnly) t.value = k.value;
    grow(t); sync(card);
    if (card.dataset.key === focusKey && !t.readOnly) { t.focus(); t.setSelectionRange(sel[0], sel[1]); }
  });
  document.querySelector('.page').scrollTop = top;
}
window.kova = {
  update(u) {
    if (u.head !== undefined) $('head').innerHTML = u.head;
    if (u.list !== undefined) setList(u.list);
    if (u.chat !== undefined) { $('chat').innerHTML = u.chat; document.body.classList.toggle('chat-on', u.chat !== ''); }
    if (u.focus && !(document.activeElement && document.activeElement.tagName === 'TEXTAREA')) window.focus();
  }
};
document.addEventListener('click', e => {
  const b = e.target.closest('button');
  if (!b || b.disabled) return;
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
  if (act === 'validate' && t && edited(t)) { msg.act = 'correct'; msg.text = t.value; }
  card.querySelectorAll('.acts button').forEach(x => x.disabled = true);
  post(msg);
});
document.addEventListener('input', e => {
  const t = e.target;
  if (t.classList && t.classList.contains('bd')) { grow(t); sync(t.closest('.card')); }
});
document.addEventListener('keydown', e => {
  if (e.key !== 'Escape') return;
  e.preventDefault();
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
        }
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
        assert_eq!(buttons(&item, true, true), [Button::Done, Button::Open]);
        item.openable = false;
        assert_eq!(buttons(&item, true, true), [Button::Done]);
        assert_eq!(buttons(&item, true, false), []);
        item.draft = Some(Draft { to: "A".into(), channel: "Email".into(), subject: None, text: "Hi".into() });
        assert_eq!(buttons(&item, true, true), [Button::Correct, Button::Validate]);
        item.state = State::Sent;
        assert_eq!(buttons(&item, true, true), []);
        let sheet = from_need(&need(NeedKind::Ask, "ask:q", 1), true).unwrap();
        // A pending sheet is answered on the sheet, never marked done here.
        assert_eq!(buttons(&sheet, true, true), [Button::OpenSheet]);
        let perm = from_need(&need(NeedKind::Permission, "pane:1", 1), false).unwrap();
        assert_eq!(buttons(&perm, true, true), [Button::Deny, Button::Allow]);
        let err = from_need(&need(NeedKind::Error, "pane:1", 1), false).unwrap();
        assert_eq!(buttons(&err, true, true), [Button::Open]);
        for b in [Button::Done, Button::Deny, Button::Allow, Button::Correct, Button::Validate, Button::Open] {
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
        let view = |filter| View { items: &items, filter, chat: None, can_act: true, can_close: true, now: 100 };
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
        let html = list_html(&View { items: &items, filter: Filter::All, chat: None, can_act: true, can_close: true, now: 100 });
        assert!(html.contains("Urgent <span class=\"k\">1</span>"));
        assert!(html.contains("Cette semaine <span class=\"k\">1</span>"));
        assert!(!html.contains("Aujourd'hui"));
        assert_eq!(subtitle(&items), ("2 actions pour toi".to_string(), "1 urgente".to_string()));
        assert_eq!(subtitle(&[]).0, "Aucune action pour toi");
        assert!(list_html(&View { items: &[], filter: Filter::All, chat: None, can_act: true, can_close: true, now: 0 }).contains("Rien ne t'attend"));
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
        let html = card_html(&item, true, true, 10);
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
        assert!(!card_html(&item, false, true, 10).contains("<textarea"));
    }

    #[test]
    fn a_sent_draft_says_so_and_a_running_click_disables_the_buttons() {
        let mut e = entry("a-1", Level::Now, None, 1);
        e.draft = Some(Draft { to: "A".into(), channel: "Email".into(), subject: None, text: "Hi".into() });
        let sent = from_attention(&e, Some(&Outcome::Sent), false).unwrap();
        let html = card_html(&sent, true, true, 10);
        assert!(html.contains("Transmis") && html.contains("readonly") && !html.contains("data-act"));
        let running = from_attention(&e, Some(&Outcome::Running), false).unwrap();
        assert!(card_html(&running, true, true, 10).contains("data-act=\"validate\" disabled"));
        let failed = from_attention(&e, Some(&Outcome::Failed { words: "<boom>".into(), until_ms: 0 }), false).unwrap();
        assert!(card_html(&failed, true, true, 10).contains("&lt;boom&gt;"));
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
}
